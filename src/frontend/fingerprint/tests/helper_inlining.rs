use super::*;

// Imported-helper inlining: an imported helper's body participates in `alpha`, so two tests
// differing only by calling equivalent helpers compare by what the calls do (see
// `helpers.rs`).
// ---------------------------------------------------------------------------------------

/// Writes one helper file into `dir`, the shared setup half of every test below.
fn write_helper(dir: &std::path::Path, relative: &str, content: &str) {
    let path = dir.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// Fingerprints a one-test source that calls `import_specifier`'s export `export_name`
/// with `argument`, and returns that test's `alpha`. The shared exercise half: every test
/// below asks the same question (what did the call render to) and differs only in what is on
/// disk. The import is aliased to one local name so the call site is byte-identical across
/// every test, whatever the export is called.
fn alpha_of_helper_call(
    dir: &std::path::Path,
    import_specifier: &str,
    export_name: &str,
    argument: &str,
) -> String {
    let source = format!(
        "import {{ {export_name} as callme }} from '{import_specifier}';\n             test('x', () => {{ callme({argument}); }});"
    );
    let prints = fingerprint_all_at(&source, &dir.join("a.test.ts"));
    assert_eq!(
        prints.len(),
        1,
        "expected exactly one test body for {source:?}"
    );
    prints[0].alpha.clone()
}

/// Depth threading: a call site near [`MAX_DEPTH`] leaves the inlined helper no second
/// budget to spend, so its body degrades (`<max-depth:...>`) instead of rendering fully
/// from depth zero -- keeping the in-use stack within one pass's measured bound.
#[test]
fn a_helper_called_near_the_depth_budget_degrades_instead_of_double_spending_it() {
    let dir = tempfile::tempdir().unwrap();
    write_helper(
        dir.path(),
        "helpers.ts",
        "export function check(label) { return label.length > 0; }",
    );

    // 125 array levels put the call site near [`MAX_DEPTH`]: still reachable (the wrapper
    // renders), but with the caller's depth threaded into the helper's serialization there is
    // no second full budget to spend, so the helper body degrades -- its `<number:0>` literal
    // is gone. 120 levels leave enough budget to render it. Without threading, the helper's
    // statements restarted at depth 0 and rendered fully from any call site.
    for (levels, expect_full_body) in [(120, true), (125, false)] {
        let source = format!(
            "import {{ check }} from './helpers';\ntest('x', () => {{ const deep = {}check('dashboard'){}; }});",
            "[".repeat(levels),
            "]".repeat(levels),
        );
        let prints = fingerprint_all_at(&source, &dir.path().join("a.test.ts"));
        assert_eq!(prints.len(), 1);
        let alpha = &prints[0].alpha;
        assert!(
            alpha.contains("helper-inline"),
            "at {levels} levels the call is still reachable and must inline, got {alpha}"
        );
        assert_eq!(
            alpha.contains("<number:0>"),
            expect_full_body,
            "at {levels} levels the helper body's literal {} render",
            if expect_full_body { "must" } else { "must not" }
        );
    }
}

/// The core claim: two helpers, in two different modules, spelled with entirely different
/// names, whose bodies are alpha-equivalent -- two tests calling them with the same argument
/// must produce the same `alpha`. Before inlining, the call sites rendered as two different
/// `free:import` identities and the pair could never match.
#[test]
fn equivalent_imported_helpers_inline_to_the_same_alpha() {
    let dir = tempfile::tempdir().unwrap();
    write_helper(
        dir.path(),
        "helpers-a.ts",
        "import { expect } from 'vitest';\n\
             export function checkActive(label: string) {\n\
                 const trimmed = label.trim();\n\
                 expect(trimmed.length).toBeGreaterThan(0);\n\
             }",
    );
    write_helper(
        dir.path(),
        "helpers-b.ts",
        "import { expect } from 'vitest';\n\
             export function assertNonEmpty(text: string) {\n\
                 const cleaned = text.trim();\n\
                 expect(cleaned.length).toBeGreaterThan(0);\n\
             }",
    );

    let a = alpha_of_helper_call(dir.path(), "./helpers-a", "checkActive", "'dashboard'");
    let b = alpha_of_helper_call(dir.path(), "./helpers-b", "assertNonEmpty", "'dashboard'");
    assert!(
        a.contains("helper-inline"),
        "the call must render the helper's body, got {a}"
    );
    assert_eq!(
        a, b,
        "two tests differing only by calling equivalent imported helpers must produce the \
             same alpha"
    );
}

/// The one-edit negative twin: helper B's body differs by one literal, and that difference
/// must survive inlining -- the alphas differ, which is what keeps the equivalent case's
/// equality meaningful.
#[test]
fn inlined_helper_bodies_keep_their_distinguishing_literals() {
    let dir = tempfile::tempdir().unwrap();
    write_helper(
        dir.path(),
        "helpers-a.ts",
        "export function check(label) { return label.length > 0; }",
    );
    write_helper(
        dir.path(),
        "helpers-b.ts",
        "export function assertNonEmpty(text) { return text.length > 1; }",
    );

    let a = alpha_of_helper_call(dir.path(), "./helpers-a", "check", "'dashboard'");
    let b = alpha_of_helper_call(dir.path(), "./helpers-b", "assertNonEmpty", "'dashboard'");
    // Prove the pair actually inlined before trusting the inequality: if a refusal ever
    // regressed both sides back to identity rendering, they would still differ (two
    // different exports) and the assertion below would pass vacuously -- review-found shape.
    assert!(
        a.contains("helper-inline") && b.contains("helper-inline"),
        "both sides must render the helper's body for the inequality to mean anything, got \
             {a} / {b}"
    );
    assert_ne!(
        a, b,
        "a literal difference inside an inlined helper body must stay visible in alpha"
    );
}

/// A written parameter (`x = 5`, `x += 1`, `x++`, a destructuring write) makes
/// substitution unsound: the write renders as a canonical local while later reads still
/// render the original argument, so `f(x) { x = 5; return x; }` called `f(1)` and
/// `g(x) { x = 5; return 1; }` called `g(1)` would render identically while returning 5
/// and 1 (review-found false merge at Error severity). The whole export is refused and
/// both calls keep their identity rendering.
#[test]
fn helpers_that_write_a_parameter_refuse_inlining_and_stay_distinct() {
    let dir = tempfile::tempdir().unwrap();
    write_helper(
        dir.path(),
        "writes-then-reads.ts",
        "export function f(x) { x = 5; return x; }",
    );
    write_helper(
        dir.path(),
        "writes-then-literal.ts",
        "export function g(x) { x = 5; return 1; }",
    );

    let f = alpha_of_helper_call(dir.path(), "./writes-then-reads", "f", "1");
    let g = alpha_of_helper_call(dir.path(), "./writes-then-literal", "g", "1");
    assert!(
        f.contains("free:import") && !f.contains("helper-inline"),
        "a helper that writes its parameter keeps the identity rendering, got {f}"
    );
    assert!(
        g.contains("free:import") && !g.contains("helper-inline"),
        "got {g}"
    );
    assert_ne!(
        f, g,
        "the review pair must stay distinct: they return 5 and 1 for the same argument"
    );
}

/// An argument no parameter read consumes is still observable at runtime -- directly
/// ignored here, but indexed through `arguments` there -- so it must stay visible in the
/// rendered wrapper. Two calls to the same helper differing only in an UNREAD argument
/// would otherwise render identically and falsely merge (review-found).
#[test]
fn unread_arguments_stay_visible_in_the_inlined_wrapper() {
    let dir = tempfile::tempdir().unwrap();
    write_helper(
        dir.path(),
        "ignores-second.ts",
        "export function check(label, unused) { return label.length > 0; }",
    );
    write_helper(
        dir.path(),
        "indexes-arguments.ts",
        "export function check(label, second) { return arguments[1].length > 0; }",
    );

    // The ignored-argument shape: same helper, differing only in the argument no read
    // consumes -- distinct, and the difference is visible as an `(args ...)` tail.
    let a = alpha_of_helper_call(dir.path(), "./ignores-second", "check", "'a', 'x'");
    let b = alpha_of_helper_call(dir.path(), "./ignores-second", "check", "'a', 'y'");
    assert!(
        a.contains("(args"),
        "the unread argument must render in an args tail, got {a}"
    );
    assert_ne!(
        a, b,
        "two calls differing only in an unread argument must not render alike"
    );

    // The `arguments` channel: the body reads neither parameter by name, so without the
    // tail both calls would render identically while returning different values.
    let indexed_a = alpha_of_helper_call(dir.path(), "./indexes-arguments", "check", "'a', 'x'");
    let indexed_b = alpha_of_helper_call(dir.path(), "./indexes-arguments", "check", "'a', 'y'");
    assert_ne!(
        indexed_a, indexed_b,
        "a difference observable only through `arguments` must still separate the calls"
    );

    // And the equal twin: identical arguments, both shapes, stay comparable to themselves.
    assert_eq!(
        a,
        alpha_of_helper_call(dir.path(), "./ignores-second", "check", "'a', 'x'")
    );
}

/// `structural` never inlines: `parameterization-candidate`'s remedy is a table in the *test
/// file*, which cannot parameterize literals living inside helper modules. Pinned here so
/// the alpha-only scope is a tested contract, not a comment.
#[test]
fn structural_keeps_the_identity_rendering_for_imported_helpers() {
    let dir = tempfile::tempdir().unwrap();
    write_helper(
        dir.path(),
        "helpers.ts",
        "export function check(label) { return label; }",
    );
    let source = "import { check } from './helpers';\ntest('x', () => { check('dashboard'); });";
    let prints = fingerprint_all_at(source, &dir.path().join("a.test.ts"));
    assert_eq!(prints.len(), 1);
    assert!(
        prints[0].alpha.contains("helper-inline"),
        "alpha inlines the helper body"
    );
    assert!(
        prints[0].structural.contains("free:import"),
        "structural keeps the export-identity rendering, got {}",
        prints[0].structural
    );
    assert!(
        !prints[0].structural.contains("helper-inline"),
        "structural must never inline an imported helper"
    );
}

/// The framework itself is never inlined, even when `vitest` genuinely resolves to a real,
/// inlinable-looking export on disk: a call to `expect` is not "the test containing the
/// framework's assertions", and the identity must not depend on whatever build is installed.
#[test]
fn vitest_specifier_is_never_inlined() {
    let dir = tempfile::tempdir().unwrap();
    write_helper(
        dir.path(),
        "node_modules/vitest/package.json",
        r#"{"name":"vitest","main":"index.js"}"#,
    );
    write_helper(
        dir.path(),
        "node_modules/vitest/index.js",
        "export function expect(actual) { return { toBe(expected) { return actual === expected; } }; }",
    );

    let source = "import { expect as check } from 'vitest';\ntest('x', () => { check(1); });";
    let prints = fingerprint_all_at(source, &dir.path().join("a.test.ts"));
    assert_eq!(prints.len(), 1);
    assert!(
        !prints[0].alpha.contains("helper-inline"),
        "a call through a 'vitest' import must keep its identity rendering even when the \
             module resolves to a real, callable export, got {}",
        prints[0].alpha
    );
    assert!(
        prints[0].alpha.contains("free:import"),
        "the framework call renders as a free-import identity"
    );
}

/// Exact arity is an admission requirement: a call whose arguments cannot bind
/// parameter-for-parameter keeps the identity rendering, because there is no argument whose
/// rendering could stand in for the unbound parameter.
#[test]
fn arity_mismatch_is_not_inlined() {
    let dir = tempfile::tempdir().unwrap();
    write_helper(
        dir.path(),
        "helpers.ts",
        "export function check(label, minimum) { return label.length > minimum; }",
    );
    let alpha = alpha_of_helper_call(dir.path(), "./helpers", "check", "'dashboard'");
    assert!(
        alpha.contains("free:import"),
        "an arity-mismatched call keeps the identity rendering, got {alpha}"
    );
}

/// `export const name = <arrow>` is an admitted shape, and the wrapper names the callable
/// form (`arrow-fn`) because `this` binding differs from a `function` even with identical
/// statements.
#[test]
fn const_arrow_helpers_inline_with_their_form_named() {
    let dir = tempfile::tempdir().unwrap();
    write_helper(
        dir.path(),
        "helpers.ts",
        "export const check = (label) => label.length > 0;",
    );
    let alpha = alpha_of_helper_call(dir.path(), "./helpers", "check", "'dashboard'");
    assert!(
        alpha.contains("(helper-inline arrow-fn"),
        "an inlined const-arrow names its form, got {alpha}"
    );
}

/// A named re-export hop (`export { x } from './inner'`) resolves to the inner declaration:
/// the helper's body participates even when the public module only re-exports it -- the
/// barrel shape real repositories use for shared runners.
#[test]
fn reexported_helpers_inline_through_the_chain() {
    let dir = tempfile::tempdir().unwrap();
    write_helper(
        dir.path(),
        "helpers-a.ts",
        "export { check } from './inner-a';",
    );
    write_helper(
        dir.path(),
        "inner-a.ts",
        "export function check(label) { return label.trim().length > 0; }",
    );
    write_helper(
        dir.path(),
        "helpers-b.ts",
        "export { assertNonEmpty } from './inner-b';",
    );
    write_helper(
        dir.path(),
        "inner-b.ts",
        "export function assertNonEmpty(text) { return text.trim().length > 0; }",
    );

    let a = alpha_of_helper_call(dir.path(), "./helpers-a", "check", "'dashboard'");
    let b = alpha_of_helper_call(dir.path(), "./helpers-b", "assertNonEmpty", "'dashboard'");
    assert!(
        a.contains("helper-inline"),
        "a re-exported helper's body must still be reached, got {a}"
    );
    assert_eq!(
        a, b,
        "equivalent helpers behind equivalent re-export chains must produce the same alpha"
    );
}

/// The reviewer's own probe, pinned: `return x + y` against `return y + y`, called with
/// two distinct caller locals. The fragments differ (`id:L0`+`id:L1` vs `id:L1` twice),
/// so the alphas differ; the namespacing below makes this true by construction rather
/// than by allocation order.
#[test]
fn param_pairing_survives_substitution_in_both_orders() {
    let dir = tempfile::tempdir().unwrap();
    write_helper(
        dir.path(),
        "sums.ts",
        "export function p(x, y) { return x + y; }\nexport function q(x, y) { return y + y; }",
    );
    let p = "import { p as callme } from './sums';\ntest('x', () => { const a = 1; const b = 2; callme(a, b); });";
    let q = "import { q as callme } from './sums';\ntest('x', () => { const a = 1; const b = 2; callme(a, b); });";
    let p_prints = fingerprint_all_at(p, &dir.path().join("p.test.ts"));
    let q_prints = fingerprint_all_at(q, &dir.path().join("q.test.ts"));
    assert_eq!(p_prints.len(), 1);
    assert_eq!(q_prints.len(), 1);
    assert!(
        p_prints[0].alpha.contains("helper-inline") && q_prints[0].alpha.contains("helper-inline"),
        "both sides must inline for the comparison to mean anything"
    );
    assert_ne!(
        p_prints[0].alpha, q_prints[0].alpha,
        "x + y and y + y over distinct caller locals must not render alike"
    );
}

/// Helper-local canonical names and caller-local names in substituted arguments must not
/// collide: the helper's own locals and the caller's locals are numbered by *separate*
/// passes, and a false equality here would merge two different tests. One side reads a
/// parameter substituted with a caller local (`id:L0` from the caller's pass); the other
/// declares and reads its own local (`id:L0` from the helper's own pass, plus the
/// declaration the caller's side cannot have). They must stay distinct.
#[test]
fn helper_locals_and_caller_locals_render_without_colliding() {
    let dir = tempfile::tempdir().unwrap();
    write_helper(
        dir.path(),
        "reads-param.ts",
        "export function f(x) { return x; }",
    );
    write_helper(
        dir.path(),
        "reads-local.ts",
        "export function g() { const x = 1; return x; }",
    );

    let with_caller_local = r#"import { f } from './reads-param';
test('x', () => { const u = 1; f(u); });"#;
    let with_helper_local = r#"import { g } from './reads-local';
test('x', () => { g(); });"#;
    let a = fingerprint_all_at(with_caller_local, &dir.path().join("a.test.ts"));
    let b = fingerprint_all_at(with_helper_local, &dir.path().join("b.test.ts"));
    assert_eq!(a.len(), 1);
    assert_eq!(b.len(), 1);
    assert_ne!(
        a[0].alpha, b[0].alpha,
        "a parameter substituted with a caller local and a helper's own local must not \
             render alike just because both canonical names spell `id:L0`"
    );
    // The mechanism, made observable: the helper's own locals render in their own
    // namespace, distinct by construction from every caller-local name in the fragments.
    assert!(
        b[0].alpha.contains("id:H0"),
        "a helper-declared local renders in the helper namespace, got {}",
        b[0].alpha
    );

    // The equal twin must stay equal: same helper, differently named caller local -- both
    // arguments canonicalize to the same caller-side name, so the alphas agree.
    let renamed = r#"import { f } from './reads-param';
test('x', () => { const w = 1; f(w); });"#;
    let c = fingerprint_all_at(renamed, &dir.path().join("c.test.ts"));
    assert_eq!(c.len(), 1);
    assert_eq!(
        a[0].alpha, c[0].alpha,
        "renaming the caller local passed as an argument must not change the alpha"
    );
}
