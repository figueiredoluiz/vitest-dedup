//! Final findings pin strict ordering, admission, direction and bounded candidate work.

use super::*;

/// A retained case with a distinct body and the requested trace.
fn sample(line: u32, events: &[u128]) -> TestCase {
    TestCase {
        body: u64::from(line),
        trace: events.to_vec(),
        ..case(line, line * 100, u64::from(line), u64::from(line))
    }
}

/// Only subsumption findings, with the rule explicitly enabled.
fn claims(cases: Vec<TestCase>) -> Vec<Finding> {
    run(
        &[file("a.test.ts", cases)],
        &config_with(&[(Rule::SubsumedTest, Severity::Warning)]),
    )
    .findings
    .into_iter()
    .filter(|f| f.rule == Rule::SubsumedTest)
    .collect()
}

/// Every direction puts the shorter trace first, regardless of source order.
#[test]
fn directions_and_one_event_positive() {
    for cases in [
        vec![sample(1, &[10]), sample(2, &[10, 20])],
        vec![sample(2, &[10, 20]), sample(1, &[10])],
    ] {
        let findings = claims(cases);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].titles, ["t1", "t2"]);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(findings[0].message.contains("1 behavior events"));
    }
}

/// A larger declaration earlier in the file exercises the reverse-direction branch.
#[test]
fn larger_first_keeps_smaller_participant_first() {
    let findings = claims(vec![sample(1, &[10, 20]), sample(2, &[10])]);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].titles, ["t2", "t1"]);
    assert_eq!(findings[0].locations[0].span.start.line, 2);
}

/// Order, multiplicity, strictness and a changed event each constrain the relation.
#[test]
fn conflicting_and_equal_traces_are_silent() {
    for (a, b) in [
        (vec![20, 10], vec![10, 20, 30]),
        (vec![10, 10], vec![10, 20, 30]),
        (vec![10, 40], vec![10, 20, 30]),
        (vec![10, 20], vec![10, 20]),
        (vec![], vec![10, 20]),
    ] {
        assert!(claims(vec![sample(1, &a), sample(2, &b)]).is_empty());
    }
    assert_eq!(
        claims(vec![sample(1, &[10, 10]), sample(2, &[10, 20, 10])]).len(),
        1
    );
}

/// Every eligibility dimension must participate, rather than merely retaining defaults.
#[test]
fn admission_boundaries() {
    let a = sample(1, &[10]);
    for variant in 0..5 {
        let mut b = sample(2, &[10, 20]);
        match variant {
            0 => b.trace_complete = false,
            1 => b.exclusion = Some(Exclusion::OrderDependentState),
            2 => b.subsumption_key = Some(1),
            3 => b.body = a.body,
            _ => b.span.start_byte = a.span.start_byte,
        }
        assert!(claims(vec![a.clone(), b]).is_empty(), "variant {variant}");
    }
    let files = [
        file("a.ts", vec![a]),
        file("b.ts", vec![sample(2, &[10, 20])]),
    ];
    assert!(run(
        &files,
        &config_with(&[(Rule::SubsumedTest, Severity::Warning)])
    )
    .findings
    .is_empty());
}

/// All three directional pairs survive; a strict relation never becomes an equality cluster.
#[test]
fn pairwise_and_table_coverage() {
    assert_eq!(
        claims(vec![
            sample(1, &[10]),
            sample(2, &[10, 20]),
            sample(3, &[10, 20, 30])
        ])
        .len(),
        3
    );
    let first = sample(1, &[40]);
    let mut row = sample(1, &[10]);
    row.row = Some(1);
    let large = sample(2, &[10, 20]);
    assert_eq!(
        claims(vec![first, row.clone(), large.clone(), row, large]).len(),
        1
    );
}

/// Opt-in severity and candidate-work exhaustion are observable even without matches.
#[test]
fn disabled_and_budget() {
    let files = [file(
        "a.ts",
        vec![sample(1, &[10]), sample(2, &[10, 20]), sample(3, &[40])],
    )];
    assert!(run(&files, &config_with(&[])).findings.is_empty());
    let config = config_with(&[(Rule::SubsumedTest, Severity::Error)]);
    let zero = run_with_budget(&files, &config, 0);
    assert!(zero.budget_exhausted);
    assert!(zero.findings.is_empty());
    let one = run_with_budget(&files, &config, 1);
    assert!(one.budget_exhausted);
    assert_eq!(one.findings.len(), 1);
    assert_eq!(one.findings[0].severity, Severity::Error);
}

/// Subsumption must not consume the budget before the stronger cross-file rule.
#[test]
fn cross_file_claim_precedes_subsumption_work() {
    let mut a = sample(1, &[10]);
    a.module_key = 99;
    let b = sample(2, &[10, 20]);
    let mut c = sample(3, &[40]);
    c.module_key = 99;
    let files = [file("a.ts", vec![a, b]), file("b.ts", vec![c])];
    let result = run_with_budget(
        &files,
        &config_with(&[(Rule::SubsumedTest, Severity::Warning)]),
        1,
    );
    assert!(result.budget_exhausted);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].rule, Rule::CrossFileDuplicate);
}
