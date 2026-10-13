//! The real CLI preserves analysis results while using the spaced terminal layout.
use assert_cmd::Command;
use serde_json::Value;

/// A minimal owned input shared by layout and stream-policy regressions.
fn duplicate_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("sample.test.ts"),
        "import { test, expect } from 'vitest';\n\
         test('first total', () => { expect(1 + 1).toBe(2); });\n\
         test('second total', () => { expect(1 + 1).toBe(2); });\n",
    )
    .unwrap();
    dir
}

/// Assert counts from the machine report before checking the human presentation of that run.
#[test]
fn terminal_layout_preserves_duplicate_outcome() {
    let dir = duplicate_project();
    let prefix = dir.path().join("report");
    let run = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(dir.path())
        .args(["--reporters", "terminal,json", "--output"])
        .arg(&prefix)
        .output()
        .unwrap();
    assert_eq!(run.status.code(), Some(1));
    let report: Value =
        serde_json::from_slice(&std::fs::read(prefix.with_extension("json")).unwrap()).unwrap();
    assert_eq!(report["testsAnalyzed"], 2);
    assert_eq!(report["filesAnalyzed"], 1);
    let findings = report["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["rule"], "duplicate-test");
    assert_eq!(findings[0]["locations"].as_array().unwrap().len(), 2);
    assert_eq!(
        String::from_utf8(run.stdout).unwrap(),
        concat!(
            "VitestDedup v",
            env!("CARGO_PKG_VERSION"),
            "\n\n",
            "duplicate-test\n\n",
            "  [error] 2 tests have identical bodies under an equivalent context\n",
            "    --> sample.test.ts:2:1  first total\n",
            "    related: sample.test.ts:3:1  second total\n",
            "    action: Review whether to remove a redundant test or repair a test whose body misses its named behavior.\n\n",
            "Analyzed 2 tests in 1 file: 1 error, 0 warnings, 0 off; 0 suppressed\n",
            "Error-level duplication: 2 unique finding locations / 2 analyzed tests (100.00%), threshold 0.00% — FAIL\n",
            "Analysis policy: FAIL\n\n",
            "Found 1 finding.\n"
        )
    );
}

/// Captured stdout is a pipe: NO_COLOR cannot force styles into it or a machine report.
#[test]
fn piped_reports_stay_plain_for_every_no_color_setting() {
    let dir = duplicate_project();
    let prefix = dir.path().join("report");
    let mut expected_json = None;
    for value in [None, Some(""), Some("1")] {
        for reporters in ["terminal,json", "jsonl,json"] {
            let mut cmd = Command::cargo_bin("vitest-dedup").unwrap();
            cmd.env_remove("NO_COLOR");
            if let Some(value) = value {
                cmd.env("NO_COLOR", value);
            }
            let run = cmd
                .arg(dir.path())
                .args(["--reporters", reporters, "--output"])
                .arg(&prefix)
                .output()
                .unwrap();
            assert_eq!(run.status.code(), Some(1));
            assert!(!run.stdout.contains(&0x1b));
            assert!(!run.stderr.contains(&0x1b));
            let json = std::fs::read(prefix.with_extension("json")).unwrap();
            let report: Value = serde_json::from_slice(&json).unwrap();
            assert_eq!(report["testsAnalyzed"], 2);
            assert_eq!(report["findings"].as_array().unwrap().len(), 1);
            assert!(!json.contains(&0x1b));
            assert_eq!(expected_json.get_or_insert(json.clone()), &json);
            if reporters.starts_with("jsonl") {
                let stdout = String::from_utf8(run.stdout).unwrap();
                let records: Vec<Value> = stdout
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                assert_eq!(records.len(), 2);
                assert_eq!(records[0]["type"], "finding");
                assert_eq!(records[1]["type"], "summary");
            }
        }
    }
}

/// Exercise the real CLI and assert the structured finding count before human policy labels.
fn policy_report(dir: &std::path::Path, flags: &[&str]) -> (std::process::Output, Value) {
    let prefix = dir.join("report");
    let run = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(dir)
        .args(["--reporters", "terminal,json", "--output"])
        .arg(&prefix)
        .args(flags)
        .output()
        .unwrap();
    let report: Value =
        serde_json::from_slice(&std::fs::read(prefix.with_extension("json")).unwrap()).unwrap();
    (run, report)
}

/// A threshold pass is not a fail-on-new pass, and accepted debt stays visible.
#[test]
fn policy_footer_matches_threshold_new_limit_and_baseline() {
    let dir = duplicate_project();
    let baseline = dir.path().join("baseline.json");
    vitest_dedup::modes::write_baseline(&baseline, &[]).unwrap();
    let path = baseline.to_str().unwrap();
    for (flags, exit, threshold, policy) in [
        (vec!["--threshold", "99"], 1, "99.00% — FAIL", "FAIL"),
        (vec!["--threshold", "100"], 0, "100.00% — PASS", "PASS"),
        (
            vec![
                "--threshold",
                "100",
                "--baseline",
                path,
                "--fail-on-new",
                "0",
            ],
            1,
            "100.00% — PASS",
            "FAIL",
        ),
        (
            vec!["--rule", "duplicate-test=warning"],
            0,
            "0.00% — PASS",
            "PASS",
        ),
    ] {
        let (run, report) = policy_report(dir.path(), &flags);
        assert_eq!(report["testsAnalyzed"], 2);
        assert_eq!(report["findings"].as_array().unwrap().len(), 1);
        assert_eq!(run.status.code(), Some(exit));
        let text = String::from_utf8(run.stdout).unwrap();
        assert!(text.contains(threshold), "{text}");
        assert!(
            text.contains(&format!("Analysis policy: {policy}")),
            "{text}"
        );
    }
    policy_report(dir.path(), &["--baseline", path, "--update-baseline"]);
    let (run, report) = policy_report(dir.path(), &["--baseline", path]);
    assert_eq!(report["findings"].as_array().unwrap().len(), 1);
    assert_eq!(report["findings"][0]["baselined"], true);
    assert_eq!(run.status.code(), Some(0));
    let text = String::from_utf8(run.stdout).unwrap();
    assert!(text.contains("1 error, 0 warnings, 0 off"));
    assert!(text.contains("Baseline: 1 accepted"));
    assert!(text.contains("0 unique finding locations / 2 analyzed tests (0.00%)"));
}

/// The exact incomplete class, not a generic warning, controls the policy decision.
#[test]
fn policy_footer_respects_incomplete_classes() {
    let dir = duplicate_project();
    std::fs::write(dir.path().join("unreadable.test.ts"), [0xff]).unwrap();
    for (class, exit, policy) in [
        (None, 0, "PASS"),
        (Some("any"), 2, "UNAVAILABLE"),
        (Some("reads"), 2, "UNAVAILABLE"),
        (Some("limits"), 0, "PASS"),
    ] {
        let flag = class.map(|class| format!("--fail-on-incomplete={class}"));
        let mut flags = vec!["--threshold", "100"];
        if let Some(flag) = &flag {
            flags.push(flag);
        }
        let (run, report) = policy_report(dir.path(), &flags);
        assert_eq!(report["testsAnalyzed"], 2);
        assert_eq!(report["findings"].as_array().unwrap().len(), 1);
        assert_eq!(report["incomplete"], true);
        assert_eq!(run.status.code(), Some(exit));
        let text = String::from_utf8(run.stdout).unwrap();
        assert!(text.contains("threshold 100.00% — PASS"));
        assert!(text.contains(&format!("Analysis policy: {policy} (incomplete coverage)")));
    }
}

/// Zero-work does not imply success, including when suites still carry real quality findings.
#[test]
fn zero_work_footer_preserves_suite_quality_failures() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("suite.test.ts");
    for (content, count, exit, policy) in [
        ("import {describe} from 'vitest';", 0, 2, "UNAVAILABLE"),
        ("import {describe} from 'vitest'; describe('copy',()=>{const v=setup()}); describe('copy',()=>{const v=setup()});", 1, 1, "FAIL"),
    ] {
        std::fs::write(&source, content).unwrap();
        let (run, report) = policy_report(dir.path(), &["--rule", "duplicate-suite=error"]);
        assert_eq!(report["testsAnalyzed"], 0);
        assert_eq!(report["findings"].as_array().unwrap().len(), count);
        assert_eq!(run.status.code(), Some(exit));
        let text = String::from_utf8(run.stdout).unwrap();
        assert!(text.contains("Error-level duplication: unavailable (no executable tests)"));
        assert!(text.contains(&format!("Analysis policy: {policy}")));
        assert!(!text.contains("% — PASS"));
    }
}

/// Analysis policy is distinct from a later report-writing operational failure.
#[test]
fn report_io_failure_still_exits_operationally_after_analysis_passes() {
    let dir = duplicate_project();
    std::fs::write(dir.path().join("blocked"), "file, not directory").unwrap();
    let run = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(dir.path())
        .args([
            "--threshold",
            "100",
            "--reporters",
            "terminal,json",
            "--output",
        ])
        .arg(dir.path().join("blocked/report"))
        .output()
        .unwrap();
    assert_eq!(run.status.code(), Some(2));
    assert!(String::from_utf8(run.stdout)
        .unwrap()
        .contains("Analysis policy: PASS"));
    assert!(String::from_utf8(run.stderr)
        .unwrap()
        .contains("creating report directory"));
}
