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

/// Runtime-distinct keys never become error duplicates. Only the data-row pair consolidates;
/// different fixture contexts remain separate.
#[test]
fn keys_that_look_alike_stay_distinct() {
    let result = run(include_str!(
        "../fixtures/duplicates/object-key-number-forms-distinct.test.ts"
    ));
    let findings = findings(&result);
    assert_eq!(findings.len(), 1, "{result}");
    assert_eq!(findings[0]["rule"], "parameterization-candidate");
    assert_eq!(findings[0]["severity"], "warning");
    assert_eq!(
        findings[0]["titles"],
        serde_json::json!(["exponent each", "decimal each"])
    );
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
