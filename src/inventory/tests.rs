use super::{inventory, Inventory, Label, Modules, Provenance, Setting, SiteKind};
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
                SiteKind::Wrapper => "wrapper".to_owned(),
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
        [SiteKind::Registers(Registers::Suite), SiteKind::Wrapper]
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
    assert_eq!(named.sites[0].kind, SiteKind::Wrapper);
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
        .all(|site| site.kind == SiteKind::Wrapper));
}
