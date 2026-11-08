use super::{Exports, Normalizer, Row, Setting, Structure, TableRow};
use crate::framework::vitest::DIALECT;
use crate::inventory::{inventory, Inventory, Modules, SiteKind};
use crate::lang::{parse, NodeId, Tree};
use std::collections::HashMap;
use std::fs;

mod registration;

/// Scalar readers keep a canonical owned string even when a pair joins across operands.
#[test]
fn nc65_scalar_concatenation_keeps_canonical_code_units() {
    let source = include_str!("../../fixtures/oracle/lossless-strings/joined.test.ts");
    with_normalizer(source, &[], false, |normalizer, tree, _| {
        let read = tree
            .nodes()
            .filter(|&node| {
                tree.kind(node) == "Identifier" && tree.text(node, "name") == Some("key")
            })
            .last()
            .unwrap();
        let Some(crate::lang::Scalar::String(value)) = normalizer.row_scalar(read) else {
            panic!("proved primitive")
        };
        assert_eq!(value, "😀");
    });
}

/// Literal and local reads avoid the import graph; the first import still proves and caches it.
#[test]
fn nc64_import_safety_waits_for_an_import_binding() {
    let source = include_str!("../../fixtures/oracle/row-value-tables/literal-imports.test.ts");
    let values = include_str!("../../fixtures/oracle/row-value-tables/scalar-values.ts");
    with_normalizer(
        source,
        &[("scalar-values.ts", values)],
        false,
        |normalizer, tree, _| {
            let read = |name| {
                tree.nodes()
                    .filter(|&node| {
                        tree.kind(node) == "Identifier" && tree.text(node, "name") == Some(name)
                    })
                    .last()
                    .unwrap()
            };
            let literal = tree
                .nodes()
                .find(|&node| tree.kind(node) == "Literal" && tree.text(node, "raw") == Some("1"))
                .unwrap();
            let identity = |normalizer: &mut Normalizer, node| {
                normalizer.row_scalar(node).map(|v| v.identity())
            };
            assert_eq!(identity(normalizer, literal).as_deref(), Some("number:1"));
            assert_eq!(
                identity(normalizer, read("local")).as_deref(),
                Some("number:1")
            );
            assert_eq!(normalizer.scalar_imports_safe, None);
            assert_eq!(
                identity(normalizer, read("NUMBER")).as_deref(),
                Some("number:1")
            );
            assert_eq!(normalizer.scalar_imports_safe, Some(true));
            assert_eq!(
                identity(normalizer, read("NUMBER")).as_deref(),
                Some("number:1")
            );
        },
    );
}

/// Runs `with` on a normalizer over `main` (written as `a.test.tsx`) in a project holding `files`,
/// with the tree and the inventory it was read with.
fn with_normalizer<T>(
    main: &str,
    files: &[(&str, &str)],
    globals: bool,
    with: impl FnOnce(&mut Normalizer, &Tree, &Inventory) -> T,
) -> T {
    crate::testing::project(main, files, globals, |_, parsed, found, normalizer| {
        with(normalizer, &parsed.tree, found)
    })
}

/// The normal form of each test callback of `main`, by title, in a project holding `files`.
fn forms(main: &str, files: &[(&str, &str)]) -> HashMap<String, String> {
    with_normalizer(main, files, false, |normalizer, tree, inventory| {
        test_callbacks(tree, inventory)
            .map(|(title, callback)| (title, normalizer.of(callback)))
            .collect()
    })
}

/// Each test of `inventory` by title, with its callback (its last argument).
fn test_callbacks<'a>(
    tree: &'a Tree,
    inventory: &'a Inventory,
) -> impl Iterator<Item = (String, NodeId)> + 'a {
    inventory
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(crate::framework::Registers::Test))
        .map(|site| {
            let callback = tree
                .list(site.node, "arguments")
                .into_iter()
                .flatten()
                .last()
                .unwrap();
            (site.title.clone().unwrap(), callback)
        })
}

/// The forms of the test registered at `site`: one per table row (`None` for a row that cannot
/// bind), or its callback's alone when it has no table.
fn site_forms(normalizer: &mut Normalizer, tree: &Tree, site: NodeId) -> Vec<Option<String>> {
    let callee = tree.child(site, "callee").unwrap();
    let modifiers = normalizer.modifiers(callee, None);
    let callback = tree
        .list(site, "arguments")
        .into_iter()
        .flatten()
        .last()
        .unwrap();
    let parameters: Vec<_> = tree
        .list(callback, "params")
        .into_iter()
        .flatten()
        .collect();
    match modifiers.rows {
        None => vec![Some(normalizer.of(callback))],
        Some(rows) => rows
            .iter()
            .map(|row| {
                let (bound, every) = normalizer.bind_row(&parameters, &row.value)?;
                Some(normalizer.substituted(callback, bound, every))
            })
            .collect(),
    }
}

/// Runs `with` on a thread with half the stack a test thread gets, the budget a form is written in.
fn on_half_stack<T: Send + 'static>(with: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(with)
        .unwrap()
        .join()
        .unwrap()
}

/// Asserts the tests titled in `equal` share a normal form and those in `differ` do not.
fn check(main: &str, files: &[(&str, &str)], equal: &[(&str, &str)], differ: &[(&str, &str)]) {
    let forms = forms(main, files);
    let form = |title: &str| {
        forms
            .get(title)
            .unwrap_or_else(|| panic!("no test {title}"))
    };
    for (left, right) in equal {
        assert_eq!(
            form(left),
            form(right),
            "{left} and {right} should be equal"
        );
    }
    for (left, right) in differ {
        assert_ne!(form(left), form(right), "{left} and {right} should differ");
    }
}

const HEADER: &str = "import { test, expect } from 'vitest'\n";

/// TypeScript-only wrappers and annotations are erased before the code runs.
#[test]
fn typescript_wrappers_on_runtime_code_are_erased() {
    check(
        &format!(
            "{HEADER}test('a', () => {{ const y: number = (x as number)!; expect(y satisfies number).toBe(1) }})\n\
             test('b', () => {{ const y = x; expect(y).toBe(1) }})\n\
             test('c', () => {{ const y = x; expect(y).toBe(2) }})"
        ),
        &[],
        &[("a", "b")],
        &[("b", "c")],
    );
}

/// Type-level declarations are what a type test checks, so they stay.
#[test]
fn type_level_declarations_are_kept() {
    check(
        &format!(
            "{HEADER}test('a', () => {{ type T = {{ a: string }}; expectTypeOf<T>().toBeObject() }})\n\
             test('b', () => {{ type T = {{ a: string }}; expectTypeOf<T>().toBeObject() }})\n\
             test('c', () => {{ type T = {{ a: number }}; expectTypeOf<T>().toBeObject() }})\n\
             test('d', () => {{ interface I {{ a?: string }}; check<I>() }})\n\
             test('e', () => {{ interface I {{ a: string }}; check<I>() }})"
        ),
        &[],
        &[("a", "b")],
        &[("b", "c"), ("d", "e")],
    );
}

/// A parameter decorator runs, so it stays.
#[test]
fn parameter_decorators_are_kept() {
    check(
        &format!(
            "{HEADER}test('a', () => {{ class A {{ constructor(@Inject() x) {{}} }}; expect(make(A)).toBeTruthy() }})\n\
             test('b', () => {{ class A {{ constructor(@Inject() x) {{}} }}; expect(make(A)).toBeTruthy() }})\n\
             test('c', () => {{ class A {{ constructor(x) {{}} }}; expect(make(A)).toBeTruthy() }})"
        ),
        &[],
        &[("a", "b")],
        &[("b", "c")],
    );
}

/// A local's name is unobservable unless the body reads a function's name or prints it in a snapshot.
#[test]
fn locals_are_renamed_unless_a_name_is_observed() {
    check(
        &format!(
            "{HEADER}test('a', () => {{ const first = 1; expect(first).toBe(1) }})\n\
             test('b', () => {{ const other = 1; expect(other).toBe(1) }})\n\
             test('c', () => {{ function f() {{}}; expect(f.name).toBe('f') }})\n\
             test('d', () => {{ function g() {{}}; expect(g.name).toBe('f') }})\n\
             test('e', () => {{ class F {{}}; expect(new F()).toMatchSnapshot() }})\n\
             test('f', () => {{ class G {{}}; expect(new G()).toMatchSnapshot() }})"
        ),
        &[],
        &[("a", "b")],
        &[("c", "d"), ("e", "f")],
    );
}

/// A private name means one class's field, whatever it is spelled.
#[test]
fn private_names_are_scoped_per_class() {
    check(
        &format!(
            "{HEADER}test('a', () => {{ class A {{ #x = 1; #y = 2; get() {{ return this.#x }} }}; expect(new A().get()).toBe(1) }})\n\
             test('b', () => {{ class A {{ #p = 1; #q = 2; get() {{ return this.#p }} }}; expect(new A().get()).toBe(1) }})\n\
             test('c', () => {{ class A {{ #p = 1; #q = 2; get() {{ return this.#q }} }}; expect(new A().get()).toBe(1) }})"
        ),
        &[],
        &[("a", "b")],
        &[("b", "c")],
    );
}

/// Every spelling of one literal key names the same property, except `__proto__`.
#[test]
fn literal_property_keys_are_canonical() {
    check(
        &format!(
            "{HEADER}test('a', () => {{ expect({{ 'k': 1, 1: 2 }}.k).toBe(1) }})\n\
             test('b', () => {{ expect({{ k: 1, '1': 2 }}['k']).toBe(1) }})\n\
             test('c', () => {{ expect({{ ['k']: 1, [1]: 2 }}.k).toBe(1) }})\n\
             test('d', () => {{ expect({{ j: 1, 1: 2 }}.k).toBe(1) }})\n\
             test('e', () => {{ expect({{ __proto__: p }}).toBe(1) }})\n\
             test('f', () => {{ expect({{ ['__proto__']: p }}).toBe(1) }})\n\
             test('g', () => {{ expect(o[1e-7]).toBe(1) }})\n\
             test('h', () => {{ expect(o['0.0000001']).toBe(1) }})\n\
             test('i', () => {{ expect(o['1e-7']).toBe(1) }})\n\
             test('j', () => {{ expect(o[true]).toBe(1) }})\n\
             test('k', () => {{ expect(o[false]).toBe(1) }})"
        ),
        &[],
        &[("a", "b"), ("a", "c"), ("g", "i")],
        &[("a", "d"), ("e", "f"), ("g", "h"), ("j", "k")],
    );
}

/// A function that never reads its own `this` runs as an arrow does; a concise arrow returns its body.
#[test]
fn a_plain_function_is_an_arrow_and_a_concise_arrow_returns() {
    check(
        &format!(
            "{HEADER}test('a', function () {{ expect(xs.map((x) => x + 1)).toEqual([2]) }})\n\
             test('b', () => {{ expect(xs.map((x) => {{ return x + 1 }})).toEqual([2]) }})\n\
             test('c', function () {{ expect(this.xs).toEqual([2]) }})\n\
             test('d', () => {{ expect(this.xs).toEqual([2]) }})\n\
             test('e', () => {{ expect(xs.map((x) => {{ x + 1 }})).toEqual([2]) }})\n\
             test('f', () => {{ const f = function () {{}}; expect(new f()).toBeTruthy() }})\n\
             test('g', () => {{ const f = () => {{}}; expect(new f()).toBeTruthy() }})\n\
             test('h', () => {{ expect(construct(function () {{}})).toBeTruthy() }})\n\
             test('i', () => {{ expect(construct(() => {{}})).toBeTruthy() }})"
        ),
        &[],
        &[("a", "b")],
        &[("c", "d"), ("b", "e"), ("f", "g"), ("h", "i")],
    );
}

/// JSX text renders with its layout whitespace collapsed; an empty template is its string.
#[test]
fn jsx_whitespace_and_empty_templates_are_what_they_render() {
    check(
        &format!(
            "{HEADER}test('a', () => {{ render(<p>\n    hello\n    world\n  </p>); expect(`x`).toBe('y') }})\n\
             test('b', () => {{ render(<p>hello world</p>); expect('x').toBe('y') }})\n\
             test('c', () => {{ render(<p>hello  world</p>); expect('x').toBe('y') }})\n\
             test('d', () => {{ render(<p>hello world</p>); expect(`x${{1}}`).toBe('y') }})"
        ),
        &[],
        &[("a", "b")],
        &[("b", "c"), ("b", "d")],
    );
}

/// A `const` literal is that literal only while its value fits the text limit, measured in value
/// bytes: an escaped value whose form is longer still fits. Past it the constant is named by its
/// declaration: equal long constants still match each other, but no longer the literal spelled out.
#[test]
fn a_long_const_literal_is_named_by_its_declaration() {
    let fits = "0".repeat(crate::lang::limits::MAX_TEXT_BYTES);
    let long = format!("{fits}0");
    let escaped = r"\x01".repeat(1_000);
    check(
        &format!(
            "{HEADER}const A = '{fits}'\n\
             const B = '{long}'\n\
             const C = '{long}'\n\
             const E = '{escaped}'\n\
             const T = `{long}`\n\
             const U = `{long}`\n\
             test('a', () => {{ expect(A).toBe(1) }})\n\
             test('a-spelled', () => {{ expect('{fits}').toBe(1) }})\n\
             test('b', () => {{ expect(B).toBe(1) }})\n\
             test('c', () => {{ expect(C).toBe(1) }})\n\
             test('b-spelled', () => {{ expect('{long}').toBe(1) }})\n\
             test('e', () => {{ expect(E).toBe(1) }})\n\
             test('e-spelled', () => {{ expect('{escaped}').toBe(1) }})\n\
             test('t', () => {{ expect(T).toBe(1) }})\n\
             test('u', () => {{ expect(U).toBe(1) }})\n\
             test('t-spelled', () => {{ expect(`{long}`).toBe(1) }})"
        ),
        &[],
        &[
            ("a", "a-spelled"),
            ("b", "c"),
            ("e", "e-spelled"),
            ("t", "u"),
        ],
        &[("b", "b-spelled"), ("t", "t-spelled")],
    );
}

/// What a test reads outside itself is named by its declaration, never by its spelling.
#[test]
fn identities_follow_declarations() {
    check(
        &format!(
            "{HEADER}import {{ helper }} from './helper'\n\
             import data from './data.json'\n\
             const N = 1\n\
             const config = {{ a: 1 }}\n\
             const same = {{ a: 1 }}\n\
             const spy = vi.fn()\n\
             const twin = vi.fn()\n\
             let counter = 0\n\
             let other = 0\n\
             function local() {{ return 1 }}\n\
             test('literal', () => {{ expect(N).toBe(1) }})\n\
             test('spelled', () => {{ expect(1).toBe(1) }})\n\
             test('config', () => {{ expect(config).toBeTruthy() }})\n\
             test('same', () => {{ expect(same).toBeTruthy() }})\n\
             test('spy', () => {{ expect(spy).toBeTruthy() }})\n\
             test('twin', () => {{ expect(twin).toBeTruthy() }})\n\
             test('counter', () => {{ expect(counter).toBe(0) }})\n\
             test('other', () => {{ expect(other).toBe(0) }})\n\
             test('imported', () => {{ expect(helper()).toBe(1) }})\n\
             test('local', () => {{ expect(local()).toBe(1) }})\n\
             test('data', () => {{ expect(data).toBeTruthy() }})\n\
             test('param', () => {{ expect(N).toBe(2) }})"
        ),
        &[
            ("helper.ts", "export function helper() { return 1 }"),
            ("data.json", "{}"),
        ],
        &[
            ("literal", "spelled"),
            ("config", "same"),
            ("imported", "local"),
        ],
        &[
            ("spy", "twin"),
            ("counter", "other"),
            ("literal", "param"),
            ("data", "config"),
        ],
    );
    // An unequal helper is another identity.
    check(
        &format!(
            "{HEADER}import {{ helper }} from './helper'\n\
             function local() {{ return 1 }}\n\
             test('imported', () => {{ expect(helper()).toBe(1) }})\n\
             test('local', () => {{ expect(local()).toBe(1) }})"
        ),
        &[("helper.ts", "export function helper() { return 2 }")],
        &[],
        &[("imported", "local")],
    );
}

/// A tag reads a template's raw text; an untagged template reads the string it spells.
#[test]
fn a_tagged_template_keeps_its_raw_text() {
    check(
        &format!(
            "{HEADER}test('escaped', () => {{ expect(String.raw`a\\nb`).toBe('x') }})\n\
             test('literal', () => {{ expect(String.raw`a\nb`).toBe('x') }})\n\
             test('escaped sub', () => {{ expect(String.raw`a\\n${{1}}`).toBe('x') }})\n\
             test('literal sub', () => {{ expect(String.raw`a\n${{1}}`).toBe('x') }})\n\
             test('untagged escaped', () => {{ expect(`a\\nb`).toBe('x') }})\n\
             test('untagged literal', () => {{ expect(`a\nb`).toBe('x') }})"
        ),
        &[],
        &[("untagged escaped", "untagged literal")],
        &[("escaped", "literal"), ("escaped sub", "literal sub")],
    );
}

/// Two bindings that each hold an equal object are equal read alone and still two objects read
/// together, in this module or through an import.
#[test]
fn bindings_holding_equal_objects_stay_two_objects() {
    check(
        &format!(
            "{HEADER}import {{ left, right }} from './objects'\n\
             const a = {{}}\n\
             const b = {{}}\n\
             const pattern = /x/\n\
             const twin = /x/\n\
             function f() {{ return 1 }}\n\
             function g() {{ return 1 }}\n\
             test('a', () => {{ expect(a).toBeTruthy() }})\n\
             test('b', () => {{ expect(b).toBeTruthy() }})\n\
             test('a is a', () => {{ expect(a === a).toBe(true) }})\n\
             test('a is b', () => {{ expect(a === b).toBe(true) }})\n\
             test('regex is regex', () => {{ expect(pattern === pattern).toBe(true) }})\n\
             test('regex is twin', () => {{ expect(pattern === twin).toBe(true) }})\n\
             test('f is f', () => {{ expect(f === f).toBe(true) }})\n\
             test('f is g', () => {{ expect(f === g).toBe(true) }})\n\
             test('left is left', () => {{ expect(left === left).toBe(true) }})\n\
             test('left is right', () => {{ expect(left === right).toBe(true) }})"
        ),
        &[(
            "objects.ts",
            "export const left = []\nexport const right = []",
        )],
        &[("a", "b")],
        &[
            ("a is a", "a is b"),
            ("regex is regex", "regex is twin"),
            ("f is f", "f is g"),
            ("left is left", "left is right"),
        ],
    );
}

/// A module-level function or class keeps the name its `.name` reads.
#[test]
fn an_outside_function_keeps_its_name() {
    check(
        &format!(
            "{HEADER}function f() {{}}\n\
             function g() {{}}\n\
             const h = () => {{}}\n\
             const k = () => {{}}\n\
             test('f', () => {{ expect(f.name).toBe('f') }})\n\
             test('g', () => {{ expect(g.name).toBe('f') }})\n\
             test('h', () => {{ expect(h.name).toBe('h') }})\n\
             test('k', () => {{ expect(k.name).toBe('h') }})"
        ),
        &[],
        &[],
        &[("f", "g"), ("h", "k")],
    );
}

/// What runs on a binding while tests register is part of what a test that reads it sees.
#[test]
fn registration_time_uses_are_part_of_an_identity() {
    check(
        &format!(
            "{HEADER}const left = {{}}\n\
             left.mode = 'a'\n\
             const right = {{}}\n\
             right.mode = 'a'\n\
             const third = {{}}\n\
             third.mode = 'b'\n\
             test('left', () => {{ expect(left.mode).toBeTruthy() }})\n\
             test('right', () => {{ expect(right.mode).toBeTruthy() }})\n\
             test('third', () => {{ expect(third.mode).toBeTruthy() }})"
        ),
        &[],
        &[("left", "right")],
        &[("right", "third")],
    );
}

/// A binding assigned while tests register may hold anything by the time a test runs.
#[test]
fn a_binding_written_while_tests_register_equals_only_itself() {
    // Two suites spell the same declaration and the same write: without the guard their bindings
    // read as one identity.
    let suite = |name: &str| {
        format!(
            "describe('{name}', () => {{ let mode = 'a'; mode = 'b'; test('{name}', () => {{ expect(mode).toBeTruthy() }}) }})\n"
        )
    };
    check(
        &format!(
            "{HEADER}import {{ describe }} from 'vitest'\n{}{}",
            suite("one"),
            suite("two")
        ),
        &[],
        &[],
        &[("one", "two")],
    );
}

/// The modifiers of each test registration's callee in `main`, in source order.
fn modifiers(main: &str) -> Vec<(String, bool, Vec<String>, bool)> {
    with_normalizer(main, &[], true, |normalizer, tree, found| {
        found
            .sites
            .iter()
            .map(|site| {
                let callee = tree.child(site.node, "callee").unwrap();
                let modifiers = normalizer.modifiers(callee, site.member);
                let arguments: Vec<_> = tree
                    .list(site.node, "arguments")
                    .into_iter()
                    .flatten()
                    .collect();
                let body = *arguments.last().unwrap();
                let (options, expected) = normalizer.options(&arguments, body);
                (
                    modifiers.chain,
                    modifiers.fails,
                    options,
                    expected == super::Expected::Failure,
                )
            })
            .collect()
    })
}

/// Skipping, focusing, concurrency and timeouts change when a test runs, never what it checks.
#[test]
fn scheduling_members_and_options_are_dropped() {
    let found = modifiers(
        "test('a', () => {})\n\
         it.skip('a', () => {})\n\
         test.only.concurrent('a', { timeout: 5, retry: 2 }, () => {})\n\
         test('a', 1000, () => {})\n\
         test.fails('a', () => {})\n\
         test('a', { fails: true }, () => {})\n\
         test.skipIf(ci)('a', () => {})\n\
         test('a', { tags: ['x'] }, () => {})",
    );
    let chains: Vec<_> = found.iter().map(|(chain, ..)| chain.as_str()).collect();
    assert_eq!(chains[..4], ["api:test"; 4]);
    assert_eq!(chains[4], "api:test");
    assert!(found[4].1, "`fails` inverts the outcome");
    assert!(found[5].3, "so does `{{ fails: true }}`");
    assert!(chains[6].starts_with("api:test.skipIf.("), "{}", chains[6]);
    assert!(found[..6]
        .iter()
        .all(|(_, _, options, _)| options.is_empty()));
    assert_eq!(found[7].2.len(), 1, "other options change what is checked");
}

/// A literal row is the test its callback is with the row's values written in.
#[test]
fn literal_table_rows_bind_into_the_callback() {
    let main = "test.each([[1, 'a'], [2, 'b']])('row %s', (n, s) => { expect(n).toBe(1); expect(s).toBe('a') })\n\
                test('spelled', () => { expect(1).toBe(1); expect('a').toBe('a') })\n\
                test.for([{ n: 1 }])('object', ({ n }) => { expect(n).toBe(1) })\n\
                test('object spelled', () => { expect(1).toBe(1) })\n\
                test.each([[make()]])('created twice', (value) => { use(value); use(value) })\n\
                test.each([[[]]])('allocated twice', (value) => { expect(value === value).toBe(true) })\n\
                let later = 0\n\
                test.each([later])('reassigned', (value) => { expect(value).toBe(0) })\n\
                later = 1\n\
                test.for([{}])('inherited', ({ toString }) => { expect(toString).toBe(undefined) })\n\
                test.for([{ __proto__: base }])('prototype', ({ n }) => { expect(n).toBe(undefined) })";
    with_normalizer(main, &[], true, |normalizer, tree, found| {
        let keyed: Vec<_> = found
            .sites
            .iter()
            .map(|site| site_forms(normalizer, tree, site.node))
            .collect();
        assert_eq!(
            keyed[0][0], keyed[1][0],
            "the first row is the spelled test"
        );
        assert_ne!(keyed[0][1], keyed[1][0], "the second row is another");
        assert_eq!(keyed[2][0], keyed[3][0], "an object row binds by property");
        assert_eq!(
            keyed[4],
            [None],
            "a creating cell read twice is not written in"
        );
        assert_eq!(keyed[5], [None], "nor is an allocating one");
        assert_eq!(keyed[6], [None], "nor one reading a reassigned binding");
        assert_eq!(keyed[7], [None], "a key the row lacks may be inherited");
        assert_eq!(keyed[8], [None], "a row that sets its prototype");
        assert!(matches!(
            normalizer
                .modifiers(tree.child(found.sites[2].node, "callee").unwrap(), None)
                .rows
                .as_deref(),
            Some([TableRow {
                value: Row::Whole(_, Some(_)),
                ..
            }])
        ));
    })
}

/// A row cell stops describing a parameter after the callback writes that binding.
#[test]
fn written_row_parameters_are_never_substituted() {
    let main = "test.each([[6]])('simple', (x) => { x = 5; expect(x).toBe(6) })\n\
                test.each([[6]])('compound', (x) => { x += 1; expect(x).toBe(6) })\n\
                test.each([[6]])('update', (x) => { x++; expect(x).toBe(6) })\n\
                test.each([[6]])('destructure', (x) => { [x] = [5]; expect(x).toBe(6) })\n\
                test.each([[6]])('loop', (x) => { for (x of [5]) {} expect(x).toBe(6) })\n\
                test.each([[6]])('nested', (x) => { const write = () => { x = 5 }; write(); expect(x).toBe(6) })\n\
                test.for([{ x: 6 }])('object', ({ x }) => { x = 5; expect(x).toBe(6) })\n\
                test.each([[6]])('missing', (x, y) => { y = 5; expect(x).toBe(6) })\n\
                test.each([[6]])('shadow', (x) => { { let x = 0; x = 1; expect(x).toBe(1) } expect(x).toBe(6) })\n\
                test('shadow spelled', () => { { let x = 0; x = 1; expect(x).toBe(1) } expect(6).toBe(6) })\n\
                test.for([6])('whole extra', (x, y) => { y = 5; expect(x).toBe(6) })";
    with_normalizer(main, &[], true, |normalizer, tree, found| {
        let keyed: Vec<_> = found
            .sites
            .iter()
            .map(|site| site_forms(normalizer, tree, site.node))
            .collect();
        assert!(
            keyed[..8].iter().all(|forms| forms == &[None]),
            "{keyed:#?}"
        );
        assert_eq!(
            keyed[8][0], keyed[9][0],
            "a write to a shadow must not refuse the row parameter"
        );
        assert!(
            keyed[10][0].is_some(),
            "a whole row does not propose a substitution for the extra parameter"
        );
    })
}

/// `for` passes each row whole; outer elisions are skipped while spreads remain unreadable.
#[test]
fn whole_rows_are_never_spread_and_outer_holes_are_skipped() {
    let found = modifiers(
        "test.for([[1, 2]])('a', (row) => {})\n\
         test.each([[1], 2])('b', (row) => {})\n\
         test.each([, [1]])('c', (row) => {})\n\
         test.each([...rows])('d', (row) => {})",
    );
    let chains: Vec<_> = found.iter().map(|(chain, ..)| chain.clone()).collect();
    assert_eq!(chains[0], "api:test.for.(rows)");
    assert_eq!(chains[1], "api:test.each.(rows)");
    assert_eq!(chains[2], "api:test.each.(rows)");
    assert!(
        chains[3].starts_with("api:test.each.((ArrayExpression"),
        "{}",
        chains[3]
    );
}

/// Erasing a table wrapper does not admit spreads, dynamic tables or sparse callback inputs.
#[test]
fn wrapped_tables_keep_unreadable_input_boundaries() {
    with_normalizer(
        "test.each(([...[1, 1]]) as const)('spread', value => {})\n\
         test.each(([[, 1], [, 1]]) as const)('cells', value => {})\n\
         test.each((rows) as readonly number[])('dynamic', value => {})\n\
         test.each(([1, , 2]) as const)('literal', value => {})",
        &[],
        true,
        |normalizer, tree, found| {
            let rows: Vec<_> = found
                .sites
                .iter()
                .filter(|site| site.kind == SiteKind::Registers(crate::framework::Registers::Test))
                .map(|site| {
                    normalizer
                        .modifiers(tree.child(site.node, "callee").unwrap(), site.member)
                        .rows
                })
                .collect();
            assert_eq!(rows.len(), 4);
            assert!(rows[..3].iter().all(Option::is_none));
            let literal = rows[3].as_ref().unwrap();
            assert_eq!(
                literal.iter().map(|row| row.index).collect::<Vec<_>>(),
                [0, 2]
            );
        },
    );
}

/// Every export form names its declaration; a default export is its value.
#[test]
fn export_forms_carry_the_identity_of_what_they_export() {
    let helpers = "function inner() { return 1 }\n\
                   export { inner as viaSpecifier }\n\
                   export const constant = { a: 1 }\n\
                   export class Model { run() { return 1 } }\n\
                   export default function () { return 1 }";
    check(
        &format!(
            "{HEADER}import fallback, {{ viaSpecifier, constant, Model }} from './helpers'\n\
             import {{ viaSpecifier as again }} from './barrel'\n\
             import other from './other'\n\
             function local() {{ return 1 }}\n\
             const same = {{ a: 1 }}\n\
             class Local {{ run() {{ return 1 }} }}\n\
             test('specifier', () => {{ expect(viaSpecifier()).toBe(1) }})\n\
             test('reexport', () => {{ expect(again()).toBe(1) }})\n\
             test('local', () => {{ expect(local()).toBe(1) }})\n\
             test('constant', () => {{ expect(constant).toBeTruthy() }})\n\
             test('same', () => {{ expect(same).toBeTruthy() }})\n\
             test('class', () => {{ expect(new Model()).toBeTruthy() }})\n\
             test('local class', () => {{ expect(new Local()).toBeTruthy() }})\n\
             test('default', () => {{ expect(fallback()).toBe(1) }})\n\
             test('other default', () => {{ expect(other()).toBe(1) }})"
        ),
        &[
            ("helpers.ts", helpers),
            ("barrel.ts", "export { viaSpecifier } from './helpers'"),
            ("other.ts", "export default function () { return 2 }"),
        ],
        &[
            ("specifier", "local"),
            ("reexport", "local"),
            ("constant", "same"),
        ],
        // A class keeps its name, which `.name` and snapshots observe.
        &[
            ("default", "other default"),
            ("default", "local"),
            ("class", "local class"),
        ],
    );
}

/// Whether each test of `main` is order-dependent, by title.
fn order_dependent(main: &str) -> HashMap<String, bool> {
    with_normalizer(main, &[], false, |normalizer, tree, inventory| {
        test_callbacks(tree, inventory)
            .map(|(title, callback)| {
                (
                    title,
                    normalizer.order_dependent(tree.inner(callback), true),
                )
            })
            .collect()
    })
}

const ORDER_HEADER: &str =
    "import { test, describe, beforeEach, beforeAll, aroundEach, expect } from 'vitest'\n";

/// A hook that writes a binding from its previous value makes the tests reading it order-dependent.
#[test]
fn an_accumulating_hook_makes_its_readers_order_dependent() {
    for (hook, expected) in [
        ("count++", true),
        ("count += 1", true),
        ("count = count + 1", true),
        ("count = 1", false),
        ("other = count + 1", false),
    ] {
        let tests = order_dependent(&format!(
            "{ORDER_HEADER}let count = 0\nlet other = 0\n\
             beforeEach(() => {{ {hook} }})\n\
             test('reads', () => {{ expect(count).toBe(1) }})"
        ));
        assert_eq!(tests["reads"], expected, "{hook}");
    }
}

/// A plain hook write reaches a test it does not run right before: another suite's hook, an
/// after-each hook that leaves a new value, a cleanup the hook returns. A write every test starts
/// from the same result of does not.
#[test]
fn a_hook_write_outside_the_tests_own_lifecycle_makes_it_order_dependent() {
    let case = |setup: &str, own: &str| {
        order_dependent(&format!(
            "{ORDER_HEADER}import {{ afterEach, afterAll }} from 'vitest'\n\
             let v = 0\nlet other = 0\n\
             {setup}\n\
             describe('scope', () => {{\n{own}\n\
             test('reads', () => {{ expect(v).toBe(0) }}) }})"
        ))["reads"]
    };
    let sibling = "describe('writer', () => { beforeEach(() => { v = 1 }); test('w', () => {}) })";
    for (setup, own, expected) in [
        // Another suite's hook writes the binding; its own binding, or an unrelated one, does not.
        (sibling, "", true),
        (
            "describe('writer', () => { let v = 5; beforeEach(() => { v = 1 }) })",
            "",
            false,
        ),
        (
            "describe('writer', () => { beforeEach(() => { other = 1 }) })",
            "",
            false,
        ),
        // A hook registered by a helper lands wherever the helper is called.
        (
            "function install() { beforeEach(() => { v = 1 }) }\ninstall()",
            "",
            true,
        ),
        // A hook around the reader rewrites it after the writer: before-each always, before-all
        // only when the writer is outside its suite.
        (sibling, "beforeAll(() => { v = 0 })", false),
        (sibling, "beforeEach(() => { ({ v } = { v: 0 }) })", false),
        (
            "",
            "beforeAll(() => { v = 0 })\n\
             describe('writer', () => { beforeEach(() => { v = 1 }) })",
            true,
        ),
        // In the reader's own suite: once per scope, before or after its tests, is the same for
        // each; after each test it must restore the declared value.
        ("", "beforeAll(() => { v = 1 })", false),
        ("", "afterAll(() => { v = 1 })", false),
        ("", "afterEach(() => { v = 1 })", true),
        ("", "afterEach(() => { v = 0 })", false),
        ("", "beforeEach(() => { return () => { v = 1 } })", true),
        // Before each test: a direct write, alone, destructured, in a sequence or a chain.
        ("", "beforeEach(() => { ({ v } = { v: 1 }) })", false),
        ("", "beforeEach(() => { ({ v } = { v: v + 1 }) })", true),
        ("", "beforeEach(() => { ((v = 1), (other = 2)) })", false),
        ("", "beforeEach(() => { other = v = 1 })", false),
        ("", "function set() { v = 1 }\nbeforeEach(set)", false),
        // A write inside a function the hook creates runs when that function is called: not seen.
        ("", "beforeEach(() => { other = () => { v = 1 } })", false),
        // An after-each restore holds only if the first test also starts from the declared value:
        // a once-per-scope hook or module code that sets another value first breaks it, unless a
        // before-each hook rewrites it.
        (
            "",
            "beforeAll(() => { v = 1 })\nafterEach(() => { v = 0 })",
            true,
        ),
        ("v = 1", "afterEach(() => { v = 0 })", true),
        ("", "v = 1\nafterEach(() => { v = 0 })", true),
        // A helper's write runs when something calls it, like a write through a method.
        (
            "function set() { v = 1 }",
            "afterEach(() => { v = 0 })",
            false,
        ),
        (
            "",
            "beforeAll(() => { v = 0 })\nafterEach(() => { v = 0 })",
            false,
        ),
        (
            "",
            "beforeAll(() => { v = 1 })\nbeforeEach(() => { v = 0 })\nafterEach(() => { v = 0 })",
            false,
        ),
    ] {
        assert_eq!(case(setup, own), expected, "{setup} | {own}");
    }
}

/// An after-each hook that sets a binding declared without a value back to `undefined` restores
/// it; a shadowed `undefined` is not the value.
#[test]
fn an_after_each_hook_restoring_undefined_is_not_order_dependent() {
    let case = |restore: &str| {
        order_dependent(&format!(
            "{ORDER_HEADER}import {{ afterEach }} from 'vitest'\n\
             describe('scope', () => {{\n\
             let v\n\
             afterEach(() => {{ {restore} }})\n\
             test('reads', () => {{ expect(v).toBeUndefined() }}) }})"
        ))["reads"]
    };
    assert!(!case("v = undefined"));
    assert!(case("v = 1"));
    assert!(case("const undefined = 1; v = undefined"));
}

/// An after-each hook restores a primitive initializer spelled alike: the global `undefined`, or a
/// sign, negation or `void` of a primitive literal.
#[test]
fn an_after_each_hook_restoring_a_primitive_initializer_is_not_order_dependent() {
    let case = |init: &str, restore: &str| {
        order_dependent(&format!(
            "{ORDER_HEADER}import {{ afterEach }} from 'vitest'\n\
             describe('scope', () => {{\n\
             let v = {init}\n\
             afterEach(() => {{ {restore} }})\n\
             test('reads', () => {{ expect(v).toBe({init}) }}) }})"
        ))["reads"]
    };
    for same in ["undefined", "-1", "+1", "!0", "void 0"] {
        assert!(!case(same, &format!("v = {same}")), "{same}");
    }
    assert!(case("-1", "v = 1"));
    assert!(case("-1", "v = +1"));
    assert!(case("-1", "v = -2"));
    assert!(case("-/a/", "v = -/a/"));
    assert!(case("-x", "v = -x"));
    assert!(case("undefined", "v = null"));
    assert!(case("undefined", "const undefined = 1; v = undefined"));
}

/// A hook write restores the declared value only when it spells the same inert literal; a cleanup
/// a hook returns runs after its tests, and a write the analysis cannot read as `v = value` varies.
#[test]
fn hook_writes_restore_only_the_declared_inert_value() {
    let case = |hook: &str| {
        order_dependent(&format!(
            "{ORDER_HEADER}import {{ afterEach, afterAll }} from 'vitest'\n\
             describe('scope', () => {{\n\
             let v = [1, {{ a: 'x' }}]\n\
             {hook}\n\
             test('reads', () => {{ expect(v).toEqual([1, {{ a: 'x' }}]) }}) }})"
        ))["reads"]
    };
    assert!(!case("afterEach(() => { v = [1, { a: 'x' }] })"));
    for differs in [
        "v = [1, { a: 'y' }]",
        "v = [1]",
        "v = { a: 'x' }",
        "v = [1, { [a]: 'x' }]",
        "v = [1, { a: f() }]",
        "[v] = [[1, { a: 'x' }]]",
    ] {
        assert!(
            case(&format!("afterEach(() => {{ {differs} }})")),
            "{differs}"
        );
    }
    // A before-all cleanup runs once after the scope's tests; a before-each one after each test.
    assert!(!case("beforeAll(() => () => { v = 2 })"));
    assert!(case("beforeEach(() => () => { v = 2 })"));
    // Vitest ignores what an after or around hook returns: that function never runs.
    assert!(!case("afterEach(() => () => { v = 2 })"));
    assert!(!case("afterAll(() => () => { v = 2 })"));
    assert!(!case(
        "aroundEach(async (run) => { await run(); return () => { v = 2 } })"
    ));
    // A cleanup is read on its own: one restoring the declared value leaves every test alike.
    assert!(!case("beforeEach(() => () => { v = [1, { a: 'x' }] })"));
    // A before-each hook around the test rewrites what an after-each one left; a before-all
    // one runs once, so it does not.
    let shielded = |shield: &str, writer: &str| case(&format!("{shield}\n{writer}"));
    assert!(!shielded(
        "beforeEach(() => { [v] = [[1]] })",
        "afterEach(() => { v = 2 })"
    ));
    assert!(!shielded(
        "beforeEach(() => { v = [1] })",
        "beforeEach(() => () => { v = 2 })"
    ));
    assert!(shielded(
        "beforeAll(() => { v = [1] })",
        "afterEach(() => { v = 2 })"
    ));
    assert!(shielded(
        "beforeEach(() => { v = v.concat(1) })",
        "afterEach(() => { v = 2 })"
    ));
    // A default in the pattern runs only when the value is missing.
    assert!(case("beforeEach(() => { [v = 1] = [] })"));
}

/// A binding every test starts from the same value of is not order-dependent: one the test assigns
/// first, or one a before-each hook around it surely assigns.
#[test]
fn state_every_test_starts_from_the_same_value_of_is_not_order_dependent() {
    let case = |setup: &str, reader: &str| {
        order_dependent(&format!(
            "{ORDER_HEADER}let wrapper = null\n\
             {setup}\n\
             test('writer', () => {{ wrapper = mount(); expect(wrapper).toBeTruthy() }})\n\
             describe('readers', () => {{ test('reader', () => {{ {reader} }}) }})"
        ))["reader"]
    };
    let reads = "expect(wrapper).toBeTruthy()";
    // A before-each reset around the reader, at module level or in its suite.
    assert!(!case("beforeEach(() => { wrapper = mount() })", reads));
    assert!(!case(
        "aroundEach(async (run) => { wrapper = mount(); await run() })",
        reads
    ));
    // Not a reset: it runs once, it reads what it replaces, or it is in another suite.
    assert!(case("beforeAll(() => { wrapper = mount() })", reads));
    assert!(case(
        "beforeEach(() => { wrapper = wrapper || mount() })",
        reads
    ));
    // A read inside a function the value holds runs later, on the new value.
    assert!(!case(
        "beforeEach(() => { wrapper = { get: () => wrapper } })",
        reads
    ));
    assert!(case(
        "describe('other', () => { beforeEach(() => { wrapper = mount() }) })",
        reads
    ));
    // Nor one that may not run: under a condition, or after a way out of the hook.
    assert!(case(
        "beforeEach(() => { if (ready) wrapper = mount() })",
        reads
    ));
    assert!(case(
        "beforeEach(() => { if (ready) return; wrapper = mount() })",
        reads
    ));
    // Nor one a helper registers for whichever suite calls it.
    assert!(case(
        "function setup() { beforeEach(() => { wrapper = mount() }) }",
        reads
    ));
    // Nor one registered under a condition.
    assert!(case(
        "if (ready) beforeEach(() => { wrapper = mount() })",
        reads
    ));
    // A concise hook body runs whenever the hook does.
    assert!(!case("beforeEach(() => (wrapper = mount()))", reads));
    // The reader's own assignment first, directly in its body.
    assert!(!case("", "wrapper = mount(); expect(wrapper).toBeTruthy()"));
    assert!(case("", "expect(wrapper).toBeNull(); wrapper = mount()"));
    assert!(case(
        "",
        "if (flag) wrapper = mount(); expect(wrapper).toBeTruthy()"
    ));
    // Only the writer reads its own write.
    let tests = order_dependent(&format!(
        "{ORDER_HEADER}let shared = 0\n\
         test('writer', () => {{ shared = 1; expect(1).toBe(1) }})\n\
         test('reader', () => {{ expect(shared).toBe(0) }})"
    ));
    assert!(!tests["writer"] && tests["reader"]);
}

/// A test's suites are the suite callbacks around it; an iteration callback is transparent, and any
/// other function means a caller decides what it registers.
#[test]
fn suites_around_a_test_and_the_statements_holding_it() {
    let main = "describe('outer', () => {\n\
                  const value = 1\n\
                  describe('inner', () => { test('nested', () => { expect(value).toBe(1) }) });\n\
                  [1].forEach(() => test('iterated', () => { expect(1).toBe(1) }))\n\
                })\n\
                function later() { test('inside', () => { expect(1).toBe(1) }) }\n\
                test('top', () => { expect(1).toBe(1) })";
    with_normalizer(main, &[], true, |normalizer, tree, found| {
        let site = |title: &str| {
            found
                .sites
                .iter()
                .find(|site| site.title.as_deref() == Some(title))
                .unwrap()
                .node
        };
        let suites = |title: &str| {
            normalizer
                .suites_around(site(title))
                .map(|chain| chain.len())
        };
        assert_eq!(suites("nested"), Some(2));
        assert_eq!(suites("iterated"), Some(1));
        assert_eq!(suites("top"), Some(0));
        assert_eq!(suites("inside"), None);
        let outer = normalizer.suites_around(site("nested")).unwrap()[0];
        let statements = normalizer.body_statements(outer);
        assert_eq!(
            statements.len(),
            3,
            "the declaration, the inner suite and the loop"
        );
        assert_eq!(
            normalizer.statement_in(site("nested"), Some(outer)),
            Some(statements[1])
        );
        let top = normalizer.statement_in(site("top"), None).unwrap();
        assert_eq!(tree.kind(top), "ExpressionStatement");
        assert_eq!(normalizer.statement_in(site("top"), Some(outer)), None);
        let concise = tree
            .nodes()
            .find(|&node| {
                tree.kind(node) == "ArrowFunctionExpression"
                    && tree
                        .child(node, "body")
                        .is_some_and(|body| tree.kind(body) != "BlockStatement")
            })
            .unwrap();
        assert!(normalizer.body_statements(concise).is_empty());
    });
}

/// Options that could set `fails` without spelling it leave the expected outcome unknown: a
/// variable, a spread, a computed key, or options passed as a value. A spelled `fails` decides.
#[test]
fn an_unread_fails_option_leaves_the_outcome_unknown() {
    let main = "const flag = true\nconst o = { fails: true }\nconst key = 'fails'\n\
                test('a', { fails: flag }, () => {})\n\
                test('b', { ...o }, () => {})\n\
                test('c', { [key]: true }, () => {})\n\
                test('d', o, () => {})\n\
                test('e', { fails: false, retry: 2 }, () => {})\n\
                test('f', { fails: true, ...o }, () => {})";
    with_normalizer(main, &[], true, |normalizer, tree, found| {
        let expected: Vec<_> = found
            .sites
            .iter()
            .map(|site| {
                let arguments: Vec<NodeId> = tree
                    .list(site.node, "arguments")
                    .into_iter()
                    .flatten()
                    .collect();
                let body = *arguments
                    .iter()
                    .find(|&&argument| tree.is_function(argument))
                    .unwrap();
                normalizer.options(&arguments, body).1
            })
            .collect();
        use super::Expected::{Failure, Pass, Unknown};
        assert_eq!(
            expected,
            [Unknown, Unknown, Unknown, Unknown, Pass, Failure]
        );
    });
}

/// A timeout is not an option; an option that changes the outcome is, and `fails` inverts it.
#[test]
fn options_are_what_changes_what_a_test_checks() {
    let main = "test('a', () => {}, 1000)\n\
                test('b', { fails: true, retry: 2 }, () => {})\n\
                test('c', { skip: false, label: x }, () => {})";
    with_normalizer(main, &[], true, |normalizer, tree, found| {
        let mut options = |index: usize| {
            let node = found.sites[index].node;
            let arguments: Vec<NodeId> =
                tree.list(node, "arguments").into_iter().flatten().collect();
            let body = *arguments
                .iter()
                .find(|&&argument| tree.is_function(argument))
                .unwrap();
            normalizer.options(&arguments, body)
        };
        assert_eq!(options(0), (vec![], super::Expected::Pass));
        let (_, expected) = options(1);
        assert_eq!(expected, super::Expected::Failure);
        let (written, expected) = options(2);
        assert_eq!(expected, super::Expected::Pass, "only `fails` decides");
        assert!(!written.is_empty());
    });
}

/// A test whose callback assigns through a chain `links` long.
fn chain(title: &str, links: usize) -> String {
    format!(
        "test('{title}', () => {{ a = {}1 }})\n",
        "a = ".repeat(links)
    )
}

/// A form is written to a bounded depth, on a stack half a test thread's: deeper code reads as
/// itself only, and code inside the bound keeps matching.
#[test]
fn a_form_deeper_than_its_bound_equals_only_itself() {
    let main = [
        format!("{HEADER}let a\n"),
        chain("near", 200),
        chain("near again", 200),
        chain("past", 300),
        chain("past again", 300),
        chain("deep", 5_000),
    ]
    .concat();
    let (forms, repeated) = on_half_stack(move || {
        with_normalizer(&main, &[], false, |normalizer, tree, inventory| {
            let callbacks: HashMap<_, _> = test_callbacks(tree, inventory).collect();
            let forms: HashMap<_, _> = callbacks
                .iter()
                .map(|(title, &callback)| (title.clone(), normalizer.of(callback)))
                .collect();
            let repeated = normalizer.of(callbacks["deep"]) == forms["deep"];
            (forms, repeated)
        })
    });
    assert_eq!(forms.len(), 5);
    assert_eq!(forms["near"], forms["near again"]);
    assert_ne!(forms["past"], forms["past again"]);
    assert!(forms["deep"].contains("(deep "));
    assert!(repeated);
}

/// The predicates a form is decided by read code of any depth the parser admits, on the same
/// half-thread stack: a const's initializer (`creates`, `allocates`) and a function's body
/// (`binds_this`).
#[test]
fn deep_code_is_read_without_recursion() {
    let negations = "!".repeat(15_000);
    let main = format!(
        "{HEADER}const b = {negations}1\ntest('reads', () => {{ b }})\n\
         test('function', function () {{ {negations}1 }})\n"
    );
    let forms = on_half_stack(move || forms(&main, &[]));
    assert_eq!(forms.len(), 2);
}

/// Past the depth bound, rows of one table still differ by their cells: the callback is the same
/// node for every row, so its token carries what the row binds.
#[test]
fn rows_past_the_depth_bound_differ_by_their_cells() {
    let negations = "!".repeat(300);
    let main = format!(
        "{HEADER}test.each([false, true, false])('row %s', value => {{\n  \
         expect({negations}value).toBe(false)\n}})\n"
    );
    let forms = on_half_stack(move || {
        with_normalizer(&main, &[], false, |normalizer, tree, found| {
            site_forms(normalizer, tree, found.sites[0].node)
        })
    });
    assert_eq!(forms.len(), 3);
    assert!(forms
        .iter()
        .all(|form| form.as_ref().unwrap().contains("(deep ")));
    assert_eq!(forms[0], forms[2]);
    assert_ne!(forms[0], forms[1]);
}

/// The form of `title` in a file holding `main`, on half a test thread's stack.
fn form_of(main: &str, title: &str) -> String {
    let main = main.to_owned();
    let title = title.to_owned();
    on_half_stack(move || forms(&main, &[]).remove(&title).unwrap())
}

/// The identity of each top-level binding of `main`, named in each of `orders` from an empty cache
/// (the first name of an order names the rest it reads).
fn identities(main: &str, orders: &[Vec<&str>]) -> Vec<Vec<(String, String)>> {
    with_normalizer(main, &[], false, |normalizer, _, _| {
        orders
            .iter()
            .map(|order| {
                normalizer.externals.clear();
                let mut named: Vec<_> = order
                    .iter()
                    .map(|name| {
                        let symbol = normalizer.scopes.top_level(name).unwrap();
                        (name.to_string(), normalizer.external(symbol))
                    })
                    .collect();
                named.sort();
                named
            })
            .collect()
    })
}

/// A declaration's identity does not depend on which test read it first or how deep that test
/// was: past the depth bound and along a chain of declarations.
#[test]
fn an_identity_does_not_depend_on_its_first_reader() {
    let shared = format!("const shared = {}1\n", "!".repeat(40));
    let reader = |title: &str, negations: usize| {
        format!(
            "test('{title}', () => {{ expect({}shared).toBe(true) }})\n",
            "!".repeat(negations)
        )
    };
    let (deep, shallow, again) = (
        reader("deep", 240),
        reader("shallow", 0),
        reader("again", 240),
    );
    let alone = form_of(&[HEADER, &shared, &shallow].concat(), "shallow");
    assert_eq!(
        form_of(&[HEADER, &shared, &deep, &shallow].concat(), "shallow"),
        alone
    );
    let between = [HEADER, &shared, &deep, &shallow, &again].concat();
    assert_eq!(form_of(&between, "deep"), form_of(&between, "again"));
    assert_eq!(
        form_of(&between, "deep"),
        form_of(&[HEADER, &shared, &deep].concat(), "deep")
    );
    let chain: String = (0..8)
        .map(|link| format!("const c{link} = !c{}\n", link + 1))
        .chain(["const c8 = 1\n".to_owned()])
        .collect();
    let far = "test('far', () => { expect(c0).toBe(true) })\n";
    let near = "test('near', () => { expect(c5).toBe(true) })\n";
    assert_eq!(
        form_of(&[HEADER, &chain, far, near].concat(), "near"),
        form_of(&[HEADER, &chain, near].concat(), "near")
    );
}

/// Declarations that read each other, or themselves, equal only themselves, whichever is named
/// first; one that reads into such a cycle from outside it keeps its structure.
#[test]
fn a_cycle_of_declarations_is_named_by_identity_from_any_entry() {
    // `c` reaches the cycle through `b`, already visited when `a` is named first.
    let main = "const a = () => b() + c()\nconst b = () => a()\nconst c = () => b()\n\
                const d = () => a()\nconst e = () => 1\nconst f = () => f()\n\
                const p = () => q()\nconst q = () => r()\nconst r = () => p()\n";
    let names = ["a", "b", "c", "d", "e", "f", "p", "q", "r"];
    let orders: Vec<Vec<&str>> = names
        .iter()
        .map(|entry| {
            let mut order = vec![*entry];
            order.extend(names.iter().filter(|name| *name != entry));
            order
        })
        .collect();
    let named = identities(main, &orders);
    for (order, identities) in orders.iter().zip(&named) {
        assert_eq!(identities, &named[0], "named from {}", order[0]);
    }
    let opaque: Vec<_> = named[0]
        .iter()
        .filter(|(_, identity)| identity.starts_with("sym:"))
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(opaque, ["a", "b", "c", "f", "p", "q", "r"]);
}

/// Each declaration is named once however many paths reach it: two functions per level, each
/// calling both of the next, twenty levels deep, is about a million paths and forty declarations.
#[test]
fn repeated_reads_of_a_declaration_name_it_once() {
    let mut main = String::from(HEADER);
    for level in 0..20 {
        let next = level + 1;
        for side in ["a", "b"] {
            main.push_str(&format!(
                "function f{level}{side}() {{ f{next}a(); f{next}b() }}\n"
            ));
        }
    }
    main.push_str("function f20a() {}\nfunction f20b() {}\ntest('reads', () => { f0a() })\n");
    let started = std::time::Instant::now();
    assert!(form_of(&main, "reads").contains("fn:"));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

/// The form of every test of the test files each of `orders` names, read in that order in one run
/// (with its own export cache) over one project holding `files`, by file and title.
fn run_forms(files: &[(&str, &str)], orders: &[&[&str]]) -> Vec<HashMap<(String, String), String>> {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files {
        fs::write(dir.path().join(path), content).unwrap();
    }
    let run = |order: &[&str]| {
        let exports = Exports::default();
        let mut forms = HashMap::new();
        for name in order {
            let path = dir.path().join(name);
            let source = fs::read_to_string(&path).unwrap();
            let parsed = parse(&path, &source).unwrap();
            let found = inventory(
                &path,
                &source,
                &parsed,
                crate::inventory::Setting {
                    dialect: &DIALECT,
                    globals: false,
                    root: dir.path(),
                },
                &mut Modules::default(),
            );
            let structure = Structure::of(&parsed, &found);
            let setting = Setting {
                dialect: &DIALECT,
                root: dir.path(),
                exports: &exports,
            };
            let mut normalizer = Normalizer::new(&path, &parsed, setting, &structure);
            for (title, callback) in test_callbacks(&parsed.tree, &found) {
                forms.insert((name.to_string(), title), normalizer.of(callback));
            }
        }
        forms
    };
    orders.iter().map(|order| run(order)).collect()
}

/// What a module exports is named the same whichever test file reads it first, across an import
/// cycle; a module read for an export names its own imports by where they resolve.
#[test]
fn an_export_is_named_the_same_whichever_file_reads_it_first() {
    let files = [
        ("m.ts", "import { x } from './n'\nexport const helper = () => x()\n"),
        ("n.ts", "import { helper } from './m'\nexport const x = () => helper()\n"),
        ("k1.ts", "export const k = () => 1\n"),
        ("k2.ts", "export const k = () => 1\n"),
        ("h1.ts", "import { k } from './k1'\nexport const helper = () => k()\n"),
        ("h2.ts", "import { k } from './k2'\nexport const helper = () => k()\n"),
        ("h3.ts", "import { k } from './k1'\nexport const helper = () => k()\n"),
        (
            "a.test.ts",
            "import { test } from 'vitest'\nimport { helper } from './m'\n\
             test('a', () => { helper() })\n",
        ),
        (
            "b.test.ts",
            "import { test } from 'vitest'\nimport { x } from './n'\nimport { helper as one } from './h1'\n\
             import { helper as two } from './h2'\nimport { helper as three } from './h3'\n\
             test('b', () => { x() })\ntest('one', () => { one() })\n\
             test('two', () => { two() })\ntest('three', () => { three() })\n",
        ),
    ];
    let runs = run_forms(
        &files,
        &[&["a.test.ts", "b.test.ts"], &["b.test.ts", "a.test.ts"]],
    );
    let (forward, backward) = (&runs[0], &runs[1]);
    assert_eq!(forward.len(), 5);
    assert_eq!(forward, backward);
    let form = |title: &str| &forward[&("b.test.ts".to_owned(), title.to_owned())];
    // Equal helpers reading the same module are one identity; reading two equal modules, two.
    assert_eq!(form("one"), form("three"));
    assert_ne!(form("one"), form("two"));
}

/// Retained export-identity keys and strings remain charged after the file's parser is dropped.
#[test]
fn export_identity_cache_retains_owned_bytes() {
    use crate::memory::HeapBytes;
    let cache = super::Exports::default();
    assert_eq!(cache.heap_bytes(), 0);
    cache.known.borrow_mut().reserve(16);
    let reserved = cache.heap_bytes();
    assert!(reserved > 0);
    cache.known.borrow_mut().insert(
        ("module.ts".into(), "name".into()),
        Some("identity".repeat(1024)),
    );
    assert!(cache.heap_bytes() > reserved);
}
