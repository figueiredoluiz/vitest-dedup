//! Name collisions between distinct declarations in one file.

use super::{emit, keyed, per_declaration, whole, Entry, Files};
use crate::config::Config;
use crate::model::{Finding, Rule, Severity};
use crate::pairs::PairBudget;

/// Runs the name rule over the corpus's proven names. A test excluded only for its callback (an
/// empty body, or a todo) still collides by name; every other exclusion is preserved.
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
        if entry
            .case
            .exclusion
            .is_some_and(|exclusion| !exclusion.keeps_name())
        {
            return None;
        }
        entry.case.name.as_ref().map(|name| (entry.path, name))
    });
    let groups = groups
        .into_iter()
        .map(|group| per_declaration(whole(&group), &entries))
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
        crate::lang::text::display(&title)
    )
}
