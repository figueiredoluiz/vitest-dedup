//! The iteration protocol a for-of loop selects remains visible through extraction and final
//! finding rules.

use vitest_dedup::analysis::analyze;
use vitest_dedup::config::{Config, ConfigOverrides};
use vitest_dedup::discovery::{ModuleFlavor, SourceFile, SourceLanguage};
use vitest_dedup::frontend::{extract, Globals};
use vitest_dedup::model::{IncompleteCauses, Rule, Severity};

/// Runtime conflicts stay comparable and distinct; same-protocol alpha-equivalent loops still
/// match.
#[test]
fn for_await_protocol_preserves_final_outcomes() {
    for (source, count, equivalent) in [
        (
            include_str!("../fixtures/duplicates/for-await-protocol.test.ts"),
            8,
            false,
        ),
        (
            include_str!("../fixtures/duplicates/for-await-equivalents.test.ts"),
            6,
            true,
        ),
    ] {
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
        assert_eq!(extracted.tests.len(), count);
        assert!(!extracted.incomplete, "{:?}", extracted.diagnostics);
        assert!(extracted
            .tests
            .iter()
            .all(|test| test.context.is_comparable()));
        let result = analyze(
            extracted.tests.clone(),
            &config,
            1,
            IncompleteCauses::default(),
        )
        .unwrap();
        assert_eq!(result.tests_analyzed, count);
        assert_eq!(result.unresolved_tests, 0);
        if equivalent {
            assert_eq!(result.findings.len(), count / 2, "{:?}", result.findings);
            assert!(result
                .findings
                .iter()
                .all(|finding| finding.rule == Rule::DuplicateTest));
        } else {
            assert!(result.findings.is_empty(), "{:?}", result.findings);
        }
        for pair in extracted.tests.as_chunks::<2>().0 {
            assert_eq!(
                pair[0].body.alpha == pair[1].body.alpha,
                equivalent,
                "{} / {}: alpha",
                pair[0].title.display(),
                pair[1].title.display()
            );
            assert_eq!(
                pair[0].body.structural == pair[1].body.structural,
                equivalent,
                "{} / {}: structural",
                pair[0].title.display(),
                pair[1].title.display()
            );
            assert_eq!(
                pair[0].body.behavior == pair[1].body.behavior,
                equivalent,
                "{} / {}: behavior",
                pair[0].title.display(),
                pair[1].title.display()
            );
        }
    }
}
