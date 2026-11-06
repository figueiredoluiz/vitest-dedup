//! The findings the corpus supports: one rule per comparison claim.
//!
//! A rule reads owned test, suite and hook records. Its equality key is the key
//! the corpus computed, its participants are the cases themselves, and its severity comes from the
//! configuration. A case carrying an exclusion never anchors a finding: the corpus refused to
//! compare it, so no rule may claim it.

use crate::config::Config;
use crate::corpus::TestCase;
use crate::model::{Finding, GateCase, Location, Rule, Severity};
use crate::pairs::{group_and_order, group_outcome, GroupOutcome, PairBudget};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

mod hooks;
mod names;
mod near;
mod parameters;
mod rows;
mod subsumed;
mod suites;

/// The files of one run, as the engine retained them: each path relative to the analysis root,
/// its cases in source order, rows in table order.
pub type Files = [(PathBuf, Vec<TestCase>)];

/// What one pass of the ported rules found.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Hook-chain candidate scanning ran out before every chain was inspected.
    pub hook_scan_exhausted: bool,
    /// Findings in report order.
    pub findings: Vec<Finding>,
    /// The pair budget ran out before every group was processed.
    pub budget_exhausted: bool,
}

/// One compared case, as a rule sees it: the file that spells it and what the corpus keyed.
#[derive(Clone, Copy, Debug)]
struct Entry<'a> {
    path: &'a Path,
    case: &'a TestCase,
}

impl<'a> Entry<'a> {
    /// The runtime case this entry is, as the quality gate counts it; `None` for a case outside
    /// the gate's denominator (a todo a name finding still names), so the numerator never counts
    /// what the denominator leaves out. Two invocations of one template are two runtime cases, so
    /// the identity carries the invocation (`TestCase::invocation`).
    fn gate_case(self) -> Option<GateCase> {
        self.case.gate_eligible().then(|| {
            (
                Location {
                    path: self.case.declaration_path(self.path).to_path_buf(),
                    span: self.case.span,
                },
                self.case.gate_row(),
                self.case.invocation(),
            )
        })
    }

    /// Source identity is independent of the caller that happens to register this execution,
    /// except that two invocations of one template are two runtime cases: the invocation
    /// discriminates them -- by caller file and the chain of call positions, since a byte offset
    /// is only unique within its own module -- so equal-argument calls pair as the duplicates
    /// they are.
    fn declaration(self) -> (&'a Path, u32, &'a Path, &'a [u32]) {
        (
            self.case.declaration_path(self.path),
            self.case.span.start_byte,
            self.case
                .origin
                .as_ref()
                .and_then(|origin| origin.registration_files.first())
                .map_or(self.path, |path| path.as_path()),
            self.case
                .origin
                .as_ref()
                .map_or(&[][..], |origin| origin.call.as_slice()),
        )
    }
}

impl PartialEq for Entry<'_> {
    /// Derived from [`Entry::cmp`].
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Entry<'_> {}

impl PartialOrd for Entry<'_> {
    /// Derived from [`Entry::cmp`].
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry<'_> {
    /// By file, then the case span, then the row: where the case is, never when it was read.
    fn cmp(&self, other: &Self) -> Ordering {
        self.declaration()
            .cmp(&other.declaration())
            .then_with(|| self.case.span.cmp(&other.case.span))
            .then_with(|| self.case.gate_row().cmp(&other.case.gate_row()))
            .then_with(|| self.path.cmp(other.path))
    }
}

/// Runs the ported rules over `files`. The order the files were read in does not reach the
/// output: grouping orders by content, and the findings are sorted as every reporter expects.
pub fn run(files: &Files, config: &Config) -> Outcome {
    run_with_budget(files, config, crate::analysis::MAX_PAIR_COMPARISONS)
}

/// [`run`] with the pair budget given, so a test can drive exhaustion without a corpus that
/// reaches the real cap.
pub fn run_with_budget(files: &Files, config: &Config, max_pair_comparisons: u64) -> Outcome {
    run_with_suites_and_budget(files, &[], config, &[], max_pair_comparisons)
}

/// Runs suite and test rules together, preserving inline-suppressed suites' child findings.
pub fn run_with_suites(
    files: &Files,
    suites: &Files,
    config: &Config,
    suppressions: &[crate::modes::InlineSuppression],
) -> Outcome {
    run_with_suites_and_budget(
        files,
        suites,
        config,
        suppressions,
        crate::analysis::MAX_PAIR_COMPARISONS,
    )
}

/// Shared bounded pass; suite containers never enter the executable-test rules.
fn run_with_suites_and_budget(
    files: &Files,
    suites: &Files,
    config: &Config,
    suppressions: &[crate::modes::InlineSuppression],
    max_pair_comparisons: u64,
) -> Outcome {
    run_all(
        files,
        suites,
        &[],
        config,
        suppressions,
        max_pair_comparisons,
        crate::analysis::MAX_HOOK_PAIR_SCANS,
    )
}

/// Runs all ported rules over the separate test, suite and hook populations.
pub fn run_with_hooks(
    files: &Files,
    suites: &Files,
    hooks: &[(PathBuf, crate::corpus::Hooks)],
    config: &Config,
    suppressions: &[crate::modes::InlineSuppression],
) -> Outcome {
    run_all(
        files,
        suites,
        hooks,
        config,
        suppressions,
        crate::analysis::MAX_PAIR_COMPARISONS,
        crate::analysis::MAX_HOOK_PAIR_SCANS,
    )
}

/// Shares the finding budget while bounding hook candidate scans separately.
fn run_all(
    files: &Files,
    suites: &Files,
    hooks: &[(PathBuf, crate::corpus::Hooks)],
    config: &Config,
    suppressions: &[crate::modes::InlineSuppression],
    max_pair_comparisons: u64,
    max_hook_scans: u64,
) -> Outcome {
    let mut findings = Vec::new();
    let mut budget = PairBudget::new(max_pair_comparisons);
    let (twins, mut budget_exhausted) = suites::run(
        suites,
        files,
        config,
        suppressions,
        &mut budget,
        &mut findings,
    );

    let duplicate_severity = config.severity(Rule::DuplicateTest);
    if duplicate_severity != Severity::Off {
        // Grouped by file and key: the corpus's same-file key says nothing about which file a
        // case is in, and a relationship across files is `cross-file-duplicate`'s claim.
        let (entries, groups) = keyed(files, |entry| {
            entry
                .case
                .exclusion
                .is_none()
                .then_some((entry.path, entry.case.key))
        });
        let groups: Vec<Reduced> = groups
            .into_iter()
            .map(|group| per_declaration(whole(&group), &entries))
            .filter(|group| group.len() >= 2)
            .collect();
        budget_exhausted |= !emit(
            Rule::DuplicateTest,
            duplicate_severity,
            &entries,
            groups,
            duplicate_test_message,
            &mut budget,
            &mut findings,
        );
    }

    budget_exhausted |= !names::run(files, config, &mut budget, &mut findings);
    budget_exhausted |= !rows::run(files, config, &mut budget, &mut findings);
    budget_exhausted |= !rows::run(suites, config, &mut budget, &mut findings);
    rows::cover_suites(&mut findings, files);
    let (parameterized, reported) = parameters::run(files, config, &mut budget, &mut findings);
    budget_exhausted |= !parameterized;
    budget_exhausted |= !near::run(files, config, &reported, &mut budget, &mut findings);

    let cross_file_severity = config.severity(Rule::CrossFileDuplicate);
    if cross_file_severity != Severity::Off {
        let (entries, groups) = keyed(files, |entry| {
            entry
                .case
                .exclusion
                .is_none()
                .then_some(entry.case.module_key)
        });
        let groups: Vec<Reduced> = groups
            .into_iter()
            .map(|group| per_file(whole(&group), &entries))
            .filter(|group| group.len() >= 2)
            .collect();
        budget_exhausted |= !emit(
            Rule::CrossFileDuplicate,
            cross_file_severity,
            &entries,
            groups,
            cross_file_duplicate_message,
            &mut budget,
            &mut findings,
        );
    }

    budget_exhausted |= !subsumed::run(files, config, &mut budget, &mut findings);

    twins.suppress(&mut findings, suppressions);
    let (hook_scan_exhausted, hook_pairs_exhausted) =
        hooks::run(hooks, config, max_hook_scans, &mut budget, &mut findings);
    budget_exhausted |= hook_pairs_exhausted;
    crate::analysis::sort_findings(&mut findings);
    Outcome {
        hook_scan_exhausted,
        findings,
        budget_exhausted,
    }
}

/// Flattens `files` into ordered entries and groups them by `key`, dropping groups smaller than
/// two. Members of a group are ordered by where they are; groups by their first member.
fn keyed<'a, K, KeyFn>(files: &'a Files, key: KeyFn) -> (Vec<Entry<'a>>, Vec<Vec<usize>>)
where
    K: Ord,
    KeyFn: Fn(&Entry<'a>) -> Option<K>,
{
    let entries: Vec<Entry<'a>> = files
        .iter()
        .flat_map(|(path, cases)| cases.iter().map(move |case| Entry { path, case }))
        .collect();
    let groups = group_and_order(&entries, key);
    (entries, groups)
}

/// Keeps at most one member per declaration site: every row of one table the file spells out
/// shares its registering call, so the rows are views of one declaration, not duplicates.
fn one_per_declaration(group: &[usize], entries: &[Entry<'_>]) -> Vec<usize> {
    let mut seen = BTreeSet::new();
    group
        .iter()
        .copied()
        .filter(|&index| seen.insert(entries[index].declaration()))
        .collect()
}

/// Keeps at most one member per exact body: equal bodies are `duplicate-test`'s claim.
fn one_per_body(group: &[usize], entries: &[Entry<'_>]) -> Vec<usize> {
    let mut bodies = BTreeSet::new();
    group
        .iter()
        .copied()
        .filter(|&index| bodies.insert(entries[index].case.body))
        .collect()
}

/// Keeps at most one member per file and per declaration: a relationship inside one file is
/// `duplicate-test`'s claim, whether or not that rule is enabled. With declarations keyed per
/// invocation, a repeated execution across callers is its own member and pairs directly.
fn one_per_file(group: &[usize], entries: &[Entry<'_>]) -> Vec<usize> {
    let mut files = BTreeSet::new();
    let mut declarations = BTreeSet::new();
    group
        .iter()
        .copied()
        .filter(|&index| {
            files.insert(entries[index].path) && declarations.insert(entries[index].declaration())
        })
        .collect()
}

/// A matching group reduced to the entries a finding names: each kept entry with every entry of
/// the original group it stands for, itself included. The named entries are the presentation; what
/// they stand for is what the quality gate counts.
type Reduced = Vec<(usize, Vec<usize>)>;

/// A group in which every entry stands for itself.
fn whole(group: &[usize]) -> Reduced {
    group.iter().map(|&index| (index, vec![index])).collect()
}

/// `group` reduced to the entries `keep` selects; each dropped entry stands with the first kept
/// entry sharing one of its `keys`, so no matched case is lost by the reduction. Indexed, so the
/// work is linear in the group before any budget is charged.
fn reduce<K: Ord>(
    group: Reduced,
    keep: impl Fn(&[usize]) -> Vec<usize>,
    keys: impl Fn(usize) -> Vec<K>,
) -> Reduced {
    let indices: Vec<usize> = group.iter().map(|(index, _)| *index).collect();
    let mut kept: Reduced = keep(&indices)
        .into_iter()
        .map(|index| (index, Vec::new()))
        .collect();
    let own: BTreeMap<usize, usize> = kept
        .iter()
        .enumerate()
        .map(|(slot, (index, _))| (*index, slot))
        .collect();
    let mut by_key: BTreeMap<K, usize> = BTreeMap::new();
    for (slot, (index, _)) in kept.iter().enumerate() {
        for key in keys(*index) {
            by_key.entry(key).or_insert(slot);
        }
    }
    for (index, stands) in group {
        let slot = own
            .get(&index)
            .copied()
            .or_else(|| keys(index).iter().find_map(|key| by_key.get(key).copied()));
        if let Some(slot) = slot {
            kept[slot].1.extend(stands);
        }
    }
    kept
}

/// [`one_per_declaration`] over a reduced group: rows of one table stand with its representative.
fn per_declaration(group: Reduced, entries: &[Entry<'_>]) -> Reduced {
    reduce(
        group,
        |group| one_per_declaration(group, entries),
        |index| vec![entries[index].declaration()],
    )
}

/// [`one_per_body`] over a reduced group: equal bodies stand with the one kept.
fn per_body(group: Reduced, entries: &[Entry<'_>]) -> Reduced {
    reduce(
        group,
        |group| one_per_body(group, entries),
        |index| vec![entries[index].case.body],
    )
}

/// [`one_per_file`] over a reduced group: an entry stands with a kept one of its file or its
/// declaration.
fn per_file(group: Reduced, entries: &[Entry<'_>]) -> Reduced {
    reduce(
        group,
        |group| one_per_file(group, entries),
        // Two kinds of key, kept apart: a file, or a declaration.
        |index| {
            let entry = entries[index];
            vec![(Some(entry.path), None), (None, Some(entry.declaration()))]
        },
    )
}

/// One physical declaration claim, with row identity only for the row rule and caller scope
/// for file-local rules. Cross-file claims aggregate their matching callers instead.
type DeclarationClaim<'a> = (&'a Path, u32, Option<usize>, &'a Path);

/// The declarations `members` stand for: two copied tables pair row by row, and every row pair is
/// the same claim about the same calls, so one finding speaks for them all. Row findings instead
/// retain row indices: each repeated case within the declaration is a separate claim.
fn declarations<'a>(rule: Rule, members: &[Entry<'a>]) -> Vec<DeclarationClaim<'a>> {
    members
        .iter()
        .map(|member| {
            (
                member.case.declaration_path(member.path),
                member.case.span.start_byte,
                (rule == Rule::DuplicateEachRow)
                    .then_some(member.case.gate_row())
                    .flatten(),
                if rule == Rule::CrossFileDuplicate {
                    member.case.declaration_path(member.path)
                } else {
                    member.path
                },
            )
        })
        .collect()
}

/// Emits `rule`'s findings: a spanning set of pair findings per group of up to
/// [`crate::pairs::SPANNING_GROUP_MAX`] members, one cluster finding per larger group, each
/// charged to `budget`
/// before it is written. Returns `false` once the budget is exhausted; the caller stops there.
fn emit(
    rule: Rule,
    severity: Severity,
    entries: &[Entry<'_>],
    groups: Vec<Reduced>,
    message: fn(&[Entry<'_>]) -> String,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    emit_where(
        rule,
        severity,
        entries,
        groups,
        message,
        &|_| true,
        budget,
        findings,
    )
}

/// [`emit`], for only the claims `keep` accepts.
#[allow(clippy::too_many_arguments)]
fn emit_where<'a>(
    rule: Rule,
    severity: Severity,
    entries: &[Entry<'a>],
    groups: Vec<Reduced>,
    message: fn(&[Entry<'_>]) -> String,
    // A trait object, not a generic: one instantiation, so coverage counts every line it runs.
    keep: &dyn Fn(&[Entry<'_>]) -> bool,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    // The index borrows paths; payloads are cloned only once when a finding is finalized.
    let has_origins = entries.iter().any(|entry| entry.case.origin.is_some());
    let mut callers: BTreeMap<_, BTreeSet<&Path>> = BTreeMap::new();
    if has_origins && rule == Rule::CrossFileDuplicate {
        for entry in entries
            .iter()
            .filter(|entry| entry.case.exclusion.is_none())
        {
            let owners = callers
                .entry((entry.declaration(), entry.case.module_key))
                .or_default();
            owners.insert(entry.path);
            if let Some(origin) = &entry.case.origin {
                owners.extend(origin.registration_files.iter().map(PathBuf::as_path));
            }
        }
    }
    let ownership = |members: &[Entry<'a>]| -> Vec<BTreeSet<&'a Path>> {
        if !has_origins {
            return Vec::new();
        }
        members
            .iter()
            .map(|member| {
                if let Some(owners) = callers.get(&(member.declaration(), member.case.module_key)) {
                    return owners.clone();
                }
                let mut owners = BTreeSet::from([member.path]);
                if let Some(origin) = &member.case.origin {
                    owners.extend(origin.registration_files.iter().map(PathBuf::as_path));
                }
                owners
            })
            .collect()
    };
    let mut claimed: BTreeMap<_, (usize, Vec<BTreeSet<&Path>>)> = BTreeMap::new();
    let mut finished = true;
    'groups: for group in groups {
        // One per declaration, row by row for the row rule; what a dropped entry stood for stays
        // with the entry kept in its place.
        let key = |index: usize| {
            let entry = entries[index];
            (
                entry.declaration(),
                (rule == Rule::DuplicateEachRow)
                    .then_some(entry.case.row)
                    .flatten(),
            )
        };
        let reduced = reduce(
            group,
            |group| {
                let mut seen = BTreeSet::new();
                group
                    .iter()
                    .copied()
                    .filter(|&index| seen.insert(key(index)))
                    .collect()
            },
            |index| vec![key(index)],
        );
        let stands: BTreeMap<usize, Vec<usize>> = reduced.iter().cloned().collect();
        let covered = |members: &[usize]| -> Vec<GateCase> {
            members
                .iter()
                .flat_map(|member| &stands[member])
                .filter_map(|&index| entries[index].gate_case())
                .collect()
        };
        let group: Vec<usize> = reduced.iter().map(|(index, _)| *index).collect();
        if group.len() < 2 {
            continue;
        }
        let (candidates, clustered) = match group_outcome(&group) {
            GroupOutcome::Spanning(pairs) => (
                pairs
                    .into_iter()
                    .map(|(left, right)| vec![left, right])
                    .collect::<Vec<_>>(),
                false,
            ),
            GroupOutcome::Cluster(members) => (vec![members], true),
        };
        let mut pending = Vec::new();
        for indices in candidates {
            let members: Vec<_> = indices.iter().map(|&index| entries[index]).collect();
            if !keep(&members) {
                continue;
            }
            let key = (declarations(rule, &members), name_claim(rule, &members));
            let owners = ownership(&members);
            let cases = covered(&indices);
            if let Some((index, known)) = claimed.get_mut(&key) {
                for (known, more) in known.iter_mut().zip(owners) {
                    known.extend(more);
                }
                findings[*index].cases.extend(cases);
            } else {
                pending.push((key, members, owners, cases));
            }
        }
        let cost = if clustered {
            pending.first().map_or(0, |(_, members, _, _)| {
                members.len().saturating_sub(1) as u64
            })
        } else {
            pending.len() as u64
        };
        if !budget.charge(cost) {
            finished = false;
            break 'groups;
        }
        for (key, members, owners, cases) in pending {
            let mut text = message(&members);
            if clustered {
                text.push_str(&crate::analysis::cluster_truncation_note(members.len()));
            }
            let index = findings.len();
            let mut finding = over(rule, severity, &members, text);
            finding.cases.extend(cases);
            findings.push(finding);
            claimed.insert(key, (index, owners));
        }
    }
    for (_, (index, owners)) in claimed {
        findings[index].cases.sort();
        findings[index].cases.dedup();
        if !owners.is_empty() {
            findings[index].set_registration_files(
                owners
                    .into_iter()
                    .map(|paths| paths.into_iter().map(Path::to_path_buf).collect())
                    .collect(),
            );
        }
    }
    finished
}

/// Assembles one finding over `members`, with one location, title and inert entry each.
fn over(rule: Rule, severity: Severity, members: &[Entry<'_>], message: String) -> Finding {
    Finding {
        rule,
        severity,
        message,
        locations: members
            .iter()
            .map(|member| Location {
                path: member.case.declaration_path(member.path).to_path_buf(),
                span: member.case.span,
            })
            .collect(),
        registration_files: if members.iter().any(|member| member.case.origin.is_some()) {
            members
                .iter()
                .map(|member| {
                    member
                        .case
                        .origin
                        .as_ref()
                        .map_or_else(Vec::new, |origin| origin.registration_files.clone())
                })
                .collect()
        } else {
            Vec::new()
        },
        titles: members
            .iter()
            .map(|member| {
                if matches!(rule, Rule::DuplicateTestName | Rule::DuplicateEachRow) {
                    member
                        .case
                        .name
                        .as_ref()
                        .expect("name findings require a proven name")
                        .join(" > ")
                } else {
                    title(member)
                }
            })
            .collect(),
        inert: members.iter().map(|member| member.case.inert).collect(),
        baselined: false,
        covers: None,
        cases: members
            .iter()
            .filter_map(|member| member.gate_case())
            .collect(),
    }
}

/// Distinct rendered names remain distinct claims even when the same two tables spell them.
fn name_claim<'a>(rule: Rule, members: &[Entry<'a>]) -> Option<&'a Vec<String>> {
    (rule == Rule::DuplicateTestName)
        .then(|| members[0].case.name.as_ref())
        .flatten()
}

/// A participant's name: as rendered, else its full title when every segment is spelled, else where
/// it is declared, since a title only known at run time cannot be rendered and a blank name says
/// nothing.
fn title(member: &Entry<'_>) -> String {
    let case = member.case;
    case.name
        .as_ref()
        .map(|name| name.join(" > "))
        .or_else(|| case.full_title.clone())
        .unwrap_or_else(|| format!("<dynamic title at line {}>", case.span.start.line))
}

/// Words a `duplicate-test` finding, calling out any inert (`.skip`/`.todo`) participant by full
/// title so a reader does not mistake "still duplicates" for "these both run".
fn duplicate_test_message(members: &[Entry<'_>]) -> String {
    let mut message = format!(
        "{} tests have identical bodies under an equivalent context",
        members.len()
    );
    let inert: Vec<String> = members
        .iter()
        .filter(|member| member.case.inert)
        .map(|member| crate::lang::text::display(&title(member)).into_owned())
        .collect();
    if !inert.is_empty() {
        message.push_str(&format!(
            "; inert (skip/todo) participant(s), still counted as duplicates: {}",
            inert.join(", ")
        ));
    }
    message
}

/// Words a `cross-file-duplicate` finding, naming how many distinct files the finding spans so a
/// reader is not surprised the participants are not siblings in one file.
fn cross_file_duplicate_message(members: &[Entry<'_>]) -> String {
    let files: BTreeSet<&Path> = members.iter().map(|member| member.path).collect();
    format!(
        "{} tests across {} files have identical bodies under an equivalent context",
        members.len(),
        files.len()
    )
}

#[cfg(test)]
mod tests;
