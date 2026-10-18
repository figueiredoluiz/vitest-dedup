//! The canonical object rendering behind `.each` cells and `test.extend` values keeps distinct
//! keys distinct through extraction and the final finding rules.

use vitest_dedup::analysis::analyze;
use vitest_dedup::config::{Config, ConfigOverrides};
use vitest_dedup::discovery::{ModuleFlavor, SourceFile, SourceLanguage};
use vitest_dedup::frontend::{extract, Globals};
use vitest_dedup::model::{IncompleteCauses, Rule, Severity};

/// One fixture extracted and analyzed with every rule enabled, asserting full coverage first.
fn run(source: &str) -> vitest_dedup::model::AnalysisResult {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(
        directory.path(),
        ConfigOverrides {
            rules: [(Rule::SubsumedTest, Severity::Warning)].into(),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();
    let input = SourceFile::without_project_context(
        directory.path().join("case.test.ts"),
        SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    let extracted = extract(source, &input, Globals::Disabled);
    assert!(!extracted.incomplete, "{:?}", extracted.diagnostics);
    assert!(extracted
        .tests
        .iter()
        .all(|test| test.context.is_comparable()));
    let count = extracted.tests.len();
    let result = analyze(extracted.tests, &config, 1, IncompleteCauses::default()).unwrap();
    assert_eq!(result.tests_analyzed, count);
    assert_eq!(result.unresolved_tests, 0);
    result
}

/// A table key that spells the rendering's framing must not collapse onto the structure it
/// spells: the pair stays a warning-severity consolidation candidate, never an error duplicate.
#[test]
fn each_table_keys_that_spell_the_framing_stay_distinct() {
    let result = run(include_str!(
        "../fixtures/duplicates/each-object-key-ambiguity.test.ts"
    ));
    assert_eq!(result.tests_analyzed, 6);
    for finding in &result.findings {
        assert_eq!(finding.rule, Rule::ParameterizationCandidate, "{finding:?}");
        assert_eq!(finding.severity, Severity::Warning, "{finding:?}");
    }
    assert_eq!(result.findings.len(), 3, "{:?}", result.findings);
}

/// A `test.extend` value whose key spells the framing is its own context: identical bodies, and
/// no finding at all may link the pair.
#[test]
fn extend_values_that_spell_the_framing_stay_distinct() {
    let result = run(include_str!(
        "../fixtures/duplicates/extend-object-key-ambiguity.test.ts"
    ));
    assert_eq!(result.tests_analyzed, 4);
    assert!(result.findings.is_empty(), "{:?}", result.findings);
}

/// Quoted, identifier and whitespace spellings of the same keys still merge at error severity.
#[test]
fn equivalent_table_key_spellings_still_merge() {
    let result = run(include_str!(
        "../fixtures/duplicates/each-object-key-equivalents.test.ts"
    ));
    assert_eq!(result.tests_analyzed, 6);
    assert_eq!(result.findings.len(), 3, "{:?}", result.findings);
    assert!(result
        .findings
        .iter()
        .all(|finding| finding.rule == Rule::DuplicateTest && finding.severity == Severity::Error));
}
