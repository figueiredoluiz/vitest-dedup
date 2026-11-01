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
fn parameterization_collapses_exact_bodies_then_declarations() {
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

/// All rows participate in exact-body coverage before one row represents each declaration.
#[test]
fn parameterization_row_order_preserves_declaration_claims() {
    for (tables, expected) in [
        (vec![vec![1, 2], vec![2, 1]], 0),
        (vec![vec![1, 1, 2], vec![2, 1]], 0),
        (vec![vec![1, 2], vec![2]], 0),
        (vec![vec![1, 2]], 0),
        (vec![vec![1, 2], vec![3]], 1),
        (vec![vec![1, 2], vec![1, 3]], 1),
        (vec![vec![1, 2], vec![3, 4]], 1),
        (vec![vec![1, 2], vec![1, 3], vec![2], vec![3]], 1),
        (vec![vec![1, 2], vec![1, 3], vec![2, 4]], 2),
    ] {
        let mut baseline = None;
        for mask in 0..(1 << tables.len()) {
            let mut cases = Vec::new();
            for (declaration, bodies) in tables.iter().enumerate() {
                let mut bodies = bodies.clone();
                if mask & (1 << declaration) != 0 {
                    bodies.reverse();
                }
                for (row, body) in bodies.into_iter().enumerate() {
                    let site = declaration as u32 + 1;
                    let mut entry = case(site, site * 100, body, body);
                    entry.body = body;
                    entry.row = Some(row);
                    entry.parameterization_key = Some(10);
                    cases.push(entry);
                }
            }
            let files = [file("a.test.ts", cases)];
            let cfg = config(Severity::Warning);
            let outcome = run(&files, &cfg);
            assert_eq!(outcome.findings.len(), expected, "{tables:?}, mask {mask}");
            assert!(!outcome.budget_exhausted);
            let initial = baseline.get_or_insert_with(|| outcome.findings.clone());
            assert_eq!(&outcome.findings, initial, "{tables:?}, mask {mask}");
            let bounded = run_with_budget(&files, &cfg, 0);
            assert!(bounded.findings.is_empty());
            assert_eq!(bounded.budget_exhausted, expected > 0);
        }
    }
}

/// Declaration permutations preserve participants and the exact comparison budget.
#[test]
fn parameterization_declaration_order_preserves_claims_and_budget() {
    for (tables, expected) in [
        (vec![vec![1, 2], vec![2]], 0),
        (vec![vec![1, 2], vec![1, 2, 3]], 0),
        (vec![vec![1, 2], vec![3]], 1),
        (vec![vec![1, 2], vec![1, 3]], 1),
        (vec![vec![1, 2], vec![3, 4], vec![2, 3]], 2),
        (vec![vec![1, 2], vec![1, 3], vec![2, 3]], 1),
    ] {
        let mut order: Vec<_> = (0..tables.len()).collect();
        let mut baseline = None;
        for _ in 0..order.len() {
            order.rotate_left(1);
            for reverse in [false, true] {
                let mut ordered = order.clone();
                if reverse {
                    ordered.reverse();
                }
                let mut cases = Vec::new();
                for (position, &declaration) in ordered.iter().enumerate() {
                    for (row, &body) in tables[declaration].iter().enumerate() {
                        let site = position as u32 + 1;
                        let mut entry = case(site, site * 100, body, body);
                        entry.body = body;
                        entry.full_title = Some(format!("declaration {declaration}"));
                        entry.row = Some(row);
                        entry.parameterization_key = Some(10);
                        cases.push(entry);
                    }
                }
                let files = [file("a.test.ts", cases)];
                let cfg = config(Severity::Warning);
                let outcome = run_with_budget(&files, &cfg, expected as u64);
                assert!(!outcome.budget_exhausted);
                assert_eq!(outcome.findings.len(), expected, "{tables:?}, {ordered:?}");
                let titles: std::collections::BTreeSet<_> = outcome
                    .findings
                    .iter()
                    .flat_map(|finding| finding.titles.iter().cloned())
                    .collect();
                let initial = baseline.get_or_insert_with(|| titles.clone());
                assert_eq!(&titles, initial, "{tables:?}, {ordered:?}");
                if expected > 0 {
                    let short = run_with_budget(&files, &cfg, expected as u64 - 1);
                    assert!(short.budget_exhausted);
                    assert!(short.findings.is_empty());
                }
            }
        }
    }
}

/// Equal body sets share a representative while the novel declaration remains reported.
#[test]
fn parameterization_equal_tables_keep_the_first_location() {
    for names in [["first", "copy"], ["copy", "first"]] {
        let mut cases = Vec::new();
        for (position, title) in names.into_iter().chain(["novel"]).enumerate() {
            let bodies: &[u64] = if title == "novel" { &[3] } else { &[1, 2] };
            for (row, &body) in bodies.iter().enumerate() {
                let site = position as u32 + 1;
                let mut entry = case(site, site * 100, body, body);
                entry.full_title = Some(title.to_owned());
                entry.body = body;
                entry.row = Some(row);
                entry.parameterization_key = Some(10);
                cases.push(entry);
            }
        }
        let outcome = run_with_budget(&[file("a.test.ts", cases)], &config(Severity::Warning), 1);
        assert!(!outcome.budget_exhausted);
        assert_eq!(outcome.findings.len(), 1);
        assert_eq!(outcome.findings[0].titles, [names[0], "novel"]);
    }
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
