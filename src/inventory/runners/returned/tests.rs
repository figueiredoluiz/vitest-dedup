//! Returned-object construction is independent from registration admission.
use super::*;
use crate::framework::vitest::DIALECT;

/// Resolve an owned factory through the same source authority as ordinary runner templates.
fn read(source: &str, budget: usize) -> Result<ReturnedTemplate, Refusal> {
    read_export(source, "make", budget)
}

/// Keep the runtime-oracle module byte-identical while selecting either exported control.
fn read_export(source: &str, export: &str, budget: usize) -> Result<ReturnedTemplate, Refusal> {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("factory.ts");
    std::fs::write(&path, source).unwrap();
    returned_template(
        &Sources::default(),
        &path,
        export,
        Setting {
            dialect: &DIALECT,
            globals: false,
            root: root.path(),
        },
        budget,
    )
}

/// Both equal and different closure bodies construct inertly; neither is a registration proof.
#[test]
fn returned_objects_preserve_owned_bindings_without_admitting_callbacks() {
    for (source, setup) in [
        ("export function make(mode){return {run:()=>mode}}", 0),
        ("export function make(mode){const cases={run:()=>mode};return cases}", 1),
        ("export function make(mode){const prefix=mode;return {run:()=>prefix}}", 1),
        ("export function make(mode){const run=()=>mode;return {run}}", 1),
        ("export function make(mode){return {data:[{run:()=>mode}]}}", 0),
        ("export function make(mode){const data=[{run:()=>mode}];return {data}}", 1),
        ("export function make(mode){function run(){effect(mode)}return {run}}", 1),
        ("export function make(mode){return {a:()=>mode,b:()=>mode}}", 0),
        ("export function make(mode){return {a:()=>mode,b:()=>other}}", 0),
        ("export function make(mode){;type T=number;interface I{};return {nested:{run:()=>mode}}}", 0),
        ("export function make(mode){const n=1;return {n,run:()=>mode}}", 1),
        ("export function make(mode:number):object;export function make(mode){return {run:()=>mode}}", 0),
    ] {
        let found = read(source, 8 * 1024 * 1024).unwrap_or_else(|e| panic!("{source}: {e:?}"));
        assert_eq!(found.parameters.len(), 1, "{source}");
        assert_eq!(found.setup.len(), setup, "{source}");
        assert_eq!(found.parsed.tree.kind(found.object), "ObjectExpression");
        assert_eq!(found.source, source);
        assert!(found.work > 0);
        let symbol = found.parsed.scopes.symbol(found.parameters[0]);
        assert_eq!(symbol.name, "mode");
        assert!(found.parsed.tree.span(found.function).contains(found.parsed.tree.span(found.object)));
    }
}

/// Unsupported syntax, initialization uncertainty and mutation each independently refuse.
#[test]
fn returned_objects_refuse_uncertain_construction() {
    for source in [
        "export function make(){effect();return {run:()=>{}}}",
        "export function make(flag){if(flag)return {};return {run:()=>{}}}",
        "export function make(){const cases={run:()=>{}};read(cases);return cases}",
        "export function make(){const cases={run:()=>{}};cases.run=other;return cases}",
        "export function make(){return {...other}}",
        "export function make(){return {get run(){return ()=>{}}}}",
        "export function make(){return {set run(v){}}}",
        "export function make(){return {run:()=>{},run:()=>{}}}",
        "export function make(){return {nested:{run:()=>{},run:()=>{}}}}",
        "export function make(){const a={run:()=>1,run:()=>2};const b=a;return {b}}",
        "export function make(){const a={run(){}};return {a}}",
        "export function make(){return {a:[{run:()=>1,run:()=>2}]}}",
        "export function make(){const a=[{run(){}}];return {a}}",
        "export function make(){return {[1]:()=>{}}}",
        "export function make(){return {__proto__:null}}",
        "export function make(){return {run(){}}}",
        "export function make(){return undefined}",
        "export function make(){return;}",
        "export function make(){return {};const n=1}",
        "export const make=()=>({run:()=>{}})",
        "export async function make(){return {run:()=>{}}}",
        "export function* make(){return {run:()=>{}}}",
        "export function make(){return {run:()=>this}}",
        "export function make({mode}){return {run:()=>mode}}",
        "export function make(mode=1){return {run:()=>mode}}",
        "export function make(...mode){return {run:()=>mode}}",
        "export function make(mode){mode=1;return {run:()=>mode}}",
        "export function make(mode){var mode;return {run:()=>mode}}",
        "export function make(mode){eval('');return {run:()=>mode}}",
        "export function make(){const n=unknown;return {run:()=>n}}",
        "export function make(){const n=other.x;return {run:()=>n}}",
        "export function make(){const n=later;const later=1;return {run:()=>n}}",
        "export function make(){let n=1;return {run:()=>n}}",
        "export function make(){using n=unknown;return {run:()=>n}}",
    ] {
        assert!(read(source, 8 * 1024 * 1024).is_err(), "{source}");
    }
}

/// The runtime-oracle source also pins the structural refusal and its unique-object control.
#[test]
fn returned_objects_check_owned_alias_properties() {
    let source = include_str!("../../../../fixtures/oracle/returned-object-proof/factory.ts");
    assert!(matches!(read(source, usize::MAX), Err(Refusal::Body)));
    assert!(matches!(
        read_export(source, "arrayMethod", usize::MAX),
        Err(Refusal::Body)
    ));
    let found = read_export(source, "valid", usize::MAX).unwrap();
    assert_eq!(found.parameters.len(), 0);
    assert_eq!(found.setup.len(), 1);
    assert_eq!(found.parsed.tree.kind(found.object), "ObjectExpression");
}

/// A completed proof's exact work allowance accepts; one less refuses rather than truncating.
#[test]
fn returned_objects_charge_setup_and_property_work() {
    // Three construction visits plus three property visits must fit independently.
    let nested = "export function make(){return {nested:{run:()=>{}}}}";
    assert_eq!(read(nested, 3).unwrap_err(), Refusal::Limit);
    assert_eq!(read(nested, 6).unwrap().work, 6);
    for source in [
        "export function make(){return {nested:{run:()=>{}}}}",
        "export function make(mode){const prefix=mode;return {run:()=>prefix}}",
        "export function make(){const n=1;const a=n;const cases={run:()=>a};return cases}",
    ] {
        let work = read(source, usize::MAX).unwrap().work;
        assert_eq!(read(source, work).unwrap().work, work);
        assert_eq!(read(source, work - 1).unwrap_err(), Refusal::Limit);
        assert_eq!(read(source, 0).unwrap_err(), Refusal::Limit);
    }
    let source = format!(
        "export function make(){{{}return {{run:()=>{{}}}}}}",
        "const n=0;".repeat(2000)
    );
    assert_eq!(read(&source, usize::MAX).unwrap_err(), Refusal::Limit);
    let source = format!(
        "export function make(){{const n=[{}];return {{run:()=>n}}}}",
        vec!["0"; 4096].join(",")
    );
    assert_eq!(read(&source, usize::MAX).unwrap_err(), Refusal::Limit);
}

/// Shared resolution keeps checked export hops, dependency bytes and ordinary runner refusals.
#[test]
fn returned_objects_keep_linkage_and_registration_boundaries() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.ts");
    let helper = root.path().join("factory.ts");
    std::fs::write(&path, "export {make} from './factory'").unwrap();
    let source = "export function make(mode){return {run:()=>mode}}";
    std::fs::write(&helper, source).unwrap();
    let setting = Setting {
        dialect: &DIALECT,
        globals: false,
        root: root.path(),
    };
    let sources = Sources::default();
    let found = returned_template(&sources, &path, "make", setting, usize::MAX).unwrap();
    assert_eq!(
        found.path,
        sources.resolve(&path, "./factory", root.path()).unwrap()
    );
    assert_eq!(
        found.path.canonicalize().unwrap(),
        helper.canonicalize().unwrap()
    );
    assert_eq!(found.source, source);
    assert_eq!(found.resolution_depth, 2);
    assert_eq!(found.dependencies.len(), 2);
    assert_eq!(found.dependency_bytes.len(), 2);
    assert!(found
        .dependency_bytes
        .iter()
        .any(|(p, bytes)| p == &found.path && *bytes == source.len()));
    assert!(matches!(
        template(&sources, &path, "make", setting),
        Err(Refusal::Body)
    ));
    for source in [
        "export type {make} from './factory'",
        "import './effect';export {make} from './factory'",
        "export {missing as make} from './factory'",
        "export {make} from './missing'",
    ] {
        std::fs::write(&path, source).unwrap();
        std::fs::write(root.path().join("effect.ts"), "effect()").unwrap();
        assert!(
            returned_template(&sources, &path, "make", setting, usize::MAX).is_err(),
            "{source}"
        );
    }
}
