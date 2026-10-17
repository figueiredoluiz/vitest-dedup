//! Command-line parsing and validation.
//!
//! This module never runs discovery, parsing, or analysis. It turns `argv` into [`ParsedArgs`],
//! validates flag combinations clap's grammar cannot express on its own (such as "don't put a
//! path before `check`"), and converts flags into
//! [`ConfigOverrides`](crate::config::ConfigOverrides) for `Config::load`. Running the pipeline
//! and turning its outcome into a process exit code is the orchestrator's job -- see [`ExitCode`]
//! for the contract it must honor.

use crate::config::{self, ConfigOverrides};
use crate::model::{Rule, Severity};
use anyhow::{bail, Result};
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Reporter backends selectable via `--reporters`.
///
/// Five reporters exist: `terminal` and `jsonl` (stdout), `json`, `html`, and `sarif` (file). The
/// CLI only advertises reporters that are actually implemented, so an unimplemented name fails
/// fast here instead of quietly producing nothing downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum ReporterKind {
    /// Human-readable output written to stdout.
    Terminal,
    /// One compact JSON record per finding, written to stdout, for streaming and agent workflows.
    Jsonl,
    /// Machine-readable JSON report, written to a file.
    Json,
    /// Self-contained HTML report, written to a file.
    Html,
    /// SARIF 2.1.0 log, written to a file, for GitHub code scanning and other SARIF consumers.
    Sarif,
}

impl ReporterKind {
    /// Whether this reporter writes to stdout rather than to a file.
    ///
    /// Two stdout reporters cannot run together: interleaving a human-readable report into a
    /// machine-parsed stream corrupts both, and the failure is silent at the point of use.
    pub fn writes_to_stdout(self) -> bool {
        matches!(self, Self::Terminal | Self::Jsonl)
    }

    /// The file extension a file-based reporter appends to the `--output` prefix.
    pub fn extension(self) -> Option<&'static str> {
        match self {
            Self::Json => Some("json"),
            Self::Html => Some("html"),
            Self::Sarif => Some("sarif"),
            Self::Terminal | Self::Jsonl => None,
        }
    }
}

/// The documented exit-code contract for the `vitest-dedup` binary.
///
/// Operational failures and quality-gate failures are kept as distinct codes on purpose: a CI
/// pipeline that treats "the tool could not run" the same as "the tool ran and found problems"
/// cannot distinguish a broken config from a real duplicate, and will mis-route the alert either
/// way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    /// The analysis ran to completion and found nothing that fails the quality gate.
    Success = 0,
    /// The analysis ran to completion but its error-severity duplication rate exceeded the
    /// configured quality-gate threshold. At the default threshold of `0` this means "at least
    /// one error-severity finding was retained"; a non-zero `--threshold` permits that rate of
    /// error-severity duplication instead of failing on the first one -- see
    /// `crate::analysis::exit_code`'s doc comment for the exact rule. `--fail-on-new` exceeding
    /// its limit is also this code, not [`ExitCode::OperationalError`]: the analyzer ran to
    /// completion and found more duplication than the baseline accounts for, which is a policy
    /// outcome, not a broken run.
    QualityGateFailed = 1,
    /// The analysis could not produce a complete quality result: bad configuration, an unreadable
    /// path, a parse failure, no tests analyzed, or -- with `--fail-on-incomplete` -- another
    /// bounded or incomplete run.
    OperationalError = 2,
}

/// Which incompleteness causes `--fail-on-incomplete` counts (C10). `Any` preserves the
/// flag's original single-bool meaning; `Reads` and `Limits` are the two classes a pipeline
/// can automate a response for. See [`crate::model::IncompleteCauses::failed_reads`] and
/// [`crate::model::IncompleteCauses::hit_limits`] for the classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum FailOnIncomplete {
    /// Every cause, including deliberate run-mode policies like changed-file withholding.
    Any,
    /// Only causes meaning the analyzed project could not be fully read.
    Reads,
    /// Only causes meaning this tool's own bounded work was capped.
    Limits,
}

impl ExitCode {
    /// The raw process exit code for this outcome.
    pub fn code(self) -> i32 {
        self as i32
    }
}

impl From<ExitCode> for i32 {
    /// The process exit code as a plain integer, for `std::process::exit`.
    fn from(value: ExitCode) -> Self {
        value.code()
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "vitest-dedup",
    version,
    about = "Detect duplicate and near-duplicate Vitest tests",
    after_help = "Examples:\n  vitest-dedup .\n  vitest-dedup check .\n  vitest-dedup . --reporters terminal,json"
)]
struct RawCli {
    /// Directory to analyze when no explicit subcommand is used.
    #[arg(value_name = "PATH", default_value = ".")]
    path: PathBuf,

    #[command(subcommand)]
    command: Option<RawCommand>,

    #[command(flatten)]
    options: RawOptions,
}

#[derive(Debug, Subcommand)]
enum RawCommand {
    /// Analyze a directory. Explicit equivalent of `vitest-dedup <PATH>`.
    Check {
        /// Directory to analyze.
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
}

#[derive(Debug, Clone, Args, Default)]
struct RawOptions {
    /// Explicit `.vitest-dedup.json` configuration file, bypassing upward auto-discovery.
    #[arg(long, short = 'c', global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Quality-gate threshold percentage (0-100).
    #[arg(
        long,
        global = true,
        value_name = "N",
        value_parser = clap::value_parser!(u8).range(0..=100)
    )]
    threshold: Option<u8>,

    /// Comma-separated reporters to run: terminal, jsonl, json, html, sarif. `terminal` and `jsonl` both write to stdout and cannot be combined.
    #[arg(long, global = true, value_delimiter = ',')]
    reporters: Option<Vec<ReporterKind>>,

    /// Filename prefix for file-based reports.
    #[arg(long, global = true, value_name = "PREFIX")]
    output: Option<PathBuf>,

    /// Emit one `"test"` record per analyzed test on the `jsonl` stream, before the findings.
    ///
    /// Requires `--reporters jsonl`: the records have no other output surface, and a flag that
    /// accepted itself and then produced nothing is exactly the silent failure this tool refuses
    /// elsewhere. Rejected here rather than ignored.
    #[arg(long, global = true)]
    emit_test_fingerprints: bool,

    /// Emit one `"memory"` record per pipeline phase on the `jsonl` stream, accounting for the
    /// analyzer's own retained heap.
    ///
    /// Requires `--reporters jsonl` for exactly the reason `--emit-test-fingerprints` does, and
    /// is rejected the same way rather than accepted and ignored.
    #[arg(long, global = true)]
    debug_memory: bool,

    /// Omit the terminal detection-time footer for reproducible output.
    #[arg(long, global = true)]
    no_metrics: bool,

    /// Override one rule's severity, e.g. `--rule duplicate-test=warning`. Repeatable; the last
    /// occurrence for a given rule wins.
    #[arg(
        long = "rule",
        global = true,
        value_name = "ID=SEVERITY",
        value_parser = parse_rule_override
    )]
    rule: Vec<(Rule, Severity)>,

    /// Exit with an operational error when the analysis could not fully cover the discovered
    /// corpus. With no value (or `=any`) every cause counts; `=reads` counts only causes that
    /// mean the analyzed project could not be fully read (unreadable/oversized/partial files,
    /// undetermined config, tests whose context could not be resolved); `=limits` counts only
    /// this tool's own bounded work being capped (pair budget, hook-scan budget, memory budget,
    /// truncated findings). The split exists because the two classes ask for different CI responses: fix
    /// the repository, or raise the limits.
    ///
    /// The value requires `=`: an optional value with a space would greedily consume the next
    /// token, so the documented bare form before a path (`--fail-on-incomplete .`) parsed the
    /// PATH as the class and errored (Copilot review, PR #30, High).
    #[arg(
        long,
        global = true,
        value_enum,
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "any"
    )]
    fail_on_incomplete: Option<FailOnIncomplete>,

    /// Baseline file of already-accepted findings. Matching findings are still reported, marked
    /// as baselined, and excluded from the quality gate.
    #[arg(long, global = true, value_name = "FILE")]
    baseline: Option<PathBuf>,

    /// Rewrite the `--baseline` file from this run's findings.
    ///
    /// Refused on an incomplete run: a baseline recorded from an analysis that could not see the
    /// whole corpus accepts a partial picture, and silently accepts whatever it missed the first
    /// time a complete run finds it. Cannot be combined with `--changed-since`, which would record
    /// only the slice of findings that run reported.
    // Expressed as clap `requires`/`conflicts_with` rather than a runtime check, so the impossible
    // combinations are rejected by the grammar with clap's own message before any work starts.
    #[arg(
        long,
        global = true,
        requires = "baseline",
        conflicts_with = "changed_since"
    )]
    update_baseline: bool,

    /// Compute the baseline from a git revision's tree instead of a file: findings the base
    /// revision produces are treated as already accepted, so a pull request fails only for what
    /// it ADDS.
    ///
    /// The base revision's files are read through `git cat-file` -- never checked out, so no
    /// hook, smudge filter, or other repository-controlled code runs. Files are admitted by the
    /// same extension classifier and the configured include/exclude globs; per-project `include`
    /// overrides and in-source candidates are not applied to the base read, which can only make
    /// the base report FEWER findings, which makes the gate stricter, never looser. Cannot be
    /// combined
    /// with `--baseline` or `--update-baseline`: this flag's baseline comes from git, not a file.
    #[arg(
        long,
        global = true,
        value_name = "REVISION",
        conflicts_with_all = ["baseline", "update_baseline"]
    )]
    baseline_from_ref: Option<String>,

    /// Fail the quality gate when more than N findings are absent from the baseline.
    ///
    /// Additional to `--threshold`, not a replacement for it: an un-baselined error-severity
    /// finding still fails the default threshold on its own.
    #[arg(
        long,
        global = true,
        value_name = "N",
        requires = "baseline",
        conflicts_with = "update_baseline"
    )]
    fail_on_new: Option<usize>,

    /// Report only findings touching a file changed since GIT_REF.
    ///
    /// Analysis still covers the whole corpus -- a cross-file duplicate whose twin lives in an
    /// unchanged file is still reported -- so this narrows the report, not the runtime. Findings
    /// lying entirely within unchanged files are withheld, and withholding anything marks the run
    /// incomplete. Needs full history: a shallow clone is rejected rather than diffed against.
    #[arg(long, global = true, value_name = "GIT_REF")]
    changed_since: Option<String>,

    /// Treat bare `test`/`describe`/`it` identifiers as Vitest, for projects configured with
    /// `test: { globals: true }`.
    ///
    /// The orchestrator already reads `test.globals` out of the Vite/Vitest config
    /// (`vitest_config::detect_globals`) and only falls back to `Disabled` when detection is
    /// inconclusive (no config found, or the setting could not be determined) -- an explicit
    /// `--globals` always wins over that detector, since the user is asserting something about
    /// their project directly. Pass this flag when detection could not read the answer: without
    /// either, a `globals: true` project extracts no tests at all; the zero-work guard then rejects
    /// the run as an operational error instead of producing useful analysis.
    #[arg(long, global = true)]
    globals: bool,

    /// Stop reading files once the analyzer holds more than MIB mebibytes (default 2048).
    ///
    /// Checked between files, with a current process RSS backstop at 1.5 times MIB. The run
    /// analyzes whole files only; the rest are reported and the run is marked incomplete.
    #[arg(
        long,
        global = true,
        value_name = "MIB",
        value_parser = clap::value_parser!(u64).range(1..),
        conflicts_with = "no_memory_cap"
    )]
    max_memory: Option<u64>,

    /// Remove the memory budget.
    #[arg(long, global = true)]
    no_memory_cap: bool,
}

/// Parsed and validated command-line arguments.
///
/// This is the CLI module's entire public surface for the orchestrator: it never runs discovery,
/// parsing, or analysis itself, only decides what should happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgs {
    /// Directory to analyze.
    pub root: PathBuf,
    /// Explicit configuration file, when passed with `--config`.
    pub config_file: Option<PathBuf>,
    /// Quality-gate threshold override.
    pub threshold: Option<u8>,
    /// Reporters to run, defaulting to `[Terminal]` when `--reporters` is omitted.
    pub reporters: Vec<ReporterKind>,
    /// Filename prefix for file-based reports.
    pub output: Option<PathBuf>,
    /// Whether the `jsonl` stream should carry a per-test fingerprint record
    /// (`--emit-test-fingerprints`). Only ever `true` alongside `ReporterKind::Jsonl`.
    pub emit_test_fingerprints: bool,
    /// Whether the `jsonl` stream should carry a per-phase retained-heap record
    /// (`--debug-memory`). Only ever `true` alongside `ReporterKind::Jsonl`.
    pub debug_memory: bool,
    /// Whether to omit terminal timing; machine reports never include these metrics.
    pub no_metrics: bool,
    /// Rule severity overrides parsed from repeated `--rule` flags.
    pub rule_overrides: BTreeMap<Rule, Severity>,
    /// Whether an incomplete analysis should be treated as an operational error, and which
    /// causes count. `None` is the flag ABSENT (the off state); `Some(Any)` is the bare flag;
    /// `Some(Reads)`/`Some(Limits)` are C10's two classes. Folding "off" into `Any` was tried
    /// and broke instantly: absent and any-everything are different decisions.
    pub fail_on_incomplete: Option<FailOnIncomplete>,
    /// Whether bare test-declaration identifiers should be treated as Vitest globals.
    pub globals: bool,
    /// Baseline file of already-accepted findings, when passed with `--baseline`.
    pub baseline: Option<PathBuf>,
    /// Whether the baseline file should be rewritten from this run (`--update-baseline`).
    pub update_baseline: bool,
    /// Git revision whose tree's findings form the baseline (`--baseline-from-ref`).
    pub baseline_from_ref: Option<String>,
    /// Quality-gate limit on findings absent from the baseline (`--fail-on-new`).
    pub fail_on_new: Option<usize>,
    /// Git revision to scope the report to (`--changed-since`).
    pub changed_since: Option<String>,
    /// Retained-memory budget in MiB, or `None` under `--no-memory-cap`.
    pub memory_budget_mib: Option<u64>,
}

impl ParsedArgs {
    /// Parses the process's own `argv`.
    ///
    /// # Errors
    /// Returns an error for invalid flags/values (clap reports these directly) and for the
    /// "path before `check`" conflict this module validates itself.
    pub fn parse() -> Result<Self> {
        match RawCli::try_parse_from(std::env::args_os()) {
            Ok(raw) => Self::from_raw(raw),
            // clap models `--help` and `--version` as *errors*. They are not: the user asked for
            // output and got it. Letting them fall through to the caller sent both to stderr and
            // exited 2 -- this tool's "the analyzer broke" code -- so `--help` looked like a
            // failure to a shell, a CI step, and any release smoke test that reads `--version`.
            // They are answered here, on stdout, with a success exit.
            //
            // Only these two are intercepted. A genuine parse error still returns `Err` so it
            // reaches `main`'s sanitizing handler: clap quotes the offending value back, and that
            // value is attacker-controllable argv, so it must not bypass `Sanitized`.
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::DisplayHelp
                        | ErrorKind::DisplayVersion
                        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                ) =>
            {
                let mut stdout = std::io::stdout().lock();
                // Ignore a write failure: the process is exiting successfully either way, and a
                // closed pipe (`vitest-dedup --help | head`) is the ordinary case, not an error.
                let _ = write!(stdout, "{error}");
                let _ = stdout.flush();
                std::process::exit(0);
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Parses an explicit argument list. Used by tests and by embedders that assemble `argv`
    /// themselves rather than reading the process environment.
    ///
    /// Unlike [`ParsedArgs::parse`], this never exits the process: `--help` and `--version` are
    /// returned as errors for the caller to render however it wants. A library function that
    /// terminated its host would be unusable from a test or an embedder.
    pub fn parse_from<I, T>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        Self::from_raw(RawCli::try_parse_from(args)?)
    }

    /// Validates and converts the raw clap output into [`ParsedArgs`]: every rejection the
    /// grammar cannot express (duplicate reporters, two stdout writers, flags riding a stream
    /// that was not selected) fails here, before any analysis work starts.
    fn from_raw(cli: RawCli) -> Result<Self> {
        if cli.command.is_some() && cli.path.as_path() != Path::new(".") {
            bail!("do not pass a path before `check`; use `vitest-dedup check <PATH>` instead");
        }
        let root = match cli.command {
            Some(RawCommand::Check { path }) => path,
            None => cli.path,
        };
        let mut reporters = cli
            .options
            .reporters
            .unwrap_or_else(|| vec![ReporterKind::Terminal]);
        // Selecting the same reporter twice is a harmless typo; running it twice is not, since a
        // file reporter would write its report and then immediately overwrite it.
        //
        // `Vec::dedup_by` only removes *adjacent* duplicates, so `terminal,json,terminal` kept both
        // `terminal` entries and was then rejected as two stdout reporters -- with a message naming
        // `jsonl`, which the user never asked for. Deduplicate by identity across the whole list
        // while preserving the order the user wrote.
        let mut seen = Vec::new();
        reporters.retain(|reporter| {
            if seen.contains(reporter) {
                false
            } else {
                seen.push(*reporter);
                true
            }
        });

        // Rejected rather than resolved by precedence. Silently dropping one of two stdout
        // reporters would give a caller a stream they believe is complete, and the corruption
        // would only surface downstream in whatever tried to parse it.
        let stdout_reporters: Vec<_> = reporters
            .iter()
            .filter(|reporter| reporter.writes_to_stdout())
            .collect();
        if stdout_reporters.len() > 1 {
            bail!(
                "`terminal` and `jsonl` both write to stdout and cannot be combined; \
                 pick one, and add `json` or `html` for a file report"
            );
        }

        // Rejected, never ignored. `--emit-test-fingerprints` adds records to one stream and only
        // that stream; accepting it under `--reporters terminal` (or a file-only run) would exit
        // 0 having emitted nothing the caller asked for, and a consumer built on those records --
        // a gate reading them to decide whether a fixture still isolates what it claims -- would
        // read the empty result as "no tests" rather than "you did not select the reporter".
        if cli.options.emit_test_fingerprints && !reporters.contains(&ReporterKind::Jsonl) {
            bail!(
                "`--emit-test-fingerprints` adds records to the `jsonl` stream and has no other \
                 output surface; add `--reporters jsonl` (it combines with the file reporters)"
            );
        }

        // Same rule, same reason, applied to the second flag that rides on this stream. Stated as
        // its own check rather than folded into the one above so each message names the flag the
        // caller actually typed: a message naming `--emit-test-fingerprints` for a
        // `--debug-memory` run would send someone to the wrong flag's documentation. A
        // memory-budget gate reading for these records is the consumer that cannot tell "the tool
        // emitted no record" from "the tool was never asked to", which is what makes silence here
        // unacceptable rather than merely unhelpful.
        if cli.options.debug_memory && !reporters.contains(&ReporterKind::Jsonl) {
            bail!(
                "`--debug-memory` adds records to the `jsonl` stream and has no other output \
                 surface; add `--reporters jsonl` (it combines with the file reporters)"
            );
        }

        Ok(Self {
            root,
            config_file: cli.options.config,
            threshold: cli.options.threshold,
            reporters,
            output: cli.options.output,
            emit_test_fingerprints: cli.options.emit_test_fingerprints,
            debug_memory: cli.options.debug_memory,
            no_metrics: cli.options.no_metrics,
            rule_overrides: cli.options.rule.into_iter().collect(),
            fail_on_incomplete: cli.options.fail_on_incomplete,
            globals: cli.options.globals,
            baseline: cli.options.baseline,
            baseline_from_ref: cli.options.baseline_from_ref,
            update_baseline: cli.options.update_baseline,
            fail_on_new: cli.options.fail_on_new,
            changed_since: cli.options.changed_since,
            memory_budget_mib: (!cli.options.no_memory_cap).then(|| {
                cli.options
                    .max_memory
                    .unwrap_or(crate::memory::DEFAULT_MEMORY_BUDGET_MIB)
            }),
        })
    }

    /// Converts CLI flags into the overrides `Config::load` applies after file configuration.
    ///
    /// Only `config_file`, `threshold`, and `rule_overrides` map onto [`crate::config::Config`]
    /// fields; `reporters`, `output`, `fail_on_incomplete`, and the baseline/changed-file flags
    /// are not part of the analyzed configuration surface and stay on `ParsedArgs` for the
    /// orchestrator to read directly.
    pub fn to_overrides(&self) -> ConfigOverrides {
        ConfigOverrides {
            config_file: self.config_file.clone(),
            threshold: self.threshold,
            rules: self.rule_overrides.clone(),
        }
    }
}

/// Parse RULE=SEVERITY into the rule and severity enums, returning a diagnostic for malformed
/// or unknown values.
fn parse_rule_override(value: &str) -> Result<(Rule, Severity), String> {
    let (id, severity) = value
        .split_once('=')
        .ok_or_else(|| "expected RULE=SEVERITY, e.g. duplicate-test=warning".to_owned())?;
    Ok((
        config::parse_rule_id(id)?,
        config::parse_severity(severity)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check CLI parsing: supports implicit and explicit check forms.
    #[test]
    fn supports_implicit_and_explicit_check_forms() {
        let implicit = ParsedArgs::parse_from(["vitest-dedup", "src"]).unwrap();
        assert_eq!(implicit.root, PathBuf::from("src"));
        let explicit = ParsedArgs::parse_from(["vitest-dedup", "check", "src"]).unwrap();
        assert_eq!(explicit.root, PathBuf::from("src"));
        assert!(ParsedArgs::parse_from(["vitest-dedup", "check"]).is_err());
    }

    /// Check CLI parsing: defaults root to current directory.
    #[test]
    fn defaults_root_to_current_directory() {
        let args = ParsedArgs::parse_from(["vitest-dedup"]).unwrap();
        assert_eq!(args.root, PathBuf::from("."));
    }

    /// Check CLI parsing: rejects a path placed before check.
    #[test]
    fn rejects_a_path_placed_before_check() {
        let error = ParsedArgs::parse_from(["vitest-dedup", "src", "check", "src"]).unwrap_err();
        assert!(error
            .to_string()
            .contains("do not pass a path before `check`"));
    }

    /// Check CLI parsing: reporters default to terminal when omitted.
    #[test]
    fn reporters_default_to_terminal_when_omitted() {
        let args = ParsedArgs::parse_from(["vitest-dedup"]).unwrap();
        assert_eq!(args.reporters, vec![ReporterKind::Terminal]);
    }

    /// Check CLI parsing: reporters accept a comma separated list and reject unknown values.
    #[test]
    fn reporters_accept_a_comma_separated_list_and_reject_unknown_values() {
        let args =
            ParsedArgs::parse_from(["vitest-dedup", "--reporters", "terminal,json"]).unwrap();
        assert_eq!(
            args.reporters,
            vec![ReporterKind::Terminal, ReporterKind::Json]
        );
        assert!(ParsedArgs::parse_from(["vitest-dedup", "--reporters", "xml"]).is_err());
    }

    /// Check CLI parsing: sarif is an accepted file reporter.
    #[test]
    fn sarif_is_an_accepted_file_reporter() {
        let args = ParsedArgs::parse_from(["vitest-dedup", "--reporters", "sarif"]).unwrap();
        assert_eq!(args.reporters, vec![ReporterKind::Sarif]);
        assert!(!ReporterKind::Sarif.writes_to_stdout());
        assert_eq!(ReporterKind::Sarif.extension(), Some("sarif"));
    }

    /// Check CLI parsing: threshold is bounded to zero through one hundred.
    #[test]
    fn threshold_is_bounded_to_zero_through_one_hundred() {
        assert_eq!(
            ParsedArgs::parse_from(["vitest-dedup", "--threshold", "0"])
                .unwrap()
                .threshold,
            Some(0)
        );
        assert_eq!(
            ParsedArgs::parse_from(["vitest-dedup", "--threshold", "100"])
                .unwrap()
                .threshold,
            Some(100)
        );
        assert!(ParsedArgs::parse_from(["vitest-dedup", "--threshold", "101"]).is_err());
        assert!(ParsedArgs::parse_from(["vitest-dedup", "--threshold", "-1"]).is_err());
    }

    /// Check CLI parsing: rule overrides parse repeated flags and reject bad input.
    #[test]
    fn rule_overrides_parse_repeated_flags_and_reject_bad_input() {
        let args = ParsedArgs::parse_from([
            "vitest-dedup",
            "--rule",
            "duplicate-test=warning",
            "--rule",
            "duplicate-test-name=off",
        ])
        .unwrap();
        assert_eq!(
            args.rule_overrides.get(&Rule::DuplicateTest),
            Some(&Severity::Warning)
        );
        assert_eq!(
            args.rule_overrides.get(&Rule::DuplicateTestName),
            Some(&Severity::Off)
        );

        assert!(ParsedArgs::parse_from(["vitest-dedup", "--rule", "not-a-rule=off"]).is_err());
        assert!(
            ParsedArgs::parse_from(["vitest-dedup", "--rule", "duplicate-test=critical"]).is_err()
        );
        assert!(ParsedArgs::parse_from(["vitest-dedup", "--rule", "duplicate-test"]).is_err());
    }

    /// Check CLI parsing: no metrics is global and off by default.
    #[test]
    fn no_metrics_is_global_and_off_by_default() {
        assert!(!ParsedArgs::parse_from(["vitest-dedup"]).unwrap().no_metrics);
        for argv in [
            vec!["vitest-dedup", "--no-metrics", "."],
            vec!["vitest-dedup", ".", "--no-metrics"],
            vec!["vitest-dedup", "check", ".", "--no-metrics"],
            vec!["vitest-dedup", "--no-metrics", "check", "."],
            vec!["vitest-dedup", "--reporters", "jsonl", "--no-metrics"],
        ] {
            assert!(ParsedArgs::parse_from(argv).unwrap().no_metrics);
        }
    }

    /// Check CLI parsing: the memory budget defaults to 2048 MiB, takes a positive value, and is
    /// removed only by `--no-memory-cap`, which conflicts with an explicit budget.
    #[test]
    fn memory_budget_defaults_and_accepts_overrides() {
        let budget =
            |argv: &[&str]| ParsedArgs::parse_from(argv).map(|args| args.memory_budget_mib);
        assert_eq!(budget(&["vitest-dedup"]).unwrap(), Some(2_048));
        assert_eq!(
            budget(&["vitest-dedup", "--max-memory", "512"]).unwrap(),
            Some(512)
        );
        assert_eq!(budget(&["vitest-dedup", "--no-memory-cap"]).unwrap(), None);
        assert!(budget(&["vitest-dedup", "--max-memory", "0"]).is_err());
        assert!(budget(&["vitest-dedup", "--max-memory", "512", "--no-memory-cap"]).is_err());
    }

    /// Check CLI parsing: repeated rule flag for the same rule keeps the last occurrence.
    #[test]
    fn repeated_rule_flag_for_the_same_rule_keeps_the_last_occurrence() {
        let args = ParsedArgs::parse_from([
            "vitest-dedup",
            "--rule",
            "duplicate-test=warning",
            "--rule",
            "duplicate-test=error",
        ])
        .unwrap();
        assert_eq!(
            args.rule_overrides.get(&Rule::DuplicateTest),
            Some(&Severity::Error)
        );
    }

    /// Check CLI parsing: to overrides maps only config relevant fields.
    #[test]
    fn to_overrides_maps_only_config_relevant_fields() {
        let args = ParsedArgs::parse_from([
            "vitest-dedup",
            "--threshold",
            "10",
            "--rule",
            "duplicate-test=off",
            "--config",
            "team.json",
        ])
        .unwrap();
        let overrides = args.to_overrides();
        assert_eq!(overrides.config_file, Some(PathBuf::from("team.json")));
        assert_eq!(overrides.threshold, Some(10));
        assert_eq!(
            overrides.rules.get(&Rule::DuplicateTest),
            Some(&Severity::Off)
        );
    }

    /// Check CLI parsing: globals defaults to false and is settable.
    #[test]
    fn globals_defaults_to_false_and_is_settable() {
        assert!(!ParsedArgs::parse_from(["vitest-dedup"]).unwrap().globals);
        assert!(
            ParsedArgs::parse_from(["vitest-dedup", "--globals"])
                .unwrap()
                .globals
        );
    }

    /// Check CLI parsing: fail on incomplete defaults to any and parses its classes.
    #[test]
    fn fail_on_incomplete_defaults_to_any_and_parses_its_classes() {
        use super::FailOnIncomplete;
        // Absent: the flag is off entirely -- the state every default run is in.
        assert_eq!(
            ParsedArgs::parse_from(["vitest-dedup"])
                .unwrap()
                .fail_on_incomplete,
            None
        );
        // The original spelling must keep its meaning: bare flag = any cause.
        assert_eq!(
            ParsedArgs::parse_from(["vitest-dedup", "--fail-on-incomplete"])
                .unwrap()
                .fail_on_incomplete,
            Some(FailOnIncomplete::Any)
        );
        // The two classes a pipeline can automate a response for (C10).
        assert_eq!(
            ParsedArgs::parse_from(["vitest-dedup", "--fail-on-incomplete=reads"])
                .unwrap()
                .fail_on_incomplete,
            Some(FailOnIncomplete::Reads)
        );
        assert_eq!(
            ParsedArgs::parse_from(["vitest-dedup", "--fail-on-incomplete=limits"])
                .unwrap()
                .fail_on_incomplete,
            Some(FailOnIncomplete::Limits)
        );
        // An unknown class is a usage error, not silently "any".
        assert!(ParsedArgs::parse_from(["vitest-dedup", "--fail-on-incomplete=oops"]).is_err());
        // The bare flag before a POSITIONAL is the documented invocation and must stay usable:
        // an optional value taken with a space would swallow the path as the class (Copilot
        // review, PR #30). The value is `=`-only.
        let bare_before_path =
            ParsedArgs::parse_from(["vitest-dedup", "--fail-on-incomplete", "."]).unwrap();
        assert_eq!(
            bare_before_path.fail_on_incomplete,
            Some(FailOnIncomplete::Any)
        );
        // With the space form the token can no longer feed the enum -- it falls through to
        // the positional path, which is the disambiguation doing its job.
        let space_form =
            ParsedArgs::parse_from(["vitest-dedup", "--fail-on-incomplete", "reads"]).unwrap();
        assert_eq!(space_form.fail_on_incomplete, Some(FailOnIncomplete::Any));
        assert_eq!(space_form.root, std::path::PathBuf::from("reads"));
    }

    /// Check CLI parsing: emit test fingerprints is off by default and requires the jsonl
    /// reporter.
    #[test]
    fn emit_test_fingerprints_is_off_by_default_and_requires_the_jsonl_reporter() {
        assert!(
            !ParsedArgs::parse_from(["vitest-dedup"])
                .unwrap()
                .emit_test_fingerprints,
            "the records are opt-in; a default run must not compute or emit them"
        );

        let args = ParsedArgs::parse_from([
            "vitest-dedup",
            "--reporters",
            "jsonl",
            "--emit-test-fingerprints",
        ])
        .unwrap();
        assert!(args.emit_test_fingerprints);

        // Combining with a file reporter is fine -- `jsonl` owns stdout, the others own files --
        // and is the shape a CI job wanting both actually writes.
        assert!(
            ParsedArgs::parse_from([
                "vitest-dedup",
                "--reporters",
                "jsonl,json",
                "--emit-test-fingerprints",
            ])
            .unwrap()
            .emit_test_fingerprints
        );

        // Without `jsonl` the records have nowhere to go. Rejected, not ignored: a flag that is
        // accepted and then does nothing is indistinguishable from a corpus with no tests to the
        // consumer reading for these records.
        for reporters in [None, Some("terminal"), Some("json"), Some("html,sarif")] {
            let mut argv = vec!["vitest-dedup"];
            if let Some(reporters) = reporters {
                argv.extend(["--reporters", reporters]);
            }
            argv.push("--emit-test-fingerprints");
            let error = ParsedArgs::parse_from(argv.clone())
                .expect_err(&format!("expected {argv:?} to be rejected"));
            assert!(
                error.to_string().contains("--emit-test-fingerprints"),
                "the message must name the rejected flag: {error}"
            );
        }
    }

    /// Check CLI parsing: debug memory is off by default and requires the jsonl reporter.
    #[test]
    fn debug_memory_is_off_by_default_and_requires_the_jsonl_reporter() {
        assert!(
            !ParsedArgs::parse_from(["vitest-dedup"])
                .unwrap()
                .debug_memory,
            "the records are opt-in; a default run must not compute or emit them"
        );

        let args =
            ParsedArgs::parse_from(["vitest-dedup", "--reporters", "jsonl", "--debug-memory"])
                .unwrap();
        assert!(args.debug_memory);

        // Combines with the file reporters, and with the other stream-riding flag.
        assert!(
            ParsedArgs::parse_from([
                "vitest-dedup",
                "--reporters",
                "jsonl,json",
                "--debug-memory",
                "--emit-test-fingerprints",
            ])
            .unwrap()
            .debug_memory
        );

        for reporters in [None, Some("terminal"), Some("json"), Some("html,sarif")] {
            let mut argv = vec!["vitest-dedup"];
            if let Some(reporters) = reporters {
                argv.extend(["--reporters", reporters]);
            }
            argv.push("--debug-memory");
            let error = ParsedArgs::parse_from(argv.clone())
                .expect_err(&format!("expected {argv:?} to be rejected"));
            assert!(
                error.to_string().contains("--debug-memory"),
                "the message must name the rejected flag, not its sibling: {error}"
            );
        }
    }

    /// Check CLI parsing: exit code values match the documented contract.
    #[test]
    fn exit_code_values_match_the_documented_contract() {
        assert_eq!(ExitCode::Success.code(), 0);
        assert_eq!(ExitCode::QualityGateFailed.code(), 1);
        assert_eq!(ExitCode::OperationalError.code(), 2);
        assert_eq!(i32::from(ExitCode::QualityGateFailed), 1);
    }
}
