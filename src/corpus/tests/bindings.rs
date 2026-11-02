//! Binding evaluation and ordinary-event eligibility over owned runtime cases.

use super::*;
use crate::model::{Rule, Severity};

/// Runs every rule, including opt-in subsumption, over the same source as the runtime oracle.
#[test]
fn owned_destructuring_keeps_effects_and_positive_controls() {
    let corpus = counted_corpus(
        include_str!("../../../fixtures/behavior-runtime/destructuring.test.ts"),
        "bindings.test.ts",
        27,
        0,
    );
    assert!(corpus.tests.iter().all(|case| case.trace_complete));
    for case in &corpus.tests {
        if case
            .title
            .as_deref()
            .is_some_and(|title| title.starts_with("binding only"))
        {
            assert_eq!(case.ordinary_events, 0);
            assert!(case.parameterization_key.is_none());
        }
    }
    let config = crate::config::Config {
        root: ".".into(),
        include: Vec::new(),
        include_is_explicit: false,
        exclude: Vec::new(),
        rules: [(Rule::SubsumedTest, Severity::Warning)].into(),
        threshold: 100,
        source: "test".into(),
    };
    let findings = crate::rules::run_with_hooks(
        &[("bindings.test.ts".into(), corpus.tests)],
        &[],
        &[],
        &config,
        &[],
    )
    .findings;
    let linked = |rule, a: &str, b: &str| {
        findings.iter().any(|finding| {
            finding.rule == rule
                && finding.titles.iter().any(|title| title == a)
                && finding.titles.iter().any(|title| title == b)
        })
    };
    for pair in [
        "unused last",
        "member",
        "observer",
        "scalar",
        "alias",
        "repeated",
        "movement",
        "iterator",
    ] {
        for rule in [
            Rule::NearDuplicateTest,
            Rule::SubsumedTest,
            Rule::ParameterizationCandidate,
        ] {
            assert!(
                !linked(
                    rule,
                    &format!("binding {pair} pass"),
                    &format!("binding {pair} fail")
                ),
                "{pair}: {findings:#?}"
            );
        }
    }
    for (a, b) in [
        ("binding retained member", "binding written member"),
        ("binding reordered one", "binding reordered two"),
    ] {
        assert!(linked(Rule::NearDuplicateTest, a, b), "{findings:#?}");
    }
    assert!(!linked(
        Rule::SubsumedTest,
        "binding ordinary smaller",
        "binding bookkeeping larger"
    ));
    assert!(linked(
        Rule::SubsumedTest,
        "binding ordinary smaller",
        "binding ordinary larger"
    ));
    assert!(!linked(
        Rule::SubsumedTest,
        "binding plain smaller",
        "binding plain larger"
    ));
}
