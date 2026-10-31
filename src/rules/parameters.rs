//! Same structure with different literal inputs: retain every case in one table.

use super::{emit, keyed, one_per_declaration, Entry, Files};
use crate::config::Config;
use crate::model::{Finding, Rule, Severity};
use crate::pairs::PairBudget;
use std::collections::BTreeSet;

/// Groups eligible shapes within one file, retaining one declaration per exact body.
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
        .map(|group| {
            let mut bodies = BTreeSet::new();
            one_per_declaration(group, &entries)
                .into_iter()
                .filter(|&index| bodies.insert(entries[index].case.body))
                .collect::<Vec<_>>()
        })
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

/// Suggests a table while explicitly preserving the coverage of every literal case.
fn message(members: &[Entry<'_>]) -> String {
    format!(
        "{} tests share identical structure but differ only in literal values; consolidate into \
         one `test.each` table that keeps every case as its own row",
        members.len()
    )
}
