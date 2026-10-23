//! Reporters for [`crate::model::AnalysisResult`].
//!
//! Five formats, split by destination. `terminal` and `jsonl` write to stdout; `json`, `html`, and
//! `sarif` write to files. Because `terminal` and `jsonl` both own stdout, selecting both is
//! rejected by the CLI rather than silently interleaving a human report into a machine-parsed
//! stream.
//!
//! Every reporter consumes the extracted IR and writes to any [`std::io::Write`], with no
//! knowledge of stdout or files. The terminal reporter can additionally accept explicit CLI
//! policy metadata; rendering itself never loads configuration or inspects the environment. That
//! decoupling is what lets tests assert on a `Vec<u8>` buffer instead of process output, and
//! what lets the orchestrator redirect a report to a file without either reporter knowing it.

mod html;
mod json;
mod jsonl;
mod sarif;
// `pub(crate)`, not private: `shared::terminal_safe` sanitizes untrusted repository-controlled
// text (filenames, error context) the same way the reporters do, so callers elsewhere in the
// crate (e.g. `main.rs` sanitizing what it prints to stdout/stderr directly) can reuse it instead
// of duplicating the sanitizer.
pub(crate) mod shared;
mod terminal;

pub use html::HtmlReporter;
pub use json::{JsonReporter, JSON_SCHEMA_VERSION};
pub use jsonl::{JsonlReporter, JSONL_SCHEMA_VERSION};
pub use sarif::SarifReporter;
pub use terminal::{DetectionTiming, TerminalReportContext, TerminalReporter};

/// Strips control characters, ANSI escapes, and bidi/invisible formatting from untrusted text.
///
/// Re-exported here rather than left inside the private `shared` module because `main.rs` is a
/// separate binary crate linking this library: `pub(crate)` visibility does not cross that
/// boundary, so the orchestrator could not reuse the sanitizer and would have had to duplicate it.
/// Duplication is the failure mode to avoid — a second copy is a second thing to forget when the
/// hazard list changes.
///
/// It lives on `reporters` because that is where the sanitizing contract is defined
/// (`docs/reports/safety.md`), and a terminal sanitizer hanging off `analysis` would be a surprising
/// place to find it.
pub use shared::terminal_safe;

use crate::model::AnalysisResult;
use std::io::Write;

/// Renders an [`AnalysisResult`] to a writer.
///
/// Implementations must treat `writer` as the only destination — never open a file or print to
/// stdout internally. The orchestrator decides where the bytes go (stdout, a file, an in-memory
/// buffer in a test); a reporter that hardcoded a destination could not be driven from either.
pub trait Reporter {
    /// Writes the complete report for `result` to `writer`.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails or if `writer` itself returns an I/O error.
    fn write_report(&self, result: &AnalysisResult, writer: &mut dyn Write) -> anyhow::Result<()>;
}

/// Support shared by the reporter modules' own test suites: the `Location`/`Finding` builders
/// every format's tests construct IR with, and the `AnalysisResult` base they render. One copy,
/// because five near-identical copies is five places a new `Finding` field lands four times.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::model::{
        AnalysisResult, Finding, IncompleteCauses, Location, Position, Rule, Severity, Span,
    };
    use std::path::PathBuf;

    /// A one-line `Location` whose span runs five columns on one line -- the shape every
    /// reporter test's participants stand at.
    pub(crate) fn location(path: &str, line: u32, column: u32) -> Location {
        Location {
            path: PathBuf::from(path),
            span: Span {
                start: Position { line, column },
                end: Position {
                    line,
                    column: column + 5,
                },
                start_byte: 0,
                end_byte: 5,
            },
        }
    }

    /// A two-participant finding of `rule` in `path`, both sharing one title.
    pub(crate) fn finding_with_title(rule: Rule, path: &str, title: &str) -> Finding {
        Finding {
            registration_files: Vec::new(),
            rule,
            severity: rule.default_severity(),
            message: "identical bodies under an equivalent context".to_string(),
            locations: vec![location(path, 10, 3), location(path, 20, 3)],
            titles: vec![title.to_string(), format!("{title} (copy)")],
            inert: vec![false, false],
            baselined: false,
        }
    }

    /// The two-participant finding every format's `sample_result` is built on: one
    /// `duplicate-test` at Error severity, participants in two different files, distinct titles.
    pub(crate) fn finding_across_files(message: &str) -> Finding {
        Finding {
            registration_files: Vec::new(),
            rule: Rule::DuplicateTest,
            severity: Severity::Error,
            message: message.to_string(),
            locations: vec![
                location("src/a.test.ts", 10, 3),
                location("src/b.test.ts", 20, 5),
            ],
            titles: vec!["suite > a".to_string(), "suite > b".to_string()],
            inert: vec![false, false],
            baselined: false,
        }
    }

    /// The sample result every JSON-carrying format's suite starts from: the cross-file
    /// finding under `message`, four tests over two files.
    pub(crate) fn cross_file_result(message: &str) -> AnalysisResult {
        AnalysisResult {
            findings: vec![finding_across_files(message)],
            tests_analyzed: 4,
            files_analyzed: 2,
            ..AnalysisResult::default()
        }
    }

    /// Extends `result`'s finding with a third participant at `src/c.test.ts:30` -- the input
    /// every "more than two locations" rendering test shares.
    pub(crate) fn append_third_participant(result: &mut AnalysisResult) {
        result.findings[0]
            .locations
            .push(location("src/c.test.ts", 30, 1));
        result.findings[0].titles.push("suite > c".to_string());
    }

    /// An `AnalysisResult` over `findings` -- two analyzed tests in one file, nothing
    /// unresolved, no error-finding pass -- the base every rendering test starts from.
    pub(crate) fn rendered_result(findings: Vec<Finding>) -> AnalysisResult {
        AnalysisResult {
            findings,
            tests_analyzed: 2,
            files_analyzed: 1,
            ..AnalysisResult::default()
        }
    }

    /// Every incompleteness cause fired, each with a distinct value -- the input the
    /// causes-carrying tests share, so a reporter that dropped (or hand-shrank) any one cause
    /// fails against the serde serialization of this fixture rather than against a subset that
    /// happens to match what it emits.
    pub(crate) fn all_fired_causes() -> IncompleteCauses {
        IncompleteCauses {
            discovery_failed: true,
            project_config_degraded: true,
            nothing_read: true,
            files_too_large: 1,
            files_unreadable: 2,
            files_partially_read: 3,
            files_globals_undetermined: 4,
            unresolved_contexts: 5,
            pair_budget_exhausted: true,
            hook_scan_exhausted: true,
            files_over_memory_budget: 6,
            findings_truncated: 7,
            withheld_unchanged: 8,
            package_worker_failures: std::collections::BTreeMap::from([
                (crate::model::PackageWorkerFailure::Start, 10),
                (crate::model::PackageWorkerFailure::Timeout, 11),
                (crate::model::PackageWorkerFailure::Exit, 12),
                (crate::model::PackageWorkerFailure::Output, 13),
                (crate::model::PackageWorkerFailure::Io, 14),
                (crate::model::PackageWorkerFailure::Nested, 15),
            ]),
            missing_prerequisites: vec![crate::model::MissingPrerequisite {
                kind: crate::model::PrerequisiteKind::NodeModules,
                path: "node_modules".to_string(),
                fix: "pnpm install".to_string(),
                unresolved_imports: 9,
            }],
        }
    }
}
