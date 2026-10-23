use super::*;

// Constructs only reachable inside a class body
// -----------------------------------------------------------------------------------------
//
// `ser_expr` renders a class as opaque raw text, so a private field or `super` can only be
// reached by a test registered *inside* a class method -- lexically within the class body,
// where those constructs are legal, but in a callback this module does fingerprint. Real
// suites do this (a test helper class, a describe-per-class layout), and the whole
// `PrivateFieldExpression`/`Super` family of arms is otherwise unreachable.

/// Every position a private field reference can take is modeled, with its optional link kept.
#[test]
fn a_private_field_reference_is_modeled_everywhere_it_can_appear() {
    let in_class = |body: &str| {
        let source =
            format!("class C {{ #p = 1; #m() {{}} run() {{ test('t', () => {{ {body} }}); }} }}");
        let prints = fingerprint_all(&source);
        assert_eq!(prints.len(), 1, "expected one test body for {body:?}");
        prints.into_iter().next().expect("one body")
    };

    // `ser_expr` -> `ser_private_field`
    assert!(
        in_class("const v = this.#p;")
            .alpha
            .contains("(member this .#p)"),
        "a private field read must be modeled, not rendered as raw text"
    );
    // `ser_assignment_target` and `ser_simple_assignment_target`
    assert!(
        in_class("this.#p = 1;")
            .alpha
            .contains("(assign = (member this .#p) <number:1>)"),
        "a private field write target must be modeled"
    );
    assert!(
        in_class("this.#p++;")
            .alpha
            .contains("(update ++ false (member this .#p))"),
        "a private field update target must be modeled"
    );
    // `ser_chain` -> `ser_private_field`; the optional link is part of the text, so it cannot
    // collapse into the plain read above.
    assert!(
        in_class("const v = this?.#p;")
            .alpha
            .contains("(member this ?.#p)"),
        "an optional private field read must be modeled"
    );
    // `ser_for_left` -> `ser_private_field`
    assert!(
        in_class("for (this.#p in o) 1;")
            .alpha
            .contains("(for-in (member this .#p) "),
        "a private field as a for-in target must be modeled"
    );
    // `walk_expr` and `call_path`
    assert!(
        in_class("this.#m(f());")
            .behavior
            .iter()
            .any(|event| event.starts_with("call:this.#m args=")),
        "a private method call must flatten into the event's path"
    );
    assert!(
        in_class("g(this.#m(f()));")
            .behavior
            .contains(&"call:global:f".to_owned()),
        "the walk must descend through a private field's object"
    );
    assert!(
        in_class("this?.#m();")
            .behavior
            .contains(&"call:this.#m".to_owned()),
        "...including through an optional chain"
    );
    // `ser_private_field`'s own depth guard, landed on exactly: a private field cannot be
    // written outside a class body, so it has no row in `LANDED_GUARDS`. `ser_expr` reaches
    // the construct at depth `4 + padding` and hands it down one frame.
    // pinned from both sides, since a `>=` guard here would otherwise be invisible.
    // `n` unary links put the private field's own `ser_expr` at depth `4 + n`,
    // `ser_private_field` at `5 + n` and its `this` object at `6 + n`. At `n = MAX_DEPTH - 6`
    // the deepest of those three lands on `MAX_DEPTH` exactly, so nothing may degrade.
    let inside = (MAX_DEPTH - 6) as usize;
    let serialized = in_class(&format!("const v = {}this.#p;", "!".repeat(inside))).alpha;
    assert!(
        serialized.contains("(member this .#p)") && !serialized.contains("<max-depth:"),
        "at depth MAX_DEPTH exactly the private field must still be serialized whole. \
             Tail: {}",
        &serialized[serialized.len().saturating_sub(80)..]
    );
    // One deeper, `ser_private_field` itself sits on `MAX_DEPTH`: it must still run -- and
    // its `this` object, now one past the cap, is what degrades. This is the single point
    // where a `>=` guard here is observable, so it is asserted on content rather than on the
    // presence of a marker.
    let boundary = in_class(&format!("const v = {}this.#p;", "!".repeat(inside + 1))).alpha;
    assert!(
        boundary.contains("(member <max-depth:this> .#p)"),
        "on MAX_DEPTH exactly the private field is serialized and only its object degrades. \
             Tail: {}",
        &boundary[boundary.len().saturating_sub(80)..]
    );
    // One deeper again, the private field is itself past the cap.
    let degraded = in_class(&format!("const v = {}this.#p;", "!".repeat(inside + 2))).alpha;
    assert!(
        degraded.contains("<max-depth:this.#p>"),
        "past the cap it degrades to its own verbatim source. Tail: {}",
        &degraded[degraded.len().saturating_sub(80)..]
    );

    // `expression_has_effect`'s own private-field arm: a pure private read must not block
    // the inlining of a single-use local.
    let inlined = in_class("const s = build(1); const p = this.#p; expect(s).toBe(3);");
    let event = inlined
        .behavior
        .iter()
        .find(|event| event.starts_with("assert:"))
        .expect("assertion event");
    assert!(
        event.contains("subject=(call id:global:build <number>)"),
        "reading a private field has no effect, so it must not block inlining: {event}"
    );
}

#[test]
fn super_is_modeled_in_both_serialization_and_call_paths() {
    let in_subclass = |body: &str| {
        let source = format!(
            "class B {{}} class C extends B {{ run() {{ test('t', () => {{ {body} }}); }} }}"
        );
        let prints = fingerprint_all(&source);
        assert_eq!(prints.len(), 1);
        prints.into_iter().next().expect("one body")
    };
    assert!(
        in_subclass("const v = super.x;")
            .alpha
            .contains("(member super .x)"),
        "`super` must serialize as itself, not as raw text"
    );
    assert!(
        in_subclass("super.x();")
            .behavior
            .contains(&"call:super.x".to_owned()),
        "`super` must flatten into a call path"
    );
}

// -----------------------------------------------------------------------------------------

// Declaration forms, import forms, and the remaining fallbacks
// -----------------------------------------------------------------------------------------

#[test]
fn every_variable_declaration_kind_is_named_in_the_fingerprint() {
    for (kind, body) in [
        ("var", "var a = 1;"),
        ("let", "let a = 1;"),
        ("const", "const a = 1;"),
        ("using", "using a = r;"),
    ] {
        let alpha = alpha_of(body);
        assert!(
            alpha.contains(&format!("({kind} (decl ")),
            "`{body}` must be serialized as a `{kind}` declaration: {alpha}"
        );
    }
    let alpha = async_alpha_of("await using a = r;");
    assert!(
        alpha.contains("(await-using (decl "),
        "`await using` is its own declaration kind: {alpha}"
    );

    // The same five kinds are re-derived for a `for` statement's own init clause.
    for (kind, body) in [
        ("var", "for (var i = 1;;) 1;"),
        ("let", "for (let i = 1;;) 1;"),
        ("const", "for (const i = 1;;) 1;"),
        ("using", "for (using i = r;;) 1;"),
    ] {
        let alpha = alpha_of(body);
        assert!(
            alpha.contains(&format!("(for ({kind} (decl ")),
            "`{body}`'s init must be serialized as a `{kind}` declaration: {alpha}"
        );
    }
    let alpha = async_alpha_of("for (await using i = r;;) 1;");
    assert!(
        alpha.contains("(for (await-using (decl "),
        "`for (await using ...)` is its own kind: {alpha}"
    );
}

/// A destructuring default and an unmodeled for-of target each keep their own shape.
#[test]
fn a_destructuring_default_and_an_unmodeled_for_target_keep_their_own_shapes() {
    assert!(
        alpha_of("const [a = 1] = arr;").contains("(default "),
        "a defaulted element of a destructuring declaration is modeled"
    );
    assert!(
        alpha_of("const {a = 1} = o;").contains("(default "),
        "...and so is a defaulted destructured property"
    );
    // A destructuring for-of target (reassigning existing bindings, not declaring) is rare
    // in test bodies; raw text is the deliberate fallback.
    assert!(
        alpha_of("let a; for ([a] of o) 1;").contains("<opaque:[a]|a=id:L0>"),
        "an unmodeled for-of target falls back to verbatim source, tied to the variable it names"
    );
}

/// Every import form resolves to a free identity that depends on the module, not on the local name.
#[test]
fn every_import_form_yields_a_stable_free_identity() {
    // A default import's exported name is `default`, not the local binding's name, so two
    // files importing the same module under different local names still agree.
    let prints = fingerprint_all(
        "import d from './m';\ntest('a', () => { g(d); });\n\
             import x from './m';\ntest('b', () => { g(x); });",
    );
    assert_eq!(prints.len(), 2);
    assert_eq!(
        prints[0].alpha, prints[1].alpha,
        "two default imports of one module must share an identity whatever they are named"
    );

    // A string export name (`import { 'a-b' as ab }`) is a name no identifier can spell.
    let prints = fingerprint_all(
        "import { 'a-b' as ab } from './m';\ntest('a', () => { g(ab); });\n\
             import { 'c-d' as cd } from './m';\ntest('b', () => { g(cd); });",
    );
    assert_ne!(
        prints[0].alpha, prints[1].alpha,
        "two different string export names from one module are different identities"
    );

    // A namespace import is not a named export and carries no exported-name identity.
    let prints = fingerprint_all(
        "import * as ns from './m';\ntest('a', () => { g(ns); });\n\
             import * as other from './m';\ntest('b', () => { g(other); });",
    );
    assert_eq!(
        prints[0].alpha, prints[1].alpha,
        "two namespace imports of one module are the same free reference"
    );
}

#[test]
fn a_literal_containing_a_backslash_is_escaped_rather_than_ambiguous() {
    // `escape_literal_delimiters` escapes `\` as well as `<` and `>`: without it a payload
    // ending in a raw backslash could pair with a following escaped delimiter.
    let plain = alpha_of(r"const v = 'a\\b';");
    assert!(
        plain.contains(r"<string:a\\b>"),
        "a backslash in a literal's value must be escaped: {plain}"
    );
    // And the escaping is injective: a literal that spells out an escaped delimiter cannot
    // collide with one that contains the delimiter itself.
    let prints = fingerprint_all(
        "test('a', () => { const v = 'x\\\\<y'; });\n\
             test('b', () => { const v = 'x\\\\\\\\<y'; });",
    );
    assert_ne!(
        prints[0].alpha, prints[1].alpha,
        "two distinct backslash payloads must not collide"
    );
}

#[test]
fn a_quoted_escape_inside_a_type_argument_is_carried_through_verbatim() {
    // The `\` branch of `normalize_type_argument_text`'s quote scanner: the escaped
    // character is consumed with its backslash, so an escaped quote does not end the string
    // and the whitespace after it is still normalized.
    let with_escape = behavior_of(r"expect(v).toEqual<{ a: 'p\'q', b: 1 }>(1);");
    let compact = behavior_of(r"expect(v).toEqual<{a:'p\'q',b:1}>(1);");
    assert_eq!(
        with_escape, compact,
        "an escaped quote must not derail the normalizer's quote tracking"
    );
    let different = behavior_of(r"expect(v).toEqual<{a:'pq',b:1}>(1);");
    assert_ne!(
        with_escape, different,
        "...while the escaped content itself is still part of the type"
    );
}

// -----------------------------------------------------------------------------------------

// First-declaration reads
// -----------------------------------------------------------------------------------------
//
// `symbol_declaration` answers from a symbol's first declaration alone, and the class side
// reads it to claim what a name holds. A binding that is reassigned, or that has more than one
// declaration which runs (a class merged with a value-bearing namespace), does not necessarily
// hold its first declaration, so the claim must degrade to uncertainty instead.

/// `class C` + `namespace C` is one symbol whose declarations both run: serializing the
/// reference from the first declaration alone reports a class state the namespace extends.
#[test]
fn a_class_merged_with_a_value_namespace_marks_the_reference_unresolved() {
    let merged = fingerprint_all(
        "test('t', () => { class C { read() { return 1 } } namespace C { export const tag = 'x' } \
         const k = C; expect(new k().read()).toBe(1) });",
    );
    assert_eq!(merged.len(), 1);
    assert!(
        merged[0].class_unresolved,
        "a class merged with a value-bearing namespace must not serialize as the bare first \
         class: {}",
        merged[0].alpha
    );

    // The namespace holding only types does not run, so the class is the whole value and no
    // uncertainty is owed.
    let type_only = fingerprint_all(
        "test('t', () => { class C { read() { return 1 } } namespace C { export type T = 1 } \
         const k = C; expect(new k().read()).toBe(1) });",
    );
    assert_eq!(type_only.len(), 1);
    assert!(
        !type_only[0].class_unresolved,
        "a type-only namespace adds nothing that runs: {}",
        type_only[0].alpha
    );
}

/// A redeclared capture is not read from its first initializer either: the write history that
/// `value_at` honors covers assignments, and a later `var` is a declaration, not a write.
#[test]
fn a_redeclared_capture_is_not_read_from_its_first_initializer() {
    let redeclared = fingerprint_all(
        "test('t', () => { var n = 1; var n = 2; class C { read() { return n } } \
         const c = new C(); expect(c.read()).toBe(2) });",
    );
    assert_eq!(redeclared.len(), 1);
    assert!(
        redeclared[0].class_unresolved,
        "a capture whose binding is redeclared must not read the first initializer: {}",
        redeclared[0].alpha
    );

    let plain = fingerprint_all(
        "test('t', () => { var n = 1; class C { read() { return n } } \
         const c = new C(); expect(c.read()).toBe(1) });",
    );
    assert_eq!(plain.len(), 1);
    assert!(
        !plain[0].class_unresolved,
        "without the redeclaration the initializer is what the capture holds: {}",
        plain[0].alpha
    );
}
