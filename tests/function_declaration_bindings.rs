//! Function declaration ownership through to the final duplicate findings.

#[path = "support/report.rs"]
mod report;
use report::{findings, report};

/// Compare two complete bodies through the analyzer: one duplicate-test exactly when they are
/// equivalent, and never a literal-only consolidation of different binding targets.
fn assert_pair(source: &str, equivalent: bool, label: &str) {
    let result = report(&[("case.test.ts", source)], &[]);
    assert_eq!(
        result["testsAnalyzed"], 2,
        "{label}: analyzed count: {result}"
    );
    assert_eq!(result["incomplete"], false, "{label}: {result}");
    let findings = findings(&result);
    let count = findings
        .iter()
        .filter(|finding| finding["rule"] == "duplicate-test")
        .count();
    assert_eq!(
        count,
        usize::from(equivalent),
        "{label}: final duplicate findings"
    );
    if !equivalent {
        assert!(
            !findings
                .iter()
                .any(|finding| finding["rule"] == "parameterization-candidate"),
            "{label}: different binding targets cannot be literal-only variants"
        );
    }
}

/// The exact Vitest oracle input passes `a` and fails `b`; it cannot be a duplicate.
#[test]
fn runtime_counterexample_keeps_function_targets_distinct() {
    assert_pair(
        include_str!("fixtures/function-bindings.test.ts"),
        false,
        "runtime oracle",
    );
}

/// Forward references, parameters, recursion and lexical shadows retain declaration ownership.
#[test]
fn declaration_binding_outcome_matrix() {
    let cases = [
        ("hoisted call", "expect(a()).toBe(1); function a(){return 1} function b(){return 2}",
            "expect(b()).toBe(1); function a(){return 1} function b(){return 2}", false),
        ("parameters", "function a(x){return x+1} function b(x){return x+2} expect(a(1)).toBe(2)",
            "function a(x){return x+1} function b(x){return x+2} expect(b(1)).toBe(2)", false),
        ("recursive target", "function a(n){return n ? a(n-1) : 1} function b(n){return n ? a(n-1) : 2} expect(a(1)).toBe(1)",
            "function a(n){return n ? a(n-1) : 1} function b(n){return n ? a(n-1) : 2} expect(b(1)).toBe(1)", false),
        ("recursive reference", "function a(){return 1} function b(){return a()} expect(b()).toBe(1)",
            "function a(){return 1} function b(){return b()} expect(b()).toBe(1)", false),
        ("default parameter reference", "function a(x=a){return x} function b(x=a){return x} expect(b()).toBe(a)",
            "function a(x=a){return x} function b(x=b){return x} expect(b()).toBe(a)", false),
        ("shadowed target", "function a(){return 1} { function a(){return 2} expect(a()).toBe(1) }",
            "function a(){return 1} { function b(){return 2} expect(a()).toBe(1) }", false),
        ("mutual recursion holdout", "expect(a(0)).toBe(1); function a(n){return n ? b(n-1) : 1} function b(n){return n ? a(n-1) : 2}",
            "expect(b(0)).toBe(1); function a(n){return n ? b(n-1) : 1} function b(n){return n ? a(n-1) : 2}", false),
        ("generator holdout", "function* a(){yield 1} function* b(){yield 2} expect(a().next().value).toBe(1)",
            "function* a(){yield 1} function* b(){yield 2} expect(b().next().value).toBe(1)", false),
        ("async holdout", "async function a(){return 1} async function b(){return 2} expect(a()).resolves.toBe(1)",
            "async function a(){return 1} async function b(){return 2} expect(b()).resolves.toBe(1)", false),
        ("renamed parameters", "function a(x){return x+1} function b(x){return x+2} expect(a(1)).toBe(2)",
            "function first(value){return value+1} function second(value){return value+2} expect(first(1)).toBe(2)", true),
        ("renamed hoisted recursion", "expect(a(1)).toBe(1); function a(n){return n ? a(n-1) : 1}",
            "expect(first(1)).toBe(1); function first(value){return value ? first(value-1) : 1}", true),
        ("renamed shadow", "function a(){return 1} { function a(){return 2} expect(a()).toBe(2) }",
            "function first(){return 1} { function second(){return 2} expect(second()).toBe(2) }", true),
    ];
    for (label, left, right, equivalent) in cases {
        let source = format!("import {{test, expect}} from 'vitest'; test('left', () => {{{left}}}); test('right', () => {{{right}}});");
        assert_pair(&source, equivalent, label);
    }
}
