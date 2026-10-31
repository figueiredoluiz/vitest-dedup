use super::{inventory, Inventory, Label, Modules, Provenance, Reach, Setting, SiteKind};
use crate::framework::vitest::DIALECT;
use crate::framework::{HookTiming, Registers};
use crate::lang::{parse, Lines};
use std::fs;
use std::path::Path;

/// The inventory of `main` (`a.test.ts`) in a project holding `files` besides it.
fn inventory_of(main: &str, files: &[(&str, &str)], globals: bool) -> (Inventory, String) {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    let path = dir.path().join("a.test.ts");
    fs::write(&path, main).unwrap();
    let parsed = parse(&path, main).unwrap();
    let setting = Setting {
        dialect: &DIALECT,
        globals,
        root: dir.path(),
    };
    let found = inventory(&path, main, &parsed, setting, &mut Modules::default());
    (found, main.to_owned())
}

/// Each site as `kind@line:column chain provenance`.
fn sites(main: &str, files: &[(&str, &str)], globals: bool) -> Vec<String> {
    let (found, source) = inventory_of(main, files, globals);
    let lines = Lines::new(&source);
    found
        .sites
        .iter()
        .map(|site| {
            let kind = match site.kind {
                SiteKind::Registers(Registers::Test) => "test".to_owned(),
                SiteKind::Registers(Registers::Suite) => "suite".to_owned(),
                SiteKind::Registers(Registers::Bench) => "bench".to_owned(),
                SiteKind::Registers(Registers::Hook(timing)) => format!("{timing:?}"),
                SiteKind::Wrapper(Reach::Tasks) => "wrapper".to_owned(),
                SiteKind::Wrapper(Reach::Hooks | Reach::Hook) => "hook-wrapper".to_owned(),
            };
            let (line, column) = lines.position(&source, site.position);
            let provenance = match site.provenance {
                Provenance::Direct => "",
                Provenance::Global => " global",
                Provenance::Wrapper => " via-wrapper",
            };
            format!(
                "{kind}@{line}:{column} {}{provenance}",
                site.chain.join(".")
            )
        })
        .collect()
}

#[test]
fn every_way_to_reach_the_api_is_a_site() {
    let found = sites(
        "import { test, describe, beforeEach, bench } from 'vitest'\n\
         import * as v from 'vitest'\n\
         const { it } = import.meta.vitest\n\
         const t = test\n\
         const my = test.extend({ a: 1 })\n\
         describe('s', () => {\n\
           beforeEach(() => {})\n\
           v.it('a', () => {})\n\
           it('b', () => {})\n\
           t.skip('c', () => {})\n\
           my('d', () => {})\n\
           test.each([[1], [2]])('e %s', (n) => {})\n\
           bench('f', () => {})\n\
           ;(only ? test.only : test)('g', () => {})\n\
         })",
        &[],
        false,
    );
    assert_eq!(
        found,
        [
            "suite@6:1 ",
            "BeforeEach@7:1 ",
            "test@8:3 it",
            "test@9:1 ",
            "test@10:3 skip",
            "test@11:1 ",
            "test@12:22 each.()",
            "bench@13:1 ",
            "test@14:3 ",
        ]
    );
}

#[test]
fn a_free_name_is_the_api_only_under_globals() {
    let main = "describe('s', () => { it('a', () => {}); afterAll(() => {}) })";
    assert!(sites(main, &[], false).is_empty());
    assert_eq!(
        sites(main, &[], true),
        [
            "suite@1:1  global",
            "test@1:23  global",
            "AfterAll@1:42  global"
        ]
    );
    // A declared name shadows the global.
    assert!(sites("const it = (a, b) => {}\nit('a', () => {})", &[], true).is_empty());
}

#[test]
fn lookalikes_and_fixture_builders_register_nothing() {
    let found = sites(
        "import { test } from 'vitest'\n\
         import { it } from 'other'\n\
         foo.test('a', () => {})\n\
         it('b', () => {})\n\
         test.extend({ a: 1 })\n\
         test.scoped({ a: 2 })\n\
         test.each([[1]])\n\
         vi.fn()",
        &[],
        false,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn wrappers_are_found_to_a_fixpoint_across_modules() {
    let helpers = "import { it } from 'vitest'\n\
                   export function shared(name) { it(name, () => {}) }\n\
                   export const twice = (name) => { shared(name); shared(name) }";
    let (found, _) = inventory_of(
        "import { describe } from 'vitest'\n\
         import { twice as run } from './again'\n\
         import './side'\n\
         describe('s', () => { run('x') })",
        &[
            ("helpers.ts", helpers),
            ("again.ts", "export { twice } from './helpers'"),
            (
                "side.ts",
                "import { test } from 'vitest'\ntest('loaded', () => {})",
            ),
        ],
        false,
    );
    let kinds: Vec<_> = found.sites.iter().map(|site| site.kind).collect();
    assert_eq!(
        kinds,
        [
            SiteKind::Registers(Registers::Suite),
            SiteKind::Wrapper(Reach::Tasks)
        ]
    );
    assert_eq!(found.sites[1].provenance, Provenance::Wrapper);
    assert_eq!(
        found.loads.len(),
        1,
        "the bare import of a registering module"
    );
    // A wrapper of this module is found too, through a second wrapper.
    let local = sites(
        "import { test } from 'vitest'\n\
         function one() { test('a', () => {}) }\n\
         const two = () => one()\n\
         two()",
        &[],
        false,
    );
    assert_eq!(
        local,
        [
            "test@2:18 ",
            "wrapper@3:19  via-wrapper",
            "wrapper@4:1  via-wrapper"
        ]
    );
}

/// A function expression named inside a `const` is a wrapper under either name, and a module that
/// re-exports any name of a registering module registers when it loads.
#[test]
fn a_wrapper_is_called_by_either_name_and_loads_pass_through_re_exports() {
    let (named, _) = inventory_of(
        "import { run } from './named'\nrun()",
        &[(
            "named.ts",
            "import { test } from 'vitest'\nexport const run = function inner() { test('a', () => {}) }",
        )],
        false,
    );
    assert_eq!(named.sites.len(), 1);
    assert_eq!(named.sites[0].kind, SiteKind::Wrapper(Reach::Tasks));
    let (loaded, _) = inventory_of(
        "import './barrel'",
        &[
            ("barrel.ts", "export { x } from './side'"),
            (
                "side.ts",
                "import { test } from 'vitest'\ntest('loaded', () => {})\nexport const x = 1",
            ),
        ],
        false,
    );
    assert_eq!(
        loaded.loads.len(),
        1,
        "the barrel loads the registering module"
    );
}

/// Under globals, a free registration function read as a value is an escape, as an imported one is.
#[test]
fn a_free_registration_function_read_as_a_value_escapes_under_globals() {
    let main = "[test].forEach((t) => t('a', () => {}))";
    assert_eq!(inventory_of(main, &[], true).0.escapes.len(), 1);
    assert!(inventory_of(main, &[], false).0.escapes.is_empty());
}

#[test]
fn a_call_handed_a_registration_function_may_register() {
    let found = sites(
        "import { it } from 'vitest'\nconst only = filterable(it)\nonly('a', () => {})",
        &[],
        false,
    );
    assert_eq!(found, ["wrapper@3:1  via-wrapper"]);
}

#[test]
fn registration_functions_read_as_values_are_escapes() {
    let (found, _) = inventory_of(
        "import { test } from 'vitest'\n\
         ;[test].forEach((t) => t('a', () => {}))\n\
         run(test.skip)\n\
         export { test }\n\
         typeof test",
        &[],
        false,
    );
    let escapes: Vec<_> = found
        .escapes
        .iter()
        .map(|escape| escape.text.as_str())
        .collect();
    assert_eq!(escapes, ["test", "test.skip"]);
}

#[test]
fn positions_are_where_v8_places_the_call() {
    let (found, source) = inventory_of(
        "import { test } from 'vitest'\n\
         test.skip('a', () => {})\n\
         test.each([\n  [1],\n])\n('b', () => {})\n\
         test('c', () => {})",
        &[],
        false,
    );
    let lines = Lines::new(&source);
    let at = |offset| lines.position(&source, offset);
    // The member's property, the `(` after a call callee (and its start), the callee.
    assert_eq!(at(found.sites[0].position), (2, 6));
    assert_eq!(at(found.sites[1].position), (6, 1));
    assert_eq!(at(found.sites[1].from), (3, 1));
    assert_eq!(found.sites[0].from, found.sites[0].position);
    assert_eq!(at(found.sites[2].position), (7, 1));
}

#[test]
fn labels_name_what_surrounds_a_site() {
    let (found, _) = inventory_of(
        "import { test, describe } from 'vitest'\n\
         describe('s', () => {\n\
           for (const x of xs) { if (x) test('a', () => {}) }\n\
           xs.forEach(() => { try { test('b', () => {}) } catch {} })\n\
         })\n\
         function later() { test('c', () => {}) }",
        &[],
        false,
    );
    let labels: Vec<_> = found.sites.iter().map(|site| site.labels.clone()).collect();
    assert_eq!(
        labels,
        [
            vec![],
            vec![Label::Conditional, Label::Loop, Label::RegistrationBody],
            vec![Label::Try, Label::Callback, Label::RegistrationBody],
            vec![Label::Function],
        ]
    );
}

#[test]
fn titles_are_read_when_the_source_spells_them() {
    let (found, _) = inventory_of(
        "import { test } from 'vitest'\n\
         test('plain', () => {})\n\
         test(`template`, () => {})\n\
         test(`a ${b}`, () => {})\n\
         test(name, () => {})",
        &[],
        false,
    );
    let titles: Vec<_> = found
        .sites
        .iter()
        .map(|site| site.title.as_deref())
        .collect();
    assert_eq!(titles, [Some("plain"), Some("template"), None, None]);
}

#[test]
fn hooks_carry_their_timing_and_registration_values_are_recorded() {
    let (found, _) = inventory_of(
        "import { test, aroundEach } from 'vitest'\n\
         const my = test.extend({})\n\
         aroundEach(async (run) => { await run() })",
        &[],
        false,
    );
    assert_eq!(
        found.sites[0].kind,
        SiteKind::Registers(Registers::Hook(HookTiming::AroundEach))
    );
    assert_eq!(found.registration_values.len(), 1);
}

#[test]
fn a_module_outside_the_root_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let inner = dir.path().join("inner");
    fs::create_dir_all(&inner).unwrap();
    fs::write(
        dir.path().join("helper.ts"),
        "import { it } from 'vitest'\nexport function shared() { it('a', () => {}) }",
    )
    .unwrap();
    let main = "import { shared } from '../helper'\nshared()";
    let path = inner.join("a.test.ts");
    fs::write(&path, main).unwrap();
    let parsed = parse(&path, main).unwrap();
    fn setting(root: &Path) -> Setting<'_> {
        Setting {
            dialect: &DIALECT,
            globals: false,
            root,
        }
    }
    let mut modules = Modules::default();
    let outside = inventory(&path, main, &parsed, setting(&inner), &mut modules);
    assert!(outside.sites.is_empty());
    let inside = inventory(
        &path,
        main,
        &parsed,
        setting(dir.path()),
        &mut Modules::default(),
    );
    assert_eq!(inside.sites.len(), 1);
}

/// One run's module cache serves projects with and without globals, each reading its own way.
#[test]
fn a_shared_helper_is_read_under_each_projects_globals() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("helper.ts"),
        "export function register() { test('a', () => {}) }",
    )
    .unwrap();
    let main = "import { register } from './helper'\nregister()";
    let path = dir.path().join("a.test.ts");
    fs::write(&path, main).unwrap();
    let parsed = parse(&path, main).unwrap();
    let setting = |globals| Setting {
        dialect: &DIALECT,
        globals,
        root: dir.path(),
    };
    let mut modules = Modules::default();
    let without = inventory(&path, main, &parsed, setting(false), &mut modules);
    assert!(without.sites.is_empty());
    let with = inventory(&path, main, &parsed, setting(true), &mut modules);
    assert_eq!(with.sites.len(), 1, "the helper registers under globals");
}

/// A module the depth limit cut short is read again from nearer the test file.
#[test]
fn a_lookup_cut_by_the_depth_limit_is_not_cached() {
    let dir = tempfile::tempdir().unwrap();
    for link in 0..10 {
        fs::write(
            dir.path().join(format!("c{link}.ts")),
            format!("export {{ register }} from './c{}'", link + 1),
        )
        .unwrap();
    }
    fs::write(
        dir.path().join("c10.ts"),
        "import { test } from 'vitest'\nexport function register() { test('a', () => {}) }",
    )
    .unwrap();
    let setting = Setting {
        dialect: &DIALECT,
        globals: false,
        root: dir.path(),
    };
    let mut modules = Modules::default();
    let mut read = |name: &str, main: &str| {
        let path = dir.path().join(name);
        fs::write(&path, main).unwrap();
        let parsed = parse(&path, main).unwrap();
        inventory(&path, main, &parsed, setting, &mut modules)
            .sites
            .len()
    };
    assert_eq!(
        read("far.test.ts", "import { register } from './c0'\nregister()"),
        0
    );
    assert_eq!(
        read(
            "near.test.ts",
            "import { register } from './c3'\nregister()"
        ),
        1
    );
}

#[test]
fn every_export_form_carries_a_wrapper() {
    let (found, _) = inventory_of(
        "import { renamed, fromBarrel } from './barrel'\n\
         import byDefault from './default'\n\
         import anonymous from './anonymous'\n\
         renamed()\nfromBarrel()\nbyDefault()\nanonymous()",
        &[
            (
                "helpers.ts",
                "import { it } from 'vitest'\n\
                 function shared() { it('a', () => {}) }\n\
                 const arrow = () => it('b', () => {})\n\
                 export { shared as renamed }\n\
                 export { arrow as fromBarrel }",
            ),
            ("barrel.ts", "export * from './helpers'"),
            (
                "default.ts",
                "import { it } from 'vitest'\nfunction named() { it('c', () => {}) }\nexport default named",
            ),
            (
                "anonymous.ts",
                "import { it } from 'vitest'\nexport default function () { it('d', () => {}) }",
            ),
        ],
        false,
    );
    assert_eq!(found.sites.len(), 4, "{:?}", found.sites);
    assert!(found
        .sites
        .iter()
        .all(|site| site.kind == SiteKind::Wrapper(Reach::Tasks)));
}

/// The kind and line of every wrapper call in `main`, in source order.
fn wrapper_calls(main: &str, files: &[(&str, &str)]) -> Vec<String> {
    sites(main, files, false)
        .into_iter()
        .filter(|site| site.contains("wrapper@"))
        .map(|site| site.split(':').next().unwrap().to_owned())
        .collect()
}

/// A wrapper whose body registers only hooks may register only hooks; one test in it, a call of a
/// wrapper that registers tests, or a registration function read as a value widens it.
#[test]
fn a_wrapper_registers_hooks_only_until_its_body_may_register_a_test() {
    let header = "import { test, beforeEach, afterEach, describe } from 'vitest'\n";
    let hooks = "function setup() { beforeEach(() => {}); afterEach(() => {}) }\n";
    assert_eq!(
        wrapper_calls(
            &format!("{header}{hooks}setup()\ndescribe('s', () => {{ setup() }})"),
            &[]
        ),
        ["hook-wrapper@3", "hook-wrapper@4"]
    );
    let widened = [
        "function setup() { beforeEach(() => {}); test('t', () => {}) }\n",
        "function tests() { test('t', () => {}) }\nfunction setup() { beforeEach(() => {}); tests() }\n",
        "function setup() { beforeEach(() => {}); [test].forEach((declare) => declare('t', () => {})) }\n",
    ];
    for body in widened {
        let calls = wrapper_calls(&format!("{header}{body}setup()"), &[]);
        assert_eq!(
            calls.last().unwrap().split('@').next(),
            Some("wrapper"),
            "{body}"
        );
    }
}

/// A wrapper is hooks-only only when its body runs nothing else: each other call, read, operator,
/// spread or default may run project code that registers a test the analysis does not see.
#[test]
fn a_hooks_only_wrapper_runs_nothing_but_hooks() {
    let files = [
        (
            "cases.ts",
            "import { test } from 'vitest'\nexport function cases() { test('hidden', () => {}) }",
        ),
        (
            "hooks.ts",
            "import { beforeEach } from 'vitest'\nexport default function () { beforeEach(() => {}) }",
        ),
        (
            "open.ts",
            "import { beforeEach } from 'vitest'\nexport default (run) => { beforeEach(() => {}); run() }",
        ),
    ];
    assert_eq!(
        wrapper_calls(
            "import hooks from './hooks'\nimport open from './open'\nhooks()\nopen(() => {})",
            &files
        ),
        ["hook-wrapper@3", "wrapper@4"]
    );
    let header = "import { beforeEach, afterEach, test } from 'vitest'; import * as v from 'vitest'\nimport * as helpers from './cases'\n\
                  const local = { cases() { test('hidden', () => {}) }, get read() { test('hidden', () => {}); return 0 } }\n";
    let closed = "function setup(timeout) { let n = 0; const state = { n, list: [n, 'a'], at: `t` }; \
                  if (timeout) { beforeEach(() => {}, 1000) } else afterEach(() => {}); return state }\nsetup()";
    assert_eq!(
        wrapper_calls(&format!("{header}{closed}"), &files),
        ["hook-wrapper@5"]
    );
    let async_closed =
        "async function setup() { const unset = undefined; v.beforeEach(() => {}); return }\nsetup()";
    assert_eq!(
        wrapper_calls(&format!("{header}{async_closed}"), &files),
        ["hook-wrapper@5"]
    );
    for open in [
        "function setup() { beforeEach(() => {}); helpers.cases() }",
        "function setup() { beforeEach(() => {}); local.cases() }",
        "function setup(run) { beforeEach(() => {}); run() }",
        "function setup(n = helpers.cases()) { beforeEach(() => {}) }",
        "function setup({ n }) { beforeEach(() => {}) }",
        "function setup() { beforeEach(() => {}); local.read }",
        "function setup() { const n = local.read; beforeEach(() => {}) }",
        "function setup() { beforeEach(() => {}, local.read) }",
        "function setup() { beforeEach(...[() => {}]) }",
        "function setup(n) { if (n + 1) beforeEach(() => {}) }",
        "function setup() { beforeEach(() => {}); return `${local.read}` }",
        "function setup() { beforeEach(() => {}); for (const n of [0]) {} }",
        "let setup = () => { beforeEach(() => {}) }; setup = helpers.cases",
        "function hooks() { beforeEach(() => {}) }; let setup = hooks; setup = helpers.cases",
        "function setup() { (helpers.cases() ? beforeEach : beforeEach)(() => {}) }",
        "async function setup() { beforeEach(() => {}); return local }",
        "const setup = async (n) => beforeEach(() => {}, n)",
        "function setup() { const config = globalConfig; beforeEach(() => {}, config) }",
        "function setup(timeout) { beforeEach(() => {}, timeout) }",
        "var setup = function () { beforeEach(() => {}) }; var setup = helpers.cases",
    ] {
        assert_eq!(
            wrapper_calls(&format!("{header}{open}\nsetup()"), &files),
            ["wrapper@5"],
            "{open}"
        );
    }
    let rewritten = "let api = v; api = { beforeEach: helpers.cases }; function setup() { api.beforeEach(() => {}) }\nsetup()";
    assert_eq!(
        wrapper_calls(&format!("{header}{rewritten}"), &files),
        ["wrapper@4", "wrapper@5"]
    );
}

/// A name's value is known only when one declaration binds it and nothing writes it: each way of
/// replacing it reaches the helper reading it, and a type annotation is not a second declaration.
#[test]
fn a_binding_rewritten_any_way_may_register_tests() {
    let files = [
        (
            "cases.ts",
            "import { test } from 'vitest'\nexport function cases() { test('hidden', () => {}) }",
        ),
        (
            "hooks.ts",
            "import { beforeEach } from 'vitest'\nexport const hook = beforeEach",
        ),
    ];
    let header = "import { beforeEach } from 'vitest'; import * as v from 'vitest'\nimport { cases } from './cases'\nimport { hook } from './hooks'\n";
    for (closed, open, sites) in [
        (
            "const kept: Function = beforeEach",
            "kept(() => {})",
            ["hook-wrapper@6"].as_slice(),
        ),
        (
            "let moved = beforeEach; moved = cases; const kept = moved",
            "kept(() => {})",
            &["wrapper@5", "wrapper@6"],
        ),
        (
            "const { beforeEach: picked } = v; picked = cases",
            "picked(() => {})",
            &["wrapper@5", "wrapper@6"],
        ),
        (
            "var kept = beforeEach; var { run: kept } = { run: cases }",
            "kept(() => {})",
            &["wrapper@5", "wrapper@6"],
        ),
        (
            "var kept = beforeEach; var [kept] = [cases]",
            "kept(() => {})",
            &["wrapper@5", "wrapper@6"],
        ),
        (
            "const kept = beforeEach; eval('kept')",
            "kept(() => {})",
            &["wrapper@5", "wrapper@6"],
        ),
        (
            "const kept = beforeEach; (eval)('kept')",
            "kept(() => {})",
            &["wrapper@5", "wrapper@6"],
        ),
    ] {
        assert_eq!(
            wrapper_calls(
                &format!("{header}{closed}\nfunction setup() {{ {open} }}\nsetup()"),
                &files
            ),
            sites,
            "{closed}"
        );
    }
    // An imported hook keeps the hook's argument rule, inside a helper and at the call.
    let imported = |body: &str| format!("{header}\nfunction setup(n) {{ {body} }}\nsetup(1)");
    assert_eq!(
        wrapper_calls(&imported("hook(() => {}, 1000)"), &files)
            .last()
            .map(String::as_str),
        Some("hook-wrapper@6")
    );
    assert_eq!(
        wrapper_calls(&imported("hook(() => {}, n)"), &files)
            .last()
            .map(String::as_str),
        Some("wrapper@6")
    );
    assert_eq!(
        wrapper_calls(&format!("{header}const n = 1\nhook(() => {{}}, n)"), &files),
        ["wrapper@5"]
    );
    assert_eq!(
        wrapper_calls(&format!("{header}hook`${{cases()}}`"), &files),
        ["wrapper@4", "wrapper@4"]
    );
    // `declare` gives a name no value, in each of its forms.
    for declared in [
        "declare const g: number",
        "declare function g(): number",
        "declare class g {}",
    ] {
        assert_eq!(
            wrapper_calls(&format!("{header}{declared}\nfunction setup() {{ const n = g; beforeEach(() => {{}}) }}\nsetup()"), &files),
            ["wrapper@6"],
            "{declared}"
        );
    }
}

/// Every declaration of a binding counts: a `var` naming a parameter or a catch binding is that
/// binding, and it holds what it was handed until the `var` runs. A type sharing a name does not.
#[test]
fn a_var_naming_a_parameter_or_catch_binding_may_register_tests() {
    let files = [(
        "cases.ts",
        "import { test } from 'vitest'\nexport function cases() { test('hidden', () => {}) }",
    )];
    let header = "import { test, beforeEach } from 'vitest'\nimport { cases } from './cases'\n";
    let body = "hook(() => {}); var hook = beforeEach";
    for source in [
        format!("function setup(hook) {{ {body} }}"),
        format!("const setup = function (hook) {{ {body} }}"),
        format!("const setup = (hook) => {{ {body} }}"),
        format!("const o = {{ setup(hook) {{ {body} }} }}"),
        format!("function setup(hook = cases) {{ {body} }}"),
        format!("function setup({{ hook }}) {{ {body} }}"),
        format!("function setup({{ a: [hook] }}) {{ {body} }}"),
        format!("try {{ throw cases }} catch (hook) {{ {body} }}"),
        format!("class K {{ constructor(private hook) {{ {body} }} }}"),
        format!("class K {{ constructor(readonly hook = cases) {{ {body} }} }}"),
    ] {
        assert_eq!(
            wrapper_calls(&format!("{header}{source}"), &files)
                .first()
                .map(String::as_str),
            Some("wrapper@3"),
            "{source}"
        );
    }
    for source in [
        "type t = number; const t = test; t('x', () => {})",
        "const t = test; type t = number; t('x', () => {})",
        "interface t { a: 1 }; const t = test; t('x', () => {})",
        "function other(hook) { return hook }; var hook = beforeEach; hook(() => {})",
        "const other = (hook) => hook; var hook = beforeEach; hook(() => {})",
    ] {
        assert_eq!(
            wrapper_calls(&format!("{header}{source}"), &files),
            Vec::<String>::new(),
            "{source}"
        );
    }
}

/// A namespace alias anything rewrites may hold anything, and so may all that is read from it.
#[test]
fn a_rewritten_namespace_alias_may_register_tests() {
    let files = [(
        "cases.ts",
        "import { test } from 'vitest'\nexport function cases() { test('hidden', () => {}) }",
    )];
    let header = "import * as v from 'vitest'\nimport { cases } from './cases'\n";
    let rewritten = "let api = v; api = { beforeEach: cases }\n";
    for read in [
        "api.beforeEach(() => {})",
        "api.test('x', () => {})",
        "api.describe('s', () => {})",
        "const b = api.beforeEach; b(() => {})",
        "const { beforeEach: b } = api; b(() => {})",
        "const other = api; other.beforeEach(() => {})",
        "(1 ? api.test : api.it)('x', () => {})",
    ] {
        assert_eq!(
            wrapper_calls(&format!("{header}{rewritten}{read}"), &files),
            ["wrapper@4"],
            "{read}"
        );
        assert_eq!(
            wrapper_calls(&format!("{header}const api = v\n{read}"), &files),
            Vec::<String>::new(),
            "{read}"
        );
    }
    assert_eq!(
        wrapper_calls(
            &format!("{header}var api = v; var api = {{ beforeEach: cases }}\napi.beforeEach(() => {{}})"),
            &files
        ),
        ["wrapper@4"]
    );
}

/// A namespace alias nothing rewrites is the namespace in the module importing it: a member reads
/// as it does on a local alias, and what is not a registration adds no site.
#[test]
fn a_namespace_alias_stays_the_namespace_across_an_export() {
    let exporting = "import * as v from 'vitest'\nimport { beforeEach } from 'vitest'\n\
                     const local = v\nexport const kept = v\nexport { v as renamed }\n\
                     export { local }\nexport default v\nexport const hook = beforeEach\n\
                     export let moved = v; moved = {}";
    let files = [
        ("api.ts", exporting),
        (
            "cast.ts",
            "import * as v from 'vitest'\nexport default v as any",
        ),
        (
            "paren.ts",
            "import * as v from 'vitest'\nexport default (v)",
        ),
        ("barrel.ts", "export { kept as viaBarrel } from './api'"),
        ("star.ts", "export * from './api'"),
    ];
    let found = |main: String| -> Vec<String> {
        sites(&main, &files, false)
            .into_iter()
            .map(|site| site.split(':').next().unwrap().to_owned())
            .collect()
    };
    for (import, read, site) in [
        (
            "{ kept } from './api'",
            "kept.test('x', () => {})",
            "test@2",
        ),
        (
            "{ renamed } from './api'",
            "renamed.test('x', () => {})",
            "test@2",
        ),
        (
            "{ local } from './api'",
            "local.test('x', () => {})",
            "test@2",
        ),
        ("api from './api'", "api.test('x', () => {})", "test@2"),
        ("api from './cast'", "api.test('x', () => {})", "test@2"),
        ("api from './paren'", "api.test('x', () => {})", "test@2"),
        (
            "{ viaBarrel } from './barrel'",
            "viaBarrel.test('x', () => {})",
            "test@2",
        ),
        (
            "{ kept } from './star'",
            "kept.test('x', () => {})",
            "test@2",
        ),
        (
            "{ kept } from './api'",
            "kept.beforeEach(() => {})",
            "BeforeEach@2",
        ),
        (
            "{ kept } from './api'",
            "const t = kept.test; t('x', () => {})",
            "test@2",
        ),
        (
            "{ kept } from './api'",
            "const { test: t } = kept; t('x', () => {})",
            "test@2",
        ),
        (
            "{ moved } from './api'",
            "moved.test('x', () => {})",
            "wrapper@2",
        ),
    ] {
        assert_eq!(found(format!("import {import}\n{read}")), [site], "{read}");
    }
    for read in ["kept.expect(1).toBe(1)", "kept.vi.fn()"] {
        assert_eq!(
            found(format!("import {{ kept }} from './api'\n{read}")),
            Vec::<String>::new(),
            "{read}"
        );
    }
    assert_eq!(
        wrapper_calls(
            "import { kept } from './api'\nfunction setup() { kept.beforeEach(() => {}) }\nsetup()",
            &files
        ),
        ["hook-wrapper@3"]
    );
    assert_eq!(
        wrapper_calls("import { hook } from './api'\nhook(() => {})", &files),
        ["hook-wrapper@2"]
    );
}

/// A star forwards the namespace only when nothing else in the module may be the source of the
/// name; otherwise the name may hold anything. A wrapper forwards as it always did.
#[test]
fn a_star_forwards_the_namespace_only_as_the_only_source_of_the_name() {
    let noop = "{ test() {} }";
    let found = |barrel: &str| -> Vec<String> {
        let files = [
            (
                "api.ts",
                "import * as v from 'vitest'\nexport const kept = v",
            ),
            ("noop.ts", "export const kept = { test() {} }"),
            ("barrel.ts", barrel),
        ];
        sites(
            "import { kept } from './barrel'\nkept.test('x', () => {})",
            &files,
            false,
        )
        .into_iter()
        .map(|site| site.split(':').next().unwrap().to_owned())
        .collect()
    };
    for visible in [
        "export * from './api'",
        "export * from './api'\nexport const other = 1",
    ] {
        assert_eq!(found(visible), ["test@2"], "{visible}");
    }
    for spelled in [
        format!("export * from './api'\nexport const kept = {noop}"),
        format!("export const kept = {noop}\nexport * from './api'"),
        "export * from './api'\nexport function kept() {}".to_owned(),
        "export * from './api'\nexport class kept {}".to_owned(),
        "export * from './api'\nexport enum kept { a }".to_owned(),
        "export * from './api'\nexport namespace kept { export function test() {} }".to_owned(),
        format!("export * from './api'\nexport const {{ kept }} = {{ kept: {noop} }}"),
        format!("export * from './api'\nconst o = {noop}\nexport {{ o as kept }}"),
        "export * from './api'\nexport { kept } from './noop'".to_owned(),
        "export * from './api'\nexport * as kept from './noop'".to_owned(),
        "export * from './api'\nexport type kept = number".to_owned(),
        "export * from './api'\nexport interface kept { a: 1 }".to_owned(),
        "export * from './api'\ntype o = number\nexport { o as kept }".to_owned(),
        "export * from './api'\ntype o = number\nexport type { o as kept }".to_owned(),
        "export * from './api'\nexport declare const kept: number".to_owned(),
        "export * from './noop'\nexport * from './api'".to_owned(),
        "export * from './api'\nexport * from './noop'".to_owned(),
    ] {
        assert_eq!(found(&spelled), ["wrapper@2"], "{spelled}");
    }
    // A wrapper that registers is never lost to a name spelled beside the star.
    let files = [
        (
            "cases.ts",
            "import { test } from 'vitest'\nexport function run() { test('x', () => {}) }",
        ),
        (
            "barrel.ts",
            "export * from './cases'\ntype o = number\nexport { o as run }",
        ),
    ];
    assert_eq!(
        wrapper_calls("import { run } from './barrel'\nrun()", &files),
        ["wrapper@2"]
    );
}

/// What follows a `return` or `throw` in its statement list never runs, so it registers nothing;
/// a function declared there is hoisted, and any other way past a statement stays reachable.
#[test]
fn a_statement_after_an_exit_registers_nothing() {
    let header = "import { describe, test } from 'vitest'\n";
    let found = |source: &str| -> Vec<String> {
        sites(&format!("{header}{source}"), &[], false)
            .into_iter()
            .map(|site| site.split(':').next().unwrap().to_owned())
            .collect()
    };
    let t = "test('t', () => {})";
    for dead in [
        format!("describe('s', () => {{ return; {t} }})"),
        format!("describe('s', () => {{ throw new Error('x'); {t} }})"),
        format!("describe('s', () => {{ return; {{ {t} }} }})"),
        format!("describe('s', () => {{ return; if (1) {{ {t} }} }})"),
        format!("describe('s', () => {{ return; describe('d', () => {{ {t} }}) }})"),
        format!("describe('s', () => {{ return; const run = () => {{ {t} }}; run() }})"),
    ] {
        assert_eq!(found(&dead), ["suite@2"], "{dead}");
    }
    assert_eq!(
        wrapper_calls(
            &format!("{header}function setup() {{ return; {t} }}\nsetup()"),
            &[]
        ),
        Vec::<String>::new()
    );
    for live in [
        format!("describe('s', () => {{ {t}; return }})"),
        format!("describe('s', () => {{ if (1) return; {t} }})"),
        format!("describe('s', () => {{ try {{ return }} finally {{ {t} }} }})"),
        format!("describe('s', () => {{ switch (1) {{ case 1: return; {t} }} }})"),
    ] {
        assert_eq!(found(&live), ["suite@2", "test@2"], "{live}");
    }
    assert_eq!(
        wrapper_calls(
            &format!("{header}function setup() {{ helper(); return; function helper() {{ {t} }} }}\nsetup()"),
            &[]
        ),
        ["wrapper@2", "wrapper@3"]
    );
}

/// A hook read as a global is the API until the module writes that global, under any spelling of
/// its key; a key only known when it runs may be that global.
#[test]
fn a_global_the_module_writes_may_register_tests() {
    let calls = |source: &str| -> Vec<String> {
        sites(source, &[], true)
            .into_iter()
            .filter(|site| site.contains("wrapper@"))
            .map(|site| site.split(':').next().unwrap().to_owned())
            .collect()
    };
    let call = "function setup() { beforeEach(() => {}) }\nsetup()";
    assert_eq!(calls(call), ["hook-wrapper@2"]);
    for write in [
        "globalThis.beforeEach = () => {}",
        "global.beforeEach = () => {}",
        "globalThis['beforeEach'] = () => {}",
        "globalThis[process.argv[2]] = () => {}",
        "({ beforeEach: globalThis.beforeEach } = { beforeEach() {} })",
        "[globalThis.beforeEach] = [() => {}]",
        "for (globalThis.beforeEach of [() => {}]) {}",
        "globalThis.beforeEach++",
        "(globalThis).beforeEach = () => {}",
        "(globalThis.beforeEach) = () => {}",
        "window.beforeEach = () => {}",
        "self.beforeEach = () => {}",
    ] {
        assert_eq!(
            calls(&format!("{write}\n{call}")),
            ["wrapper@2", "wrapper@3"],
            "{write}"
        );
    }
    for unrelated in [
        "globalThis.other = 1",
        "globalThis['other'] = 1",
        "globalThis['*'] = 1",
        "const local = {}; local.beforeEach = 1",
        "const globalThis = {}; globalThis.beforeEach = 1",
    ] {
        assert_eq!(
            calls(&format!("{unrelated}\n{call}")),
            ["hook-wrapper@3"],
            "{unrelated}"
        );
    }
}

/// Reach is a fixpoint over recursion: two hooks-only wrappers calling each other stay hooks-only,
/// and a test in either widens both, whichever is declared first.
#[test]
fn recursive_wrappers_take_the_widest_reach_of_their_cycle() {
    let header = "import { test, beforeEach } from 'vitest'\n";
    let pair = |extra: &str| {
        format!(
            "{header}function a(n) {{ beforeEach(() => {{}}); if (n) b(0) }}\n\
             function b(n) {{ beforeEach(() => {{}}); {extra}if (n) a(0) }}\n\
             a(2)\nb(2)"
        )
    };
    assert_eq!(
        wrapper_calls(&pair(""), &[]),
        [
            "hook-wrapper@2",
            "hook-wrapper@3",
            "hook-wrapper@4",
            "hook-wrapper@5"
        ]
    );
    assert_eq!(
        wrapper_calls(&pair("test('t', () => {}); "), &[]),
        ["wrapper@2", "wrapper@3", "wrapper@4", "wrapper@5"]
    );
}

/// An alias of a wrapper, and a value a call handed a registration function returns, follow the
/// wrapper's final reach: an alias taken before the wrapper widened is widened with it.
#[test]
fn an_alias_follows_its_wrapper_and_a_derived_value_may_register_tests() {
    let header = "import { it, test, beforeEach } from 'vitest'\n";
    let calls = wrapper_calls(
        &format!(
            "{header}const early = setup\n\
             function setup() {{ beforeEach(() => {{}}); helper() }}\n\
             function helper() {{ test('t', () => {{}}) }}\n\
             early()"
        ),
        &[],
    );
    assert_eq!(calls.last().map(String::as_str), Some("wrapper@5"));
    let derived = wrapper_calls(
        &format!("{header}const filterable = (t) => t\nfilterable(it)('x', () => {{}})"),
        &[],
    );
    assert_eq!(derived, ["wrapper@3"]);
}

/// What an imported wrapper registers crosses modules and barrel re-exports.
#[test]
fn an_imported_wrapper_keeps_its_reach_through_re_exports() {
    let files = [
        (
            "server.ts",
            "import { beforeAll, afterAll } from 'vitest'\n\
             export function setupServer() { beforeAll(() => {}); afterAll(() => {}) }",
        ),
        (
            "cases.ts",
            "import { test } from 'vitest'\nexport function cases() { test('t', () => {}) }",
        ),
        (
            "barrel.ts",
            "export { setupServer } from './server'\n\
             export { cases as named } from './cases'\n\
             export * from './cases'",
        ),
        (
            "aliases.ts",
            "import { beforeEach } from 'vitest'\nimport { cases } from './cases'\n\
             export const kept = beforeEach\nlet moved = beforeEach\nmoved = cases\nexport { moved }",
        ),
    ];
    // A hook alias exported as is stays hooks-only; one reassigned may hold anything.
    assert_eq!(
        wrapper_calls(
            "import { kept, moved } from './aliases'\nkept(() => {})\nmoved(() => {})",
            &files
        ),
        ["hook-wrapper@2", "wrapper@3"]
    );
    // Arguments are evaluated before the call: one that runs code may register a test.
    assert_eq!(
        wrapper_calls(
            "import { setupServer } from './server'\nimport * as h from './cases'\n\
             setupServer('a', () => {})\nsetupServer(h.cases())\nsetupServer`${h.cases()}`",
            &files
        ),
        ["hook-wrapper@3", "wrapper@4", "wrapper@5"]
    );
    assert_eq!(
        wrapper_calls(
            "import { setupServer, cases, named } from './barrel'\nsetupServer()\ncases()\nnamed()",
            &files
        ),
        ["hook-wrapper@2", "wrapper@3", "wrapper@4"]
    );
    assert_eq!(
        wrapper_calls(
            "import { setupServer } from './server'\nsetupServer()",
            &files
        ),
        ["hook-wrapper@2"]
    );
}

/// Only a call of a hooks-only wrapper, or of an alias of it, is hooks-only: a member of it, read
/// or destructured, or a call of what it returns is any value, which may register a test.
#[test]
fn a_member_of_a_hooks_only_wrapper_may_register_tests() {
    let files = [(
        "setup.ts",
        "import { beforeEach, test } from 'vitest'\n\
         export function setup() { beforeEach(() => {}) }\n\
         setup.test = test",
    )];
    assert_eq!(
        wrapper_calls(
            "import { setup } from './setup'\nconst alias = setup\nconst member = setup.test\n\
             setup()\nalias()\nsetup.test('hidden', () => {})\nmember('also', () => {})",
            &files
        ),
        ["hook-wrapper@4", "hook-wrapper@5", "wrapper@6", "wrapper@7"]
    );
    assert_eq!(
        wrapper_calls(
            "import { setup } from './setup'\nconst { test: register } = setup\n\
             const [first] = setup\nsetup()\nregister('hidden', () => {})\nfirst()",
            &files
        ),
        ["hook-wrapper@4", "wrapper@5", "wrapper@6"]
    );
}

/// A hooks-only helper calling a function that only reads a registration function as a value may
/// register tests.
#[test]
fn a_helper_reading_a_registration_function_may_register_tests() {
    let files = [(
        "cases.ts",
        "import { beforeEach, test } from 'vitest'\n\
         function cases() { [test].forEach((t) => t('hidden', () => {})) }\n\
         export function setup() { beforeEach(() => {}); cases() }\n\
         export function hooks() { beforeEach(() => {}) }",
    )];
    assert_eq!(
        wrapper_calls(
            "import { setup, hooks } from './cases'\nsetup()\nhooks()",
            &files
        ),
        ["wrapper@2", "hook-wrapper@3"]
    );
}

/// A cycle's members read its head before it is known, so a call of what they could not see is
/// not hooks-only, in the head, in a module reading it, or in the test file.
#[test]
fn a_cycle_head_and_its_readers_may_register_tests() {
    let files = [
        (
            "a.ts",
            "import { beforeEach, test } from 'vitest'\nimport { b } from './b'\n\
             export function setup() { beforeEach(() => {}); b() }\n\
             export function cases() { test('hidden', () => {}) }\n\
             export function only() { b() }",
        ),
        (
            "b.ts",
            "import { cases } from './a'\nexport function b() { cases() }",
        ),
        (
            "c.ts",
            "import { beforeEach } from 'vitest'\nimport { setup } from './a'\n\
             export function around() { beforeEach(() => {}); setup() }",
        ),
        (
            "plain.ts",
            "import { beforeEach } from 'vitest'\nexport function hooks() { beforeEach(() => {}) }",
        ),
    ];
    assert_eq!(
        wrapper_calls("import { setup } from './a'\nsetup()", &files),
        ["wrapper@2"]
    );
    assert_eq!(
        wrapper_calls("import { around } from './c'\naround()", &files),
        ["wrapper@2"]
    );
    // A test file's own hooks-only helper calling an export the cycle left out may register tests.
    assert_eq!(
        wrapper_calls(
            "import { beforeEach } from 'vitest'\nimport { only } from './a'\n\
             function setup() { beforeEach(() => {}); only() }\nsetup()",
            &files
        ),
        ["wrapper@4"]
    );
    // A test file reading only complete modules keeps its hooks-only calls.
    assert_eq!(
        wrapper_calls("import { hooks } from './plain'\nhooks()", &files),
        ["hook-wrapper@2"]
    );
}

/// Modules that import each other read the cycle's head before it is known, so a hooks-only
/// answer there may hide a test: what they export may register tests, whichever is read first.
#[test]
fn a_wrapper_in_an_import_cycle_may_register_tests() {
    let files = [
        (
            "a.ts",
            "import { beforeEach } from 'vitest'\nimport { b } from './b'\n\
             export function a(n) { beforeEach(() => {}); if (n) b(n - 1) }",
        ),
        (
            "b.ts",
            "import { test } from 'vitest'\nimport { a } from './a'\n\
             export function b(n) { test('t', () => {}); if (n) a(n - 1) }",
        ),
    ];
    for imports in [
        "import { b } from './b'\nimport { a } from './a'",
        "import { a } from './a'\nimport { b } from './b'",
    ] {
        assert_eq!(
            wrapper_calls(&format!("{imports}\na(1)"), &files),
            ["wrapper@3"],
            "{imports}"
        );
    }
}

/// Past the import depth limit a module reads as registering nothing, so every module whose answer
/// rests on that cut may register tests.
#[test]
fn a_wrapper_past_the_depth_limit_may_register_tests() {
    let modules: Vec<(String, String)> = (0..12)
        .map(|n| {
            let source = if n == 11 {
                format!("import {{ test }} from 'vitest'\nexport function f{n}() {{ test('t', () => {{}}) }}")
            } else {
                format!(
                    "import {{ beforeEach }} from 'vitest'\nimport {{ f{next} }} from './m{next}'\n\
                     export function f{n}() {{ beforeEach(() => {{}}); f{next}() }}",
                    next = n + 1
                )
            };
            (format!("m{n}.ts"), source)
        })
        .collect();
    let files: Vec<(&str, &str)> = modules
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect();
    assert_eq!(
        wrapper_calls("import { f0 } from './m0'\nf0()", &files),
        ["wrapper@2"]
    );
}
