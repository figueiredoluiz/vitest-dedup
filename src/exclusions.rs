//! Collecting every file excluded from comparison, with a stable reason code.
//!
//! The terminal names at most ten wholly excluded files; the machine reports list all of them
//! (up to [`MAX_EXCLUDED_FILES`]) and the histogram counts every one. Reasons are created where a
//! refusal happens ([`ExclusionReason`]), never recovered from sentences.

use crate::analysis::is_executable_declaration;
use crate::model::{
    ExcludedFile, ExclusionReason, ExclusionSummary, ExclusionTally, FixtureValue, MockFactory,
    TestCase,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Most files listed in the `json` and `jsonl` reports. Past it the lowest-ranked files (fewest
/// excluded tests, then latest path) are left out of the list but still counted in the summary,
/// and `exclusions.omitted` says how many.
pub const MAX_EXCLUDED_FILES: usize = 10_000;

/// Longest `detail` kept per file, in characters; the full sentence stays on the terminal.
const MAX_DETAIL_CHARS: usize = 500;

/// Whether `reason` is recorded by a helper refusal, which taints every test in its scope and so
/// already explains the file's excluded tests.
fn explains_excluded_tests(reason: ExclusionReason) -> bool {
    use ExclusionReason::*;
    matches!(
        reason,
        HelperExportUnreadable
            | HelperSetupEffects
            | HelperEffectsUnproved
            | HelperLimit
            | HelperSourcesUnavailable
            | RunnerMocked
            | UnsupportedExpression
    )
}

/// Reasons the excluded executable tests of one file carry in their own context, first seen first.
fn context_reasons(tests: &[TestCase]) -> Vec<ExclusionReason> {
    let mut reasons = Vec::new();
    for test in tests
        .iter()
        .filter(|test| is_executable_declaration(test.kind) && !test.context.is_comparable())
    {
        let reason = if test
            .context
            .mocked_modules
            .iter()
            .any(|mock| mock.factory == MockFactory::Unresolved)
        {
            ExclusionReason::MockFactoryUnreadable
        } else if test
            .context
            .fixtures
            .iter()
            .any(|fixture| fixture.body == FixtureValue::Unresolved)
        {
            ExclusionReason::FixtureUnreadable
        } else {
            ExclusionReason::UnresolvedContext
        };
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
    }
    reasons
}

/// Executable tests of one file and how many of them were excluded.
pub fn count_excluded(tests: &[TestCase]) -> (usize, usize) {
    let executable = tests
        .iter()
        .filter(|test| is_executable_declaration(test.kind));
    executable.fold((0, 0), |(excluded, total), test| {
        (
            excluded + usize::from(!test.context.is_comparable()),
            total + 1,
        )
    })
}

/// Builds the entry for a file that was parsed, or `None` when nothing was excluded or incomplete.
///
/// `extraction_reasons` pairs each frontend reason with its sentence, in the order recorded; they
/// come first because they explain the taint. Reasons derived from the tests' own contexts follow,
/// except where a helper refusal already accounts for the excluded tests.
pub fn parsed_file(
    path: PathBuf,
    tests: &[TestCase],
    extraction_reasons: Vec<(ExclusionReason, String)>,
) -> Option<ExcludedFile> {
    let (tests_excluded, tests_total) = count_excluded(tests);
    let mut reasons = extraction_reasons;
    // An import refused for leaving the analysis root is the cause of the helper refusal that
    // follows it, so it leads. Stable: every other order is the order recorded.
    reasons.sort_by_key(|(reason, _)| *reason != ExclusionReason::ImportOutsideRoot);
    if tests_excluded > 0 && !reasons.iter().any(|(r, _)| explains_excluded_tests(*r)) {
        for reason in context_reasons(tests) {
            reasons.push((reason, derived_detail(reason).to_owned()));
        }
    }
    let mut file = file_from_reasons(path, reasons)?;
    file.tests_excluded = tests_excluded;
    file.tests_total = tests_total;
    Some(file)
}

/// The sentence for a reason derived from test contexts rather than recorded by the frontend.
fn derived_detail(reason: ExclusionReason) -> &'static str {
    match reason {
        ExclusionReason::MockFactoryUnreadable => {
            "a `vi.mock` factory could not be read, so the tests it governs were excluded"
        }
        ExclusionReason::FixtureUnreadable => {
            "a `test.extend` fixture value could not be read, so the tests it governs were excluded"
        }
        _ => "an ambient fact of the tests (title, `.each` table or config) could not be resolved",
    }
}

/// An entry for a file that was never parsed (or only partly considered), with no test counts.
pub fn unparsed_file(path: PathBuf, reason: ExclusionReason, detail: &str) -> ExcludedFile {
    file_from_reasons(path, vec![(reason, detail.to_owned())])
        .expect("one reason always yields an entry")
}

/// Primary reason is the first listed; the rest are distinct and in code order.
fn file_from_reasons(
    path: PathBuf,
    reasons: Vec<(ExclusionReason, String)>,
) -> Option<ExcludedFile> {
    let (reason, detail) = reasons.first()?.clone();
    let mut other_reasons: Vec<ExclusionReason> = reasons
        .iter()
        .map(|(reason, _)| *reason)
        .filter(|other| *other != reason)
        .collect();
    other_reasons.sort();
    other_reasons.dedup();
    Some(ExcludedFile {
        path,
        reason,
        other_reasons,
        detail: detail.chars().take(MAX_DETAIL_CHARS).collect(),
        tests_excluded: 0,
        tests_total: 0,
    })
}

/// Accumulates entries with bounded memory: the histogram is exact, the list is capped.
#[derive(Debug, Default)]
pub struct ExclusionCollector {
    entries: Vec<ExcludedFile>,
    tally: BTreeMap<ExclusionReason, (usize, usize)>,
    files: usize,
    tests: usize,
}

impl ExclusionCollector {
    /// Records `file` when there is one; the form `parsed_file` returns.
    pub fn extend(&mut self, file: Option<ExcludedFile>) {
        if let Some(file) = file {
            self.record(file);
        }
    }

    /// Counts `file` and keeps it for the list.
    pub fn record(&mut self, file: ExcludedFile) {
        let entry = self.tally.entry(file.reason).or_default();
        entry.0 += 1;
        entry.1 += file.tests_excluded;
        self.files += 1;
        self.tests += file.tests_excluded;
        self.entries.push(file);
        if self.entries.len() >= 2 * MAX_EXCLUDED_FILES {
            self.prune();
        }
    }

    /// Keeps the highest-ranked [`MAX_EXCLUDED_FILES`] entries: most excluded tests, then path.
    /// Independent of when it runs, so the kept set is deterministic.
    fn prune(&mut self) {
        if self.entries.len() > MAX_EXCLUDED_FILES {
            self.entries.sort_by(|a, b| {
                b.tests_excluded
                    .cmp(&a.tests_excluded)
                    .then_with(|| a.path.cmp(&b.path))
            });
            self.entries.truncate(MAX_EXCLUDED_FILES);
        }
    }

    /// The list sorted by path and the summary over every recorded file.
    pub fn finish(mut self) -> (Vec<ExcludedFile>, ExclusionSummary) {
        self.prune();
        self.entries.sort_by(|a, b| a.path.cmp(&b.path));
        let mut reasons: Vec<ExclusionTally> = self
            .tally
            .into_iter()
            .map(|(reason, (files, tests))| ExclusionTally {
                reason,
                files,
                tests,
            })
            .collect();
        reasons.sort_by(|a, b| {
            b.files
                .cmp(&a.files)
                .then_with(|| b.tests.cmp(&a.tests))
                .then_with(|| a.reason.cmp(&b.reason))
        });
        let summary = ExclusionSummary {
            files: self.files,
            tests: self.tests,
            omitted: self.files - self.entries.len(),
            reasons,
        };
        (self.entries, summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file entry with `tests` excluded tests under `reason`.
    fn entry(path: &str, reason: ExclusionReason, tests: usize) -> ExcludedFile {
        let mut file = unparsed_file(PathBuf::from(path), reason, "detail");
        file.tests_excluded = tests;
        file.tests_total = tests;
        file
    }

    /// The code spelling is the serde spelling, for every variant.
    #[test]
    fn code_matches_the_serialized_spelling_for_every_reason() {
        for reason in ExclusionReason::ALL {
            assert_eq!(
                serde_json::to_value(reason).unwrap(),
                serde_json::json!(reason.code())
            );
        }
        let mut sorted = ExclusionReason::ALL;
        sorted.sort();
        assert_eq!(sorted, ExclusionReason::ALL, "ALL is in tie-break order");
        let mut codes: Vec<_> = ExclusionReason::ALL.map(ExclusionReason::code).to_vec();
        codes.dedup();
        assert_eq!(codes.len(), ExclusionReason::ALL.len());
    }

    /// Past the cap the list keeps the files with the most excluded tests (then the earliest
    /// paths), the summary still counts every file, and the kept set does not depend on the order
    /// files were recorded in.
    #[test]
    fn the_list_is_capped_deterministically_and_the_summary_counts_every_file() {
        let total = 2 * MAX_EXCLUDED_FILES + 5;
        let make = |order: Vec<usize>| {
            let mut collector = ExclusionCollector::default();
            for index in order {
                // One heavy file per ten; the rest have one test each.
                let tests = if index % 10 == 0 { 3 } else { 1 };
                collector.record(entry(
                    &format!("f{index:06}.test.ts"),
                    ExclusionReason::UnresolvedContext,
                    tests,
                ));
            }
            collector.finish()
        };
        let forward = make((0..total).collect());
        let backward = make((0..total).rev().collect());
        assert_eq!(forward.0, backward.0);
        let (list, summary) = forward;
        assert_eq!(list.len(), MAX_EXCLUDED_FILES);
        assert_eq!(summary.files, total);
        assert_eq!(summary.omitted, total - MAX_EXCLUDED_FILES);
        let heavy = (0..total).filter(|i| i % 10 == 0).count();
        assert_eq!(summary.tests, heavy * 3 + (total - heavy));
        assert_eq!(summary.reasons.len(), 1);
        assert_eq!(summary.reasons[0].files, total);
        assert!(list.windows(2).all(|pair| pair[0].path < pair[1].path));
        assert!(
            list.iter().filter(|f| f.tests_excluded == 3).count() == heavy,
            "every heavy file outranks a light one and is kept"
        );
    }

    /// Reasons are tallied by primary reason: most files, then most tests, then code order.
    #[test]
    fn the_histogram_orders_by_files_then_tests_then_code() {
        let mut collector = ExclusionCollector::default();
        for (path, reason, tests) in [
            ("a", ExclusionReason::ParseError, 0),
            ("b", ExclusionReason::HelperLimit, 5),
            ("c", ExclusionReason::HelperLimit, 1),
            ("d", ExclusionReason::RunnerMocked, 9),
            ("e", ExclusionReason::Unreadable, 0),
        ] {
            collector.record(entry(path, reason, tests));
        }
        let (_, summary) = collector.finish();
        let order: Vec<_> = summary
            .reasons
            .iter()
            .map(|t| (t.reason, t.files, t.tests))
            .collect();
        assert_eq!(
            order,
            vec![
                (ExclusionReason::HelperLimit, 2, 6),
                (ExclusionReason::RunnerMocked, 1, 9),
                (ExclusionReason::ParseError, 1, 0),
                (ExclusionReason::Unreadable, 1, 0),
            ]
        );
    }

    /// The first reason is primary, the others are distinct and in code order, and an import that
    /// left the root leads the helper refusal it caused.
    #[test]
    fn primary_is_the_first_reason_and_the_others_are_sorted_and_distinct() {
        let reasons = vec![
            (ExclusionReason::UnsupportedStatement, "with".to_owned()),
            (ExclusionReason::HelperLimit, "limit".to_owned()),
            (ExclusionReason::ParseError, "parse".to_owned()),
            (ExclusionReason::HelperLimit, "limit again".to_owned()),
        ];
        let file = parsed_file(PathBuf::from("x.test.ts"), &[], reasons).unwrap();
        assert_eq!(file.reason, ExclusionReason::UnsupportedStatement);
        assert_eq!(file.detail, "with");
        assert_eq!(
            file.other_reasons,
            vec![ExclusionReason::HelperLimit, ExclusionReason::ParseError]
        );
        let led = parsed_file(
            PathBuf::from("y.test.ts"),
            &[],
            vec![
                (ExclusionReason::HelperExportUnreadable, "helper".to_owned()),
                (ExclusionReason::ImportOutsideRoot, "outside".to_owned()),
            ],
        )
        .unwrap();
        assert_eq!(led.reason, ExclusionReason::ImportOutsideRoot);
        assert!(parsed_file(PathBuf::from("z.test.ts"), &[], Vec::new()).is_none());
    }

    /// A detail past the bound is cut, so a hostile sentence cannot grow a report without limit.
    #[test]
    fn detail_is_bounded() {
        let long = "x".repeat(MAX_DETAIL_CHARS * 3);
        let file = unparsed_file(PathBuf::from("a"), ExclusionReason::Unreadable, &long);
        assert_eq!(file.detail.chars().count(), MAX_DETAIL_CHARS);
    }
}
