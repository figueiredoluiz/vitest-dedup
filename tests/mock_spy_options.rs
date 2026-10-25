//! Owned runtime examples keep spy registrations comparable without conflating mock modes.

use std::path::Path;
use vitest_dedup::analysis::analyze;
use vitest_dedup::config::{Config, ConfigOverrides};
use vitest_dedup::discovery::discover;
use vitest_dedup::frontend::{extract, Globals};
use vitest_dedup::model::{IncompleteCauses, Rule};

/// Analyze the exact permanent corpus sources also exercised by the Vitest runtime oracle.
fn check_case(name: &str, expected_tests: usize, unresolved: usize, findings: usize) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/recall")
        .join(name);
    let config = Config::load(&root, ConfigOverrides::default()).unwrap();
    let discovered = discover(&config, None).unwrap();
    assert!(!discovered.incomplete);
    let mut tests = Vec::new();
    for file in &discovered.files {
        let source = std::fs::read_to_string(&file.path).unwrap();
        let extracted = extract(&source, file, Globals::Disabled);
        assert!(!extracted.incomplete, "{:?}", extracted.diagnostics);
        assert!(!extracted.tests.is_empty(), "{}", file.path.display());
        tests.extend(extracted.tests);
    }
    assert_eq!(tests.len(), expected_tests);
    let result = analyze(
        tests,
        &config,
        discovered.files.len(),
        IncompleteCauses::default(),
    )
    .unwrap();
    assert_eq!(result.tests_analyzed, expected_tests);
    assert_eq!(result.unresolved_tests, unresolved);
    assert_eq!(result.findings.len(), findings, "{:?}", result.findings);
    if findings > 0 {
        assert_eq!(result.findings[0].rule, Rule::CrossFileDuplicate);
    }
}

/// Matching spy options retain duplicate findings; other modes and unknown options cannot match.
#[test]
fn spy_options_preserve_positive_and_conflicting_contexts() {
    check_case("mock-spy-options-positive", 2, 0, 1);
    check_case("mock-spy-options-context-negative", 4, 0, 0);
    check_case("mock-spy-options-unreadable", 2, 2, 0);
}
