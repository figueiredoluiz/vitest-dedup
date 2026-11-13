//! Factory setup must remain usable at its existing statement bound.
use assert_cmd::Command;
use serde_json::Value;
use std::time::Duration;

/// The maximum inert alias setup registers its one test without speculative callable walks.
#[test]
fn maximal_inert_factory_setup_remains_comparable() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("aliases.ts"),
        include_str!("../fixtures/oracle/factory-resource-work/aliases.ts"),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("case.test.ts"),
        include_str!("../fixtures/oracle/factory-resource-work/aliases.test.ts"),
    )
    .unwrap();
    let output = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .timeout(Duration::from_secs(15))
        .arg(directory.path())
        .args(["--reporters", "jsonl"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let summary = records
        .iter()
        .find(|record| record["type"] == "summary")
        .unwrap();
    assert_eq!(summary["testsAnalyzed"], 1);
    assert_eq!(summary["unresolvedTests"], 0);
    assert_eq!(summary["incomplete"], false);
    assert_eq!(summary["errors"], 0);
    assert_eq!(summary["warnings"], 0);
}
