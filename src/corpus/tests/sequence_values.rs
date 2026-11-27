//! Owned runtime sources account for sequence calls while retaining independent comparison.
use super::*;

macro_rules! fixture {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!(
                "../../../fixtures/oracle/sequence-registration/",
                $name
            )),
        )
    };
}

/// Named/default transport and prefix calls retain their records and the independent duplicate.
#[test]
fn sequence_wrapper_corpus_retains_calls_and_prefixes() {
    let files = [
        fixture!("named.ts"),
        fixture!("default.ts"),
        fixture!("named.test.ts"),
        fixture!("default.test.ts"),
        fixture!("prefix.test.ts"),
        fixture!("control.test.ts"),
    ];
    let paths = [
        "named.test.ts",
        "default.test.ts",
        "prefix.test.ts",
        "control.test.ts",
    ];
    let results = corpora(&files, &paths);
    assert_eq!(results.len(), 4);
    let counts: Vec<_> = results.iter().map(|corpus| corpus.tests.len()).collect();
    assert_eq!(counts, [1, 1, 3, 2]);
    for corpus in &results {
        assert_eq!(corpus.parse_errors, 0);
    }
    assert!(results[..2]
        .iter()
        .flat_map(|corpus| &corpus.tests)
        .all(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)));
    assert!(results[2]
        .tests
        .iter()
        .any(|case| case.title.as_deref() == Some("prefix one")));
    assert!(results[2]
        .tests
        .iter()
        .any(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)));
    assert!(results[3].tests.iter().all(|case| case.exclusion.is_none()));
    let outcome = owned_outcome(paths.into_iter().zip(results).collect());
    assert_eq!(outcome.findings.len(), 1, "{:?}", outcome.findings);
    assert_eq!(outcome.findings[0].rule, crate::model::Rule::DuplicateTest);
    assert_eq!(outcome.findings[0].locations.len(), 2);
    assert!(outcome.findings[0]
        .locations
        .iter()
        .all(|location| location.path.as_path() == Path::new("control.test.ts")));
}

/// A sequence-selected fixture builder cannot leave a clean report on the direct tests alone.
#[test]
fn sequence_fixture_builder_keeps_the_independent_duplicate() {
    let files = [
        fixture!("builder.test.ts"),
        fixture!("namespace-builder.test.ts"),
        fixture!("control.test.ts"),
    ];
    let paths = [
        "builder.test.ts",
        "namespace-builder.test.ts",
        "control.test.ts",
    ];
    let results = corpora(&files, &paths);
    assert_eq!(results.len(), 3);
    for (path, result) in paths[..2].iter().zip(&results[..2]) {
        assert_eq!(result.parse_errors, 0, "{path}");
        assert_eq!(result.tests.len(), 1, "{path}");
        assert_eq!(
            result.tests[0].exclusion,
            Some(Exclusion::RegisteredElsewhere),
            "{path}"
        );
    }
    assert_eq!(results[2].tests.len(), 2);
    assert!(results[2].tests.iter().all(|case| case.exclusion.is_none()));
    let outcome = owned_outcome(paths.into_iter().zip(results).collect());
    assert_eq!(outcome.findings.len(), 1, "{:?}", outcome.findings);
    let finding = &outcome.findings[0];
    assert_eq!(finding.rule, crate::model::Rule::DuplicateTest);
    assert_eq!(finding.locations.len(), 2);
    assert!(finding
        .locations
        .iter()
        .all(|location| location.path.as_path() == Path::new("control.test.ts")));
}

/// The Vitest source keeps every invocation eligible and preserves both equal-body groups.
#[test]
fn direct_sequence_corpus_keeps_titles_and_duplicate_participants() {
    let path = "case.test.ts";
    let source = include_str!("../../../fixtures/oracle/direct-sequence-registration/case.test.ts");
    let corpus = corpora(&[(path, source)], &[path]).remove(0);
    assert_eq!(corpus.parse_errors, 0);
    assert_eq!(corpus.tests.len(), 6, "{corpus:?}");
    assert!(corpus.exclusions().is_empty(), "{corpus:?}");
    let titles: Vec<_> = corpus
        .tests
        .iter()
        .map(|test| test.title.as_deref())
        .collect();
    assert_eq!(
        titles,
        [
            Some("selected"),
            Some("different body"),
            Some("retained left"),
            Some("retained right"),
            Some("asserted"),
            Some("instantiated"),
        ]
    );
    let outcome = owned_outcome(vec![(path, corpus)]);
    let duplicates: Vec<_> = outcome
        .findings
        .iter()
        .filter(|finding| finding.rule == crate::model::Rule::DuplicateTest)
        .map(|finding| {
            finding
                .locations
                .iter()
                .map(|location| location.span.start.line)
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(duplicates, [[4, 6], [6, 7], [8, 13]]);
}
