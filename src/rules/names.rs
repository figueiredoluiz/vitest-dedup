//! Name collisions between distinct declarations in one file.

use super::{emit, keyed, one_per_declaration, Entry, Files};
use crate::config::Config;
use crate::model::{Finding, Rule, Severity};
use crate::pairs::PairBudget;

/// Runs the name rule over the corpus's proven names, preserving every existing exclusion.
pub(super) fn run(
    files: &Files,
    config: &Config,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    let severity = config.severity(Rule::DuplicateTestName);
    if severity == Severity::Off {
        return true;
    }
    let (entries, groups) = keyed(files, |entry| {
        if entry.case.exclusion.is_some() {
            return None;
        }
        entry.case.name.as_ref().map(|name| (entry.path, name))
    });
    let groups = groups
        .into_iter()
        .map(|group| one_per_declaration(&group, &entries))
        .filter(|group| group.len() >= 2)
        .collect();
    emit(
        Rule::DuplicateTestName,
        severity,
        &entries,
        groups,
        message,
        budget,
        findings,
    )
}

/// Describes the name collision independently of callback equivalence.
fn message(members: &[Entry<'_>]) -> String {
    let title = members[0]
        .case
        .name
        .as_ref()
        .map(|name| name.join(" > "))
        .unwrap_or_default();
    format!(
        "{} tests in this file share the full title \"{}\"",
        members.len(),
        title
    )
}
