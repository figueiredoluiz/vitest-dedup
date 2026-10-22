//! CLI presentation and option-combination contracts.
use std::process::{Command, Stdio};
use vitest_dedup::cli::ParsedArgs;

/// Either baseline source permits a new-finding limit; incompatible modes still fail.
#[test]
fn fail_on_new_accepts_either_baseline_source() {
    for source in ["--baseline", "--baseline-from-ref"] {
        assert!(
            ParsedArgs::parse_from(["vitest-dedup", source, "base", "--fail-on-new", "0"]).is_ok()
        );
    }
    for args in [
        vec!["--fail-on-new", "0"],
        vec!["--baseline", "base", "--baseline-from-ref", "HEAD"],
        vec![
            "--baseline",
            "base",
            "--update-baseline",
            "--fail-on-new",
            "0",
        ],
        vec!["--baseline-from-ref", "HEAD", "--update-baseline"],
    ] {
        assert!(ParsedArgs::parse_from(std::iter::once("vitest-dedup").chain(args)).is_err());
    }
}

/// Explicit output needs a file reporter and a filename, while defaults remain accepted.
#[test]
fn output_rejects_unused_or_directory_prefixes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    for args in [
        vec!["--output", "report"],
        vec!["--reporters", "jsonl", "--output", "report"],
        vec!["--reporters", "json", "--output", "reports/"],
        vec!["--reporters", "json", "--output", "missing/."],
        vec!["--reporters", "json", "--output", path],
    ] {
        assert!(ParsedArgs::parse_from(std::iter::once("vitest-dedup").chain(args)).is_err());
    }
    for args in [
        vec![],
        vec!["--reporters", "json"],
        vec!["--reporters", "jsonl,json", "--output", "nested/report"],
    ] {
        assert!(ParsedArgs::parse_from(std::iter::once("vitest-dedup").chain(args)).is_ok());
    }
}

/// Trusted usage lines remain readable while hostile argument content cannot forge lines.
#[test]
fn clap_errors_preserve_usage_without_hostile_layout() {
    for args in [
        vec!["--bogus\nFORGED\u{1b}\u{202e}"],
        vec!["--threshold", "bad\nFORGED\u{1b}\u{202e}"],
        vec!["--rule", "duplicate-test=bad\nFORGED\u{1b}\u{202e}"],
    ] {
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("\n\n"), "{stderr}");
        if stderr.contains("unexpected argument") {
            assert!(stderr.contains("\nUsage: "), "{stderr}");
        }
        assert!(
            !stderr.contains("\nFORGED")
                && !stderr.contains('\u{1b}')
                && !stderr.contains('\u{202e}'),
            "{stderr}"
        );
        assert!(stderr.contains("For more information"));
    }
}

/// A closed stdout consumer cannot suppress file reports or change the computed gate exit.
#[test]
fn closed_stdout_preserves_file_reports_and_quality_gate() {
    let dir = tempfile::tempdir().unwrap();
    let source = "import { test, expect } from 'vitest'; test('a',()=>expect(1).toBe(1)); test('b',()=>expect(1).toBe(1));";
    std::fs::write(dir.path().join("a.test.ts"), source).unwrap();
    for (threshold, expected) in [("100", 0), ("0", 1)] {
        let mut child = Command::new(assert_cmd::cargo::cargo_bin("vitest-dedup"))
            .current_dir(dir.path())
            .arg(dir.path())
            .args(["--reporters", "jsonl,json", "--threshold", threshold])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdout.take());
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("reports/vitest-dedup.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(report["testsAnalyzed"], 2);
        assert!(!report["findings"].as_array().unwrap().is_empty());
    }
}

/// Config paths use the analysis root and automatic inheritance crosses a Git boundary.
#[test]
fn config_resolution_retains_analysis_root_and_upward_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(&root)
        .output()
        .unwrap()
        .status
        .success());
    std::fs::write(dir.path().join(".vitest-dedup.json"), r#"{"threshold":73}"#).unwrap();
    std::fs::write(root.join("explicit.json"), r#"{"threshold":81}"#).unwrap();
    let auto = vitest_dedup::config::Config::load(&root, Default::default()).unwrap();
    assert_eq!(auto.threshold, 73);
    let args = ParsedArgs::parse_from([
        "vitest-dedup",
        root.to_str().unwrap(),
        "--config",
        "explicit.json",
    ])
    .unwrap();
    let explicit = vitest_dedup::config::Config::load(&args.root, args.to_overrides()).unwrap();
    assert_eq!(explicit.threshold, 81);
}

/// Census remains a presence-based debugging opt-in for every value.
#[test]
fn census_presence_has_consistent_cli_and_registry_semantics() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.test.ts"),
        "import { test } from 'vitest'; const N = mk(); test.each([[N], [2]])('n %s', () => {});",
    )
    .unwrap();
    let mut ordinary = None;
    for value in [None, Some(""), Some("0"), Some("1"), Some("invalid")] {
        let mut command = assert_cmd::Command::cargo_bin("vitest-dedup").unwrap();
        command
            .arg(dir.path())
            .args(["--reporters", "jsonl", "--threshold", "100"])
            .env_remove("VD_CELL_CENSUS");
        if let Some(value) = value {
            command.env("VD_CELL_CENSUS", value);
        }
        let output = command.output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).contains("cell census"),
            value.is_some()
        );
        if let Some(ordinary) = &ordinary {
            assert_eq!(&output.stdout, ordinary);
        } else {
            ordinary = Some(output.stdout);
        }
    }
}

/// Traversal failures retain their cause and partial status without exposing the root prefix.
#[cfg(unix)]
#[test]
fn traversal_diagnostics_use_relative_paths() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let hidden = root.join("private");
    std::fs::create_dir(&hidden).unwrap();
    std::fs::write(
        root.join("a.test.ts"),
        "import {test, expect} from 'vitest'; test('a',()=>expect(1).toBe(1));",
    )
    .unwrap();
    std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o0)).unwrap();
    let output = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(&root)
        .args(["--reporters", "jsonl", "--fail-on-incomplete=reads"])
        .output();
    std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = output.unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("private") && stderr.contains("failed while walking"),
        "{stderr}"
    );
    assert!(!stderr.contains(root.to_str().unwrap()), "{stderr}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let summary: serde_json::Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(summary["testsAnalyzed"], 1);
    assert_eq!(summary["incompleteCauses"]["discoveryFailed"], true);
}

/// A JSONL reader can consume one record and close a stream larger than the pipe buffer.
#[test]
fn jsonl_accepts_a_consumer_that_closes_after_one_record() {
    use std::io::{BufRead, BufReader};
    let dir = tempfile::tempdir().unwrap();
    let mut source = "import {test, expect} from 'vitest';\n".to_owned();
    for index in 0..256 {
        source.push_str(&format!(
            "test('case {index}', () => expect({index}).toBe({index}));\n"
        ));
    }
    std::fs::write(dir.path().join("a.test.ts"), source).unwrap();
    let mut command = Command::new(assert_cmd::cargo::cargo_bin("vitest-dedup"));
    command.arg(dir.path()).args([
        "--reporters",
        "jsonl",
        "--emit-test-fingerprints",
        "--threshold",
        "100",
    ]);
    for rule in vitest_dedup::model::Rule::ALL {
        command.args(["--rule", &format!("{}=off", rule.id())]);
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let record: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(record["type"], "test");
    drop(reader);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
