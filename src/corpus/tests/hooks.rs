//! Final-outcome controls for proven hook effects and lifecycle chains.

use super::*;
use crate::model::Rule;

const IMPORTS: &str = "import { describe, test, expect, beforeEach, afterEach, beforeAll, afterAll, aroundEach, vi as tools } from 'vitest';";
const TEST: &str = "test('works', () => { expect(value).toBe(1) });";

/// Requires the executable population before selecting hook claims.
fn findings(source: &str, count: usize) -> Vec<crate::model::Finding> {
    let mut corpora = corpora(&[("cases.test.ts", source)], &["cases.test.ts"]);
    let corpus = corpora.remove(0);
    assert_eq!(corpus.tests.len(), count, "{source}");
    owned_findings("cases.test.ts", corpus)
        .findings
        .into_iter()
        .filter(|finding| finding.rule == Rule::RedundantHook)
        .collect()
}

/// One registered pair and a nontrivial consumer of its setup.
fn pair(first: &str, second: &str) -> String {
    format!("{IMPORTS} let value = 0; {first} {second} {TEST}")
}

/// The same owned source runs under Vitest, including two unsafe default-engine positives.
#[test]
fn owned_hook_runtime_controls() {
    let source = include_str!("../../../fixtures/oracle/redundant-hooks/cases.test.ts");
    let result = findings(source, 5);
    assert_eq!(result.len(), 1, "{result:?}");
    assert_eq!(
        result
            .iter()
            .map(|f| f
                .locations
                .iter()
                .map(|l| l.span.start.line)
                .collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        [vec![5, 6]]
    );
    assert!(result.iter().all(|f| f
        .locations
        .iter()
        .all(|l| l.path == std::path::Path::new("cases.test.ts"))));
}

/// Each lifecycle phase works in one scope; only per-test phases cross scope boundaries.
#[test]
fn hook_phase_and_scope_are_dimensions() {
    for phase in ["beforeEach", "afterEach", "beforeAll", "afterAll"] {
        let hook = format!("{phase}(() => {{ value = 1 }});");
        assert_eq!(findings(&pair(&hook, &hook), 1).len(), 1, "{phase}");
        let nested = format!(
            "{IMPORTS} let value = 0; {hook} describe('inner', () => {{ {hook} {TEST} }});"
        );
        assert_eq!(
            findings(&nested, 1).len(),
            usize::from(phase.ends_with("Each")),
            "{phase}"
        );
    }
    let a = "beforeEach(() => { value = 1 });";
    assert!(findings(&pair(a, "afterEach(() => { value = 1 });"), 1).is_empty());
    assert!(findings(&format!("{IMPORTS} let value = 0; describe('a', () => {{ {a} {TEST} }}); describe('b', () => {{ {a} {TEST} }});"), 2).is_empty());
}

/// Different and opaque work block a pair; removing the barrier keeps the positive.
#[test]
fn hook_intervening_effects_and_observers_are_barriers() {
    let hook = "beforeEach(() => { value = 1 });";
    for middle in ["expect(value).toBe(1)", "observe()", ""] {
        let source = pair(hook, &format!("beforeEach(() => {{ {middle} }}); {hook}"));
        assert!(findings(&source, 1).is_empty(), "{middle}");
    }
    let result = findings(&pair(hook, &format!("{hook} {hook}")), 1);
    assert_eq!(result.len(), 3);
    let source = format!(
        "{} test('another', () => {{ expect(value).not.toBe(9) }});",
        pair(hook, hook)
    );
    assert_eq!(findings(&source, 2).len(), 1);
}

/// No unknown effect, allocation, setter or cleanup can borrow a lexical overwrite proof.
#[test]
fn hook_effect_proof_refuses_unknown_work() {
    for body in [
        "value += 1",
        "value = make()",
        "value ||= make()",
        "if (!value) value = make()",
        "state.value = 1",
        "value = {}",
        "return () => { value = 1 }",
        "observe()",
        "value = /x/",
        "value = '\\uD800'",
        "value = 1; observe()",
    ] {
        let hook = format!("beforeEach(() => {{ {body} }});");
        assert!(findings(&pair(&hook, &hook), 1).is_empty(), "{body}");
    }
    for hook in [
        "beforeEach(async () => { value = 1 });",
        "beforeEach((ctx) => { value = 1 });",
        "beforeEach(() => { value = 1 }, 1000);",
        "beforeEach(() => { value = 1 }, { valueOf() { return 1 } });",
        "beforeEach(named);",
    ] {
        assert!(findings(&pair(hook, hook), 1).is_empty(), "{hook}");
    }
    assert!(findings(
        &pair(
            "beforeEach(() => { value = 1 });",
            "beforeEach(() => { value = 2 });"
        ),
        1
    )
    .is_empty());
}

/// Reset calls remain opaque until utility-object stability is modeled.
#[test]
fn hook_resets_need_an_object_stability_proof() {
    for member in [
        "tools.clearAllMocks",
        "tools['clearAllMocks']",
        "tools.resetModules",
        "tools.unstubAllGlobals",
    ] {
        let hook = format!("beforeEach(() => {{ {member}() }});");
        assert!(findings(&pair(&hook, &hook), 1).is_empty(), "{member}");
    }
    let hook = "beforeEach(() => { tools.clearAllMocks() });";
    for extra in [
        "const alias = tools;",
        "tools.clearAllMocks = other;",
        "use(tools);",
        "test('mutates', () => { tools.clearAllMocks = other });",
    ] {
        let source = format!("{} {extra}", pair(hook, hook));
        assert!(
            findings(&source, if extra.starts_with("test") { 2 } else { 1 }).is_empty(),
            "{extra}"
        );
    }
    for callee in [
        "fake.clearAllMocks",
        "tools.clearAllMocks.call",
        "tools.clearAllMocks?.",
        "tools[key]",
    ] {
        let hook = format!("beforeEach(() => {{ {callee}() }});");
        assert!(findings(&pair(&hook, &hook), 1).is_empty(), "{callee}");
    }
}

/// Conditional, indirect and inactive registration never establishes a shared execution chain.
#[test]
fn hook_chain_registration_must_be_known() {
    let hook = "beforeEach(() => { value = 1 });";
    for body in [
        format!("if (true) {{ {hook} {hook} }} {TEST}"),
        format!("unknown(); {hook} {hook} {TEST}"),
        format!("const x = other.member; {hook} {hook} {TEST}"),
        format!("aroundEach(async (run) => {{ await run() }}); {hook} {hook} {TEST}"),
    ] {
        assert!(findings(&format!("{IMPORTS} let value = 0; {body}"), 1).is_empty());
    }
    for callee in [
        "describe.skip",
        "describe.runIf(true)",
        "describe.each([1])",
        "describe.concurrent",
    ] {
        let source =
            format!("{IMPORTS} let value = 0; {callee}('s', () => {{ {hook} {hook} {TEST} }});");
        assert!(findings(&source, 1).is_empty(), "{callee}");
    }
    assert!(findings(&format!("{} describe('elsewhere', () => {{ test.only('focused', () => {{ expect(2).toBe(2) }}) }});", pair(hook, hook)), 2).is_empty());
}

/// Equal-looking declarations in separate scopes still denote different lexical state cells.
#[test]
fn hook_targets_keep_binding_identity() {
    let source = format!("{IMPORTS} let value = 0; beforeEach(() => {{ value = 1 }}); describe('inner', () => {{ let value = 0; beforeEach(() => {{ value = 1 }}); {TEST} }});");
    assert!(findings(&source, 1).is_empty());
}

/// Scope reversal cannot make an apparent source-order neighbour skip an observer.
#[test]
fn hook_cross_scope_ordering_requires_a_uniform_phase() {
    for phase in ["beforeEach", "afterEach"] {
        let source = format!("{IMPORTS} let value = 0; let other = 0; {phase}(() => {{ other = 9 }}); {phase}(() => {{ value = 1 }}); describe('inner', () => {{ {phase}(() => {{ value = 1 }}); {TEST} }});");
        assert!(findings(&source, 1).is_empty(), "{phase}");
    }
}

/// Expression bodies cannot make unexecuted hook registrations unconditional.
#[test]
fn hook_concise_suite_keeps_registration_uncertainty() {
    let source = include_str!("../../../fixtures/oracle/hook-proof-boundaries/concise.test.ts");
    assert!(findings(source, 1).is_empty());
}

/// An erased ambient declaration does not establish a setter-free runtime cell.
#[test]
fn hook_ambient_declaration_is_not_a_lexical_cell() {
    let source = include_str!("../../../fixtures/oracle/hook-proof-boundaries/ambient.test.ts");
    assert!(findings(source, 1).is_empty());
}

/// One ordinary test cannot license deletion in a scope with concurrent descendants.
#[test]
fn hook_concurrent_descendants_refuse_the_population() {
    let hook = "beforeEach(() => { value = 1 });";
    let source = format!("{IMPORTS} let value = 0; {hook} {hook} {TEST} describe('nested', () => {{ test.concurrent('parallel', async () => {{ await Promise.resolve(); value = 2; expect(value).toBe(2) }}) }});");
    assert!(findings(&source, 2).is_empty());
}

/// An eligible sibling cannot hide mutation through an alternate utility import.
#[test]
fn hook_reset_alias_mutation_refuses_the_proof() {
    let source = include_str!("../../../fixtures/oracle/hook-proof-boundaries/reset-alias.test.ts");
    let mut read = corpora(&[("cases.test.ts", source)], &["cases.test.ts"]);
    let corpus = read.remove(0);
    assert_eq!(corpus.tests.len(), 2);
    assert!(corpus.tests[0].exclusion.is_some());
    assert!(corpus.tests[1].exclusion.is_none());
    assert!(owned_findings("cases.test.ts", corpus).findings.is_empty());
}

/// Options and uninspected callbacks cannot change execution outside an anchor's path.
#[test]
fn hook_off_path_registration_must_be_closed() {
    let hook = "beforeEach(() => { value = 1 });";
    for body in ["describe('parallel', { concurrent: true }, () => { test('p', () => { expect(1).toBe(1) }) })", "test('p', { concurrent: true }, () => { expect(1).toBe(1) })", "test('p', { only: true }, () => { expect(1).toBe(1) })"] {
        let source = format!("{IMPORTS} let value = 0; {hook} {hook} {TEST} describe('off path', () => {{ {body} }});");
        assert!(findings(&source, 2).is_empty(), "{body}");
    }
}

/// A discarded-looking call still leaks its utility object when its return value is captured.
#[test]
fn hook_reset_results_cannot_escape() {
    let source =
        include_str!("../../../fixtures/oracle/hook-proof-boundaries/reset-result.test.ts");
    let read = corpora(&[("cases.test.ts", source)], &["cases.test.ts"]);
    assert_eq!(read[0].tests.len(), 3);
    assert!(read[0]
        .tests
        .iter()
        .any(|test| test.exclusion.is_none() && !test.inert));
    assert!(findings(source, 3).is_empty());
}

/// The permanent corpus requires a meaningful lexical positive and an opaque observer silence.
#[test]
fn hook_owned_corpus_retains_lexical_setup() {
    let source =
        include_str!("../../../fixtures/recall/redundant-hook-lexical-overwrite/main.test.ts");
    let result = findings(source, 2);
    assert_eq!(result.len(), 1, "{result:?}");
    assert_eq!(
        result[0]
            .locations
            .iter()
            .map(|location| location.span.start.line)
            .collect::<Vec<_>>(),
        [14, 15]
    );
}

/// Queued writes can run between synchronous hooks even with sequential test scheduling.
#[test]
fn hook_queued_write_cannot_borrow_idempotence() {
    let source =
        include_str!("../../../fixtures/oracle/hook-proof-boundaries/microtask-write.test.ts");
    let read = corpora(&[("cases.test.ts", source)], &["cases.test.ts"]);
    assert_eq!(read[0].tests.len(), 2);
    assert!(read[0]
        .tests
        .iter()
        .any(|test| test.exclusion.is_none() && !test.inert));
    assert!(findings(source, 2).is_empty());
}

/// The complete write set, including helpers and dynamic lexical evaluation, bounds the proof.
#[test]
fn hook_lexical_writes_must_have_one_value() {
    let hook = "beforeEach(() => { value = 1 });";
    for extra in [
        "function later() { value = 2 }",
        "function later() { value++ }",
        "function later() { value = read() }",
        "function later() { eval('value = 2') }",
        "function later() { ({ value } = source) }",
    ] {
        assert!(
            findings(&format!("{} {extra}", pair(hook, hook)), 1).is_empty(),
            "{extra}"
        );
    }
    assert_eq!(
        findings(
            &format!("{} function later() {{ value = 1 }}", pair(hook, hook)),
            1
        )
        .len(),
        1
    );
}

/// Defining an uncalled helper does not register its hook or turn its test into an anchor.
#[test]
fn hook_uncalled_helper_keeps_only_the_ordinary_chain() {
    let hook = "beforeEach(() => { value = 1 });";
    let source = format!(
        "{} function unused() {{ {hook} test('unknown', () => {{ expect(value).toBe(1) }}) }}",
        pair(hook, hook)
    );
    assert_eq!(findings(&source, 2).len(), 1);
}

/// Only actual mutable lexical declarations participate in this bounded effect proof.
#[test]
fn hook_binding_declaration_kind_is_part_of_the_proof() {
    for declaration in ["const value = 0", "var value = 0", "function value() {}"] {
        let source = format!("{IMPORTS} {declaration}; beforeEach(() => {{ value = 1 }}); beforeEach(() => {{ value = 1 }}); {TEST}");
        assert!(findings(&source, 1).is_empty(), "{declaration}");
    }
}
