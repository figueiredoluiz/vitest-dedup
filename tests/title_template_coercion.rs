//! A template-literal title interpolating a composite coerces the way JavaScript's `ToString`
//! coerces it, so the runtime title -- not the internal canonical fold -- is what the analyzer
//! records and compares.

#[path = "support/report.rs"]
mod report;
use report::{of_type, records};

/// The titles of `records`' tests, in declaration order.
fn titles(records: &[serde_json::Value]) -> Vec<String> {
    of_type(records, "test")
        .iter()
        .map(|test| test["title"].as_str().unwrap().to_owned())
        .collect()
}

/// The coerced template title and its literal twin are byte-identical, and each colliding pair
/// is one duplicate-test-name warning.
#[test]
fn composite_title_substitutions_coerce_as_javascript() {
    let records = records(
        &[(
            "case.test.ts",
            include_str!("../fixtures/duplicates/title-template-composite-coercion.test.ts"),
        )],
        &[],
    );
    assert_eq!(
        titles(&records),
        [
            "obj [object Object]",
            "obj [object Object]",
            "arr 1,2",
            "arr 1,2",
        ],
        "a template title is the string JavaScript builds, never the canonical fold"
    );
    assert_eq!(of_type(&records, "summary")[0]["unresolvedTests"], 0);
    let findings = of_type(&records, "finding");
    assert_eq!(
        findings.len(),
        2,
        "each colliding-title pair is one duplicate-test-name finding: {findings:?}"
    );
    assert!(findings.iter().all(
        |finding| finding["rule"] == "duplicate-test-name" && finding["severity"] == "warning"
    ));
}

/// The refusal half: a composite whose coercion is interceptable has no provable title text, so
/// the test leaves comparison rather than guessing.
#[test]
fn interceptable_coercion_leaves_the_title_dynamic() {
    let source = "import { test, expect } from 'vitest';\n\
                  test(`x ${{ toString: () => 'y' }}`, () => { expect(1).toBe(1); });\n\
                  test('x y', () => { expect(1).toBe(1); });\n";
    let records = records(&[("case.test.ts", source)], &[]);
    let tests = of_type(&records, "test");
    assert_eq!(tests.len(), 2);
    assert!(
        !titles(&records)
            .iter()
            .any(|title| title == "x y y" || title.contains("[object Object]")),
        "the interceptable coercion must not be guessed"
    );
    let literal = tests.iter().find(|test| test["title"] == "x y").unwrap();
    assert!(literal["exclusion"].is_null(), "{literal}");
    assert_eq!(
        tests
            .iter()
            .filter(|test| !test["exclusion"].is_null())
            .count(),
        1,
        "a dynamic title marks its test unresolved: {tests:?}"
    );
}

/// A composite whose coercion is interceptable is still a known value: the loop over such
/// elements is read like any other, and the iterations share one declaration site, so no rule
/// reports them. The loop's test is counted once, not once per iteration (RC21).
#[test]
fn interceptable_coercion_still_reads_its_loop() {
    let source = "import { test, expect } from 'vitest';\n\
                  for (const row of [{ toString: 1 }, { toString: 2 }]) {\n\
                  test('literal', () => { expect(1).toBe(1); });\n\
                  }\n";
    let records = records(&[("case.test.ts", source)], &[]);
    assert_eq!(titles(&records), ["literal"]);
    assert_eq!(of_type(&records, "summary")[0]["unresolvedTests"], 0);
    assert!(
        of_type(&records, "finding").is_empty(),
        "loop iterations share a declaration site and are never reported as a pair"
    );
}

/// A helper parameter bound to a composite never renders its canonical fold as a title: the
/// test a helper declares is excluded where the helper is called, so no title is claimed for it.
#[test]
fn helper_parameter_titles_never_render_the_canonical_fold() {
    let source = "import { test, expect } from 'vitest';\n\
                  function declare(label: unknown) {\n\
                  test(`ctx ${label}`, () => { expect(1).toBe(1); });\n\
                  }\n\
                  declare({ a: 1 });\n";
    let records = records(&[("case.test.ts", source)], &[]);
    let tests = of_type(&records, "test");
    assert!(!tests.is_empty());
    assert!(
        tests.iter().all(|test| !test["exclusion"].is_null()),
        "{tests:?}"
    );
    assert!(
        !titles(&records)
            .iter()
            .any(|title| title.contains("a: 1") || title.contains("{")),
        "a bound composite title never renders the canonical fold"
    );
}
