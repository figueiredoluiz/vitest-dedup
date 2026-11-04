//! End-to-end eligibility and finding controls for API export identity.
use super::*;
use crate::model::Rule;

macro_rules! fixture {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!(
                "../../../fixtures/oracle/api-export-identity/",
                $name
            )),
        )
    };
}

const FILES: &[(&str, &str)] = &[
    fixture!("a.ts"),
    fixture!("b.ts"),
    fixture!("diamond.ts"),
    fixture!("explicit.ts"),
    fixture!("reverse.ts"),
    fixture!("override.ts"),
    fixture!("shadow.ts"),
    fixture!("conflicting.ts"),
    fixture!("side-effect.ts"),
    fixture!("with-load.ts"),
    fixture!("loaded.test.ts"),
    fixture!("diamond.test.ts"),
    fixture!("explicit.test.ts"),
    fixture!("reverse.test.ts"),
    fixture!("override.test.ts"),
    fixture!("conflicting.test.ts"),
    fixture!("control.test.ts"),
];

/// Every proved API path keeps the executed population and duplicate claim, including cache hits.
#[test]
fn owned_export_identity_cases_remain_comparable_in_both_read_orders() {
    let forward = [
        "control.test.ts",
        "diamond.test.ts",
        "explicit.test.ts",
        "reverse.test.ts",
        "override.test.ts",
    ];
    let reverse = forward.iter().rev().copied().collect::<Vec<_>>();
    for (order, result) in [&forward[..], &reverse[..]]
        .into_iter()
        .zip(runs(FILES, &[&forward, &reverse]))
    {
        for (name, corpus) in order.iter().zip(result) {
            assert_eq!(corpus.tests.len(), 2, "{name}");
            assert!(
                corpus.tests.iter().all(|test| test.exclusion.is_none()),
                "{name}"
            );
            let outcome = owned_findings(name, corpus);
            let duplicates = outcome
                .findings
                .iter()
                .filter(|finding| finding.rule == Rule::DuplicateTest)
                .collect::<Vec<_>>();
            assert_eq!(duplicates.len(), 1, "{name}: {:?}", outcome.findings);
            assert_eq!(duplicates[0].titles, ["first", "second"]);
        }
    }
}

/// An ordinary value exported by an earlier star never acquires framework-call semantics.
#[test]
fn an_opaque_star_keeps_its_owned_refusal() {
    let corpus = corpora(FILES, &["conflicting.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 2);
    assert!(corpus.tests.iter().all(|test| test.exclusion.is_some()));
    assert!(owned_findings("conflicting.test.ts", corpus)
        .findings
        .is_empty());
}

/// Proving a callable's identity does not hide registrations performed while its module loads.
#[test]
fn proved_api_identity_keeps_module_load_uncertainty() {
    let corpus = corpora(FILES, &["loaded.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 3);
    assert_eq!(
        corpus
            .tests
            .iter()
            .filter(|test| test.exclusion.is_some())
            .count(),
        1
    );
    let outcome = owned_findings("loaded.test.ts", corpus);
    assert_eq!(
        outcome
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateTest)
            .count(),
        1
    );
}
