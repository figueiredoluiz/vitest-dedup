//! A same-file helper called at collection scope registers its tests at the call; anything that
//! may run it elsewhere, or not run what it writes, keeps today's exclusions.

use super::*;

/// The cases of a file read on its own, keyed by title. The import is the fixtures' own: one that
/// reaches setup utilities such as `vi` refuses every expansion.
fn cases(source: &str) -> HashMap<String, TestCase> {
    by_title(&format!(
        "import {{ test, expect }} from 'vitest'\n{source}"
    ))
}

const BODY: &str = "() => { expect(1).toBe(1) }";

/// Whether the helper's `hidden` test is compared and keys like `visible`.
fn expanded(source: &str) -> bool {
    let tests = cases(source);
    let hidden = tests.get("hidden");
    hidden.is_some_and(|hidden| hidden.exclusion.is_none() && hidden.key == tests["visible"].key)
}

/// A plain call of a declared helper registers `hidden` there, and the helper's written site is
/// not counted again.
#[test]
fn a_called_helper_registers_its_test_once() {
    let source = format!(
        "function declare() {{ test('hidden', {BODY}) }}\ndeclare()\ntest('visible', {BODY})\n"
    );
    assert!(expanded(&source));
    // Counted on the corpus itself: the written site shares the title `hidden`.
    let file = format!("import {{ test, expect }} from 'vitest'\n{source}");
    let corpus = corpora(&[("a.test.ts", &file)], &["a.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 2);
}

/// A registration under a branch inside the helper may not run, so nothing is expanded.
#[test]
fn a_registration_under_a_branch_keeps_the_helper_unexpanded() {
    let source = format!(
        "const on = true\nfunction declare() {{ if (on) test('hidden', {BODY}) }}\ndeclare()\ntest('visible', {BODY})\n"
    );
    assert!(!expanded(&source));
}

/// A `const` helper is not initialized before the call that runs first: a hoisted function
/// called above the declaration would meet its temporal dead zone.
#[test]
fn a_constant_helper_declared_after_the_first_call_is_not_expanded() {
    let late = format!(
        "run()\nconst declare = () => {{ test('hidden', {BODY}) }}\nfunction run() {{ declare() }}\ntest('visible', {BODY})\n"
    );
    assert!(!expanded(&late));
    let early = format!(
        "const declare = () => {{ test('hidden', {BODY}) }}\nrun()\nfunction run() {{ declare() }}\ntest('visible', {BODY})\n"
    );
    assert!(expanded(&early));
}

/// A function that registers nothing is ordinary code: reading it in a test does not refuse the
/// helper beside it.
#[test]
fn an_ordinary_function_beside_a_helper_does_not_refuse_it() {
    let source = format!(
        "function one() {{ return 1 }}\nfunction declare() {{ test('hidden', {BODY}) }}\ndeclare()\n\
         test('visible', () => {{ expect(one()).toBe(1) }})\n"
    );
    let tests = cases(&source);
    assert!(
        tests["hidden"].exclusion.is_none(),
        "{:?}",
        tests["hidden"].exclusion
    );
}

/// A helper read other than as a callee may run at another time, so nothing is expanded.
#[test]
fn a_helper_read_as_a_value_is_not_expanded() {
    let source = format!(
        "function declare() {{ test('hidden', {BODY}) }}\ndeclare()\n\
         test('visible', () => {{ expect(typeof declare).toBe('function') }})\n"
    );
    assert!(cases(&source)["hidden"].exclusion.is_some());
}

/// A registration in a function that is not a same-file helper (here a `let` arrow) is never
/// dropped as a template, so the file keeps its exclusions.
#[test]
fn a_registration_outside_a_helper_keeps_the_file_unexpanded() {
    let source = format!(
        "let later = () => {{ test('elsewhere', {BODY}) }}\nfunction declare() {{ test('hidden', {BODY}) }}\ndeclare()\ntest('visible', {BODY})\n"
    );
    let tests = cases(&source);
    assert!(tests["hidden"].exclusion.is_some());
    assert!(tests["elsewhere"].exclusion.is_some());
}

/// A helper reached through another helper sets up nothing around the file's tests: they key as
/// they do where the helper is called directly.
#[test]
fn a_nested_helper_call_is_not_file_context() {
    let header = "import { test, expect } from 'vitest'\n";
    let helper = format!("const declare = () => {{ test('hidden', {BODY}) }}\n");
    let nested =
        format!("{header}{helper}function run() {{ declare() }}\nrun()\ntest('visible', {BODY})\n");
    let direct = format!("{header}{helper}declare()\ntest('visible', {BODY})\n");
    let corpora = corpora(
        &[("a.test.ts", &nested), ("b.test.ts", &direct)],
        &["a.test.ts", "b.test.ts"],
    );
    let visible = |corpus: &FileCorpus| {
        corpus
            .tests
            .iter()
            .find(|test| test.title.as_deref() == Some("visible"))
            .map(|test| (test.key, test.module_key))
    };
    assert!(corpora
        .iter()
        .all(|corpus| corpus.tests.iter().all(|test| test.exclusion.is_none())));
    assert_eq!(visible(&corpora[0]), visible(&corpora[1]));
}

/// A constant a helper reads while it registers is read when the first call runs, so it must be
/// initialized by then; a hoisted helper called above it meets its temporal dead zone.
#[test]
fn a_constant_the_helper_reads_must_be_initialized_before_the_first_call() {
    let template = format!("function run() {{ test(`hidden ${{n}}`, {BODY}) }}\n");
    let late = format!("run()\nconst n = 1\n{template}test('visible', {BODY})\n");
    let tests = cases(&late);
    assert!(
        tests["hidden 1"].exclusion.is_some(),
        "{:?}",
        tests["hidden 1"].exclusion
    );
    let early = format!("const n = 1\nrun()\n{template}test('visible', {BODY})\n");
    let tests = cases(&early);
    assert!(tests["hidden 1"].exclusion.is_none());
    assert_eq!(tests["hidden 1"].key, tests["visible"].key);
}

/// An expanded call is a registration, not module setup, in the tests it registers too: a file
/// whose only test comes from a helper keys like a file that writes that test directly.
#[test]
fn an_expanded_helper_call_is_not_module_setup_of_its_own_tests() {
    let header = "import { test, expect } from 'vitest'\n";
    let helper = format!("{header}function declare() {{ test('same', {BODY}) }}\ndeclare()\n");
    let direct = format!("{header}test('same', {BODY})\n");
    let corpora = corpora(
        &[("a.test.ts", &helper), ("b.test.ts", &direct)],
        &["a.test.ts", "b.test.ts"],
    );
    let only = |corpus: &FileCorpus| {
        assert_eq!(corpus.tests.len(), 1);
        assert_eq!(corpus.tests[0].exclusion, None);
        corpus.tests[0].module_key
    };
    assert_eq!(only(&corpora[0]), only(&corpora[1]));
}

/// Whether a helper with `body` (which registers `hidden`) is expanded, beside `prelude`.
fn body_expanded(prelude: &str, body: &str) -> bool {
    expanded(&format!(
        "{prelude}\nfunction declare() {{ {body} }}\ndeclare()\ntest('visible', {BODY})\n"
    ))
}

/// A `try` whose `finally` runs nothing leaves the block's registrations as they are; a `finally`
/// that calls code, or a `catch`, may run what the reader does not model.
#[test]
fn a_try_with_an_inert_finally_is_read_as_its_block() {
    let hidden = format!("test('hidden', {BODY})");
    assert!(body_expanded(
        "",
        &format!("try {{ {hidden} }} finally {{ for (;;) {{ break }} }}")
    ));
    assert!(body_expanded(
        "",
        &format!("try {{ return {hidden} }} finally {{ }}")
    ));
    // Each refusal calls a helper that registers `extra`: admitted, the call would never run.
    let o = "function other() { test('extra', () => { expect(1).toBe(1) }); return 1 }";
    assert!(!body_expanded(
        o,
        &format!("try {{ {hidden} }} finally {{ other() }}")
    ));
    assert!(!body_expanded(
        "",
        &format!("try {{ {hidden} }} catch {{ }} finally {{ }}")
    ));
    assert!(!body_expanded(
        o,
        &format!("try {{ {hidden} }} finally {{ for (other(); ; ) {{ break }} }}")
    ));
    assert!(!body_expanded(
        o,
        &format!("try {{ {hidden} }} finally {{ for (; other(); ) {{ break }} }}")
    ));
}

/// A `const` holding a registration is that registration; a `let` is not admitted.
#[test]
fn a_constant_holding_a_registration_is_that_registration() {
    let hidden = format!("test('hidden', {BODY})");
    assert!(body_expanded("", &format!("const registered = {hidden}")));
    assert!(!body_expanded("", &format!("let registered = {hidden}")));
}

/// Building a registration function is not admitted, even unused and from literals: building one
/// sets state on the shared test chain, and table builders coerce their arguments. Nor is a
/// `const` holding any other call.
#[test]
fn a_built_registration_function_is_not_admitted() {
    let hidden = format!("test('hidden', {BODY})");
    for declaration in [
        "const each = test.each([1, 2])",
        "const each = test.skipIf(false).each([1, 2])",
        "const tagged = test.each`v\n${1}`",
        "const rows = test.for([1]).for([2])",
        "const scoped = test.scoped({ n: 1 })",
    ] {
        assert!(
            !body_expanded("", &format!("{declaration}\n{hidden}")),
            "{declaration}"
        );
    }
    let throws = "function fail() { throw new Error('collected') }";
    assert!(!body_expanded(
        throws,
        &format!("const value = fail()\n{hidden}")
    ));
}

/// The in-source binding is not admitted in a helper: an imported module may not receive it, and
/// a hoisted function can read its names before it is initialized.
#[test]
fn the_in_source_binding_is_not_admitted_in_a_helper() {
    let source = "const register = () => {\nconst { test, expect } = import.meta.vitest\n\
                  test('first', () => { expect(1).toBe(1) })\ntest('second', () => { expect(1).toBe(1) })\n}\nregister()\n";
    let tests = by_title(source);
    assert!(tests.values().all(|test| test.exclusion.is_some()));
}

/// An overloaded helper runs its implementation; reassigning it refuses.
#[test]
fn an_overloaded_helper_runs_its_implementation() {
    let template =
        format!("function declare(): void\nfunction declare() {{ test('hidden', {BODY}) }}\n");
    assert!(expanded(&format!(
        "{template}declare()\ntest('visible', {BODY})\n"
    )));
    assert!(!expanded(&format!(
        "{template}declare = () => {{}}\ndeclare()\ntest('visible', {BODY})\n"
    )));
}
