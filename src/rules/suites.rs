//! Whole-suite copies and the child findings already covered by them.

use super::{emit, keyed, per_declaration, whole, Entry, Files};
use crate::config::Config;
use crate::model::{Covers, Finding, GateCase, Location, Rule, Severity};
use crate::modes::InlineSuppressionIndex;
use crate::pairs::PairBudget;
use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

mod scopes;
use scopes::Scopes;

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty set of copies has no enclosing suite findings.
    #[test]
    fn absent_suite_copies_have_no_enclosing_findings() {
        assert!(
            enclosing_findings(&[], &[], &Scopes::new([]), &mut PairBudget::new(0))
                .unwrap()
                .is_empty()
        );
    }
}

#[cfg(test)]
thread_local! {
    /// File entries traversed while collecting suite cases on this test thread.
    pub(super) static DESCENDANT_FILE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Successfully reported suite groups. No unreported or suppressed claim covers a child.
#[derive(Default)]
pub(super) struct Twins(Vec<Vec<Location>>);

struct SuppressionPlan {
    removed: BTreeSet<usize>,
    covers: Vec<(usize, Covers)>,
}

impl Twins {
    /// Removes a child test/name claim only when one reported suite finding names every copy it
    /// spans: that finding stands for the relationship and carries it to the gate, so baselining
    /// or suppressing either one acts on both. An unfinished plan leaves the claims intact.
    pub(super) fn suppress(
        &self,
        findings: &mut Vec<Finding>,
        suppressions: &InlineSuppressionIndex<'_>,
        work: &mut PairBudget,
    ) -> bool {
        if self.0.is_empty() {
            return true;
        }
        let Ok(SuppressionPlan { removed, covers }) = self.plan(findings, suppressions, work)
        else {
            return false;
        };
        for (index, coverage) in covers {
            findings[index].covers = Some(coverage);
        }
        let mut index = 0;
        findings.retain(|_| {
            index += 1;
            !removed.contains(&(index - 1))
        });
        true
    }

    /// Plans child replacement and propagation before changing any finding or gate coverage.
    fn plan(
        &self,
        findings: &[Finding],
        suppressions: &InlineSuppressionIndex<'_>,
        work: &mut PairBudget,
    ) -> Result<SuppressionPlan, ()> {
        let twins = Scopes::new(self.0.iter().enumerate().flat_map(|(group, locations)| {
            locations.iter().enumerate().map(move |(copy, location)| {
                let span = location.span;
                (location, (span.end_byte - span.start_byte, group, copy))
            })
        }));
        let suite_locations = || {
            findings
                .iter()
                .enumerate()
                .filter(|(_, finding)| finding.rule == Rule::DuplicateSuite)
                .flat_map(|(index, finding)| {
                    finding.locations.iter().map(move |copy| (copy, index))
                })
        };
        let mut by_copy = BTreeMap::<_, Vec<usize>>::new();
        for (copy, index) in suite_locations() {
            by_copy.entry(copy).or_default().push(index);
        }
        let mut removed = BTreeSet::new();
        let mut direct = BTreeMap::<_, Coverage<'_>>::new();
        for (index, child) in findings.iter().enumerate().filter(|(_, child)| {
            matches!(child.rule, Rule::DuplicateTest | Rule::DuplicateTestName)
        }) {
            let copies = self.copies(child, &twins, &by_copy, work)?;
            let Some(first) = copies.iter().min_by_key(|ids| ids.len()) else {
                continue;
            };
            let suppressed = child
                .locations
                .iter()
                .any(|site| suppressions.contains(site));
            for &suite in *first {
                work.charge(copies.len() as u64).then_some(()).ok_or(())?;
                if copies.iter().all(|ids| ids.binary_search(&suite).is_ok()) {
                    removed.insert(index);
                    if !suppressed {
                        direct
                            .entry(suite)
                            .or_default()
                            .add(child.severity, &child.cases, work)?;
                    }
                }
            }
        }
        let scopes = Scopes::new(suite_locations());
        let mut inherited = BTreeMap::<_, Coverage<'_>>::new();
        for (&inner, coverage) in &direct {
            for outer in enclosing_findings(findings, &findings[inner].locations, &scopes, work)? {
                inherited.entry(outer).or_default().add(
                    coverage.severity,
                    coverage.cases.iter().copied(),
                    work,
                )?;
            }
        }
        for (outer, coverage) in inherited {
            direct
                .entry(outer)
                .or_default()
                .add(coverage.severity, coverage.cases, work)?;
        }
        Ok(SuppressionPlan {
            removed,
            covers: direct
                .into_iter()
                .map(|(suite, coverage)| (suite, coverage.finish()))
                .collect(),
        })
    }

    /// Finds one smallest scope per participant, requiring distinct copies of a single group.
    fn copies<'a>(
        &self,
        child: &Finding,
        scopes: &Scopes<'_, (u32, usize, usize)>,
        by_copy: &'a BTreeMap<&Location, Vec<usize>>,
        work: &mut PairBudget,
    ) -> Result<Vec<&'a [usize]>, ()> {
        if child.locations.len() < 2 {
            return Ok(Vec::new());
        }
        let mut selected = BTreeSet::new();
        let mut group_id = None;
        let mut copies = Vec::new();
        for site in &child.locations {
            work.charge(1).then_some(()).ok_or(())?;
            let mut smallest = None;
            scopes.enclosing(site, work, |scope| {
                smallest = Some(smallest.map_or(scope, |prior| scope.min(prior)));
            })?;
            let Some((_, group, copy)) = smallest else {
                return Ok(Vec::new());
            };
            if *group_id.get_or_insert(group) != group || !selected.insert(copy) {
                return Ok(Vec::new());
            }
            let Some(ids) = by_copy.get(&self.0[group][copy]) else {
                return Ok(Vec::new());
            };
            copies.push(ids.as_slice());
        }
        Ok(copies)
    }
}

/// Intersects the enclosing suite findings of every copy, retaining the original file scope.
fn enclosing_findings(
    findings: &[Finding],
    copies: &[Location],
    scopes: &Scopes<'_, usize>,
    work: &mut PairBudget,
) -> Result<BTreeSet<usize>, ()> {
    let Some((first, rest)) = copies.split_first() else {
        return Ok(BTreeSet::new());
    };
    let mut outer = BTreeSet::new();
    scopes.enclosing(first, work, |id| {
        if findings[id].locations[0].path == first.path {
            outer.insert(id);
        }
    })?;
    for site in rest {
        if outer.is_empty() {
            break;
        }
        let mut matching = BTreeSet::new();
        scopes.enclosing(site, work, |id| {
            if outer.contains(&id) {
                matching.insert(id);
            }
        })?;
        outer = matching;
    }
    Ok(outer)
}

/// Collects distinct gate cases at the highest severity, borrowing them until the plan succeeds.
pub(super) struct Coverage<'a> {
    severity: Severity,
    cases: BTreeSet<&'a GateCase>,
}

impl Default for Coverage<'_> {
    /// Starts without any covered case or gate severity.
    fn default() -> Self {
        Self {
            severity: Severity::Off,
            cases: BTreeSet::new(),
        }
    }
}

impl<'a> Coverage<'a> {
    /// Adds a completed source of coverage, charging its case visits before retaining references.
    pub(super) fn add(
        &mut self,
        severity: Severity,
        cases: impl IntoIterator<Item = &'a GateCase, IntoIter: ExactSizeIterator>,
        work: &mut PairBudget,
    ) -> Result<(), ()> {
        if severity >= self.severity {
            let cases = cases.into_iter();
            work.charge(cases.len() as u64 + 1)
                .then_some(())
                .ok_or(())?;
            if severity > self.severity {
                self.severity = severity;
                self.cases.clear();
            }
            self.cases.extend(cases);
        }
        Ok(())
    }

    /// Materializes each distinct case once and derives the matching declaration locations.
    pub(super) fn finish(self) -> Covers {
        let mut locations: Vec<_> = self
            .cases
            .iter()
            .map(|(location, ..)| location.clone())
            .collect();
        locations.dedup();
        Covers {
            severity: self.severity,
            locations,
            cases: self.cases.into_iter().cloned().collect(),
        }
    }
}

/// Emits each copied suite level and records its successfully reported groups.
pub(super) fn run(
    files: &Files,
    tests: &Files,
    config: &Config,
    suppressions: &InlineSuppressionIndex<'_>,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> (Twins, bool) {
    let mut twins = Twins::default();
    let severity = config.severity(Rule::DuplicateSuite);
    if severity == Severity::Off {
        return (twins, false);
    }
    let (entries, groups) = keyed(files, |entry| {
        (!entry.case.inert && entry.case.exclusion.is_none() && entry.case.row.is_none())
            .then(|| Some((entry.path, entry.case.name.as_ref()?, entry.case.key)))
            .flatten()
    });
    // Retain every entry for an equal path; build the borrowed index only for an emitted finding.
    let by_path = OnceCell::new();
    for group in groups {
        let group = per_declaration(whole(&group), &entries);
        if group.len() < 2 {
            continue;
        }
        let locations: Vec<_> = group
            .iter()
            .map(|&(index, _)| Location {
                path: entries[index].path.to_path_buf(),
                span: entries[index].case.span,
            })
            .collect();
        let mut emitted = Vec::new();
        if emit(
            Rule::DuplicateSuite,
            severity,
            &entries,
            vec![group],
            message,
            budget,
            &mut emitted,
        )
        .is_err()
        {
            return (twins, true);
        }
        // A copy stands for the tests declared inside it, whether or not a child rule reports them.
        for finding in &mut emitted {
            let by_path = by_path.get_or_init(|| {
                let mut index: BTreeMap<&Path, Vec<usize>> = BTreeMap::new();
                for (position, (path, _)) in tests.iter().enumerate() {
                    index.entry(path.as_path()).or_default().push(position);
                }
                index
            });
            finding.cases = descendants(finding, tests, by_path);
        }
        // A partially suppressed spanning group cannot stand for every relation in the group.
        if !emitted.is_empty()
            && emitted.iter().all(|finding| {
                !finding
                    .locations
                    .iter()
                    .any(|site| suppressions.contains(site))
            })
        {
            twins.0.push(locations);
        }
        findings.extend(emitted);
    }
    (twins, false)
}

/// The eligible tests declared inside the suite copies `finding` names: what it stands for.
fn descendants(
    finding: &Finding,
    tests: &Files,
    by_path: &BTreeMap<&Path, Vec<usize>>,
) -> Vec<GateCase> {
    let mut cases: Vec<GateCase> = finding
        .locations
        .iter()
        .flat_map(|copy| {
            by_path
                .get(copy.path.as_path())
                .into_iter()
                .flatten()
                .map(|&index| &tests[index])
                .inspect(|_| {
                    #[cfg(test)]
                    DESCENDANT_FILE_VISITS.with(|visits| visits.set(visits.get() + 1));
                })
                .flat_map(move |(path, declared)| {
                    declared
                        .iter()
                        .filter(move |case| {
                            case.origin.is_none()
                                && case.gate_eligible()
                                && copy.span.start_byte <= case.span.start_byte
                                && case.span.end_byte <= copy.span.end_byte
                        })
                        .map(move |case| {
                            case.gate_case(Location {
                                path: path.clone(),
                                span: case.span,
                            })
                        })
                })
        })
        .collect();
    cases.sort();
    cases.dedup();
    cases
}

/// Describes the complete block rather than repeating its test-level findings.
fn message(members: &[Entry<'_>]) -> String {
    format!("{} suites have identical complete bodies under an equivalent context; cross-copy child findings are suppressed", members.len())
}
