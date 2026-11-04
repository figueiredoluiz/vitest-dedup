//! Object descriptor semantics remain visible through to the final findings.

#[path = "support/report.rs"]
mod report;
use report::{assert_complete, findings, report};

/// Runtime conflicts stay comparable and distinct, so no rule links them; same-kind and shorthand
/// equivalents still match, one duplicate-test per pair.
#[test]
fn object_property_descriptors_preserve_final_outcomes() {
    for (source, count, equivalent) in [
        (
            include_str!("../fixtures/duplicates/object-property-kinds.test.ts"),
            12,
            false,
        ),
        (
            include_str!("../fixtures/duplicates/object-property-equivalents.test.ts"),
            14,
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
            let duplicates = findings
                .iter()
                .filter(|finding| finding["rule"] == "duplicate-test")
                .count();
            assert_eq!(duplicates as u64, count / 2, "{findings:?}");
            // `x(){}` and `['x'](){}` define the same method, so those two pairs are also one
            // literal-only consolidation; nothing else may be linked.
            let others: Vec<_> = findings
                .iter()
                .filter(|finding| finding["rule"] != "duplicate-test")
                .map(|finding| (finding["rule"].clone(), finding["titles"].clone()))
                .collect();
            assert_eq!(
                others,
                [(
                    serde_json::json!("parameterization-candidate"),
                    serde_json::json!(["method first", "computed method first"])
                )],
                "{findings:?}"
            );
        } else {
            assert!(findings.is_empty(), "{findings:?}");
        }
    }
}
