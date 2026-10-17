//! Lexical private-brand ownership through extraction and final finding rules.

use vitest_dedup::analysis::analyze;
use vitest_dedup::config::{Config, ConfigOverrides};
use vitest_dedup::discovery::{ModuleFlavor, SourceFile, SourceLanguage};
use vitest_dedup::frontend::{extract, Globals};
use vitest_dedup::model::{IncompleteCauses, Rule, Severity};

/// Runtime-owned conflicting brands remain comparable and distinct; renamed owners still match.
#[test]
fn nested_private_owners_preserve_findings_and_comparison_eligibility() {
    for (source, count, unresolved, pairs) in [
        (
            include_str!("../fixtures/duplicates/nested-private-owner.test.ts"),
            10,
            0,
            vec![
                (0, 1, false),
                (2, 3, true),
                (4, 5, true),
                (6, 7, false),
                (8, 9, false),
            ],
        ),
        (
            include_str!("../fixtures/duplicates/nested-private-owner-renamed.test.ts"),
            4,
            0,
            vec![(0, 1, true), (2, 3, true)],
        ),
        (
            include_str!("../fixtures/duplicates/nested-private-owner-depth.test.ts"),
            4,
            0,
            vec![(0, 1, false), (2, 3, true)],
        ),
        (
            include_str!("../fixtures/duplicates/nested-private-owner-variants.test.ts"),
            4,
            0,
            vec![(0, 1, false), (2, 3, true)],
        ),
        (
            include_str!("../fixtures/duplicates/nested-private-owner-uncertain.test.ts"),
            4,
            2,
            vec![(2, 3, true)],
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let file = SourceFile::without_project_context(
            root.path().join("case.test.ts"),
            SourceLanguage::TypeScript,
            ModuleFlavor::Ambiguous,
        );
        let extracted = extract(source, &file, Globals::Disabled);
        assert_eq!(extracted.tests.len(), count);
        assert!(!extracted.incomplete, "{:?}", extracted.diagnostics);
        assert_eq!(
            extracted
                .tests
                .iter()
                .filter(|test| !test.context.is_comparable())
                .count(),
            unresolved
        );
        for &(left, right, _) in &pairs {
            assert!(extracted.tests[left].context.is_comparable());
            assert!(extracted.tests[right].context.is_comparable());
        }
        let excluded: Vec<_> = extracted
            .tests
            .iter()
            .filter(|test| !test.context.is_comparable())
            .map(|test| test.title.display().to_owned())
            .collect();
        let titles: Vec<_> = extracted
            .tests
            .iter()
            .map(|test| test.title.display().to_owned())
            .collect();
        for &(left, right, equivalent) in &pairs {
            assert_eq!(
                extracted.tests[left].body.alpha == extracted.tests[right].body.alpha,
                equivalent,
                "{} / {}: alpha",
                titles[left],
                titles[right]
            );
            assert_eq!(
                extracted.tests[left].body.structural == extracted.tests[right].body.structural,
                equivalent,
                "{} / {}: structural",
                titles[left],
                titles[right]
            );
        }
        let config = Config::load(
            root.path(),
            ConfigOverrides {
                rules: [(Rule::SubsumedTest, Severity::Warning)].into(),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();
        let result = analyze(extracted.tests, &config, 1, IncompleteCauses::default()).unwrap();
        assert_eq!(result.tests_analyzed, count);
        assert_eq!(result.unresolved_tests, unresolved);
        assert!(result
            .findings
            .iter()
            .all(|finding| finding.titles.iter().all(|title| !excluded.contains(title))));
        for (left, right, equivalent) in pairs {
            let findings: Vec<_> = result
                .findings
                .iter()
                .filter(|finding| {
                    finding.titles.contains(&titles[left])
                        && finding.titles.contains(&titles[right])
                })
                .collect();
            if equivalent {
                assert_eq!(
                    findings
                        .iter()
                        .filter(|finding| finding.rule == Rule::DuplicateTest)
                        .count(),
                    1,
                    "{} / {}: {findings:?}",
                    titles[left],
                    titles[right]
                );
            } else {
                assert!(
                    findings.is_empty(),
                    "{} / {}: {findings:?}",
                    titles[left],
                    titles[right]
                );
            }
        }
    }
}
