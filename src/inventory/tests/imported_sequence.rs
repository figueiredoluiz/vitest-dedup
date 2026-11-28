//! Named transport keeps the selected sequence builder's task capability.

use super::*;

/// Count unresolved task wrappers and keep the independently registered control visible.
fn task_wrappers(importer: &str, producer: &str) -> usize {
    let source = format!(
        "{importer} import {{test as control}} from 'vitest'; control('retained', () => {{}});"
    );
    sites_with_retained_control(
        &source,
        &[
            ("builder.ts", producer),
            ("barrel.ts", "export {run as forwarded} from './builder';"),
            (
                "forward.ts",
                "import {run as local} from './builder'; export {local as forwarded};",
            ),
            ("star.ts", "export * from './builder';"),
            ("fallback.ts", "export {run as default} from './builder';"),
            (
                "setup.ts",
                "import {test as api} from 'vitest'; api.extend = () => () => {};",
            ),
            (
                "mixed.ts",
                "import './setup'; export {run} from './builder';",
            ),
            ("cycle.ts", "export {run} from './builder';"),
        ],
        false,
    )
    .iter()
    .filter(|site| site.kind == SiteKind::Wrapper(Reach::Tasks))
    .count()
}

/// Named imports and re-exports of the same proved stable value get the same unresolved site.
#[test]
fn named_exports_carry_a_sequence_selected_fixture_builder() {
    for value in [
        "(0, api.extend({}))",
        "(null, (0, api.extend({})))",
        "(false, api['extend']({}) as unknown)",
    ] {
        for declaration in [
            format!("export const run = {value};"),
            format!("const local = {value}; export {{local as run}};"),
            format!("export {{local as run}}; const local = {value};"),
        ] {
            let producer = format!("import {{test as api}} from 'vitest'; {declaration}");
            for import in [
                "import {run as selected} from './builder';",
                "import {forwarded as selected} from './barrel';",
                "import {forwarded as selected} from './forward';",
            ] {
                let importer = format!("{import} selected('chosen', () => {{}});");
                assert_eq!(
                    task_wrappers(&importer, &producer),
                    1,
                    "{producer} {importer}"
                );
            }
        }
    }
}

/// A builder in a prefix, an erased name, or a replaced binding carries no proved capability.
#[test]
fn only_a_runtime_export_of_the_selected_stable_value_is_carried() {
    let importer = "import {run} from './builder'; run('unknown', () => {});";
    for producer in [
        "import {test as api} from 'vitest'; export const run = (api.extend({}), () => {});",
        "import {test as api} from 'vitest'; export const run = ((api.extend = () => () => {}), api.extend({}));",
        "import type {test as api} from 'vitest'; export const run = (0, api.extend({}));",
        "import {test as api} from 'vitest'; const run = (0, api.extend({})); export type {run};",
        "import {test as api} from 'vitest'; export let run = (0, api.extend({})); run = () => {};",
        "export declare const run: (name: string, callback: () => void) => void;",
    ] {
        assert_eq!(task_wrappers(importer, producer), 0, "{producer}");
    }
    assert_eq!(
        task_wrappers(
            "import {type run} from './builder'; run('erased', () => {});",
            "import {test as api} from 'vitest'; export const run = (0, api.extend({}));",
        ),
        0
    );
}

/// Exposing an export binding does not itself run it or move a prefix into the caller.
#[test]
fn named_sequence_export_keeps_its_module_load_separate() {
    let source = "import {forwarded} from './barrel'; forwarded('selected', () => {});";
    let (found, parsed) = inventory_with_tree(
        source,
        &[
            ("barrel.ts", "export {run as forwarded} from './builder';"),
            (
                "builder.ts",
                "import {test as api} from 'vitest'; export const run = (api('prefix', () => {}), api.extend({}));",
            ),
        ],
        false,
    );
    assert_eq!(parsed.parse_errors, 0);
    assert_eq!(found.loads.len(), 1, "{found:?}");
    assert_eq!(found.sites.len(), 1, "{found:?}");
    assert_eq!(found.sites[0].kind, SiteKind::Wrapper(Reach::Tasks));
    assert_eq!(found.sites[0].title.as_deref(), Some("selected"));
}

/// Only an eager call through the proved imported binding contributes a module load.
#[test]
fn forwarding_builder_load_requires_a_live_module_call() {
    let source = "import {forwarded} from './helper'; forwarded('retained', () => {});";
    let builder = "import {test} from 'vitest'; export const run = (0, test.extend({}));";
    for (before, after, loads) in [
        ("", "local('eager', () => {});", 1),
        ("", "(local)('eager', () => {});", 1),
        ("local('hoisted import', () => {});", "", 1),
        (
            "",
            "local('before throw', () => {}); throw 0; local('dead', () => {});",
            1,
        ),
        ("", "", 0),
        ("", "function later() { local('deferred', () => {}); }", 0),
        ("", "if (false) local('guarded', () => {});", 0),
        ("", "local?.('optional', () => {});", 0),
        (
            "",
            "{ const local = () => {}; local('shadowed', () => {}); }",
            0,
        ),
        ("", "throw 0; local('dead', () => {});", 0),
        ("", "const result = local('initializer', () => {});", 1),
        (
            "",
            "export const result = local('export initializer', () => {});",
            1,
        ),
        (
            "",
            "let empty, result = (local('typed initializer', () => {})) as unknown;",
            1,
        ),
        (
            "",
            "const result = false && local('guarded initializer', () => {});",
            0,
        ),
        (
            "",
            "const result = () => local('deferred initializer', () => {});",
            0,
        ),
        ("", "const result = local?.('optional initializer', () => {});", 0),
        (
            "",
            "if (false) { var result = local('non-module initializer', () => {}); }",
            0,
        ),
        (
            "",
            "{ const local = () => {}; const result = local('shadowed initializer', () => {}); }",
            0,
        ),
        (
            "",
            "throw 0; const result = local('dead initializer', () => {});",
            0,
        ),
        (
            "",
            "throw 0; export const result = local('dead export initializer', () => {});",
            0,
        ),
        (
            "",
            "const result = false && local('guarded', () => {}); local('later standalone', () => {});",
            1,
        ),
    ] {
        let helper = format!(
            "{before} import {{run as local}} from './builder'; {after} export {{local as forwarded}};"
        );
        let (found, parsed) = inventory_with_tree(
            source,
            &[("helper.ts", &helper), ("builder.ts", builder)],
            false,
        );
        assert_eq!(parsed.parse_errors, 0);
        assert_eq!(found.loads.len(), loads, "{helper}");
        assert_eq!(found.sites.len(), 1, "{helper}");
        assert_eq!(found.sites[0].kind, SiteKind::Wrapper(Reach::Tasks));
        assert_eq!(found.sites[0].title.as_deref(), Some("retained"));
    }
}

/// A default expression reads the binding even though a named export specifier need not.
#[test]
fn named_sequence_export_does_not_prove_a_default_expression_read() {
    assert_eq!(
        task_wrappers(
            "import selected from './builder'; selected('before initialization', () => {});",
            "import {test as api} from 'vitest'; export default run; const run = (0, api.extend({}));",
        ),
        0
    );
}

/// A second runtime dependency can change the producer's receiver before its value is captured.
#[test]
fn stored_builder_transport_requires_an_isolated_dependency_chain() {
    let producer = "import {test as api} from 'vitest'; export const run = (0, api.extend({}));";
    for import in [
        "import './setup'; import {run as call} from './builder';",
        "import {run as call} from './builder'; import './setup';",
        "import './setup'; import {forwarded as call} from './barrel';",
        "import {forwarded as call} from './barrel'; import './setup';",
        "import {run as call} from './mixed';",
    ] {
        let importer = format!("{import} call('unproved', () => {{}});");
        assert_eq!(task_wrappers(&importer, producer), 0, "{importer}");
    }
    assert_eq!(
        task_wrappers(
            "import type {Shape} from './setup'; import {run} from './builder'; run('chosen', () => {});",
            producer,
        ),
        1
    );
    assert_eq!(
        task_wrappers(
            "import {run} from './cycle'; run('unproved', () => {});",
            "import './cycle'; import {test as api} from 'vitest'; export const run = (0, api.extend({}));",
        ),
        0
    );
}

/// Other export forms and indirect, optional, or shadowed calls keep their prior accounting.
#[test]
fn stored_builder_transport_requires_a_direct_named_call() {
    let producer = "import {test as api} from 'vitest'; export const run = (0, api.extend({}));";
    for importer in [
        "import {run} from './builder'; const alias = run; alias('unknown', () => {});",
        "import {run} from './builder'; { const run = () => {}; run('shadow', () => {}); }",
        "import {run} from './builder'; run?.('optional', () => {});",
        "import * as ns from './builder'; ns.run('namespace', () => {});",
        "import run from './fallback'; run('default', () => {});",
        "import {run} from './star'; run('star re-export', () => {});",
    ] {
        assert_eq!(task_wrappers(importer, producer), 0, "{importer}");
    }
}
