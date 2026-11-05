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

/// Scheduling can make identical hook writes observable without rewriting their target cell.
#[test]
fn queued_observation_prevents_hook_removal() {
    let source = include_str!("../../../fixtures/oracle/hook-scheduling/cases.test.ts");
    assert!(findings(source, 1).is_empty());
    let safe = include_str!("../../../fixtures/oracle/hook-scheduling/safe.test.ts");
    assert_eq!(findings(safe, 1).len(), 1);
    let hook = "beforeEach(() => { value = 1 });";
    assert_eq!(findings(&pair(hook, hook), 1).len(), 1);
    for body in [
        "const later = queueMicrotask; later(() => {}); expect(value).toBe(1)",
        "schedule(); expect(value).toBe(1)",
        "Promise.resolve().then(named); expect(value).toBe(1)",
        "expect(value).resolves.toBe(1)",
        "import('node:fs'); expect(value).toBe(1)",
    ] {
        let source =
            format!("{IMPORTS} let value = 0; {hook} {hook} test('runtime', () => {{ {body} }});");
        assert!(findings(&source, 1).is_empty(), "{body}");
    }
    let opaque_assertion = format!("{IMPORTS} import check from 'node:assert/strict'; let value = 0; {hook} {hook} test('opaque assertion', () => {{ check(value); }});");
    assert!(findings(&opaque_assertion, 1).is_empty());
    let bounded_chain = format!("{IMPORTS} let value = 0; {hook} {hook} test('bounded assertion', () => {{ expect(value){}.toBe(1); }});", ".not".repeat(80));
    assert!(findings(&bounded_chain, 1).is_empty());
}

/// Erased declarations preserve the same lexical proof at file and suite scope.
#[test]
fn erased_types_do_not_hide_lexical_hook_pairs() {
    let hook = "beforeEach(() => { value = 1 });";
    for declaration in [
        "interface Shape { value: string }",
        "type Shape = string;",
        "declare const unrelated: string;",
        "declare function unused(): void;",
        "declare class Unused {}",
        "declare enum Unused { A }",
        "declare namespace Unused { type T = string }",
        "export interface Shape {}",
        "export default interface Shape {}",
        "interface Shape {} export { type Shape };",
        "export type Shape = string;",
        "export type { Shape } from './missing';",
        "export type * from './missing';",
        "import type { Shape } from './missing';",
    ] {
        let source = format!("{IMPORTS} {declaration} let value = 0; {hook} {hook} {TEST}");
        assert_eq!(findings(&source, 1).len(), 1, "{declaration}");
        if !declaration.contains("export") && !declaration.starts_with("import") {
            let nested = format!("{IMPORTS} describe('scope', () => {{ {declaration} let value = 0; {hook} {hook} {TEST} }});");
            assert_eq!(findings(&nested, 1).len(), 1, "nested {declaration}");
        }
    }
}

/// Runtime enum evaluation remains an opaque registration barrier.
#[test]
fn runtime_declarations_do_not_borrow_erased_hook_admission() {
    let hooks = "beforeEach(() => { value = 1 }); beforeEach(() => { value = 1 });";
    for declaration in [
        "enum Active { A = observe() }",
        "namespace Active { observe(); }",
        "class Active { static value = observe(); }",
        "export default observe();",
        "interface Shape {} let active = 1; export { type Shape, active };",
        "export { type Shape } from './missing';",
        "export {};",
    ] {
        assert!(findings(
            &format!("{IMPORTS} {declaration} let value = 0; {hooks} {TEST}"),
            1
        )
        .is_empty());
    }
}

/// Imported runners erase declarations in their module and registration body consistently.
#[test]
fn imported_hook_runners_preserve_erased_declarations() {
    let helper = "import { beforeEach } from 'vitest'; export interface Shape {} export type Name = string; let value = 0; export function install() { interface Local {} type Label = string; beforeEach(() => { value = 1 }); beforeEach(() => { value = 1 }); }";
    let caller = "import { install } from './helper'; import { test, expect } from 'vitest'; install(); test('direct', () => { expect(2).toBe(2); });";
    let corpus = corpora(
        &[("helper.ts", helper), ("cases.test.ts", caller)],
        &["cases.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 1);
    assert!(corpus.tests.iter().all(|test| test.exclusion.is_none()));
    let outcome = owned_findings("cases.test.ts", corpus);
    assert_eq!(
        outcome
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::RedundantHook)
            .count(),
        1
    );
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

/// Owned Vitest counterexamples to a reset proof: each file passes with both reset hooks and fails
/// with one, through a route that replaces the utility object's member or refills a registry between
/// the hooks, so neither hook may be reported.
#[test]
fn owned_reset_counterexamples_report_no_pair() {
    for source in [
        include_str!("../../../fixtures/oracle/hook-proof-boundaries/reset-dynamic-import.test.ts"),
        include_str!("../../../fixtures/oracle/hook-proof-boundaries/reset-value-of.test.ts"),
        include_str!("../../../fixtures/oracle/hook-proof-boundaries/reset-import-actual.test.ts"),
        include_str!("../../../fixtures/oracle/hook-proof-boundaries/reset-mock-method.test.ts"),
        include_str!("../../../fixtures/oracle/hook-proof-boundaries/reset-queued-stub.test.ts"),
        include_str!("../../../fixtures/oracle/hook-proof-boundaries/reset-meta-computed.test.ts"),
        include_str!("../../../fixtures/oracle/hook-proof-boundaries/reset-result.test.ts"),
        include_str!("../../../fixtures/oracle/hook-proof-boundaries/reset-alias.test.ts"),
    ] {
        let tests = source.matches("\ntest(").count();
        assert!(findings(source, tests).is_empty(), "{source}");
    }
    let mut corpora = corpora(
        &[
            (
                "cases.test.ts",
                include_str!(
                    "../../../fixtures/oracle/hook-proof-boundaries/reset-helper-load.test.ts"
                ),
            ),
            (
                "reset-helper.ts",
                include_str!("../../../fixtures/oracle/hook-proof-boundaries/reset-helper.ts"),
            ),
        ],
        &["cases.test.ts"],
    );
    let corpus = corpora.remove(0);
    assert_eq!(corpus.tests.len(), 2);
    assert!(owned_findings("cases.test.ts", corpus)
        .findings
        .iter()
        .all(|finding| finding.rule != Rule::RedundantHook));
}

/// The reset pair in this owned file is redundant under Vitest (it passes with one hook and fails
/// with none), and stays unreported while reset calls are opaque.
#[test]
fn an_owned_redundant_reset_pair_stays_unreported() {
    let source = include_str!("../../../fixtures/oracle/redundant-hooks/reset-stub.test.ts");
    assert!(findings(source, 2).is_empty());
}

/// Each lifecycle phase works in one scope; per-test phases cross scope boundaries, and a
/// `beforeAll` does too when its written cells cannot change between the scopes.
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
            usize::from(phase != "afterAll"),
            "{phase}"
        );
    }
    let a = "beforeEach(() => { value = 1 });";
    assert!(findings(&pair(a, "afterEach(() => { value = 1 });"), 1).is_empty());
    assert!(findings(&format!("{IMPORTS} let value = 0; describe('a', () => {{ {a} {TEST} }}); describe('b', () => {{ {a} {TEST} }});"), 2).is_empty());
}

/// Cross-scope `beforeAll` pairs only over constant lexical writes; other effects re-dirty
/// between the scopes.
#[test]
fn hook_cross_scope_before_all_needs_invariant_state() {
    let hook = "beforeAll(() => { value = 1 });";
    let nested = |body: &str| {
        format!("{IMPORTS} let value = 0; {body} describe('inner', () => {{ {body} test('works', () => {{ expect(value).toBe(1) }}); }});")
    };
    assert_eq!(findings(&nested(hook), 1).len(), 1);
    // A second, differing write to the cell breaks the single-value proof.
    let reassigned = format!(
        "{IMPORTS} let value = 0; {hook} test('mutates', () => {{ value = 2; expect(value).toBe(2) }}); describe('inner', () => {{ {hook} {TEST} }});"
    );
    assert!(findings(&reassigned, 2).is_empty());
    // Property effects do not cross scopes: their receiver's state is not proven invariant.
    let property = "beforeAll(() => { audit.mode = 1 });";
    let receiver = format!(
        "{IMPORTS} const audit = {{ mode: 0 }}; {property} describe('inner', () => {{ {property} test('works', () => {{ expect(audit.mode).toBe(1) }}); }});"
    );
    assert!(findings(&receiver, 1).is_empty());
    // Reset effects stay refused everywhere.
    let reset = "beforeAll(() => { tools.unstubAllEnvs() });";
    assert!(findings(&nested(reset), 1).is_empty());
}

/// An own-property write over a proven plain receiver is proven setup; every route around the
/// proof refuses. Hazard routes sit in a never-called function: inert to scope closure, outside
/// every runtime callback, yet still part of the module-wide write set.
#[test]
fn hook_property_writes_pair_over_a_proven_receiver() {
    let receiver = "const audit = { mode: '' };";
    let hook = "beforeEach(() => { audit.mode = 'primary' });";
    let source = |extra: &str| {
        format!("{IMPORTS} {receiver} {extra} {hook} {hook} test('reads', () => {{ expect(audit.mode).toBe('primary') }});")
    };
    assert_eq!(findings(&source(""), 1).len(), 1);
    let route = |statement: &str| format!("function route() {{ {statement} }}");
    // A hook over the receiver and a hook over a lexical cell are different work.
    let cell = "beforeEach(() => { value = 1 });";
    assert!(findings(
        &format!("{IMPORTS} let value = 0; {receiver} {hook} {cell} {TEST}"),
        1
    )
    .is_empty());
    // The receiver escapes as a value.
    assert!(findings(&source(&route("return audit;")), 1).is_empty());
    // The receiver is reassigned.
    assert!(findings(&source(&route("audit = { mode: 'x' };")), 1).is_empty());
    // A delete on the receiver removes the state the hook establishes.
    assert!(findings(&source(&route("delete audit.mode;")), 1).is_empty());
    // A computed write keeps the property unknown.
    let computed = "beforeEach(() => { audit[key] = 'primary' });";
    assert!(findings(
        &format!("{IMPORTS} {receiver} {computed} {computed} {TEST}"),
        1
    )
    .is_empty());
    // An accessor in the literal makes the write call user code.
    let accessor = "const audit = { get mode() { return '' } };";
    assert!(findings(&format!("{IMPORTS} {accessor} {hook} {hook} {TEST}"), 1).is_empty());
    // A deeper path goes through an unproven intermediate.
    let deeper = "beforeEach(() => { audit.inner.mode = 'primary' });";
    assert!(findings(&format!("{IMPORTS} {receiver} {deeper} {deeper} {TEST}"), 1).is_empty());
    // A compound write to the tracked path breaks the single-value invariant.
    assert!(findings(&source(&route("audit.mode += 'x';")), 1).is_empty());
    // An increment to the tracked path is a write the proof cannot value.
    assert!(findings(&source(&route("audit.mode++;")), 1).is_empty());
    // A prototype pollution route makes the assignment call user code.
    assert!(findings(
        &source(&route(
            "Object.defineProperty(Object.prototype, 'mode', { set(v) {} });"
        )),
        1
    )
    .is_empty());
    // The remaining define-property routes refuse the same way, one route per module.
    for pollution in [
        "Object.defineProperties(Object.prototype, { mode: { set(v) {} } });",
        "Reflect.set(Object.prototype, 'mode', 1);",
        "Object.prototype.__defineSetter__('mode', () => {});",
    ] {
        assert!(
            findings(&source(&route(pollution)), 1).is_empty(),
            "{pollution}"
        );
    }
    // A `__proto__` key in the literal gives the receiver a foreign prototype.
    let foreign = "const audit = { __proto__: base, mode: '' };";
    assert!(findings(&format!("{IMPORTS} {foreign} {hook} {hook} {TEST}"), 1).is_empty());
    // A quoted key is still a static data property: the pair fires.
    let quoted = "const audit = { 'mode': '' };";
    let quoted_hook = "beforeEach(() => { audit.mode = 'primary' });";
    assert_eq!(
        findings(
            &format!("{IMPORTS} {quoted} {quoted_hook} {quoted_hook} test('reads', () => {{ expect(audit.mode).toBe('primary') }});"),
            1
        )
        .len(),
        1
    );
    // A `declare`d receiver has no initializer to prove.
    let declared = "declare const audit: { mode: string };";
    assert!(findings(&format!("{IMPORTS} {declared} {hook} {hook} {TEST}"), 1).is_empty());
    // A receiver local to the hook callback is fresh per run, not shared setup.
    let local = "beforeEach(() => { const audit = { mode: '' }; audit.mode = 'primary' });";
    assert!(findings(&format!("{IMPORTS} {local} {local} {TEST}"), 1).is_empty());
    // A non-literal write on the tracked path has no value to keep.
    assert!(findings(&source(&route("audit.mode = compute();")), 1).is_empty());
    // A computed access on the receiver anywhere keeps its use unknown.
    assert!(findings(&source(&route("audit[key] = 'x';")), 1).is_empty());
    // A `__proto__` segment replaces the receiver's prototype.
    assert!(findings(&source(&route("audit.__proto__.x = 1;")), 1).is_empty());
    // Differing values are a barrier, not a pair.
    let other = "beforeEach(() => { audit.mode = 'secondary' });";
    assert!(findings(&format!("{IMPORTS} {receiver} {hook} {other} {TEST}"), 1).is_empty());
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
        "[value] = [1]",
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
        "describe.concurrent",
    ] {
        let source =
            format!("{IMPORTS} let value = 0; {callee}('s', () => {{ {hook} {hook} {TEST} }});");
        assert!(findings(&source, 1).is_empty(), "{callee}");
    }
    assert!(findings(&format!("{} describe('elsewhere', () => {{ test.only('focused', () => {{ expect(2).toBe(2) }}) }});", pair(hook, hook)), 2).is_empty());
}

/// A proven suite table is an ordinary lifecycle scope; unproven rows and dead tables refuse.
#[test]
fn suite_table_hooks_pair_when_rows_are_proven() {
    let hook = "beforeEach(() => { value = 1 });";
    let table = |rows: &str| {
        format!("{IMPORTS} let value = 0; describe.each({rows})('s', () => {{ {hook} {hook} {TEST} }});")
    };
    // One finding per declaration pair, whatever the row count.
    assert_eq!(findings(&table("[1, 2]"), 2).len(), 1);
    assert_eq!(findings(&table("[1]"), 1).len(), 1);
    // Unread rows: the population is unknown, so no chain and no pair.
    assert!(findings(&table("make()"), 1).is_empty());
    // Proven-empty rows: the hooks never register and neither does the test.
    assert!(findings(&table("[]"), 0).is_empty());
    // Differing bodies are not a pair.
    let mixed = format!(
        "{IMPORTS} let value = 0; describe.each([1])('s', () => {{ {hook} beforeEach(() => {{ value = 2 }}); {TEST} }});"
    );
    assert!(findings(&mixed, 1).is_empty());
    // A table inside a loop keeps its refusal, like any conditional registration.
    let looped = format!(
        "{IMPORTS} let value = 0; for (const row of [1, 2]) {{ describe.each([1])('s', () => {{ {hook} {hook} {TEST} }}); }}"
    );
    assert!(findings(&looped, 1).is_empty());
    // A hook in the table and an identical one in a nested ordinary describe pair across scopes.
    let nested = format!(
        "{IMPORTS} let value = 0; describe.each([1])('s', () => {{ {hook} describe('inner', () => {{ {hook} {TEST} }}); }});"
    );
    assert_eq!(findings(&nested, 1).len(), 1);
    // A tagged-template table and a suite wrapped in a loop keep the ordinary refusal.
    for shape in [
        format!("{IMPORTS} let value = 0; describe.each`row ${{1}}`(() => {{ {hook} {hook} {TEST} }});"),
        format!("{IMPORTS} let value = 0; for (const row of [1]) {{ describe('s', () => {{ {hook} {hook} {TEST} }}); }}"),
    ] {
        assert!(findings(&shape, 1).is_empty());
    }
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
