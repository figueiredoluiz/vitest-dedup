//! Lexical private-brand ownership through to the final findings.

#[path = "support/report.rs"]
mod report;
use report::{of_type, records};

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
        // The escaped callables' owner is unresolved, but the two bodies are spelled identically
        // in an identical context, so they run the same and are linked.
        (
            include_str!("../fixtures/duplicates/nested-private-owner-uncertain.test.ts"),
            4,
            0,
            vec![(0, 1, true), (2, 3, true)],
        ),
    ] {
        let records = records(
            &[("case.test.ts", source)],
            &["--rule", "subsumed-test=warning"],
        );
        let tests = of_type(&records, "test");
        assert_eq!(tests.len(), count);
        let summary = of_type(&records, "summary")[0];
        assert_eq!(summary["testsAnalyzed"], count);
        assert_eq!(summary["unresolvedTests"], unresolved);
        let titles: Vec<String> = tests
            .iter()
            .map(|test| test["title"].as_str().unwrap().to_owned())
            .collect();
        let excluded: Vec<&String> = tests
            .iter()
            .zip(&titles)
            .filter(|(test, _)| !test["exclusion"].is_null())
            .map(|(_, title)| title)
            .collect();
        assert_eq!(excluded.len(), unresolved);
        for &(left, right, _) in &pairs {
            assert!(tests[left]["exclusion"].is_null(), "{}", titles[left]);
            assert!(tests[right]["exclusion"].is_null(), "{}", titles[right]);
        }
        let findings = of_type(&records, "finding");
        assert!(findings.iter().all(|finding| finding["titles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|title| !excluded.iter().any(|excluded| title == excluded.as_str()))));
        for (left, right, equivalent) in pairs {
            let linking: Vec<_> = findings
                .iter()
                .filter(|finding| {
                    let titles_of = finding["titles"].as_array().unwrap();
                    titles_of.iter().any(|title| title == titles[left].as_str())
                        && titles_of
                            .iter()
                            .any(|title| title == titles[right].as_str())
                })
                .collect();
            if equivalent {
                assert_eq!(
                    linking
                        .iter()
                        .filter(|finding| finding["rule"] == "duplicate-test")
                        .count(),
                    1,
                    "{} / {}: {linking:?}",
                    titles[left],
                    titles[right]
                );
            } else {
                assert!(
                    linking.is_empty(),
                    "{} / {}: {linking:?}",
                    titles[left],
                    titles[right]
                );
            }
        }
    }
}
