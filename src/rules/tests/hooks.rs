//! Budgets, severity and declaration identity for hook claims.

use super::*;
use crate::corpus::{Hook, Hooks};
use crate::framework::HookTiming;

/// One file with three equivalent before-each hooks and two views of the same chain.
fn population() -> Vec<(PathBuf, Hooks)> {
    let declarations = (1..=3)
        .map(|line| Hook {
            span: case(line, line * 100, 0, 0).span,
            timing: HookTiming::BeforeEach,
            label: "beforeEach",
            scope: None,
            key: Some("same".into()),
        })
        .collect();
    vec![(
        PathBuf::from("a.test.ts"),
        Hooks {
            declarations,
            chains: vec![vec![0, 1, 2], vec![0, 1, 2]],
        },
    )]
}

/// Candidate scans and emitted relationships have independent budgets and exact exhaustion flags.
#[test]
fn hook_budgets_charge_scans_and_distinct_pairs() {
    let hooks = population();
    let config = config_with(&[(Rule::RedundantHook, Severity::Error)]);
    let result = run_all(&[], &[], &hooks, &config, &[], 3, 6);
    assert!(!result.budget_exhausted && !result.hook_scan_exhausted);
    assert_eq!(result.findings.len(), 3);
    assert!(result
        .findings
        .iter()
        .all(|f| f.rule == Rule::RedundantHook && f.severity == Severity::Error));
    let scan = run_all(&[], &[], &hooks, &config, &[], 3, 0);
    assert!(scan.hook_scan_exhausted && !scan.budget_exhausted);
    assert!(scan.findings.is_empty());
    let pair = run_all(&[], &[], &hooks, &config, &[], 2, 6);
    assert!(pair.budget_exhausted && !pair.hook_scan_exhausted);
    assert_eq!(pair.findings.len(), 2);
    let off = run_all(
        &[],
        &[],
        &hooks,
        &config_with(&[(Rule::RedundantHook, Severity::Off)]),
        &[],
        0,
        0,
    );
    assert!(off.findings.is_empty() && !off.hook_scan_exhausted && !off.budget_exhausted);
}

/// Opaque and unequal hooks stay in the chain; distinct files cannot form a shared pair.
#[test]
fn hook_barriers_and_file_identity_survive_reporting() {
    let mut hooks = population();
    hooks[0].1.declarations[1].key = None;
    assert!(run_with_hooks(&[], &[], &hooks, &config_with(&[]), &[])
        .findings
        .is_empty());
    hooks[0].1.declarations[1].key = Some("different".into());
    assert!(run_with_hooks(&[], &[], &hooks, &config_with(&[]), &[])
        .findings
        .is_empty());
    hooks = population();
    hooks.push((PathBuf::from("b.test.ts"), hooks[0].1.clone()));
    let expected = run_with_hooks(&[], &[], &hooks, &config_with(&[]), &[]).findings;
    assert_eq!(expected.len(), 6);
    assert!(expected
        .iter()
        .all(|finding| finding.locations[0].path == finding.locations[1].path));
    hooks.reverse();
    assert_eq!(
        run_with_hooks(&[], &[], &hooks, &config_with(&[]), &[]).findings,
        expected
    );
}

/// A different proven outer effect refuses cross-scope pairs, but not same-scope neighbours.
#[test]
fn hook_cross_scope_uniformity_preserves_all_phase_effects() {
    for timing in [HookTiming::BeforeEach, HookTiming::AfterEach] {
        let mut hooks = population();
        for declaration in &mut hooks[0].1.declarations {
            declaration.timing = timing;
        }
        hooks[0].1.declarations[0].key = Some("other".into());
        hooks[0].1.declarations[2].scope = Some(1);
        assert!(run_with_hooks(&[], &[], &hooks, &config_with(&[]), &[])
            .findings
            .is_empty());
        hooks[0].1.chains = vec![vec![1, 2]];
        assert_eq!(
            run_with_hooks(&[], &[], &hooks, &config_with(&[]), &[])
                .findings
                .len(),
            1
        );
        hooks[0].1.chains = vec![vec![0, 1, 2]];
        hooks[0].1.declarations[2].scope = None;
        assert_eq!(
            run_with_hooks(&[], &[], &hooks, &config_with(&[]), &[])
                .findings
                .len(),
            1
        );
    }
}
