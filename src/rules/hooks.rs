//! Redundant-hook claims over owned, proven lifecycle chains.

use crate::config::Config;
use crate::corpus::{Hook, Hooks};
use crate::framework::HookTiming;
use crate::model::{Finding, Location, Rule, Severity};
use crate::pairs::PairBudget;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Bounded candidate scanning followed by one finding per distinct declaration pair.
pub(super) fn run(
    files: &[(PathBuf, Hooks)],
    config: &Config,
    max_scans: u64,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> (bool, bool) {
    let severity = config.severity(Rule::RedundantHook);
    if severity == Severity::Off {
        return (false, false);
    }
    let mut scan = PairBudget::new(max_scans);
    let mut pairs = BTreeMap::new();
    let mut exhausted = false;
    let mut ordered: Vec<_> = files.iter().collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0));
    'files: for (path, hooks) in ordered {
        for chain in &hooks.chains {
            let uniform = |timing| {
                let mut keys = chain
                    .iter()
                    .map(|&index| &hooks.declarations[index])
                    .filter(|hook| hook.timing == timing)
                    .map(|hook| &hook.key);
                let first = keys.next();
                keys.all(|key| Some(key) == first)
            };
            let each_uniform = (
                uniform(HookTiming::BeforeEach),
                uniform(HookTiming::AfterEach),
            );
            for (offset, &index) in chain.iter().enumerate() {
                let outer = &hooks.declarations[index];
                for &index in &chain[offset + 1..] {
                    if !scan.charge(1) {
                        exhausted = true;
                        break 'files;
                    }
                    let inner = &hooks.declarations[index];
                    if inner.timing != outer.timing {
                        continue;
                    }
                    // Unknown or different work is a barrier, even if it only observes state.
                    if outer.key.is_none() || outer.key != inner.key {
                        break;
                    }
                    if !same_execution(outer, inner)
                        || outer.span == inner.span
                        || (outer.scope != inner.scope
                            && !match outer.timing {
                                HookTiming::BeforeEach => each_uniform.0,
                                HookTiming::AfterEach => each_uniform.1,
                                _ => false,
                            })
                    {
                        continue;
                    }
                    let location = |hook: &Hook| Location {
                        path: path.clone(),
                        span: hook.span,
                    };
                    pairs.insert((location(outer), location(inner)), outer.label);
                }
            }
        }
    }
    for ((outer, inner), label) in pairs {
        if !budget.charge(1) {
            return (exhausted, true);
        }
        findings.push(Finding {
            rule: Rule::RedundantHook,
            severity,
            message: format!(
                "a {label} repeats idempotent work another {label} in its chain already performs"
            ),
            locations: vec![outer, inner],
            registration_files: Vec::new(),
            titles: vec![label.to_owned(), label.to_owned()],
            inert: vec![false, false],
            baselined: false,
            identity: Default::default(),
        });
    }
    (exhausted, false)
}

/// Once-per-suite phases in distinct scopes may have test execution between them.
fn same_execution(outer: &Hook, inner: &Hook) -> bool {
    outer.scope == inner.scope
        || matches!(outer.timing, HookTiming::BeforeEach | HookTiming::AfterEach)
}
