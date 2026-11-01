//! The findings the corpus supports: one rule per equality claim.
//!
//! A rule reads owned test, suite and hook records. Its equality key is the key
//! the corpus computed, its participants are the cases themselves, and its severity comes from the
//! configuration. A case carrying an exclusion never anchors a finding: the corpus refused to
//! compare it, so no rule may claim it.

use crate::config::Config;
use crate::corpus::TestCase;
use crate::model::{Finding, Location, Rule, Severity};
use crate::pairs::{group_and_order, group_outcome, GroupOutcome, PairBudget};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

mod hooks;
mod names;
mod parameters;
mod rows;
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
        self.path
            .cmp(other.path)
            .then_with(|| self.case.span.cmp(&other.case.span))
            .then_with(|| self.case.row.cmp(&other.case.row))
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
    let (twins, mut budget_exhausted) =
        suites::run(suites, config, suppressions, &mut budget, &mut findings);

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
        let groups: Vec<Vec<usize>> = groups
            .into_iter()
            .map(|group| one_per_declaration(&group, &entries))
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
    budget_exhausted |= !parameters::run(files, config, &mut budget, &mut findings);

    let cross_file_severity = config.severity(Rule::CrossFileDuplicate);
    if cross_file_severity != Severity::Off {
        let (entries, groups) = keyed(files, |entry| {
            entry
                .case
                .exclusion
                .is_none()
                .then_some(entry.case.module_key)
        });
        let groups: Vec<Vec<usize>> = groups
            .into_iter()
            .map(|group| one_per_file(&group, &entries))
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

    twins.suppress(&mut findings);
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
        .filter(|&index| seen.insert((entries[index].path, entries[index].case.span.start_byte)))
        .collect()
}

/// Keeps at most one member per file: a relationship inside one file is `duplicate-test`'s claim,
/// whether or not that rule is enabled.
fn one_per_file(group: &[usize], entries: &[Entry<'_>]) -> Vec<usize> {
    let mut files = BTreeSet::new();
    group
        .iter()
        .copied()
        .filter(|&index| files.insert(entries[index].path))
        .collect()
}

/// The declarations `members` stand for: two copied tables pair row by row, and every row pair is
/// the same claim about the same calls, so one finding speaks for them all. Row findings instead
/// retain row indices: each repeated case within the declaration is a separate claim.
fn declarations<'a>(rule: Rule, members: &[Entry<'a>]) -> Vec<(&'a Path, u32, Option<usize>)> {
    members
        .iter()
        .map(|member| {
            (
                member.path,
                member.case.span.start_byte,
                (rule == Rule::DuplicateEachRow)
                    .then_some(member.case.row)
                    .flatten(),
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
    groups: Vec<Vec<usize>>,
    message: fn(&[Entry<'_>]) -> String,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    let mut claimed = BTreeSet::new();
    for group in &groups {
        match group_outcome(group) {
            GroupOutcome::Spanning(pairs) => {
                let pairs: Vec<[Entry; 2]> = pairs
                    .into_iter()
                    .map(|(left, right)| [entries[left], entries[right]])
                    .filter(|members| {
                        claimed.insert((declarations(rule, members), name_claim(rule, members)))
                    })
                    .collect();
                if !budget.charge(pairs.len() as u64) {
                    return false;
                }
                for members in pairs {
                    findings.push(over(rule, severity, &members, message(&members)));
                }
            }
            GroupOutcome::Cluster(members) => {
                let members: Vec<Entry> = members.iter().map(|&index| entries[index]).collect();
                if !claimed.insert((declarations(rule, &members), name_claim(rule, &members))) {
                    continue;
                }
                if !budget.charge((members.len() - 1) as u64) {
                    return false;
                }
                let mut text = message(&members);
                text.push_str(&crate::analysis::cluster_truncation_note(members.len()));
                findings.push(over(rule, severity, &members, text));
            }
        }
    }
    true
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
                path: member.path.to_path_buf(),
                span: member.case.span,
            })
            .collect(),
        registration_files: Vec::new(),
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
    }
}

/// Distinct rendered names remain distinct claims even when the same two tables spell them.
fn name_claim<'a>(rule: Rule, members: &[Entry<'a>]) -> Option<&'a Vec<String>> {
    (rule == Rule::DuplicateTestName)
        .then(|| members[0].case.name.as_ref())
        .flatten()
}

/// A participant's name: its full title when every segment is spelled, else where it is declared,
/// since a title only known at run time cannot be rendered and a blank name says nothing.
fn title(member: &Entry<'_>) -> String {
    member
        .case
        .full_title
        .clone()
        .unwrap_or_else(|| format!("<dynamic title at line {}>", member.case.span.start.line))
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
        .map(title)
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
