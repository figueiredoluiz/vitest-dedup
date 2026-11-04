//! Whole-suite copies and the child findings already covered by them.

use super::{emit, keyed, one_per_declaration, Entry, Files};
use crate::config::Config;
use crate::model::{Covers, Finding, GateCase, Location, Rule, Severity};
use crate::modes::{apply_inline_suppressions, InlineSuppression};
use crate::pairs::PairBudget;
use std::collections::{BTreeMap, BTreeSet};

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

    /// The copies `locations` sit in, when every one sits in a reported copy.
    fn copies(&self, locations: &[Location]) -> Option<Vec<&Location>> {
        locations
            .iter()
            .map(|location| {
                let (group, index) = self.scope(location)?;
                Some(&self.0[group][index])
            })
            .collect()
    }

    /// Removes a child test/name claim only when one reported suite finding names every copy it
    /// spans: that finding stands for the relationship and carries it to the gate, so baselining
    /// or suppressing either one acts on both. A child no single suite finding stands for stays
    /// visible. A child the user suppressed inline would not have counted, so nothing carries it.
    pub(super) fn suppress(&self, findings: &mut Vec<Finding>, suppressions: &[InlineSuppression]) {
        // Suite findings by the copies they name, so a child's representatives are found from its
        // first copy instead of by scanning every suite finding.
        let mut by_copy: BTreeMap<&Location, Vec<usize>> = BTreeMap::new();
        for (index, finding) in findings.iter().enumerate() {
            if finding.rule == Rule::DuplicateSuite {
                for copy in &finding.locations {
                    by_copy.entry(copy).or_default().push(index);
                }
            }
        }
        // Each child's copies are computed once.
        let represented: Vec<(usize, Vec<usize>)> = findings
            .iter()
            .enumerate()
            .filter(|(_, finding)| {
                matches!(finding.rule, Rule::DuplicateTest | Rule::DuplicateTestName)
                    && self.covered(&finding.locations)
            })
            .filter_map(|(index, child)| {
                let copies = self.copies(&child.locations)?;
                let suites: Vec<usize> = by_copy
                    .get(copies.first()?)?
                    .iter()
                    .copied()
                    .filter(|&suite| {
                        copies
                            .iter()
                            .all(|copy| findings[suite].locations.contains(copy))
                    })
                    .collect();
                (!suites.is_empty()).then_some((index, suites))
            })
            .collect();
        for (child, suites) in &represented {
            let child = findings[*child].clone();
            if apply_inline_suppressions(&mut vec![child.clone()], suppressions) > 0 {
                continue;
            }
            for &suite in suites {
                carry(&mut findings[suite], child.severity, child.cases());
            }
        }
        // An outer copy also stands for the tests a suite finding nested in its copies carries, so
        // nested copies count each test once rather than adding the outer blocks as tests.
        let mut by_file: BTreeMap<&std::path::Path, Vec<usize>> = BTreeMap::new();
        for (index, finding) in findings.iter().enumerate() {
            if finding.rule == Rule::DuplicateSuite {
                if let Some(first) = finding.locations.first() {
                    by_file.entry(&first.path).or_default().push(index);
                }
            }
        }
        let mut nested = Vec::new();
        for suites in by_file.values() {
            for &outer in suites {
                for &inner in suites {
                    let Some(covers) = &findings[inner].covers else {
                        continue;
                    };
                    let inside = findings[inner].locations.iter().all(|location| {
                        findings[outer].locations.iter().any(|copy| {
                            copy.path == location.path
                                && copy.span.start_byte < location.span.start_byte
                                && location.span.end_byte <= copy.span.end_byte
                        })
                    });
                    if inside {
                        nested.push((outer, covers.clone()));
                    }
                }
            }
        }
        for (outer, covers) in nested {
            carry(&mut findings[outer], covers.severity, covers.cases);
        }
        let removed: BTreeSet<usize> = represented.iter().map(|(child, _)| *child).collect();
        let mut index = 0;
        findings.retain(|_| {
            index += 1;
            !removed.contains(&(index - 1))
        });
    }
}

/// Folds `cases` at `severity` into `suite`'s covers, keeping only the highest severity's. The
/// reported locations stay one per declaration; the cases keep each row.
pub(super) fn carry(suite: &mut Finding, severity: Severity, cases: Vec<GateCase>) {
    let covers = suite.covers.get_or_insert_with(|| Covers {
        severity,
        locations: Vec::new(),
        cases: Vec::new(),
    });
    if severity > covers.severity {
        covers.severity = severity;
        covers.locations.clear();
        covers.cases.clear();
    }
    if severity == covers.severity {
        covers.cases.extend(cases);
        covers.cases.sort();
        covers.cases.dedup();
        covers.locations = covers
            .cases
            .iter()
            .map(|(location, _)| location.clone())
            .collect();
        covers.locations.dedup();
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
