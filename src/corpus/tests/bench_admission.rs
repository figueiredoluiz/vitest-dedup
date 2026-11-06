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

/// Vitest 4 runs no hook around a benchmark, so a before-each reset does not make shared state
/// safe for one: two benchmarks counting the same binding are order-dependent, as two tests are
/// only without the hook.
#[test]
fn a_hook_reset_does_not_protect_a_benchmark() {
    let shape = |kind: &str| {
        format!(
            "import {{ beforeEach }} from 'vitest'\nlet count = 0\nbeforeEach(() => {{ count = 0 }})\n\
             {kind}('a', () => {{ count += 1; String(count) }})\n\
             {kind}('b', () => {{ count += 1; String(count) }})\n"
        )
    };
    let (benches, _) = read(&shape("bench"), true);
    assert_eq!(benches["a"].exclusion, Some(Exclusion::OrderDependentState));
    assert_eq!(benches["b"].exclusion, Some(Exclusion::OrderDependentState));
    let (tests, _) = read(&shape("test"), true);
    assert!(tests["a"].exclusion.is_none(), "{:?}", tests["a"].exclusion);
}

/// Benchmarks that differ only in a literal are told to register from a loop of `bench` calls,
/// never to move into a `test.each` table, which would stop measuring them.
#[test]
fn a_benchmark_parameterization_keeps_them_benchmarks() {
    let message = |kind: &str| {
        let (_, corpus) = read(
            &format!(
                "{kind}('a', () => {{ JSON.stringify({{ a: 1 }}) }})\n\
                 {kind}('b', () => {{ JSON.stringify({{ a: 2 }}) }})\n"
            ),
            true,
        );
        owned_findings("speed.bench.ts", corpus)
            .findings
            .into_iter()
            .find(|finding| finding.rule == crate::model::Rule::ParameterizationCandidate)
            .map(|finding| finding.message)
            .unwrap_or_else(|| panic!("no parameterization finding for {kind}"))
    };
    let bench = message("bench");
    assert!(bench.contains("loop of `bench(...)` calls"), "{bench}");
    assert!(
        !bench.contains("consolidate into one `test.each` table"),
        "{bench}"
    );
    assert!(message("test").contains("consolidate into one `test.each` table"));
}
