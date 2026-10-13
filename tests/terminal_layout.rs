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
            "Summary: 1 error, 0 warnings, 0 off (1 finding total)\n",
            "Tests analyzed: 2\nFiles analyzed: 1\n"
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
