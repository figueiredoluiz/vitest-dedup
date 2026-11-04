//! The iteration protocol a for-of loop selects remains visible through to the final findings.

#[path = "support/report.rs"]
mod report;
use report::{assert_complete, findings, report};

/// Runtime conflicts stay comparable and distinct, so no rule links them; same-protocol
/// alpha-equivalent loops still match, one duplicate-test per pair.
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
        let result = report(
            &[("case.test.ts", source)],
            &["--rule", "subsumed-test=warning"],
        );
        assert_complete(&result, count);
        let findings = findings(&result);
        if equivalent {
            assert_eq!(findings.len() as u64, count / 2, "{findings:?}");
            assert!(findings
                .iter()
                .all(|finding| finding["rule"] == "duplicate-test"));
        } else {
            assert!(findings.is_empty(), "{findings:?}");
        }
    }
}
