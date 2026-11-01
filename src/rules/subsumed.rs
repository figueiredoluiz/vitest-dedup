//! Directional, strict ordered event inclusion: an opt-in review suggestion.

use super::{keyed, over, Files};
use crate::config::Config;
use crate::model::{Finding, Rule, Severity};
use crate::pairs::PairBudget;
use std::collections::BTreeSet;

/// Compares every eligible same-context row pair, charging candidate work rather than findings.
/// Rows of one declaration never subsume each other; repeated directional declaration claims
/// share a finding, without discarding other rows before comparing their traces.
pub(super) fn run(
    files: &Files,
    config: &Config,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    let severity = config.severity(Rule::SubsumedTest);
    if severity == Severity::Off {
        return true;
    }
    let (entries, groups) = keyed(files, |entry| {
        let case = entry.case;
        (case.exclusion.is_none() && case.trace_complete && !case.trace.is_empty())
            .then_some((entry.path, case.subsumption_key?))
    });
    let mut claimed = BTreeSet::new();
    for group in groups {
        for left in 0..group.len() {
            for right in left + 1..group.len() {
                if !budget.charge(1) {
                    return false;
                }
                let a = entries[group[left]];
                let b = entries[group[right]];
                if a.case.span.start_byte == b.case.span.start_byte || a.case.body == b.case.body {
                    continue;
                }
                let members = if strict_subsequence(&a.case.trace, &b.case.trace) {
                    [a, b]
                } else if strict_subsequence(&b.case.trace, &a.case.trace) {
                    [b, a]
                } else {
                    continue;
                };
                if !claimed.insert((
                    members[0].path,
                    members[0].case.span.start_byte,
                    members[1].case.span.start_byte,
                )) {
                    continue;
                }
                let message = format!(
                    "one test's {} behavior events are a strict ordered subset of the other's {}; the \
                     smaller test may add no coverage, but only review can confirm the larger test's \
                     extra observations make it worthless",
                    members[0].case.trace.len(), members[1].case.trace.len()
                );
                findings.push(over(Rule::SubsumedTest, severity, &members, message));
            }
        }
    }
    true
}

/// Matches in order with multiplicity, requiring a nonempty and strictly smaller trace.
fn strict_subsequence(small: &[u128], large: &[u128]) -> bool {
    if small.is_empty() || small.len() >= large.len() {
        return false;
    }
    let mut cursor = 0;
    for event in large {
        if cursor < small.len() && small[cursor] == *event {
            cursor += 1;
        }
    }
    cursor == small.len()
}
