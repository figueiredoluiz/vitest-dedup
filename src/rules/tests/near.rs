//! What `near-duplicate-test` claims, and each case it refuses to compare.

use super::*;

/// A case at `line` whose body and events are the ones given, under one shared similarity key.
fn near(line: u32, body: u64, trace: &[u128]) -> TestCase {
    TestCase {
        body,
        trace: trace.to_vec(),
        similarity_key: 7,
        ..case(
            line,
            line * 100,
            1000 + u64::from(line),
            2000 + u64::from(line),
        )
    }
}

/// The participant lines of every `near-duplicate-test` finding over one file's `cases`.
fn claims_with(cases: Vec<TestCase>, config: &Config) -> Vec<Vec<u32>> {
    let files = [file("a.test.ts", cases)];
    near_findings(&run(&files, config))
}

/// The participant lines of every `near-duplicate-test` finding of `outcome`.
fn near_findings(outcome: &Outcome) -> Vec<Vec<u32>> {
    outcome
        .findings
        .iter()
        .filter(|finding| finding.rule == Rule::NearDuplicateTest)
        .map(|finding| {
            finding
                .locations
                .iter()
                .map(|location| location.span.start.line)
                .collect()
        })
        .collect()
}

/// [`claims_with`] under the built-in severities.
fn claims(cases: Vec<TestCase>) -> Vec<Vec<u32>> {
    claims_with(cases, &config_with(&[]))
}

/// The message of the one finding over `cases`.
fn message_of(cases: Vec<TestCase>) -> String {
    let outcome = run(&[file("a.test.ts", cases)], &config_with(&[]));
    assert_eq!(outcome.findings.len(), 1);
    outcome.findings[0].message.clone()
}

/// Equal events in another order are a pair, and the message says it is a reordering.
#[test]
fn equal_events_in_another_order_are_a_reordered_pair() {
    let cases = vec![near(1, 1, &[10, 20]), near(2, 2, &[20, 10])];
    assert_eq!(claims(cases.clone()), [[1, 2]]);
    assert!(message_of(cases).contains("at least one different ordering"));
}

/// Equal events in one order are a pair the bodies alone tell apart, and the message says so.
#[test]
fn equal_events_in_one_order_name_the_normalization() {
    let message = message_of(vec![near(1, 1, &[10, 20]), near(2, 2, &[10, 20])]);
    assert!(message.contains("in the same order"), "{message}");
}

/// One differing event, or a repeated one, is another multiset: the gate is exact.
#[test]
fn one_differing_event_is_distinct() {
    assert!(claims(vec![near(1, 1, &[10, 20]), near(2, 2, &[10, 30])]).is_empty());
    assert!(claims(vec![near(1, 1, &[10, 10, 20]), near(2, 2, &[10, 20, 20])]).is_empty());
}

/// A pair must agree on everything around its events, in one file.
#[test]
fn a_pair_shares_its_context_and_its_file() {
    let mut other = near(2, 2, &[20, 10]);
    other.similarity_key = 8;
    assert!(claims(vec![near(1, 1, &[10, 20]), other]).is_empty());
    let files = [
        file("a.test.ts", vec![near(1, 1, &[10, 20])]),
        file("b.test.ts", vec![near(2, 2, &[20, 10])]),
    ];
    assert!(near_findings(&run(&files, &config_with(&[]))).is_empty());
}

/// An unfinished trace is no evidence, and a case the corpus refused is never claimed.
#[test]
fn an_incomplete_trace_or_an_exclusion_is_not_compared() {
    let mut incomplete = near(2, 2, &[20, 10]);
    incomplete.trace_complete = false;
    assert!(claims(vec![near(1, 1, &[10, 20]), incomplete]).is_empty());
    let mut excluded = near(2, 2, &[20, 10]);
    excluded.exclusion = Some(Exclusion::OrderDependentState);
    assert!(claims(vec![near(1, 1, &[10, 20]), excluded]).is_empty());
}

/// One event has nothing to reorder.
#[test]
fn one_event_is_below_the_floor() {
    assert!(claims(vec![near(1, 1, &[10]), near(2, 2, &[10])]).is_empty());
}

/// Equal bodies are `duplicate-test`'s claim, and rows of one table are one declaration.
#[test]
fn equal_bodies_and_rows_of_one_table_are_not_pairs() {
    assert!(claims(vec![near(1, 1, &[10, 20]), near(2, 1, &[20, 10])]).is_empty());
    let mut row = near(2, 2, &[20, 10]);
    row.span.start_byte = 100;
    assert!(claims(vec![near(1, 1, &[10, 20]), row]).is_empty());
}

/// A pair parameterization reported keeps that stronger claim, and only that pair does.
#[test]
fn a_pair_parameterization_reported_is_not_claimed_twice() {
    let shaped = |line, body| TestCase {
        parameterization_key: Some(5),
        ..near(line, body, &[10, 20])
    };
    assert!(claims(vec![shaped(1, 1), shaped(2, 2)]).is_empty());
    let third = TestCase {
        parameterization_key: Some(6),
        ..near(3, 3, &[20, 10])
    };
    // Groups chain as consecutive pairs, so the third test still links through the second.
    assert_eq!(claims(vec![shaped(1, 1), shaped(2, 2), third]), [[2, 3]]);
    let silenced = config_with(&[(Rule::ParameterizationCandidate, Severity::Off)]);
    assert_eq!(
        claims_with(vec![shaped(1, 1), shaped(2, 2)], &silenced),
        [[1, 2]]
    );
}

/// The rule obeys its severity and spends the shared budget.
#[test]
fn severity_and_budget_reach_the_rule() {
    let cases = || vec![near(1, 1, &[10, 20]), near(2, 2, &[20, 10])];
    let off = config_with(&[(Rule::NearDuplicateTest, Severity::Off)]);
    assert!(claims_with(cases(), &off).is_empty());
    let files = [file("a.test.ts", cases())];
    let outcome = run_with_budget(&files, &config_with(&[]), 0);
    assert!(outcome.budget_exhausted);
    assert!(near_findings(&outcome).is_empty());
}
