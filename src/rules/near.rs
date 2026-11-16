//! The same behavior events, spelled differently: a suspicion, never a proof.

use super::parameters::{declaration, Reported};
use super::{emit_where, keyed, per_body, per_declaration, whole, Entry, Files};
use crate::analysis::{near_duplicate_text, MIN_BEHAVIOR_EVENTS_FOR_NEAR_DUPLICATE};
use crate::config::Config;
use crate::model::{Finding, Rule, Severity};
use crate::pairs::PairBudget;

/// Groups cases of one file on the exact multiset of their events, keeping one declaration per
/// exact body. An unfinished trace is no evidence, and a pair inside a group `reported` by
/// parameterization already has the stronger claim and the same remedy.
pub(super) fn run(
    files: &Files,
    config: &Config,
    reported: &Reported<'_>,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    let severity = config.severity(Rule::NearDuplicateTest);
    if severity == Severity::Off {
        return true;
    }
    let (entries, groups) = keyed(files, |entry| {
        let case = entry.case;
        let compared = case.exclusion.is_none()
            && case.trace_complete
            && case.ordinary_events >= MIN_BEHAVIOR_EVENTS_FOR_NEAR_DUPLICATE;
        compared.then(|| {
            let mut events = case.trace.clone();
            events.sort_unstable();
            (entry.path, case.similarity_key, events)
        })
    });
    let groups = groups
        .iter()
        .map(|group| per_body(per_declaration(whole(group), &entries), &entries))
        .filter(|group| group.len() >= 2)
        .collect();
    emit_where(
        Rule::NearDuplicateTest,
        severity,
        &entries,
        groups,
        message,
        &|members| !covered(members, reported),
        budget,
        findings,
    )
    .is_ok()
}

/// Whether one parameterization group reports every one of `members`.
fn covered(members: &[Entry<'_>], reported: &Reported<'_>) -> bool {
    let group =
        |member: &Entry<'_>| declaration(member).and_then(|key| reported.get(&key).copied());
    members.split_first().is_some_and(|(first, rest)| {
        group(first).is_some_and(|id| rest.iter().all(|member| group(member) == Some(id)))
    })
}

/// Says which of the two claims this is: the events run in another order, or a normalization
/// explains the bodies differing.
fn message(members: &[Entry<'_>]) -> String {
    let reordered = members
        .windows(2)
        .any(|pair| pair[0].case.trace != pair[1].case.trace);
    near_duplicate_text(members.len(), reordered)
}
