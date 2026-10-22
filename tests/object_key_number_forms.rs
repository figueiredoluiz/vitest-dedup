//! Numeric object keys canonicalize to the runtime key they bind, in both the `.each` cell and
//! `test.extend` value paths.

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

/// Runtime-distinct keys stay distinct: the `.each` pair lands at parameterization-candidate
/// (alpha apart, structure equal) and the `test.extend` pair carries the folded value in its
/// context, so no finding may link it at all.
#[test]
fn keys_that_look_alike_stay_distinct() {
    let result = run(include_str!(
        "../fixtures/duplicates/object-key-number-forms-distinct.test.ts"
    ));
    assert_eq!(result.tests_analyzed, 4);
    assert_eq!(
        result.findings.len(),
        1,
        "only the `.each` pair may be reported: {:?}",
        result.findings
    );
    assert_eq!(result.findings[0].rule, Rule::ParameterizationCandidate);
    assert_eq!(result.findings[0].severity, Severity::Warning);
}

/// Runtime-identical keys fold together in both paths: identical-bodied tests report
/// duplicate-test at error severity whether the twin key spelling sits in an `.each` cell or a
/// `test.extend` value.
#[test]
fn keys_that_spell_one_key_fold_together() {
    let result = run(include_str!(
        "../fixtures/duplicates/object-key-number-forms-equivalents.test.ts"
    ));
    assert_eq!(result.tests_analyzed, 4);
    assert_eq!(result.findings.len(), 2, "{:?}", result.findings);
    assert!(result
        .findings
        .iter()
        .all(|finding| finding.rule == Rule::DuplicateTest && finding.severity == Severity::Error));
}
