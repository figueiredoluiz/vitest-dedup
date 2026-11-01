//! Retained accounting follows the engine's actual owned populations and capacities.
use super::*;
use crate::memory::{HeapBytes, MemoryRoof};
use std::path::Path;

/// Read a small owned project through the same shell-facing engine entry point.
fn engine() -> Engine {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.test.ts");
    let source = "import {test,expect} from 'vitest'; test('a',()=>expect(value).toBe(1)); test('b',()=>expect(value).toBe(1));";
    fs::write(&path, source).unwrap();
    let mut engine = Engine::default();
    let (excluded, partial) = engine.read(
        &path,
        Path::new("a.test.ts"),
        source,
        Setting {
            dialect: &DIALECT,
            globals: false,
            root: dir.path(),
        },
        true,
        Vec::new(),
    );
    assert!(excluded.is_none());
    assert!(!partial);
    assert_eq!(engine.tests, 2);
    assert_eq!(engine.files[0].1.len(), 2);
    engine
}

/// An empty engine costs nothing; each independently retained population contributes its bucket.
#[test]
fn all_engine_populations_are_charged() {
    assert_eq!(Engine::default().heap_bytes(), 0);
    let source="import {test,expect} from 'vitest';test('a',()=>expect(value).toBe(1));test('b',()=>expect(value).toBe(1));";
    let mut corpus = corpora(&[("a.test.ts", source)], &["a.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 2);
    let before = corpus.tests[0].heap_bytes();
    corpus.tests[0].title.as_mut().unwrap().reserve(8192);
    corpus.tests[0].trace.reserve(256);
    assert!(corpus.tests[0].heap_bytes() > before);
    let case = corpus.tests[0].clone();
    corpus.suites.push(case.clone());
    corpus.hooks.declarations.push(crate::corpus::Hook {
        span: case.span,
        timing: crate::framework::HookTiming::BeforeEach,
        label: "phase",
        scope: None,
        key: Some("effect".into()),
    });
    corpus.hooks.chains.push(vec![0]);
    let mut run = Engine::default();
    let (excluded, partial) = run.record(Path::new("a.test.ts"), Ok(Ok(corpus)), true, Vec::new());
    assert!(excluded.is_none());
    assert!(!partial);
    run.files.reserve(32);
    let fields = run.memory_fields();
    assert_eq!(fields.cases, run.files.heap_bytes());
    assert_eq!(fields.suites, run.suites.heap_bytes());
    assert_eq!(fields.hooks, run.hooks.heap_bytes());
    assert_eq!(fields.sites, run.sites.heap_bytes());
    assert_eq!(fields.blind, run.blind.heap_bytes());
    assert!(fields.cases > 0 && fields.suites > 0 && fields.hooks > 0 && fields.sites > 0);
    assert_eq!(run.heap_bytes(), fields.total());
    run.take_files();
    assert_eq!(run.memory_fields().cases, 0);
}

/// Retained heap alone enforces the roof when the platform cannot sample RSS.
#[test]
fn engine_heap_is_not_free_when_rss_is_missing() {
    let run = engine();
    let empty = Vec::new();
    let held = run.heap_bytes();
    assert!(held > 0);
    let roof = MemoryRoof::new(Some(held as u64 - 1));
    assert!(roof.exceeded_with_retained(&empty, held, || None));
    assert!(!MemoryRoof::new(Some(held as u64)).exceeded_with_retained(&empty, held, || Some(0)));
    assert!(!MemoryRoof::new(None)
        .exceeded_with_retained(&empty, usize::MAX, || panic!("disabled budget sampled RSS")));
    assert!(!MemoryRoof::new(Some(held as u64)).exceeded_with_retained(&empty, held, || None));
}

/// The debug snapshot uses retained counts before moving cases to the evaluator.
#[test]
fn memory_snapshot_precedes_case_transfer() {
    let mut run = engine();
    let report = run.memory_report(1);
    assert_eq!(report.tests_analyzed, 2);
    assert_eq!(report.files_analyzed, 1);
    assert_eq!(report.declarations_retained, 2);
    assert_eq!(report.retained_bytes, run.heap_bytes());
    assert_eq!(report.fields.total(), report.retained_bytes);
    assert_eq!(report.engine_fields.unwrap().total(), report.retained_bytes);
    run.take_files();
    assert_eq!(run.memory_fields().cases, 0);
    assert!(report.retained_bytes > run.heap_bytes());
}

/// Inventory and normalization cache identities are retained even after imported parser trees die.
#[test]
fn imported_identities_are_part_of_the_engine_roof() {
    let dir = tempfile::tempdir().unwrap();
    let source="import {test,expect} from 'vitest';import {value} from './values';import {register} from './helpers';register();test('a',()=>expect(value).toBe(1));";
    fs::write(dir.path().join("values.ts"), "export const value=1;").unwrap();
    fs::write(
        dir.path().join("helpers.ts"),
        "import {test} from 'vitest';export function register(){test('hidden',()=>{})}",
    )
    .unwrap();
    let path = dir.path().join("a.test.ts");
    fs::write(&path, source).unwrap();
    let mut run = Engine::default();
    run.read(
        &path,
        Path::new("a.test.ts"),
        source,
        Setting {
            dialect: &DIALECT,
            globals: false,
            root: dir.path(),
        },
        false,
        Vec::new(),
    );
    assert_eq!(run.tests, 2);
    let fields = run.memory_fields();
    assert!(fields.modules > 0);
    assert!(fields.exports > 0);
    assert_eq!(fields.cases, run.files.heap_bytes());
    assert_eq!(
        fields.modules,
        crate::memory::HeapBytes::heap_bytes(&run.run.modules)
    );
}

/// Blind paths remain owned while optional site records are absent when not requested.
#[test]
fn blind_file_storage_is_independent_of_optional_sites() {
    let source="import {test,expect} from 'vitest';const callback=()=>expect(value).toBe(1);test('named',callback);";
    let corpus = corpora(&[("a.test.ts", source)], &["a.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 1);
    let mut run = Engine::default();
    run.record(Path::new("a.test.ts"), Ok(Ok(corpus)), false, Vec::new());
    assert_eq!(run.tests, 1);
    assert_eq!(run.unresolved, 1);
    let fields = run.memory_fields();
    assert!(fields.blind > 0);
    assert_eq!(fields.blind, run.blind.heap_bytes());
    assert_eq!(fields.sites, 0);
}

/// Each heap-bearing case dimension charges reserved capacity independently, not just contents.
#[test]
fn case_heap_dimensions_each_charge_their_own_capacity() {
    let run = engine();
    let template = &run.files[0].1[0];
    for dimension in 0..5 {
        let mut case = template.clone();
        let before = case.heap_bytes();
        let growth = match dimension {
            0 => {
                let text = case.title.as_mut().unwrap();
                let old = text.capacity();
                text.reserve(8192);
                text.capacity() - old
            }
            1 => {
                let text = case.full_title.as_mut().unwrap();
                let old = text.capacity();
                text.reserve(8192);
                text.capacity() - old
            }
            2 => {
                let names = case.name.as_mut().unwrap();
                let old = names.capacity();
                names.reserve(32);
                (names.capacity() - old) * size_of::<String>()
            }
            3 => {
                let text = &mut case.name.as_mut().unwrap()[0];
                let old = text.capacity();
                text.reserve(8192);
                text.capacity() - old
            }
            _ => {
                let old = case.trace.capacity();
                case.trace.reserve(256);
                (case.trace.capacity() - old) * size_of::<u128>()
            }
        };
        assert!(growth > 0);
        assert_eq!(case.heap_bytes() - before, growth, "dimension {dimension}");
    }
}
