//! The reachability proof that a call or an import registers no test, hook or mock.
use super::registration_free::{
    call_registers_nothing, imports_register_nothing, is_known_runtime_export,
    is_known_runtime_module,
};
use std::path::PathBuf;

/// A project on disk: each `(relative path, text)` pair is written under a fresh root.
fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    for (relative, text) in files {
        let path = directory.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    directory
}

fn path(directory: &tempfile::TempDir, relative: &str) -> PathBuf {
    directory.path().join(relative)
}

/// Whether importing `relative` is proved harmless.
fn imports(directory: &tempfile::TempDir, relative: &str) -> bool {
    imports_register_nothing(&path(directory, relative))
}

/// Whether calling `export` of `relative` is proved harmless.
fn calls(directory: &tempfile::TempDir, relative: &str, export: &str) -> bool {
    call_registers_nothing(&path(directory, relative), export).is_some()
}

/// A function that never names a registration API registers nothing; one that does is not proved,
/// whichever way the name reaches it.
#[test]
fn a_function_is_proved_by_what_it_can_reach() {
    let directory = project(&[
        ("pure.ts", "export function run(a: number) { return a + 1 }"),
        (
            "named.ts",
            "import { test } from 'vitest'\nexport function run() { test('a', () => {}) }",
        ),
        (
            "aliased.ts",
            "import { test as t } from 'vitest'\nexport const run = () => { t('a', () => {}) }",
        ),
        (
            "namespace.ts",
            "import * as v from 'vitest'\nexport const run = () => v.test('a', () => {})",
        ),
        (
            "global.ts",
            "export function run() { describe('suite', () => {}) }",
        ),
        (
            "hook.ts",
            "import { beforeEach } from 'vitest'\nexport function run() { beforeEach(() => {}) }",
        ),
        (
            "shadowed.ts",
            "export function run(it: number[]) { return it.map((test) => test + 1) }",
        ),
        (
            "typeonly.ts",
            "import type { test } from 'vitest'\nexport function run(): typeof test | null { return null }",
        ),
    ]);
    assert!(calls(&directory, "pure.ts", "run"));
    for refused in [
        "named.ts",
        "aliased.ts",
        "namespace.ts",
        "global.ts",
        "hook.ts",
    ] {
        assert!(!calls(&directory, refused, "run"), "{refused}");
    }
    // A local that happens to be called `test`, and a name used only as a type, are not the API.
    assert!(calls(&directory, "shadowed.ts", "run"));
    assert!(calls(&directory, "typeonly.ts", "run"));
}

/// The first-party proof reads the syntax tree, so how a registration global is separated from its
/// arguments does not matter.
#[test]
fn registration_globals_are_refused_however_the_call_is_spelled() {
    let spellings = [
        "describe('a', () => {})",
        "describe\n('a', () => {})",
        "describe /* c */ ('a', () => {})",
        "describe?.('a', () => {})",
        "describe`a`",
        "describe\n.each([1])('a', () => {})",
        "describe?.each([1])('a', () => {})",
    ];
    let mut files: Vec<(String, String)> = spellings
        .iter()
        .enumerate()
        .map(|(index, call)| {
            (
                format!("spelled{index}.ts"),
                format!("export function run() {{ {call} }}"),
            )
        })
        .collect();
    files.push((
        "near.ts".to_owned(),
        "export function run(describe: number[]) { return describe.map((x) => x) }".to_owned(),
    ));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    let directory = project(&borrowed);
    for (index, spelling) in spellings.iter().enumerate() {
        assert!(
            !calls(&directory, &format!("spelled{index}.ts"), "run"),
            "{spelling}"
        );
    }
    assert!(calls(&directory, "near.ts", "run"));
}

/// Only code that runs is judged: a module that also holds a hook-registering function is harmless
/// to import and to call through its other exports, and refused when the registering one runs.
#[test]
fn a_function_the_call_never_reaches_does_not_count() {
    let directory = project(&[(
        "mixed.ts",
        "import { beforeAll } from 'vitest'\n\
         const state = { ready: false }\n\
         export function pure() { return state.ready }\n\
         export function hooked() { beforeAll(() => { state.ready = true }) }\n\
         export function viaLocal() { return helper() }\n\
         function helper() { return hooked() }\n",
    )]);
    assert!(imports(&directory, "mixed.ts"));
    assert!(calls(&directory, "mixed.ts", "pure"));
    assert!(!calls(&directory, "mixed.ts", "hooked"));
    // Reaching the registering function through a local call counts as reaching it.
    assert!(!calls(&directory, "mixed.ts", "viaLocal"));
}

/// Importing runs top-level initialisers: a call that registers there refuses the import, a
/// function body that is merely defined does not, and bare statements are refused outright. `vi`
/// and `expect` used while the module loads refuse it too: the effect reaches every test in the
/// importing file, and no context records it.
#[test]
fn importing_runs_initialisers_not_function_bodies() {
    let directory = project(&[
        (
            "defined.ts",
            "export const run = () => { describe('x', () => {}) }\nexport function other() { it('y', () => {}) }",
        ),
        (
            "initialiser.ts",
            "import { describe } from 'vitest'\nexport const suite = describe('x', () => {})",
        ),
        (
            "via-helper.ts",
            "import { it } from 'vitest'\nfunction make() { return it('y', () => {}) }\nexport const made = make()",
        ),
        ("statement.ts", "setup()\nexport const a = 1"),
        ("control.ts", "if (true) { console.info('x') }\nexport const a = 1"),
        (
            "framework.ts",
            "import { vi } from 'vitest'\nvi.mock('./dep', () => ({ value: 1 }))\nexport const a = vi.fn()",
        ),
        ("class.ts", "export class Box { static value = compute() }"),
        ("inert-class.ts", "export class Box { value = compute(); run() {} }"),
    ]);
    assert!(imports(&directory, "defined.ts"));
    assert!(!imports(&directory, "initialiser.ts"));
    assert!(!imports(&directory, "via-helper.ts"));
    assert!(!imports(&directory, "statement.ts"));
    assert!(!imports(&directory, "control.ts"));
    assert!(!imports(&directory, "framework.ts"));
    assert!(!imports(&directory, "class.ts"));
    assert!(imports(&directory, "inert-class.ts"));
}

/// The file's own extension picks the syntax: JSX parses in `.tsx` and `.jsx`.
#[test]
fn syntax_follows_the_file_extension() {
    let directory = project(&[
        ("view.tsx", "export const run = () => <div>{1}</div>"),
        ("view.jsx", "export const run = () => <div>{1}</div>"),
        ("plain.js", "export const run = () => 1"),
        ("broken.ts", "export const run = () => <div>"),
    ]);
    assert!(calls(&directory, "view.tsx", "run"));
    assert!(calls(&directory, "view.jsx", "run"));
    assert!(calls(&directory, "plain.js", "run"));
    assert!(!calls(&directory, "broken.ts", "run"));
}

/// What is taken from Vitest is judged by name wherever it passes through, re-exports included;
/// hooks added after the first generation of the API count too.
#[test]
fn registration_names_are_judged_through_re_exports_and_include_the_around_hooks() {
    let directory = project(&[
        ("bridge.ts", "export { beforeEach, expect } from 'vitest'"),
        ("star.ts", "export * from 'vitest'"),
        (
            "through-bridge.ts",
            "import { beforeEach } from './bridge'\nexport function run() { beforeEach(() => {}) }",
        ),
        (
            "harmless-bridge.ts",
            "import { expect } from './bridge'\nexport function run() { return expect(1) }",
        ),
        (
            "through-star.ts",
            "import { describe } from './star'\nexport function run() { describe('x', () => {}) }",
        ),
        (
            "around.ts",
            "import { aroundEach } from 'vitest'\nexport function run() { aroundEach(async (inner) => { await inner() }) }",
        ),
        (
            "around-all.ts",
            "import { aroundAll } from 'vitest'\nexport function run() { aroundAll(async (inner) => { await inner() }) }",
        ),
        ("namespace.ts", "export const run = () => import('vitest')"),
    ]);
    assert!(!calls(&directory, "through-bridge.ts", "run"));
    assert!(calls(&directory, "harmless-bridge.ts", "run"));
    assert!(!calls(&directory, "through-star.ts", "run"));
    assert!(!calls(&directory, "around.ts", "run"));
    assert!(!calls(&directory, "around-all.ts", "run"));
    assert!(!calls(&directory, "namespace.ts", "run"));
}

/// Enum members are evaluated when the module is imported.
#[test]
fn enum_initialisers_run_at_import() {
    let directory = project(&[
        (
            "registers.ts",
            "import { beforeEach } from 'vitest'\nenum Boot { X = (beforeEach(() => {}), 1) }\nexport function run() { return Boot.X }",
        ),
        (
            "exported.ts",
            "import { beforeEach } from 'vitest'\nexport enum Boot { X = (beforeEach(() => {}), 1) }\nexport function run() { return 1 }",
        ),
        ("benign.ts", "export enum Mode { A = 1, B = A << 1 }\nexport function run() { return Mode.B }"),
    ]);
    assert!(!imports(&directory, "registers.ts"));
    assert!(!imports(&directory, "exported.ts"));
    assert!(imports(&directory, "benign.ts"));
}

/// A Node module that runs code from text or in another context defeats a lexical proof.
#[test]
fn node_modules_that_run_code_from_text_are_refused() {
    let directory = project(&[
        ("vm.ts", "import { runInThisContext } from 'node:vm'\nexport const run = () => runInThisContext(\"1\")"),
        ("bare-vm.ts", "import vm from 'vm'\nexport const run = () => vm"),
        ("child.ts", "import { execSync } from 'node:child_process'\nexport const run = () => execSync"),
        ("workers.ts", "import { Worker } from 'node:worker_threads'\nexport const run = () => Worker"),
        ("module.ts", "import { createRequire } from 'node:module'\nexport const run = () => createRequire"),
        ("fs.ts", "import { readFileSync } from 'node:fs'\nimport path from 'path'\nexport const run = () => [readFileSync, path]"),
    ]);
    for refused in ["vm.ts", "bare-vm.ts", "child.ts", "workers.ts", "module.ts"] {
        assert!(!calls(&directory, refused, "run"), "{refused}");
    }
    assert!(calls(&directory, "fs.ts", "run"));
}

/// `#imports` is the generated list under another spelling: each name stands for what the list says.
#[test]
fn explicit_nuxt_imports_are_judged_like_listed_auto_imports() {
    let directory = project(&[
        ("pure.ts", "import { usePure } from '#imports'\nexport const run = () => usePure()"),
        ("hooked.ts", "import { useHooked } from '#imports'\nexport const run = () => useHooked()"),
        ("unlisted.ts", "import { useUnlisted } from '#imports'\nexport const run = () => useUnlisted()"),
        ("composables/pure.ts", "export function usePure() { return 1 }"),
        (
            "composables/hooked.ts",
            "import { beforeEach } from 'vitest'\nexport function useHooked() { beforeEach(() => {}) }",
        ),
        (
            ".nuxt/imports.d.ts",
            "export { usePure } from '../composables/pure';\nexport { useHooked } from '../composables/hooked';\n",
        ),
    ]);
    assert!(calls(&directory, "pure.ts", "run"));
    assert!(!calls(&directory, "hooked.ts", "run"));
    assert!(!calls(&directory, "unlisted.ts", "run"));
}

/// `import.meta.vitest` hands over the registration API; other `import.meta` metadata is fine.
#[test]
fn import_meta_vitest_is_a_registration_route() {
    let directory = project(&[
        (
            "url.ts",
            "export const run = () => new URL('./x.json', import.meta.url)",
        ),
        ("env.ts", "export const run = () => import.meta.env.MODE"),
        (
            "vitest.ts",
            "export const run = () => { const { it } = import.meta.vitest!; it('x', () => {}) }",
        ),
        (
            "computed.ts",
            "export const run = () => import.meta['vitest']",
        ),
        (
            "destructured.ts",
            "export const run = () => { const { vitest } = import.meta; return vitest }",
        ),
    ]);
    assert!(calls(&directory, "url.ts", "run"));
    assert!(calls(&directory, "env.ts", "run"));
    for refused in ["vitest.ts", "computed.ts", "destructured.ts"] {
        assert!(!calls(&directory, refused, "run"), "{refused}");
    }
}

/// Generated build modules can re-export the project's own code; only the runtime internals are
/// harmless.
#[test]
fn generated_build_modules_are_not_trusted_blindly() {
    let directory = project(&[
        ("build.ts", "import { useSetup } from '#build/imports.mjs'\nexport const run = () => useSetup()"),
        ("app.ts", "import { useRuntimeConfig } from '#app/nuxt'\nexport const run = () => useRuntimeConfig()"),
    ]);
    assert!(!calls(&directory, "build.ts", "run"));
    assert!(calls(&directory, "app.ts", "run"));
}

/// Under Vitest globals an installed package can register tests without importing anything.
#[test]
fn a_package_that_calls_a_registration_global_is_refused_under_globals() {
    let directory = project(&[
        (
            "node_modules/setup-kit/package.json",
            r#"{"name":"setup-kit","main":"index.js"}"#,
        ),
        (
            "node_modules/setup-kit/index.js",
            "exports.run = () => test('hidden', () => {})",
        ),
        (
            "uses.ts",
            "import { run } from 'setup-kit'\nexport const go = () => run()",
        ),
    ]);
    super::registration_free::begin_file(false);
    assert!(calls(&directory, "uses.ts", "go"));
    super::registration_free::forget();
    super::registration_free::begin_file(true);
    assert!(!calls(&directory, "uses.ts", "go"));
    super::registration_free::begin_file(false);
}

/// Calling an export imports its module first, so the module's import-time code is judged too.
#[test]
fn calling_an_export_also_judges_the_import() {
    let directory = project(&[(
        "registers-on-import.ts",
        "import { describe } from 'vitest'\nexport const suite = describe('x', () => {})\nexport function run() { return 1 }",
    )]);
    assert!(!imports(&directory, "registers-on-import.ts"));
    assert!(!calls(&directory, "registers-on-import.ts", "run"));
}

/// The proof follows what the code reads across modules, by export; a hop that never reaches a
/// declaration, an import that does not resolve and an unread import behave as stated.
#[test]
fn the_proof_follows_imports_and_re_exports() {
    let directory = project(&[
        (
            "registrar.ts",
            "import { test } from 'vitest'\nexport const registers = () => test('a', () => {})\nexport const pure = () => 1",
        ),
        (
            "uses-pure.ts",
            "import { pure } from './registrar'\nexport const run = () => pure()",
        ),
        (
            "uses-registers.ts",
            "import { registers } from './registrar'\nexport const run = () => registers()",
        ),
        (
            "unreferenced.ts",
            "import { registers } from './registrar'\nexport const run = () => 1",
        ),
        ("barrel.ts", "export { pure as renamed } from './registrar'"),
        ("star.ts", "export * from './registrar'"),
        ("namespace.ts", "import * as r from './registrar'\nexport const run = () => r.pure()"),
        (
            "namespace-registers.ts",
            "import * as r from './registrar'\nexport const run = () => r.registers()",
        ),
        ("whole-namespace.ts", "import * as r from './registrar'\nexport const run = () => r"),
        ("cycle-a.ts", "export { x } from './cycle-b'"),
        ("cycle-b.ts", "export { x } from './cycle-a'"),
        ("mutual-a.ts", "import { b } from './mutual-b'\nexport const a = () => b()"),
        ("mutual-b.ts", "import { a } from './mutual-a'\nexport const b = () => a()"),
        ("missing.ts", "import { x } from './absent'\nexport const run = () => x()"),
        (
            "unread.ts",
            "import { Shape } from 'package-that-is-not-installed'\nexport const run = (): Shape | null => null",
        ),
        (
            "side-effect.ts",
            "import 'package-that-is-not-installed'\nexport const run = () => 1",
        ),
        (
            "dynamic.ts",
            "export const run = (name: string) => import(name)",
        ),
        ("literal-dynamic.ts", "export const run = () => import('./registrar')"),
    ]);
    assert!(calls(&directory, "uses-pure.ts", "run"));
    assert!(!calls(&directory, "uses-registers.ts", "run"));
    // An import nothing reads is loaded, but only its import-time code runs.
    assert!(calls(&directory, "unreferenced.ts", "run"));
    assert!(calls(&directory, "barrel.ts", "renamed"));
    assert!(calls(&directory, "star.ts", "pure"));
    assert!(!calls(&directory, "star.ts", "registers"));
    assert!(calls(&directory, "namespace.ts", "run"));
    assert!(!calls(&directory, "namespace-registers.ts", "run"));
    assert!(!calls(&directory, "whole-namespace.ts", "run"));
    // A re-export that only points at itself never reaches a declaration; mutual calls do.
    assert!(!calls(&directory, "cycle-a.ts", "x"));
    assert!(calls(&directory, "mutual-a.ts", "a"));
    assert!(!calls(&directory, "missing.ts", "run"));
    assert!(calls(&directory, "unread.ts", "run"));
    // A side-effect import runs the module, so one that cannot be found cannot be proved.
    assert!(!calls(&directory, "side-effect.ts", "run"));
    assert!(!calls(&directory, "dynamic.ts", "run"));
    assert!(!calls(&directory, "literal-dynamic.ts", "run"));
}

/// Reaching a registration API by name is refused, whichever spelling; one named property of the
/// global object is not.
#[test]
fn reflective_access_is_refused_but_a_named_global_property_is_not() {
    let directory = project(&[
        (
            "property.ts",
            "export const run = () => { (globalThis as { ready?: boolean }).ready = true }",
        ),
        (
            "plain-property.ts",
            "export const run = () => { globalThis.ready = true }",
        ),
        (
            "registration.ts",
            "export const run = () => globalThis.describe('x', () => {})",
        ),
        ("bare.ts", "export const run = () => globalThis"),
        (
            "computed.ts",
            "export const run = (n: string) => globalThis[n]",
        ),
        ("eval.ts", "export const run = () => eval('1')"),
        ("require.ts", "export const run = () => require('./x')"),
        (
            "glob.ts",
            "export const run = () => import.meta.glob('./*.test.ts')",
        ),
        (
            "window.ts",
            "export const run = () => window.scrollTo(0, 0)",
        ),
    ]);
    assert!(calls(&directory, "property.ts", "run"));
    assert!(calls(&directory, "plain-property.ts", "run"));
    assert!(calls(&directory, "window.ts", "run"));
    for refused in [
        "registration.ts",
        "bare.ts",
        "computed.ts",
        "eval.ts",
        "require.ts",
        "glob.ts",
    ] {
        assert!(!calls(&directory, refused, "run"), "{refused}");
    }
}

/// `vi` and `expect` change state and mock modules; neither registers a test. A free name the host
/// does not define is unknown unless the project's generated list names it.
#[test]
fn free_names_are_known_builtin_listed_or_refused() {
    let directory = project(&[
        (
            "framework.ts",
            "import { vi, expect } from 'vitest'\nexport const run = () => { vi.spyOn(console, 'info'); expect.extend({}); return vi.fn() }",
        ),
        ("builtin.ts", "export const run = () => JSON.stringify(Object.keys({ a: 1 }))"),
        ("unknown.ts", "export const run = () => useMystery()"),
        ("listed.ts", "export const run = () => useListed()"),
        ("aliased.ts", "export const run = () => useAlias()"),
        ("library.ts", "export const run = () => useLibrary()"),
        ("registering.ts", "export const run = () => useRegistering()"),
        ("composables/listed.ts", "export function useListed() { return 1 }"),
        ("composables/alias.ts", "export default function alias() { return 1 }"),
        (
            "composables/registering.ts",
            "import { test } from 'vitest'\nexport function useRegistering() { test('a', () => {}) }",
        ),
        (
            ".nuxt/imports.d.ts",
            "export { useListed } from '../composables/listed';\n\
             export { default as useAlias } from '../composables/alias';\n\
             export { useRegistering } from '../composables/registering';\n\
             export { useLibrary } from 'some-library';\n",
        ),
    ]);
    assert!(calls(&directory, "framework.ts", "run"));
    assert!(calls(&directory, "builtin.ts", "run"));
    assert!(!calls(&directory, "unknown.ts", "run"));
    assert!(calls(&directory, "listed.ts", "run"));
    assert!(calls(&directory, "aliased.ts", "run"));
    // A listed name stands for what the list says: a first-party export that registers is refused,
    // and a library that cannot be found is unknown.
    assert!(!calls(&directory, "registering.ts", "run"));
    assert!(!calls(&directory, "library.ts", "run"));
}

/// Names that reach code by name are refused even when a project lists them.
#[test]
fn reflective_names_are_refused_even_when_listed() {
    let directory = project(&[
        ("run.ts", "export const run = () => globalThis"),
        ("window.ts", "export const run = () => window"),
        (
            "w.ts",
            "export const globalThis = {}\nexport const window = {}",
        ),
        (
            ".nuxt/imports.d.ts",
            "export { globalThis, window } from '../w';\n",
        ),
    ]);
    assert!(!calls(&directory, "run.ts", "run"));
    assert!(!calls(&directory, "window.ts", "run"));
}

/// A name the module does not export is judged as the whole module: the code it does have must not
/// register either.
#[test]
fn an_export_that_cannot_be_found_is_judged_as_the_whole_module() {
    let directory = project(&[
        (
            "registers.ts",
            "import { test } from 'vitest'\nexport function other() { test('a', () => {}) }",
        ),
        ("pure.ts", "export function other() { return 1 }"),
        (
            "destructured.ts",
            "import { test } from 'vitest'\nconst { a } = { a: () => test('a', () => {}) }\nexport { a }",
        ),
    ]);
    assert!(!calls(&directory, "registers.ts", "missing"));
    assert!(calls(&directory, "pure.ts", "missing"));
    assert!(!calls(&directory, "destructured.ts", "a"));
}

/// Without a generated list nothing is listed, so a name the project would auto-import is unknown.
#[test]
fn an_unprepared_project_resolves_no_auto_imports() {
    let directory = project(&[
        ("run.ts", "export const run = () => useListed()"),
        (
            "composables/listed.ts",
            "export function useListed() { return 1 }",
        ),
    ]);
    assert!(!calls(&directory, "run.ts", "run"));
}

/// Packages are judged by their manifests; the entry text catches an undeclared Vitest import.
#[test]
fn an_installed_package_is_judged_by_its_manifest_and_entry() {
    let directory = project(&[
        (
            "node_modules/clean/package.json",
            r#"{"name":"clean","main":"index.js"}"#,
        ),
        ("node_modules/clean/index.js", "exports.go = () => 1"),
        (
            "node_modules/runner/package.json",
            r#"{"name":"runner","main":"index.js","peerDependencies":{"vitest":"*"}}"#,
        ),
        ("node_modules/runner/index.js", "exports.go = () => 1"),
        (
            "node_modules/undeclared/package.json",
            r#"{"name":"undeclared","main":"index.js"}"#,
        ),
        (
            "node_modules/undeclared/index.js",
            "const { test } = require('vitest'); exports.go = () => test('a', () => {})",
        ),
        (
            "uses-clean.ts",
            "import { go } from 'clean'\nexport const run = () => go()",
        ),
        (
            "uses-runner.ts",
            "import { go } from 'runner'\nexport const run = () => go()",
        ),
        (
            "uses-undeclared.ts",
            "import { go } from 'undeclared'\nexport const run = () => go()",
        ),
        (
            "side-effect.ts",
            "import 'clean'\nexport const run = () => 1",
        ),
    ]);
    assert!(calls(&directory, "uses-clean.ts", "run"));
    assert!(!calls(&directory, "uses-runner.ts", "run"));
    assert!(!calls(&directory, "uses-undeclared.ts", "run"));
    assert!(imports(&directory, "side-effect.ts"));
}

/// Components run `<script setup>` at mount; a plain script runs when imported. Assets and Nuxt
/// virtual modules carry no code that could register anything.
#[test]
fn components_assets_and_virtual_modules() {
    let directory = project(&[
        ("ref.ts", "export const ref = (value: number) => value"),
        (
            "setup.vue",
            "<template><div/></template>\n<script setup lang=\"ts\">\nconst count = ref(0)\nconsole.info(count)\n</script>\n",
        ),
        (
            "plain.vue",
            "<template><div/></template>\n<script lang=\"ts\">\nconsole.info('on import')\nexport default {}\n</script>\n",
        ),
        (
            "registering.vue",
            "<script setup lang=\"ts\">\nimport { test } from 'vitest'\ntest('a', () => {})\n</script>\n",
        ),
        ("external.vue", "<script src=\"./x.ts\"></script>\n"),
        ("style.css", ".a { color: red }"),
        (
            "uses-assets.ts",
            "import './style.css'\nimport data from './data.json'\nimport { useRoute } from '#app'\nimport fs from 'node:fs'\nexport const run = () => [data, useRoute, fs]",
        ),
        ("data.json", "{}"),
        (
            "imports-component.ts",
            "import Setup from './setup.vue'\nexport const run = () => Setup",
        ),
        (
            "imports-registering.ts",
            "import Registering from './registering.vue'\nexport const run = () => Registering",
        ),
    ]);
    // `ref` is an unlisted free name here, so the setup component needs a list to pass.
    assert!(!imports(&directory, "setup.vue"));
    std::fs::create_dir_all(directory.path().join(".nuxt")).unwrap();
    std::fs::write(
        directory.path().join(".nuxt/imports.d.ts"),
        "export { ref } from '../ref';\n",
    )
    .unwrap();
    super::registration_free::forget();
    assert!(imports(&directory, "setup.vue"));
    assert!(calls(&directory, "imports-component.ts", "run"));
    assert!(!imports(&directory, "plain.vue"));
    assert!(!imports(&directory, "external.vue"));
    assert!(!imports(&directory, "registering.vue"));
    assert!(calls(&directory, "uses-assets.ts", "run"));
    assert!(!calls(&directory, "imports-registering.ts", "run"));
}

/// Framework runtime exports are known by the specifier as written and by name.
#[test]
fn known_runtime_exports_are_matched_by_specifier_and_name() {
    for name in [
        "mountSuspended",
        "renderSuspended",
        "registerEndpoint",
        "mockNuxtImport",
        "mockComponent",
    ] {
        assert!(
            is_known_runtime_export("@nuxt/test-utils/runtime", name),
            "{name}"
        );
    }
    assert!(!is_known_runtime_export(
        "@nuxt/test-utils/runtime",
        "setup"
    ));
    assert!(!is_known_runtime_export(
        "@nuxt/test-utils/e2e",
        "mountSuspended"
    ));
    assert!(is_known_runtime_module("@nuxt/test-utils/runtime"));
    assert!(!is_known_runtime_module("@nuxt/test-utils/e2e"));
}

/// A proof is bounded: a chain of modules past the cap is refused rather than followed.
#[test]
fn a_chain_of_modules_past_the_cap_is_refused() {
    let mut files: Vec<(String, String)> = Vec::new();
    for index in 0..80 {
        files.push((
            format!("m{index}.ts"),
            format!(
                "import {{ next }} from './m{}'\nexport const run = () => next()",
                index + 1
            ),
        ));
    }
    files.push((
        "m80.ts".to_owned(),
        "export const next = () => 1".to_owned(),
    ));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let directory = project(&refs);
    assert!(!calls(&directory, "m0.ts", "run"));
    assert!(calls(&directory, "m60.ts", "run"));
}

/// Parses a call `helper(<arguments>)` to `function helper(<parameters>) {}`, written after
/// `prelude` in a test file, and answers `judge` about it.
fn judge_helper_call(
    prelude: &str,
    parameters: &str,
    arguments: &str,
    judge: impl FnOnce(
        &oxc_ast::ast::Program<'_>,
        &oxc_semantic::Semantic<'_>,
        &oxc_ast::ast::FormalParameters<'_>,
        &[oxc_ast::ast::Argument<'_>],
        oxc_span::Span,
    ) -> bool,
) -> bool {
    use oxc_ast::ast::{Expression, Statement};
    let source = format!(
        "import {{ beforeEach }} from 'vitest';\n{prelude}\nfunction helper({parameters}) {{}}\nhelper({arguments});\n"
    );
    let allocator = oxc_allocator::Allocator::default();
    let parsed = oxc_parser::Parser::new(&allocator, &source, oxc_span::SourceType::ts()).parse();
    assert!(
        parsed.diagnostics.is_empty(),
        "{source}: {:?}",
        parsed.diagnostics
    );
    let semantic = oxc_semantic::SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&parsed.program)
        .semantic;
    let program = &parsed.program;
    let parameters = program
        .body
        .iter()
        .find_map(|statement| match statement {
            Statement::FunctionDeclaration(function)
                if function.id.as_ref().is_some_and(|id| id.name == "helper") =>
            {
                Some(&function.params)
            }
            _ => None,
        })
        .expect("helper declared");
    let Some(Statement::ExpressionStatement(statement)) = program.body.last() else {
        panic!("call last");
    };
    let Expression::CallExpression(call) = &statement.expression else {
        panic!("call last");
    };
    judge(program, &semantic, parameters, &call.arguments, call.span)
}

/// Whether the call provably evaluates no registration before the helper's body runs.
fn inputs_register_nothing(prelude: &str, parameters: &str, arguments: &str) -> bool {
    judge_helper_call(
        prelude,
        parameters,
        arguments,
        |program, semantic, parameters, arguments, call| {
            let written = super::registration_free::written_bindings(program, semantic);
            super::registration_free::call_inputs_register_nothing(
                program,
                semantic,
                &written,
                std::path::Path::new("case.test.ts"),
                parameters,
                arguments,
                call,
            )
        },
    )
}

/// Whether binding the call's arguments to the helper's parameters is known to throw.
fn binding_fails(parameters: &str, arguments: &str) -> bool {
    binding_fails_after("", parameters, arguments)
}

/// [`binding_fails`] for a call written after `prelude`.
fn binding_fails_after(prelude: &str, parameters: &str, arguments: &str) -> bool {
    judge_helper_call(
        prelude,
        parameters,
        arguments,
        |_, semantic, parameters, arguments, _| {
            super::registration_free::call_binding_fails(semantic, parameters, arguments)
        },
    )
}

/// Binding fails only for a value the source spells out and that cannot bind: `undefined` or
/// `null` for any pattern, a non-iterable literal for an array pattern, nested through literals
/// and the defaults that run. A value the source does not show is assumed to bind.
#[test]
fn a_binding_fails_only_for_a_value_that_cannot_bind() {
    for (parameters, arguments) in [
        ("{ _a }", ""),
        ("{ _a }", "undefined"),
        ("{ _a }", "void 0"),
        ("{ _a }", "(null as any)"),
        ("_x, [_a]", "0, {}"),
        ("[_a]", "1"),
        ("[_a]", "{ __proto__ }"),
        ("[_a]", "() => {}"),
        ("[_a]", "class {}"),
        ("{ _a } = null", ""),
        ("{ _a: { _b } }", "{}"),
        ("{ _a: { _b } }", "{ _a: null }"),
        ("{ _a: { _b } = null }", "{}"),
        ("[[_a]]", "[0]"),
        ("[, [_a]]", "[0]"),
        ("[_a, { _b }]", "[0, , ]"),
    ] {
        assert!(
            binding_fails(parameters, arguments),
            "({parameters}) <- ({arguments})"
        );
    }
    for (parameters, arguments) in [
        ("_a", ""),
        ("{ _a }", "{}"),
        ("{ _a }", "0"),
        ("{ _a }", "options"),
        ("{ _a } = {}", ""),
        ("{ _a } = {}", "undefined"),
        ("{ _a } = {}", "(undefined as any)"),
        ("[_a]", "{ __proto__: [1] }"),
        ("{ _a: { _b } }", "{ __proto__: { _a: {} } }"),
        ("[_a]", "'ab'"),
        ("[_a]", "`ab`"),
        ("[_a]", "[]"),
        ("[_a]", "options"),
        ("[_a]", "{ [Symbol.iterator]: f }"),
        ("[_a]", "{ ...iterable }"),
        ("{ [key]: { _b } }", "{}"),
        ("{ _a: { _b } }", "{ ...rest }"),
        ("{ _a: { _b } }", "{ get _a() { return {}; } }"),
        ("{ _a: { _b } }", "{ [name]: {} }"),
        ("{ _a: { _b } = {} }", "{}"),
        ("{ _a: { _b } }", "{ _a: 1 }"),
        ("[[_a]]", "[...rows]"),
        ("[_x, [_a]]", "[...rows, 0]"),
        ("_x, { _a }", "...rest"),
    ] {
        assert!(
            !binding_fails(parameters, arguments),
            "({parameters}) <- ({arguments})"
        );
    }
    // A local `undefined` binds like the value it holds; `void 0` is still `undefined`.
    assert!(!binding_fails_after(
        "const undefined = {};",
        "{ _a }",
        "undefined"
    ));
    assert!(binding_fails_after(
        "const undefined = {};",
        "{ _a }",
        "void 0"
    ));
}

/// Inputs that only read or create values pass without a scan; any input that runs code passes
/// only when the scan proves it registers nothing, whichever pattern position evaluates it.
#[test]
fn call_inputs_are_proved_by_what_they_evaluate() {
    let unproved = "function make() { return eval('0'); }\nconst made = make();\n\
        const input = { get value() { beforeEach(() => {}); return 0; } };";
    for (parameters, arguments) in [
        ("_a", "1n, /a/, `x`, null, true, 'x'"),
        (
            "_a",
            "[1, , made], { made, f() {}, get g() { return beforeEach; }, k: (made as any)! }",
        ),
        (
            "_a",
            "(made satisfies unknown), () => beforeEach(() => {}), function () {}",
        ),
        ("_a = made", ""),
        ("[_a = 1, ..._rest]", "[]"),
        ("{ _a, ..._rest }", "{}"),
        ("..._rest", "made"),
        // A default its argument never lets run, and destructuring that reads only plain data.
        ("_a = beforeEach(() => {})", "null"),
        ("{ _a } = {}", ""),
        (
            "{ value: _v } = { get value() { beforeEach(() => {}); return 0; } }",
            "{ value: 1 }",
        ),
        ("{ _a }", "{ _a: 1 }"),
        // A parameter that receives a registering function but is never called.
        ("_register, _h = 1", "() => beforeEach(() => {})"),
        // A nested default that only binds is not read.
        ("{ child: _c = input }", "{}"),
    ] {
        assert!(
            inputs_register_nothing(unproved, parameters, arguments),
            "({parameters}) <- ({arguments})"
        );
    }
    for (parameters, arguments) in [
        ("_a", "beforeEach(() => {})"),
        ("_a", "...[made]"),
        ("_a", "{ [String(beforeEach(() => {}))]: 0 }"),
        ("_a", "`${beforeEach(() => {})}`"),
        ("_a", "made.value"),
        ("_a = beforeEach(() => {})", ""),
        ("[_a = beforeEach(() => {})]", "[]"),
        ("[..._rest] = [beforeEach(() => {})]", ""),
        ("{ [String(beforeEach(() => {}))]: _a }", "{}"),
        ("{ ..._rest } = { a: beforeEach(() => {}) }", ""),
        ("...[_a = beforeEach(() => {})]", ""),
        // Destructuring reads the argument (a getter here), and an unproved value may be iterated
        // or read.
        ("{ value: _v }", "input"),
        ("[_a]", "made"),
        ("_a = beforeEach(() => {})", "made"),
        // A default that can run is destructured too, so a getter in it registers.
        (
            "{ value: _v } = { get value() { beforeEach(() => {}); return 0; } }",
            "",
        ),
        (
            "{ value: _v } = { get value() { beforeEach(() => {}); return 0; } }",
            "undefined",
        ),
        // A default calls another parameter, whose value only the caller shows.
        ("register, _h = register()", "() => beforeEach(() => {})"),
        // A nested default destructured further is read, and a computed key is coerced.
        ("{ child: { value: _v } = input }", "{}"),
        ("{ [input]: _v }", "{}"),
    ] {
        assert!(
            !inputs_register_nothing(unproved, parameters, arguments),
            "({parameters}) <- ({arguments})"
        );
    }
    // A local `undefined` is a value like any other, so destructuring it runs its getter.
    assert!(!inputs_register_nothing(
        "const undefined = { get value() { beforeEach(() => {}); return 0; } };",
        "{ value: _v }",
        "undefined",
    ));
}
