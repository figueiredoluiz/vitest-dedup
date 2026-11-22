use super::super::{parse, Parsed, SymbolId};
use super::{declared, immutable, initialized, unwritten, Declarations};
use std::path::Path;

fn parsed(path: &str, source: &str) -> Parsed {
    parse(Path::new(path), source).expect("parses")
}

/// The symbol the `nth` identifier spelled `name` binds or reads.
fn symbol(parsed: &Parsed, name: &str, nth: usize) -> SymbolId {
    let tree = &parsed.tree;
    let node = tree
        .nodes()
        .filter(|&node| tree.kind(node) == "Identifier" && tree.text(node, "name") == Some(name))
        .nth(nth)
        .unwrap_or_else(|| panic!("no identifier {name} #{nth}"));
    parsed
        .scopes
        .symbol_at(tree.span(node).start)
        .expect("resolves")
}

#[test]
fn every_write_form_refuses_unwritten() {
    for (source, expected) in [
        ("let a = 1; a;", true),
        ("let a = 1; a = 2;", false),
        ("let a = 1; a += 2;", false),
        ("let a = 1; a ??= 2;", false),
        ("let a = 1; a++;", false),
        ("let a = 1; [a] = [2];", false),
        ("let a = 1; ({ a } = { a: 2 });", false),
        ("let a = 1; for (a of [2]);", false),
        ("let a = 1; for (a in {});", false),
        ("let a = 1; () => { a = 2 };", false),
    ] {
        let parsed = parsed("a.ts", source);
        let a = symbol(&parsed, "a", 0);
        assert_eq!(unwritten(&parsed.scopes, a).is_some(), expected, "{source}");
    }
}

#[test]
fn a_second_runtime_declaration_refuses_immutable() {
    for (path, source, single, occurrences) in [
        ("a.ts", "const a = 1;", true, 1),
        ("a.js", "var a; var a;", false, 2),
        ("a.js", "var a; function a() {}", false, 2),
        ("a.js", "function f(a) { var a; }", false, 2),
        ("a.ts", "enum a { X } enum a { Y }", false, 2),
        // An erased overload signature declares no value.
        ("a.ts", "function a(): void; function a() {}", true, 1),
        // Shorthand destructuring repeats the one binding span.
        ("a.ts", "const { a } = { a: 1 };", true, 2),
    ] {
        let parsed = parsed(path, source);
        let a = symbol(&parsed, "a", 0);
        let found = declared(&parsed.tree, &parsed.scopes, a);
        assert_eq!(found.single(), single, "{source}");
        assert_eq!(found.occurrences, occurrences, "{source}");
        assert_eq!(
            Declarations::of(&parsed.tree, &parsed.scopes).get(a),
            found,
            "{source}"
        );
        assert_eq!(
            immutable(&parsed.scopes, a, found).is_some(),
            single,
            "{source}"
        );
    }
}

/// A body `function x` redeclares the parameter `x` without a write: at run time every read sees
/// the function, so only the declaration count tells the parameter's value is not the argument.
#[test]
fn a_body_function_redeclares_the_parameter() {
    let parsed = parsed("a.js", "(function (x) { function x() {} return x })");
    let parameter = symbol(&parsed, "x", 0);
    assert_eq!(symbol(&parsed, "x", 1), parameter);
    assert!(unwritten(&parsed.scopes, parameter).is_some());
    let found = declared(&parsed.tree, &parsed.scopes, parameter);
    assert!(!found.single());
    assert!(immutable(&parsed.scopes, parameter, found).is_none());
}

#[test]
fn a_symbol_without_a_runtime_binding_is_not_single() {
    let parsed = parsed("a.ts", "const a = 1;");
    let a = symbol(&parsed, "a", 0);
    let none = Declarations::default().get(a);
    assert!(!none.single());
    assert!(immutable(&parsed.scopes, a, none).is_none());
}

/// Later reads prove initialization only when control cannot bypass the declaration.
#[test]
fn initialization_is_proved_from_position_only() {
    for (source, expected) in [
        ("const a = 1; a;", true),
        ("const a = 1, b = a;", true),
        ("export const a = 1, b = a;", true),
        ("export const a = 1; export default a;", true),
        ("export default a; export const a = 1;", false),
        ("export const a = [a];", false),
        ("export declare const a: number; export default a;", false),
        ("namespace N { export const a = 1; a; }", false),
        ("const a = [a];", false),
        ("a; const a = 1;", false),
        ("{ const a = 1; { a; } }", true),
        ("const a = 1; const f = () => a;", true),
        ("for (const a = 1; a < 2;) {}", true),
        // The iterable runs before the loop binds `a`.
        ("for (const a of [a]) {}", false),
        ("for (const a in a) {}", false),
        ("for (const a of [1]) { a; }", true),
        // A later `switch` case can be entered past the declaration.
        (
            "switch (0) { case 0: const a = 1; break; case 1: a; }",
            false,
        ),
        ("switch (0) { case 0: const a = 1; a; }", true),
        // Hoisting can call a function declaration before the declaration runs.
        ("const a = 1; function f() { return a; }", false),
        ("export const a = 1; function f() { return a; }", false),
        ("function f() { const a = 1; return a; }", true),
    ] {
        let parsed = parsed("a.ts", source);
        let tree = &parsed.tree;
        let declarator = tree
            .nodes()
            .find(|&node| tree.kind(node) == "VariableDeclarator")
            .unwrap();
        let a = symbol(&parsed, "a", 0);
        let read = parsed.scopes.symbol(a).references.last().map(|r| r.node);
        let read = read
            .flatten()
            .unwrap_or_else(|| panic!("{source}: no read"));
        assert_eq!(
            initialized(tree, declarator, read).is_some(),
            expected,
            "{source}"
        );
    }
}
