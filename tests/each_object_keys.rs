//! The canonical object rendering behind `.each` cells and `test.extend` values keeps distinct
//! keys distinct through to the final findings.

#[path = "support/report.rs"]
mod report;
use report::{assert_complete, findings, report};

/// One fixture analyzed with every rule enabled, asserting full coverage of `count` tests first.
fn run(source: &str, count: u64) -> serde_json::Value {
    let result = report(
        &[("case.test.ts", source)],
        &["--rule", "subsumed-test=warning"],
    );
    assert_complete(&result, count);
    result
}

/// A table key that spells the rendering's framing must not collapse onto the structure it
/// spells: all three pairs are consolidation candidates, never error duplicates.
#[test]
fn each_table_keys_that_spell_the_framing_stay_distinct() {
    let result = run(
        include_str!("../fixtures/duplicates/each-object-key-ambiguity.test.ts"),
        6,
    );
    let findings = findings(&result);
    assert_eq!(findings.len(), 3, "{result}");
    assert!(
        findings
            .iter()
            .all(|finding| finding["rule"] == "parameterization-candidate"
                && finding["severity"] == "warning"),
        "{result}"
    );
}

/// A `test.extend` value whose key spells the framing is its own context: identical bodies, and
/// no finding at all may link the pair.
#[test]
fn extend_values_that_spell_the_framing_stay_distinct() {
    let result = run(
        include_str!("../fixtures/duplicates/extend-object-key-ambiguity.test.ts"),
        4,
    );
    assert!(findings(&result).is_empty(), "{result}");
}

/// Quoted, identifier and whitespace spellings of the same keys still merge at error severity.
#[test]
fn equivalent_table_key_spellings_still_merge() {
    let result = run(
        include_str!("../fixtures/duplicates/each-object-key-equivalents.test.ts"),
        6,
    );
    let findings = findings(&result);
    assert_eq!(findings.len(), 3, "{findings:?}");
    assert!(findings
        .iter()
        .all(|finding| finding["rule"] == "duplicate-test" && finding["severity"] == "error"));
}
