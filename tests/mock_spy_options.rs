//! Owned runtime examples keep spy registrations comparable without conflating mock modes.

#[path = "support/report.rs"]
mod report;
use report::{findings, report_on};
use std::path::Path;

/// Analyze the permanent corpus sources that the collection oracle also collects with Vitest.
fn check_case(name: &str, expected_tests: u64, unresolved: u64, expected_findings: usize) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/recall")
        .join(name);
    let result = report_on(&root, &[]);
    assert_eq!(result["testsAnalyzed"], expected_tests, "{result}");
    assert_eq!(result["unresolvedTests"], unresolved, "{result}");
    let findings = findings(&result);
    assert_eq!(findings.len(), expected_findings, "{findings:?}");
    if expected_findings > 0 {
        assert_eq!(findings[0]["rule"], "cross-file-duplicate");
    }
}

/// Matching spy options retain duplicate findings; other modes and unknown options cannot match.
#[test]
fn spy_options_preserve_positive_and_conflicting_contexts() {
    check_case("mock-spy-options-positive", 2, 0, 1);
    check_case("mock-spy-options-context-negative", 4, 0, 0);
    check_case("mock-spy-options-unreadable", 2, 2, 0);
}
