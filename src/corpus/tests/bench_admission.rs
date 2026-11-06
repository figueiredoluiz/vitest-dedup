//! A top-level `bench` in a file known to run it is read as a test is, marked as a benchmark, and
//! never compared with a test; anywhere else it stays one excluded case.

use super::*;

/// The cases of `source`, read with or without benchmarks admitted, keyed by title.
fn read(source: &str, benchmarks: bool) -> (HashMap<String, TestCase>, FileCorpus) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("speed.bench.ts");
    let source = format!("import {{ bench, test, expect }} from 'vitest'\n{source}");
    fs::write(&path, &source).unwrap();
    let setting = Setting {
        dialect: &DIALECT,
        globals: false,
        root: dir.path(),
        benchmarks,
    };
    let corpus = file(&path, &source, setting, &mut Run::default()).unwrap();
    let cases = corpus
        .tests
        .iter()
        .map(|case| (case.title.clone().unwrap_or_default(), case.clone()))
        .collect();
    (cases, corpus)
}

const BODY: &str = "() => { expect([1].length).toBe(1) }";

/// Admitted, two equal benchmarks key alike and are marked; a test with the same body keys apart,
/// and none of them counts in the quality gate but the test.
#[test]
fn an_admitted_benchmark_is_compared_only_with_benchmarks() {
    let source = format!("bench('a', {BODY})\nbench('b', {BODY})\ntest('c', {BODY})\n");
    let (cases, corpus) = read(&source, true);
    assert_eq!(corpus.tests.len(), 3);
    let (a, b, c) = (&cases["a"], &cases["b"], &cases["c"]);
    assert!(a.exclusion.is_none() && b.exclusion.is_none() && c.exclusion.is_none());
    assert!(a.benchmark && b.benchmark && !c.benchmark);
    assert_eq!(a.key, b.key);
    assert_ne!(a.key, c.key);
    assert_ne!(a.module_key, c.module_key);
    assert_ne!(a.similarity_key, c.similarity_key);
    assert!(!a.gate_eligible() && c.gate_eligible());
}

/// Not admitted, every benchmark is one excluded case and the test beside it is unchanged.
#[test]
fn a_benchmark_not_known_to_run_stays_excluded() {
    let source = format!("bench('a', {BODY})\nbench('b', {BODY})\ntest('c', {BODY})\n");
    let (cases, _) = read(&source, false);
    assert_eq!(cases["a"].exclusion, Some(Exclusion::Benchmark));
    assert_eq!(cases["b"].exclusion, Some(Exclusion::Benchmark));
    assert!(cases["c"].exclusion.is_none());
    assert!(!cases["a"].gate_eligible());
}

/// Vitest 4 has no `bench.each` or `bench.for`, so a table of benchmarks throws and is refused.
#[test]
fn a_benchmark_table_is_refused() {
    for chain in ["each", "for"] {
        let (_, corpus) = read(&format!("bench.{chain}([1, 2])('row %i', {BODY})\n"), true);
        assert!(!corpus.tests.is_empty());
        assert!(
            corpus
                .tests
                .iter()
                .all(|case| case.exclusion == Some(Exclusion::Benchmark)),
            "{chain}"
        );
    }
}

/// Two benchmarks of one name collide; a benchmark and a test of that name are different tasks.
#[test]
fn a_benchmark_name_collides_only_with_benchmarks() {
    let names = |source: &str| {
        let (_, corpus) = read(source, true);
        owned_findings("speed.bench.ts", corpus)
            .findings
            .iter()
            .filter(|finding| finding.rule == crate::model::Rule::DuplicateTestName)
            .count()
    };
    let other = "() => { expect([2].length).toBe(1) }";
    assert_eq!(
        names(&format!("bench('x', {BODY})\nbench('x', {other})\n")),
        1
    );
    assert_eq!(
        names(&format!("bench('x', {BODY})\ntest('x', {other})\n")),
        0
    );
}
