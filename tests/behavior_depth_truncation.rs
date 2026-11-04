//! A body whose behavior trace the depth cap cut short must leave comparison entirely -- its
//! trace compares equal to a different body's equally-truncated trace, and that equality is an
//! artifact of the cap, not a similarity claim.

#[path = "support/report.rs"]
mod report;
use report::{of_type, records};

/// The deep pair is unresolved and silent; the shallow pair stays comparable.
///
/// The fixture holds four tests: two whose constructors (`trace.push(1)` versus
/// `trace.unshift(2)`) sit past the behavior collector's depth cap under 260 unbraced `if`
/// statements, and the same construction at two levels of nesting where the effect is
/// collected. Under Node v26.0.0 / Vitest 5.0.5 the file runs 3 passed / 1 failed, which is
/// exactly the difference the truncated trace cannot see. The shallow pair would be a
/// near-duplicate; that miss is recorded in `next-duplicates-known.json`.
#[test]
fn behavior_depth_truncation_leaves_comparison_and_keeps_the_shallow_pair() {
    let records = records(
        &[(
            "case.test.ts",
            include_str!("../fixtures/duplicates/behavior-depth-truncation.test.ts"),
        )],
        &[],
    );
    let tests = of_type(&records, "test");
    assert_eq!(tests.len(), 4);
    let by_title = |needle: &str| {
        *tests
            .iter()
            .find(|test| test["title"] == needle)
            .unwrap_or_else(|| panic!("no test titled {needle:?}"))
    };
    for needle in ["deep push", "deep unshift"] {
        assert!(
            !by_title(needle)["exclusion"].is_null(),
            "{needle}: a depth-truncated trace must never be compared"
        );
    }
    for needle in ["shallow sink", "shallow sink inert marker"] {
        assert!(
            by_title(needle)["exclusion"].is_null(),
            "{needle}: a body inside the cap stays comparable"
        );
    }
    assert_eq!(of_type(&records, "summary")[0]["unresolvedTests"], 2);
    assert!(
        of_type(&records, "finding").is_empty(),
        "neither pair may be linked on a truncated trace"
    );
}

/// A hook body past the depth cap is still one hook both tests run: with no shared state it
/// writes, identical twins under it are duplicates; once it writes state they read, the order
/// they run in decides their results (Vitest 5.0.5: the second fails) and neither is compared.
#[test]
fn behavior_depth_truncation_in_a_hook_keeps_its_effects_visible() {
    let deep = |effect: &str| format!("{}{effect}", "if (true) ".repeat(260));
    let source = |hook: String, check: &str| {
        format!(
            "import {{ test, expect, beforeEach }} from 'vitest';\n\
             let n = 0;\n\
             beforeEach(() => {{ {hook} }});\n\
             test('under deep hook', () => {{ {check} }});\n\
             test('under deep hook twin', () => {{ {check} }});\n",
        )
    };
    let quiet = records(
        &[(
            "case.test.ts",
            &source(deep("expect(1).toBe(1);"), "expect(1).toBe(1);"),
        )],
        &[],
    );
    assert_eq!(of_type(&quiet, "summary")[0]["unresolvedTests"], 0);
    let findings = of_type(&quiet, "finding");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0]["rule"], "duplicate-test");

    let counting = records(
        &[("case.test.ts", &source(deep("n++;"), "expect(n).toBe(1);"))],
        &[],
    );
    assert_eq!(of_type(&counting, "summary")[0]["unresolvedTests"], 2);
    assert!(
        of_type(&counting, "finding").is_empty(),
        "tests whose results depend on the order a deep hook runs in must not be linked"
    );
}
