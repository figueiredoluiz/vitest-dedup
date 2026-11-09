//! Repeated cases within a single directly read table.

use super::{emit, keyed, whole, Entry, Files};
use crate::config::Config;
use crate::model::{Finding, GateCase, Rule, Severity};
use crate::pairs::PairBudget;
use std::collections::BTreeMap;

#[cfg(test)]
thread_local! {
    /// Gate cases materialized while charging suite-row claims on this test thread.
    pub(super) static COVER_EXPANSIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

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
            entry.declaration(),
            &case.suite_rows,
            case.row_class?,
            case.name.as_ref()?,
            case.key,
        ))
    });
    emit(
        Rule::DuplicateEachRow,
        severity,
        &entries,
        groups.iter().map(|group| whole(group)).collect(),
        message,
        budget,
        findings,
    )
}

/// A suite-row claim charges its actual child cases, never its non-executable suite declaration.
pub(super) fn cover_suites(findings: &mut [Finding], files: &Files) {
    // Only the suite rows a row finding names are indexed: the rest have nothing to charge.
    let claimed: std::collections::BTreeSet<(std::path::PathBuf, u32, usize)> = findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateEachRow)
        .flat_map(|f| f.cases.iter())
        .filter_map(|(location, row, ..)| {
            row.map(|row| (location.path.clone(), location.span.start_byte, row))
        })
        .collect();
    let mut children = BTreeMap::new();
    for (path, cases) in files {
        for case in cases.iter().filter(|c| c.gate_eligible()) {
            let owner = case.declaration_path(path);
            for &(start, row) in &case.suite_rows {
                if !claimed.contains(&(owner.to_path_buf(), start, row)) {
                    continue;
                }
                #[cfg(test)]
                COVER_EXPANSIONS.with(|count| count.set(count.get() + 1));
                children
                    .entry((owner, start, row))
                    .or_insert_with(Vec::new)
                    .push(case.gate_case(crate::model::Location {
                        path: owner.to_path_buf(),
                        span: case.span,
                    }));
            }
        }
    }
    for finding in findings
        .iter_mut()
        .filter(|f| f.rule == Rule::DuplicateEachRow)
    {
        let mut covered: Vec<GateCase> = Vec::new();
        let mut suite_claim = false;
        for (location, row, ..) in finding
            .cases
            .iter()
            .filter_map(|(location, row, ..)| row.map(|row| (location, row)))
        {
            if let Some(cases) =
                children.get(&(location.path.as_path(), location.span.start_byte, row))
            {
                suite_claim = true;
                covered.extend(cases.iter().cloned());
            }
        }
        if suite_claim {
            covered.sort();
            covered.dedup();
            finding.cases = covered;
        }
    }
}

/// Names the equality claim without implying that other declarations are involved.
fn message(members: &[Entry<'_>]) -> String {
    format!("{} rows of one parameterized test have identical rendered titles and equivalent inputs and bodies", members.len())
}
