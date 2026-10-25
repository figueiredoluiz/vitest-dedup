use super::*;
use std::path::PathBuf;

/// A test file `t.test.ts` beside `modules`, extracted from disk so imports resolve.
struct ConstantCase {
    _directory: tempfile::TempDir,
    file: SourceFile,
    source: String,
}

impl ConstantCase {
    /// Writes `modules` (relative path, source) and the test file `source` into a fresh root.
    fn new(modules: &[(&str, &str)], source: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        for (path, text) in modules {
            let path = directory.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let path = directory.path().join("t.test.ts");
        std::fs::write(&path, source).unwrap();
        let file = SourceFile::without_project_context(
            path,
            crate::discovery::SourceLanguage::TypeScript,
            ModuleFlavor::Ambiguous,
        );
        Self {
            _directory: directory,
            file,
            source: source.to_owned(),
        }
    }

    /// The file's tests as `(title, unresolved)`.
    fn tests(&self) -> Vec<(String, bool)> {
        self.tests_with(true)
    }

    /// [`Self::tests`], extracted as a historical file when `allow_imported` is false.
    fn tests_with(&self, allow_imported: bool) -> Vec<(String, bool)> {
        extract_with_imports(&self.source, &self.file, Globals::Disabled, allow_imported)
            .tests
            .iter()
            .map(|case| (case.full_title(), case.context.unresolved))
            .collect()
    }
}

/// One table row reading `X` imported from `./m`, over the module source `module`.
fn single_row(module: &str) -> Vec<(String, bool)> {
    ConstantCase::new(
        &[("m.ts", module)],
        "import { test } from 'vitest'\nimport { X } from './m'\ntest.each([[X]])('t %s', () => {})\n",
    )
    .tests()
}

#[test]
/// Each admitted declaration shape resolves the cell to the declaring module's literal.
fn closed_const_literals_resolve_through_every_admitted_export_shape() {
    for (module, title) in [
        ("export const X = 'fast'", "t fast"),
        ("export const X: string = 'fast' as const", "t fast"),
        ("export const X = -1", "t -1"),
        ("export const X = `tpl`", "t tpl"),
        ("const A = 'a'\nexport const X = A", "t a"),
        ("const A = 'a'\nexport { A as X }", "t a"),
        ("export const Y = 1, X = 'second'", "t second"),
    ] {
        assert_eq!(
            single_row(module),
            vec![(title.to_owned(), false)],
            "{module}"
        );
    }
}

#[test]
/// A declaration whose value the importer cannot prove leaves the row unresolved.
fn unproved_declarations_leave_the_cell_unresolved() {
    for module in [
        "export let X = 'a'",
        "export var X = 'a'",
        "export declare const X = 'a'",
        "export const X = 'a' + 'b'",
        "export const X = undefined",
        "export const X = NaN",
        "export const X = +1n",
        "export const X = { a: 1 }",
        "export const X = ['a']",
        "export const X = make()",
        "import { Y } from './n'\nexport const X = Y",
        "export * from './n'",
        "export default 'a'",
        "export function X() {}",
        "export const X = 'a' +",
        "export const X = 'a'\nlet X = 'b'",
        // A redeclared neighbour is a SyntaxError: the module never evaluates.
        "let Y = 1\nlet Y = 2\nexport const X = 'a'",
    ] {
        let tests = single_row(module);
        assert_eq!(tests.len(), 1, "{module}");
        assert!(tests[0].1, "{module} must stay unresolved: {tests:?}");
    }
}

#[test]
/// Named re-export hops are followed, including an import the module exports again; a local
/// alias of an import in a barrel, `export *` and type-only exports are not.
fn reexport_hops_are_followed_to_the_declaring_module() {
    let read = |barrel: &str| {
        ConstantCase::new(
            &[
                ("values.ts", "export const MODE = 'fast'"),
                ("barrel.ts", barrel),
            ],
            "import { test } from 'vitest'\nimport { X } from './barrel'\ntest.each([[X]])('t %s', () => {})\n",
        )
        .tests()
    };
    for barrel in [
        "export { MODE as X } from './values'",
        "import { MODE } from './values'\nexport { MODE as X }",
        "import { MODE as X } from './values'\nexport { X }",
    ] {
        assert_eq!(read(barrel), vec![("t fast".to_owned(), false)], "{barrel}");
    }
    for barrel in [
        "import type { MODE } from './values'\nexport { MODE as X }",
        "import { MODE } from './values'\nconst X = MODE\nexport { X }",
        "export * from './values'\nexport {}",
        "export type { MODE as X } from './values'",
    ] {
        assert!(read(barrel)[0].1, "{barrel}");
    }
}

#[test]
/// Default, namespace and type-only imports are not proved.
fn only_named_value_imports_are_tried() {
    let module = [("m.ts", "export const X = 'fast'\nexport default 'fast'")];
    for import in [
        "import X from './m'\ntest.each([[X]])('t %s', () => {})",
        "import * as M from './m'\ntest.each([[M.X]])('t %s', () => {})",
        "import { default as X } from './m'\ntest.each([[X]])('t %s', () => {})",
    ] {
        let tests = ConstantCase::new(
            &module,
            &format!("import {{ test }} from 'vitest'\n{import}\n"),
        )
        .tests();
        assert!(tests[0].1, "{import}: {tests:?}");
    }
}

#[test]
/// A mock of the module, or of a hop, replaces the value; so does a same-name Nuxt mock.
fn mocked_modules_are_never_read() {
    let modules = [
        ("values.ts", "export const X = 'fast'"),
        ("barrel.ts", "export { X } from './values'"),
    ];
    let header = "import { test, vi } from 'vitest'\nimport { mockNuxtImport } from '@nuxt/test-utils/runtime'\nimport { X } from './barrel'\n";
    for mock in [
        "",
        "vi.mock('./barrel', () => ({ X: 'slow' }))",
        "vi.mock('./values', () => ({ X: 'slow' }))",
        "vi.mock('./values')",
        "mockNuxtImport('X', () => 'slow')",
    ] {
        let tests = ConstantCase::new(
            &modules,
            &format!("{header}{mock}\ntest.each([[X]])('t %s', () => {{}})\n"),
        )
        .tests();
        assert_eq!(tests[0].1, !mock.is_empty(), "{mock:?}: {tests:?}");
    }
}

#[test]
/// A chain that returns to the test file reads a binding still in its temporal dead zone.
fn a_chain_back_to_the_importing_file_is_refused() {
    for (modules, specifier) in [
        (vec![], "./t.test"),
        (
            vec![("barrel.ts", "export { X } from './t.test'")],
            "./barrel",
        ),
    ] {
        let tests = ConstantCase::new(
            &modules,
            &format!("import {{ test }} from 'vitest'\nimport {{ X as Y }} from '{specifier}'\ntest.each([[Y]])('t %s', () => {{}})\nexport const X = 'a'\n"),
        )
        .tests();
        assert!(tests[0].1, "{specifier}: {tests:?}");
    }
}

#[test]
/// Past the per-file bound, further imports are not tried and their rows stay unresolved.
fn the_number_of_proved_imports_is_bounded() {
    let count = crate::frontend::registry::imported_constants::MAX_IMPORTED_CONSTANTS + 1;
    let (names, module, rows) = numbered_constants("C", count);
    let source = format!(
        "import {{ test }} from 'vitest'\nimport {{ {} }} from './m'\ntest.each([{rows}])('t %s', () => {{}})\n",
        names.join(", "),
    );
    let tests = ConstantCase::new(&[("m.ts", &module)], &source).tests();
    assert_eq!(tests.len(), count);
    // Imports are proved in the order cells first read them, so only the last row is past it.
    let unresolved: Vec<usize> = tests
        .iter()
        .enumerate()
        .filter(|(_, (_, unresolved))| *unresolved)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(unresolved, vec![count - 1]);
    assert_eq!(tests[count - 2].0, format!("t C{}", count - 2));
}

#[test]
/// A declaration file describes a module without being one; its values are never read.
fn declaration_files_supply_no_values() {
    let directory = tempfile::tempdir().unwrap();
    let path: PathBuf = directory.path().join("m.d.ts");
    std::fs::write(&path, "export const X = 'a'").unwrap();
    let allocator = Allocator::default();
    assert!(
        crate::frontend::registry::imported_constants::exported_literal(
            &allocator,
            &path,
            "X",
            Span::default()
        )
        .is_none()
    );
    let plain = directory.path().join("m.ts");
    std::fs::write(&plain, "export const X = 'a'").unwrap();
    assert!(
        crate::frontend::registry::imported_constants::exported_literal(
            &allocator,
            &plain,
            "X",
            Span::default()
        )
        .is_some()
    );
}

#[test]
/// A historical file's imports name the modules of its own revision, not today's files.
fn historical_files_never_read_present_day_constants() {
    let case = ConstantCase::new(
        &[("m.ts", "export const X = 'fast'")],
        "import { test } from 'vitest'\nimport { X } from './m'\ntest.each([[X]])('t %s', () => {})\n",
    );
    assert_eq!(case.tests_with(true), vec![("t fast".to_owned(), false)]);
    assert!(case.tests_with(false)[0].1);
    // The base cannot read what the current tree reads, so it is partial; a historical file with
    // nothing to prove is not.
    let historical = extract_with_imports(&case.source, &case.file, Globals::Disabled, false);
    assert!(historical.incomplete);
    assert_eq!(
        historical.reasons,
        vec![ExclusionReason::HelperSourcesUnavailable]
    );
    let plain = ConstantCase::new(
        &[("m.ts", "export const X = 'fast'")],
        "import { test } from 'vitest'\nimport { X } from './m'\ntest('t', () => { X })\n",
    );
    let historical = extract_with_imports(&plain.source, &plain.file, Globals::Disabled, false);
    assert!(!historical.incomplete, "{:?}", historical.diagnostics);
}

#[test]
/// An exported object's member can be written by an importer before the initializer reads it.
fn a_member_read_in_the_declaring_module_is_refused() {
    for module in [
        "const O = { a: 'x' }\nexport const X = O.a",
        "const O = { a: 'x' }\nconst A = O.a\nexport const X = A",
        // An erased alias reads whatever the runtime binds to the name.
        "(globalThis as any).A = 'slow'\ndeclare const A = 'x'\nexport const X = A",
    ] {
        assert!(single_row(module)[0].1, "{module}");
    }
    assert_eq!(
        single_row("const A = 'x'\nconst B = A\nexport const X = B"),
        vec![("t x".to_owned(), false)]
    );
}

#[test]
/// A hop that leaves the analysis root leaves the cell unresolved without marking the file
/// incomplete: the lookup is optional.
fn a_hop_outside_the_root_does_not_mark_the_file_incomplete() {
    for (barrel, test_tail, unresolved) in [
        ("export { X } from '@shared/consts'", "", true),
        // A dynamic import leaving the root is not looked up by the proof at all.
        (
            "export const X = 'a'",
            "const later = () => import('@shared/consts')\n",
            false,
        ),
    ] {
        outside_root_case(barrel, test_tail, unresolved);
    }
}

/// One `t.test.ts` under `root/src` reading `X` from `./barrel`, beside a tsconfig alias that
/// leaves the root; asserts the file stays complete and whether the cell resolves.
fn outside_root_case(barrel: &str, test_tail: &str, unresolved: bool) {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("root");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(outer.path().join("shared")).unwrap();
    std::fs::write(
        outer.path().join("shared/consts.ts"),
        "export const X = 'a'",
    )
    .unwrap();
    std::fs::write(
        root.join("tsconfig.json"),
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@shared/*": ["../shared/*"] } } }"#,
    )
    .unwrap();
    std::fs::write(root.join("src/barrel.ts"), barrel).unwrap();
    let path = root.join("src/t.test.ts");
    let source = format!(
        "import {{ test }} from 'vitest'\nimport {{ X }} from './barrel'\n{test_tail}test.each([[X]])('t %s', () => {{}})\n"
    );
    std::fs::write(&path, &source).unwrap();
    let file = SourceFile::without_project_context(
        path,
        crate::discovery::SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    let extraction = extract_within(&source, &file, Globals::Disabled, &root);
    assert_eq!(extraction.tests.len(), 1);
    assert_eq!(extraction.tests[0].context.unresolved, unresolved);
    assert!(
        !extraction
            .reasons
            .contains(&ExclusionReason::ImportOutsideRoot),
        "{:?}",
        extraction.diagnostics
    );
}

#[test]
/// A Nuxt mock of a name renamed at an intermediate hop still replaces the value.
fn a_nuxt_mock_of_any_name_on_the_chain_is_never_read() {
    let modules = [
        ("values.ts", "export const Z = 'a'"),
        ("barrel2.ts", "export { Z as Y } from './values'"),
        ("barrel1.ts", "export { Y as X } from './barrel2'"),
    ];
    for (mock, unresolved) in [("", false), ("mockNuxtImport('Y', () => 'b')", true)] {
        let tests = ConstantCase::new(
            &modules,
            &format!("import {{ test }} from 'vitest'\nimport {{ mockNuxtImport }} from '@nuxt/test-utils/runtime'\nimport {{ X }} from './barrel1'\n{mock}\ntest.each([[X]])('t %s', () => {{}})\n"),
        )
        .tests();
        assert_eq!(tests[0].1, unresolved, "{mock:?}: {tests:?}");
    }
}

#[test]
/// Redefining an accessor is how an export changes under its importers, and the module object
/// can be reached many ways, so any accessor redefinition in the file refuses every import. A
/// namespace object alone, a function spy and an unresolved dynamic import do not.
fn accessor_redefinitions_refuse_imported_constants() {
    let modules = [
        ("m.ts", "export const X = 'fast'"),
        ("helper.ts", "export * as values from './m'"),
    ];
    for (setup, unresolved) in [
        ("import * as ns from './m'", false),
        ("import type * as types from './m'", false),
        ("const actual = () => vi.importActual('./m')", false),
        ("const later = () => import('node:fs')", false),
        ("vi.spyOn(console, 'log')", false),
        ("import * as ns from './m'\nvi.spyOn(ns, 'X', 'get')", true),
        (
            "import { values } from './helper'\nvi.spyOn(values, 'X', 'get')",
            true,
        ),
        (
            "const { spyOn } = vi\nconst ns = await vi.importActual('./m')\nspyOn(ns, 'X', 'get')",
            true,
        ),
        ("const spy = vi.spyOn", true),
        ("vi['spyOn'](globalThis, 'X', 'get')", true),
        (
            "import * as ns from './m'\nObject.defineProperty(ns, 'X', { get: () => 'slow' })",
            true,
        ),
        ("Reflect.defineProperty(globalThis, 'Y', {})", true),
        ("({}).__defineGetter__('X', () => 1)", true),
        ("vi.mock('~/nowhere', () => ({}))", true),
        (
            "const { defineProperty: def } = Object\ndef(globalThis, 'Y', {})",
            true,
        ),
        ("Object[`defineProperty`](globalThis, 'Y', {})", true),
        (
            "const args = [globalThis, 'Y', 'get'] as const\nvi.spyOn(...args)",
            true,
        ),
        ("const { spyOn: s } = vi\ns(globalThis, 'Y', 'get')", true),
        (
            "vi.spyOn(console, 'log')\nvi.spyOn(console, 'error')",
            false,
        ),
        (
            "vi.spy\\u004Fn(console, 'log')\nvi.spyOn(globalThis, 'Y', 'get')",
            true,
        ),
        // A package-shaped specifier that does not resolve may be a bundler alias.
        ("vi.mock('vitest-dedup-not-installed', () => ({}))", true),
        ("vi.mock('@app/modes', () => ({}))", true),
        ("vi.mock('node:fs')", false),
        // Names are compared decoded and whole, so escapes count and look-alikes do not.
        ("vi['\\x73pyOn'](globalThis, 'Y', 'get')", true),
        ("Object['define\\x50roperty'](globalThis, 'Y', {})", true),
        (
            "// spyOn and defineProperty in a comment\nconst spyOnce = 1",
            false,
        ),
        ("const label = 'caf\\u00e9'", false),
        // Deleting a module object's property rewrites the import to `undefined`.
        ("import * as ns from './m'\ndelete (ns as any).X", true),
        ("Reflect.deleteProperty(globalThis, 'Y')", true),
        // `process.env` itself can be reassigned to a module object first.
        ("delete process.env.MODE", true),
        (
            "import * as ns from './m'\nconst saved = process.env\nprocess.env = ns as any\ndelete process.env.X\nprocess.env = saved",
            true,
        ),
        (
            "import * as ns from './m'\nconst process = { env: ns }\ndelete (process.env as any).X",
            true,
        ),
        ("let spy: ReturnType<typeof vi.spyOn> | undefined", false),
        // A call in a type runs nothing, so it cannot cancel a real accessor spy.
        (
            "type K = { [vi.spyOn(console, 'log')]: 1 }\nvi.spyOn(globalThis, 'Y', 'get')",
            true,
        ),
    ] {
        let tests = ConstantCase::new(
            &modules,
            &format!("import {{ test, vi }} from 'vitest'\nimport {{ X }} from './m'\n{setup}\ntest.each([[X]])('t %s', () => {{}})\n"),
        )
        .tests();
        assert_eq!(tests[0].1, unresolved, "{setup:?}: {tests:?}");
    }
}

#[test]
/// A local `const` alias of the import reads the same value whether or not the import is also
/// written in a table.
fn a_local_alias_of_the_import_resolves() {
    let tests = ConstantCase::new(
        &[("m.ts", "export const X = 'fast'")],
        "import { test } from 'vitest'\nimport { X } from './m'\nconst L = X\ntest.each([[L]])('t %s', () => {})\n",
    )
    .tests();
    assert_eq!(tests, vec![("t fast".to_owned(), false)]);
}

#[test]
/// An alias through a type wrapper, or a computation over the import, is an alias.
fn a_wrapped_or_computed_local_alias_of_the_import_resolves() {
    for (alias, title) in [("(X as string)!", "t fast"), ("`${X}-1`", "t fast-1")] {
        let tests = ConstantCase::new(
            &[("m.ts", "export const X = 'fast'")],
            &format!("import {{ test }} from 'vitest'\nimport {{ X }} from './m'\nconst L = {alias}\ntest.each([[L]])('t %s', () => {{}})\n"),
        )
        .tests();
        assert_eq!(tests, vec![(title.to_owned(), false)], "{alias}");
    }
}

#[test]
/// A mock of the module an `import … export` barrel imports from is a mock of the chain.
fn a_mock_of_the_module_behind_an_import_export_barrel_is_seen() {
    let modules = [
        ("values.ts", "export const X = 'fast'"),
        ("barrel.ts", "import { X } from './values'\nexport { X }"),
    ];
    for (mock, unresolved) in [
        ("", false),
        ("vi.mock('./values', () => ({ X: 'slow' }))", true),
    ] {
        let tests = ConstantCase::new(
            &modules,
            &format!("import {{ test, vi }} from 'vitest'\nimport {{ X }} from './barrel'\n{mock}\ntest.each([[X]])('t %s', () => {{}})\n"),
        )
        .tests();
        assert_eq!(tests[0].1, unresolved, "{mock:?}: {tests:?}");
    }
}

#[test]
/// Only a value over the import is an alias: imports read inside calls or composites are not
/// candidates, so they cannot push a table's own import past the bound.
fn only_value_aliases_make_an_import_a_candidate() {
    let (names, module) = bound_filling_module();
    let typeofs: String = names
        .iter()
        .map(|name| format!("const is{name} = typeof {name}\n"))
        .collect();
    for readers in [
        format!("const props = make({{ {} }})\n", names.join(", ")),
        typeofs,
    ] {
        let source = format!(
            "import {{ test }} from 'vitest'\nimport {{ {}, X }} from './m'\n{readers}test.each([[X]])('t %s', () => {{}})\n",
            names.join(", "),
        );
        let tests = ConstantCase::new(&[("m.ts", &module)], &source).tests();
        assert_eq!(tests, vec![("t fast".to_owned(), false)], "{readers}");
    }
}

#[test]
/// An import used as a whole table, or inside a callback that builds one, is not a cell: it
/// spends no proof, so the bound is left to the table's real cells.
fn non_cell_table_references_are_not_candidates() {
    let (names, module) = bound_filling_module();
    for table in ["[1].map(() => [{name}])", "{name}"] {
        let tables: String = names
            .iter()
            .map(|name| {
                let table = table.replace("{name}", name);
                format!("test.each({table})('u %s', () => {{}})\n")
            })
            .collect();
        let source = format!(
            "import {{ test }} from 'vitest'\nimport {{ {}, X }} from './m'\n{tables}test.each([[X]])('t %s', () => {{}})\n",
            names.join(", "),
        );
        let tests = ConstantCase::new(&[("m.ts", &module)], &source).tests();
        assert_eq!(tests.last(), Some(&("t fast".to_owned(), false)), "{table}");
    }
}

#[test]
/// A historical file is partial when a table cell reads a first-party import, and it opens no
/// present-day module to decide that.
fn a_historical_file_is_partial_only_for_cells_reading_an_import() {
    let modules = [(
        "m.ts",
        "export const cases = [[1]]\nexport let LET = 'a'\nexport const X = 'fast'",
    )];
    for (body, partial) in [
        ("test.each(cases)('t %s', () => {})", false),
        // The base opens no module, so any cell reading a first-party import counts.
        ("test.each([[LET]])('t %s', () => {})", true),
        ("test.each([[X]])('t %s', () => {})", true),
        // Read by no table: an alias in a test body, and a table the reader cannot open.
        ("test('t', () => { const doubled = `${X}${X}` })", false),
        ("test.each([[X]] as const)('t %s', () => {})", false),
    ] {
        let case = ConstantCase::new(
            &modules,
            &format!(
                "import {{ test }} from 'vitest'\nimport {{ cases, LET, X }} from './m'\n{body}\n"
            ),
        );
        let parses = crate::frontend::helpers::parse_count();
        let historical = extract_with_imports(&case.source, &case.file, Globals::Disabled, false);
        assert_eq!(
            crate::frontend::helpers::parse_count(),
            parses,
            "{body}: a revision baseline opens no present-day module"
        );
        assert_eq!(
            historical
                .reasons
                .contains(&ExclusionReason::HelperSourcesUnavailable),
            partial,
            "{body}: {:?}",
            historical.diagnostics
        );
    }
}

/// `MAX_IMPORTED_CONSTANTS` provable names `C0..`, and the module exporting them followed by
/// `X = 'fast'`: imports that would use up the bound before `X` if they were tried.
fn bound_filling_module() -> (Vec<String>, String) {
    let count = crate::frontend::registry::imported_constants::MAX_IMPORTED_CONSTANTS;
    let (names, module, _) = numbered_constants("C", count);
    (names, module + "export const X = 'fast'\n")
}

/// `count` names `{prefix}0..`, a module exporting each as its own name, and one table row each.
fn numbered_constants(prefix: &str, count: usize) -> (Vec<String>, String, String) {
    let names: Vec<String> = (0..count).map(|index| format!("{prefix}{index}")).collect();
    let module = names
        .iter()
        .map(|name| format!("export const {name} = '{name}'\n"))
        .collect();
    let rows = names
        .iter()
        .map(|name| format!("[{name}]"))
        .collect::<Vec<_>>()
        .join(", ");
    (names, module, rows)
}

#[test]
/// The proof follows whatever the reader reaches, so a member of a local object literal built
/// from the import reads it too.
fn a_member_of_a_local_object_over_the_import_resolves() {
    let tests = ConstantCase::new(
        &[("m.ts", "export const X = 'fast'")],
        "import { test } from 'vitest'\nimport { X } from './m'\nconst O = { a: X }\ntest.each([[O.a]])('t %s', () => {})\n",
    )
    .tests();
    assert_eq!(tests, vec![("t fast".to_owned(), false)]);
}

#[test]
/// Only import reads spend the bound: cells naming local bindings do not.
fn local_bindings_do_not_spend_the_bound() {
    let count = crate::frontend::registry::imported_constants::MAX_IMPORTED_CONSTANTS;
    let locals: String = (0..count)
        .map(|index| format!("const L{index} = 'l{index}'\n"))
        .collect();
    let rows: Vec<String> = (0..count).map(|index| format!("[L{index}]")).collect();
    let source = format!(
        "import {{ test }} from 'vitest'\nimport {{ X }} from './m'\n{locals}test.each([{}])('u %s', () => {{}})\ntest.each([[X]])('t %s', () => {{}})\n",
        rows.join(", ")
    );
    let tests = ConstantCase::new(&[("m.ts", "export const X = 'fast'")], &source).tests();
    assert_eq!(tests.last(), Some(&("t fast".to_owned(), false)));
}

#[test]
/// An import a historical file reads that is gone today (a renamed module) may have held a
/// constant in the base, so the base is partial; a Node builtin could not have.
fn a_historical_read_of_a_vanished_path_is_partial() {
    for (import, partial) in [
        ("import { X } from './gone'", true),
        // An unresolved package-shaped name may be an alias in the base's revision.
        ("import { X } from '@app/gone'", true),
        ("import { X } from 'node:os'", false),
    ] {
        let case = ConstantCase::new(
            &[],
            &format!(
                "import {{ test }} from 'vitest'\n{import}\ntest.each([[X]])('t %s', () => {{}})\n"
            ),
        );
        let historical = extract_with_imports(&case.source, &case.file, Globals::Disabled, false);
        assert_eq!(
            historical
                .reasons
                .contains(&ExclusionReason::HelperSourcesUnavailable),
            partial,
            "{import}"
        );
    }
}

#[test]
/// An import from an installed package can never be proved, so reading it spends no bound.
fn package_imports_do_not_spend_the_bound() {
    let count = crate::frontend::registry::imported_constants::MAX_IMPORTED_CONSTANTS;
    let (names, package, rows) = numbered_constants("P", count);
    let source = format!(
        "import {{ test }} from 'vitest'\nimport {{ {} }} from 'consts'\nimport {{ X }} from './m'\ntest.each([{rows}])('u %s', () => {{}})\ntest.each([[X]])('t %s', () => {{}})\n",
        names.join(", "),
    );
    let tests = ConstantCase::new(
        &[
            ("m.ts", "export const X = 'fast'"),
            (
                "node_modules/consts/package.json",
                r#"{ "name": "consts", "main": "index.js" }"#,
            ),
            ("node_modules/consts/index.js", &package),
        ],
        &source,
    )
    .tests();
    assert_eq!(tests.last(), Some(&("t fast".to_owned(), false)));
}

#[test]
/// A historical file whose helper call already recorded unavailable sources gets the reason once.
fn unavailable_sources_are_reported_once_per_file() {
    let case = ConstantCase::new(
        &[
            ("m.ts", "export const X = 'fast'"),
            (
                "runner.ts",
                "import { test } from 'vitest'\nexport function run() { test('r', () => {}) }",
            ),
        ],
        "import { test } from 'vitest'\nimport { X } from './m'\nimport { run } from './runner'\nrun()\ntest.each([[X]])('t %s', () => {})\n",
    );
    let historical = extract_with_imports(&case.source, &case.file, Globals::Disabled, false);
    let unavailable = historical
        .reasons
        .iter()
        .filter(|reason| **reason == ExclusionReason::HelperSourcesUnavailable)
        .count();
    assert_eq!(unavailable, 1, "{:?}", historical.diagnostics);
}

#[test]
/// A historical file's own text decides: a vanished mock path still leaves the base partial,
/// while a property rewrite the current tree also refuses does not.
fn a_historical_file_decides_from_its_own_text() {
    for (setup, partial) in [
        ("vi.mock('./old-name', () => ({}))", true),
        ("import * as ns from './m'\nvi.spyOn(ns, 'X', 'get')", false),
    ] {
        let case = ConstantCase::new(
            &[("m.ts", "export const X = 'fast'")],
            &format!("import {{ test, vi }} from 'vitest'\nimport {{ X }} from './m'\n{setup}\ntest.each([[X]])('t %s', () => {{}})\n"),
        );
        let historical = extract_with_imports(&case.source, &case.file, Globals::Disabled, false);
        assert_eq!(
            historical
                .reasons
                .contains(&ExclusionReason::HelperSourcesUnavailable),
            partial,
            "{setup}: {:?}",
            historical.diagnostics
        );
    }
}

#[test]
/// Unavailable sources a helper refusal already recorded, even after another reason, are not
/// recorded again for imported cells.
fn unavailable_sources_are_reported_once_after_another_helper_refusal() {
    use crate::frontend::registry::imported_constants::Proof;
    let source = "import { X } from './m'; X;";
    let file = SourceFile::without_project_context(
        PathBuf::from("t.test.ts"),
        crate::discovery::SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&parsed.program);
    let mut walker = Walker::new(
        source,
        &file,
        &parsed.program,
        &semantic.semantic,
        Globals::Disabled,
    );
    walker.refuse_helper_expansion(refusal(ExclusionReason::HelperLimit, "budget"));
    walker.refuse_helper_expansion(refusal(
        ExclusionReason::HelperSourcesUnavailable,
        "historical",
    ));
    walker.imported_constants = ImportedConstants::lazy(Box::new(|_| Proof::Unavailable));
    let symbol = semantic.semantic.scoping().symbol_ids().next().unwrap();
    assert!(walker.imported_constants.lookup(symbol).is_none());
    walker.finish_imported_constants();
    let recorded = walker
        .reasons
        .iter()
        .chain(walker.further_refusals.iter().map(|(reason, _)| reason))
        .filter(|reason| **reason == ExclusionReason::HelperSourcesUnavailable)
        .count();
    assert_eq!(recorded, 1);
}
