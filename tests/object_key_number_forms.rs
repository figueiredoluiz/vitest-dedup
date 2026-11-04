//! Numeric object keys canonicalize to the runtime key they bind, in both the `.each` cell and
//! `test.extend` value paths.

#[path = "support/report.rs"]
mod report;
use report::{assert_complete, findings, report};

/// One fixture analyzed with every rule enabled, asserting full coverage first.
fn run(source: &str) -> serde_json::Value {
    let result = report(
        &[("case.test.ts", source)],
        &["--rule", "subsumed-test=warning"],
    );
    assert_complete(&result, 4);
    result
}

/// Runtime-distinct keys stay distinct: neither the `.each` pair nor the `test.extend` pair is
/// linked. The `.each` pair would be a parameterization candidate under a whole-value row model;
/// until then its different runtime keys keep it out (`next-duplicates-known.json` records the miss).
#[test]
fn keys_that_look_alike_stay_distinct() {
    let result = run(include_str!(
        "../fixtures/duplicates/object-key-number-forms-distinct.test.ts"
    ));
    assert!(findings(&result).is_empty(), "{result}");
}

/// Runtime-identical keys fold together in both paths: identical-bodied tests report
/// duplicate-test at error severity whether the twin key spelling sits in an `.each` cell or a
/// `test.extend` value.
#[test]
fn keys_that_spell_one_key_fold_together() {
    let result = run(include_str!(
        "../fixtures/duplicates/object-key-number-forms-equivalents.test.ts"
    ));
    let findings = findings(&result);
    assert_eq!(findings.len(), 2, "{findings:?}");
    assert!(findings
        .iter()
        .all(|finding| finding["rule"] == "duplicate-test" && finding["severity"] == "error"));
}
