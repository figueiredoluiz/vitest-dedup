//! Optional extglob expansion must refuse overflow without losing imported-test findings.

use assert_cmd::Command;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// The owned overflow and exact-budget fixtures retain findings and report discovery coverage.
#[test]
fn extglob_budget_preserves_findings_and_incomplete_exit_policy() {
    for (fixture, incomplete, titles) in [
        (
            "config-extglob-overflow",
            true,
            ["uncertain first", "uncertain second"],
        ),
        (
            "config-extglob-boundary",
            false,
            ["kept first", "kept second"],
        ),
    ] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/recall")
            .join(fixture);
        for fail_on_incomplete in [false, true] {
            let mut command = Command::cargo_bin("vitest-dedup").unwrap();
            command
                .arg(&root)
                .args(["--reporters", "jsonl", "--no-metrics"])
                .timeout(Duration::from_secs(15));
            if fail_on_incomplete {
                command.arg("--fail-on-incomplete");
            }
            let output = command.output().unwrap();
            let expected_exit = if incomplete && fail_on_incomplete {
                2
            } else {
                1
            };
            assert_eq!(
                output.status.code(),
                Some(expected_exit),
                "{fixture}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let records: Vec<Value> = String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let summary = records.iter().find(|r| r["type"] == "summary").unwrap();
            assert_eq!(summary["testsAnalyzed"], 2, "{summary}");
            assert_eq!(summary["unresolvedTests"], 0, "{summary}");
            assert_eq!(summary["qualityGate"]["eligibleCases"], 2, "{summary}");
            assert_eq!(summary["incomplete"], incomplete, "{summary}");
            assert_eq!(
                summary["incompleteCauses"]["projectConfigDegraded"], incomplete,
                "{summary}"
            );
            let findings: Vec<_> = records.iter().filter(|r| r["type"] == "finding").collect();
            assert_eq!(findings.len(), 1, "{findings:?}");
            assert_eq!(findings[0]["rule"], "duplicate-test");
            assert_eq!(findings[0]["titles"], json!(titles));
        }
    }
}
