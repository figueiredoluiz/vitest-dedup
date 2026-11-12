//! Runner resolution and reachable templates over owned first-party modules.

use super::*;
use crate::framework::vitest::DIALECT;

/// Read one temporary project's export with the ordinary source authority.
fn read(files: &[(&str, &str)], export: &str) -> Result<Template, Refusal> {
    let root = tempfile::tempdir().unwrap();
    for (name, source) in files {
        std::fs::write(root.path().join(name), source).unwrap();
    }
    template(
        &Sources::default(),
        &root.path().join(files[0].0),
        export,
        Setting {
            dialect: &DIALECT,
            globals: false,
            root: root.path(),
        },
    )
}

/// Every existing public export shape resolves to the callable itself, never an alias expression.
#[test]
fn immutable_export_shapes_retain_the_registration_plan() {
    for source in [
        "export function run(label) { test(label, () => expect(1).toBe(1)); }",
        "export const run = label => { test(label, () => expect(1).toBe(1)); };",
        "export const run = function(label) { test(label, () => expect(1).toBe(1)); };",
        "function local(label) { test(label, () => expect(1).toBe(1)); } export { local as run };",
        "export default function(label) { test(label, () => expect(1).toBe(1)); }",
        "export default label => test(label, () => expect(1).toBe(1));",
        "const local = label => test(label, () => expect(1).toBe(1)); export default local;",
        "export default local; function local(label) { test(label, () => expect(1).toBe(1)); }",
    ] {
        let source = format!("import {{test, expect}} from 'vitest'; {source}");
        let export = if source.contains("export default") {
            "default"
        } else {
            "run"
        };
        let template = read(&[("runner.ts", &source)], export).unwrap();
        assert_eq!(template.source, source);
        assert_eq!(template.parameters.len(), 1, "{source}");
        assert_eq!(
            template.parsed.scopes.symbol(template.parameters[0]).name,
            "label"
        );
        assert_eq!(template.steps.len(), 1, "{source}");
        assert!(matches!(template.steps[0], Step::Registration(_, _)));
        assert!(template.parsed.tree.is_function(template.function));
    }
}

/// Named re-export hops preserve each module needed for the proof, and cycles are refused.
#[test]
fn reexports_are_bounded_and_do_not_skip_hop_effects() {
    let source = "import {test,expect} from 'vitest'; export function declare(){test('a',()=>expect(1).toBe(1))}";
    let entry = "export {declare as run} from './body'";
    let template = read(&[("entry.ts", entry), ("body.ts", source)], "run").unwrap();
    assert_eq!(template.steps.len(), 1);
    assert_eq!(template.source, source);
    assert_eq!(template.dependencies.len(), 2);
    assert_eq!(template.resolution_depth, 2);
    assert_eq!(
        template
            .dependencies
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap())
            .collect::<Vec<_>>(),
        ["body.ts", "entry.ts"]
    );
    let mut bytes: Vec<_> = template
        .dependency_bytes
        .iter()
        .map(|(path, bytes)| (path.file_name().unwrap().to_str().unwrap(), *bytes))
        .collect();
    bytes.sort();
    assert_eq!(
        bytes,
        [("body.ts", source.len()), ("entry.ts", entry.len())]
    );
    assert!(template.path.ends_with("body.ts"));
    for entry in [
        "sideEffect(); export {declare as run} from './body'",
        "export {run} from './cycle'",
    ] {
        let result = read(
            &[
                ("entry.ts", entry),
                ("body.ts", source),
                ("cycle.ts", "export {run} from './entry'"),
            ],
            "run",
        );
        assert!(result.is_err(), "{entry}");
    }
}

/// Every imported runtime binding must link even when no invocation reads it.
#[test]
fn nested_callable_imports_keep_linkage_and_inert_closure_proofs() {
    let leaf = "import {test} from 'vitest';export default function leaf(){test('leaf',()=>{})} export {leaf as named}";
    for import in [
        "import child from './leaf';",
        "import {named as child} from './leaf';",
    ] {
        let source = format!("{import}export function run(){{return child()}}");
        let template = read(&[("runner.ts", &source), ("leaf.ts", leaf)], "run").unwrap();
        assert_eq!(template.steps.len(), 1);
        assert!(matches!(template.steps[0], Step::Call(_)));
        assert_eq!(template.dependencies.len(), 2);
        assert_eq!(template.resolution_depth, 1);
        assert_eq!(template.dependency_bytes.len(), 2);
    }
    for import in [
        "import {missing} from './leaf';",
        "import * as unused from './leaf';",
    ] {
        let source = format!(
            "{import}import {{test}} from 'vitest';export function run(){{test('own',()=>{{}})}}"
        );
        assert!(read(&[("runner.ts", &source), ("leaf.ts", leaf)], "run").is_err());
    }
    let source = "import child from './leaf';export function run(){return child()}";
    assert!(read(
        &[
            ("runner.ts", source),
            ("leaf.ts", "effect(); export default function child(){}")
        ],
        "run"
    )
    .is_err());
}

/// Parameter validation itself is bounded before repeated whole-tree binding checks begin.
#[test]
fn parameter_validation_refuses_work_beyond_its_owned_bound() {
    let params = (0..6000)
        .map(|i| format!("p{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let source = format!(
        "import {{test}} from 'vitest';export function run({params}){{test('one',()=>{{}})}}"
    );
    assert!(matches!(
        read(&[("runner.ts", &source)], "run"),
        Err(Refusal::Limit)
    ));
    assert!(read(
        &[(
            "runner.ts",
            "import {test} from 'vitest';export function run(a,b){test('one',()=>{})}"
        )],
        "run"
    )
    .is_ok());
}

/// Reassignment, initialization order and binding feasibility are independent admission barriers.
#[test]
fn uncertain_callables_and_parameters_never_return_a_partial_plan() {
    for source in [
        "export function run(){return {leaf:()=>test('a',()=>{})}}",
        "export let run = () => test('a',()=>{});",
        "export function run(){test('a',()=>{})} run = replacement;",
        "export default run; const run = () => test('a',()=>{});",
        "export async function run(){test('a',()=>{})}",
        "export function* run(){test('a',()=>{})}",
        "export function run({value}){test('a',()=>{})}",
        "export function run([value]){test('a',()=>{})}",
        "export function run(value=1){test('a',()=>{})}",
        "export function run(...values){test('a',()=>{})}",
        "export function run(value){value=2; test('a',()=>{})}",
        "export function run(){test('a',()=>{}); if(flag) test('b',()=>{})}",
        "export function run(){test('a',()=>{}); unknown()}",
    ] {
        let source = format!("import {{test}} from 'vitest'; {source}");
        let export = if source.contains("export default") {
            "default"
        } else {
            "run"
        };
        assert!(read(&[("runner.ts", &source)], export).is_err(), "{source}");
    }
}

/// Returned registrations terminate the sequence, retaining a direct positive before the return.
#[test]
fn a_returned_registration_cannot_make_dead_tests_live() {
    let source = "import {test} from 'vitest'; export function run(){test('before',()=>{}); return test('returned',()=>{}); test('dead',()=>{})}";
    let template = read(&[("runner.ts", source)], "run").unwrap();
    assert_eq!(template.steps.len(), 2);
    let found: Vec<_> = template
        .steps
        .iter()
        .map(|step| match step {
            Step::Registration(node, _) => {
                let title = template.parsed.tree.list(*node, "arguments")[0].unwrap();
                template.parsed.tree.text(title, "value").unwrap()
            }
            _ => panic!("expected a direct registration"),
        })
        .collect();
    assert_eq!(found, ["before", "returned"]);
}

/// Import-time execution is checked even in a dependency the export does not otherwise read.
#[test]
fn importing_effectful_dependencies_refuses_the_whole_template() {
    let entry = "import './dependency'; import {test} from 'vitest'; export function run(){test('a',()=>{})}";
    for dependency in [
        "register();",
        "import {test} from 'vitest'; test('hidden',()=>{})",
        "const x = unknown.value;",
    ] {
        assert!(read(&[("entry.ts", entry), ("dependency.ts", dependency)], "run").is_err());
    }
    let template = read(
        &[
            ("entry.ts", entry),
            ("dependency.ts", "export const value = {a: [1, 'x']};"),
        ],
        "run",
    )
    .unwrap();
    assert_eq!(template.steps.len(), 1);
    assert_eq!(template.dependencies.len(), 2);
}

/// Utilities may affect import-time code even when their read is written inside a callback.
#[test]
fn framework_setup_capability_cannot_hide_inside_callbacks_or_hops() {
    for source in [
        "import {test, vi} from 'vitest'; export function run(){test('a',()=>{vi.mock('./x');})}",
        "import {test, vitest as tools} from 'vitest'; export function run(){test('a',()=>{tools.mock('./x');})}",
        "import {test} from 'vitest'; export function run(){test('a',()=>{vi.mock('./x');})}",
        "import * as api from 'vitest'; export function run(){api.test('a',()=>{})}",
    ] {
        assert!(read(&[("runner.ts",source)],"run").is_err(),"{source}");
    }
    let source="import {test,expect} from 'vitest'; export function run(){test('a',()=>{const vi={x:1};expect(vi.x).toBe(1)})}";
    let template = read(&[("runner.ts", source)], "run").unwrap();
    assert_eq!(template.steps.len(), 1);
    let entry = "export {run} from './body'; export {vi as tools} from 'vitest'";
    assert!(read(&[("entry.ts", entry), ("body.ts", source)], "run").is_err());
}
