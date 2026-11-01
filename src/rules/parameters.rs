//! Same structure with different literal inputs: retain every case in one table.

use super::{emit, keyed, Entry, Files};
use crate::config::Config;
use crate::model::{Finding, Rule, Severity};
use crate::pairs::PairBudget;
use std::collections::{BTreeMap, BTreeSet};

/// Groups eligible shapes within one file using all rows to select distinct representatives.
pub(super) fn run(
    files: &Files,
    config: &Config,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    let severity = config.severity(Rule::ParameterizationCandidate);
    if severity == Severity::Off {
        return true;
    }
    let (entries, groups) = keyed(files, |entry| {
        let case = entry.case;
        case.exclusion.is_none().then_some(())?;
        Some((entry.path, case.parameterization_key?))
    });
    let groups = groups
        .iter()
        .map(|group| representatives(group, &entries))
        .filter(|group| group.len() >= 2)
        .collect();
    emit(
        Rule::ParameterizationCandidate,
        severity,
        &entries,
        groups,
        message,
        budget,
        findings,
    )
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
