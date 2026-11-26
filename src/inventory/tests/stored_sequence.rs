//! Calls of a stored sequence result require an initialized stable binding.

use super::*;

/// Count the task wrappers while protecting the independently registered control.
fn wrappers(expression: &str) -> usize {
    wrappers_with_globals(expression, false)
}

/// Count task wrappers using the selected framework globals configuration.
fn wrappers_with_globals(expression: &str, globals: bool) -> usize {
    let source = format!(
        "import {{test as api, test as control, vi}} from 'vitest'; import * as v from 'vitest'; {expression}; control('retained', () => {{}});"
    );
    let (found, parsed) = inventory_with_tree(&source, &[], globals);
    assert_eq!(parsed.parse_errors, 0, "{source}");
    assert!(
        found.sites.iter().any(|site| {
            site.title.as_deref() == Some("retained")
                && site.kind == SiteKind::Registers(Registers::Test)
        }),
        "{source}: {found:?}"
    );
    found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Wrapper(Reach::Tasks))
        .count()
}

/// Prove the final operand at the direct invocation of its one stable binding.
#[test]
fn stable_sequence_builder_binding_accounts_for_its_call() {
    for builder in ["api.extend({})", "v.test.extend({})", "api['extend']({})"] {
        for prefix in ["0", "null", "control('prefix', () => {})"] {
            for value in [
                builder.to_owned(),
                format!("(0, {builder})"),
                format!("({builder}) as any"),
            ] {
                let expression =
                    format!("const held = ({prefix}, {value}); held('selected', () => {{}})");
                assert_eq!(wrappers(&expression), 1, "{expression}");
            }
        }
    }
    assert_eq!(
        wrappers("switch (key) { case 0: const held = (0, api.extend({})); held('same case', () => {}) }"),
        1
    );
}

/// Unproved initializers and reads do not acquire the selected registration capability.
#[test]
fn sequence_binding_requires_a_proved_local_read() {
    for expression in [
        "held('before', () => {}); const held = (0, api.extend({}))",
        "function collect() { held('hoisted', () => {}) } collect(); const held = (0, api.extend({}))",
        "switch (key) { case 0: const held = (0, api.extend({})); break; case 1: held('other case', () => {}) }",
        "let held = (0, api.extend({})); held = () => {}; held('replaced', () => {})",
        "const held = (0, api.extend({})); { const held = () => {}; held('shadowed', () => {}) }",
        "const held = (api.extend({}), () => {}); held('scalar', () => {})",
        "const foreign = { extend: () => () => {} }; const held = (0, foreign.extend()); held('foreign', () => {})",
        "declare const held: (name: string, fn: () => void) => void; held('ambient', () => {})",
        "const held = (0, api.extend({})); held?.('optional call', () => {})",
        "const held = (0, api.extend({})); eval(''); held('eval in scope', () => {})",
        "export const held = (0, api.extend({})); held('exported declaration', () => {})",
        "const [held] = (0, api.extend({})); held('destructured', () => {})",
        "const held = (0, api.extend({})); const forwarded = held; forwarded('forwarded', () => {})",
        "const held = (0, api.extend({})); held.unknown('member', () => {})",
        "const held = (0, api.extend({})); void held",
    ] {
        assert_eq!(wrappers(expression), 0, "{expression}");
    }
}

/// A sequence can run user code before the selected extension is read.
#[test]
fn sequence_prefix_must_preserve_the_fixture_extension() {
    for prefix in [
        "api.extend = () => () => {}",
        "api['extend'] = () => () => {}",
        "alias.extend = () => () => {}",
        "Object.defineProperty(api, 'extend', {value: () => () => {}})",
        "(() => { api.extend = () => () => {} })()",
        "control((api.extend = () => () => {}, 'prefix'), () => {})",
        "unknownEffect()",
    ] {
        for operands in [
            format!("{prefix}, api.extend({{}})"),
            format!("0, ({prefix}, api.extend({{}}))"),
        ] {
            let expression = format!(
                "const alias = api; const held = ({operands}); held('unproved', () => {{}})"
            );
            assert_eq!(wrappers(&expression), 0, "{expression}");
        }
    }
}

/// Property writes and untracked uses of the API withhold the stored-builder proof.
#[test]
fn stored_builders_require_tracked_registration_uses() {
    for before in [
        "api.extend = () => () => {}",
        "const alias = api; alias['extend'] = () => () => {}",
        "const alias = api; alias[key] = () => () => {}",
        "Object.defineProperty(api, 'extend', {value: () => () => {}})",
        "expose(api)",
        "delete api.extend",
        "function replace() { api.extend = () => () => {} } replace()",
        "control('prefix', () => { api.extend = () => () => {} })",
        "const [copy] = [api]",
        "export { api }",
    ] {
        let expression =
            format!("{before}; const held = (0, api.extend({{}})); held('unproved', () => {{}})");
        assert_eq!(wrappers(&expression), 0, "{expression}");
    }
    for before in [
        "const unrelated = {extend: 1}; unrelated.extend = 2",
        "{ const api = {extend: 1}; api.extend = 2 }",
        "const alias = api; alias('prefix', () => {})",
        "typeof api; void api; !api",
    ] {
        let expression =
            format!("{before}; const held = (0, api.extend({{}})); held('selected', () => {{}})");
        assert_eq!(wrappers(&expression), 1, "{expression}");
    }
}

/// Unknown receiver methods and in-source writes can replace a selected fixture builder.
#[test]
fn stored_builder_receivers_track_member_calls_and_in_source_uses() {
    for before in [
        "api.__defineGetter__('extend', () => () => () => {})",
        "api['__defineGetter__']('extend', () => () => () => {})",
        "v.test.__defineGetter__('extend', () => () => () => {})",
        "const alias = api; alias[key]('extend', () => () => () => {})",
        "import.meta.vitest.test.extend = () => () => {}",
        "import.meta['vitest'].test.extend = () => () => {}",
        "const alias = import.meta.vitest; alias.test.extend = () => () => {}",
        "const meta = import.meta; meta.vitest.test.extend = () => () => {}",
    ] {
        let expression =
            format!("{before}; const held = (0, api.extend({{}})); held('unproved', () => {{}})");
        assert_eq!(wrappers(&expression), 0, "{expression}");
    }

    assert_eq!(
        wrappers(
            "if (import.meta.vitest) { const held = (0, import.meta.vitest.test.extend({})); held('selected', () => {}) }"
        ),
        1
    );
    assert_eq!(
        wrappers(
            "api.skip('tracked', () => {}); v.test('independent', () => {}); const held = (0, api.extend({})); held('selected', () => {})"
        ),
        1
    );
}

/// A loader or the global object may expose an untracked reference to the same receiver.
#[test]
fn stored_builders_require_closed_framework_receiver_access() {
    for before in [
        "const { test: other } = await import('vitest'); other.extend = () => () => {}",
        "const { other } = await import('./helper'); other.extend = () => () => {}",
        "const other = (await import(path)).test; other.extend = () => () => {}",
        "const { test: other } = await vi.importActual('vitest'); other.extend = () => () => {}",
        "const { other } = await vi.importActual('./helper'); other.extend = () => () => {}",
        "const load = v.vi.importActual; const { test: other } = await load(path); other.extend = () => () => {}",
        "const tools = vi; const { test: other } = await tools[loader]('vitest'); other.extend = () => () => {}",
    ] {
        let expression =
            format!("{before}; const held = (0, api.extend({{}})); held('unproved', () => {{}})");
        assert_eq!(wrappers(&expression), 0, "{expression}");
    }
    for (before, expected) in [
        (
            "globalThis.test.__defineGetter__('extend', () => () => () => {})",
            0,
        ),
        (
            "const root = globalThis; root.test.extend = () => () => {}",
            0,
        ),
        ("globalThis.globalThis.test.extend = () => () => {}", 0),
        (
            "globalThis['global'].globalThis.test.extend = () => () => {}",
            0,
        ),
        (
            "const root = globalThis['globalThis']; root.test.extend = () => () => {}",
            0,
        ),
        ("window.window.test.extend = () => () => {}", 0),
        ("self['self'].test.extend = () => () => {}", 0),
        (
            "globalThis.__defineGetter__('test', () => ({ extend: () => () => {} }))",
            0,
        ),
        (
            "globalThis['__defineGetter__']('test', () => ({ extend: () => () => {} }))",
            0,
        ),
        ("globalThis.Math.max(0, 1)", 0),
        ("Math.max(0, 1)", 1),
        (
            "const globalThis = { test: {} }; globalThis.test.extend = () => () => {}",
            1,
        ),
    ] {
        let expression =
            format!("{before}; const held = (0, api.extend({{}})); held('global', () => {{}})");
        assert_eq!(
            wrappers_with_globals(&expression, true),
            expected,
            "{expression}"
        );
    }
}

/// Recognized assertions use the namespace without exposing a registration receiver.
#[test]
fn namespace_assertions_keep_stored_builders_visible() {
    for assertion in [
        "v.expect(1).toBe(1)",
        "v.expect.soft(1).toBe(1)",
        "v.expectTypeOf(1)",
    ] {
        let expression = format!(
            "await import('vitest'); const held = (0, v.test.extend({{}})); held('assertion', () => {assertion})"
        );
        assert_eq!(wrappers(&expression), 1, "{expression}");
    }
    for call in ["v.test.expect(1)", "v.unknown(1)"] {
        let expression =
            format!("const held = (0, v.test.extend({{}})); held('uncertain', () => {call})");
        assert_eq!(wrappers(&expression), 0, "{expression}");
    }
}

/// A discarded load of the real framework preserves the local receiver proof.
#[test]
fn unused_loader_results_keep_stored_builders_visible() {
    for before in [
        "await import('vitest')",
        "import('vitest')",
        "await vi.importActual('vitest')",
    ] {
        let expression =
            format!("{before}; const held = (0, api.extend({{}})); held('discarded', () => {{}})");
        assert_eq!(wrappers(&expression), 1, "{expression}");
    }
}

/// Both named and namespace use of a project import leave module effects uncertain.
#[test]
fn project_imports_withhold_stored_builders() {
    for (import, effect) in [
        ("* as helper", "helper.other.extend = () => () => {}"),
        (
            "* as helper",
            "const copy = helper; copy.other.extend = () => () => {}",
        ),
        ("{ other }", "other.extend = () => () => {}"),
        ("* as helper", "helper.identity(1)"),
    ] {
        let source = format!(
            "import {{test as api}} from 'vitest'; import {import} from './helper'; {effect}; const held = (0, api.extend({{}})); held('selected', () => {{}}); api('direct', () => {{}})"
        );
        let (found, parsed) = inventory_with_tree(
            &source,
            &[(
                "helper.ts",
                "export {test as other} from 'vitest'; export const identity = (x) => x;",
            )],
            false,
        );
        assert_eq!(parsed.parse_errors, 0, "{source}");
        assert!(found
            .sites
            .iter()
            .any(|site| site.title.as_deref() == Some("direct")));
        assert_eq!(
            found
                .sites
                .iter()
                .filter(|site| site.kind == SiteKind::Wrapper(Reach::Tasks))
                .count(),
            0,
            "{source}",
        );
    }
}

/// Runtime module links can change a builder even if their results are discarded.
#[test]
fn stored_builders_require_known_module_links() {
    for (before, expected) in [
        ("import './setup'", 0),
        ("import {type Value} from './setup'", 0),
        ("export {value} from './setup'", 0),
        ("export * from './setup'", 0),
        ("import setup = require('./setup')", 0),
        ("await import('./setup')", 0),
        ("import(path)", 0),
        ("await vi.importActual('./setup')", 0),
        ("await vi.importMock('vitest')", 0),
        ("require('./setup')", 0),
        ("const load = require; load('./setup')", 0),
        ("import type {Value} from './setup'", 1),
        ("export type {Value} from './setup'", 1),
        ("export type * from './setup'", 1),
        ("{ const require = () => {}; require('./setup'); }", 1),
    ] {
        let expression =
            format!("{before}; const held = (0, api.extend({{}})); held('selected', () => {{}})");
        assert_eq!(wrappers(&expression), expected, "{expression}");
    }
}

/// A mocking namespace handed to untracked code may expose the framework loader.
#[test]
fn mocking_namespace_reads_require_tracked_uses() {
    for (before, expected) in [
        ("const {importActual} = vi; const {test: other} = await importActual('vitest'); other.extend = () => () => {}", 0),
        ("const {importActual: load} = {...vi}; const other = (await load('vitest')).test; other.extend = () => () => {}", 0),
        ("const [tools] = [vi]; const other = (await tools.importActual('vitest')).test; other.extend = () => () => {}", 0),
        ("const tools = vi; unknown(tools)", 0),
        ("let tools = vi; tools.fn()", 0),
        ("const tools = vi.valueOf(); tools.stubGlobal('test', {})", 0),
        ("const alias = vi; const tools = alias['valueOf'](); tools.stubGlobal('test', {})", 0),
        ("const tools = vi; const second = tools; second.fn()", 0),
        ("(vi as any).fn()", 0),
        ("const tools = vi; const second = tools; await second.importActual('vitest')", 1),
        ("(vi as any)['importActual']('vitest')", 1),
        ("typeof vi", 1),
        ("{ const vi = { importActual: 0 }; const { importActual } = vi; void importActual; }", 1),
    ] {
        let expression =
            format!("{before}; const held = (0, api.extend({{}})); held('selected', () => {{}})");
        assert_eq!(wrappers(&expression), expected, "{expression}");
    }
}

/// A global stub can replace the receiver before the stored builder reads it.
#[test]
fn global_stubs_can_replace_stored_builder_receivers() {
    for (before, expected) in [
        ("vi.stubGlobal('test', { extend: () => () => {} })", 0),
        ("const tools = vi; tools['stubGlobal'](key, {})", 0),
        ("const stub = vi.stubGlobal; stub('test', {})", 0),
        (
            "const tools = vi.stubGlobal('unrelated', true); tools.stubGlobal('test', {})",
            0,
        ),
        ("vi.stubGlobal('unrelated', true).stubGlobal('test', {})", 0),
        ("vi.stubGlobal('unrelated', true)", 1),
    ] {
        let expression =
            format!("{before}; const held = (0, test.extend({{}})); held('global', () => {{}})");
        assert_eq!(
            wrappers_with_globals(&expression, true),
            expected,
            "{expression}"
        );
    }
}
