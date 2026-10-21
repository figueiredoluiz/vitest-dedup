//! The `vitest-dedup` command-line entry point.
//!
//! This binary is deliberately thin: it sequences discovery, extraction, analysis, and reporting,
//! and it owns the exit-code contract. Every decision it makes is delegated to a library module so
//! the same pipeline is reachable by embedders.

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode as ProcessExitCode;
use std::time::Instant;
use vitest_dedup::cli::{ExitCode, ParsedArgs, ReporterKind};
use vitest_dedup::config::Config;
use vitest_dedup::exclusions::{self, ExclusionCollector};
use vitest_dedup::frontend::{self, Globals};
use vitest_dedup::model::{AnalysisResult, ExclusionReason, IncompleteCauses};
use vitest_dedup::modes;
use vitest_dedup::reporters::{
    terminal_safe, DetectionTiming, HtmlReporter, JsonReporter, JsonlReporter, Reporter,
    SarifReporter, TerminalReportContext, TerminalReporter,
};
use vitest_dedup::vitest_config::{self, GlobalsDetection};
use vitest_dedup::vitest_projects;
use vitest_dedup::{analysis, discovery};

mod diagnostics;
use diagnostics::Sanitized;

mod rss;

#[cfg(test)]
mod memory_roof_tests;

/// Run the CLI, preserving quality-gate exit codes and reporting operational errors with
/// sanitized diagnostics.
fn main() -> ProcessExitCode {
    match run() {
        Ok(code) => process_code(code),
        Err(error) => {
            // Operational failures are distinguishable from quality-gate failures. Conflating them
            // would make CI unable to tell "your tests are duplicated" from "the analyzer broke".
            // The error chain can carry repository-controlled text (a path from a
            // `.with_context` wrapper, a config value quoted back in a validation message):
            // sanitized the same way a reporter would, so it cannot forge or rewrite this line.
            let _ = diagnostics::write_error(&mut std::io::stderr().lock(), &error);
            process_code(ExitCode::OperationalError)
        }
    }
}

/// Narrows the documented exit-code contract to the `u8` the process API accepts.
///
/// `ExitCode::code` returns `i32` to match the contract as documented; every variant is in `0..=2`,
/// so the conversion cannot lose information. Clamping rather than casting keeps that true even if
/// a future variant is added carelessly.
fn process_code(code: ExitCode) -> ProcessExitCode {
    ProcessExitCode::from(u8::try_from(code.code()).unwrap_or(2))
}

/// Converts a discovered test's absolute [`Location::path`](vitest_dedup::model::Location::path)
/// into one relative to the analysis `root`, for portable, diffable reports.
///
/// Falls back to the absolute `path` unchanged when it does not fall under `root`. A direct
/// `strip_prefix` can refuse equivalent Windows spellings when one side has the verbatim `\\?\`
/// prefix, so existing paths get one canonical comparison before that fallback. A genuinely
/// outside-root or missing path is still forwarded unchanged: silently dropping or inventing a
/// relative path would be a coverage gap disguised as success.
///
/// `discovery::discover` walks with `follow_links(false)` (see `discovery.rs`), so a file reached
/// through a symlink that resolves outside `root` is not walked at all today -- this function's
/// fallback branch is therefore defense in depth against a discovery change or a frontend path
/// that arrives from somewhere else, not a route this binary currently exercises end to end.
/// Verified empirically before writing this comment: pointing the binary at a directory containing
/// a symlink (to a file, and separately to a directory) that resolves outside the root extracts
/// zero tests, confirming discovery never yields the escaping path in the first place. Pinned
/// directly against this function instead, via a `#[cfg(unix)]` symlink fixture, so the decision is
/// tested regardless of whether discovery's traversal policy ever changes.
/// A repository path as every stderr diagnostic must print it: relativized against the analysis
/// root, then sanitized.
///
/// The two halves belong together and had been applied separately at nine call sites, which is
/// how the S12 sweep found sites that had one and not the other -- `discovery.rs`'s
/// governance-reason line and the `includeSource` globals diagnostic both printed an absolute
/// path while their immediate siblings did not. Spelling the pair out per site also put
/// `main.rs` over the production-Rust duplication ceiling (0.63%), which is the same defect
/// measured a second way: a five-line idiom repeated is a five-line idiom that can be got wrong
/// in one copy.
///
/// A new diagnostic that forgets this function is still possible, but it is now one obvious call
/// rather than a block to reassemble from memory, and
/// `tests::no_stderr_line_contains_the_analysis_root_prefix` sweeps every line the fixture can
/// reach.
fn diagnostic_path(path: &std::path::Path, root: &std::path::Path) -> Sanitized {
    Sanitized::new(&display_relative(path, root))
}

/// A relativized path spelled with `/` on every platform, for anything a human or a gate reads.
///
/// `.display()` on its own renders the PLATFORM separator, so stderr said `src\broken.test.ts` on
/// Windows while every reporter in the same run said `src/broken.test.ts` — the reporters normalize
/// through `report_safe_path`, and `analysis.rs` normalizes its ordering key for the same reason its
/// doc gives: otherwise determinism becomes platform-dependent. stderr was the one surface left out,
/// which is why `stderr_diagnostic_paths_are_relativized_against_the_analysis_root` failed on
/// windows-latest and nowhere else.
///
/// Separators are normalized BEFORE sanitizing, the same order `report_safe_path` uses, so a
/// control character cannot hide a separator from the replacement.
fn display_relative(path: &std::path::Path, root: &std::path::Path) -> String {
    relativize_test_path(path, root)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Paths beneath `root` become root-relative (the portable form every report and fingerprint
/// carries); paths outside it pass through unchanged, because no better answer exists and
/// inventing one would be wrong in a different way.
fn relativize_test_path(path: &std::path::Path, root: &std::path::Path) -> std::path::PathBuf {
    if let Ok(relative) = path.strip_prefix(root) {
        return relative.to_path_buf();
    }

    let canonical_relative = path
        .canonicalize()
        .and_then(|path| root.canonicalize().map(|root| (path, root)))
        .ok()
        .and_then(|(path, root)| path.strip_prefix(root).map(ToOwned::to_owned).ok());

    canonical_relative.unwrap_or_else(|| path.to_path_buf())
}

/// Relativizes declaration, registration and hook paths against the analysis root. Both
/// `Location` families that escape to reports and fingerprints, which must never
/// disagree about what a reported path means. Shared by the working-tree extraction loop and the
/// `--baseline-from-ref` base pass.
fn relativize_extraction_paths(
    tests: &mut [vitest_dedup::model::TestCase],
    root: &std::path::Path,
) {
    for test in tests {
        test.location.path = relativize_test_path(&test.location.path, root);
        if let Some(path) = &mut test.registration_file {
            *path = relativize_test_path(path, root);
        }
        for hook in &mut test.context.hooks {
            hook.location.path = relativize_test_path(&hook.location.path, root);
        }
    }
}

#[cfg(test)]
mod registration_path_tests {
    use super::*;

    /// Regression fixture for CLI reporting: registration identity relativizes source caller
    /// and hook paths for both passes.
    #[test]
    fn registration_identity_relativizes_source_caller_and_hook_paths_for_both_passes() {
        use vitest_dedup::discovery::{ModuleFlavor, SourceFile, SourceLanguage};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let file = SourceFile::without_project_context(
            root.join("shared/runner.ts"),
            SourceLanguage::TypeScript,
            ModuleFlavor::Ambiguous,
        );
        let source = "import { it, beforeEach } from 'vitest'; beforeEach(() => { seed(); }); it('case', () => { expect(1).toBe(1); });";
        let mut tests = frontend::extract(source, &file, Globals::Disabled).tests;
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].context.hooks.len(), 1);
        tests[0].registration_file = Some(root.join("tests/caller.test.ts"));
        relativize_extraction_paths(&mut tests, root);
        assert_eq!(
            tests[0].location.path,
            std::path::PathBuf::from("shared/runner.ts")
        );
        assert_eq!(
            tests[0].registration_file(),
            std::path::Path::new("tests/caller.test.ts")
        );
        assert_eq!(
            tests[0].context.hooks[0].location.path,
            std::path::PathBuf::from("shared/runner.ts")
        );
        let outside = tempfile::tempdir().unwrap();
        let caller = outside.path().join("external.test.ts");
        tests[0].registration_file = Some(caller.clone());
        relativize_extraction_paths(&mut tests, root);
        assert_eq!(tests[0].registration_file(), caller.as_path());
    }
}

/// Builds a report path from the `--output` prefix and a reporter's extension.
///
/// A prefix is a filename stem, not a directory: `--output reports/vitest-dedup` yields
/// `reports/vitest-dedup.json`. Treating it as a directory would silently change where a caller's
/// report lands when a second reporter is added later.
fn report_path(prefix: &std::path::Path, extension: &str) -> std::path::PathBuf {
    let mut path = prefix.to_path_buf();
    let stem = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "vitest-dedup".to_owned());
    path.set_file_name(format!("{stem}.{extension}"));
    path
}

/// The memory roof `--max-memory`/`--no-memory-cap` selected.
fn memory_roof(args: &ParsedArgs) -> vitest_dedup::memory::MemoryRoof {
    vitest_dedup::memory::MemoryRoof::new(
        args.memory_budget_mib
            .map(|mib| mib.saturating_mul(1024 * 1024)),
    )
}

/// Names where the memory roof stopped a pass and what to do about it.
fn write_roof_diagnostic(
    stderr: &mut impl Write,
    pass: &str,
    args: &ParsedArgs,
    files_read: usize,
    remaining: usize,
) -> Result<()> {
    writeln!(
        stderr,
        "\nvitest-dedup: the {pass} reached the memory budget ({} MiB) after {files_read} file(s); \
         {remaining} remaining file(s) were not analyzed. Raise --max-memory, pass \
         --no-memory-cap, or narrow the analyzed directory or `include`.",
        args.memory_budget_mib.unwrap_or_default()
    )?;
    Ok(())
}

/// Parse arguments and run discovery, extraction, analysis and reporting with native RSS samples.
fn run() -> Result<ExitCode> {
    run_with_rss(ParsedArgs::parse()?, rss::current_bytes)
}

/// Run the pipeline with an injectable current-RSS reader for deterministic boundary tests.
fn run_with_rss(args: ParsedArgs, mut resident: impl FnMut() -> Option<u64>) -> Result<ExitCode> {
    // The unlocked handle, by type: `Stderr` takes the process lock for each write only, while a
    // `StderrLock` held for the whole run would block any stderr write made on the extraction
    // worker thread that the main thread joins below. Annotated so putting `.lock()` back does not
    // compile.
    let mut stderr: std::io::Stderr = std::io::stderr();
    let config = Config::load(&args.root, args.to_overrides())?;

    // Resolved before any analysis, so an unusable revision, a shallow clone, or a non-repository
    // root fails immediately with exit 2 rather than after a full run. Every one of those would
    // otherwise produce an empty changed set, which filters away every finding and reports a clean
    // run over a repository nobody looked at -- see `modes::changed_files_since`.
    let changed_files = match &args.changed_since {
        Some(revision) => Some(modes::changed_files_since(&config.root, revision)?),
        None => None,
    };
    // Loaded up front for the same reason: a stale, malformed, or unreadable baseline must not be
    // discovered only after the analysis it was supposed to filter.
    //
    // Skipped under `--update-baseline`: that run rewrites the file from scratch, so applying the
    // old one first would mark this run's findings against a baseline that is about to be replaced
    // -- and, worse, would make an unreadable old file block the very command meant to fix it.
    let baseline = match (&args.baseline, args.update_baseline) {
        (Some(path), false) => Some(modes::load_baseline(path)?),
        _ => None,
    };

    // Collect only for a terminal report that requested timing. Phase boundaries exclude
    // baseline preparation and report generation, so slow output cannot inflate detection time.
    let metrics_enabled = !args.no_metrics && args.reporters.contains(&ReporterKind::Terminal);
    let discovery_started = metrics_enabled.then(Instant::now);

    // Per-project resolution first: a monorepo's `test.projects` entries each resolve their own
    // config and can diverge via `extends: false`, so a single repo-wide answer is wrong for at
    // least one project. Falls back to the single-project detector when no projects are declared.
    let projects = vitest_projects::resolve_projects(&config.root);
    // `resolve_projects` deliberately records why a project map is partial. Discarding that let a
    // malformed or truncated `test.projects` setup produce a map that looks complete: files fell
    // back to default matching, `incomplete` stayed false, and nothing said a project's globals or
    // include globs may have been missed.
    // Read the explicit signal rather than inferring from emptiness. `projects` is empty for three
    // different situations -- no config at all, an explicitly empty `test.projects`, and a config
    // whose project list could not be read -- and only the first means nothing was missed. An
    // earlier version inferred "no config" from `is_empty()`, which quietly disabled scoping and
    // suppressed the incompleteness signal for the malformed case too.
    //
    // A repository with no Vitest config is the ordinary case for a plain library: nothing to
    // resolve, nothing missed, and announcing it on every run is noise in the channel the genuinely
    // partial cases need.
    let project_scoping = if projects.config_found {
        Some(&projects)
    } else {
        None
    };
    if project_scoping.is_some() {
        for diagnostic in &projects.diagnostics {
            writeln!(stderr, "vitest-dedup: {}", Sanitized::new(diagnostic))?;
        }
    }
    // An incomplete project map degrades the run even when every discovered file happens to
    // attribute to some project: the entry this scan could not read governed files whose
    // attribution is therefore a guess. Recorded here because the flag must say what the banner
    // below says -- the 2026-09-27 scan measured 13 repositories printing this message with
    // `projectConfigDegraded: false`, because discovery only set the flag when a file actually
    // fell through to the defaults, and a JSON/CI consumer that never reads stderr saw a clean
    // run (backlog S2's second half).
    let project_map_incomplete =
        project_scoping.is_some() && (projects.has_unresolved_entries || projects.truncated);
    if project_map_incomplete {
        writeln!(
            stderr,
            "vitest-dedup: the Vitest project map is incomplete{}; files it could not attribute \
             were matched against the built-in defaults instead, so a project's `globals` or \
             `include` may have been missed.",
            if projects.truncated {
                " (truncated at the project limit)"
            } else {
                ""
            }
        )?;
    }

    let discovered = discovery::discover(&config, project_scoping)?;

    // An explicit `--globals` always wins: the user is asserting something about their project,
    // and a detector that overrode them would be unarguable. Otherwise read the Vitest config.
    let resolved_globals = if args.globals {
        Globals::Enabled
    } else {
        let analysis_root = config.root.clone();
        match vitest_config::detect_globals_with_projects(&config.root, Some(&projects)) {
            GlobalsDetection::Enabled {
                config: globals_config,
            } => {
                // `globals_config` is a path read out of the analyzed repository, not something
                // this tool generated -- sanitize it before it reaches the terminal, and
                // relativize it against the analysis root like every other diagnostic path
                // (S12): absolute here would differ every checkout.
                writeln!(
                    stderr,
                    "vitest-dedup: {} sets `test.globals = true`; analyzing ambient declarations.",
                    diagnostic_path(&globals_config, &analysis_root)
                )?;
                Globals::Enabled
            }
            GlobalsDetection::Disabled { .. } => Globals::Disabled,
            // Conservative by design. Guessing `Enabled` would invent findings in Playwright and
            // Jest files; guessing `Disabled` silently analyzes nothing. The zero-test check below
            // is what stops the second failure from looking like success.
            // Only worth saying when there was something to read and it was inconclusive. A
            // project with no Vitest config at all is the ordinary case for a plain library, and
            // announcing it on every run is noise that trains people to ignore the channel the
            // genuinely important warnings use. The zero-test guard below is what catches the
            // case where this silence would have been wrong.
            GlobalsDetection::Unknown { reason } if reason.starts_with("no vitest.config") => {
                Globals::Disabled
            }
            GlobalsDetection::Unknown { reason } => {
                // `reason` may echo back filesystem-derived text (a path, a parse error); sanitize
                // it for the same reason as the branch above.
                writeln!(
                    stderr,
                    "vitest-dedup: could not determine `test.globals` ({}).",
                    Sanitized::new(&reason)
                )?;
                Globals::Disabled
            }
            // `GlobalsDetection` is `#[non_exhaustive]`, so a future variant must not silently
            // become "enabled". Anything unrecognized is treated as the conservative case.
            _ => Globals::Disabled,
        }
    };
    let discovery_time = discovery_started.map(|started| started.elapsed());

    // `--baseline-from-ref`: the baseline is the base revision's own finding set, computed by a
    // contained second analysis pass over git blobs (see `modes::revision_sources` for the
    // containment design -- no checkout, no repository-controlled code runs). Built before the
    // main analysis so `RunModes` sees it exactly like a file baseline; it dies with the run,
    // and nothing is ever written to disk. Placed after `resolved_globals` because the base run
    // reuses that answer: config detection is not re-run against the tree, and a base revision
    // whose `globals` genuinely differed shows up as extra new findings (visible, fail-closed).
    let ref_baseline: Option<modes::Baseline> = match &args.baseline_from_ref {
        Some(revision) => {
            let mut base_tests = Vec::new();
            let mut base_causes = IncompleteCauses::default();
            let mut base_roof = memory_roof(&args);
            let mut base_files = 0;
            // One base file is held at a time; the roof stops the read, not only the extraction.
            let unread = modes::revision_sources(
                &config.root,
                revision,
                &config.include,
                &config.exclude,
                |source| {
                    base_files += 1;
                    let Some((language, module_flavor)) =
                        discovery::classify_source_file(&source.relative)
                    else {
                        return Ok(true);
                    };
                    let base_file = discovery::SourceFile::without_project_context(
                        config.root.join(&source.relative),
                        language,
                        module_flavor,
                    );
                    let mut extraction = frontend::extract_from_revision(
                        &source.content,
                        &base_file,
                        resolved_globals,
                    );
                    if extraction.incomplete {
                        base_causes.files_partially_read += 1;
                    }
                    relativize_extraction_paths(&mut extraction.tests, &config.root);
                    base_roof.charge(&extraction.tests, 0);
                    base_tests.extend(extraction.tests);
                    Ok(!base_roof.exceeded_with_rss(&base_tests, &mut resident))
                },
            )?;
            if unread > 0 {
                base_causes.files_over_memory_budget = unread;
                write_roof_diagnostic(&mut stderr, "base revision", &args, base_files, unread)?;
            }
            let base_result = analysis::analyze(base_tests, &config, base_files, base_causes)?;
            // A base run that could not see the whole tree (oversized, non-UTF-8, or skipped
            // files) produces a PARTIAL baseline: it accepts less than the base really had, so
            // the gate reports findings as new that the base would have accepted. The safe
            // direction for a gate, but only if it is visible -- same principle as refusing
            // `--update-baseline` on an incomplete run, expressed as a warning because the file
            // baseline's refusal has no equivalent here: there is nothing to refuse, the
            // baseline already is what it is.
            if base_result.incomplete() {
                writeln!(
                    stderr,
                    "vitest-dedup: warning: the {} analysis was incomplete, so the \
                     baseline may be partial and the gate stricter than intended.",
                    terminal_safe(revision)
                )?;
            }
            let baseline = modes::baseline_from_findings(&base_result.findings);
            writeln!(stderr,
                "vitest-dedup: baseline from {}: {} finding(s) in the base revision are treated as accepted.",
                terminal_safe(revision),
                baseline.total()
            )?;
            Some(baseline)
        }
        None => None,
    };

    let parsing_started = metrics_enabled.then(Instant::now);

    // Each cause is counted where it is detected, and `discovered.incomplete` is NOT one cause
    // -- it is the aggregate of several. Reading it as `discovery_failed` printed "directory
    // traversal failed, so files may be missing from this run entirely" for a run whose only
    // problem was one oversized file, which is the same misattribution this struct exists to
    // remove, one layer down. Discovery now says which, and an oversized file is counted from
    // `skipped` (it never reaches the read below, so the read-time counter never sees it).
    let mut causes = IncompleteCauses {
        discovery_failed: discovered.traversal_failed,
        project_config_degraded: project_map_incomplete || discovered.project_config_degraded,
        files_too_large: discovered
            .skipped
            .iter()
            .filter(|file| matches!(file.reason, discovery::SkipReason::TooLarge { .. }))
            .count(),
        ..IncompleteCauses::default()
    };
    let mut tests = Vec::new();
    let mut excluded = ExclusionCollector::default();
    let mut diagnostics: Vec<Sanitized> = Vec::new();
    // Distinct from `diagnostics.len()`: one file can push several diagnostics (one per
    // extraction-time issue), so the diagnostic count and the affected-file count disagree in
    // general and neither may stand in for the other. Discovery-level traversal errors (below)
    // are not tied to one file at all, so they are never added here.
    let mut diagnostic_files: BTreeSet<PathBuf> = BTreeSet::new();
    // Files whose zero-test extraction is a *proven* answer, not a sign of a missed `globals`
    // setting: every declaration the frontend found in them was a statically empty `.each`/`.for`
    // table (`FileExtraction::proven_empty`'s doc). Used only to decide what the "extracted no
    // tests" guard below should say -- never to suppress it or to affect `incomplete`.
    let mut proven_empty_files: BTreeSet<PathBuf> = BTreeSet::new();
    // Counted separately from `discovered.files`: a file that could not be read never reached the
    // frontend, so reporting it as analyzed overstates coverage on exactly the runs where coverage
    // matters most.
    let mut files_read = 0usize;
    let mut read_errors = 0usize;
    // Discovery sets `incomplete` for files it refused to read (oversized, unsupported language).
    // Those reasons were previously dropped, leaving the flag with no explanation behind it.
    for skipped in &discovered.skipped {
        // `skipped.path` is a repository-controlled filename; sanitize it before it becomes
        // part of a diagnostic line this run later prints to stderr.
        // Both halves are repository-derived. The reason's `Debug` rendering can carry project
        // names and paths straight from the analyzed tree, so sanitizing only the path left the
        // other half able to forge or visually reorder a diagnostic line. Sanitize the complete
        // formatted string rather than one field of it.
        // S12: relativized against the analysis root like every `locations[].path`, so a
        // diagnostic line is portable across machines and CI checkouts.
        diagnostics.push(Sanitized::new(&format!(
            "{}: {}",
            display_relative(&skipped.path, &config.root),
            skipped.reason
        )));
        diagnostic_files.insert(skipped.path.clone());
        // A file no project governs, or in an unsupported language, is a deliberate
        // classification that stays a diagnostic (documented as not incomplete); it is listed
        // only when the skip stands on project configuration that could not be read.
        let reason = match skipped.reason {
            discovery::SkipReason::TooLarge { .. } => Some(ExclusionReason::FileTooLarge),
            discovery::SkipReason::NotInVitestProject { .. } if skipped.degraded => {
                Some(ExclusionReason::ProjectConfigUnreadable)
            }
            _ => None,
        };
        if let Some(reason) = reason {
            excluded.record(exclusions::unparsed_file(
                relativize_test_path(&skipped.path, &config.root),
                reason,
                &skipped.reason.to_string(),
            ));
        }
    }
    // Non-fatal filesystem traversal errors (a directory that could not be listed, metadata that
    // could not be read) were previously dropped entirely -- discovery already flips `incomplete`
    // for them, but a run degraded for a reason nobody can see is as unexplained as one with the
    // reason silently discarded. These are not attributable to one file, so they are not counted
    // in `diagnostic_files`.
    for error in &discovered.errors {
        // These messages are built from filesystem paths discovery could not fully read;
        // sanitize the same way every other diagnostic line is.
        diagnostics.push(Sanitized::new(error));
    }

    // Participant snippets for the HTML report (P1): when that reporter is selected, retain the
    // exact `String` handed to the frontend for every file that parses, keyed by the same
    // relativized path `Finding::locations` carry. This is the only retention of source text in
    // the pipeline, it exists only for report runs, and it dies with the run -- the default
    // pipeline's memory profile is untouched, and a snippet sliced from this map is by
    // construction the text the analyzer compared (never a later re-read that could have
    // drifted). See `HtmlReporter::with_sources`.
    let retain_sources = args
        .reporters
        .iter()
        .any(|r| matches!(r, ReporterKind::Html));
    let mut retained_sources: std::collections::BTreeMap<std::path::PathBuf, String> =
        std::collections::BTreeMap::new();
    // Inline suppression directives (P3): scanned from the same text extraction parses, so a
    // directive and the finding it suppresses always come from the same view of the file.
    // Kept as a Vec even when empty -- `RunModes` borrows it either way.
    let mut inline_suppressions: Vec<modes::InlineSuppression> = Vec::new();

    let mut seen_helper_sources = std::collections::BTreeSet::new();
    let mut roof = memory_roof(&args);
    for (index, file) in discovered.files.iter().enumerate() {
        // Source text this file adds to `retained_sources`, charged to the memory roof.
        let mut retained_text = 0;
        // A single unreadable file must degrade this file, not the run. `read_to_string` fails on
        // any non-UTF-8 byte, and propagating that aborted the whole analysis with exit 2 and no
        // report at all -- losing every finding from every other file, and not even naming the
        // culprit. Reporting it as incomplete matches how discovery already handles an oversized
        // file, and honours the rule that degradation stays visible.
        // Discovery checked `metadata.len()`, but that snapshot goes stale: a file can grow or be
        // replaced between discovery and this read, so an unbounded read would allocate the whole
        // thing regardless of the limit. Read through a bounded reader instead.
        let source = match discovery::read_bounded(&file.path, discovery::MAX_SOURCE_FILE_BYTES) {
            Ok(Some(source)) => source,
            Ok(None) => {
                // `file.path` is a repository-controlled filename: sanitize before printing.
                diagnostics::write_read_error(
                    &mut stderr,
                    &mut read_errors,
                    &format!(
                        "skipping {}: exceeds the {} byte limit",
                        display_relative(&file.path, &config.root),
                        discovery::MAX_SOURCE_FILE_BYTES,
                    ),
                )?;
                causes.files_too_large += 1;
                excluded.record(exclusions::unparsed_file(
                    relativize_test_path(&file.path, &config.root),
                    ExclusionReason::FileTooLarge,
                    "the file exceeds the source-size limit and was not read",
                ));
                continue;
            }
            Err(error) => {
                diagnostics::write_read_error(
                    &mut stderr,
                    &mut read_errors,
                    &format!(
                        "skipping {}: {error}",
                        display_relative(&file.path, &config.root),
                    ),
                )?;
                causes.files_unreadable += 1;
                excluded.record(exclusions::unparsed_file(
                    relativize_test_path(&file.path, &config.root),
                    ExclusionReason::Unreadable,
                    &format!("the file could not be read: {error}"),
                ));
                continue;
            }
        };
        files_read += 1;
        if retain_sources {
            // Clone, not move: `extract` parses this same `source` below, and the byte spans in
            // every `Location` it produces index into exactly these bytes. Charged only when new:
            // a test file already retained as a helper's source holds one copy.
            let previous = retained_sources.insert(
                relativize_test_path(&file.path, &config.root),
                source.clone(),
            );
            if previous.is_none() {
                retained_text += source.len();
            }
        }
        inline_suppressions.extend(modes::scan_inline_suppressions(
            &source,
            relativize_test_path(&file.path, &config.root),
        ));
        // `resolved_globals` was already decided above: an explicit `--globals` wins outright,
        // otherwise it is whatever the Vite/Vitest config detector determined (falling back to
        // `Disabled` when detection was inconclusive). Every file in this run shares that one
        // answer -- detection is a property of the project, not of an individual file.
        let globals = resolved_globals;
        // Per file, not per run: two projects in one repository can disagree about `globals`,
        // and applying one project's answer to another's files is how ambient `describe` in a
        // Jest or Playwright suite becomes a finding.
        let (file_globals, unresolved_globals) = if args.globals {
            (Globals::Enabled, None)
        } else if let Some(detection) = &file.project_globals {
            // Discovery already pinned down which project admitted this file via its
            // `includeSource` glob (`discovery::SourceFile::project_globals`'s doc comment): an
            // in-source candidate is ordinary application source and never matches any project's
            // `include`, so `projects.globals_for` below would find no governing project and fall
            // back to the repository-wide `globals` answer -- in a `test.projects` monorepo that
            // fallback is often `Unknown`/`Disabled`, silently analyzing a `globals: true`
            // project's in-source tests as if they were not Vitest tests at all. Using the
            // admitting project's own answer directly closes that gap.
            match detection {
                GlobalsDetection::Enabled { .. } => (Globals::Enabled, None),
                GlobalsDetection::Disabled { .. } => (Globals::Disabled, None),
                // The admitting project's own `globals` could not be read. Unlike the "no
                // governing project" fallback below, this file *was* attributed to a specific
                // project, just not confidently -- silently borrowing the repository-wide answer
                // here would recreate the very silent-omission failure `includeSource` support
                // was supposed to close. Mark the run incomplete instead of guessing.
                _ => (
                    Globals::Disabled,
                    Some(
                        "matched an `includeSource` project whose `globals` could not be \
                         determined; analyzed as `globals: false`, so an in-source test here may \
                         have been missed",
                    ),
                ),
            }
        } else {
            // `ProjectSet::globals_for` collapses "no project governs this file" and "a project
            // governs this file, or might, but the answer could not be determined" into the same
            // `GlobalsDetection::Unknown` -- fine for a caller that treats both alike, but the two
            // deserve opposite treatment here. The first is ordinary: nothing project-specific
            // applies, so the repository-wide answer is exactly right. The second is the same
            // silent-omission hazard as the `file.project_globals` arm above -- a project *does*
            // (or might) govern this file, just not confidently -- so it must fail closed instead
            // of quietly borrowing the repository-wide answer. `governed_globals_for` keeps the
            // two distinguishable; `globals_for` cannot.
            match projects.governed_globals_for(&file.path) {
                vitest_projects::GovernedGlobals::Enabled => (Globals::Enabled, None),
                vitest_projects::GovernedGlobals::Disabled => (Globals::Disabled, None),
                vitest_projects::GovernedGlobals::NotIncluded => (globals, None),
                // A project governs this file (or might), but its own `globals`, or the
                // `include`/`exclude` that would confirm which project governs, could not be
                // read, and no unanimous-answer shortcut saved it. Sibling of the
                // `file.project_globals` arm above: mark the run incomplete instead of guessing.
                _ => (
                    Globals::Disabled,
                    Some(
                        "matched a `test.projects` entry whose `globals` could not be \
                         determined; analyzed as `globals: false`, so ambient tests here may \
                         have been missed",
                    ),
                ),
            }
        };
        // One tail for both fail-closed arms, and the reason it is shared rather than spelled
        // out twice. The two arms record *identical* bookkeeping -- diagnostic, diagnostic file,
        // incompleteness counter -- and differ only in which config channel admitted the file,
        // which is a difference in wording, not in behaviour. Written out per arm they were a
        // nine-line clone that put production Rust over its duplication ceiling (0.62% -> 0.66%
        // against 0.63%), and, worse, they were the exact shape S12 is about: the relativization
        // was applied to one arm and not the other, and nothing could notice. Sharing the tail
        // makes "fail closed and say why" one decision that cannot be half-applied.
        if let Some(reason) = unresolved_globals {
            diagnostics.push(Sanitized::new(&format!(
                "{}: {reason}",
                diagnostic_path(&file.path, &config.root)
            )));
            diagnostic_files.insert(file.path.clone());
            causes.files_globals_undetermined += 1;
        }
        let mut extraction = frontend::extract_within(&source, file, file_globals, &config.root);
        if extraction.incomplete {
            causes.files_partially_read += 1;
        }
        for (path, helper_source) in std::mem::take(&mut extraction.helper_sources) {
            if !seen_helper_sources.insert(path.clone()) {
                continue;
            }
            let relative = relativize_test_path(&path, &config.root);
            inline_suppressions.extend(modes::scan_inline_suppressions(
                &helper_source,
                relative.clone(),
            ));
            if retain_sources {
                let length = helper_source.len();
                if retained_sources.insert(relative, helper_source).is_none() {
                    retained_text += length;
                }
            }
        }
        if extraction.proven_empty {
            proven_empty_files.insert(file.path.clone());
        }
        // The frontend explains *why* it could not fully read a file. Computing that and
        // discarding it left users with a bare "INCOMPLETE" and no way to tell one odd file from
        // half the corpus -- which makes the degradation visible in name only.
        let mut file_reasons: Vec<(ExclusionReason, String)> = extraction
            .reasons
            .iter()
            .copied()
            .zip(extraction.diagnostics.iter().cloned())
            .collect();
        file_reasons.append(&mut extraction.further_refusals);
        if let Some(reason) = unresolved_globals {
            file_reasons.push((ExclusionReason::GlobalsUndetermined, reason.to_owned()));
        }
        excluded.extend(exclusions::parsed_file(
            relativize_test_path(&file.path, &config.root),
            &extraction.tests,
            file_reasons,
        ));
        for diagnostic in extraction.diagnostics.drain(..) {
            // Both the filename and the frontend-produced diagnostic text can carry
            // repository-controlled content; sanitize the whole line.
            diagnostics.push(Sanitized::new(&format!(
                "{}: {diagnostic}",
                display_relative(&file.path, &config.root)
            )));
            diagnostic_files.insert(file.path.clone());
        }
        // `extract` receives one file at a time and has no analysis root to measure against, so it
        // reports absolute paths. Relativizing here keeps reports portable and diffable across
        // machines and CI checkouts, where an absolute path is noise that differs every run.
        //
        // A `TestCase` carries *two* kinds of `Location`, and for a long time only the first was
        // rewritten here: its own declaration site, and one per [`Hook`] in `context.hooks`. The
        // hook locations are not an internal detail -- they escape this function through two
        // published surfaces, both of which reported checkout-specific absolute paths until this
        // loop covered them:
        //
        // - `analysis::hook_fingerprints` copies `hook.location` verbatim into the `"hook"`
        //   records `--emit-test-fingerprints` emits, which the isolation gate consumes and
        //   compares across machines.
        // - `analysis::emit_redundant_hook` copies it into `Finding::locations`, so every
        //   `redundant-hook` finding -- in every reporter, and in the baseline fingerprint that
        //   `Finding::fingerprint` derives from those paths -- named an absolute path where every
        //   other rule named a relative one.
        //
        // Relativizing both here, at the single point where the analysis root is known, keeps the
        // two location families from ever disagreeing about what a reported path means.
        relativize_extraction_paths(&mut extraction.tests, &config.root);
        roof.charge(&extraction.tests, retained_text);
        tests.extend(extraction.tests);
        // Whole files only: stopping here keeps every per-file side effect above consistent, so
        // the run is exactly a run over the files read so far.
        let remaining = discovered.files.len() - index - 1;
        if remaining > 0 && roof.exceeded_with_rss(&tests, &mut resident) {
            causes.files_over_memory_budget = remaining;
            for skipped in &discovered.files[index + 1..] {
                excluded.record(exclusions::unparsed_file(
                    relativize_test_path(&skipped.path, &config.root),
                    ExclusionReason::MemoryBudget,
                    "the memory budget was reached before this file was analyzed",
                ));
            }
            write_roof_diagnostic(&mut stderr, "analysis", &args, files_read, remaining)?;
            break;
        }
    }

    drop(seen_helper_sources);

    diagnostics::write_read_omissions(&mut stderr, read_errors)?;

    // The worst failure this tool had: a `globals: true` project analyzed without the flag yielded
    // zero tests and an otherwise clean report, which was indistinguishable from success. Discovery
    // found test-shaped files, so extracting nothing from all of them is never a normal outcome.
    //
    // Guarded on `files_read > 0`: if every discovered file failed to read, extraction never even
    // ran, so blaming `globals` here would send someone chasing the wrong fix. That case already
    // has its own explanation in `diagnostics` (one entry per unreadable file), so this message
    // only needs to name the different, unrelated failure mode it actually diagnoses.
    // One canonical definition drives the diagnostic, the machine `testsAnalyzed` field, and the
    // exit code. `tests.len()` also includes `describe`/`suite` containers, which do not execute
    // and therefore cannot turn a zero-test run into a quality result.
    let total_tests = tests
        .iter()
        .filter(|test| analysis::is_executable_declaration(test.kind))
        .count();

    if discovered.files.is_empty() {
        // S2 (2026-09-27 scan): four repositories analysed zero files and returned
        // `incomplete: false` with every cause flag clear, because the guard below keyed on
        // "files were found but yielded nothing". A run that measured nothing must not report
        // success -- this branch names the state and sets the cause a JSON/CI consumer reads.
        // Deliberately not guessing a single culprit: an `include` glob matching nothing and a
        // root holding no tests produce the same observation, and the diagnostics below carry
        // anything more specific discovery knows.
        writeln!(
            stderr,
            "\nvitest-dedup: discovered no test file(s) under the analysis root.\n\
             This is not a clean result -- nothing was analyzed. The usual causes are an \
             `include` glob that matches no files, or a root that holds no tests."
        )?;
        causes.nothing_read = true;
    } else if total_tests == 0 {
        if files_read > 0 {
            // Vitest itself refuses a file whose only declarations are statically empty
            // `.each`/`.for` tables ("No test suite found in file"), so a run that discovered
            // nothing from a set of files entirely explained by that shape is not the ambient-
            // globals failure this guard otherwise exists to catch (see this `if`'s outer
            // comment) -- `test.globals` had nothing to do with it, and recommending `--globals`
            // sends someone chasing a fix that cannot work. Only said when *every* read file is
            // accounted for this way: one ordinary file mixed in still means the usual `globals`
            // explanation applies to at least that file, and must not be suppressed by another
            // file's unrelated, fully-explained emptiness.
            if proven_empty_files.len() == files_read {
                writeln!(
                    stderr,
                    "\nvitest-dedup: discovered {} test file(s) but extracted no tests.\n\
                     This is not a clean result -- nothing was analyzed. But this is not the \
                     usual `test.globals` failure: every declaration found is a statically \
                     empty `.each`/`.for` table (`test.each([])`, `describe.each([])`, ...), \
                     which Vitest itself runs zero times -- it reports \"No test suite found in \
                     file\" for exactly this shape, not a clean pass. Affected file(s):",
                    discovered.files.len(),
                )?;
                diagnostics::write_empty_files(&mut stderr, &proven_empty_files, &config.root)?;
            } else if !tests.is_empty() {
                writeln!(
                    stderr,
                    "\nvitest-dedup: discovered {} test file(s) but extracted only suite \
                     containers and no executable tests.\n\
                     This is not a clean result -- nothing was analyzed. Add an executable \
                     `test`/`it`/`bench` declaration, or check why the suite's expected children \
                     were not present in the source this static analyzer reads.",
                    discovered.files.len()
                )?;
            } else {
                writeln!(
                    stderr,
                    "\nvitest-dedup: discovered {} test file(s) but extracted no tests.\n\
                     This is not a clean result -- nothing was analyzed. The analyzer extracted \
                     no tests from the files that were read. Check whether the project \
                     enables `test.globals = true` (or re-run with --globals), and whether a \
                     build-time transform such as `unplugin-auto-import` injects Vitest APIs \
                     that are absent from the source this static analyzer reads.",
                    discovered.files.len()
                )?;
            }
        } else {
            writeln!(
                stderr,
                "\nvitest-dedup: discovered {} test file(s) but could not read any of them.\n\
                 This is not a clean result -- nothing was analyzed. See the diagnostic(s) below \
                 for why each file could not be read.",
                discovered.files.len()
            )?;
        }
        causes.nothing_read = true;
    }

    let unresolved_tests = tests
        .iter()
        .filter(|test| {
            analysis::is_executable_declaration(test.kind) && !test.context.is_comparable()
        })
        .count();

    // Computed here, before `analyze` takes ownership of `tests`, and only when asked for. The
    // records need `alpha`, `structural`, and `behavior`, all of which a default run already
    // retains for the rules -- `Fingerprints` no longer carries an `exact` field for anything to
    // avoid touching. Nothing is computed at all without the flag: one record per test is
    // proportional to the corpus rather than to the findings, which is exactly what would make
    // this retention worth avoiding when nobody asked for it.
    // Measured here, for the same reason and at the same place the fingerprint records are
    // computed: this is the last point at which the whole `Vec<TestCase>` is still live and owned
    // by this function. One line later `analyze` takes it, and `AnalysisResult` retains none of
    // it -- so `post-extraction` is not merely a convenient phase to measure, it is the phase the
    // per-test budget is about, and the working set the analysis then runs over.
    //
    // Peak RSS is deliberately *not* sampled here -- it is stamped on at report time instead, by
    // `with_peak_rss` below. A high-water mark read in the middle of a run is not the run's peak,
    // and a field named `peakRssBytes` carrying something other than the run's peak is the wrong
    // number under its own name. (Nothing here orders it against the retained figure: peak is a
    // resident-page high-water mark and retained is a sum of reserved capacity, so neither bounds
    // the other -- see `MemoryReport::peak_rss_bytes`.)
    //
    // Costs nothing when the flag is off: one walk of the corpus is proportional to the corpus,
    // which is exactly what a default run should not pay for an answer nobody asked for.
    let memory_reports = if args.debug_memory {
        vec![vitest_dedup::memory::MemoryReport::post_extraction(
            &tests,
            total_tests,
            files_read,
            None,
        )]
    } else {
        Vec::new()
    };

    let test_fingerprints = if args.emit_test_fingerprints {
        analysis::test_fingerprints(&tests)
    } else {
        Vec::new()
    };
    // Same reasoning, same flag: `hook_fingerprints` reads `Fingerprints::alpha/structural/
    // behavior/trivial` off each test's `Context::hooks`, and costs nothing when the flag is off.
    let hook_fingerprints = if args.emit_test_fingerprints {
        analysis::hook_fingerprints(&tests)
    } else {
        Vec::new()
    };

    let parsing_time = parsing_started.map(|started| started.elapsed());
    let analysis_started = metrics_enabled.then(Instant::now);
    let mut result = analysis::analyze_with_modes(
        tests,
        &config,
        files_read,
        causes,
        analysis::RunModes {
            changed_files: changed_files.as_ref(),
            // A file baseline and a ref baseline are mutually exclusive by the CLI grammar; one
            // of these is always `None`.
            baseline: baseline.as_ref().or(ref_baseline.as_ref()),
            suppressions: &inline_suppressions,
        },
    )?;

    (result.excluded_files, result.exclusions) = excluded.finish();

    let timing = discovery_time.zip(parsing_time).zip(analysis_started).map(
        |((discovery, parsing), started)| {
            DetectionTiming::new(discovery, parsing, started.elapsed())
        },
    );

    // `--update-baseline` records this run as the accepted state. Everything it records is
    // therefore baselined by definition, which is what the reports below should show.
    if args.update_baseline {
        let path = args
            .baseline
            .as_ref()
            .expect("clap requires --baseline alongside --update-baseline");
        // A baseline written from a run that could not see the whole corpus records a partial
        // picture as "accepted", and everything the run missed is then silently accepted the first
        // time a complete run finds it. Refusing outright -- an operational error, not a warning
        // buried in a stream the caller may not read -- is the only outcome that cannot quietly
        // bake a coverage gap into a committed file.
        if result.incomplete() {
            anyhow::bail!(
                "refusing to write baseline {}: this analysis is incomplete, so it does not know \
                 what it would be accepting. Fix the reported diagnostics (or re-run without \
                 --update-baseline to see them) and try again.",
                path.display()
            );
        }
        let update = modes::write_baseline(path, &result.findings)?;
        writeln!(
            stderr,
            "vitest-dedup: wrote baseline {}: {} finding(s) recorded ({} added, {} removed).",
            Sanitized::new(&path.display().to_string()),
            update.recorded,
            update.added,
            update.removed
        )?;
        modes::mark_all_baselined(&mut result);
    }
    let result = result;

    // A file reporter needs somewhere to write even when `--output` is omitted. Defaulting keeps
    // `--reporters html` useful on its own instead of erroring on a missing flag.
    let prefix = args
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from("reports/vitest-dedup"));

    // Built once, outside the loop, because it owns the records rather than reading them off the
    // shared `AnalysisResult`. With the flag off this is exactly `JsonlReporter::new()`: an empty
    // record list, and therefore the same bytes this reporter has always written.
    // The peak RSS high-water mark is stamped here, at the latest point before any byte is
    // written, so it covers discovery, parsing, extraction, and analysis rather than only the
    // phase each record measures. Reading it once and giving every record the same value is
    // correct by construction: it is a run-level fact, and `getrusage` reports a maximum, not an
    // instantaneous reading, so a later sample can never be smaller than an earlier one.
    let peak_rss = args.debug_memory.then(rss::peak_bytes).flatten();
    let memory_reports: Vec<_> = memory_reports
        .into_iter()
        .map(|report| report.with_peak_rss(peak_rss))
        .collect();
    let jsonl_reporter = JsonlReporter::with_fingerprints(hook_fingerprints, test_fingerprints)
        .with_memory(memory_reports);

    let exit_code = final_exit_code(args.fail_on_incomplete, args.fail_on_new, &result, &config);
    let terminal_context = TerminalReportContext::new(&config, exit_code).with_timing(timing);
    let mut stdout = std::io::stdout().lock();
    let mut written = Vec::new();
    for reporter in &args.reporters {
        match reporter {
            ReporterKind::Terminal => {
                let color = terminal_color_enabled(
                    stdout.is_terminal(),
                    std::env::var_os("NO_COLOR").as_deref(),
                    enable_stdout_ansi,
                );
                TerminalReporter { no_color: !color }.write_report_with_context(
                    &result,
                    &terminal_context,
                    &mut stdout,
                )?;
            }
            ReporterKind::Jsonl => jsonl_reporter.write_report(&result, &mut stdout)?,
            ReporterKind::Json | ReporterKind::Html | ReporterKind::Sarif => {
                let extension = reporter
                    .extension()
                    .expect("file reporters declare an extension");
                let path = report_path(&prefix, extension);
                if let Some(parent) = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("creating report directory {}", parent.display())
                    })?;
                }
                vitest_dedup::atomic_write::write(&path, |file| match reporter {
                    ReporterKind::Json => JsonReporter::new().write_report(&result, file),
                    ReporterKind::Html => {
                        HtmlReporter::with_sources(std::mem::take(&mut retained_sources))
                            .write_report(&result, file)
                    }
                    ReporterKind::Sarif => SarifReporter::new().write_report(&result, file),
                    _ => unreachable!("only file reporters reach this arm"),
                })
                .with_context(|| format!("writing report {}", path.display()))?;
                written.push(path);
            }
        }
    }

    // A run that produced only files would otherwise say nothing at all, leaving the caller to
    // guess where its report went. Announcing paths on stdout is safe here because this branch is
    // only reached when no stdout reporter ran.
    if !written.is_empty() && !args.reporters.iter().any(|r| r.writes_to_stdout()) {
        diagnostics::write_reports_written(&mut stdout, &written)?;
    }
    stdout.flush()?;

    let census = frontend::take_cell_census();
    diagnostics::write_run(
        &mut stderr,
        &diagnostics::RunDiagnostics {
            result: &result,
            unresolved_tests,
            total_tests,
            changed: args
                .changed_since
                .as_deref()
                .zip(changed_files.as_ref().map(BTreeSet::len)),
            baseline: args.baseline.as_deref().filter(|_| !args.update_baseline),
            diagnostics: &diagnostics,
            files_with_diagnostics: diagnostic_files.len(),
            census: &census,
        },
    )?;

    Ok(exit_code)
}

/// Determines the final process exit code for a completed run, applying `--fail-on-incomplete` on
/// top of the quality-gate decision.
///
/// An incomplete analysis under `--fail-on-incomplete` is an *operational* failure
/// ([`ExitCode::OperationalError`]), not a quality-gate failure: the corpus did not fail the
/// configured rules, the analyzer failed to fully read it. Returning
/// [`ExitCode::QualityGateFailed`] here (as an earlier version of this function did) made
/// incomplete coverage indistinguishable from a real duplicate-test finding, which is exactly the
/// distinction `cli.rs`'s [`ExitCode`] contract exists to preserve -- see its doc comment.
///
/// `--fail-on-new` is the opposite case and is deliberately [`ExitCode::QualityGateFailed`]: the
/// analyzer ran to completion and the corpus carries more duplication than the baseline accounts
/// for. That is a policy outcome, exactly like exceeding `--threshold`, and a CI pipeline routing
/// "the analyzer broke" and "you added duplicate tests" to the same place cannot act on either.
///
/// A run with zero executable tests has no quality result and is therefore operational code 2,
/// unless it still carries a real suite-level quality failure. `--fail-on-new` and
/// [`analysis::exit_code`] are consequently checked inside that branch before the empty run is
/// classified as operational. For a non-empty run, their relative order is immaterial because
/// both return [`ExitCode::QualityGateFailed`].
fn final_exit_code(
    fail_on_incomplete: Option<vitest_dedup::cli::FailOnIncomplete>,
    fail_on_new: Option<usize>,
    result: &AnalysisResult,
    config: &Config,
) -> ExitCode {
    // C10: the flag counts a class of causes, not every cause blindly. `None` is the flag
    // absent (off). `Reads` and `Limits` are the two responses a pipeline can automate --
    // fix the repository, or raise this tool's budgets -- and `Any` keeps the original
    // single-bool meaning, deliberate run-mode policies (changed-file withholding) included.
    if let Some(class) = fail_on_incomplete {
        let incomplete_for_this_flag = match class {
            vitest_dedup::cli::FailOnIncomplete::Any => result.incomplete(),
            vitest_dedup::cli::FailOnIncomplete::Reads => result.incomplete_causes.failed_reads(),
            vitest_dedup::cli::FailOnIncomplete::Limits => result.incomplete_causes.hit_limits(),
        };
        if incomplete_for_this_flag {
            return ExitCode::OperationalError;
        }
    }
    // A run that measured no executable tests ordinarily has no quality result to report. Suite
    // containers are the exception: `duplicate-suite` can legitimately report them while this
    // count remains zero. Preserve a real quality failure (including `--fail-on-new`) before
    // classifying a finding-free/Warning-only empty measurement as operational; the ordering for
    // every non-empty run remains unchanged below.
    if result.tests_analyzed == 0 {
        if fail_on_new.is_some_and(|limit| result.modes.new_findings > limit) {
            return ExitCode::QualityGateFailed;
        }
        let quality_exit = analysis::exit_code(result, config);
        if quality_exit != ExitCode::Success {
            return quality_exit;
        }
        return ExitCode::OperationalError;
    }
    if fail_on_new.is_some_and(|limit| result.modes.new_findings > limit) {
        return ExitCode::QualityGateFailed;
    }
    analysis::exit_code(result, config)
}

/// Color requires an eligible TTY and working ANSI support; opt-outs avoid console mutation.
fn terminal_color_enabled(
    is_terminal: bool,
    no_color: Option<&std::ffi::OsStr>,
    enable_ansi: impl FnOnce() -> bool,
) -> bool {
    is_terminal && !no_color.is_some_and(|value| !value.is_empty()) && enable_ansi()
}

/// Enables VT on Windows stdout only, falling back to plain text on any console error.
fn enable_stdout_ansi() -> bool {
    #[cfg(windows)]
    {
        winapi_util::console::Console::stdout()
            .and_then(|mut console| console.set_virtual_terminal_processing(true))
            .is_ok()
    }
    #[cfg(not(windows))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vitest_dedup::config::ConfigOverrides;

    /// Every TTY/NO_COLOR combination follows the same policy as CukeDedup.
    #[test]
    fn terminal_color_requires_tty_and_respects_no_color() {
        use std::ffi::OsStr;
        for (value, allows_color) in [(None, true), (Some(""), true), (Some("0"), false)] {
            assert_eq!(
                terminal_color_enabled(true, value.map(OsStr::new), || true),
                allows_color
            );
            assert!(!terminal_color_enabled(
                false,
                value.map(OsStr::new),
                || true
            ));
        }
    }

    /// A console that cannot process ANSI must use plain output even when it is a TTY.
    #[test]
    fn terminal_color_falls_back_when_ansi_cannot_be_enabled() {
        use std::ffi::OsStr;
        for is_terminal in [false, true] {
            for (value, allows_color) in [(None, true), (Some(""), true), (Some("1"), false)] {
                for ansi_available in [false, true] {
                    let mut attempted = false;
                    let color = terminal_color_enabled(is_terminal, value.map(OsStr::new), || {
                        attempted = true;
                        ansi_available
                    });
                    assert_eq!(attempted, is_terminal && allows_color);
                    assert_eq!(color, is_terminal && allows_color && ansi_available);
                }
            }
        }
    }

    // -- End-to-end: carrying an `includeSource` project's `globals` through to analysis -----
    //
    // Regression coverage for the defect this file was changed for: `discovery::discover`
    // resolves in-source-testing candidates (`test.includeSource`) against a project's globs,
    // but until this fix that governing project was never carried forward, so `main.rs` resolved
    // `globals` for such a file from path globs alone (`ProjectSet::globals_for`), found no
    // governing project, and silently fell back to the repository-wide answer -- `Disabled` in a
    // `test.projects` monorepo. These tests run the actual binary end to end (not just
    // `discovery::discover`) so a regression in *either* half of the fix (discovery carrying the
    // value, or `main.rs` consuming it) is caught.

    /// An ambient `test(...)` call -- no import, no `import.meta.vitest` destructure -- inside an
    /// `if (import.meta.vitest)` guard. This is deliberately *not*
    /// `fixtures/insource/in-source-basic.ts`'s destructured form: that form binds `test`
    /// explicitly and is recognized as Vitest regardless of `globals`, so it cannot exercise the
    /// defect here. Only an ambient binding is statically indistinguishable from Mocha/Jasmine
    /// without knowing the project's `globals` setting -- which is exactly the fact that was
    /// getting lost.
    const AMBIENT_IN_SOURCE_TEST: &str = r#"
        export function add(a: number, b: number) {
            return a + b;
        }

        if (import.meta.vitest) {
            test('add works', () => {
                expect(add(1, 1)).toBe(2);
            });
        }
    "#;

    /// An ordinary, always-recognized Vitest test (explicit import, no ambient binding). Every
    /// fixture also gets this file so `tests` is never empty on its own: `run()`'s "discovered a
    /// test file but extracted nothing" guard (see the comment above its `incomplete = true`,
    /// a few hundred lines up in this file) would otherwise mark *every* run below incomplete
    /// regardless of `globals`, on the sole strength of a one-file, zero-test corpus -- which
    /// would make these tests unable to tell that guard apart from the specific
    /// `project_globals` behavior they exist to check.
    const ORDINARY_TEST: &str = r#"
        import { test, expect } from 'vitest';

        test('ordinary works', () => {
            expect(1 + 1).toBe(2);
        });
    "#;

    /// Write a Vitest config and companion ordinary/in-source test fixtures beneath root.
    fn write_project(root: &std::path::Path, config_test_body: &str) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("vitest.config.ts"),
            format!("export default defineConfig({{ test: {{ {config_test_body} }} }});"),
        )
        .unwrap();
        std::fs::write(root.join("src/thing.ts"), AMBIENT_IN_SOURCE_TEST).unwrap();
        std::fs::write(root.join("src/companion.test.ts"), ORDINARY_TEST).unwrap();
    }

    /// The `--fail-on-incomplete` re-run the unresolved-globals tests end with: the same
    /// repository, reported to a throwaway prefix, must fail with `OperationalError` -- not the
    /// duplicate-test exit and not a crash.
    fn assert_fail_on_incomplete_is_operational_error(root: &std::path::Path) {
        assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(root)
            .args(["--reporters", "json", "--output"])
            .arg(root.join("report2"))
            .arg("--fail-on-incomplete")
            .assert()
            .failure()
            .code(ExitCode::OperationalError.code());
    }

    /// [`run_json`] for the tests that also assert on the diagnostic text: the same
    /// JSON-to-file report, returning stderr alongside the parsed report.
    fn run_json_with_stderr(
        root: &std::path::Path,
        expected: ExitCode,
    ) -> (String, serde_json::Value) {
        let prefix = root.join("report").into_os_string().into_string().unwrap();
        let (code, _, stderr) = run_cli(root, &["--reporters", "json", "--output", &prefix]);
        assert_eq!(
            code,
            Some(expected.code()),
            "the JSON report run must have the expected outcome: {stderr}"
        );
        (stderr, read_json_report(&root.join("report")))
    }

    /// Run the CLI successfully with a JSON output prefix, then read and parse the generated
    /// report.
    fn run_json(root: &std::path::Path, extra_args: &[&str]) -> serde_json::Value {
        let output_prefix = root.join("report");
        // The process outcome is part of what this helper verifies, not just the file it leaves
        // behind: a non-zero exit here would mean the report below is stale (or missing), and the
        // downstream assertions on it would either fail confusingly or -- worse -- pass against
        // leftover state from an earlier run.
        assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(root)
            .args(["--reporters", "json", "--output"])
            .arg(&output_prefix)
            .args(extra_args)
            .assert()
            .success();
        let report = std::fs::read_to_string(root.join("report.json")).unwrap_or_else(|error| {
            panic!(
                "expected {}.json to be written: {error}",
                output_prefix.display()
            )
        });
        serde_json::from_str(&report).unwrap()
    }

    /// Regression fixture for CLI reporting: cell census counts files once and stays off
    /// machine stdout.
    #[test]
    fn cell_census_counts_files_once_and_stays_off_machine_stdout() {
        let directory = tempfile::tempdir().unwrap();
        let source = "import { test } from 'vitest'; const N = mk(); test.each([[N], [2]])('n %s', () => {});";
        for name in ["a.test.ts", "b.test.ts"] {
            std::fs::write(directory.path().join(name), source).unwrap();
        }
        let output = |enabled| {
            let mut command = assert_cmd::Command::cargo_bin("vitest-dedup").unwrap();
            command
                .arg(directory.path())
                .args(["--reporters", "jsonl", "--threshold", "100"]);
            if enabled {
                command.env("VD_CELL_CENSUS", "1");
            } else {
                command.env_remove("VD_CELL_CENSUS");
            }
            command.output().unwrap()
        };
        let enabled = output(true);
        let disabled = output(false);
        assert!(enabled.status.success());
        assert!(disabled.status.success());
        assert_eq!(enabled.stdout, disabled.stdout);
        let stderr = String::from_utf8(enabled.stderr).unwrap();
        assert!(stderr.contains("unreadable `.each` cell census"));
        assert_eq!(
            stderr
                .lines()
                .filter(|line| line.split_whitespace().collect::<Vec<_>>() == ["2", "identifier"])
                .count(),
            1
        );
        assert!(!String::from_utf8(disabled.stderr)
            .unwrap()
            .contains("cell census"));
    }

    /// Regression fixture for CLI reporting: in source file in a globals true project yields
    /// its ambient tests.
    #[test]
    fn in_source_file_in_a_globals_true_project_yields_its_ambient_tests() {
        let directory = tempfile::tempdir().unwrap();
        write_project(
            directory.path(),
            "globals: true, includeSource: ['src/**/*.ts']",
        );

        let report = run_json(directory.path(), &[]);

        // The ambient in-source test plus the always-recognized companion test.
        assert_eq!(
            report["testsAnalyzed"], 2,
            "the ambient in-source test must be extracted under `globals: true`: {report:#?}"
        );
        assert_eq!(report["incomplete"], false);
    }

    /// Regression fixture for CLI reporting: in source file in a globals false project yields
    /// no ambient test.
    #[test]
    fn in_source_file_in_a_globals_false_project_yields_no_ambient_test() {
        // Same file, same `includeSource`, only `globals` differs: the ambient `test(...)` call
        // is correctly *not* recognized as Vitest here, so only the companion test is counted --
        // confirming the fix carries the project's real answer through rather than defaulting
        // every in-source file to `Enabled`.
        let directory = tempfile::tempdir().unwrap();
        write_project(
            directory.path(),
            "globals: false, includeSource: ['src/**/*.ts']",
        );

        let report = run_json(directory.path(), &[]);

        assert_eq!(
            report["testsAnalyzed"], 1,
            "expected only the companion test: {report:#?}"
        );
        assert_eq!(report["incomplete"], false);
    }

    /// Regression fixture for CLI reporting: in source file whose projects globals is unknown
    /// marks the run incomplete.
    #[test]
    fn in_source_file_whose_projects_globals_is_unknown_marks_the_run_incomplete() {
        // The admitting project's own `globals` cannot be read. Falling back to the
        // repository-wide answer here would recreate the silent-omission bug this fix closes --
        // this must surface as incompleteness instead, both in the report and in the exit code
        // under `--fail-on-incomplete`. The companion test keeps `tests` non-empty, so this
        // incompleteness is provably coming from the `project_globals` handling under test, not
        // from `run()`'s generic "zero tests extracted" guard.
        let directory = tempfile::tempdir().unwrap();
        write_project(
            directory.path(),
            "globals: someHelper(), includeSource: ['src/**/*.ts']",
        );

        let report = run_json(directory.path(), &[]);
        assert_eq!(
            report["testsAnalyzed"], 1,
            "the ambient in-source test must not be analyzed under unresolved `globals`: {report:#?}"
        );
        assert_eq!(
            report["incomplete"], true,
            "an in-source file whose governing project's `globals` is unresolved must mark the \
             run incomplete rather than silently falling back: {report:#?}"
        );

        assert_fail_on_incomplete_is_operational_error(directory.path());
    }

    // -- Copilot review #8, item 1: the ordinary (non-`includeSource`) path must fail closed on -
    // -- an unreadable governing project's `globals`, the same way the `includeSource` path      -
    // -- above already does. -----------------------------------------------------------------

    /// An ambient `test(...)` call with no import and no `import.meta.vitest` guard -- an
    /// ordinary test file, not an in-source one, so this exercises the plain `else` branch in
    /// `run()` (`ProjectSet::governed_globals_for`), not the `file.project_globals` arm above.
    const AMBIENT_ORDINARY_TEST: &str = r#"
        test('ambient works', () => {
            expect(1 + 1).toBe(2);
        });
    "#;

    /// A `test.projects` monorepo with one sub-project whose `globals` is a non-literal
    /// expression this analyzer cannot read statically -- `GlobalsDetection::Unknown` -- and
    /// which governs every file under `sub/`.
    fn write_projects_config(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(
            root.join("vitest.config.ts"),
            r#"export default defineConfig({
                test: {
                    projects: [
                        {
                            test: {
                                name: 'sub',
                                include: ['sub/**/*.test.ts'],
                                globals: someHelper(),
                            },
                        },
                    ],
                },
            });"#,
        )
        .unwrap();
        std::fs::write(root.join("sub/ambient.test.ts"), AMBIENT_ORDINARY_TEST).unwrap();
        // Always recognized regardless of `globals` (explicit import), so `tests` is never empty
        // on its own -- same reasoning as `ORDINARY_TEST`'s doc comment above.
        std::fs::write(root.join("sub/companion.test.ts"), ORDINARY_TEST).unwrap();
    }

    /// Regression fixture for CLI reporting: ordinary file whose projects globals is unknown
    /// marks the run incomplete.
    #[test]
    fn ordinary_file_whose_projects_globals_is_unknown_marks_the_run_incomplete() {
        // `sub/ambient.test.ts` is governed by the `sub` project (matched via its `include`
        // glob, not `includeSource`), whose own `globals` cannot be read. Before this fix,
        // `ProjectSet::globals_for`'s `Unknown` fell through `main.rs`'s `_ => globals` catch-all
        // straight to the repository-wide answer, silently dropping the ambient test. It must
        // instead mark the run incomplete and fail under `--fail-on-incomplete`, exactly like the
        // `includeSource` sibling above.
        let directory = tempfile::tempdir().unwrap();
        write_projects_config(directory.path());

        let report = run_json(directory.path(), &[]);
        assert_eq!(
            report["testsAnalyzed"], 1,
            "the ambient test must not be analyzed under unresolved `globals`: {report:#?}"
        );
        assert_eq!(
            report["incomplete"], true,
            "a file whose governing `test.projects` entry has unresolved `globals` must mark \
             the run incomplete rather than silently falling back: {report:#?}"
        );

        assert_fail_on_incomplete_is_operational_error(directory.path());
    }

    /// Regression fixture for CLI reporting: file governed by no project still uses the
    /// repository wide fallback.
    #[test]
    fn file_governed_by_no_project_still_uses_the_repository_wide_fallback() {
        // No `vitest.config.*`/`vite.config.*` at all: `resolve_projects` reports
        // `config_found == false`, so `run()` passes `project_scoping = None` to `discovery::
        // discover` and every file's `ProjectSet::governing_project` is `NotIncluded` -- no
        // project exists to have an unreadable answer, so this is the ordinary case the
        // repository-wide fallback exists for, and must stay complete. This guards against
        // over-correcting the fix above into failing closed on `NotIncluded` too, which would
        // flip every plain, config-less repository to incomplete.
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("thing.test.ts"), ORDINARY_TEST).unwrap();

        let report = run_json(directory.path(), &[]);
        assert_eq!(report["testsAnalyzed"], 1, "{report:#?}");
        assert_eq!(
            report["incomplete"], false,
            "a file governed by no project must fall back to the repository-wide answer, not \
             be treated as a failure to determine `globals`: {report:#?}"
        );
    }

    // -- Copilot review #8, item 2 (revised): the "extracted no tests" guard's *advice* must -----
    // -- name a statically empty `.each`/`.for` table when that is the actual cause, instead of --
    // -- always blaming `test.globals` -- but the guard itself must keep firing (`incomplete`     -
    // -- stays `true`), since Vitest itself refuses such a file ("No test suite found in file").  -

    /// Regression fixture for CLI reporting: statically empty each table explains zero tests
    /// without blaming globals.
    #[test]
    fn statically_empty_each_table_explains_zero_tests_without_blaming_globals() {
        // The exact reproduction from the review: `test.each([])` statically proves zero rows,
        // so extraction correctly contributes nothing for this file -- but Vitest's own verdict on
        // it is still a failure, not a clean pass, so `incomplete` staying `true` is correct. What
        // was wrong before this fix was the *message*: it blamed `test.globals` and recommended
        // `--globals`, which cannot do anything here.
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("empty.test.ts"),
            r#"
                import { test, expect } from 'vitest';
                test.each([])('never runs %s', (value) => {
                    expect(value).toBeDefined();
                });
            "#,
        )
        .unwrap();

        let (stderr, report) = run_json_with_stderr(directory.path(), ExitCode::OperationalError);
        assert!(
            !stderr.contains("--globals"),
            "a statically empty `.each` table has nothing to do with `globals`; that advice must \
             not be shown here: {stderr}"
        );
        assert!(
            stderr.contains("No test suite found in file") && stderr.contains("empty.test.ts"),
            "the message must name the empty-table cause and the file it applies to: {stderr}"
        );

        assert_eq!(report["testsAnalyzed"], 0, "{report:#?}");
        assert_eq!(
            report["incomplete"], true,
            "matches Vitest's own verdict on this file -- this is not a clean run: {report:#?}"
        );
    }

    /// Regression fixture for CLI reporting: a wholly excluded file is named on stderr and
    /// counted in the report.
    #[test]
    fn a_wholly_excluded_file_is_named_on_stderr_and_counted_in_the_report() {
        // The deception this closes: `blind.test.ts` produces no findings, so it appears nowhere
        // in any reporter's output. Without this diagnostic, a reader cannot tell it apart from a
        // file that was compared and came back clean -- and the corpus-wide count cannot help,
        // because it says how MANY tests were excluded, never that they were all of one file.
        //
        // `vi.doMock` is not hoisted, so it taints every test in the file (extract.rs F4): both
        // tests here get an unresolved context, which makes the whole file blind.
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("blind.test.ts"),
            r#"
                import { test, expect, vi } from 'vitest';
                vi.doMock('./service', () => ({ run: () => 1 }));
                test('a', () => { expect(1).toBe(1); });
                test('b', () => { expect(2).toBe(2); });
            "#,
        )
        .unwrap();
        std::fs::write(
            directory.path().join("seen.test.ts"),
            r#"
                import { test, expect } from 'vitest';
                test('c', () => { expect(3).toBe(3); });
            "#,
        )
        .unwrap();

        let (stderr, report) = run_json_with_stderr(directory.path(), ExitCode::Success);
        assert!(
            stderr.contains("blind.test.ts"),
            "a file whose every test was excluded must be NAMED, not merely counted: {stderr}"
        );
        assert!(
            !stderr.contains("seen.test.ts"),
            "a file that was actually compared must not be listed as blind: {stderr}"
        );

        assert_eq!(
            report["blindFiles"], 1,
            "the machine contract must carry the count a gate can act on: {report:#?}"
        );
    }

    /// Regression fixture for CLI reporting: ordinary zero extraction names observation and
    /// both binding sources.
    #[test]
    fn ordinary_zero_extraction_names_observation_and_both_binding_sources() {
        // Contrast with the test above: zero extraction gives no static proof of WHY no test was
        // declared. It may be a project-level globals setting, a build transform may inject the
        // import only at runtime, or a supported binding may simply be unused. The hint must
        // state the observation and preserve both remedies instead of promoting a cause.
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("ambient.test.ts"),
            r#"
                test('ambient, never recognized without globals', () => {
                    expect(1).toBe(1);
                });
            "#,
        )
        .unwrap();
        let (stderr, report) = run_json_with_stderr(directory.path(), ExitCode::OperationalError);
        assert!(
            stderr.contains("The analyzer extracted no tests"),
            "the hint must say what static analysis actually observed: {stderr}"
        );
        assert!(
            stderr.contains("--globals") && stderr.contains("unplugin-auto-import"),
            "the hint must name both supported binding sources to investigate: {stderr}"
        );
        assert!(
            !stderr.contains("The usual cause is"),
            "the hint must not claim an unobserved cause is usual: {stderr}"
        );
        assert!(
            !stderr.contains("No test suite found in file"),
            "the empty-table explanation must not appear when nothing was proven empty: {stderr}"
        );

        assert_eq!(report["testsAnalyzed"], 0, "{report:#?}");
        assert_eq!(report["incomplete"], true, "{report:#?}");
    }

    /// Regression fixture for CLI reporting: imported but empty file is an independent
    /// operational failure.
    #[test]
    fn imported_but_empty_file_is_an_independent_operational_failure() {
        // Kept independent of the ambient-globals fixture so this ordinary supported-import case
        // owns its exit-code contract.
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("imported-empty.test.ts"),
            "import { test } from 'vitest';\n",
        )
        .unwrap();

        let (stderr, report) = run_json_with_stderr(directory.path(), ExitCode::OperationalError);
        assert!(
            stderr.contains("discovered 1 test file(s) but extracted no tests"),
            "the imported-but-empty file must reach extraction, not collapse into the empty-root \
             case: {stderr}"
        );
        assert!(
            stderr.contains("The analyzer extracted no tests"),
            "the diagnostic must state the observed outcome: {stderr}"
        );
        assert_eq!(report["testsAnalyzed"], 0, "{report:#?}");
        assert_eq!(report["incomplete"], true, "{report:#?}");
    }

    /// Pins the exact substring `scripts/check/run-external-corpus.mjs` greps for.
    ///
    /// That gate has no structured surface for the resolved globals mode -- the JSONL summary
    /// carries `filesGlobalsUndetermined`, which counts files whose mode could not be determined
    /// and therefore stays at zero when detection reaches a *confidently wrong* answer. The one
    /// place the run states which mode it actually resolved is this stderr announcement, so the
    /// gate compares its presence against each project's `expectedGlobalsDetection`.
    ///
    /// Matching on prose is what `AnalysisResult::unresolved_tests`' doc comment warns against:
    /// reword the sentence and the gate silently concludes every project derives `globals: false`,
    /// which is a pass. This test is the tripwire that makes such a reword fail here instead.
    ///
    /// Both directions are asserted deliberately. Checking only the `globals: true` arm would let
    /// a detector that announced enablement unconditionally keep this green while destroying the
    /// gate's ability to tell the two modes apart -- the marker has to be a biconditional for the
    /// mode, not merely a string that appears sometimes.
    #[test]
    fn globals_true_announcement_matches_the_string_the_corpus_gate_greps_for() {
        // Keep byte-identical to `GLOBALS_ENABLED_MARKER` in
        // `scripts/check/run-external-corpus.mjs`.
        const MARKER: &str = "sets `test.globals = true`";

        // Same corpus in both projects, differing only in the config's `globals` value, so the
        // marker's presence can be attributed to detection and to nothing else.
        let run_with_config = |config: &str| {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join("vitest.config.ts"), config).unwrap();
            std::fs::write(
                directory.path().join("a.test.ts"),
                r#"
                    import { describe, it, expect } from 'vitest';
                    describe('s', () => {
                        it('a', () => { expect(1).toBe(1); });
                    });
                "#,
            )
            .unwrap();
            let (_, _, stderr) = run_cli(
                directory.path(),
                &[
                    "--reporters",
                    "json",
                    "--output",
                    directory.path().join("report").to_str().unwrap(),
                ],
            );
            stderr
        };

        let enabled = run_with_config("export default { test: { globals: true } }\n");
        assert!(
            enabled.contains(MARKER),
            "a `globals: true` config must announce itself in the exact words the corpus gate \
             matches ({MARKER:?}); rewording it breaks that gate silently: {enabled}"
        );

        let disabled = run_with_config("export default { test: { globals: false } }\n");
        assert!(
            !disabled.contains(MARKER),
            "a `globals: false` config must not print the enablement marker, or the corpus gate \
             cannot tell the two derived modes apart: {disabled}"
        );
    }

    /// A statically readable `mergeConfig` with no `test` object on either side governs no
    /// comparison dimension. It must not partition its test away from an otherwise identical
    /// peer under the analysis root.
    #[test]
    fn a_testless_merge_config_does_not_hide_a_cross_file_duplicate() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("configured")).unwrap();
        std::fs::create_dir_all(root.join("plain")).unwrap();
        std::fs::write(
            root.join("configured/base.ts"),
            "export default { plugins: [] };",
        )
        .unwrap();
        std::fs::write(
            root.join("configured/vitest.config.ts"),
            "import { mergeConfig } from 'vitest/config';\n\
             import base from './base';\n\
             export default mergeConfig(base, { resolve: {} });",
        )
        .unwrap();
        let test_body = |title: &str| {
            format!(
                "import {{ expect, test }} from 'vitest';\n\n\
                 test('{title}', () => {{\n\
                   expect(1 + 1).toBe(2);\n\
                 }});\n"
            )
        };
        std::fs::write(root.join("configured/first.test.ts"), test_body("first")).unwrap();
        std::fs::write(root.join("plain/second.test.ts"), test_body("second")).unwrap();

        let report = run_json(root, &[]);
        assert_eq!(report["testsAnalyzed"], 2, "{report:#?}");
        assert_eq!(report["incomplete"], false, "{report:#?}");
        let findings = report["findings"].as_array().unwrap();
        assert_eq!(
            findings.len(),
            1,
            "the duplicate must not be hidden: {report:#?}"
        );
        assert_eq!(findings[0]["rule"], "cross-file-duplicate");
    }

    /// The ordinary case: a path under `root` is rewritten relative to it.
    /// The invariant `Sanitized` exists to enforce, checked end to end rather than at the type.
    ///
    /// A filename carrying a bidi override and an ANSI escape reaches stderr through the
    /// diagnostics vector (this file is unreadable as UTF-8, so it becomes a skip diagnostic).
    /// Asserting on the process's real stderr -- not on `Sanitized::new` in isolation -- is what
    /// makes this a regression test for the *print* path a reviewer flagged, rather than a
    /// restatement of `terminal_safe`'s own unit tests.
    /// `--help` and `--version` are successful output, not failures.
    ///
    /// clap models both as `Err`, so they used to reach `main`'s error handler: printed to stderr
    /// and exited 2, the code this tool reserves for "the analyzer broke". A shell, a CI step, or
    /// a release smoke test reading `--version` would all have seen a failure. Asserted on the
    /// real process, because the bug was entirely in how the error was routed to the streams.
    #[test]
    fn help_and_version_succeed_on_stdout() {
        for flag in ["--help", "--version"] {
            let output = assert_cmd::Command::cargo_bin("vitest-dedup")
                .expect("binary")
                .arg(flag)
                .output()
                .expect("run");
            assert!(
                output.status.success(),
                "{flag} exited {:?}",
                output.status.code()
            );
            assert!(!output.stdout.is_empty(), "{flag} wrote nothing to stdout");
            assert!(
                output.stderr.is_empty(),
                "{flag} wrote to stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    /// The other half of the contract: a genuine argv error must still be an operational error on
    /// stderr, so intercepting help/version did not turn every parse failure into a success.
    #[test]
    fn an_invalid_flag_value_is_still_an_operational_error_on_stderr() {
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .expect("binary")
            .args([".", "--reporters", "bogus"])
            .output()
            .expect("run");
        assert_eq!(output.status.code(), Some(2), "expected exit 2");
        assert!(output.stdout.is_empty(), "error text leaked to stdout");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("bogus"),
            "the offending value should be named"
        );
    }

    /// Regression fixture for CLI reporting: a hostile filename reaches stderr sanitized.
    #[test]
    fn a_hostile_filename_reaches_stderr_sanitized() {
        let dir = tempfile::tempdir().expect("tempdir");
        // U+202E only, deliberately: Windows rejects control characters such as ESC in a
        // filename outright (`InvalidFilename`), so including one made this test fail on
        // windows-latest for a reason unrelated to what it checks. A bidi override is a legal
        // NTFS filename and proves the same property -- that an untrusted filename reaches
        // stderr stripped. ESC handling itself is covered by `terminal_safe`'s own unit tests.
        let hostile = dir.path().join("a\u{202e}b.test.ts");
        // Invalid UTF-8 makes the file unreadable, which routes its name into a diagnostic.
        std::fs::write(&hostile, [0xff, 0xfe, 0xfd]).expect("write");

        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .expect("binary")
            .arg(dir.path())
            .args(["--reporters", "jsonl"])
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Assert the *stripped* filename is present, not merely that some "a" and "b" occur --
        // an earlier version of this test checked the latter and passed vacuously when the name
        // never reached stderr at all, which is the same silent-success shape it exists to catch.
        assert!(
            stderr.contains("ab.test.ts"),
            "the sanitized filename should appear in stderr: {stderr}"
        );
        assert!(
            !stderr.contains('\u{202e}'),
            "U+202E survived into stderr: {stderr}"
        );
    }

    /// Every path a human reads is spelled with `/`, whatever platform produced it.
    ///
    /// `.display()` renders the platform separator, so stderr said `src\broken.test.ts` on Windows
    /// while every reporter in the same run said `src/broken.test.ts`. That is what failed
    /// `stderr_diagnostic_paths_are_relativized_against_the_analysis_root` on windows-latest and
    /// nowhere else.
    ///
    /// Only the NORMALIZATION is asserted here, not the stripping, and the reason is worth
    /// recording: `std::path` parses separators per platform, so a hand-built `D:\a\repo\...` is a
    /// single component on Unix and `strip_prefix` refuses it. A first version of this test tried
    /// to simulate Windows path parsing on macOS and failed with
    /// `left: "D:/a/repo/src/broken.test.ts"` — the normalization worked, the stripping could not.
    /// Faking the platform is not available; the strip half is covered on windows-latest by
    /// `stderr_diagnostic_paths_are_relativized_against_the_analysis_root`, which is the test that
    /// caught this.
    ///
    /// The case below answers the same portably: a path the root does not prefix comes through
    /// unstripped, so what is left to observe is exactly the separator replacement, and it reads
    /// the same on every platform.
    ///
    /// On Unix a backslash is a legal filename character, so this normalization is lossy there —
    /// deliberately, and not a new decision: `report_safe_path` and `analysis.rs`'s
    /// `forward_slash_key` already make it, the latter's doc explaining that otherwise determinism
    /// becomes platform-dependent. stderr was simply the one surface left out of that convention.
    #[test]
    fn a_displayed_path_uses_forward_slashes_on_every_platform() {
        assert_eq!(
            display_relative(
                std::path::Path::new(r"a\b.test.ts"),
                std::path::Path::new("/elsewhere")
            ),
            "a/b.test.ts",
            "stderr must spell a separator the way every reporter in the same run spells it"
        );
        assert_eq!(
            display_relative(
                std::path::Path::new("/repo/src/thing.test.ts"),
                std::path::Path::new("/repo")
            ),
            "src/thing.test.ts",
            "and the ordinary case still strips the root"
        );
    }

    /// Regression fixture for CLI reporting: relativize test path strips the root for a path
    /// beneath it.
    #[test]
    fn relativize_test_path_strips_the_root_for_a_path_beneath_it() {
        let root = std::path::Path::new("/repo/analysis-root");
        let path = root.join("src/thing.test.ts");
        assert_eq!(
            relativize_test_path(&path, root),
            std::path::PathBuf::from("src/thing.test.ts")
        );
    }

    /// C11: the shapes that CANNOT be stripped to the analysis root, one test per shape. All of
    /// them answer the same way -- `strip_prefix` refuses and the absolute form survives -- but
    /// each shape reaches that refusal through a different platform mechanism, and only Windows
    /// can build the volume-bound ones as real `Path` values:
    ///
    /// - a **symlink escaping the root** (the Unix test above; the frontend reads through the
    ///   link, so the path the frontend carries is the target's);
    /// - a **differing drive letter** -- `D:\data\...` under a `C:\repo` root shares no volume
    ///   prefix, so no relative form exists;
    /// - a **verbatim/ordinary prefix mismatch** -- when both spellings exist and resolve to the
    ///   same file, canonical comparison identifies the path as inside the root; and a UNC share
    ///   root (`\\server\share\repo`) strips only for paths on the same share.
    ///
    /// The platform-neutral test at the end pins the CONTRACT itself on every OS: whatever
    /// `strip_prefix` refuses is forwarded absolute, never dropped, never rewritten to something
    /// that looks relative but is not.
    #[cfg(windows)]
    #[test]
    fn relativize_test_path_keeps_the_absolute_path_for_a_path_on_another_drive() {
        let root = std::path::Path::new(r"C:\repo");
        let path = std::path::Path::new(r"D:\data\thing.test.ts");
        assert_eq!(
            relativize_test_path(path, root),
            std::path::PathBuf::from(r"D:\data\thing.test.ts"),
            "no relative form exists across volumes; the absolute path is the honest answer"
        );
    }

    /// Regression fixture for CLI reporting: relativize test path strips an existing verbatim
    /// prefix mismatch.
    #[cfg(windows)]
    #[test]
    fn relativize_test_path_strips_an_existing_verbatim_prefix_mismatch() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("src")).unwrap();
        let ordinary = root.join("src/thing.test.ts");
        std::fs::write(&ordinary, "// source").unwrap();
        let verbatim = ordinary.canonicalize().unwrap();
        assert_ne!(
            ordinary, verbatim,
            "the fixture needs two Windows spellings"
        );
        assert_eq!(
            relativize_test_path(&verbatim, root),
            std::path::PathBuf::from(r"src\thing.test.ts"),
            "equivalent existing paths must remain root-relative"
        );
    }

    /// Regression fixture for CLI reporting: relativize test path keeps a missing verbatim path
    /// unchanged.
    #[cfg(windows)]
    #[test]
    fn relativize_test_path_keeps_a_missing_verbatim_path_unchanged() {
        let root = std::path::Path::new(r"C:\repo");
        let missing = std::path::Path::new(r"\\?\C:\repo\missing.test.ts");
        assert_eq!(relativize_test_path(missing, root), missing);
    }

    /// Regression fixture for CLI reporting: relativize test path strips a matching unc share
    /// root.
    #[cfg(windows)]
    #[test]
    fn relativize_test_path_strips_a_matching_unc_share_root() {
        let root = std::path::Path::new(r"\\server\share\repo");
        let path = std::path::Path::new(r"\\server\share\repo\src\thing.test.ts");
        assert_eq!(
            relativize_test_path(path, root),
            std::path::PathBuf::from(r"src\thing.test.ts"),
            "a path on the same share strips like any other prefix"
        );
        let other_share = std::path::Path::new(r"\\server\other\thing.test.ts");
        assert_eq!(
            relativize_test_path(other_share, root),
            std::path::PathBuf::from(r"\\server\other\thing.test.ts"),
            "a different share cannot be stripped to this root"
        );
    }

    /// Regression fixture for CLI reporting: relativize test path forwards whatever the
    /// platform cannot strip.
    #[test]
    fn relativize_test_path_forwards_whatever_the_platform_cannot_strip() {
        // Platform-neutral statement of the contract, exercised with the Windows-shaped strings
        // as ordinary (non-separator) bytes on Unix: a path the root does not prefix arrives
        // verbatim. Windows asserts the real semantics above; this asserts the shape of the
        // answer everywhere else.
        if cfg!(windows) {
            return;
        }
        let root = std::path::Path::new("C:\\repo");
        let path = std::path::Path::new("D:\\data\\thing.test.ts");
        assert_eq!(
            relativize_test_path(path, root),
            std::path::PathBuf::from("D:\\data\\thing.test.ts"),
        );
    }

    /// A path outside `root` -- reachable in principle via a symlink that resolves outside
    /// the analysis root, see this function's doc comment for why discovery does not exercise this
    /// today -- must keep its absolute form rather than being silently dropped or mangled.
    #[cfg(unix)]
    #[test]
    fn relativize_test_path_keeps_the_absolute_path_for_a_symlink_escaping_root() {
        use std::os::unix::fs::symlink;

        let workdir = tempfile::tempdir().unwrap();
        let root = workdir.path().join("root");
        let outside = workdir.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let target = outside.join("escaped.test.ts");
        std::fs::write(&target, "// outside the analysis root").unwrap();
        let link = root.join("link.test.ts");
        symlink(&target, &link).unwrap();

        // Canonicalizing the symlink is what a real frontend path would carry (the frontend reads
        // through the link to the file it points at), and matches how `Config::load` canonicalizes
        // `root` itself before this function ever runs.
        let resolved = link.canonicalize().unwrap();
        let canonical_root = root.canonicalize().unwrap();

        assert!(
            !resolved.starts_with(&canonical_root),
            "sanity: the symlink must actually resolve outside root, or this test proves nothing"
        );
        assert_eq!(
            relativize_test_path(&resolved, &canonical_root),
            resolved,
            "a path outside root must be reported unchanged (absolute), never dropped or truncated"
        );
    }

    /// Load a temporary fixture config with the requested threshold override.
    fn config_with_threshold(threshold: u8) -> Config {
        let directory = tempfile::tempdir().unwrap();
        Config::load(
            directory.path(),
            ConfigOverrides {
                threshold: Some(threshold),
                ..ConfigOverrides::default()
            },
        )
        .unwrap()
    }

    /// `--fail-on-incomplete` on an incomplete run must return the documented operational
    /// error code (2), never the quality-gate code (1) -- an analyzer that could not fully read
    /// the corpus is a different failure than one that read it and found duplicates, and
    /// `cli.rs`'s `ExitCode` contract exists precisely so CI can tell them apart.
    /// C10: `=reads` counts only read-side causes. A limits-only degraded run (pair budget
    /// exhausted) must NOT trip it -- that run's response is raising the budget, and failing a
    /// reads gate on it would send someone chasing the wrong fix.
    use vitest_dedup::cli::FailOnIncomplete;

    /// Regression fixture for CLI reporting: fail on incomplete reads ignores limit causes.
    #[test]
    fn fail_on_incomplete_reads_ignores_limit_causes() {
        let config = config_with_threshold(0);
        let limits_only = AnalysisResult {
            tests_analyzed: 1,
            incomplete_causes: IncompleteCauses {
                pair_budget_exhausted: true,
                findings_truncated: 3,
                ..IncompleteCauses::default()
            },
            ..AnalysisResult::default()
        };
        assert_eq!(
            final_exit_code(Some(FailOnIncomplete::Reads), None, &limits_only, &config),
            analysis::exit_code(&limits_only, &config),
            "a limits-only cause is not a read failure"
        );
        let reads_only = AnalysisResult {
            tests_analyzed: 1,
            incomplete_causes: IncompleteCauses {
                files_unreadable: 12,
                ..IncompleteCauses::default()
            },
            ..AnalysisResult::default()
        };
        assert_eq!(
            final_exit_code(Some(FailOnIncomplete::Reads), None, &reads_only, &config),
            ExitCode::OperationalError,
            "twelve unreadable files is exactly what =reads exists to gate"
        );
    }

    /// C10: `=limits` is the mirror -- read-side degradation must not trip it.
    #[test]
    fn fail_on_incomplete_limits_ignores_read_causes() {
        let config = config_with_threshold(0);
        let reads_only = AnalysisResult {
            tests_analyzed: 1,
            incomplete_causes: IncompleteCauses {
                files_unreadable: 12,
                unresolved_contexts: 40,
                ..IncompleteCauses::default()
            },
            ..AnalysisResult::default()
        };
        assert_eq!(
            final_exit_code(Some(FailOnIncomplete::Limits), None, &reads_only, &config),
            analysis::exit_code(&reads_only, &config),
        );
        let limits_only = AnalysisResult {
            tests_analyzed: 1,
            incomplete_causes: IncompleteCauses {
                hook_scan_exhausted: true,
                ..IncompleteCauses::default()
            },
            ..AnalysisResult::default()
        };
        assert_eq!(
            final_exit_code(Some(FailOnIncomplete::Limits), None, &limits_only, &config),
            ExitCode::OperationalError,
        );
    }

    /// Regression fixture for CLI reporting: fail on incomplete returns operational error not
    /// quality gate failed.
    #[test]
    fn fail_on_incomplete_returns_operational_error_not_quality_gate_failed() {
        let config = config_with_threshold(0);
        let result = AnalysisResult {
            tests_analyzed: 1,
            incomplete_causes: IncompleteCauses {
                pair_budget_exhausted: true,
                ..IncompleteCauses::default()
            },
            ..AnalysisResult::default()
        };
        assert_eq!(
            final_exit_code(Some(FailOnIncomplete::Any), None, &result, &config),
            ExitCode::OperationalError
        );
    }

    /// Regression fixture for CLI reporting: a finding-free zero-test run is an operational error.
    #[test]
    fn nothing_analyzed_is_always_an_operational_error() {
        let config = config_with_threshold(0);
        let result = AnalysisResult::default();
        assert_eq!(
            final_exit_code(None, None, &result, &config),
            ExitCode::OperationalError
        );
    }

    /// Regression fixture for CLI reporting: zero executable tests preserve real quality
    /// failures.
    #[test]
    fn zero_executable_tests_preserve_real_quality_failures() {
        let strict_config = config_with_threshold(0);
        let suite_error = AnalysisResult {
            had_error_finding: true,
            incomplete_causes: IncompleteCauses {
                nothing_read: true,
                ..IncompleteCauses::default()
            },
            ..AnalysisResult::default()
        };
        assert_eq!(
            final_exit_code(None, None, &suite_error, &strict_config),
            ExitCode::QualityGateFailed
        );
        assert_eq!(
            final_exit_code(
                Some(FailOnIncomplete::Any),
                None,
                &suite_error,
                &strict_config
            ),
            ExitCode::OperationalError
        );
        assert_eq!(
            final_exit_code(
                Some(FailOnIncomplete::Reads),
                None,
                &suite_error,
                &strict_config
            ),
            ExitCode::OperationalError
        );
        assert_eq!(
            final_exit_code(
                Some(FailOnIncomplete::Limits),
                None,
                &suite_error,
                &strict_config
            ),
            ExitCode::QualityGateFailed
        );

        let permissive_config = config_with_threshold(100);
        let new_suite_finding = AnalysisResult {
            modes: vitest_dedup::model::ModeSummary {
                baseline_applied: true,
                new_findings: 1,
                ..vitest_dedup::model::ModeSummary::default()
            },
            ..AnalysisResult::default()
        };
        assert_eq!(
            final_exit_code(None, Some(0), &new_suite_finding, &permissive_config),
            ExitCode::QualityGateFailed
        );
    }

    /// Without `--fail-on-incomplete`, an incomplete run still falls through to the ordinary
    /// quality-gate decision -- the flag only changes behavior when it is actually passed.
    #[test]
    fn incomplete_without_the_flag_falls_through_to_the_quality_gate() {
        let config = config_with_threshold(0);
        let result = AnalysisResult {
            tests_analyzed: 1,
            incomplete_causes: IncompleteCauses {
                pair_budget_exhausted: true,
                ..IncompleteCauses::default()
            },
            ..AnalysisResult::default()
        };
        assert_eq!(
            final_exit_code(None, None, &result, &config),
            analysis::exit_code(&result, &config)
        );
    }

    /// `--fail-on-incomplete` on a complete run must not itself force a failure -- it only
    /// upgrades an already-incomplete run's severity, never invents incompleteness.
    #[test]
    fn fail_on_incomplete_is_a_no_op_on_a_complete_run() {
        let config = config_with_threshold(0);
        let result = AnalysisResult {
            tests_analyzed: 1,
            ..AnalysisResult::default()
        };
        assert_eq!(
            final_exit_code(Some(FailOnIncomplete::Any), None, &result, &config),
            ExitCode::Success
        );
    }
    // =========================================================================================
    // Baseline and changed-file modes, end to end through the real binary.
    //
    // `crate::modes` unit-tests matching and git plumbing, and `analysis::tests` pins what a
    // baselined finding does to the gate. What neither can reach is the thing a user actually
    // depends on: the process exit code, and whether the finding is still on screen. Both are
    // asserted here against the built binary, because the contract is "exit 0 *and* you can still
    // see it", and a test that checked only one half would pass while the other half regressed.
    // =========================================================================================

    /// Two tests with the same normalized body in one file: one `duplicate-test` finding at Error
    /// severity, which fails the default quality gate.
    const DUPLICATE_PAIR: &str = r#"
        import { test, expect } from 'vitest';

        test('adds numbers', () => {
            const total = 1 + 1;
            expect(total).toBe(2);
        });

        test('adds numbers, again', () => {
            const total = 1 + 1;
            expect(total).toBe(2);
        });
    "#;

    /// A second, independent duplicate pair, for "one new finding appeared" scenarios.
    const OTHER_DUPLICATE_PAIR: &str = r#"
        import { test, expect } from 'vitest';

        test('subtracts numbers', () => {
            const total = 5 - 3;
            expect(total).toBe(2);
        });

        test('subtracts numbers, again', () => {
            const total = 5 - 3;
            expect(total).toBe(2);
        });
    "#;

    /// A file with nothing duplicated in it.
    const CLEAN_TESTS: &str = r#"
        import { test, expect } from 'vitest';

        test('multiplies', () => {
            expect(3 * 4).toBe(12);
        });

        test('divides', () => {
            expect(12 / 4).toBe(3);
        });
    "#;

    /// Runs the binary and returns `(exit code, stdout, stderr)`.
    fn run_cli(root: &std::path::Path, args: &[&str]) -> (Option<i32>, String, String) {
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .expect("binary")
            .arg(root)
            .args(args)
            .output()
            .expect("run");
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    /// Read and parse the JSON file for an output prefix, panicking with path context on
    /// failure.
    fn read_json_report(prefix: &std::path::Path) -> serde_json::Value {
        let path = prefix.with_extension("json");
        serde_json::from_str(
            &std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("reading {}: {error}", path.display())),
        )
        .expect("the JSON reporter emits valid JSON")
    }

    /// An oversized file is not a traversal failure, and the banner must not say it is.
    ///
    /// `discovery` sets its aggregate `incomplete` flag for nine different reasons, and mapping
    /// that flag to `discovery_failed` made every one of them print "directory traversal failed,
    /// so files may be missing from this run entirely". For a repository whose only problem is
    /// one large file, that sends the reader looking for a broken filesystem walk that never
    /// happened -- while `filesTooLarge`, the counter that describes exactly what did happen,
    /// stayed at zero because an oversized file is skipped in discovery and never reaches the
    /// read-time size check that increments it.
    #[test]
    fn an_oversized_file_is_counted_as_too_large_and_not_as_a_failed_traversal() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        // A real duplicate pair, so the run has something to say and the report is not empty.
        std::fs::write(root.join("math.test.ts"), DUPLICATE_PAIR).unwrap();
        // Just past the limit, not comfortably past it: a bound is tested at its edge.
        let oversized = "// ".to_owned()
            + &"x".repeat(usize::try_from(vitest_dedup::discovery::MAX_SOURCE_FILE_BYTES).unwrap())
            + "\n";
        std::fs::write(root.join("big.test.ts"), oversized).unwrap();

        let prefix = root.join("report");
        let (_, _, stderr) = run_cli(
            root,
            &["--reporters", "json", "--output", prefix.to_str().unwrap()],
        );
        let report = read_json_report(&prefix);
        let causes = &report["incompleteCauses"];

        assert_eq!(report["incomplete"], true, "{report:#?}");
        assert_eq!(
            causes["filesTooLarge"], 1,
            "the oversized file is counted as what it is: {report:#?}"
        );
        assert_eq!(
            causes["discoveryFailed"], false,
            "nothing failed to traverse: {report:#?}"
        );
        assert!(
            !stderr.contains("traversal failed"),
            "and no reporter may say it did: {stderr}"
        );
    }

    /// `--output` creates the directories its prefix names.
    ///
    /// A reporter writing into `reports/2026-09/run` must not fail because `reports/2026-09` does
    /// not exist yet -- the prefix is a path the caller chose, not a directory they were asked to
    /// prepare. Covers the `create_dir_all` arm, which no other test reaches because every other
    /// `--output` in this suite points straight at a temp directory that already exists.
    #[test]
    fn output_creates_the_directories_its_prefix_names() {
        let directory = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        let prefix = root.join("nested").join("deeper").join("report");

        let (_, _, stderr) = run_cli(
            root,
            &["--reporters", "json", "--output", prefix.to_str().unwrap()],
        );

        assert!(
            prefix.with_extension("json").exists(),
            "the report is written through directories that did not exist: {stderr}"
        );
        let report = read_json_report(&prefix);
        assert_eq!(report["testsAnalyzed"], 2, "{report:#?}");
    }

    #[test]
    /// End to end: record a baseline, re-run, and the finding passes the gate while staying on
    /// screen marked `[baselined]` -- visible debt, not hidden debt.
    fn a_baselined_finding_passes_the_gate_and_is_still_visible_in_every_report() {
        let directory = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        let baseline = root.join("vitest-dedup-baseline.json");

        // Before any baseline: the gate fails. Without this the rest proves nothing.
        let (code, _, _) = run_cli(root, &[]);
        assert_eq!(code, Some(1), "the pair must fail the gate to begin with");

        // Record it.
        let (code, _, stderr) = run_cli(
            root,
            &[
                "--baseline",
                baseline.to_str().unwrap(),
                "--update-baseline",
            ],
        );
        assert_eq!(code, Some(0), "recording a baseline is not a gate failure");
        assert!(stderr.contains("wrote baseline"), "{stderr}");
        assert!(baseline.is_file());

        // Apply it: clean exit, finding still on screen, marked rather than hidden.
        let prefix = root.join("report");
        let (code, stdout, stderr) = run_cli(
            root,
            &[
                "--baseline",
                baseline.to_str().unwrap(),
                "--reporters",
                "terminal,json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        assert_eq!(code, Some(0), "a baselined finding must not fail the gate");
        assert!(
            stdout.contains("duplicate-test") && stdout.contains("math.test.ts"),
            "the finding must stay in the report, not disappear: {stdout}"
        );
        assert!(
            stdout.contains("[baselined]"),
            "a suppressed finding must be labeled as such: {stdout}"
        );
        assert!(
            stdout.contains("Baseline: 1 accepted"),
            "the summary must say how much is accepted: {stdout}"
        );
        assert!(
            stderr.contains("do not affect the quality gate"),
            "{stderr}"
        );

        let report = read_json_report(&prefix);
        assert_eq!(report["findings"].as_array().unwrap().len(), 1);
        assert_eq!(report["findings"][0]["baselined"], true);
        assert_eq!(report["modes"]["baselined"], 1);
        assert_eq!(report["modes"]["newFindings"], 0);
        assert_eq!(report["modes"]["baselineApplied"], true);
    }

    /// P3: `// vitest-dedup-disable-next-line` beside one participant removes the finding, the
    /// summary says so, and the precedence holds end to end -- the suppressed pair is never
    /// recorded by `--update-baseline`, so the baseline records nothing and a later run of the
    /// same tree gates clean for the right reason (nothing to accept, not debt accepted).
    /// P4 flagship: `--baseline-from-ref HEAD` treats the base revision's findings as accepted,
    /// so a PR touching nothing about the duplicates gates clean -- while the finding itself
    /// stays in every report, marked like any baselined debt. The containment is asserted
    /// operationally: the flag works in a repo whose analysis would otherwise be identical.
    #[test]
    fn baseline_from_ref_accepts_the_base_revisions_findings() {
        // The duplicate pair is committed; the working tree is identical to it.
        let directory = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        init_repository(root);

        // Without the flag: the gate fails.
        let (code, _, _) = run_cli(root, &[]);
        assert_eq!(
            code,
            Some(1),
            "the pair must fail the gate without the flag"
        );

        // With the flag: the base revision's finding is accepted debt, and the report says both
        // the source of that answer and the finding's marked status.
        let prefix = root.join("report");
        let (code, stdout, stderr) = run_cli(
            root,
            &[
                "--baseline-from-ref",
                "HEAD",
                "--reporters",
                "terminal,json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        assert_eq!(
            code,
            Some(0),
            "the committed pair is the base's own debt: {stderr}"
        );
        assert!(
            stderr.contains("baseline from HEAD: 1 finding(s)"),
            "the run must state what the base revision contributed: {stderr}"
        );
        assert!(
            stdout.contains("[baselined]"),
            "the finding stays visible, marked as accepted: {stdout}"
        );
        let report = read_json_report(&prefix);
        assert_eq!(report["modes"]["baselineApplied"], true);
        assert_eq!(report["modes"]["baselined"], 1);
        assert_eq!(report["modes"]["newFindings"], 0);
    }

    /// A base revision that predates the duplication accepts nothing, so the pair is new and
    /// the gate fails -- the flag can only accept what the base actually had, never paper over
    /// the working tree.
    #[test]
    fn baseline_from_ref_accepts_nothing_the_base_revision_did_not_have() {
        // Commit an empty tree, THEN add the duplicate pair uncommitted.
        let directory = single_file_project("placeholder.test.ts", CLEAN_TESTS);
        let root = directory.path();
        init_repository(root);
        std::fs::write(root.join("dirty.test.ts"), DUPLICATE_PAIR).unwrap();

        let (code, _, stderr) = run_cli(root, &["--baseline-from-ref", "HEAD"]);
        assert_eq!(
            code,
            Some(1),
            "a finding the base lacks is new, and new errors gate"
        );
        assert!(
            stderr.contains("baseline from HEAD: 0 finding(s)"),
            "the empty base must be stated, not silently treated as no flag: {stderr}"
        );
    }

    /// The review-found geometry: the analysis root is a SUBDIRECTORY of the git repository.
    /// `ls-tree` run from there lists paths relative to it; a read that resolved those paths from
    /// the repository root would silently read the wrong files (or fail), and the baseline would
    /// be computed from nothing. Blobs are now read by the object id `ls-tree` lists. Pinned from
    /// `src/`: a committed duplicate pair under a subdirectory must still be accepted from the
    /// base revision.
    #[test]
    fn baseline_from_ref_works_when_the_analysis_root_is_below_the_git_root() {
        // One git repository; the duplicate pair is committed under packages/app/ -- committed
        // by the OUTER repository (a nested `git init` would make the subdirectory its own root
        // and the geometry under test would vanish).
        let repository = single_file_project("placeholder.test.ts", CLEAN_TESTS);
        let root = repository.path();
        init_repository(root);
        let package = root.join("packages").join("app");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("math.test.ts"), DUPLICATE_PAIR).unwrap();
        for arguments in [
            ["add", "."].as_slice(),
            ["commit", "-qm", "add package"].as_slice(),
        ] {
            assert!(std::process::Command::new("git")
                .args(arguments)
                .current_dir(root)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .status()
                .unwrap()
                .success());
        }

        // The analysis root is packages/app: a subdirectory of the git repository.
        let (code, stdout, stderr) = run_cli(&package, &["--baseline-from-ref", "HEAD"]);
        assert_eq!(
            code,
            Some(0),
            "the committed pair under packages/app is the base's own debt: {stderr}"
        );
        assert!(
            stderr.contains("baseline from HEAD: 1 finding(s)"),
            "{stderr}"
        );
        assert!(stdout.contains("[baselined]"), "{stdout}");
    }

    /// The revision is validated like `--changed-since`'s: a flag-looking or empty revision is
    /// an operational error (exit 2), never an injection into a git invocation.
    #[test]
    fn baseline_from_ref_rejects_an_invalid_revision() {
        let directory = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        init_repository(root);
        // `--exec` as a separate arg is rejected by the CLI grammar itself; the `=` form
        // reaches this tool's own validation, which must still refuse it -- a revision is data,
        // never a flag, whichever spelling delivered it.
        let (code, _, stderr) = run_cli(root, &["--baseline-from-ref=--exec"]);
        assert_eq!(code, Some(2));
        assert!(
            stderr.contains("invalid --baseline-from-ref revision"),
            "{stderr}"
        );
    }

    /// Refusals keep listing order: an oversized base file listed before an unreadable entry (a
    /// gitlink named like a test file) still gets its skip line before the run fails, as it did
    /// when each file was read in turn.
    #[test]
    fn baseline_from_ref_reports_earlier_skips_before_a_later_refusal() {
        let directory = single_file_project("c-ok.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        std::fs::write(root.join("a-huge.test.ts"), "x".repeat(8 * 1024 * 1024 + 1)).unwrap();
        init_repository(root);
        // A gitlink's commit need not exist in this repository.
        git(
            root,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000",
                "1234567890123456789012345678901234567890",
                "b-vendor.test.ts",
            ],
        );
        git(root, &["commit", "-qm", "gitlink"]);
        let (code, _, stderr) = run_cli(root, &["--baseline-from-ref", "HEAD"]);
        assert_eq!(code, Some(2), "{stderr}");
        let skip = stderr
            .find("skipping a-huge.test.ts in the base revision: exceeds the byte limit")
            .unwrap_or_else(|| panic!("the earlier skip is reported: {stderr}"));
        let refusal = stderr
            .find("git failed while reading base file `b-vendor.test.ts`")
            .unwrap_or_else(|| panic!("the gitlink is refused: {stderr}"));
        assert!(skip < refusal, "skip before refusal: {stderr}");
    }

    #[test]
    /// P3: the directive beside one participant removes the finding, the summary says so, and
    /// `--update-baseline` on the suppressed tree records nothing -- a baseline records accepted
    /// debt, and a suppressed finding claims there is none.
    fn inline_suppression_removes_the_finding_and_outranks_the_baseline() {
        // The directive sits on the line before the first declaration of the pair.
        let source = DUPLICATE_PAIR.replace(
            "        test('adds numbers', () => {",
            "        // vitest-dedup-disable-next-line these two are deliberately parallel examples\n        test('adds numbers', () => {",
        );
        let directory = single_file_project("math.test.ts", &source);
        let root = directory.path();

        // Suppressed: the gate passes and the run says why.
        let prefix = root.join("report");
        let (code, stdout, stderr) = run_cli(
            root,
            &[
                "--reporters",
                "terminal,json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        assert_eq!(
            code,
            Some(0),
            "a suppressed pair must not fail the gate: {stderr}"
        );
        assert!(
            stdout.contains("1 finding suppressed by inline directive"),
            "the removal must be visible in the summary: {stdout}"
        );
        let report = read_json_report(&prefix);
        assert_eq!(report["findings"].as_array().unwrap().len(), 0);
        assert_eq!(report["modes"]["suppressionsApplied"], 1);

        // Precedence: `--update-baseline` on the suppressed tree records NOTHING, because the
        // finding never survives to the writer -- a baseline is a record of accepted debt, and
        // a suppressed finding is a claim that there is none.
        let baseline = root.join("baseline.json");
        let (code, _, stderr) = run_cli(
            root,
            &[
                "--baseline",
                baseline.to_str().unwrap(),
                "--update-baseline",
            ],
        );
        assert_eq!(code, Some(0), "{stderr}");
        assert!(
            stderr.contains("0 finding(s) recorded"),
            "the suppressed finding must never reach the baseline writer: {stderr}"
        );

        // The unsuppressed twin, same tree: a directive beside only one of the pair's
        // participants would not be enough -- both participants must be checked. Here neither
        // is, on a fresh copy without any directive: it fires.
        let plain = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let (code, _, _) = run_cli(plain.path(), &[]);
        assert_eq!(code, Some(1), "without a directive the pair still fires");
    }

    /// Regression fixture for CLI reporting: fail on new fires only above its limit.
    #[test]
    fn fail_on_new_fires_only_above_its_limit() {
        let directory = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        let baseline = root.join("baseline.json");
        let baseline_arg = baseline.to_str().unwrap();

        run_cli(root, &["--baseline", baseline_arg, "--update-baseline"]);

        // Exactly one finding is now new.
        std::fs::write(root.join("more.test.ts"), OTHER_DUPLICATE_PAIR).unwrap();
        let prefix = root.join("report");
        let (_, _, _) = run_cli(
            root,
            &[
                "--baseline",
                baseline_arg,
                "--reporters",
                "json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        assert_eq!(
            read_json_report(&prefix)["modes"]["newFindings"],
            1,
            "sanity: the boundary cases below are meaningless unless this is exactly 1"
        );

        // `--fail-on-new N` fails when *more than* N are new, so 1 passes at 1 and fails at 0.
        // `--threshold 100` isolates this gate from the severity gate, which at its default of 0
        // would fail on the same error-severity finding and make both boundaries read `1`.
        let boundary = |limit: &str| {
            run_cli(
                root,
                &[
                    "--baseline",
                    baseline_arg,
                    "--threshold",
                    "100",
                    "--fail-on-new",
                    limit,
                ],
            )
            .0
        };
        assert_eq!(
            boundary("1"),
            Some(0),
            "1 new finding is within a limit of 1"
        );
        assert_eq!(
            boundary("0"),
            Some(1),
            "exceeding --fail-on-new is a quality-gate failure (1), never an operational one (2)"
        );

        // The two gates are additive, never alternatives: a generous `--fail-on-new` must not
        // rescue a run the severity gate already fails, or it would become a way to raise the
        // threshold without saying so.
        let (code, _, _) = run_cli(root, &["--baseline", baseline_arg, "--fail-on-new", "99"]);
        assert_eq!(code, Some(1));
    }

    /// Regression fixture for CLI reporting: update baseline refuses to record an incomplete
    /// run.
    #[test]
    fn update_baseline_refuses_to_record_an_incomplete_run() {
        // An incomplete run does not know what it would be accepting, and a baseline written from
        // one silently accepts everything it could not see the first time a complete run finds it.
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        // An ambient-`globals` file with no config: extraction yields nothing and the run is
        // marked incomplete, the same shape `--fail-on-incomplete` exists for.
        std::fs::write(
            root.join("ambient.test.ts"),
            "test('ambient', () => { expect(1).toBe(1); });",
        )
        .unwrap();
        let baseline = root.join("baseline.json");

        let (code, _, stderr) = run_cli(
            root,
            &[
                "--baseline",
                baseline.to_str().unwrap(),
                "--update-baseline",
            ],
        );
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("refusing to write baseline"), "{stderr}");
        assert!(!baseline.exists(), "no partial baseline may be left behind");
    }

    // -- `--changed-since` ------------------------------------------------------------------

    /// Run a fixture Git operation after removing inherited repository-path environment
    /// variables.
    fn git(root: &std::path::Path, args: &[&str]) {
        let mut command = std::process::Command::new("git");
        // The same inherited-environment hazard `modes.rs` documents: when this suite runs from a
        // Git hook, `GIT_DIR` and friends outrank `current_dir` and these fixtures would stage
        // themselves into the real repository's index.
        for variable in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_COMMON_DIR",
            "GIT_PREFIX",
            "GIT_CEILING_DIRECTORIES",
        ] {
            command.env_remove(variable);
        }
        let status = command
            .current_dir(root)
            .args(args)
            .status()
            .expect("git must be available to run these tests");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Initialize and commit the fixture tree with a local test identity on main.
    fn init_repository(root: &std::path::Path) {
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.email", "test@example.com"]);
        git(root, &["config", "user.name", "Test"]);
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "initial"]);
    }

    #[test]
    fn changed_since_reports_an_incomplete_run_rather_than_a_false_clean() {
        // The failure this whole design exists to prevent. The file that changed is clean; the
        // duplication lives in a file nobody touched. A changed-files run has nothing to report --
        // and must not let that be read as "the repository is clean".
        let directory = single_file_project("clean.test.ts", CLEAN_TESTS);
        let root = directory.path();
        std::fs::write(root.join("dirty.test.ts"), DUPLICATE_PAIR).unwrap();
        init_repository(root);
        std::fs::write(
            root.join("clean.test.ts"),
            format!("{CLEAN_TESTS}\n// touched\n"),
        )
        .unwrap();

        let prefix = root.join("report");
        let (code, stdout, stderr) = run_cli(
            root,
            &[
                "--changed-since",
                "HEAD",
                "--reporters",
                "terminal,json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );

        let report = read_json_report(&prefix);
        assert_eq!(
            report["findings"].as_array().unwrap().len(),
            0,
            "the changed file really is clean"
        );
        assert_eq!(
            report["incomplete"], true,
            "an empty changed-files report that withheld a real finding is not a clean run"
        );
        assert_eq!(report["modes"]["withheldUnchanged"], 1);
        assert_eq!(report["modes"]["changedScope"], true);
        assert!(
            stdout.contains("CHANGED-FILES RUN") && stdout.contains("withheld"),
            "the report must say why it cannot speak for the repository: {stdout}"
        );
        assert!(stderr.contains("withheld"), "{stderr}");
        assert_eq!(
            code,
            Some(0),
            "no reported finding means the gate passes -- which is exactly why `incomplete` has \
             to carry the caveat"
        );

        // And CI that refuses degraded runs gets the operational error it asked for.
        let (code, _, _) = run_cli(root, &["--changed-since", "HEAD", "--fail-on-incomplete"]);
        assert_eq!(code, Some(2));
    }

    #[test]
    fn changed_since_still_reports_a_finding_in_a_file_that_changed() {
        let directory = single_file_project("dirty.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        std::fs::write(root.join("clean.test.ts"), CLEAN_TESTS).unwrap();
        init_repository(root);
        std::fs::write(
            root.join("dirty.test.ts"),
            format!("{DUPLICATE_PAIR}\n// touched\n"),
        )
        .unwrap();

        let prefix = root.join("report");
        let (code, _, _) = run_cli(
            root,
            &[
                "--changed-since",
                "HEAD",
                "--reporters",
                "json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        let report = read_json_report(&prefix);
        assert_eq!(report["findings"].as_array().unwrap().len(), 1);
        assert_eq!(report["modes"]["withheldUnchanged"], 0);
        assert_eq!(
            report["incomplete"], false,
            "a changed-files run that withheld nothing is a full run's report and must not cry \
             wolf about coverage"
        );
        assert_eq!(code, Some(1));
    }

    #[test]
    fn changed_since_finds_a_cross_file_duplicate_whose_twin_is_unchanged() {
        // The trap: `cross-file-duplicate` compares across files. Analysis therefore covers the
        // whole corpus and only findings are filtered -- had the corpus been restricted to changed
        // files, the twin would never have been extracted and this run would have reported the
        // changed file as clean.
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let shared = r#"
            import { test, expect } from 'vitest';

            test('shared behavior', () => {
                const total = 2 + 2;
                expect(total).toBe(4);
            });
        "#;
        std::fs::write(root.join("a.test.ts"), shared).unwrap();
        std::fs::write(root.join("b.test.ts"), shared).unwrap();
        init_repository(root);
        std::fs::write(root.join("a.test.ts"), format!("{shared}\n// touched\n")).unwrap();

        let prefix = root.join("report");
        run_cli(
            root,
            &[
                "--changed-since",
                "HEAD",
                "--rule",
                "cross-file-duplicate=error",
                "--reporters",
                "json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        let report = read_json_report(&prefix);
        let findings = report["findings"].as_array().unwrap();
        assert_eq!(
            findings.len(),
            1,
            "the unchanged twin must still be analyzed and reported: {report:#?}"
        );
        assert_eq!(findings[0]["rule"], "cross-file-duplicate");
    }

    #[test]
    fn changed_since_against_a_missing_revision_is_an_operational_error() {
        let directory = single_file_project("a.test.ts", CLEAN_TESTS);
        let root = directory.path();
        init_repository(root);

        let (code, _, stderr) = run_cli(root, &["--changed-since", "no-such-ref"]);
        assert_eq!(
            code,
            Some(2),
            "an unresolvable revision must not be a silent empty analysis: {stderr}"
        );
        assert!(stderr.contains("does not resolve to a commit"), "{stderr}");
    }

    #[test]
    fn changed_since_outside_a_git_repository_is_an_operational_error() {
        let directory = single_file_project("a.test.ts", CLEAN_TESTS);
        let root = directory.path();

        let (code, _, stderr) = run_cli(root, &["--changed-since", "HEAD"]);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("requires a Git repository"), "{stderr}");
    }

    #[test]
    fn changed_since_in_a_shallow_clone_is_an_operational_error_naming_the_fix() {
        // A shallow clone is the dangerous case: `HEAD~1` or a base branch may exist as a name
        // while its history does not, so the diff is silently wrong rather than obviously absent.
        // GitHub Actions checks out depth 1 by default, which makes this the single most likely
        // way `--changed-since` gets used wrongly in CI.
        let origin = tempfile::tempdir().unwrap();
        std::fs::write(origin.path().join("a.test.ts"), CLEAN_TESTS).unwrap();
        init_repository(origin.path());
        std::fs::write(
            origin.path().join("a.test.ts"),
            format!("{CLEAN_TESTS}\n// second\n"),
        )
        .unwrap();
        git(origin.path(), &["commit", "-qam", "second"]);

        let workspace = tempfile::tempdir().unwrap();
        let clone = workspace.path().join("shallow");
        git(
            workspace.path(),
            &[
                "clone",
                "-q",
                "--depth",
                "1",
                "--no-local",
                &format!("file://{}", origin.path().display()),
                clone.to_str().unwrap(),
            ],
        );

        let (code, _, stderr) = run_cli(&clone, &["--changed-since", "HEAD"]);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("shallow clone"), "{stderr}");
        assert!(stderr.contains("fetch-depth: 0"), "{stderr}");
    }

    #[test]
    fn a_stale_baseline_is_an_operational_error_not_a_wall_of_new_findings() {
        // A baseline computed against a different fingerprint derivation matches nothing. Treating
        // that as "everything is new" would look exactly like a sudden mass regression and would
        // be acted on as one; it is a broken input, so it exits 2.
        let directory = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        let baseline = root.join("baseline.json");
        std::fs::write(
            &baseline,
            r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v0","findings":[]}"#,
        )
        .unwrap();

        let (code, _, stderr) = run_cli(root, &["--baseline", baseline.to_str().unwrap()]);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("fingerprint version"), "{stderr}");
        assert!(stderr.contains("--update-baseline"), "{stderr}");
    }

    #[test]
    fn impossible_flag_combinations_are_rejected_by_the_grammar() {
        for arguments in [
            // Nothing to rewrite.
            vec!["--update-baseline"],
            // Nothing to compare against.
            vec!["--fail-on-new", "0"],
        ] {
            assert!(
                ParsedArgs::parse_from(
                    std::iter::once("vitest-dedup").chain(arguments.iter().copied())
                )
                .is_err(),
                "{arguments:?} must be rejected"
            );
        }
        // A baseline written from a changed-files run would record only the slice that run
        // happened to report, silently accepting everything it never looked at.
        assert!(ParsedArgs::parse_from([
            "vitest-dedup",
            "--baseline",
            "b.json",
            "--update-baseline",
            "--changed-since",
            "HEAD",
        ])
        .is_err());
        assert!(ParsedArgs::parse_from([
            "vitest-dedup",
            "--baseline",
            "b.json",
            "--update-baseline",
            "--fail-on-new",
            "0",
        ])
        .is_err());
    }

    /// `--fail-on-new` is a quality-gate failure, not an operational one: the analyzer ran fine.
    #[test]
    fn fail_on_new_maps_to_the_quality_gate_exit_code() {
        let config = config_with_threshold(100);
        let result = AnalysisResult {
            tests_analyzed: 1,
            modes: vitest_dedup::model::ModeSummary {
                baseline_applied: true,
                baselined: 3,
                new_findings: 2,
                ..vitest_dedup::model::ModeSummary::default()
            },
            ..AnalysisResult::default()
        };
        assert_eq!(
            final_exit_code(Some(FailOnIncomplete::Any), Some(1), &result, &config),
            ExitCode::QualityGateFailed
        );
        assert_eq!(
            final_exit_code(Some(FailOnIncomplete::Any), Some(2), &result, &config),
            ExitCode::Success,
            "the limit is `more than N`, so N new findings passes"
        );
        assert_eq!(
            final_exit_code(Some(FailOnIncomplete::Any), None, &result, &config),
            ExitCode::Success
        );
        // An incomplete run under `--fail-on-incomplete` still outranks it: "the analyzer could
        // not see everything" is a different failure from "you added duplicate tests".
        let degraded = AnalysisResult {
            incomplete_causes: IncompleteCauses {
                pair_budget_exhausted: true,
                ..IncompleteCauses::default()
            },
            ..result.clone()
        };
        assert_eq!(
            final_exit_code(Some(FailOnIncomplete::Any), Some(1), &degraded, &config),
            ExitCode::OperationalError
        );
    }

    // =========================================================================================
    // Type-level tests, end to end through the real binary.
    //
    // A type-level suite's whole content is its type arguments. When those were erased from the
    // body fingerprint, `expectTypeOf`/`assertType`/hand-rolled `assertEqual<A, B>` suites read
    // as a file full of byte-identical tests and failed other people's builds at Error severity.
    // The three rules below are asserted separately because they key on different things:
    // `duplicate-test` on `alpha`, `near-duplicate-test` and `subsumed-test` on behavior events.
    // =========================================================================================

    /// Every rule enabled, including `subsumed-test`, the one rule that is off by default, so a
    /// run that reports nothing has actually been asked the question by all three.
    fn run_all_rules(root: &std::path::Path, prefix: &std::path::Path) -> serde_json::Value {
        run_cli(
            root,
            &[
                "--rule",
                "subsumed-test=warning",
                "--reporters",
                "json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        read_json_report(prefix)
    }

    /// The reproduction: two type-level assertions differing only in the type they assert about.
    /// This was one `duplicate-test` at Error severity, `incomplete: false`, on correct code.
    #[test]
    fn type_level_tests_differing_only_in_type_arguments_are_not_duplicates() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(
            root.join("types.test.ts"),
            r#"
            import { test } from 'vitest';
            import { assertEqual } from './helpers';

            type Expected = Record<string, unknown>;

            test('checks a string mapping', () => {
                assertEqual<Record<string, string>, Expected>(true);
            });

            test('checks a number mapping', () => {
                assertEqual<Record<string, number>, Expected>(true);
            });
            "#,
        )
        .unwrap();

        let prefix = root.join("report");
        let report = run_all_rules(root, &prefix);
        assert_eq!(
            report["findings"].as_array().unwrap().len(),
            0,
            "two assertions about different types are not duplicates: {report:#?}"
        );
    }

    /// The same shape under `near-duplicate-test`, which keys on the sorted behavior multiset
    /// rather than on `alpha`: reordering the setup call makes the two bodies' `alpha` differ, so
    /// only the behavior events stand between these two tests and a finding.
    #[test]
    fn reordered_type_level_tests_are_not_near_duplicates() {
        let report = type_level_report(
            ", prepare",
            r#"
            test('checks a string mapping', () => {
                prepare();
                assertEqual<Record<string, string>, Expected>(true);
            });

            test('checks a number mapping', () => {
                assertEqual<Record<string, number>, Expected>(true);
                prepare();
            });
            "#,
        );
        assert_eq!(
            report["findings"].as_array().unwrap().len(),
            0,
            "the two bodies run different assertions, so their behavior events must differ: \
             {report:#?}"
        );
    }

    /// The same shape under `subsumed-test` (off by default, enabled here), which asks whether one
    /// body's behavior events are a strict ordered subsequence of the other's. With the type
    /// arguments erased, the shorter test's events were exactly a prefix of the longer one's.
    #[test]
    fn a_shorter_type_level_test_does_not_subsume_a_longer_one() {
        let report = type_level_report(
            ", prepare, cleanup",
            r#"
            test('checks a string mapping', () => {
                prepare();
                assertEqual<Record<string, string>, Expected>(true);
            });

            test('checks a number mapping and tidies up', () => {
                prepare();
                assertEqual<Record<string, number>, Expected>(true);
                cleanup();
            });
            "#,
        );
        assert_eq!(
            report["findings"].as_array().unwrap().len(),
            0,
            "an assertion about `string` is not contained in one about `number`: {report:#?}"
        );
    }

    /// The control that keeps the fix honest: two type-level tests that really are identical, type
    /// arguments included, are still reported. A fix that silenced the false positive by making
    /// every generic call distinct would pass the three tests above and fail this one.
    #[test]
    fn identical_type_level_tests_are_still_reported_as_duplicates() {
        let report = type_level_report(
            "",
            &[
                STRING_MAPPING_TEST,
                r#"

            test('checks a string mapping, again', () => {
                assertEqual<Record<string, string>, Expected>(true);
            });
            "#,
            ]
            .concat(),
        );
        let findings = report["findings"].as_array().unwrap();
        assert_eq!(findings.len(), 1, "{report:#?}");
        assert_eq!(findings[0]["rule"], "duplicate-test");
    }

    /// Whitespace inside a type argument is formatting, not content: a prettier run that respaced
    /// `Record<string, string>` must not hide a real duplicate.
    #[test]
    fn type_arguments_differing_only_in_whitespace_are_still_duplicates() {
        let report = type_level_report(
            "",
            &[
                STRING_MAPPING_TEST,
                r#"

            test('checks a string mapping, again', () => {
                assertEqual<
                    Record< string , string >,
                    Expected
                >(true);
            });
            "#,
            ]
            .concat(),
        );
        let findings = report["findings"].as_array().unwrap();
        assert_eq!(
            findings.len(),
            1,
            "the two type arguments name the same type: {report:#?}"
        );
        assert_eq!(findings[0]["rule"], "duplicate-test");
    }

    // =========================================================================================
    // `--emit-test-fingerprints`, end to end through the real binary.
    //
    // The records exist so a consumer can check a claim about tests that appear in NO finding --
    // which is what every `rule=none` negative fixture's participants are
    // (`scripts/check/check-fixture-isolation.mjs`). The assertions below are therefore about
    // what the analyzer saw, not about what it reported: two tests differing only in a literal
    // must share `structural` and differ in `alpha`; two differing only in a `beforeEach` must
    // share both and differ in `contextKey`. A record kind that could not distinguish those two
    // cases would leave the gate exactly where it is today.
    // =========================================================================================

    /// Every line of a `jsonl` run, parsed. Exit code is deliberately not asserted: these corpora
    /// are meant to contain findings, and a run that reports one exits 1 by design.
    /// Writes one fixture file into a fresh tempdir and returns the tempdir's root -- the
    /// four-line setup every stream/memory test in this half of the module opens with.
    /// Writes a `types.test.ts` fixture and runs every rule over it: the harness the four
    /// type-level tests reduce to. The imports and `type Expected` alias are the shared
    /// preamble (differing only in which `./helpers` names each test needs); `declarations` is
    /// the per-test `test(...)` blocks, indentation included so the written bytes match the
    /// fixtures these replaced.
    /// The plain string-mapping declaration the identical-duplicate and whitespace fixtures
    /// both open with -- the control both compare their variation against.
    const STRING_MAPPING_TEST: &str = r#"
            test('checks a string mapping', () => {
                assertEqual<Record<string, string>, Expected>(true);
            });"#;

    /// Build a temporary type-level declaration fixture and return its all-rules JSON report.
    fn type_level_report(helpers: &str, declarations: &str) -> serde_json::Value {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(
            root.join("types.test.ts"),
            format!(
                "\n            import {{ test }} from 'vitest';\n            import {{ assertEqual{helpers} }} from './helpers';\n\n            type Expected = Record<string, unknown>;\n{declarations}",
            ),
        )
        .unwrap();
        run_all_rules(root, &root.join("report"))
    }

    /// The additive-flag comparison the two stream-riding flags each get pinned by: one
    /// one-file corpus, run with and without `flag`, keeping stdout and exit code of both --
    /// everything an "adds records, edits nothing" assertion needs.
    fn jsonl_with_and_without(flag: &str) -> ((Option<i32>, String), (Option<i32>, String)) {
        let root = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let (code, stdout, _) = run_cli(root.path(), &["--reporters", "jsonl"]);
        let (flagged_code, flagged_stdout, _) =
            run_cli(root.path(), &["--reporters", "jsonl", flag]);
        ((code, stdout), (flagged_code, flagged_stdout))
    }

    /// The `--emit-test-fingerprints` stream over one fixture file plus its guaranteed-final
    /// summary record: the opening shape of every stream test that reads the summary.
    fn fingerprint_stream_and_summary(
        file: &str,
        contents: &str,
    ) -> (Vec<serde_json::Value>, serde_json::Value) {
        let lines = run_jsonl(
            single_file_project(file, contents).path(),
            &["--reporters", "jsonl", "--emit-test-fingerprints"],
        );
        let summary = lines.last().expect("a summary line is always last").clone();
        assert_eq!(summary["type"], "summary");
        (lines, summary)
    }

    /// Create and return a temporary workspace containing the requested source file.
    fn single_file_project(file: &str, contents: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(file), contents).unwrap();
        directory
    }

    /// Run the fixture CLI and parse each stdout line as an independent JSON record.
    fn run_jsonl(root: &std::path::Path, extra_args: &[&str]) -> Vec<serde_json::Value> {
        let (_, stdout, _) = run_cli(root, extra_args);
        stdout
            .lines()
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("not standalone JSON: {error}\n{line}"))
            })
            .collect()
    }

    /// Borrow records matching the requested JSONL type.
    fn records_of<'a>(lines: &'a [serde_json::Value], kind: &str) -> Vec<&'a serde_json::Value> {
        lines
            .iter()
            .filter(|line| line["type"] == kind)
            .collect::<Vec<_>>()
    }

    /// The record for the test whose full title is `title`, which must be unique in the run.
    fn record_titled<'a>(lines: &'a [serde_json::Value], title: &str) -> &'a serde_json::Value {
        let matches = records_of(lines, "test")
            .into_iter()
            .filter(|record| record["title"] == title)
            .collect::<Vec<_>>();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one test record titled {title:?}, got {matches:#?}"
        );
        matches[0]
    }

    /// Two tests that differ in exactly one literal. `structural` collapses literal values to
    /// their kind, `alpha` keeps them, so this pair is the one that pins the difference between
    /// those two fields being real and both being the same hash under another name.
    const LITERAL_ONLY_DIFFERENCE: &str = r#"
        import { test, expect } from 'vitest';
        import { add } from './math';

        test('adds one and one', () => {
            expect(add(1, 1)).toBe(2);
        });

        test('adds one and two', () => {
            expect(add(1, 2)).toBe(3);
        });
    "#;

    #[test]
    fn two_tests_differing_only_in_a_literal_share_structural_and_differ_in_alpha() {
        let directory = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let first = record_titled(&lines, "adds one and one");
        let second = record_titled(&lines, "adds one and two");

        assert_eq!(
            first["structural"], second["structural"],
            "literal values are collapsed to their kind in `structural`, so a pair differing \
             only in literals must share it: {first} / {second}"
        );
        assert_ne!(
            first["alpha"], second["alpha"],
            "`alpha` preserves literal values, so this pair must differ in it -- if it does not, \
             `alpha` and `structural` are the same field and a consumer cannot tell a \
             literal-only difference from no difference at all"
        );
        assert_eq!(
            first["contextKey"], second["contextKey"],
            "the pair shares a file, its imports, and its (empty) hook chain"
        );
        assert_eq!(first["modifierKey"], second["modifierKey"]);
        assert_eq!(first["trivial"], false);
    }

    /// Two byte-identical bodies under different hook chains. The claim a source-level gate can
    /// only approximate -- "these differ in context and in nothing else" -- is a direct field
    /// comparison here.
    const CONTEXT_ONLY_DIFFERENCE: &str = r#"
        import { test, describe, beforeEach, expect } from 'vitest';
        import { compute, reset } from './machine';

        describe('with setup', () => {
            beforeEach(() => {
                reset();
            });

            test('computes', () => {
                expect(compute()).toBe(3);
            });
        });

        describe('without setup', () => {
            test('computes', () => {
                expect(compute()).toBe(3);
            });
        });
    "#;

    #[test]
    fn two_tests_differing_only_in_a_before_each_share_alpha_and_differ_in_context_key() {
        let directory = single_file_project("machine.test.ts", CONTEXT_ONLY_DIFFERENCE);
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let with_setup = record_titled(&lines, "with setup > computes");
        let without_setup = record_titled(&lines, "without setup > computes");

        assert_eq!(
            with_setup["alpha"], without_setup["alpha"],
            "the two bodies are byte-identical: {with_setup} / {without_setup}"
        );
        assert_eq!(with_setup["structural"], without_setup["structural"]);
        assert_eq!(with_setup["behavior"], without_setup["behavior"]);
        assert_ne!(
            with_setup["contextKey"], without_setup["contextKey"],
            "one runs under a `beforeEach` and the other does not; a `contextKey` that could not \
             tell them apart would make an `isolates=context` claim uncheckable, which is the \
             entire reason this field is published"
        );

        // And the analyzer agrees with the records: differing contexts are not a duplicate.
        assert!(
            records_of(&lines, "finding")
                .iter()
                .all(|finding| finding["rule"] != "duplicate-test"),
            "a differing hook chain must suppress `duplicate-test`: {lines:#?}"
        );
    }

    #[test]
    fn tests_that_appear_in_no_finding_still_get_a_record() {
        let (lines, summary) =
            fingerprint_stream_and_summary("machine.test.ts", CONTEXT_ONLY_DIFFERENCE);
        assert_eq!(
            summary["recordCount"], 0,
            "this corpus is deliberately finding-free: {lines:#?}"
        );
        assert_eq!(
            records_of(&lines, "test").len(),
            2,
            "both tests appear in no finding, which is exactly when a consumer has no other way \
             to learn anything about them: {lines:#?}"
        );
    }

    #[test]
    fn record_count_matches_tests_analyzed_and_every_record_precedes_every_finding() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        // Two files, one with findings and one without, plus a `describe` container that must
        // NOT get a record -- `tests_analyzed` counts executable declarations only, and a
        // consumer comparing the two numbers would be misled by a suite record.
        std::fs::write(root.join("math.test.ts"), LITERAL_ONLY_DIFFERENCE).unwrap();
        std::fs::write(root.join("machine.test.ts"), CONTEXT_ONLY_DIFFERENCE).unwrap();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let summary = lines.last().unwrap();
        assert_eq!(
            records_of(&lines, "test").len(),
            summary["testsAnalyzed"].as_u64().unwrap() as usize,
            "one record per analyzed test is the whole contract; a count that disagrees with the \
             summary in the same stream makes it uncheckable: {lines:#?}"
        );

        let last_test = lines.iter().rposition(|line| line["type"] == "test");
        let first_finding = lines.iter().position(|line| line["type"] == "finding");
        assert!(
            !records_of(&lines, "finding").is_empty(),
            "this corpus must produce at least one finding for the ordering claim to mean \
             something: {lines:#?}"
        );
        assert!(
            last_test < first_finding,
            "every test record must precede every finding: {lines:#?}"
        );

        // Order is a property of the code, not of the filesystem walk.
        let again = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        assert_eq!(lines, again, "the stream must be deterministic across runs");
    }

    /// One test whose `.each` table cannot be statically read (rows come from a function call,
    /// so `EachTable::rows` is `EachRows::Unreadable` and `Context::unresolved` is set), alongside
    /// one ordinary, fully-resolved test.
    const ONE_UNRESOLVED_AND_ONE_RESOLVED: &str = r#"
        import { test, expect } from 'vitest';

        function loadCases() {
            return [[1, 2]];
        }

        test.each(loadCases())('unresolved case', (a: number, b: number) => {
            expect(a).toBeLessThan(b);
        });

        test('resolved case', () => {
            expect(1).toBeLessThan(2);
        });
    "#;

    #[test]
    fn a_record_for_an_unresolved_test_carries_comparable_false_and_a_resolved_one_carries_true() {
        let directory = single_file_project("mixed.test.ts", ONE_UNRESOLVED_AND_ONE_RESOLVED);
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let unresolved = record_titled(&lines, "unresolved case");
        let resolved = record_titled(&lines, "resolved case");

        assert_eq!(
            unresolved["comparable"], false,
            "this test's `.each` table cannot be statically read, so its context is unresolved \
             and it must be marked not-comparable rather than left indistinguishable from a \
             genuinely compared test: {unresolved}"
        );
        assert_eq!(
            resolved["comparable"], true,
            "an ordinary test with no unresolved ambient fact must be marked comparable: {resolved}"
        );
    }

    #[test]
    fn the_count_of_non_comparable_records_matches_summary_unresolved_tests() {
        let (lines, summary) =
            fingerprint_stream_and_summary("mixed.test.ts", ONE_UNRESOLVED_AND_ONE_RESOLVED);

        let non_comparable = records_of(&lines, "test")
            .into_iter()
            .filter(|record| record["comparable"] == false)
            .count();
        assert_eq!(
            non_comparable as u64,
            summary["unresolvedTests"].as_u64().unwrap(),
            "a consumer counting `comparable: false` records must land on exactly the summary's \
             `unresolvedTests`, or the two surfaces disagree about how many tests were excluded \
             from comparison: {lines:#?}"
        );
        assert_eq!(
            non_comparable, 1,
            "sanity check that this corpus actually exercises one unresolved test: {lines:#?}"
        );
    }

    #[test]
    fn the_flag_is_purely_additive_to_the_stream_it_joins() {
        let ((code_off, stdout_off), (code_on, stdout_on)) =
            jsonl_with_and_without("--emit-test-fingerprints");

        assert!(
            !stdout_off.contains("\"type\":\"test\""),
            "the default stream must not carry a record kind nobody asked for: {stdout_off}"
        );
        let on_without_records: String = stdout_on
            .lines()
            .filter(|line| !line.contains("\"type\":\"test\""))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(
            on_without_records, stdout_off,
            "with the test records removed, the flagged stream must be byte-identical to the \
             default one -- the flag adds records, it never edits, reorders, or renumbers the \
             ones that were already there"
        );
        assert_eq!(
            code_on, code_off,
            "an output flag must not move the exit code"
        );
    }

    #[test]
    fn the_flag_without_a_jsonl_reporter_is_rejected_rather_than_silently_ignored() {
        let directory = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let root = directory.path();

        for reporters in [
            vec!["--emit-test-fingerprints"],
            vec!["--reporters", "terminal", "--emit-test-fingerprints"],
            vec!["--reporters", "json", "--emit-test-fingerprints"],
        ] {
            let (code, stdout, stderr) = run_cli(root, &reporters);
            assert_eq!(
                code,
                Some(ExitCode::OperationalError.code()),
                "`--emit-test-fingerprints` with no `jsonl` reporter has nowhere to put its \
                 records; producing none of them and exiting 0 is the silent-success failure \
                 this project refuses. args={reporters:?} stdout={stdout} stderr={stderr}"
            );
            assert!(
                stderr.contains("--emit-test-fingerprints"),
                "the message must name the flag that was rejected: {stderr}"
            );
        }
    }

    // =========================================================================================
    // `--debug-memory`'s `"memory"` records.
    //
    // The instrumentation half of a memory fix: the 947 MB -> 149 MB reduction was found and
    // attributed with throwaway instrumentation that was never shipped, so nothing in the tool
    // measured retained bytes at all and the next regression would have to be rediscovered the
    // same way. These pin the contract a budget gate reads, and the two properties that make the
    // number trustworthy: the breakdown accounts for every byte of the total, and the flag adds
    // records without touching the stream it joins.
    // =========================================================================================

    #[test]
    fn the_memory_record_carries_a_breakdown_that_accounts_for_its_own_total() {
        let directory = single_file_project(
            "hooks.test.ts",
            TWO_BEFORE_EACH_HOOKS_DIFFERING_IN_A_LITERAL,
        );
        let root = directory.path();
        std::fs::write(root.join("math.test.ts"), LITERAL_ONLY_DIFFERENCE).unwrap();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--debug-memory"]);
        let records = records_of(&lines, "memory");
        assert_eq!(
            records.len(),
            1,
            "one record per measured phase, and `post-extraction` is the only phase that retains \
             IR: {lines:#?}"
        );
        let record = records[0];
        assert_eq!(record["phase"], "post-extraction");
        assert_eq!(
            record["schemaVersion"],
            vitest_dedup::reporters::JSONL_SCHEMA_VERSION
        );

        let retained = record["retainedBytes"].as_u64().unwrap();
        let tests_analyzed = record["testsAnalyzed"].as_u64().unwrap();
        assert!(
            tests_analyzed >= 4,
            "work-performed floor: a record measured over no tests proves nothing, and a budget \
             read off it would pass forever: {record}"
        );
        assert!(
            retained > 0,
            "the corpus holds titles, paths, and a hook chain; zero retained bytes would mean \
             the accounting is measuring nothing: {record}"
        );

        // `bytesPerTest` is the number a budget gate compares, so it must be exactly the quotient
        // the record's own two other numbers imply -- not a separately derived figure that could
        // drift from them.
        assert_eq!(
            record["bytesPerTest"].as_u64().unwrap(),
            retained / tests_analyzed,
            "bytesPerTest must equal retainedBytes / testsAnalyzed: {record}"
        );

        // The breakdown is what turns "memory grew" into "memory grew *here*". If it does not sum
        // to the total, a consumer cannot tell a field that grew from a field nobody accounted
        // for -- which is the exact failure this instrumentation exists to make impossible.
        let fields = record["fields"].as_object().unwrap();
        for name in [
            "body",
            "context",
            "suitePath",
            "title",
            "modifiers",
            "location",
            "spine",
            "scalars",
        ] {
            assert!(
                fields.contains_key(name),
                "the published breakdown must carry `{name}`: {record}"
            );
        }
        let summed: u64 = fields.values().map(|value| value.as_u64().unwrap()).sum();
        assert_eq!(
            summed, retained,
            "`fields` must be disjoint and exhaustive: {record}"
        );

        // `contextFields` refines `fields.context` rather than adding to it.
        let context_fields = record["contextFields"].as_object().unwrap();
        let context_summed: u64 = context_fields
            .values()
            .map(|value| value.as_u64().unwrap())
            .sum();
        assert_eq!(
            context_summed,
            fields["context"].as_u64().unwrap(),
            "`contextFields` must sum to `fields.context`, never to something else: {record}"
        );
        assert!(
            context_fields["hooks"].as_u64().unwrap() > 0,
            "this corpus has two `beforeEach` chains; a hooks bucket of zero would mean the \
             largest structure in the IR is invisible to the breakdown: {record}"
        );
    }

    /// The record's counts must be the same counts every other surface of this tool reports, or a
    /// gate reading `bytesPerTest` is dividing by a denominator nothing else agrees with.
    #[test]
    fn the_memory_record_agrees_with_the_summary_about_what_was_analyzed() {
        let directory = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let root = directory.path();
        std::fs::write(root.join("clean.test.ts"), CLEAN_TESTS).unwrap();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--debug-memory"]);
        let record = records_of(&lines, "memory")[0];
        let summary = lines.last().unwrap();
        assert_eq!(summary["type"], "summary");

        assert_eq!(
            record["testsAnalyzed"], summary["testsAnalyzed"],
            "two numbers named `testsAnalyzed` on one stream must be the same number: {lines:#?}"
        );
        assert_eq!(
            record["filesAnalyzed"], summary["filesAnalyzed"],
            "same for `filesAnalyzed`: {lines:#?}"
        );
        assert!(
            record["declarationsRetained"].as_u64().unwrap()
                >= record["testsAnalyzed"].as_u64().unwrap(),
            "every executable declaration is a retained `TestCase`, and suites add more: {record}"
        );
    }

    /// Both notices must ride on the record, because both say how to read a number beside them.
    ///
    /// There is deliberately **no assertion ordering `retainedBytes` against `peakRssBytes`**.
    /// This test used to require retained to be below peak, on the theory that peak bounds it from
    /// above. It does not: `retainedBytes` sums collection *capacity*, and pages that were
    /// reserved and never written are not resident, so a capacity-counted total can legitimately
    /// exceed a high-water mark of resident pages. The ordering held on every corpus measured so
    /// far, which is exactly what made it a latent way to reject a valid measurement rather than a
    /// live failure. `peakRssNotice` is what the record has instead, and asserting it is here is
    /// the point of this test.
    #[test]
    fn the_memory_record_says_how_to_read_peak_rss_and_shared_text() {
        let directory = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--debug-memory"]);
        let record = records_of(&lines, "memory")[0];

        let notice = record["peakRssNotice"].as_str().unwrap();
        assert!(
            notice.contains("high-water mark") && notice.contains("neither direction"),
            "the record must say what peak RSS is, in the record -- a process high-water mark that \
             bounds retained bytes in neither direction: two external reviewers have already read \
             it as though it were the retained figure: {record}"
        );
        assert!(
            record["sharedTextAttribution"].as_str().unwrap().len() > 20,
            "the record must say how shared text is charged: {record}"
        );

        // `null` on a platform with no reader wired up; a number everywhere this project's CI
        // actually runs the binary. Read and type-checked, not ordered against anything.
        assert!(
            record["peakRssBytes"].is_null() || record["peakRssBytes"].as_u64().is_some(),
            "peakRssBytes must be a non-negative integer or null: {record}"
        );
    }

    #[test]
    fn debug_memory_is_purely_additive_to_the_stream_it_joins() {
        let ((code_off, stdout_off), (code_on, stdout_on)) =
            jsonl_with_and_without("--debug-memory");

        assert!(
            !stdout_off.contains("\"type\":\"memory\""),
            "the default stream must not carry a record kind nobody asked for: {stdout_off}"
        );
        let on_without_records: String = stdout_on
            .lines()
            .filter(|line| !line.contains("\"type\":\"memory\""))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(
            on_without_records, stdout_off,
            "with the memory records removed, the flagged stream must be byte-identical to the \
             default one -- the flag adds records, it never edits or reorders the ones that were \
             already there"
        );
        assert_eq!(
            code_on, code_off,
            "an output flag must not move the exit code"
        );
    }

    /// Both stream-riding flags at once: neither may disturb the other's records, and the memory
    /// record comes first so a consumer sees it before the corpus-sized record stream.
    #[test]
    fn debug_memory_and_emit_test_fingerprints_compose() {
        let directory = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let root = directory.path();

        let (_, both, _) = run_cli(
            root,
            &[
                "--reporters",
                "jsonl",
                "--debug-memory",
                "--emit-test-fingerprints",
            ],
        );
        let (_, fingerprints_only, _) =
            run_cli(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);

        let both_without_memory: String = both
            .lines()
            .filter(|line| !line.contains("\"type\":\"memory\""))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(
            both_without_memory, fingerprints_only,
            "each flag's records must be independent of the other's: {both}"
        );
        assert!(
            both.lines().next().unwrap().contains("\"type\":\"memory\""),
            "the memory record describes a phase that is over before any other record could be \
             written, and is emitted first: {both}"
        );
    }

    #[test]
    fn debug_memory_without_a_jsonl_reporter_is_rejected_rather_than_silently_ignored() {
        let directory = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let root = directory.path();

        for arguments in [
            vec!["--debug-memory"],
            vec!["--reporters", "terminal", "--debug-memory"],
            vec!["--reporters", "json", "--debug-memory"],
            vec!["--reporters", "html,sarif", "--debug-memory"],
        ] {
            let (code, stdout, stderr) = run_cli(root, &arguments);
            assert_eq!(
                code,
                Some(ExitCode::OperationalError.code()),
                "`--debug-memory` with no `jsonl` reporter has nowhere to put its records; \
                 producing none of them and exiting 0 is the silent-success failure this project \
                 refuses. args={arguments:?} stdout={stdout} stderr={stderr}"
            );
            assert!(
                stderr.contains("--debug-memory"),
                "the message must name the flag that was rejected, not its sibling: {stderr}"
            );
        }
    }

    // =========================================================================================
    // `--emit-test-fingerprints`'s `"hook"` records.
    //
    // `redundant-hook` compares hook bodies for alpha-equality but, before this record kind
    // existed, published nothing that let a negative fixture prove its hooks were withheld for
    // the claimed reason (`fixtures/duplicates/not-redundant-hook-different-body.test.ts`'s
    // `unchecked` opt-out). These pin the same two claims that opt-out named as exactly statable:
    // a literal-only difference must still show up at `alpha` while collapsing at `structural`,
    // and two different hook kinds with byte-identical bodies must stay distinguishable.
    // =========================================================================================

    /// Sort borrowed JSONL records by their numeric line field.
    fn sorted_by_line(records: Vec<&serde_json::Value>) -> Vec<&serde_json::Value> {
        let mut records = records;
        records.sort_by_key(|record| record["line"].as_u64().unwrap());
        records
    }

    /// A `beforeEach` repeated one suite down, differing only in the string literal passed to
    /// `connect`. Mirrors `not-redundant-hook-different-body.test.ts`.
    const TWO_BEFORE_EACH_HOOKS_DIFFERING_IN_A_LITERAL: &str = r#"
        import { describe, test, beforeEach, expect } from 'vitest';

        describe('reporting', () => {
            beforeEach(() => {
                connect('primary');
            });

            test('outer', () => {
                expect(1).toBe(1);
            });

            describe('regional', () => {
                beforeEach(() => {
                    connect('replica');
                });

                test('inner', () => {
                    expect(2).toBe(2);
                });
            });
        });
    "#;

    #[test]
    fn two_before_each_hooks_differing_only_in_a_literal_share_structural_and_differ_in_alpha() {
        let directory = single_file_project(
            "hooks.test.ts",
            TWO_BEFORE_EACH_HOOKS_DIFFERING_IN_A_LITERAL,
        );
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let hooks = sorted_by_line(records_of(&lines, "hook"));
        assert_eq!(
            hooks.len(),
            2,
            "one record per beforeEach declaration: {lines:#?}"
        );
        let (outer, inner) = (hooks[0], hooks[1]);

        assert_eq!(outer["kind"], "beforeEach");
        assert_eq!(inner["kind"], "beforeEach");
        assert_eq!(
            outer["structural"], inner["structural"],
            "literal values are collapsed to their kind in `structural`, so a pair of hook \
             bodies differing only in the connected replica's name must share it: {outer} / \
             {inner}"
        );
        assert_ne!(
            outer["alpha"], inner["alpha"],
            "`alpha` preserves literal values, so this pair must differ in it -- if it does \
             not, `redundant-hook`'s own alpha-equality check could never distinguish this \
             fixture's claim from a genuine redundancy: {outer} / {inner}"
        );
    }

    /// A `beforeEach` and an `afterEach` with byte-identical bodies.
    const BEFORE_EACH_AND_AFTER_EACH_WITH_IDENTICAL_BODIES: &str = r#"
        import { describe, test, beforeEach, afterEach, expect } from 'vitest';

        describe('reporting', () => {
            beforeEach(() => {
                resetState();
            });

            afterEach(() => {
                resetState();
            });

            test('does the thing', () => {
                expect(1).toBe(1);
            });
        });
    "#;

    #[test]
    fn a_before_each_and_an_after_each_with_identical_bodies_stay_distinguishable_by_kind() {
        let directory = single_file_project(
            "hooks.test.ts",
            BEFORE_EACH_AND_AFTER_EACH_WITH_IDENTICAL_BODIES,
        );
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let hooks = records_of(&lines, "hook");
        assert_eq!(
            hooks.len(),
            2,
            "one record per hook declaration: {lines:#?}"
        );
        let before = hooks
            .iter()
            .find(|record| record["kind"] == "beforeEach")
            .expect("a beforeEach record");
        let after = hooks
            .iter()
            .find(|record| record["kind"] == "afterEach")
            .expect("an afterEach record");

        assert_eq!(
            before["alpha"], after["alpha"],
            "the bodies are byte-identical"
        );
        assert_eq!(before["structural"], after["structural"]);
        assert_eq!(before["behavior"], after["behavior"]);
        assert_ne!(
            before["kind"], after["kind"],
            "a beforeEach and an afterEach with byte-identical bodies must remain \
             distinguishable by `kind` -- a record that could not tell them apart would let a \
             consumer treat them as the same claim `redundant-hook` would never make: \
             {before} / {after}"
        );
    }

    #[test]
    fn hook_records_appear_only_with_the_flag_and_precede_every_test_record_and_finding() {
        let directory = single_file_project(
            "hooks.test.ts",
            TWO_BEFORE_EACH_HOOKS_DIFFERING_IN_A_LITERAL,
        );
        let root = directory.path();

        let (_, stdout_off, _) = run_cli(root, &["--reporters", "jsonl"]);
        assert!(
            !stdout_off.contains("\"type\":\"hook\""),
            "the default stream must not carry a record kind nobody asked for: {stdout_off}"
        );

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        assert_eq!(records_of(&lines, "hook").len(), 2, "{lines:#?}");

        let last_hook = lines.iter().rposition(|line| line["type"] == "hook");
        let first_test = lines.iter().position(|line| line["type"] == "test");
        assert!(
            last_hook < first_test,
            "every hook record must precede every test record, which itself precedes every \
             finding: {lines:#?}"
        );
    }

    #[test]
    fn a_hook_record_carries_exactly_its_documented_fields_and_nothing_from_a_test_record() {
        let directory = single_file_project(
            "hooks.test.ts",
            BEFORE_EACH_AND_AFTER_EACH_WITH_IDENTICAL_BODIES,
        );
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let hook = records_of(&lines, "hook")[0];
        let object = hook.as_object().expect("a hook record is a JSON object");
        let mut actual: Vec<&str> = object.keys().map(String::as_str).collect();
        actual.sort_unstable();
        assert_eq!(
            actual,
            [
                "alpha",
                "behavior",
                "column",
                "contentNotice",
                "kind",
                "line",
                "path",
                "schemaVersion",
                "structural",
                "trivial",
                "truncatedFields",
                "type",
            ],
            "a hook record must carry exactly these fields -- no `title`, `comparable`, \
             `contextKey`, or `modifierKey`, none of which is a well-defined fact about a hook \
             body on its own (see `HookFingerprint`'s doc comment for why each is omitted \
             rather than published with an always-the-same value): {hook}"
        );
    }

    /// A nested `beforeEach` whose body is byte-identical to the one above it, which is exactly
    /// what `redundant-hook` fires on -- so one run produces both surfaces that publish a hook
    /// `Location`: the `"hook"` fingerprint records and a `redundant-hook` finding.
    /// The hook body is `vi.clearAllMocks()` rather than an ordinary call, because since S5
    /// `redundant-hook` reports only pairs whose bodies are provably IDEMPOTENT. This constant
    /// exists to give the relativization assertions below a finding to read a `Location` off; an
    /// ordinary `connect('primary')` -- what it used to say -- is now correctly unreportable, and
    /// the test would have passed its first assertion and silently lost its second.
    const TWO_REDUNDANT_BEFORE_EACH_HOOKS: &str = r#"
        import { describe, test, beforeEach, expect, vi } from 'vitest';

        describe('reporting', () => {
            beforeEach(() => {
                vi.clearAllMocks();
            });

            test('outer', () => {
                expect(1).toBe(1);
            });

            describe('regional', () => {
                beforeEach(() => {
                    vi.clearAllMocks();
                });

                test('inner', () => {
                    expect(2).toBe(2);
                });
            });
        });
    "#;

    /// Hook locations must be relativized against the analysis root exactly as test locations
    /// are.
    ///
    /// Both surfaces that publish a hook `Location` are asserted in one run, because they share
    /// one root cause and a fix applied to only one of them would leave the other reporting
    /// checkout-specific absolute paths: the `"hook"` records of `--emit-test-fingerprints`
    /// (consumed by the isolation gate, and compared across machines) and the `locations` of a
    /// `redundant-hook` finding (which every reporter prints, and which `Finding::fingerprint`
    /// hashes into a baseline entry).
    ///
    /// The file is written one directory *below* the root so a correct result still carries a
    /// directory component: asserting only on the basename would pass against an implementation
    /// that truncated the path instead of stripping the root prefix.
    #[test]
    fn hook_locations_are_relativized_against_the_analysis_root_like_test_locations() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("suites")).unwrap();
        std::fs::write(
            root.join("suites/hooks.test.ts"),
            TWO_REDUNDANT_BEFORE_EACH_HOOKS,
        )
        .unwrap();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);

        let hooks = records_of(&lines, "hook");
        assert_eq!(hooks.len(), 2, "one record per beforeEach: {lines:#?}");
        for hook in &hooks {
            assert_eq!(
                hook["path"], "suites/hooks.test.ts",
                "a hook record's path must be relative to the analysis root, exactly as a test \
                 record's is -- an absolute path here differs on every machine and checkout, \
                 which makes this stream's `\"hook\"` records undiffable for the isolation gate \
                 that consumes them: {hook}"
            );
        }

        // Sibling surface, same `Location`: the finding `emit_redundant_hook` builds directly
        // from `hook.location`.
        let redundant = records_of(&lines, "finding")
            .into_iter()
            .filter(|record| record["rule"] == "redundant-hook")
            .collect::<Vec<_>>();
        assert_eq!(
            redundant.len(),
            1,
            "the nested beforeEach repeats the outer one verbatim, so exactly one \
             `redundant-hook` finding must fire -- without it this test would assert nothing \
             about the finding surface: {lines:#?}"
        );
        let locations = redundant[0]["locations"]
            .as_array()
            .expect("a finding carries an array of locations");
        assert_eq!(locations.len(), 2, "{}", redundant[0]);
        for location in locations {
            assert_eq!(
                location["path"], "suites/hooks.test.ts",
                "a `redundant-hook` finding's locations come straight from `hook.location`, so \
                 they must be relativized too -- otherwise this one rule names an absolute path \
                 where every other rule names a relative one, and its baseline fingerprint \
                 (hashed from these paths) stops matching across checkouts: {}",
                redundant[0]
            );
        }

        // The test records in the same run are the control: whatever form they take, the hook
        // records above must take the identical one.
        for test in records_of(&lines, "test") {
            assert_eq!(
                test["path"], "suites/hooks.test.ts",
                "sanity: test locations were already relativized, and are what the hook \
                 locations above are being held to: {test}"
            );
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Rows specified by the 2026-09-27 tests-implementation plan (S2/S12 blocks)
    // ---------------------------------------------------------------------------------------------

    /// Runs the real binary over `root` and returns its stderr, for the diagnostic-path rows.
    fn run_stderr(root: &std::path::Path, expected: ExitCode) -> String {
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(root)
            .args(["--reporters", "json", "--output"])
            .arg(root.join("report"))
            .output()
            .expect("binary runs");
        assert_eq!(
            output.status.code(),
            Some(expected.code()),
            "the stderr fixture must have the expected process outcome"
        );
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    /// Writes the one file whose analysis carries a diagnostic: a fatal parse error.
    fn write_broken_parse_file(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/broken.test.ts"), "test(: broken {\n").unwrap();
    }

    /// R16 (S2): a run that discovered NO files must be incomplete with a named cause. Before
    /// the fix, the `nothing_read` guard was only reached when files were discovered but yielded
    /// nothing (`tests.is_empty() && !discovered.files.is_empty()`); zero discovered files
    /// cleared every flag and a JSON consumer read a clean, complete pass over nothing -- the
    /// threshold trap of CLAUDE.local.md §2 in the product.
    #[test]
    fn a_run_that_discovered_no_files_is_incomplete_with_a_named_cause() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("vitest.config.ts"),
            "export default defineConfig({ test: { include: ['nothing/here/**/*.test.ts'] } });",
        )
        .unwrap();
        let prefix = dir.path().join("report");
        let prefix_arg = prefix.to_string_lossy().into_owned();
        let (code, _, stderr) = run_cli(
            dir.path(),
            &["--reporters", "json", "--output", &prefix_arg],
        );
        assert_eq!(code, Some(ExitCode::OperationalError.code()), "{stderr}");
        let report = read_json_report(&prefix);
        assert_eq!(report["incomplete"], serde_json::json!(true));
        assert_eq!(
            report["incompleteCauses"]["nothingRead"],
            serde_json::json!(true),
            "the cause flag must name WHY the run measured nothing: {}",
            report["incompleteCauses"]
        );
    }

    /// T5: discovering a test-shaped file is not evidence that any Vitest declaration was
    /// measured. Competing-runner files are the sharp control: they must remain excluded, and a
    /// corpus containing only them must fail operationally instead of reporting a clean exit.
    #[test]
    fn discovered_but_unextracted_files_exit_operationally() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("native.test.ts"),
            "import { describe, it } from 'node:test';\n\
             describe('native suite', () => { it('native case', () => {}); });",
        )
        .unwrap();
        let prefix = dir.path().join("report");
        let prefix_arg = prefix.to_string_lossy().into_owned();
        let (code, _, stderr) = run_cli(
            dir.path(),
            &["--reporters", "json", "--output", &prefix_arg],
        );

        assert_eq!(code, Some(ExitCode::OperationalError.code()), "{stderr}");
        assert!(stderr.contains("discovered 1 test file(s) but extracted no tests"));
        let report = read_json_report(&prefix);
        assert_eq!(report["testsAnalyzed"], serde_json::json!(0));
        assert_eq!(
            report["incompleteCauses"]["nothingRead"],
            serde_json::json!(true)
        );
    }

    /// A suite container is retained for `duplicate-suite`, but it is not an executable test and
    /// must not turn an empty measurement into a clean quality result.
    #[test]
    fn suite_only_files_exit_operationally() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("suite-only.test.ts"),
            "import { describe } from 'vitest';\n\
             describe('container without children', () => {});",
        )
        .unwrap();
        let prefix = dir.path().join("report");
        let prefix_arg = prefix.to_string_lossy().into_owned();
        let (code, _, stderr) = run_cli(
            dir.path(),
            &["--reporters", "json", "--output", &prefix_arg],
        );

        assert_eq!(code, Some(ExitCode::OperationalError.code()), "{stderr}");
        assert!(
            stderr.contains("only suite containers and no executable tests"),
            "the diagnostic must distinguish a retained container from a test that runs: {stderr}"
        );
        let report = read_json_report(&prefix);
        assert_eq!(report["testsAnalyzed"], serde_json::json!(0));
        assert_eq!(
            report["incompleteCauses"]["nothingRead"],
            serde_json::json!(true)
        );
    }

    /// Two executable-empty suite containers can still produce a real suite-level quality
    /// finding. That finding keeps exit 1 even though the report correctly says zero tests ran.
    #[test]
    fn suite_only_quality_findings_keep_the_quality_gate_exit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("duplicate-suites.test.ts"),
            "import { describe } from 'vitest';\n\
             describe('copied suite', () => { const value = setup(); });\n\
             describe('copied suite', () => { const value = setup(); });",
        )
        .unwrap();
        let prefix = dir.path().join("report");
        let prefix_arg = prefix.to_string_lossy().into_owned();
        let (code, _, stderr) = run_cli(
            dir.path(),
            &[
                "--rule",
                "duplicate-suite=error",
                "--reporters",
                "json",
                "--output",
                &prefix_arg,
            ],
        );

        assert_eq!(code, Some(ExitCode::QualityGateFailed.code()), "{stderr}");
        let report = read_json_report(&prefix);
        assert_eq!(report["testsAnalyzed"], serde_json::json!(0));
        assert_eq!(
            report["incompleteCauses"]["nothingRead"],
            serde_json::json!(true)
        );
        assert!(
            report["findings"]
                .as_array()
                .is_some_and(|findings| findings
                    .iter()
                    .any(|finding| { finding["rule"] == serde_json::json!("duplicate-suite") })),
            "the quality exit must be backed by the suite finding: {report:#?}"
        );

        let incomplete_prefix = dir.path().join("incomplete-report");
        let incomplete_prefix_arg = incomplete_prefix.to_string_lossy().into_owned();
        let (incomplete_code, _, incomplete_stderr) = run_cli(
            dir.path(),
            &[
                "--rule",
                "duplicate-suite=error",
                "--fail-on-incomplete",
                "--reporters",
                "json",
                "--output",
                &incomplete_prefix_arg,
            ],
        );
        assert_eq!(
            incomplete_code,
            Some(ExitCode::OperationalError.code()),
            "an explicitly requested incomplete gate outranks the suite quality finding: \
             {incomplete_stderr}"
        );
    }

    /// Negative control for the suite-only case: the same container with one executable child is
    /// a real one-test corpus and must retain the ordinary clean exit.
    #[test]
    fn a_suite_with_an_executable_child_is_analyzed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("suite-with-child.test.ts"),
            "import { describe, test } from 'vitest';\n\
             describe('container', () => { test('child', () => {}); });",
        )
        .unwrap();
        let prefix = dir.path().join("report");
        let prefix_arg = prefix.to_string_lossy().into_owned();
        let (code, _, stderr) = run_cli(
            dir.path(),
            &["--reporters", "json", "--output", &prefix_arg],
        );

        assert_eq!(code, Some(ExitCode::Success.code()), "{stderr}");
        let report = read_json_report(&prefix);
        assert_eq!(report["testsAnalyzed"], serde_json::json!(1));
        assert_eq!(
            report["incompleteCauses"]["nothingRead"],
            serde_json::json!(false)
        );
    }

    /// R26 (S12): stderr diagnostic paths are relativized against the analysis root, like every
    /// `locations[].path` in every report already is. Before the fix, one site was relativized
    /// and its sibling was not -- the shape CLAUDE.local.md §3 keeps catching.
    #[test]
    fn stderr_diagnostic_paths_are_relativized_against_the_analysis_root() {
        let dir = tempfile::tempdir().unwrap();
        write_broken_parse_file(dir.path());
        let stderr = run_stderr(dir.path(), ExitCode::OperationalError);
        assert!(
            stderr.contains("src/broken.test.ts"),
            "the diagnostic names its file relative to the root: {stderr}"
        );
        assert!(
            !stderr.contains(dir.path().display().to_string().as_str()),
            "the absolute analysis root must not appear: {stderr}"
        );
    }

    /// R27: the sweep form of R26 -- the property holds over EVERY stderr line, not only the one
    /// site the fix touches. A fix applied to the reported list but not the "could not be fully
    /// analyzed" banner (or the reverse) passes R26 and fails here. The one deliberate
    /// exception is discovery's traversal-error line, which names the analysis root the caller
    /// passed: relativized it would read as a bare `.`, saying nothing.
    #[test]
    fn no_stderr_line_contains_the_analysis_root_prefix() {
        let dir = tempfile::tempdir().unwrap();
        write_broken_parse_file(dir.path());
        // A broken-parse file alone reaches exactly ONE of the sites that print a path, which is
        // how this sweep passed while two others still printed absolute paths: `discovery.rs`'s
        // governance-reason error and `main.rs`'s `includeSource` globals diagnostic, the latter
        // the untouched sibling of a site the same commit relativized. A sweep whose fixture
        // cannot reach a site is not sweeping it. Every shape below exists to reach one more.
        //
        // - an unresolved `test.projects` entry, so every discovered file draws a governance
        //   diagnostic and the project-map banner fires;
        // - a file whose every test has an unresolved context, so the blind-file listing prints;
        // - the broken-parse file above, for the extraction-diagnostic path.
        std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
        std::fs::write(
            dir.path().join("pkg/api.spec.ts"),
            "import { it } from 'vitest'; it('works', () => {});\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/blind.test.ts"),
            "import { it, expect } from 'vitest';\n\
             import { rows } from './rows';\n\
             it.each(rows())('case %s', (v) => { expect(v).toBe(v); });\n",
        )
        .unwrap();
        // An in-source candidate admitted by a project whose own `globals` cannot be read: the
        // site that printed an absolute path while its sibling six lines below was relativized.
        std::fs::write(
            dir.path().join("src/in-source.ts"),
            "export const add = (a: number, b: number) => a + b;\n\
             if (import.meta.vitest) { test('adds', () => { expect(add(1, 1)).toBe(2); }); }\n",
        )
        .unwrap();
        // A helper chain past `MAX_HELPER_EXPANSION_DEPTH`, so S11's refusal diagnostic runs.
        // Without it that site printed an absolute path and this sweep could not see it: the
        // fixture reached no refusal at all. A sweep whose fixture cannot reach a site is not
        // sweeping it -- the same lesson as the four shapes above.
        std::fs::write(dir.path().join("src/deep-helper.test.ts"), {
            let mut lines = vec![
                "import { it, expect } from 'vitest';".to_owned(),
                "h0();".to_owned(),
            ];
            for level in 0..12 {
                lines.push(format!("function h{level}(){{ h{}(); }}", level + 1));
            }
            lines.push("function h12(){ it('deep', () => { expect(1).toBe(1); }); }".to_owned());
            lines.join("\n")
        })
        .unwrap();
        // Oversized, so discovery records a `SkippedFile` and the skipped-diagnostic loop runs.
        // Without this the loop never executes and its relativization is unasserted -- measured:
        // reverting it changed no test.
        std::fs::write(
            dir.path().join("src/huge.test.ts"),
            "x".repeat(discovery::MAX_SOURCE_FILE_BYTES as usize + 1),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("vitest.config.ts"),
            "import { makeProjects } from './projects';\n\
             export default defineConfig({ test: { projects: [makeProjects('other'), \
             { test: { name: 'pkg', globals: someHelper(), \
             includeSource: ['src/**/*.ts'], include: ['pkg/**/*.spec.ts'] } }] } });",
        )
        .unwrap();

        let stderr = run_stderr(dir.path(), ExitCode::Success);
        let root = dir.path().display().to_string();
        let offenders: Vec<&str> = stderr
            .lines()
            .filter(|line| line.contains(root.as_str()))
            .collect();
        assert!(
            offenders.is_empty(),
            "every stderr line must be free of the absolute analysis root: {offenders:?}"
        );

        // A work-performed floor on the sweep itself. Without it, a fixture that stopped
        // producing diagnostics -- a config this scan learns to read, a shape that stops being
        // degraded -- would leave the assertion above true over almost no lines and still green.
        // These are the four shapes the fixture exists to reach.
        for expected in [
            "the Vitest project map is incomplete",
            "could not be fully analyzed",
            "had EVERY test excluded",
            "fatal parse error",
            "exceeds the",
            "includeSource",
            "a called helper's body was not analyzed",
        ] {
            assert!(
                stderr.contains(expected),
                "the fixture no longer reaches the `{expected}` path, so this sweep is not \
                 sweeping it: {stderr}"
            );
        }
    }

    /// S2's second half, at the boundary where the two outputs meet: an unresolved
    /// `test.projects` entry degrades the run in the JSON report even when every discovered
    /// file attributes to a readable project. Thirteen repositories in the 2026-09-27 scan
    /// printed the "project map is incomplete" warning with `projectConfigDegraded: false`,
    /// because only discovery's fell-through-to-defaults path set the flag.
    #[test]
    fn an_unresolved_projects_entry_degrades_the_json_run_even_when_every_file_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("pkg");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("api.spec.ts"),
            "import { it } from 'vitest'; it('works', () => {});\n",
        )
        .unwrap();
        // Entry ORDER is the whole discrimination, and the first draft of this test got it
        // wrong. With the unreadable entry declared AFTER the project that matches, the file is
        // `IncludeDegraded` and discovery raises `project_config_degraded` on its own -- the test
        // passed identically with the fix reverted, measured. Declared BEFORE, "later wins"
        // settles attribution in `pkg`'s favour, discovery stays silent, and the only thing that
        // knows the map was incomplete is the CLI. Reverted, this run reports
        // `incomplete: false` with every cause clear while stderr prints "the Vitest project map
        // is incomplete" -- the exact disagreement S2's second half is about.
        std::fs::write(
            dir.path().join("vitest.config.ts"),
            "import { makeProjects } from './projects';\nexport default defineConfig({ test: { projects: [makeProjects('other'), { test: { name: 'pkg', include: ['pkg/**/*.spec.ts'] } }] } });",
        )
        .unwrap();
        let report = run_json(dir.path(), &[]);
        assert_eq!(
            report["testsAnalyzed"],
            serde_json::json!(1),
            "precondition: the file still attributes and is analyzed -- this is not a zero-file \
             run wearing a different hat: {report}"
        );
        assert_eq!(
            report["incompleteCauses"]["projectConfigDegraded"],
            serde_json::json!(true),
            "the flag must say what the stderr banner says: {}",
            report["incompleteCauses"]
        );
        assert_eq!(
            report["incomplete"],
            serde_json::json!(true),
            "and the aggregate a CI consumer actually branches on must follow: {report}"
        );
    }
}

#[cfg(test)]
#[path = "cli/tests/imported_delivery.rs"]
mod imported_delivery_tests;
