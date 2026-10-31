//! Whole-suite copies and the child findings already covered by them.

use super::{emit, keyed, one_per_declaration, Entry, Files};
use crate::config::Config;
use crate::model::{Finding, Location, Rule, Severity};
use crate::modes::{apply_inline_suppressions, InlineSuppression};
use crate::pairs::PairBudget;
use std::collections::BTreeSet;

/// Successfully reported suite groups. No unreported or suppressed claim covers a child.
#[derive(Default)]
pub(super) struct Twins(Vec<Vec<Location>>);

impl Twins {
    /// Finds the smallest reported suite that strictly encloses a participant.
    fn scope(&self, location: &Location) -> Option<(usize, usize)> {
        self.0
            .iter()
            .enumerate()
            .flat_map(|(group, members)| {
                members
                    .iter()
                    .enumerate()
                    .filter_map(move |(index, suite)| {
                        (suite.path == location.path
                            && suite.span.start_byte < location.span.start_byte
                            && location.span.end_byte <= suite.span.end_byte)
                            .then_some((suite.span.end_byte - suite.span.start_byte, group, index))
                    })
            })
            .min()
            .map(|(_, group, index)| (group, index))
    }

    /// A pure cross-copy finding has one participant in each of several copies of one group.
    /// Mixed clusters stay visible: discarding them would also discard outsiders or local copies.
    fn covered(&self, locations: &[Location]) -> bool {
        let scopes: Option<Vec<_>> = locations
            .iter()
            .map(|location| self.scope(location))
            .collect();
        let Some(scopes) = scopes.filter(|scopes| scopes.len() >= 2) else {
            return false;
        };
        scopes.iter().all(|scope| scope.0 == scopes[0].0)
            && scopes
                .iter()
                .map(|scope| scope.1)
                .collect::<BTreeSet<_>>()
                .len()
                == scopes.len()
    }

    /// Removes only child test/name claims whose relationships were all reported as suites.
    pub(super) fn suppress(&self, findings: &mut Vec<Finding>) {
        findings.retain(|finding| {
            !matches!(finding.rule, Rule::DuplicateTest | Rule::DuplicateTestName)
                || !self.covered(&finding.locations)
        });
    }
}

/// Emits each copied suite level and records its successfully reported groups.
pub(super) fn run(
    files: &Files,
    config: &Config,
    suppressions: &[InlineSuppression],
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> (Twins, bool) {
    let mut twins = Twins::default();
    let severity = config.severity(Rule::DuplicateSuite);
    if severity == Severity::Off {
        return (twins, false);
    }
    let (entries, groups) = keyed(files, |entry| {
        (!entry.case.inert && entry.case.exclusion.is_none())
            .then(|| Some((entry.path, entry.case.name.as_ref()?, entry.case.key)))
            .flatten()
    });
    for group in groups {
        let group = one_per_declaration(&group, &entries);
        if group.len() < 2 {
            continue;
        }
        let locations: Vec<_> = group
            .iter()
            .map(|&index| Location {
                path: entries[index].path.to_path_buf(),
                span: entries[index].case.span,
            })
            .collect();
        let mut emitted = Vec::new();
        if !emit(
            Rule::DuplicateSuite,
            severity,
            &entries,
            vec![group],
            message,
            budget,
            &mut emitted,
        ) {
            return (twins, true);
        }
        let mut visible = emitted.clone();
        let removed = apply_inline_suppressions(&mut visible, suppressions);
        // A partially suppressed spanning group cannot stand for every relation in the group.
        if removed == 0 && !visible.is_empty() {
            twins.0.push(locations);
        }
        findings.extend(emitted);
    }
    (twins, false)
}

/// Describes the complete block rather than repeating its test-level findings.
fn message(members: &[Entry<'_>]) -> String {
    format!("{} suites have identical complete bodies under an equivalent context; cross-copy child findings are suppressed", members.len())
}
