//! Runner resolution and reachable templates over owned first-party modules.

use super::*;
use crate::framework::vitest::DIALECT;

/// Locate one top-level initializer in an owned proof input.
fn proof_initializer(parsed: &Parsed, name: &str) -> NodeId {
    let symbol = parsed
        .scopes
        .symbols()
        .find(|&symbol| parsed.scopes.symbol(symbol).name == name)
        .unwrap();
    let Declaration::Variable(declaration) = parsed.scopes.symbol(symbol).declaration else {
        panic!("expected a variable")
    };
    parsed.tree.child(declaration, "init").unwrap()
}

/// Binding-fact reuse preserves each existing inert-expression admission barrier.
#[test]
fn inert_binding_shapes_keep_their_existing_result() {
    for (source, expected) in [
        ("const source=1;const value=source", true),
        (
            "const source=1;function f(){let source=2;source=3}const value=source",
            true,
        ),
        (
            "function source(){}function source(){}const value=source",
            false,
        ),
        (
            "function source(x:number):number;function source(x){return x}const value=source",
            true,
        ),
        ("let source=1;const value=source", false),
        ("const source=1;source=2;const value=source", false),
        ("const value=source;const source=1", false),
        ("const source=unknown;const value=source", false),
        ("const source=1;const value=[source,source]", true),
        ("const source=1;const value={x:source}", true),
        ("const source={};const value=source.x", false),
        ("const source=1;const value={[source]:1}", false),
        ("const source=1;const value={__proto__:source}", false),
        (
            "import type {source} from './dep';const value=source",
            false,
        ),
        ("declare function source():void;const value=source", false),
    ] {
        let parsed = lang::parse(Path::new("proof.ts"), source).unwrap();
        assert_eq!(parsed.parse_errors, 0);
        assert_eq!(
            inert_value(&parsed, proof_initializer(&parsed, "value"), &mut 4096),
            expected,
            "{source}"
        );
    }
}

/// Repeated references still consume the full expression allowance after facts become cached.
#[test]
fn inert_binding_proofs_retain_the_expression_limit() {
    for (references, expected, remaining) in [(2047, true, 1), (2048, false, 0)] {
        let items = vec!["source"; references].join(",");
        let source = format!("const source=1;const value=[{items}]");
        let parsed = lang::parse(Path::new("proof.ts"), &source).unwrap();
        assert_eq!(parsed.parse_errors, 0);
        let mut left = 4096;
        assert_eq!(
            inert_value(&parsed, proof_initializer(&parsed, "value"), &mut left),
            expected
        );
        assert_eq!(left, remaining);
    }
}

/// The permanent alias input reuses source scans without reducing logical expression work.
#[test]
fn inert_binding_facts_reuse_bounded_source_work() {
    let source = include_str!("../../../fixtures/oracle/runner-binding-facts/runner.ts");
    let parsed = lang::parse(Path::new("runner.ts"), source).unwrap();
    assert_eq!(parsed.parse_errors, 0);
    let node = proof_initializer(&parsed, "a127");
    let mut proof = InertProof::new(&parsed, usize::MAX);
    let mut left = 4096;
    assert!(proof.value(node, &mut left));
    let steps = 4096 - left;
    assert_eq!(steps, 128);
    let work = usize::MAX - proof.left;
    let before = proof.left;
    left = 4096;
    assert!(proof.value(node, &mut left));
    assert_eq!(4096 - left, steps);
    assert!(before - proof.left <= steps * 2);
    assert!(work < parsed.tree.nodes().count() * 4);
    let symbols = parsed.scopes.symbols().count();
    assert!(proof.declarations.as_ref().unwrap().len() <= symbols);
    assert!(proof.symbols.len() <= symbols && proof.unwritten.len() <= symbols);
    let mut short = InertProof::new(&parsed, work - 1);
    assert!(!short.value(node, &mut 4096));
    assert!(short.limited);
}

/// A repeated recorded-write query consumes one lookup rather than rescanning references.
#[test]
fn inert_binding_write_facts_are_reused() {
    let source = include_str!("../../../fixtures/oracle/runner-binding-facts/runner.ts");
    let parsed = lang::parse(Path::new("runner.ts"), source).unwrap();
    let symbol = parsed
        .scopes
        .symbols()
        .find(|&s| parsed.scopes.symbol(s).name == "a0")
        .unwrap();
    let mut proof = InertProof::new(&parsed, usize::MAX);
    assert!(proof.unwritten(symbol));
    let before = proof.left;
    assert!(proof.unwritten(symbol));
    assert_eq!(before - proof.left, 1);
}

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
            template
                .parsed
                .scopes
                .symbol(template.parameters[0].bindings[0].symbol)
                .name,
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

/// Returned factories expose only one inert object reached after inert setup.
#[test]
fn returned_object_templates_refuse_effects_and_uncertain_outcomes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("factory.ts");
    let setting = Setting {
        dialect: &DIALECT,
        globals: false,
        root: root.path(),
    };
    for source in [
        "export function make(mode){const cases={run:()=>mode};return cases}",
        "export function make({enabled=true}){return {run:()=>enabled}}",
        "export function make(mode){const prefix=mode;return {run:()=>prefix}}",
    ] {
        std::fs::write(&path, source).unwrap();
        let template =
            factory_template(&Sources::default(), &path, "make", setting, 8 * 1024 * 1024)
                .unwrap_or_else(|error| panic!("{source}: {error:?}"));
        assert_eq!(template.parameters.len(), 1, "{source}");
        assert_eq!(
            template.parsed.tree.kind(template.object),
            "ObjectExpression"
        );
    }
    for source in [
        "export function make(){effect();return {run(){}}}",
        "export function make(flag){if(flag)return {run(){}};return {run(){}}}",
        "export function make(){const cases={run(){}};read(cases);return cases}",
        "export function make(){return {...other}}",
        "export function make(){return {get run(){return ()=>{}}}}",
        "export function make(){return {run(){},run:()=>{}}}",
        "export function make(){return {run(){}}}",
        "export async function make(){return {run(){}}}",
    ] {
        std::fs::write(&path, source).unwrap();
        assert!(
            factory_template(&Sources::default(), &path, "make", setting, 8 * 1024 * 1024).is_err(),
            "{source}"
        );
    }
}

/// The permanent large pattern stops before owning every amplified path, even before final checks.
#[test]
fn returned_parameter_paths_stop_before_allocation_limit() {
    let source = include_str!("../../../fixtures/oracle/factory-parameter-work/factory.ts");
    let parsed = lang::parse(std::path::Path::new("factory.ts"), source).unwrap();
    let tree = &parsed.tree;
    let function = tree
        .nodes()
        .find(|&node| {
            tree.kind(node) == "FunctionDeclaration"
                && tree
                    .child(node, "id")
                    .is_some_and(|id| tree.text(id, "name") == Some("makeLarge"))
        })
        .unwrap();
    let pattern = tree.list(function, "params")[0].unwrap();
    let mut bindings = Vec::new();
    let mut left = MAX_PARAMETER_WORK;
    let result = parameter_bindings(
        &parsed,
        pattern,
        &mut Vec::new(),
        &mut bindings,
        &binding_declarations(&parsed),
        tree.nodes().count(),
        &mut left,
    );
    assert!(matches!(result, Err(Refusal::Limit)));
    assert!(!bindings.is_empty() && bindings.len() < 600);
}

/// Flattened destructuring bindings participate in the same bounded parameter work estimate.
#[test]
fn returned_parameter_binding_work_is_bounded() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("factory.ts");
    let properties = (0..4000)
        .map(|index| format!("p{index}=0"))
        .collect::<Vec<_>>()
        .join(",");
    let source = format!("export function make({{{properties}}}){{return {{run:()=>0}}}}");
    std::fs::write(&path, source).unwrap();
    assert!(matches!(
        factory_template(
            &Sources::default(),
            &path,
            "make",
            Setting {
                dialect: &DIALECT,
                globals: false,
                root: root.path(),
            },
            8 * 1024 * 1024,
        ),
        Err(Refusal::Limit)
    ));
}

/// A linear alias chain reuses proofs and refuses before exceeding the remaining work.
#[test]
fn returned_factory_setup_proofs_share_actual_work() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("factory.ts");
    std::fs::write(
        &path,
        include_str!("../../../fixtures/oracle/factory-resource-work/aliases-short.ts"),
    )
    .unwrap();
    let setting = Setting {
        dialect: &DIALECT,
        globals: false,
        root: root.path(),
    };
    let template =
        factory_template(&Sources::default(), &path, "make", setting, 8 * 1024 * 1024).unwrap();
    assert!(template.work > 400);
    assert!(template.work <= template.parsed.tree.nodes().count() * 6);
    assert!(matches!(
        factory_template(
            &Sources::default(),
            &path,
            "make",
            setting,
            template.work - 1
        ),
        Err(Refusal::Limit)
    ));
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
