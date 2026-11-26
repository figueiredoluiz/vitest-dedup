//! The `vitest-dedup` command-line entry point.
//!
//! Orchestrates discovery, extraction, analysis and reporting, including CLI-specific path
//! handling, diagnostics, source retention and output writing. Library modules expose the
//! individual stages; this binary composes them and preserves their exit-code contract.

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode as ProcessExitCode;
use std::time::Instant;
use vitest_dedup::cli::{ExitCode, ParsedArgs, ReporterKind};
use vitest_dedup::config::Config;
use vitest_dedup::exclusions::{self, ExclusionCollector};
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
mod stdout;

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

/// Relativizes a diagnostic path, normalizes separators and sanitizes repository-controlled text.
/// All stderr path diagnostics use this helper so display rules remain consistent.
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

/// Makes a test path root-relative when possible. Existing paths get a canonical comparison
/// if lexical stripping fails, including equivalent Windows verbatim-prefix spellings.
/// Paths pass through unchanged only when lexical stripping and canonical comparison both fail.
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
    vitest_dedup::memory::MemoryRoof::new(memory_budget_bytes(args))
}

/// The resolved byte limit shared by retained corpus and historical-index construction.
fn memory_budget_bytes(args: &ParsedArgs) -> Option<u64> {
    args.memory_budget_mib
        .map(|mib| mib.saturating_mul(1024 * 1024))
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

/// Reads each helper source the engine used once: its inline suppressions apply, and the text is
/// kept for the HTML report when `retain` is given the map and its running size.
fn take_helper_sources(
    sources: impl IntoIterator<Item = (PathBuf, String)>,
    root: &Path,
    seen: &mut BTreeSet<PathBuf>,
    suppressions: &mut Vec<modes::InlineSuppression>,
    mut retain: Option<(&mut BTreeMap<PathBuf, String>, &mut usize)>,
) {
    for (path, source) in sources {
        if !seen.insert(path.clone()) {
            continue;
        }
        // The engine names its helpers root-relative already; resolving one against the
        // working directory would join it to a different file of the same relative name.
        let relative = if path.is_absolute() {
            relativize_test_path(&path, root)
        } else {
            path.clone()
        };
        suppressions.extend(modes::scan_inline_suppressions(&source, relative.clone()));
        if let Some((kept, text)) = retain.as_mut() {
            let length = source.len();
            if kept.insert(relative, source).is_none() {
                **text += length;
            }
        }
    }
}

/// Parse arguments and run discovery, extraction, analysis and reporting with native RSS samples.
fn run() -> Result<ExitCode> {
    run_with_rss(ParsedArgs::parse()?, rss::current_bytes)
}

/// Run the pipeline with an injectable current-RSS reader for deterministic boundary tests.
fn run_with_rss(args: ParsedArgs, resident: impl FnMut() -> Option<u64>) -> Result<ExitCode> {
    run_with_engine(args, resident, vitest_dedup::corpus::Engine::default())
}

/// [`run_with_rss`] with the engine given.
fn run_with_engine(
    args: ParsedArgs,
    mut resident: impl FnMut() -> Option<u64>,
    mut engine: vitest_dedup::corpus::Engine,
) -> Result<ExitCode> {
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
    // Which Vitest each file imports, read once per directory; it decides whether `bench` exists.
    let mut vitest_majors = vitest_dedup::framework::vitest::version::Majors::new(
        &config.root,
        args.vitest_major.or(config.vitest_major),
    );
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
                " (truncated at a project discovery limit)"
            } else {
                ""
            }
        )?;
    }

    let discovered = discovery::discover(&config, project_scoping)?;

    // An explicit `--globals` always wins: the user is asserting something about their project,
    // and a detector that overrode them would be unarguable. Otherwise read the Vitest config.
    let resolved_globals = if args.globals {
        true
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
                true
            }
            GlobalsDetection::Disabled { .. } => false,
            // Conservative by design. Guessing `Enabled` would invent findings in Playwright and
            // Jest files; guessing `Disabled` silently analyzes nothing. The zero-test check below
            // is what stops the second failure from looking like success.
            // Only worth saying when there was something to read and it was inconclusive. A
            // project with no Vitest config at all is the ordinary case for a plain library, and
            // announcing it on every run is noise that trains people to ignore the channel the
            // genuinely important warnings use. The zero-test guard below is what catches the
            // case where this silence would have been wrong.
            GlobalsDetection::Unknown { reason } if reason.starts_with("no vitest.config") => false,
            GlobalsDetection::Unknown { reason } => {
                // `reason` may echo back filesystem-derived text (a path, a parse error); sanitize
                // it for the same reason as the branch above.
                writeln!(
                    stderr,
                    "vitest-dedup: could not determine `test.globals` ({}).",
                    Sanitized::new(&reason)
                )?;
                false
            }
            // `GlobalsDetection` is `#[non_exhaustive]`, so a future variant must not silently
            // become "enabled". Anything unrecognized is treated as the conservative case.
            _ => false,
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
            let mut base_causes = IncompleteCauses::default();
            let base_roof = memory_roof(&args);
            let mut base_files = 0;
            let revision_files = std::sync::Arc::new(modes::RevisionFiles::load(
                &config.root,
                revision,
                memory_budget_bytes(&args),
            )?);
            let mut base_engine = vitest_dedup::corpus::Engine::with_sources(
                vitest_dedup::lang::Sources::committed(revision_files.clone()),
            );
            let mut base_suppressions = Vec::new();
            let mut base_helper_sources = BTreeSet::new();
            // Titles render as the base revision's own committed Vitest major.
            let mut base_majors = vitest_dedup::framework::vitest::version::Majors::committed(
                &config.root,
                args.vitest_major.or(config.vitest_major),
                revision_files.clone(),
            );
            // One base file is held at a time; the roof stops the read, not only the extraction.
            let unread = modes::revision_sources(
                &config.root,
                revision_files.commit(),
                &config.include,
                &config.exclude,
                |source| {
                    base_files += 1;
                    {
                        let engine = &mut base_engine;
                        let files = &revision_files;
                        let path = config.root.join(&source.relative);
                        files.regular_root(&path)?;
                        base_suppressions.extend(modes::scan_inline_suppressions(
                            &source.content,
                            source.relative.clone(),
                        ));
                        let read = engine.read_with_sources(
                            &path,
                            &source.relative,
                            &source.content,
                            vitest_dedup::corpus::Setting {
                                dialect: vitest_dedup::framework::current().dialect(),
                                globals: resolved_globals,
                                root: &config.root,
                                // From the current tree, as `globals` is: config is not re-read
                                // against the base revision.
                                benchmarks: projects.collects_benchmarks(&path)
                                    && vitest_majors.exports_bench(&path),
                                formatting: base_majors.formatting(&path),
                            },
                            false,
                            Vec::new(),
                        );
                        let partial = read.partial;
                        // The committed helper text, so a directive the base revision carried
                        // suppresses the finding it suppressed there.
                        take_helper_sources(
                            read.helper_sources,
                            &config.root,
                            &mut base_helper_sources,
                            &mut base_suppressions,
                            None,
                        );
                        files.ensure_readable()?;
                        base_causes.files_partially_read += usize::from(partial);
                        base_causes.unresolved_contexts = engine.unresolved;
                        Ok(!base_roof.exceeded_with_heap(
                            vitest_dedup::memory::HeapBytes::heap_bytes(engine),
                            &mut resident,
                        ))
                    }
                },
            )?;
            if unread > 0 {
                base_causes.files_over_memory_budget = unread;
                write_roof_diagnostic(&mut stderr, "base revision", &args, base_files, unread)?;
            }
            let configuration_degraded = revision_files.configuration_degraded();
            base_causes.project_config_degraded |= configuration_degraded;
            let (mut base_findings, base_incomplete) = {
                let mut engine = base_engine;
                let files = engine.take_files();
                let outcome = vitest_dedup::rules::run_with_hooks(
                    &files,
                    engine.suites(),
                    engine.hooks(),
                    &config,
                    &base_suppressions,
                );
                base_causes.pair_budget_exhausted |= outcome.budget_exhausted;
                base_causes.hook_scan_exhausted |= outcome.hook_scan_exhausted;
                let finished = analysis::finish_findings(
                    outcome.findings,
                    base_causes,
                    analysis::RunModes {
                        changed_files: None,
                        baseline: None,
                        suppressions: &base_suppressions,
                    },
                );
                (finished.findings, finished.causes.any())
            };
            // The base read records no gaps here: its committed-view resolver reports an unusable
            // tsconfig or a missing package through the revision tree, which the base outcome
            // already reflects. The drain keeps the main pass's report to its own resolutions,
            // whatever ran before this point.
            let _ = vitest_dedup::lang::take_tsconfig_gaps();
            let _ = vitest_dedup::lang::take_package_gaps();
            // A base run that could not see the whole tree (oversized, non-UTF-8, or skipped
            // files) produces a PARTIAL baseline: it accepts less than the base really had, so
            // the gate reports findings as new that the base would have accepted. The safe
            // direction for a gate, but only if it is visible -- same principle as refusing
            // `--update-baseline` on an incomplete run, expressed as a warning because the file
            // baseline's refusal has no equivalent here: there is nothing to refuse, the
            // baseline already is what it is.
            if configuration_degraded {
                // Unknown aliases can affect any cached identity; none is proved allowance.
                base_findings.clear();
                writeln!(stderr, "vitest-dedup: unavailable historical package resolution; withholding all revision baseline allowance.")?;
            }
            if base_incomplete {
                writeln!(
                    stderr,
                    "vitest-dedup: warning: the {} analysis was incomplete, so the \
                     baseline may be partial and the gate stricter than intended.",
                    terminal_safe(revision)
                )?;
            }
            let baseline = modes::baseline_from_findings(&base_findings);
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
    let mut excluded = ExclusionCollector::new(&config.root);
    let mut diagnostics: Vec<Sanitized> = Vec::new();
    // Distinct from `diagnostics.len()`: one file can push several diagnostics (one per
    // extraction-time issue), so the diagnostic count and the affected-file count disagree in
    // general and neither may stand in for the other. Discovery-level traversal errors (below)
    // are not tied to one file at all, so they are never added here.
    let mut diagnostic_files: BTreeSet<PathBuf> = BTreeSet::new();
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
        // Whole files only: stopping before a file, once the files read so far reach the budget,
        // keeps every per-file side effect consistent, so the run is exactly a run over them.
        if index > 0
            && roof.exceeded_with_heap(
                vitest_dedup::memory::HeapBytes::heap_bytes(&engine),
                &mut resident,
            )
        {
            let remaining = discovered.files.len() - index;
            causes.files_over_memory_budget = remaining;
            for skipped in &discovered.files[index..] {
                excluded.record(exclusions::unparsed_file(
                    relativize_test_path(&skipped.path, &config.root),
                    ExclusionReason::MemoryBudget,
                    "the memory budget was reached before this file was analyzed",
                ));
            }
            write_roof_diagnostic(&mut stderr, "analysis", &args, files_read, remaining)?;
            break;
        }
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
        // Per file, not per run: two projects in one repository can disagree about `globals`,
        // and applying one project's answer to another's files is how ambient `describe` in a
        // Jest or Playwright suite becomes a finding.
        let (file_globals, unresolved_globals) = if args.globals {
            (true, None)
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
                GlobalsDetection::Enabled { .. } => (true, None),
                GlobalsDetection::Disabled { .. } => (false, None),
                // The admitting project's own `globals` could not be read. Unlike the "no
                // governing project" fallback below, this file *was* attributed to a specific
                // project, just not confidently -- silently borrowing the repository-wide answer
                // here would recreate the very silent-omission failure `includeSource` support
                // was supposed to close. Mark the run incomplete instead of guessing.
                _ => (
                    false,
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
                vitest_projects::GovernedGlobals::Enabled => (true, None),
                vitest_projects::GovernedGlobals::Disabled => (false, None),
                vitest_projects::GovernedGlobals::NotIncluded => (resolved_globals, None),
                // A project governs this file (or might), but its own `globals`, or the
                // `include`/`exclude` that would confirm which project governs, could not be
                // read, and no unanimous-answer shortcut saved it. Sibling of the
                // `file.project_globals` arm above: mark the run incomplete instead of guessing.
                _ => (
                    false,
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
        let setting = vitest_dedup::corpus::Setting {
            dialect: vitest_dedup::framework::current().dialect(),
            globals: file_globals,
            root: &config.root,
            // An in-source candidate is not a benchmark file, whatever its glob.
            benchmarks: file.project_globals.is_none()
                && projects.collects_benchmarks(&file.path)
                && vitest_majors.exports_bench(&file.path),
            formatting: vitest_majors.formatting(&file.path),
        };
        let relative = relativize_test_path(&file.path, &config.root);
        let read = engine.read_with_sources(
            &file.path,
            &relative,
            &source,
            setting,
            args.emit_test_fingerprints,
            unresolved_globals
                .map(|reason| (ExclusionReason::GlobalsUndetermined, reason.to_owned()))
                .into_iter()
                .collect(),
        );
        let (entry, partial) = (read.excluded_file, read.partial);
        take_helper_sources(
            read.helper_sources,
            &config.root,
            &mut seen_helper_sources,
            &mut inline_suppressions,
            retain_sources.then_some((&mut retained_sources, &mut retained_text)),
        );
        // A file read only in part is named beside its primary cause; the reports carry every
        // reason.
        if let (true, Some(entry)) = (partial, &entry) {
            diagnostics.push(Sanitized::new(&format!(
                "{}: {}",
                display_relative(&file.path, &config.root),
                entry.detail
            )));
            diagnostic_files.insert(file.path.clone());
        }
        excluded.extend(entry);
        causes.files_partially_read += usize::from(partial);
        // Source kept for the HTML report is held whichever engine read the file.
        roof.charge(&[], retained_text);
    }

    drop(seen_helper_sources);

    // A broken project configuration or a missing install degrades the run: the gaps the resolver
    // recorded are what the prerequisites name.
    let resolution_gaps = vitest_dedup::lang::ResolutionGaps {
        tsconfig: vitest_dedup::lang::take_tsconfig_gaps(),
        packages_not_found: vitest_dedup::lang::take_package_gaps(),
    };
    causes.missing_prerequisites =
        vitest_dedup::prerequisites::assess(&config.root, &resolution_gaps);

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
    let total_tests = engine.tests;

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
            let proven_empty_files: BTreeSet<PathBuf> = engine
                .proven_empty_files()
                .iter()
                .map(|relative| config.root.join(relative))
                .collect();
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
            } else if engine.containers > 0 {
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

    let unresolved_tests = engine.unresolved;

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
        vec![engine.memory_report(files_read)]
    } else {
        Vec::new()
    };

    let parsing_time = parsing_started.map(|started| started.elapsed());
    let analysis_started = metrics_enabled.then(Instant::now);
    // The rules read the corpus the engine retained once every file has been read.
    let engine_files = engine.take_files();
    let outcome = vitest_dedup::rules::run_with_hooks(
        &engine_files,
        engine.suites(),
        engine.hooks(),
        &config,
        &inline_suppressions,
    );
    causes.pair_budget_exhausted |= outcome.budget_exhausted;
    causes.hook_scan_exhausted |= outcome.hook_scan_exhausted;
    causes.unresolved_contexts = engine.unresolved;
    let mut finished = analysis::finish_findings(
        outcome.findings,
        causes,
        analysis::RunModes {
            changed_files: changed_files.as_ref(),
            // A file baseline and a ref baseline are mutually exclusive by the CLI grammar; one
            // of these is always `None`.
            baseline: baseline.as_ref().or(ref_baseline.as_ref()),
            suppressions: &inline_suppressions,
        },
    );
    finished.modes.suppressions_applied += outcome.inline_suppressed;
    let (excluded_files, exclusions) = excluded.finish();
    let mut result = AnalysisResult {
        had_error_finding: finished.had_error_finding,
        error_cases: finished.error_cases,
        unresolved_tests: engine.unresolved,
        blind_files: engine.blind_files(),
        excluded_files,
        exclusions,
        findings: finished.findings,
        tests_analyzed: engine.tests,
        eligible_cases: engine.eligible,
        files_analyzed: files_read,
        incomplete_causes: finished.causes,
        modes: finished.modes,
    };

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
    let jsonl_reporter = JsonlReporter::new()
        .with_memory(memory_reports)
        .with_owned(
            if args.emit_test_fingerprints {
                engine_files
            } else {
                Vec::new()
            },
            engine.take_diagnostic_hooks(),
        )
        .with_sites(engine.into_sites());

    let exit_code = final_exit_code(args.fail_on_incomplete, args.fail_on_new, &result, &config);
    let terminal_context = TerminalReportContext::new(&config, exit_code).with_timing(timing);
    let mut stdout = stdout::PipeWriter::new(std::io::stdout().lock());
    let mut written = Vec::new();
    for reporter in &args.reporters {
        match reporter {
            ReporterKind::Terminal => {
                let color = terminal_color_enabled(
                    stdout.inner().is_terminal(),
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

    /// A path on another Windows drive keeps its absolute identity.
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
}
