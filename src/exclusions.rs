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

/// Longest `detail` kept per file, in characters, applied when an entry is recorded and after the
/// analysis root is removed; the full sentence stays on the terminal.
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
            | PackageManifestUnreadable
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
        let mock = test
            .context
            .mocked_modules
            .iter()
            .any(|mock| mock.factory == MockFactory::Unresolved);
        let fixture = test
            .context
            .fixtures
            .iter()
            .any(|fixture| fixture.body == FixtureValue::Unresolved);
        // The explicit causes are independent: a test can have both. The fallback stands for
        // "something else in the context", and only when neither explicit cause applies.
        let own = [
            (mock, ExclusionReason::MockFactoryUnreadable),
            (fixture, ExclusionReason::FixtureUnreadable),
        ];
        let mut found: Vec<ExclusionReason> = own
            .into_iter()
            .filter_map(|(applies, reason)| applies.then_some(reason))
            .collect();
        if found.is_empty() {
            found.push(ExclusionReason::UnresolvedContext);
        }
        for reason in found {
            if !reasons.contains(&reason) {
                reasons.push(reason);
            }
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
    if tests_excluded > 0 {
        // A helper refusal taints every test in its scope, which shows as the generic fallback;
        // the explicit causes (an unreadable mock factory or fixture) are the tests' own and are
        // listed regardless.
        let explained = reasons.iter().any(|(r, _)| explains_excluded_tests(*r));
        for reason in context_reasons(tests) {
            if explained && reason == ExclusionReason::UnresolvedContext {
                continue;
            }
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
        _ => {
            "an ambient fact of the tests (title, `.each` table, config, or a body too deep \
               for the analyzer's recursion cap) could not be resolved"
        }
    }
}

/// `text` with the analysis root removed from every path it mentions, so a detail sentence never
/// carries the checkout's absolute location: `<root>/a/b` reads `a/b`, a bare `<root>` reads `.`.
/// For text built outside the frontend (a skip explanation, an I/O error, a config reason), which
/// can quote a path where the frontend's own sentences never do.
pub fn without_root(text: &str, root: &std::path::Path) -> String {
    strip_root_spellings(text, &root.to_string_lossy(), std::path::MAIN_SEPARATOR)
}

/// [`without_root`] over the spellings of one root: as given and, for a Windows verbatim root
/// (`\\?\C:\work`, `\\?\UNC\host\share`), the ordinary form the same location is also printed in.
/// A root with nothing to strip (`/`, `C:\`) strips nothing.
fn strip_root_spellings(text: &str, root: &str, separator: char) -> String {
    let mut spellings = vec![root.to_owned()];
    if let Some(rest) = root.strip_prefix(r"\\?\UNC\") {
        spellings.push(format!(r"\\{rest}"));
    } else if let Some(rest) = root.strip_prefix(r"\\?\") {
        spellings.push(rest.to_owned());
    }
    let mut stripped = text.to_owned();
    for spelling in &spellings {
        let bare = spelling.trim_end_matches(['/', '\\']);
        let drive_only = bare.len() == 2 && bare.ends_with(':');
        if bare.is_empty() || drive_only {
            continue;
        }
        stripped = strip_at_boundaries(&stripped, bare, separator);
    }
    stripped
}

/// Whether `c` can be part of a path component, for deciding that a match is a whole path prefix.
fn is_path_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '~')
}

/// `text` with each whole-component occurrence of `root` removed (`root/x` reads `x`, a bare
/// `root` reads `.`). An occurrence inside a longer name (`/work/project-two` for `/work/proj`)
/// is another path and is left alone.
fn strip_at_boundaries(text: &str, root: &str, separator: char) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (start, found) in text.match_indices(root) {
        let end = start + found.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        let boundary_before = before.is_none_or(|c| !is_path_char(c));
        let boundary_after = after.is_none_or(|c| c == separator || !is_path_char(c));
        if start < copied || !(boundary_before && boundary_after) {
            continue;
        }
        out.push_str(&text[copied..start]);
        if after == Some(separator) {
            copied = end + separator.len_utf8();
        } else {
            out.push('.');
            copied = end;
        }
    }
    out.push_str(&text[copied..]);
    out
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
        detail,
        tests_excluded: 0,
        tests_total: 0,
    })
}

/// Accumulates entries with bounded memory: the histogram is exact, the list is capped.
#[derive(Debug, Default)]
pub struct ExclusionCollector {
    /// The analysis root removed from every detail as it is recorded; empty strips nothing.
    root: PathBuf,
    entries: Vec<ExcludedFile>,
    tally: BTreeMap<ExclusionReason, (usize, usize)>,
    files: usize,
    tests: usize,
}

impl ExclusionCollector {
    /// A collector for a run over `root`: no recorded detail quotes where the project lives.
    pub fn new(root: &std::path::Path) -> Self {
        Self {
            root: root.to_path_buf(),
            ..Self::default()
        }
    }

    /// Records `file` when there is one; the form `parsed_file` returns.
    pub fn extend(&mut self, file: Option<ExcludedFile>) {
        if let Some(file) = file {
            self.record(file);
        }
    }

    /// Counts `file` and keeps it for the list.
    pub fn record(&mut self, mut file: ExcludedFile) {
        // Redact first, then cut: a cut through the middle of a root would leave a prefix of it.
        file.detail = without_root(&file.detail, &self.root)
            .chars()
            .take(MAX_DETAIL_CHARS)
            .collect();
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

    /// A detail sentence never carries the analysis root: a path under it loses the prefix, the root
    /// alone reads as `.`, and a path elsewhere is untouched.
    #[test]
    fn without_root_removes_the_checkout_location_from_text() {
        let root = std::path::Path::new("/work/proj");
        let sep = std::path::MAIN_SEPARATOR;
        let inside = format!("a matches /work/proj{sep}vitest.config.ts and /work/proj");
        assert_eq!(
            without_root(&inside, root),
            "a matches vitest.config.ts and ."
        );
        assert_eq!(
            without_root("see /elsewhere/x.ts", root),
            "see /elsewhere/x.ts"
        );
        assert_eq!(
            without_root("no paths", std::path::Path::new("")),
            "no paths"
        );
    }

    /// A Windows verbatim root is stripped in both spellings it is printed in.
    #[test]
    fn a_verbatim_root_is_stripped_in_its_ordinary_spelling_too() {
        let root = r"\\?\C:\work\proj";
        let text = r"C:\work\proj\vitest.config.ts and \\?\C:\work\proj\a.ts and C:\work\proj";
        assert_eq!(
            strip_root_spellings(text, root, '\\'),
            "vitest.config.ts and a.ts and ."
        );
        let unc = r"\\?\UNC\host\share\proj";
        assert_eq!(
            strip_root_spellings(r"\\host\share\proj\x.ts", unc, '\\'),
            "x.ts"
        );
    }

    /// Only whole path components are stripped, and a root with nothing to strip strips nothing.
    #[test]
    fn root_stripping_respects_component_boundaries() {
        let sep = '/';
        let strip = |text: &str, root: &str| strip_root_spellings(text, root, sep);
        assert_eq!(
            strip("/work/proj/a.ts and /work/project-two/a.ts", "/work/proj"),
            "a.ts and /work/project-two/a.ts"
        );
        assert_eq!(
            strip("under x/work/proj/a.ts", "/work/proj"),
            "under x/work/proj/a.ts"
        );
        assert_eq!(
            strip("`/work/proj`, (/work/proj/b)", "/work/proj"),
            "`.`, (b)"
        );
        assert_eq!(strip("size/nesting limit", "/"), "size/nesting limit");
        assert_eq!(strip("a/b", ""), "a/b");
        let win = |text: &str, root: &str| strip_root_spellings(text, root, '\\');
        assert_eq!(
            win(
                r"C:\work\project-two\a.ts and C:\work\proj\a.ts",
                r"\\?\C:\work\proj"
            ),
            r"C:\work\project-two\a.ts and a.ts"
        );
        assert_eq!(win(r"C:\work\a.ts", r"C:\"), r"C:\work\a.ts");
        assert_eq!(win(r"C:\work\a.ts", r"\\?\C:\"), r"C:\work\a.ts");
    }

    /// The cap on a detail applies after the root is removed, so a cut cannot leave a prefix of it.
    #[test]
    fn a_cut_detail_never_shows_part_of_the_root() {
        let root = std::path::Path::new("/Users/someone/proj");
        let mut collector = ExclusionCollector::new(root);
        let detail = format!(
            "{}{}/f.ts",
            "a ".repeat(MAX_DETAIL_CHARS / 2 - 4),
            root.display()
        );
        collector.record(unparsed_file(
            PathBuf::from("x.test.ts"),
            ExclusionReason::Unreadable,
            &detail,
        ));
        let (list, _) = collector.finish();
        assert!(!list[0].detail.contains("/Users"), "{}", list[0].detail);
        assert!(list[0].detail.chars().count() <= MAX_DETAIL_CHARS);
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
        let mut collector = ExclusionCollector::default();
        collector.record(unparsed_file(
            PathBuf::from("a"),
            ExclusionReason::Unreadable,
            &long,
        ));
        let (list, _) = collector.finish();
        assert_eq!(list[0].detail.chars().count(), MAX_DETAIL_CHARS);
    }
}
