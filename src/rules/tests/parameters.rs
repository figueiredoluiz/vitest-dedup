//! Consolidation grouping, eligibility, severity and pair-budget controls.

use super::*;

/// Enables only the consolidation rule, so exact-body silence cannot be suite suppression.
fn config(severity: Severity) -> Config {
    let mut result = config_with(&[]);
    for rule in Rule::ALL {
        result.rules.insert(*rule, Severity::Off);
    }
    result
        .rules
        .insert(Rule::ParameterizationCandidate, severity);
    result
}

/// Distinct declarations with one shared shape and distinct exact bodies.
fn candidates(count: u32) -> Vec<TestCase> {
    (1..=count)
        .map(|i| {
            let mut test = case(i, i * 100, u64::from(i), u64::from(i));
            test.body = u64::from(i);
            test.parameterization_key = Some(10);
            test
        })
        .collect()
}

/// Repeated bodies and repeated rows cannot create extra consolidation claims.
#[test]
fn parameterization_collapses_declarations_then_exact_bodies() {
    let mut cases = candidates(4);
    cases[1].body = cases[0].body;
    let mut row = cases[0].clone();
    row.row = Some(1);
    row.body = 99;
    cases.push(row);
    let outcome = run(&[file("a.test.ts", cases)], &config(Severity::Warning));
    assert!(!outcome.budget_exhausted);
    assert_eq!(outcome.findings.len(), 2);
    assert_eq!(outcome.findings[0].titles, ["t1", "t3"]);
    assert_eq!(outcome.findings[1].titles, ["t3", "t4"]);
}

/// Every eligibility dimension is independently capable of preventing a finding.
#[test]
fn parameterization_keeps_scope_and_eligibility() {
    let cfg = config(Severity::Warning);
    for change in 0..4 {
        let mut cases = candidates(2);
        match change {
            0 => cases[1].parameterization_key = Some(11),
            1 => cases[1].parameterization_key = None,
            2 => cases[1].exclusion = Some(Exclusion::NamedCallback),
            _ => cases[1].body = cases[0].body,
        }
        assert!(run(&[file("a.test.ts", cases)], &cfg).findings.is_empty());
    }
    let cases = candidates(2);
    assert!(run(
        &[
            file("a.test.ts", vec![cases[0].clone()]),
            file("b.test.ts", vec![cases[1].clone()])
        ],
        &cfg
    )
    .findings
    .is_empty());
    let mut rows = candidates(2);
    rows[1].span = rows[0].span;
    rows[0].row = Some(0);
    rows[1].row = Some(1);
    assert!(run(&[file("a.test.ts", rows)], &cfg).findings.is_empty());
}

/// A cluster spends all its spanning comparisons before it emits, and off spends nothing.
#[test]
fn parameterization_budgets_clusters_and_honors_off() {
    let files = [file("a.test.ts", candidates(6))];
    let cfg = config(Severity::Warning);
    let full = run_with_budget(&files, &cfg, 5);
    assert!(!full.budget_exhausted);
    assert_eq!(full.findings.len(), 1);
    assert_eq!(full.findings[0].locations.len(), 6);
    assert!(full.findings[0]
        .message
        .contains("keeps every case as its own row"));
    let short = run_with_budget(&files, &cfg, 4);
    assert!(short.budget_exhausted);
    assert!(short.findings.is_empty());
    let off = run_with_budget(&files, &config(Severity::Off), 0);
    assert!(!off.budget_exhausted);
    assert!(off.findings.is_empty());
}

/// Report order and participant metadata do not depend on the input order.
#[test]
fn parameterization_orders_spanning_pairs_and_keeps_metadata() {
    let mut cases = candidates(3);
    cases[0].inert = true;
    cases[0].full_title = None;
    let cfg = config(Severity::Error);
    let expected = run_with_budget(&[file("a.test.ts", cases.clone())], &cfg, 2);
    assert!(!expected.budget_exhausted);
    assert_eq!(expected.findings.len(), 2);
    assert_eq!(expected.findings[0].severity, Severity::Error);
    assert_eq!(expected.findings[0].inert, [true, false]);
    assert_eq!(
        expected.findings[0].titles,
        ["<dynamic title at line 1>", "t2"]
    );
    cases.reverse();
    let actual = run(&[file("a.test.ts", cases.clone())], &cfg);
    assert_eq!(actual.findings, expected.findings);
    let short = run_with_budget(&[file("a.test.ts", cases)], &cfg, 1);
    assert!(short.budget_exhausted);
    assert!(short.findings.is_empty());
}
