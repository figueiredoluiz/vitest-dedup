//! Module metadata follows lexical provenance through runner expansion and cache reuse.
use super::*;
use crate::model::Rule;

const RUNNER: &str = include_str!("../../../fixtures/oracle/runner-module-meta/runner.ts");
const CONFLICTING: &str =
    include_str!("../../../fixtures/oracle/runner-module-meta/conflicting.test.ts");
const SAME: &str = include_str!("../../../fixtures/oracle/runner-module-meta/same.test.ts");
const OTHER: &str = include_str!("../../../fixtures/oracle/runner-module-meta/other.test.ts");

/// Read exactly two comparable executions before making any equality or finding assertion.
pub(super) fn compared_pair(files: &[(&str, &str)], path: &str) -> FileCorpus {
    let corpus = corpora(files, &[path]).remove(0);
    assert_eq!(corpus.tests.len(), 2, "{path}");
    assert!(corpus.tests.iter().all(|case| case.exclusion.is_none()));
    corpus
}

/// Different metadata owners must stay distinct in the body and every finding consumer.
pub(super) fn distinct_modules(files: &[(&str, &str)], path: &str) {
    let corpus = compared_pair(files, path);
    assert_ne!(corpus.tests[0].body, corpus.tests[1].body);
    assert_ne!(corpus.tests[0].key, corpus.tests[1].key);
    assert!(owned_findings(path, corpus).findings.is_empty());
}

/// Require the retained duplicate claim while allowing a caller to inspect companion findings.
pub(super) fn duplicate(path: &str, corpus: FileCorpus) -> crate::rules::Outcome {
    let outcome = owned_findings(path, corpus);
    assert!(outcome
        .findings
        .iter()
        .any(|f| f.rule == Rule::DuplicateTest));
    outcome
}

/// Runtime-different assertions stay compared but never acquire a duplicate or subsumption claim.
#[test]
fn module_meta_distinguishes_a_runner_from_the_callers_written_out_body() {
    distinct_modules(
        &[("runner.ts", RUNNER), ("conflicting.test.ts", CONFLICTING)],
        "conflicting.test.ts",
    );
}

/// Reading callers in either order keeps the runner's module identity on cache hits.
#[test]
fn module_meta_is_owned_by_the_runner_not_each_registering_file() {
    let files = [
        ("runner.ts", RUNNER),
        ("same.test.ts", SAME),
        ("other.test.ts", OTHER),
    ];
    for order in [
        ["same.test.ts", "other.test.ts"],
        ["other.test.ts", "same.test.ts"],
    ] {
        let results = corpora(&files, &order);
        assert_eq!(results.iter().map(|c| c.tests.len()).sum::<usize>(), 3);
        let cases = results.iter().flat_map(|c| &c.tests).collect::<Vec<_>>();
        assert!(cases.iter().all(|case| case.exclusion.is_none()));
        assert!(cases.iter().all(|case| case.key == cases[0].key));
    }
}

/// Distinct declarations in one source still produce the original metadata positive findings.
#[test]
fn module_meta_preserves_same_source_duplicate_and_name_findings() {
    let runner = include_str!("../../../fixtures/recall/runner-import-meta/runner.ts");
    let caller = include_str!("../../../fixtures/recall/runner-import-meta/meta.test.ts");
    let corpus = compared_pair(
        &[("runner.ts", runner), ("meta.test.ts", caller)],
        "meta.test.ts",
    );
    let outcome = duplicate("meta.test.ts", corpus);
    assert!(outcome
        .findings
        .iter()
        .any(|f| f.rule == Rule::DuplicateTestName));
}

/// Every metadata spelling reaches body, trace and structural consumers without losing provenance.
#[test]
fn module_meta_provenance_survives_member_alias_and_nested_callback_forms() {
    for expression in [
        "expect(import.meta).toBeTruthy()",
        "expect(import.meta['url']).toBeTruthy()",
        "expect(new URL('./asset', import.meta.url).href).toBeTruthy()",
        "const meta = import.meta; expect(meta.url).toBeTruthy()",
        "expect(() => import.meta.url).toBeTruthy()",
    ] {
        let original = "expect(import.meta.url.endsWith('/runner.ts')).toBe(true)";
        let runner = RUNNER.replace(original, expression);
        let caller = CONFLICTING.replace(original, expression);
        distinct_modules(
            &[("runner.ts", &runner), ("conflicting.test.ts", &caller)],
            "conflicting.test.ts",
        );
    }
}

/// Equal source text in two runner modules must not erase the modules they observe.
#[test]
fn module_meta_distinguishes_separate_runner_sources() {
    let caller =
        "import {run as a} from './a'; import {run as b} from './b'; a('first'); b('second');";
    distinct_modules(
        &[("a.ts", RUNNER), ("b.ts", RUNNER), ("case.test.ts", caller)],
        "case.test.ts",
    );
}

/// Checkout location is not module identity, and sibling filenames are not equal identities.
#[test]
fn module_meta_keys_are_root_relative_and_preserve_same_file_positives() {
    let source = "import {test,expect} from 'vitest'; test('first',()=>expect(import.meta.url).toBeTruthy()); test('second',()=>expect(import.meta.url).toBeTruthy());";
    let first = compared_pair(&[("a.test.ts", source)], "a.test.ts");
    let repeated = compared_pair(&[("a.test.ts", source)], "a.test.ts");
    let other = compared_pair(&[("b.test.ts", source)], "b.test.ts");
    for corpus in [&first, &repeated, &other] {
        assert_eq!(corpus.tests[0].key, corpus.tests[1].key);
    }
    assert_eq!(first.tests[0].key, repeated.tests[0].key);
    assert_ne!(first.tests[0].key, other.tests[0].key);
    duplicate("a.test.ts", first);

    let files = [("runner.ts", RUNNER), ("same.test.ts", SAME)];
    let first = compared_pair(&files, "same.test.ts");
    let repeated = compared_pair(&files, "same.test.ts");
    assert_eq!(first.tests[0].key, repeated.tests[0].key);
}

/// The other JavaScript meta property and metadata-free bodies retain cross-source positives.
#[test]
fn module_meta_does_not_partition_unrelated_callback_bodies() {
    for body in ["expect(new.target).toBeUndefined()", "expect(1).toBe(1)"] {
        let runner = format!("import {{test,expect}} from 'vitest'; export function run(label) {{test(label,function(){{{body}}});}}");
        let caller = format!("import {{test,expect}} from 'vitest'; import {{run}} from './runner'; run('imported'); test('direct',function(){{{body}}});");
        let corpus = compared_pair(
            &[("runner.ts", &runner), ("case.test.ts", &caller)],
            "case.test.ts",
        );
        assert_eq!(corpus.tests[0].key, corpus.tests[1].key, "{body}");
        duplicate("case.test.ts", corpus);
    }
}
