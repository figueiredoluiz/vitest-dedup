//! Same structure with different literal inputs: retain every case in one table.

use super::{emit, keyed, Entry, Files};
use crate::config::Config;
use crate::model::{Finding, Rule, Severity};
use crate::pairs::PairBudget;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One declaration of a group, under the shape that grouped it.
pub(super) type Declaration<'a> = (&'a Path, u32, u64);

/// The declarations of each group this rule reported.
pub(super) type Reported<'a> = Vec<BTreeSet<Declaration<'a>>>;

/// The declaration `entry` stands for, when it has a shape.
pub(super) fn declaration<'a>(entry: &Entry<'a>) -> Option<Declaration<'a>> {
    let key = entry.case.parameterization_key?;
    Some((entry.path, entry.case.span.start_byte, key))
}

/// Groups eligible shapes within one file using all rows to select distinct representatives.
/// Returns whether the budget lasted, and the groups reported.
pub(super) fn run<'a>(
    files: &'a Files,
    config: &Config,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> (bool, Reported<'a>) {
    let severity = config.severity(Rule::ParameterizationCandidate);
    if severity == Severity::Off {
        return (true, Vec::new());
    }
    let (entries, groups) = keyed(files, |entry| {
        let case = entry.case;
        case.exclusion.is_none().then_some(())?;
        Some((entry.path, case.parameterization_key?))
    });
    let groups: Vec<Vec<usize>> = groups
        .iter()
        .map(|group| representatives(group, &entries))
        .filter(|group| group.len() >= 2)
        .collect();
    let reported = groups
        .iter()
        .map(|group| {
            group
                .iter()
                .filter_map(|&index| declaration(&entries[index]))
                .collect()
        })
        .collect();
    let finished = emit(
        Rule::ParameterizationCandidate,
        severity,
        &entries,
        groups,
        message,
        budget,
        findings,
    );
    (finished, reported)
}

/// Covers bodies in stable content order, then reports representatives in source order.
fn representatives(group: &[usize], entries: &[Entry<'_>]) -> Vec<usize> {
    let mut declarations = BTreeMap::<_, BTreeMap<_, _>>::new();
    for &index in group {
        let case = entries[index].case;
        declarations
            .entry(case.span.start_byte)
            .or_default()
            .entry(case.body)
            .or_insert(index);
    }
    let mut declarations: Vec<_> = declarations.into_values().collect();
    declarations.sort_by(|left, right| {
        right
            .len()
            .cmp(&left.len())
            .then_with(|| left.keys().cmp(right.keys()))
    });
    let mut seen = BTreeSet::new();
    let mut members = Vec::new();
    for bodies in declarations {
        let representative = bodies
            .iter()
            .find_map(|(body, &index)| (!seen.contains(body)).then_some(index));
        seen.extend(bodies.into_keys());
        members.extend(representative);
    }
    members.sort_unstable_by_key(|&index| entries[index]);
    members
}

/// Suggests a table while explicitly preserving the coverage of every literal case.
fn message(members: &[Entry<'_>]) -> String {
    format!(
        "{} tests share identical structure but differ only in literal values; consolidate into \
         one `test.each` table that keeps every case as its own row",
        members.len()
    )
}
