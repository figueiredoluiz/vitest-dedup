//! One run of the analyzer over files written to a temporary project, read back as its JSON report.
#![allow(dead_code)]

use serde_json::Value;

/// The JSON report of the binary run over `files` (path, source) with `flags` added.
pub fn report(files: &[(&str, &str)], flags: &[&str]) -> Value {
    let directory = tempfile::tempdir().unwrap();
    for (path, source) in files {
        let path = directory.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    report_on(directory.path(), flags)
}

/// The JSON report of the binary run over the project at `root` with `flags` added.
pub fn report_on(root: &std::path::Path, flags: &[&str]) -> Value {
    let output = tempfile::tempdir().unwrap();
    let prefix = output.path().join("report");
    let run = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(root)
        .args(["--reporters", "json", "--no-metrics", "--output"])
        .arg(&prefix)
        .args(flags)
        .output()
        .unwrap();
    let code = run.status.code();
    assert!(
        matches!(code, Some(0 | 1)),
        "the analyzer did not finish a run: {code:?}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    serde_json::from_str(&std::fs::read_to_string(prefix.with_extension("json")).unwrap()).unwrap()
}

/// The report's findings.
pub fn findings(report: &Value) -> &Vec<Value> {
    report["findings"].as_array().unwrap()
}

/// Asserts the run read every test, `count` of them, with none excluded.
pub fn assert_complete(report: &Value, count: u64) {
    assert_eq!(report["testsAnalyzed"], count, "{report}");
    assert_eq!(report["unresolvedTests"], 0, "{report}");
    assert_eq!(report["incomplete"], false, "{report}");
}

/// The JSONL records of the binary run over `files` with test records emitted and `flags` added.
pub fn records(files: &[(&str, &str)], flags: &[&str]) -> Vec<Value> {
    let directory = tempfile::tempdir().unwrap();
    for (path, source) in files {
        let path = directory.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    let run = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(directory.path())
        .args([
            "--reporters",
            "jsonl",
            "--no-metrics",
            "--emit-test-fingerprints",
        ])
        .args(flags)
        .output()
        .unwrap();
    assert!(
        matches!(run.status.code(), Some(0 | 1)),
        "the analyzer did not finish a run: {:?}\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8(run.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The records of one `type`.
pub fn of_type<'a>(records: &'a [Value], kind: &str) -> Vec<&'a Value> {
    records
        .iter()
        .filter(|record| record["type"] == kind)
        .collect()
}
