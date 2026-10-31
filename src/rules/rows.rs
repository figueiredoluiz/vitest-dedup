//! Repeated cases within a single directly read table.

use super::{emit, keyed, Entry, Files};
use crate::config::Config;
use crate::model::{Finding, Rule, Severity};
use crate::pairs::PairBudget;

/// Requires both proven inputs and equivalent callback/context keys, under one rendered name.
pub(super) fn run(
    files: &Files,
    config: &Config,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    let severity = config.severity(Rule::DuplicateEachRow);
    if severity == Severity::Off {
        return true;
    }
    let (entries, groups) = keyed(files, |entry| {
        let case = entry.case;
        if case.exclusion.is_some() || case.row.is_none() {
            return None;
        }
        Some((
            entry.path,
            case.span.start_byte,
            case.row_class?,
            case.name.as_ref()?,
            case.key,
        ))
    });
    emit(
        Rule::DuplicateEachRow,
        severity,
        &entries,
        groups,
        message,
        budget,
        findings,
    )
}

/// Names the equality claim without implying that other declarations are involved.
fn message(members: &[Entry<'_>]) -> String {
    format!("{} rows of one parameterized test have identical rendered titles and equivalent inputs and bodies", members.len())
}
