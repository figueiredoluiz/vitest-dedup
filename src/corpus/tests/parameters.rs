//! Parameterization outcomes over the same corpus records every next-engine rule reads.

use super::*;
use crate::model::Rule;

/// Checks extraction and comparison eligibility before a caller inspects any rule result.
fn eligible(source: &str, expected: usize) -> FileCorpus {
    let corpus = corpora(&[("a.test.ts", source)], &["a.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), expected, "{source}");
    assert!(
        corpus.tests.iter().all(|case| case.exclusion.is_none()),
        "{source}"
    );
    corpus
}

/// Requires exactly one final consolidation finding and its two named participants.
fn only_candidate(corpus: FileCorpus, titles: [&str; 2]) {
    let findings = owned_findings("a.test.ts", corpus).findings;
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert_eq!(findings[0].rule, Rule::ParameterizationCandidate);
    assert_eq!(findings[0].titles, titles);
}

/// Extracts a pair, proves both are eligible, then checks the final rule outcome.
fn pair(left: &str, right: &str, expected: usize) {
    let source = format!("{HEADER}test('left', {left}); test('right', {right});");
    let corpus = eligible(&source, 2);
    assert!(corpus
        .tests
        .iter()
        .all(|case| case.parameterization_key.is_some()));
    let findings = owned_findings("a.test.ts", corpus).findings;
    let candidates: Vec<_> = findings
        .iter()
        .filter(|f| f.rule == Rule::ParameterizationCandidate)
        .collect();
    assert_eq!(candidates.len(), expected, "{source}\n{findings:#?}");
    for finding in candidates {
        assert_eq!(finding.titles, ["left", "right"]);
    }
}

/// Literal values vary while their kinds, operations and local binding relationships agree.
#[test]
fn parameterization_keeps_literal_cases() {
    for (left, right) in [
        (
            "() => expect(check(0)).toBe(false)",
            "() => expect(check(1)).toBe(true)",
        ),
        (
            "() => { const first = 'a'; expect(first).toBe('a') }",
            "() => { const second = 'b'; expect(second).toBe('b') }",
        ),
        ("() => expect(1n).toBe(1n)", "() => expect(2n).toBe(2n)"),
        (
            "async () => { const m = await import(`./same`); expect(m.run(0)).toBe(0) }",
            "async () => { const m = await import(`./same`); expect(m.run(1)).toBe(1) }",
        ),
        (
            "() => { const { ['a']: value } = { a: 0 }; expect(value).toBe(0) }",
            "() => { const { ['a']: value } = { a: 1 }; expect(value).toBe(1) }",
        ),
        (
            "() => expect(obj[`a`]).toBe(0)",
            "() => expect(obj[`a`]).toBe(1)",
        ),
        (
            "() => expect(/a/).toEqual(/a/)",
            "() => expect(/b/i).toEqual(/b/i)",
        ),
        ("() => expect(`a`).toBe('a')", "() => expect('b').toBe(`b`)"),
        (
            "() => { expect(0).toBe(0) }",
            "function () { expect(1).toBe(1) }",
        ),
    ] {
        pair(left, right, 1);
    }
}

/// Erasing a structural dimension or dropping the substance floor must change these outcomes.
#[test]
fn parameterization_preserves_conflicting_structure() {
    for (left, right) in [
        ("() => expect(1).toBe(1)", "() => expect(1).toBe(1)"),
        (
            "async () => { const m = await import(`./a`); expect(m.value).toBe(0) }",
            "async () => { const m = await import(`./b`); expect(m.value).toBe(1) }",
        ),
        ("() => expect(0).toBe(0)", "() => expect('1').toBe('1')"),
        (
            "() => expect(first(0)).toBe(0)",
            "() => expect(second(1)).toBe(1)",
        ),
        (
            "() => expect(1 + value).toBe(1)",
            "() => expect(2 - value).toBe(2)",
        ),
        (
            "() => { first(0); second(0) }",
            "() => { second(1); first(1) }",
        ),
        (
            "() => expect(obj['a']).toBe(0)",
            "() => expect(obj['b']).toBe(1)",
        ),
        (
            "() => expect(this.value).toBe(0)",
            "function () { expect(this.value).toBe(1) }",
        ),
        ("() => expect(1n).toBe(1n)", "() => expect(1).toBe(1)"),
        (
            "() => expect(obj[`a`]).toBe(0)",
            "() => expect(obj[`b`]).toBe(1)",
        ),
        (
            "() => expect({ [`a`]: 0 }).toBeTruthy()",
            "() => expect({ [`b`]: 1 }).toBeTruthy()",
        ),
        (
            "() => expect(tag`a`).toBe(0)",
            "() => expect(tag`b`).toBe(1)",
        ),
        (
            "() => expect(`a${value}`).toBe(0)",
            "() => expect(`b${value}`).toBe(1)",
        ),
        (
            "() => expectTypeOf<1>().toEqualTypeOf<1>()",
            "() => expectTypeOf<2>().toEqualTypeOf<2>()",
        ),
    ] {
        pair(left, right, 0);
    }
}

/// Context, modifiers and literal options stay exact even though body literals vary.
#[test]
fn parameterization_keeps_registration_semantics() {
    for source in [
        "test('a', () => expect(0).toBe(0)); test.fails('b', () => expect(1).toBe(1));",
        "test('a', { custom: 10 }, () => expect(0).toBe(0)); test('b', { custom: 20 }, () => expect(1).toBe(1));",
        "describe('a', () => { beforeEach(() => setup(0)); test('a', () => expect(0).toBe(0)) }); describe('b', () => { beforeEach(() => setup(1)); test('b', () => expect(1).toBe(1)) });",
        "const a = () => 0; const b = () => 1; test('a', () => expect(a()).toBe(0)); test('b', () => expect(b()).toBe(1));",
        "test.each([[0]])('a', (...values) => expect(values).toEqual([0])); test.each([[1]])('b', (...values) => expect(values).toEqual([1]));",
    ] {
        let source = format!("{HEADER}{source}");
        let corpus = eligible(&source, 2);
        let outcome = owned_findings("a.test.ts", corpus);
        assert!(outcome.findings.iter().all(|f| f.rule != Rule::ParameterizationCandidate), "{source}\n{outcome:#?}");
    }
}

/// A table cannot consolidate with itself, but a bound row can match a written-out case.
#[test]
fn parameterization_uses_proven_rows_once_per_declaration() {
    let source = format!("{HEADER}test.each([0, 1])('row %i', value => expect(value).toBe(value)); test('manual', () => expect(2).toBe(2));");
    let corpus = eligible(&source, 3);
    let outcome = owned_findings("a.test.ts", corpus);
    let candidates: Vec<_> = outcome
        .findings
        .iter()
        .filter(|f| f.rule == Rule::ParameterizationCandidate)
        .collect();
    assert_eq!(candidates.len(), 1, "{outcome:#?}");
    assert_eq!(candidates[0].titles, ["row %i", "manual"]);
}

/// The permanent corpus source is also executed under Vitest, including its two table rows.
#[test]
fn parameterization_owned_contract() {
    let source = include_str!("../../../fixtures/duplicates/parameterization-contract.test.ts");
    let corpus = eligible(source, 10);
    let outcome = owned_findings("a.test.ts", corpus);
    let candidates: Vec<_> = outcome
        .findings
        .iter()
        .filter(|f| f.rule == Rule::ParameterizationCandidate)
        .map(|f| f.titles.clone())
        .collect();
    assert_eq!(
        candidates,
        vec![
            vec!["zero boundary", "one boundary"],
            vec!["row %i", "manual row"]
        ],
        "{outcome:#?}"
    );
    let duplicates: Vec<_> = outcome
        .findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateTest)
        .map(|f| f.titles.clone())
        .collect();
    assert_eq!(duplicates, vec![vec!["one boundary", "one repeated"]]);
}

/// Bare literals provide no behavior to consolidate, even though extraction admits both tests.
#[test]
fn parameterization_requires_substance() {
    let source = format!("{HEADER}test('a', () => {{ 0 }}); test('b', () => {{ 1 }});");
    let corpus = eligible(&source, 2);
    assert!(corpus
        .tests
        .iter()
        .all(|case| case.parameterization_key.is_none()));
    assert!(owned_findings("a.test.ts", corpus).findings.is_empty());
}

/// The framework's recorded scheduling-only options do not change the compared behavior.
#[test]
fn parameterization_ignores_scheduling_only_options() {
    let source = format!("{HEADER}test('a', {{ timeout: 10 }}, () => expect(0).toBe(0)); test('b', {{ timeout: 20 }}, () => expect(1).toBe(1));");
    let corpus = eligible(&source, 2);
    let outcome = owned_findings("a.test.ts", corpus);
    assert_eq!(outcome.findings.len(), 1);
    assert_eq!(outcome.findings[0].rule, Rule::ParameterizationCandidate);
}

/// A copied suite does not cover the differing input cases inside its retained representative.
#[test]
fn parameterization_survives_whole_suite_suppression() {
    let suite = "describe('suite', () => { test('zero', () => expect(0).toBe(0)); test('one', () => expect(1).toBe(1)); });";
    let source = format!("{HEADER}{suite}{suite}");
    let corpus = eligible(&source, 4);
    let outcome = owned_findings("a.test.ts", corpus);
    let candidates: Vec<_> = outcome
        .findings
        .iter()
        .filter(|finding| finding.rule == Rule::ParameterizationCandidate)
        .collect();
    assert_eq!(candidates.len(), 1, "{outcome:#?}");
    assert_eq!(candidates[0].titles, ["suite > zero", "suite > one"]);
    assert_eq!(
        outcome
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateSuite)
            .count(),
        1
    );
}

/// These permanent template-snapshot inputs run in Vitest and both corpus gates.
#[test]
fn parameterization_refuses_source_keyed_snapshots() {
    let source =
        include_str!("../../../fixtures/duplicates/parameterization-inline-snapshots.test.ts");
    let corpus = eligible(source, 12);
    assert!(corpus.tests[..10]
        .iter()
        .all(|case| case.parameterization_key.is_none()));
    assert!(corpus.tests[10..]
        .iter()
        .all(|case| case.parameterization_key.is_some()));
    let candidates: Vec<_> = owned_findings("a.test.ts", corpus)
        .findings
        .into_iter()
        .filter(|finding| finding.rule == Rule::ParameterizationCandidate)
        .collect();
    assert_eq!(candidates.len(), 1, "{candidates:#?}");
    assert_eq!(candidates[0].titles, ["ordinary a", "ordinary b"]);
}

/// Quoted arguments, computed keys, aliases and local helper dependencies retain source sensitivity.
#[test]
fn parameterization_tracks_source_keyed_dependencies() {
    for (setup, body) in [
        ("", "expect('a').toMatchInlineSnapshot('a')"),
        ("", "expect('a')[`toMatchInlineSnapshot`]('a')"),
        ("", "expect('a')['toMatch' + 'InlineSnapshot']('a')"),
        ("import { matcher } from './external';", "expect('a')[matcher]('a')"),
        ("const key = 'toMatchInlineSnapshot';", "expect('a')[key]('a')"),
        ("const check = value => expect(value).toMatchInlineSnapshot('a');", "check('a')"),
        ("const first = value => second(value); const second = value => { first; expect(value).toMatchInlineSnapshot('a') };", "first('a')"),
        ("", "const { toMatchInlineSnapshot: snapshot } = expect('a'); snapshot('a')"),
    ] {
        let source = format!("{HEADER}{setup}test('one', () => {{ {body} }});");
        let corpus = eligible(&source, 1);
        assert!(corpus.tests[0].parameterization_key.is_none(), "{source}");
        assert!(owned_findings("a.test.ts", corpus).findings.is_empty());
    }
}

/// Equal helper text at different source sites must not erase binding provenance for consolidation.
#[test]
fn parameterization_preserves_external_binding_sites() {
    let source = format!("{HEADER}const first = value => value; const second = value => value; test('one', () => expect(first(0)).toBe(0)); test('two', () => expect(second(1)).toBe(1)); test('three', () => expect(first(2)).toBe(2));");
    let corpus = eligible(&source, 3);
    assert!(corpus
        .tests
        .iter()
        .all(|case| case.parameterization_key.is_some()));
    only_candidate(corpus, ["one", "three"]);
}

/// The owned computed-key source preserves conflicting names and retains same-key literal cases.
#[test]
fn parameterization_owned_computed_keys() {
    let source =
        include_str!("../../../fixtures/duplicates/parameterization-computed-keys.test.ts");
    let corpus = eligible(source, 8);
    assert!(corpus.tests[..6]
        .iter()
        .all(|case| case.parameterization_key.is_some()));
    assert!(corpus.tests[6..]
        .iter()
        .all(|case| case.parameterization_key.is_none()));
    only_candidate(corpus, ["retain zero", "retain one"]);
}

/// Custom snapshot names and unknown destructuring keys cannot hide source-sensitive assertions.
#[test]
fn parameterization_owned_custom_snapshots() {
    let source =
        include_str!("../../../fixtures/duplicates/parameterization-custom-snapshots.test.ts");
    let corpus = eligible(source, 6);
    assert!(corpus.tests[..4]
        .iter()
        .all(|case| case.parameterization_key.is_none()));
    assert!(corpus.tests[4..]
        .iter()
        .all(|case| case.parameterization_key.is_some()));
    only_candidate(corpus, ["retain a", "retain b"]);
}

/// Module identity is structural while data passed to the same imported operation may vary.
#[test]
fn parameterization_owned_module_sources() {
    let source = include_str!("../../../fixtures/duplicates/parameterization-modules.test.ts");
    let corpus = eligible(source, 4);
    assert!(corpus
        .tests
        .iter()
        .all(|case| case.parameterization_key.is_some()));
    only_candidate(corpus, ["same module zero", "same module one"]);
}

/// Opaque import sources and import options cannot prove the static module identity.
#[test]
fn parameterization_refuses_unproven_module_sources() {
    for body in [
        "async () => { await import(name); expect(0).toBe(0) }",
        "async () => { await import(`./${name}`); expect(0).toBe(0) }",
        "async () => { await import('./data', { with: { type: 'json' } }); expect(0).toBe(0) }",
    ] {
        let source = format!("{HEADER}test('one', {body});");
        let corpus = eligible(&source, 1);
        assert!(corpus.tests[0].parameterization_key.is_none());
        assert!(owned_findings("a.test.ts", corpus).findings.is_empty());
    }
    let source = format!("{HEADER}test.each(['./a'])('a', async name => {{ await import(name); expect(0).toBe(0) }}); test.each(['./b'])('b', async name => {{ await import(name); expect(1).toBe(1) }});");
    let corpus = eligible(&source, 2);
    assert!(corpus
        .tests
        .iter()
        .all(|case| case.parameterization_key.is_none()));
    assert!(owned_findings("a.test.ts", corpus).findings.is_empty());
}

/// Deep external identities keep both owned duplicate pairs with margin below a 1 MiB stack.
#[test]
fn normalization_stack_preserves_owned_findings() {
    std::thread::Builder::new()
        .stack_size(768 * 1024)
        .spawn(|| {
            let source = include_str!("../../../fixtures/duplicates/normalization-stack.test.ts");
            let corpus = eligible(source, 4);
            let findings = owned_findings("a.test.ts", corpus).findings;
            assert_eq!(findings.len(), 2, "{findings:#?}");
            assert!(findings
                .iter()
                .all(|finding| finding.rule == Rule::DuplicateTest));
            let titles: Vec<_> = findings
                .iter()
                .map(|finding| finding.titles.clone())
                .collect();
            assert_eq!(
                titles,
                vec![vec!["deep a", "deep b"], vec!["positive a", "positive b"]]
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

/// Flat-form eligibility does not turn a partial trace into a consolidation proof.
#[test]
fn parameterization_requires_a_complete_substance_trace() {
    let source = format!(
        "{HEADER}const key = 'value';
         test('partial one', () => {{ expect(1).toBe(1); class C {{ [key]() {{ return 1 }} }}; new C() }});
         test('partial two', () => {{ expect(2).toBe(2); class C {{ [key]() {{ return 2 }} }}; new C() }});
         test('complete one', () => {{ expect(3).toBe(3) }});
         test('complete two', () => {{ expect(4).toBe(4) }});"
    );
    let corpus = eligible(&source, 4);
    for case in &corpus.tests[..2] {
        assert!(!case.trace_complete);
        assert!(!case.trace.is_empty());
        assert!(case.parameterization_key.is_none());
    }
    only_candidate(corpus, ["complete one", "complete two"]);
}
