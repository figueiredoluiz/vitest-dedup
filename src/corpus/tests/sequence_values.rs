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
