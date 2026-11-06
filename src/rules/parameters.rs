//! Same structure with different literal inputs: retain every case in one table.

use super::{emit, keyed, reduce, whole, Entry, Files, Reduced};
use crate::config::Config;
use crate::model::{Finding, Rule, Severity};
use crate::pairs::PairBudget;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One declaration of a group, under the shape that grouped it.
pub(super) type Declaration<'a> = (&'a Path, (&'a Path, u32), u64);

/// The declarations of each group this rule reported.
pub(super) type Reported<'a> = Vec<BTreeSet<Declaration<'a>>>;

/// The declaration `entry` stands for, when it has a shape.
pub(super) fn declaration<'a>(entry: &Entry<'a>) -> Option<Declaration<'a>> {
    let key = entry.case.parameterization_key?;
    Some((entry.path, entry.declaration(), key))
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
    // A representative stands for the other rows of its own declaration.
    let groups: Vec<Reduced> = groups
        .iter()
        .map(|group| {
            reduce(
                whole(group),
                |group| representatives(group, &entries),
                |index| vec![entries[index].declaration()],
            )
        })
        .filter(|group| group.len() >= 2)
        .collect();
    let reported = groups
        .iter()
        .map(|group| {
            group
                .iter()
                .filter_map(|(index, _)| declaration(&entries[*index]))
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
            .entry(entries[index].declaration())
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

/// Suggests a table while explicitly preserving the coverage of every literal case. Benchmarks
/// (only ever grouped with benchmarks) have no `bench.each`, and a `test.each` table would stop
/// measuring them, so their advice is a loop of `bench` calls.
fn message(members: &[Entry<'_>]) -> String {
    if members.iter().all(|member| member.case.benchmark) {
        return format!(
            "{} benchmarks share identical structure but differ only in literal values; register \
             them from one loop of `bench(...)` calls over those values, keeping every case as \
             its own benchmark (there is no `bench.each`, and a `test.each` table would not be \
             measured)",
            members.len()
        );
    }
    format!(
        "{} tests share identical structure but differ only in literal values; consolidate into \
         one `test.each` table that keeps every case as its own row",
        members.len()
    )
}
