//! A same-file helper called at collection scope registers its tests at the call; anything that
//! may run it elsewhere, or not run what it writes, keeps today's exclusions.

use super::*;
use crate::model::Rule;

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
    // Both tests the helper writes are read and excluded; an empty result would satisfy `all`.
    for title in ["first", "second"] {
        let test = tests
            .get(title)
            .unwrap_or_else(|| panic!("no test {title}: {tests:?}"));
        assert!(test.exclusion.is_some(), "{title} is compared");
    }
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

/// A helper call inside a suite body registers into that suite: the case takes the caller's
/// suite path exactly as an ordinary test written in the same body would, so an expanded case
/// and a hand-written twin key together.
#[test]
fn a_helper_call_inside_a_suite_joins_it() {
    let header = "import { describe, test, expect } from 'vitest'\n";
    let helper = "function makeTest(label: string) {\n  test(`${label} works`, () => {\n    const rows = 3\n    expect(rows).toBe(3)\n  })\n}\n";
    let with_call = format!(
        "{header}{helper}describe('s', () => {{\n  makeTest('twin')\n  test('twin works', () => {{\n    const rows = 3\n    expect(rows).toBe(3)\n  }})\n}})\n"
    );
    let tests = by_title(&with_call);
    let expanded = tests
        .get("twin works")
        .expect("the call's case takes the suite path");
    assert!(expanded.exclusion.is_none());
    assert_eq!(
        expanded.full_title.as_deref(),
        Some("s > twin works"),
        "the case takes the suite path",
    );
    assert_eq!(
        expanded.key, tests["twin works"].key,
        "the hand-written twin in the same suite keys with it",
    );
    // Expanded versus hand-written twin in one suite is a reported pair.
    let findings = owned_findings(
        "a.test.ts",
        corpora(&[("a.test.ts", &with_call)], &["a.test.ts"]).remove(0),
    )
    .findings
    .into_iter()
    .filter(|finding| finding.rule == Rule::DuplicateTest)
    .count();
    assert_eq!(findings, 1);
}

/// A suite-position call of a `const` helper declared after the describe statement runs after
/// the module body, so nothing is uninitialized (measured on Vitest 5.0.5).
#[test]
fn a_suite_position_call_admits_a_later_const_helper() {
    let header = "import { describe, test, expect } from 'vitest'\n";
    let source = format!(
        "{header}describe('s', () => {{\n  makeTest('calls late const')\n}})\nconst makeTest = (label: string) => {{\n  test(`${{label}}`, () => {{\n    expect(1).toBe(1)\n  }})\n}}\n"
    );
    let tests = by_title(&source);
    assert!(tests.contains_key("calls late const"));
    assert!(tests["calls late const"].exclusion.is_none());
    assert_eq!(
        tests["calls late const"].full_title.as_deref(),
        Some("s > calls late const"),
    );
    // The same shape at top level keeps slice 1's refusal: the call meets the dead zone, so no
    // title is proven and every case keeps an exclusion.
    let top = format!(
        "{header}makeTest('top')\nconst makeTest = (label: string) => {{\n  test(`${{label}}`, () => {{\n    expect(1).toBe(1)\n  }})\n}}\n"
    );
    assert!(by_title(&top)
        .values()
        .all(|test| test.exclusion.is_some() && test.title.is_none()));
}

/// Inert labels beside a call inside a suite keep the expansion. A caller that writes a hook, or a
/// call inside a suite whose helper registers one, refuses: hook context reaches handwritten and
/// expanded tests by different paths, and a hook registered in a suite governs every test there.
#[test]
fn labels_keep_the_expansion_and_hooks_refuse_it() {
    let header = "import { describe, test, expect, beforeEach } from 'vitest'\n";
    let reads = "function reads() {\n  test('reads', () => {\n    expect(ready).toBe(1)\n  })\n}\n";
    let labeled = format!(
        "{header}let ready = 1\n{reads}describe('s', () => {{\n  reads()\n  test.skip('skipped', () => {{\n    expect(1).toBe(1)\n  }})\n}})\n"
    );
    let tests = by_title(&labeled);
    assert!(
        tests["reads"].exclusion.is_none(),
        "{:?}",
        tests["reads"].exclusion
    );
    assert_eq!(tests["reads"].full_title.as_deref(), Some("s > reads"));
    assert!(
        tests["skipped"].inert && tests["skipped"].exclusion.is_none(),
        "the inert label marks the participant, it does not exclude it",
    );
    let install = "function install() {\n  beforeEach(() => {\n    ready = 1\n  })\n  test('reads', () => {\n    expect(ready).toBe(1)\n  })\n}\n";
    for (name, source) in [
        (
            "a caller hook",
            format!("{header}let ready = 0\n{reads}describe('s', () => {{\n  beforeEach(() => {{\n    ready = 1\n  }})\n  reads()\n}})\n"),
        ),
        (
            "a top-level caller hook",
            format!("{header}let ready = 0\n{reads}beforeEach(() => {{\n  ready = 1\n}})\ndescribe('s', () => {{\n  reads()\n}})\n"),
        ),
        (
            "a helper hook inside a suite",
            format!("{header}let ready = 0\n{install}describe('s', () => {{\n  install()\n}})\ntest('reads', () => {{\n  expect(ready).toBe(1)\n}})\n"),
        ),
    ] {
        // Only a test the file writes itself, never an expanded one, is compared.
        let corpus = corpora(&[("a.test.ts", &source)], &["a.test.ts"]).remove(0);
        assert!(
            corpus
                .tests
                .iter()
                .filter(|test| test.exclusion.is_none())
                .all(|test| test.origin.is_none() && test.span.start.line > 8),
            "{name}: {:?}",
            corpus.tests.iter().map(|t| (&t.title, t.exclusion, t.span.start.line)).collect::<Vec<_>>()
        );
    }
}

/// Shapes the slice refuses: a call inside a skipped suite, inside a table suite, a suite
/// declared inside the helper, and a loop or an unrenderable title inside the caller suite.
#[test]
fn suite_position_refusals_keep_the_exclusions() {
    let header = "import { describe, test, expect } from 'vitest'\n";
    let helper = "function makeTest(label: string) {\n  test(`${label}`, () => {\n    expect(1).toBe(1)\n  })\n}\n";
    for (name, source) in [
        (
            "skipped suite",
            format!("{header}{helper}describe.skip('s', () => {{\n  makeTest('x')\n}})\n"),
        ),
        (
            "table suite",
            format!("{header}{helper}describe.each([1])('s', () => {{\n  makeTest('x')\n}})\n"),
        ),
        (
            "suite inside the helper",
            format!("{header}function makeSuite() {{\n  describe('inner', () => {{\n    test('deep', () => {{\n      expect(1).toBe(1)\n    }})\n  }})\n}}\nmakeSuite()\n"),
        ),
        (
            "loop inside the suite body",
            format!("{header}{helper}describe('s', () => {{\n  for (const row of [1]) {{\n    makeTest('x')\n  }}\n}})\n"),
        ),
        (
            "unrenderable suite title",
            format!("{header}{helper}const name = makeName()\ndescribe(name, () => {{\n  makeTest('x')\n}})\n"),
        ),
    ] {
        let tests = by_title(&source);
        assert!(
            tests.values().all(|test| test.exclusion.is_some()),
            "{name}: every test keeps an exclusion"
        );
    }
}

/// Two equal-argument calls are two runtime cases of one template: they pair as the duplicates
/// they are, one finding whose members are the two invocations.
#[test]
fn two_equal_calls_in_one_suite_pair() {
    let header = "import { describe, test, expect } from 'vitest'\n";
    let helper = "function makeTest(label: string) {\n  test(`${label} works`, () => {\n    const rows = 3\n    expect(rows).toBe(3)\n  })\n}\n";
    let source = format!(
        "{header}{helper}describe('s', () => {{\n  makeTest('shared')\n  makeTest('shared')\n}})\n"
    );
    let findings = owned_findings(
        "a.test.ts",
        corpora(&[("a.test.ts", &source)], &["a.test.ts"]).remove(0),
    )
    .findings
    .into_iter()
    .filter(|finding| finding.rule == Rule::DuplicateTest)
    .count();
    assert_eq!(findings, 1);
}

/// An imported runner that registers a hook, called inside a suite, refuses: the hook governs every
/// test in that suite, which the expansion's file-wide lifecycle does not express. Called at the top
/// level, the same runner expands.
#[test]
fn an_imported_runner_with_a_hook_inside_a_suite_refuses() {
    let runner = "import { beforeEach, expect, test } from 'vitest'\nlet ready = 0\nexport function install() {\n  beforeEach(() => {\n    ready = 1\n  })\n  test('reads', () => {\n    expect(ready).toBe(1)\n  })\n}\n";
    let expanded = |caller: &str| {
        let caller = format!(
            "import {{ describe }} from 'vitest'\nimport {{ install }} from './runner'\n{caller}"
        );
        corpora(
            &[("runner.ts", runner), ("a.test.ts", &caller)],
            &["a.test.ts"],
        )
        .remove(0)
        .tests
        .iter()
        .any(|test| test.title.as_deref() == Some("reads") && test.exclusion.is_none())
    };
    assert!(expanded("install()\n"), "the top-level call expands");
    assert!(!expanded("describe('s', () => {\n  install()\n})\n"));
}

/// A suite opens its body to the sequence only with exactly a title and a callback that takes
/// nothing: a third argument or a parameter default runs while the suite registers and can throw
/// (Vitest 5.0.5 fails collecting either), so a call inside one refuses the file's expansion.
#[test]
fn a_suite_with_options_or_parameters_refuses_the_expansion() {
    let header = "import { describe, test, expect } from 'vitest'\n";
    let helper =
        "function makeTest() {\n  test('shared', () => {\n    expect(1).toBe(1)\n  })\n}\n";
    let plain = "describe('t', () => {\n  makeTest()\n})\n";
    let expanded = |suite: &str| {
        by_title(&format!("{header}{helper}{suite}{plain}"))
            .values()
            .any(|test| test.exclusion.is_none())
    };
    assert!(
        expanded("describe('s', () => {\n  makeTest()\n})\n"),
        "the control expands"
    );
    for suite in [
        "describe('s', () => {\n  makeTest()\n}, (() => { throw new Error('boom') })())\n",
        "describe('s', () => {\n  makeTest()\n}, 1000)\n",
        "describe('s', ({ missing = (() => { throw new Error('boom') })() }) => {\n  makeTest()\n})\n",
        "describe('s', (context) => {\n  makeTest()\n})\n",
    ] {
        assert!(!expanded(suite), "{suite}");
    }
}

/// A suite that does not open its body to the sequence still runs its callback while the file is
/// collected (a skipped one too), so beside it no call expands: here the callback throws and Vitest
/// collects nothing.
#[test]
fn an_unopened_suite_beside_calls_refuses_the_expansion() {
    let header = "import { describe, test, expect } from 'vitest'\n";
    let helper =
        "function makeTest() {\n  test('shared', () => {\n    expect(1).toBe(1)\n  })\n}\n";
    let calls =
        "describe('a', () => {\n  makeTest()\n})\ndescribe('b', () => {\n  makeTest()\n})\n";
    let expanded = |extra: &str| {
        by_title(&format!("{header}{helper}{extra}{calls}"))
            .values()
            .any(|test| test.exclusion.is_none())
    };
    assert!(expanded(""), "the control expands");
    for extra in [
        "describe.skip('side', () => {\n  throw new Error('boom')\n})\n",
        "describe.each([1])('side %i', () => {\n  throw new Error('boom')\n})\n",
        "describe(String(Math.random()), () => {\n  throw new Error('boom')\n})\n",
    ] {
        assert!(!expanded(extra), "{extra}");
    }
}

/// A suite title proved from a `const` names the expanded tests even with no literal display, so
/// two calls under `left` and `right` do not collide by name.
#[test]
fn a_const_suite_title_prefixes_the_expanded_names() {
    let source = "import { describe, test, expect } from 'vitest'\nconst left = 'left'\nconst right = 'right'\n\
                  function makeTest() {\n  test('shared', () => {\n    expect(1).toBe(1)\n  })\n}\n\
                  describe(left, () => {\n  makeTest()\n})\ndescribe(right, () => {\n  makeTest()\n})\n";
    let corpus = corpora(&[("a.test.ts", source)], &["a.test.ts"]).remove(0);
    let mut names: Vec<_> = corpus
        .tests
        .iter()
        .filter(|test| test.exclusion.is_none())
        .map(|test| test.name.clone())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            Some(vec!["left".to_owned(), "shared".to_owned()]),
            Some(vec!["right".to_owned(), "shared".to_owned()])
        ]
    );
    let collisions = owned_findings("a.test.ts", corpus)
        .findings
        .iter()
        .filter(|finding| finding.rule == Rule::DuplicateTestName)
        .count();
    assert_eq!(collisions, 0);
}

/// A helper a top-level call reaches only through another helper still reads what it reads when
/// that call runs: a `const` declared after it is uninitialized then (Vitest 5.0.5 throws), so the
/// file keeps its exclusions. With no top-level call, a suite-position call runs after the module
/// body and the same helper expands.
#[test]
fn a_nested_helper_reads_late_bindings_when_the_top_level_call_runs() {
    let header = "import { describe, test, expect } from 'vitest'\n";
    let helpers = "function run() {\n  middle()\n}\nfunction middle() {\n  test(label, () => {\n    expect(1).toBe(1)\n  })\n}\n";
    let early = format!("{header}run()\nconst label = 'late'\n{helpers}");
    assert!(by_title(&early)
        .values()
        .all(|test| test.exclusion.is_some()));
    let suite =
        format!("{header}describe('s', () => {{\n  run()\n}})\nconst label = 'late'\n{helpers}");
    let tests = by_title(&suite);
    assert!(
        tests["late"].exclusion.is_none(),
        "{:?}",
        tests["late"].exclusion
    );
}

/// A generator suite callback never runs its body, so a call inside it registers nothing and
/// the file keeps its exclusions.
#[test]
fn a_generator_suite_callback_keeps_the_file_unexpanded() {
    let header = "import { describe, test, expect } from 'vitest'\n";
    let helper = "function makeTest(label: string) {\n  test(`${label}`, () => {\n    expect(1).toBe(1)\n  })\n}\n";
    let source =
        format!("{header}{helper}describe('generator', function* () {{\n  makeTest('x')\n}})\n");
    assert!(by_title(&source)
        .values()
        .all(|test| test.exclusion.is_some()));
}

/// Two identical callers of one runner pair across the files: the invocation identity carries
/// the caller file, so equal byte offsets in two modules do not collapse them.
#[test]
fn identical_callers_pair_across_files() {
    let runner = "import { test, expect } from 'vitest'\nexport function run() {\n  test('from the runner', () => {\n    expect(1).toBe(1)\n  })\n}\n";
    let caller = "import { run } from './runner'\nrun()\n";
    let corpora = corpora(
        &[
            ("runner.ts", runner),
            ("a.test.ts", caller),
            ("b.test.ts", caller),
        ],
        &["a.test.ts", "b.test.ts"],
    );
    assert!(corpora
        .iter()
        .flat_map(|corpus| corpus.tests.iter())
        .all(|test| test.exclusion.is_none()));
    let outcome = owned_outcome(
        corpora
            .into_iter()
            .enumerate()
            .map(|(index, corpus)| (["a.test.ts", "b.test.ts"][index], corpus))
            .collect(),
    );
    assert_eq!(
        outcome
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::CrossFileDuplicate)
            .count(),
        1,
        "{:?}",
        outcome.findings
    );
}

/// The quality gate counts both invocations of one template as the two runtime cases they are.
#[test]
fn the_gate_counts_each_invocation() {
    let header = "import { describe, test, expect } from 'vitest'\n";
    let helper = "function makeTest(label: string) {\n  test(`${label} works`, () => {\n    expect(1).toBe(1)\n  })\n}\n";
    let source = format!(
        "{header}{helper}describe('s', () => {{\n  makeTest('shared')\n  makeTest('shared')\n}})\n"
    );
    let corpus = corpora(&[("a.test.ts", &source)], &["a.test.ts"]).remove(0);
    let outcome = owned_findings("a.test.ts", corpus);
    let finding = outcome
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateTest)
        .expect("the pair fires");
    assert_eq!(finding.cases.len(), 2, "both invocations are counted");
}

/// A nested call is its own execution: a helper that calls another twice registers two runtime
/// cases each time it runs, so two calls of it register four, all counted and paired.
#[test]
fn nested_calls_are_distinct_invocations() {
    let header = "import { describe, test, expect } from 'vitest'\n";
    let helpers = "function run() {\n  test('shared', () => {\n    expect(1).toBe(1)\n  })\n}\nfunction outer() {\n  run()\n  run()\n}\n";
    let cases = |calls: &str| {
        let source = format!("{header}{helpers}describe('s', () => {{\n{calls}}})\n");
        let corpus = corpora(&[("a.test.ts", &source)], &["a.test.ts"]).remove(0);
        let expanded = corpus
            .tests
            .iter()
            .filter(|test| test.exclusion.is_none())
            .count();
        let counted = owned_findings("a.test.ts", corpus)
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateTest)
            .flat_map(|finding| finding.cases.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        (expanded, counted)
    };
    assert_eq!(
        cases("  outer()\n"),
        (2, 2),
        "one outer call still pairs its two runs"
    );
    assert_eq!(cases("  outer()\n  outer()\n"), (4, 4));
}

/// A `using` declaration disposes its value when the suite body ends, which runs code (Vitest
/// 5.0.5 fails collecting a non-disposable one), so a suite body holding one refuses.
#[test]
fn a_using_declaration_in_a_suite_body_refuses() {
    let header = "import { describe, test, expect } from 'vitest'\n";
    let helper = "function run() {\n  test('shared', () => {\n    expect(1).toBe(1)\n  })\n}\n";
    let expanded = |statement: &str| {
        by_title(&format!(
            "{header}{helper}describe('s', () => {{\n  {statement}\n  run()\n  run()\n}})\n"
        ))
        .values()
        .any(|test| test.exclusion.is_none())
    };
    assert!(expanded("const resource = {}"), "the control expands");
    assert!(!expanded("using resource = {}"));
}
