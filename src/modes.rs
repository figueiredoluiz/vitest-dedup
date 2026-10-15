//! Baseline and changed-file modes: the adoption path for a repository that already has findings.
//!
//! A team with hundreds of existing findings cannot turn a quality gate on. Their only options are
//! failing the build on day one or setting `--threshold` so high it gates nothing. Both are worse
//! than not running the tool. This module is the third option: record what the repository already
//! has, gate on what is added to it.
//!
//! Two independent mechanisms live here, and they answer different questions.
//!
//! - **A baseline** (`--baseline`) answers "which findings has this repository already accepted?"
//!   It is keyed on [`crate::model::Finding::fingerprint`], which is location-independent, so an
//!   accepted finding stays accepted when unrelated lines move above it.
//! - **Changed-file scoping** (`--changed-since`) answers "which findings touch what I just
//!   changed?" It narrows the *report*, never the analysis -- see "Changed files narrow the
//!   report, not the corpus" below, which is the single most important thing in this module.
//!
//! - **Inline suppressions** (`// vitest-dedup-disable-next-line` in the analyzed source)
//!   answer "this specific pair is intentional, and here is why". A baseline says "accept this
//!   debt"; a directive says "this is not debt". The directive is applied *before* the baseline,
//!   so an intentional pair never consumes baseline allowance and never lands in a
//!   `--update-baseline` file as accepted debt -- see [`apply_inline_suppressions`].
//!
//! Both modes above are modeled on CukeDedup's `src/modes.rs` (`--baseline`,
//! `--update-baseline`, `--fail-on-new`, `--changed-since`), down to the flag names: the same
//! author maintains both tools, and someone who has configured one should not have to relearn
//! the other. Deliberate differences are called out where they occur. The inline suppression is
//! VitestDedup's own (CukeDedup has no counterpart); it follows ESLint's
//! `eslint-disable-next-line` convention because that is where the ecosystem's muscle memory
//! lives.
//!
//! # Suppressed is not invisible
//!
//! A baselined finding is **still reported**, by every reporter, marked
//! [`Finding::baselined`](crate::model::Finding::baselined). It is excluded from the quality gate
//! and from nothing else. A mechanism that made accepted debt disappear from the report would turn
//! a baseline into a way to stop seeing the problem, which is the failure mode this whole codebase
//! is built to prevent.
//!
//! # Changed files narrow the report, not the corpus
//!
//! `--changed-since` does **not** restrict discovery, extraction, or analysis. Every discovered
//! file is read and compared exactly as in a full run; only findings are filtered afterwards, to
//! those with at least one participant in a changed file.
//!
//! This is not an implementation shortcut, it is the only correct shape for this tool's rules.
//! `duplicate-test` compares within one file, but `cross-file-duplicate` compares *across* files:
//! analyzing only the changed files would silently lose any duplicate whose twin lives in an
//! unchanged one, and would then report the changed file as clean. That is precisely the "silence
//! is not cleanliness" failure `AnalysisResult::incomplete` exists for, and it would be invisible
//! -- the run would look complete. Analyzing everything and filtering findings costs the same wall
//! clock as a full run and cannot produce that class of false clean. `--changed-since` is a
//! reporting-scope flag, never a performance flag.
//!
//! What filtering *can* still hide is a finding entirely within files you did not touch. That is
//! real narrowing, so [`retain_changed_findings`] reports how many findings it withheld and the
//! caller marks the run incomplete when that number is non-zero (see
//! `crate::analysis::analyze_with_budget`). A changed-files run that withheld nothing is
//! byte-for-byte a full run's report and is honestly reported as complete; one that withheld
//! something says so, loudly, rather than presenting a clean bill of health it cannot support.
//!
//! # Baseline content is untrusted
//!
//! The baseline file is repository content: it is committed, and on a pull request it is written
//! by whoever opened it. Only [`BaselineEntry::count`] and [`BaselineEntry::fingerprint`] are ever
//! read back into a decision. [`BaselineEntry::rule`] and [`BaselineEntry::tests`] are written for
//! the human reading the diff in code review and are **never** consulted, never printed, and never
//! compared -- so a crafted title in a baseline cannot influence what this tool suppresses, and
//! cannot reach a terminal through this path at all. They are sanitized and length-bounded on the
//! way *in* to the file as well, so the file itself stays reviewable.

use crate::model::{Finding, FINGERPRINT_VERSION};
use crate::reporters::terminal_safe;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Version of the baseline file format. Bumped when the *file shape* changes; a mismatch is a
/// hard error naming `--update-baseline`, never a silent partial read.
pub const BASELINE_SCHEMA_VERSION: u32 = 1;

/// Largest baseline file that will be read. A baseline is one line per accepted finding plus
/// annotation; 8 MiB is far past any real repository's accepted debt and keeps a hostile or
/// corrupt file from being read into memory in full.
const MAX_BASELINE_BYTES: u64 = 8 * 1024 * 1024;

/// Largest number of entries accepted from a baseline file, matching
/// [`crate::analysis::MAX_FINDINGS`]'s order of magnitude: a baseline cannot meaningfully record
/// more distinct findings than one run can produce.
const MAX_BASELINE_ENTRIES: usize = 100_000;

/// Per-title cap for the human-readable annotation written into a baseline entry. Titles are
/// unbounded repository content; a baseline meant to be read in review must not contain a
/// paragraph-length line.
const MAX_BASELINE_TITLE_CHARS: usize = 200;

// -------------------------------------------------------------------------------------------
// The baseline file
// -------------------------------------------------------------------------------------------

/// The on-disk baseline: every finding this repository has accepted, with its multiplicity.
///
/// Serialized as JSON with two-space indentation and a trailing newline, entries sorted by
/// fingerprint. Both properties exist for code review rather than for the parser: a stable order
/// means accepting one new finding is a single added hunk in the diff instead of a reshuffle, and
/// one entry per object means the reviewer sees *which* finding was accepted, not only that a hash
/// count changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineFile {
    /// Format version of this file. See [`BASELINE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The fingerprint derivation these entries were computed with, e.g.
    /// `vitestDedupFingerprint/v1`.
    ///
    /// Recorded separately from the fingerprints themselves (which already embed it) so a stale
    /// baseline is detected in one comparison, with an actionable message, instead of silently
    /// matching nothing and reporting every accepted finding as new -- which would look exactly
    /// like a sudden regression and would be acted on as one.
    pub fingerprint_version: String,
    /// Accepted findings, sorted by fingerprint.
    pub findings: Vec<BaselineEntry>,
}

/// One accepted finding identity and how many findings may carry it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineEntry {
    /// [`crate::model::Finding::fingerprint`]'s output, verbatim.
    pub fingerprint: String,
    /// How many findings carrying this fingerprint are accepted.
    ///
    /// Not implied to be `1`. `Finding::fingerprint`'s own doc comment documents that several
    /// genuinely different findings can share one fingerprint -- a run of more than two tests with
    /// one copy-pasted title is reported as a spanning set of pairwise findings, and two of those
    /// pairs can carry the identical `(path, title)` participant set. A baseline keyed on
    /// fingerprint *uniqueness* would accept one of those and report the rest as new, forever.
    /// Counting instead is the same choice CukeDedup's baseline loader makes.
    pub count: usize,
    /// The rule that produced it, for the human reading this file. Never read back.
    #[serde(default)]
    pub rule: String,
    /// The participating tests' full titles, for the human reading this file. Never read back.
    #[serde(default)]
    pub tests: Vec<String>,
}

/// A loaded baseline, reduced to the only thing matching consumes: fingerprint -> allowance.
#[derive(Debug, Clone, Default)]
pub struct Baseline {
    allowance: BTreeMap<String, usize>,
    total: usize,
}

impl Baseline {
    /// Total accepted findings across every fingerprint (the sum of the counts, not the number of
    /// distinct fingerprints).
    pub fn total(&self) -> usize {
        self.total
    }

    /// Whether this baseline accepts nothing at all.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// What a `--update-baseline` run changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BaselineUpdate {
    /// Findings recorded in the rewritten file.
    pub recorded: usize,
    /// Accepted instances the rewrite added relative to the previous file.
    pub added: usize,
    /// Accepted instances the rewrite removed relative to the previous file.
    pub removed: usize,
}

/// Reads and validates a baseline file.
///
/// # Errors
///
/// Every failure here is operational (exit 2), never a quality-gate result: an unreadable,
/// oversized, malformed, stale, or self-contradicting baseline means the gate's input is unknown,
/// and a tool that fell back to "suppress nothing" would turn a corrupt file into a wall of
/// apparently-new findings, while one that fell back to "suppress everything" would turn it into a
/// silent pass. Neither is an answer, so neither is offered.
pub fn load_baseline(path: &Path) -> Result<Baseline> {
    let text = crate::discovery::read_bounded(path, MAX_BASELINE_BYTES)
        .with_context(|| format!("reading baseline {}", path.display()))?
        .with_context(|| {
            format!(
                "baseline {} exceeds the {MAX_BASELINE_BYTES} byte limit",
                path.display()
            )
        })?;
    let file: BaselineFile = serde_json::from_str(&text)
        .with_context(|| format!("failed to parse baseline {}", path.display()))?;
    if file.schema_version != BASELINE_SCHEMA_VERSION {
        bail!(
            "baseline {} has schema version {} (this build understands {}); regenerate it with \
             --update-baseline",
            path.display(),
            file.schema_version,
            BASELINE_SCHEMA_VERSION
        );
    }
    if file.fingerprint_version != FINGERPRINT_VERSION {
        bail!(
            "baseline {} was computed with fingerprint version `{}` but this build emits `{}`; \
             its entries can no longer be matched. Regenerate it with --update-baseline and \
             re-review what it accepts.",
            path.display(),
            terminal_safe(&file.fingerprint_version),
            FINGERPRINT_VERSION
        );
    }
    if file.findings.len() > MAX_BASELINE_ENTRIES {
        bail!(
            "baseline {} has {} entries, past the {MAX_BASELINE_ENTRIES} entry limit",
            path.display(),
            file.findings.len()
        );
    }

    let mut allowance = BTreeMap::new();
    let mut total = 0usize;
    for entry in file.findings {
        if entry.count == 0 {
            bail!(
                "baseline {} accepts 0 occurrences of a fingerprint; remove the entry instead",
                path.display()
            );
        }
        if !entry.fingerprint.starts_with(FINGERPRINT_VERSION) {
            bail!(
                "baseline {} contains an entry whose fingerprint is not a `{FINGERPRINT_VERSION}` \
                 value; regenerate it with --update-baseline",
                path.display()
            );
        }
        // A repeated fingerprint is a merge artifact, not a legitimate shape: the writer emits one
        // entry per fingerprint with a count. Summing them silently would let a bad three-way
        // merge double an allowance and suppress a genuinely new finding.
        if allowance.insert(entry.fingerprint, entry.count).is_some() {
            bail!(
                "baseline {} lists the same fingerprint twice; entries must be unique and carry \
                 their multiplicity in `count` (likely a bad merge)",
                path.display()
            );
        }
        total = total.saturating_add(entry.count);
    }
    Ok(Baseline { allowance, total })
}

/// Rewrites `path` from `findings` and reports what changed relative to the file that was there.
///
/// Refusing to write from an incomplete run is the caller's job, not this function's -- see
/// `main.rs`. This function only records what it was given.
pub fn write_baseline(path: &Path, findings: &[Finding]) -> Result<BaselineUpdate> {
    let previous = if path.is_file() {
        // A previous file that cannot be read at all is still worth reporting against, but an
        // unreadable one must not stop the rewrite the user explicitly asked for: treat it as
        // empty for the added/removed arithmetic rather than failing to record the current run.
        load_baseline(path).map(|baseline| baseline.allowance).ok()
    } else {
        Some(BTreeMap::new())
    }
    .unwrap_or_default();

    let file = build_baseline(findings);
    let current: BTreeMap<String, usize> = file
        .findings
        .iter()
        .map(|entry| (entry.fingerprint.clone(), entry.count))
        .collect();

    let update = BaselineUpdate {
        recorded: current.values().sum(),
        added: surplus(&current, &previous),
        removed: surplus(&previous, &current),
    };

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating baseline directory {}", parent.display()))?;
    }
    let mut text =
        serde_json::to_string_pretty(&file).context("failed to serialize the baseline")?;
    text.push('\n');
    std::fs::write(path, text).with_context(|| format!("writing baseline {}", path.display()))?;
    Ok(update)
}

/// Builds the file body for `findings`: one entry per fingerprint, counted, sorted.
fn build_baseline(findings: &[Finding]) -> BaselineFile {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut annotations: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for finding in findings {
        let fingerprint = finding.fingerprint();
        *counts.entry(fingerprint.clone()).or_default() += 1;
        annotations.entry(fingerprint).or_insert_with(|| {
            (
                finding.rule.id().to_owned(),
                finding.titles.iter().map(|title| annotate(title)).collect(),
            )
        });
    }
    BaselineFile {
        schema_version: BASELINE_SCHEMA_VERSION,
        fingerprint_version: FINGERPRINT_VERSION.to_owned(),
        // `BTreeMap` iteration is already fingerprint-sorted, which is the diffable order this
        // file's whole shape exists for.
        findings: counts
            .into_iter()
            .map(|(fingerprint, count)| {
                let (rule, tests) = annotations.remove(&fingerprint).unwrap_or_default();
                BaselineEntry {
                    fingerprint,
                    count,
                    rule,
                    tests,
                }
            })
            .collect(),
    }
}

/// Sanitizes and bounds a title for the human-readable annotation fields.
///
/// `terminal_safe` (not the JSON-flavored sanitizer) on purpose: this value lands in a file a
/// person opens in a diff viewer, and a raw newline inside a title would break one accepted
/// finding across several lines of the file and make the diff unreadable.
fn annotate(title: &str) -> String {
    let sanitized = terminal_safe(title);
    if sanitized.chars().count() <= MAX_BASELINE_TITLE_CHARS {
        return sanitized;
    }
    let mut bounded: String = sanitized
        .chars()
        .take(MAX_BASELINE_TITLE_CHARS.saturating_sub(1))
        .collect();
    bounded.push('…');
    bounded
}

/// Total instances present in `left` beyond what `right` accounts for.
fn surplus(left: &BTreeMap<String, usize>, right: &BTreeMap<String, usize>) -> usize {
    left.iter()
        .map(|(fingerprint, count)| {
            count.saturating_sub(right.get(fingerprint).copied().unwrap_or_default())
        })
        .sum()
}

/// Marks every finding the baseline accounts for, and returns `(baselined, new)`.
///
/// Allowance is consumed in finding order, which is deterministic because findings are sorted
/// before this runs. The consequence that matters: a fingerprint recorded with `count: 2` marks
/// the first two findings carrying it and leaves a third as new, so *adding* another copy of an
/// already-accepted duplicate is still reported and still fails `--fail-on-new`. A set-membership
/// baseline would have accepted it silently forever.
pub fn apply_baseline(findings: &mut [Finding], baseline: &Baseline) -> (usize, usize) {
    let mut allowance = baseline.allowance.clone();
    let mut baselined = 0usize;
    let mut new = 0usize;
    for finding in findings.iter_mut() {
        let fingerprint = finding.fingerprint();
        match allowance.get_mut(&fingerprint) {
            Some(remaining) if *remaining > 0 => {
                *remaining -= 1;
                finding.baselined = true;
                baselined += 1;
            }
            _ => {
                finding.baselined = false;
                new += 1;
            }
        }
    }
    (baselined, new)
}

/// Builds a [`Baseline`] directly from findings, with no file in between -- the
/// `--baseline-from-ref` path, whose "baseline file" is the analyzed base revision itself.
///
/// Same counting rule as [`build_baseline`]/`load_baseline`: one allowance entry per distinct
/// fingerprint with its multiplicity. Not a serialization round-trip: the base revision's
/// findings never touch disk (the containment design below), so there is nothing to write or
/// re-read.
pub fn baseline_from_findings(findings: &[Finding]) -> Baseline {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for finding in findings {
        *counts.entry(finding.fingerprint()).or_default() += 1;
    }
    let total = counts.values().sum();
    Baseline {
        allowance: counts,
        total,
    }
}

/// Records every finding of a `--update-baseline` run as accepted, and clears the gate with it.
///
/// Separate from [`apply_baseline`] because the file being matched against was written *from* this
/// run: re-reading it to discover that it matches everything would be theatre. Zeroing
/// [`AnalysisResult::had_error_finding`] and
/// [`AnalysisResult::error_finding_locations`](crate::model::AnalysisResult::error_finding_locations)
/// here is sound only because the caller refuses to write a baseline from an incomplete run, and
/// truncation is the only way a finding could exist that this result does not carry.
pub fn mark_all_baselined(result: &mut crate::model::AnalysisResult) {
    for finding in &mut result.findings {
        finding.baselined = true;
    }
    result.modes.baseline_applied = true;
    result.modes.baselined = result.findings.len();
    result.modes.new_findings = 0;
    result.had_error_finding = false;
    result.error_finding_locations = 0;
}

/// Keeps findings with at least one participant in `changed`; returns how many were withheld.
///
/// A finding is kept when *any* participant is in a changed file, not only the first: a
/// `cross-file-duplicate` between a file you touched and one you did not is exactly the finding a
/// changed-files run must surface, and keying on the primary location alone would drop half of
/// them depending on which side sorted first.
pub fn retain_changed_findings(findings: &mut Vec<Finding>, changed: &BTreeSet<PathBuf>) -> usize {
    let before = findings.len();
    findings.retain(|finding| {
        finding
            .locations
            .iter()
            .any(|location| changed.contains(&location.path))
            || (0..finding.locations.len()).any(|index| {
                finding
                    .registration_files_for(index)
                    .any(|file| changed.contains(file))
            })
    });
    before - findings.len()
}

// -------------------------------------------------------------------------------------------
// Baseline from a revision (--baseline-from-ref)
// -------------------------------------------------------------------------------------------

/// The per-file byte cap for a base revision's file content -- the same bound the working-tree
/// read applies ([`crate::discovery::MAX_SOURCE_FILE_BYTES`]). A dist file committed to the base
/// revision costs a skip diagnostic, not an unbounded allocation.
const MAX_REVISION_SOURCE_BYTES: u64 = crate::discovery::MAX_SOURCE_FILE_BYTES;

/// One analyzed-repository file's content at a revision: the path relative to the analysis root
/// (the form every later stage keys on) and the exact blob text.
pub struct RevisionSource {
    /// Tree-relative path, `/`-separated, as `git ls-tree` reported it.
    pub relative: PathBuf,
    /// The blob's text. `git cat-file` never applies smudge filters, so this is the committed
    /// bytes decoded as UTF-8; a non-UTF-8 blob is skipped exactly like an unreadable
    /// working-tree file.
    pub content: String,
}

/// Reads every admitted file's content out of `revision`'s tree, without ever checking anything
/// out.
///
/// This is the containment design the `--baseline-from-ref` row required before the flag could
/// exist, and every piece is load-bearing:
///
/// - **No checkout, no worktree, ever.** Hostile checkout-time code is the whole threat model:
///   `.gitattributes` smudge/clean filters, `core.fsmonitor`, and process-level hooks all
///   execute *working-tree* plumbing, and an analyzed repository controls all of them. File
///   content here arrives through `git cat-file --batch`, fed the object ids `git ls-tree`
///   listed -- plumbing that reads objects from the database and runs nothing from the tree. The analyzed repository never
///   gets code execution from being analyzed.
/// - **The revision never reaches git as an option.** It is resolved through
///   [`resolve_commit`], which passes `--end-of-options` and verifies the `<commit>` suffix --
///   the same treatment `--changed-since` gets, so a revision crafted to look like a flag is an
///   error, not an injection.
/// - **Admission is narrow and fail-closed.** A base file is analyzed only if its extension
///   classifies as source (the same classifier the working-tree walk uses) and it matches the
///   caller's include globs without matching the exclude globs. Two documented narrowings fall
///   out of not re-running project/config discovery against a tree: per-project `include`
///   overrides and in-source candidates are not applied to the base run, and `.gitignore`
///   files are read from the working tree, not from the revision. Each narrowing can only
///   SHRINK the base run's finding set, which for a gate means reporting more findings as new
///   than strictly necessary -- the visible, fail-closed direction -- never silently accepting
///   one.
/// - **Inherited `GIT_*` environment is stripped** ([`git_at`]), so a hook or CI matrix
///   exporting `GIT_DIR` cannot point the base read at some other repository's objects.
pub fn revision_sources(
    root: &Path,
    revision: &str,
    include_globs: &[String],
    exclude_globs: &[String],
) -> Result<Vec<RevisionSource>> {
    if revision.trim().is_empty() || revision.starts_with('-') {
        bail!(
            "invalid --baseline-from-ref revision `{}`",
            terminal_safe(revision)
        );
    }
    ensure_git_repository(root, "--baseline-from-ref")?;
    ensure_not_shallow(root, "--baseline-from-ref")?;
    let commit = resolve_commit(root, revision, "--baseline-from-ref")?;

    let include = compile_revision_globs(include_globs, "include")?;
    let exclude = compile_revision_globs(exclude_globs, "exclude")?;

    // `-z` because an analyzed repository controls its own filenames: a newline in a filename
    // would otherwise split into two entries, and `--` so a path can never read as an option.
    // `-l` adds each entry's object id and size, so an oversized blob is skipped unread and the
    // admitted ones are read by id through one `cat-file --batch` (see [`BlobReader`]).
    let listing = git_at(root)
        .args(["ls-tree", "-r", "-l", "-z", "--"])
        .arg(&commit)
        .output()
        .with_context(|| "git failed while listing the base revision's files")?;
    if !listing.status.success() {
        bail!(
            "git failed while listing the base revision's files: {}",
            String::from_utf8_lossy(&listing.stderr).trim()
        );
    }

    let mut admitted = Vec::new();
    for entry in listing.stdout.split(|byte| *byte == b'\0') {
        if entry.is_empty() {
            continue;
        }
        // `<mode> <type> <object> <size>\t<path>`; the path follows the first tab.
        let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
            bail!("git returned an unreadable listing entry for the base revision");
        };
        let path_text = std::str::from_utf8(&entry[tab + 1..])
            .with_context(|| "git returned a non-UTF-8 file name for the base revision")?;
        let relative = PathBuf::from(path_text);
        // A tree path cannot escape its tree, but admission is asserted here rather than
        // assumed: the classifier and the globs decide, and anything they decline is skipped.
        if crate::discovery::classify_source_file(&relative).is_none() {
            continue;
        }
        let forward = path_text.replace('\\', "/");
        if exclude.is_match(Path::new(&forward)) || !include.is_match(Path::new(&forward)) {
            continue;
        }
        let fields: Vec<&[u8]> = entry[..tab]
            .split(u8::is_ascii_whitespace)
            .filter(|field| !field.is_empty())
            .collect();
        let (Some(kind), Some(object), Some(size)) = (fields.get(1), fields.get(2), fields.get(3))
        else {
            bail!(
                "git returned an unreadable listing entry for base file `{}`",
                terminal_safe(path_text)
            );
        };
        let too_large = std::str::from_utf8(size)
            .ok()
            .and_then(|size| size.parse::<u64>().ok())
            .is_none_or(|size| size > MAX_REVISION_SOURCE_BYTES);
        admitted.push(AdmittedEntry {
            path_text: path_text.to_owned(),
            relative,
            kind: String::from_utf8_lossy(kind).into_owned(),
            object: String::from_utf8_lossy(object).into_owned(),
            too_large,
        });
    }

    let mut reader = None;
    let mut sources = Vec::new();
    for AdmittedEntry {
        path_text,
        relative,
        kind,
        object,
        too_large,
    } in admitted
    {
        // Skips and refusals are reported in listing order, exactly where the per-file read used
        // to report them, so stderr is unchanged.
        //
        // An admitted entry that is not a blob (a submodule gitlink named like a test file) is an
        // operational error, as an unreadable file always was: skipping it would leave a silent
        // gap that makes the gate more lenient than it claims.
        if kind != "blob" {
            bail!(
                "git failed while reading base file `{}`: not a blob ({})",
                terminal_safe(&path_text),
                terminal_safe(&kind)
            );
        }
        if too_large {
            eprintln!(
                "vitest-dedup: skipping {} in the base revision: exceeds the byte limit",
                terminal_safe(&path_text)
            );
            continue;
        }
        if reader.is_none() {
            reader = Some(BlobReader::spawn(root)?);
        }
        let blob = match reader.as_mut().expect("spawned above").read(&object) {
            Ok(blob) => blob,
            Err(error) => {
                // Reap the process and keep what git said before reporting the failure.
                let said = reader.take().map(BlobReader::abort).unwrap_or_default();
                let said = said.trim();
                return Err(error).with_context(|| {
                    let git = if said.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", terminal_safe(said))
                    };
                    format!(
                        "git failed while reading base file `{}`{git}",
                        terminal_safe(&path_text)
                    )
                });
            }
        };
        let content = match String::from_utf8(blob) {
            Ok(content) => content,
            Err(_) => {
                eprintln!(
                    "vitest-dedup: skipping {} in the base revision: not valid UTF-8",
                    terminal_safe(&path_text)
                );
                continue;
            }
        };
        sources.push(RevisionSource { relative, content });
    }
    if let Some(reader) = reader {
        reader.finish()?;
    }
    Ok(sources)
}

/// One `git cat-file --batch` process answering blob reads by object id, one at a time.
///
/// Request/response, never pipelined: cat-file flushes after each object, so writing one id and
/// reading its whole answer before the next cannot fill both pipes and deadlock. Only hex object
/// ids from `git ls-tree` reach its stdin -- never a path -- and it reads objects from the
/// database, so it runs nothing from the analyzed tree.
struct BlobReader {
    child: std::process::Child,
    /// `None` once closed; closing it is how cat-file is told to exit.
    input: Option<std::process::ChildStdin>,
    output: std::io::BufReader<std::process::ChildStdout>,
    /// `None` once joined.
    stderr: Option<std::thread::JoinHandle<String>>,
}

/// One listing entry that passed admission, kept until the read loop reaches it so every skip
/// and refusal is reported in listing order.
struct AdmittedEntry {
    path_text: String,
    relative: PathBuf,
    /// The object type `ls-tree` reported; anything but `blob` is refused in the read loop.
    kind: String,
    object: String,
    too_large: bool,
}

/// How much of a child's stderr [`drain_stderr`] keeps for an error message.
const MAX_KEPT_STDERR_BYTES: u64 = 64 * 1024;

/// Reads `pipe` to its end on a separate thread and returns the first [`MAX_KEPT_STDERR_BYTES`]
/// as text. Draining is what keeps the child from blocking on a full stderr pipe -- mid-batch or
/// at exit -- while this side waits on stdout or `wait()`; the excess is discarded.
fn drain_stderr(pipe: impl std::io::Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        use std::io::Read;
        let mut pipe = pipe;
        let mut kept = Vec::new();
        let _ = pipe
            .by_ref()
            .take(MAX_KEPT_STDERR_BYTES)
            .read_to_end(&mut kept);
        let _ = std::io::copy(&mut pipe, &mut std::io::sink());
        String::from_utf8_lossy(&kept).into_owned()
    })
}

impl BlobReader {
    /// Starts the batch process at `root`.
    fn spawn(root: &Path) -> Result<Self> {
        let mut child = git_at(root)
            .args(["cat-file", "--batch"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| "git failed to start reading the base revision's files")?;
        let input = child.stdin.take().context("git cat-file has no stdin")?;
        let output = child.stdout.take().context("git cat-file has no stdout")?;
        let stderr = drain_stderr(child.stderr.take().context("git cat-file has no stderr")?);
        Ok(Self {
            child,
            input: Some(input),
            output: std::io::BufReader::new(output),
            stderr: Some(stderr),
        })
    }

    /// Returns the bytes of blob `object`. A missing object, a non-blob, or the process ending
    /// mid-answer is an error, never an empty file.
    fn read(&mut self, object: &str) -> Result<Vec<u8>> {
        use std::io::{BufRead, Read, Write};
        let input = self
            .input
            .as_mut()
            .context("git cat-file input is closed")?;
        writeln!(input, "{object}")?;
        input.flush()?;
        let mut header = String::new();
        if self.output.read_line(&mut header)? == 0 {
            bail!("git cat-file ended before answering");
        }
        let fields: Vec<&str> = header.split_whitespace().collect();
        let size = match fields.as_slice() {
            [_, "blob", size] => size.parse::<usize>()?,
            _ => bail!("unexpected git cat-file answer `{}`", header.trim()),
        };
        // The contents are followed by one newline.
        let mut blob = vec![0; size + 1];
        self.output.read_exact(&mut blob)?;
        if blob.pop() != Some(b'\n') {
            bail!("git cat-file answer was not newline-terminated");
        }
        Ok(blob)
    }

    /// Closes the request stream and requires a clean exit.
    fn finish(mut self) -> Result<()> {
        self.input = None;
        let status = self.child.wait()?;
        let stderr = self.join_stderr();
        if !status.success() {
            bail!(
                "git failed while reading the base revision's files: {}",
                terminal_safe(stderr.trim())
            );
        }
        Ok(())
    }

    /// Stops the process after a failed read: closes its input, kills and reaps it, and returns
    /// what it wrote to stderr so the caller can report it with the failure.
    fn abort(mut self) -> String {
        self.stop();
        self.join_stderr()
    }

    /// Closes the input, then kills and reaps the process; a no-op once it has been waited on.
    fn stop(&mut self) {
        self.input = None;
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }

    /// Joins the stderr drainer once; later calls return an empty string.
    fn join_stderr(&mut self) -> String {
        self.stderr
            .take()
            .map(|stderr| stderr.join().unwrap_or_default())
            .unwrap_or_default()
    }
}

impl Drop for BlobReader {
    /// Reaps a reader abandoned on an early return, so no `git` process outlives the read.
    fn drop(&mut self) {
        self.stop();
        self.join_stderr();
    }
}

/// Compiles the admission globs for a base-revision read; `globset`'s errors are wrapped with
/// the pattern list they came from, matching [`crate::discovery::compile_glob_set`]'s shape.
fn compile_revision_globs(patterns: &[String], kind: &str) -> Result<globset::GlobSet> {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        let glob = globset::Glob::new(pattern).with_context(|| {
            format!("invalid {kind} pattern `{pattern}` in the base revision read")
        })?;
        builder.add(glob);
    }
    builder
        .build()
        .with_context(|| format!("compiling {kind} globs for the base revision read"))
}

// -------------------------------------------------------------------------------------------
// Inline suppressions
// -------------------------------------------------------------------------------------------

/// The exact text an inline suppression directive starts with (after optional whitespace).
///
/// Everything after the token on the same line is the directive's free-form reason -- "this pair
/// is intentional" belongs next to the code, and the reason is for the human who reads the
/// source later, not for this tool, which never prints it (the finding is gone; the explanation
/// stays where the code is).
pub const INLINE_SUPPRESSION_DIRECTIVE: &str = "// vitest-dedup-disable-next-line";

/// One inline suppression found in one analyzed source file: the directive on `line` suppresses
/// findings whose participant declaration starts on `line + 1` of `path`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineSuppression {
    /// The file the directive sits in, relative to the analysis root -- the same form
    /// [`crate::model::Location::path`] uses, so matching is a direct comparison.
    pub path: PathBuf,
    /// 1-indexed line the directive itself occupies.
    pub line: u32,
    /// The rest of the directive line, trimmed -- the author's stated reason. Unused by the
    /// tool; retained so a future report that wants to surface the reasons has them.
    #[allow(dead_code)]
    pub reason: String,
}

/// Scans one file's source for inline suppression directives.
///
/// Line-oriented on purpose: the directive is a comment convention, and this scan asks the same
/// question a reader does -- does a line announce itself as a suppression? The prefix is
/// specific enough that a false positive requires a string literal line starting with `// `
/// plus the exact token, which is the author writing the text; the failure direction is
/// over-suppression of one finding, not a false merge. `path` must already be relativized (the
/// caller in `main` uses the same `relativize_test_path` the findings' locations go through).
pub fn scan_inline_suppressions(source: &str, path: PathBuf) -> Vec<InlineSuppression> {
    let mut suppressions = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(INLINE_SUPPRESSION_DIRECTIVE) {
            suppressions.push(InlineSuppression {
                path: path.clone(),
                line: (index + 1) as u32,
                reason: rest.trim().to_string(),
            });
        }
    }
    suppressions
}

/// Removes every finding with at least one participant suppressed by a directive, returning how
/// many were removed.
///
/// A finding is suppressed when *any* participant site carries the directive -- the author
/// wrote it at one test's declaration, and the finding is about that test's relationship to
/// another; "this pair is intentional" suppresses the pair.
///
/// **The precedence this ordering establishes** (the caller runs this before
/// [`apply_baseline`]): a suppressed finding never consumes baseline allowance, never appears
/// in a report, never reaches the quality gate, and is never recorded by `--update-baseline`.
/// A baseline is a record of accepted debt; an inline suppression is a claim that the finding
/// is not debt at all -- and the stronger claim wins by running first. A finding suppressed
/// here would otherwise sit in every baseline file forever as "accepted" on every re-record,
/// which is not what either mechanism means.
pub fn apply_inline_suppressions(
    findings: &mut Vec<Finding>,
    suppressions: &[InlineSuppression],
) -> usize {
    if suppressions.is_empty() {
        return 0;
    }
    let before = findings.len();
    findings.retain(|finding| {
        !finding.locations.iter().any(|location| {
            suppressions.iter().any(|suppression| {
                suppression.path == location.path
                    && suppression.line + 1 == location.span.start.line
            })
        })
    });
    before - findings.len()
}

// -------------------------------------------------------------------------------------------
// Changed files
// -------------------------------------------------------------------------------------------

/// Git environment variables a parent `git` exports to the processes it spawns.
///
/// They outrank a child invocation's `-C`, so leaving them in place would point changed-file mode
/// at whichever repository invoked the hook rather than at the analyzed root. Borrowed verbatim
/// from CukeDedup's `modes.rs`, which found this the hard way.
const INHERITED_GIT_VARIABLES: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_PREFIX",
    "GIT_CEILING_DIRECTORIES",
];

#[cfg(test)]
thread_local! {
    /// Test-only count of `git` commands built on this thread by [`git_at`].
    static GIT_SPAWNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A `git` invocation bound to `root` and nothing else.
fn git_at(root: &Path) -> Command {
    #[cfg(test)]
    GIT_SPAWNS.with(|spawns| spawns.set(spawns.get() + 1));
    let mut command = Command::new("git");
    for variable in INHERITED_GIT_VARIABLES {
        command.env_remove(variable);
    }
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.arg("-C").arg(root);
    command
}

/// Files under `root` changed since `base`, relative to `root`, plus untracked files.
///
/// Untracked files are included because a pull request's newly added test file is the single most
/// likely thing a changed-files run is being asked about, and it is not in any diff until it is
/// committed.
///
/// # Errors
///
/// All three of the operational failures a changed-files run has are errors here, never an empty
/// result:
///
/// - `root` is not inside a Git repository at all,
/// - the repository is a shallow clone, where `base` may exist as a name but its history does not,
///   so a diff against it is silently wrong rather than absent,
/// - `base` does not resolve to a commit.
///
/// Every one of them would otherwise produce an empty changed set, which filters away every
/// finding and reports a clean run over a repository nobody looked at. That is the exact false
/// clean this module exists to prevent, so it is an operational error (exit 2) in all three cases.
pub fn changed_files_since(root: &Path, base: &str) -> Result<BTreeSet<PathBuf>> {
    if base.trim().is_empty() || base.starts_with('-') {
        bail!("invalid --changed-since revision `{}`", terminal_safe(base));
    }
    ensure_git_repository(root, "--changed-since")?;
    ensure_not_shallow(root, "--changed-since")?;
    let commit = resolve_commit(root, base, "--changed-since")?;

    let mut changed = git_paths(
        root,
        &[
            "diff",
            "--relative",
            "--name-only",
            "-z",
            "--diff-filter=ACMR",
            &commit,
            "--",
        ],
        "listing files changed since the --changed-since revision",
    )?;
    changed.extend(git_paths(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z", "--"],
        "listing untracked files for --changed-since",
    )?);
    Ok(changed)
}

/// Fails with a `{flag}`-named message unless `root` sits inside a Git repository -- the
/// precondition both revision modes share, reported under whichever flag brought the caller
/// here.
fn ensure_git_repository(root: &Path, flag: &str) -> Result<()> {
    let output = git_at(root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context(format!(
            "{flag} needs `git` on PATH, and it could not be run"
        ))?;
    if !output.status.success() {
        bail!(
            "{flag} requires a Git repository, and {} is not inside one: {}",
            root.display(),
            terminal_safe(String::from_utf8_lossy(&output.stderr).trim())
        );
    }
    Ok(())
}

/// Refuses a shallow clone under the caller's flag name: a truncated history makes revision
/// comparisons silently wrong rather than obviously missing.
fn ensure_not_shallow(root: &Path, flag: &str) -> Result<()> {
    let output = git_at(root)
        .args(["rev-parse", "--is-shallow-repository"])
        .output()
        .context("failed to determine whether the repository is a shallow clone")?;
    if !output.status.success() {
        bail!(
            "could not determine whether {} is a shallow clone: {}",
            root.display(),
            terminal_safe(String::from_utf8_lossy(&output.stderr).trim())
        );
    }
    if String::from_utf8_lossy(&output.stdout).trim() == "true" {
        bail!(
            "{flag} cannot be used in a shallow clone: the revision it compares against \
             may be absent or truncated, so the diff would be silently wrong rather than \
             obviously missing. Fetch full history first (in GitHub Actions, \
             `actions/checkout` with `fetch-depth: 0`)."
        );
    }
    Ok(())
}

/// Resolves a user-supplied revision to a full commit id, with the revision never reaching
/// git as an option (`--end-of-options`) and the flag name carried into every failure.
pub(crate) fn resolve_commit(root: &Path, base: &str, flag: &str) -> Result<String> {
    let output = git_at(root)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(format!("{base}^{{commit}}"))
        .output()
        .context(format!("failed to resolve the {flag} revision"))?;
    if !output.status.success() {
        bail!(
            "{flag} revision `{}` does not resolve to a commit in {}",
            terminal_safe(base),
            root.display()
        );
    }
    let oid = String::from_utf8(output.stdout)
        .context("git returned a non-UTF-8 commit id")?
        .trim()
        .to_owned();
    if oid.is_empty() || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!(
            "git returned an unusable commit id for {flag} revision `{}`",
            terminal_safe(base)
        );
    }
    Ok(oid)
}

/// Runs a `-z`-terminated path-listing git command and decodes its output.
fn git_paths(root: &Path, arguments: &[&str], what: &str) -> Result<BTreeSet<PathBuf>> {
    let output = git_at(root)
        // Without this, git escapes non-ASCII names into C-style quoted strings, and the decoded
        // path would never equal the real one -- silently dropping every non-ASCII file from the
        // changed set.
        .args(["-c", "core.quotePath=false"])
        .args(arguments)
        .output()
        .with_context(|| format!("failed to run git while {what}"))?;
    if !output.status.success() {
        bail!(
            "git failed while {what}: {}",
            terminal_safe(String::from_utf8_lossy(&output.stderr).trim())
        );
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(path_from_git_bytes)
        .collect()
}

#[cfg(unix)]
/// Decodes a `git ls-files -z` path: raw bytes on unix (git quotes nothing there), the
/// quoted-and-lossy fallback elsewhere, matching each platform's `ls-files` behavior.
fn path_from_git_bytes(bytes: &[u8]) -> Result<PathBuf> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

/// The non-unix twin of the unix decoder above: git quotes bytes it cannot encode, so the
/// quoted form is decoded (lossily, and only ever for a diagnostic path).
#[cfg(not(unix))]
fn path_from_git_bytes(bytes: &[u8]) -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::str::from_utf8(bytes).context("git returned a non-UTF-8 file name")?,
    ))
}

#[cfg(test)]
mod suppression_tests {
    use super::*;
    use crate::model::{Location, Position, Rule, Severity, Span};

    /// A one-participant location at `path:line`, column 3, for the suppression tests.
    fn location(path: &str, line: u32) -> Location {
        Location {
            path: PathBuf::from(path),
            span: Span {
                start: Position { line, column: 3 },
                end: Position { line, column: 8 },
                start_byte: 0,
                end_byte: 5,
            },
        }
    }

    /// An Error-severity `duplicate-test` finding with the single participant [`location`]
    /// builds.
    fn finding(path: &str, line: u32) -> Finding {
        Finding {
            registration_files: Vec::new(),
            rule: Rule::DuplicateTest,
            severity: Severity::Error,
            message: "m".to_string(),
            locations: vec![location(path, line)],
            titles: vec!["t".to_string()],
            inert: vec![false],
            baselined: false,
        }
    }

    /// The directive on line 4 suppresses a participant on line 5, and nothing else: a
    /// participant on line 6, in another file, or on the directive's own line survives.
    #[test]
    fn a_directive_suppresses_exactly_the_next_line_of_its_file() {
        let suppressions = scan_inline_suppressions(
            "a\nb\nc\n// vitest-dedup-disable-next-line this pair is intentional\nx\ny\nz",
            PathBuf::from("src/a.test.ts"),
        );
        assert_eq!(suppressions.len(), 1);
        assert_eq!(suppressions[0].path, PathBuf::from("src/a.test.ts"));
        assert_eq!(suppressions[0].line, 4);
        assert_eq!(suppressions[0].reason, "this pair is intentional");

        let mut findings = vec![
            finding("src/a.test.ts", 5),
            finding("src/a.test.ts", 6),
            finding("src/other.test.ts", 5),
        ];
        let removed = apply_inline_suppressions(&mut findings, &suppressions);
        assert_eq!(removed, 1);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].locations[0].span.start.line, 6);
        assert_eq!(
            findings[0].locations[0].path,
            PathBuf::from("src/a.test.ts")
        );
    }

    /// A finding is suppressed when ANY participant carries the directive: the author wrote it
    /// at one test's declaration, and the finding is about that test's relationship.
    #[test]
    fn any_suppressed_participant_suppresses_the_whole_finding() {
        let suppressions =
            scan_inline_suppressions("// vitest-dedup-disable-next-line", PathBuf::from("b.ts"));
        let mut finding = finding("a.ts", 2);
        finding.locations.push(location("b.ts", 2));
        let mut findings = vec![finding];
        assert_eq!(apply_inline_suppressions(&mut findings, &suppressions), 1);
        assert!(findings.is_empty());
    }

    /// Empty input, no directives, no findings: each is a no-op that reports zero, never an
    /// error -- the common case (no directives anywhere) must cost nothing.
    #[test]
    fn empty_inputs_are_no_ops_reporting_zero() {
        assert!(scan_inline_suppressions("", PathBuf::from("a.ts")).is_empty());
        assert!(scan_inline_suppressions(
            "const x = 1;\ntest('no directives here', () => {});",
            PathBuf::from("a.ts")
        )
        .is_empty());
        let mut findings = vec![finding("a.ts", 2)];
        assert_eq!(apply_inline_suppressions(&mut findings, &[]), 0);
        assert_eq!(findings.len(), 1);
    }

    /// The precedence claim, pinned through the real baseline file format: a finding suppressed
    /// by a directive never lands in a `--update-baseline` file, because the suppression runs
    /// before the result is finalized -- a repository that suppresses one pair and baselines
    /// another records only the other. Suppression is "not debt"; a baseline file that contained
    /// it would misstate the repository.
    #[test]
    fn a_suppressed_finding_is_never_recorded_by_the_baseline() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("baseline.json");

        let suppressions = scan_inline_suppressions(
            "// vitest-dedup-disable-next-line\nconst x = 1;",
            PathBuf::from("a.ts"),
        );
        let mut findings = vec![finding("a.ts", 2), finding("a.ts", 9)];
        apply_inline_suppressions(&mut findings, &suppressions);
        assert_eq!(findings.len(), 1, "the directive-covered participant went");

        write_baseline(&path, &findings).unwrap();
        let baseline = load_baseline(&path).unwrap();
        assert_eq!(
            baseline.total(),
            1,
            "only the unsuppressed finding is recorded"
        );

        // And on the next run the same suppressed finding is suppressed again before the
        // baseline is applied, so the recorded allowance still matches exactly what is new.
        let mut next_run = vec![finding("a.ts", 2), finding("a.ts", 9)];
        apply_inline_suppressions(&mut next_run, &suppressions);
        let (baselined, new_findings) = apply_baseline(&mut next_run, &baseline);
        assert_eq!((baselined, new_findings), (1, 0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Location, Position, Rule, Severity, Span};

    /// Build a mode-test location with a deterministic line-based byte span.
    fn location(path: &str, line: u32) -> Location {
        Location {
            path: PathBuf::from(path),
            span: Span {
                start: Position { line, column: 3 },
                end: Position { line, column: 8 },
                start_byte: line * 100,
                end_byte: line * 100 + 5,
            },
        }
    }

    /// Build a mode-filter finding with participant provenance, default severity and no initial
    /// baseline flag.
    fn finding(rule: Rule, participants: &[(&str, u32, &str)]) -> Finding {
        Finding {
            registration_files: Vec::new(),
            rule,
            severity: rule.default_severity(),
            message: "identical bodies under an equivalent context".to_owned(),
            locations: participants
                .iter()
                .map(|(path, line, _)| location(path, *line))
                .collect(),
            titles: participants
                .iter()
                .map(|(_, _, title)| (*title).to_owned())
                .collect(),
            inert: participants.iter().map(|_| false).collect(),
            baselined: false,
        }
    }

    /// Build a duplicate-test finding with two participants ten lines apart.
    fn pair(path: &str, title: &str, line: u32) -> Finding {
        finding(
            Rule::DuplicateTest,
            &[(path, line, title), (path, line + 10, title)],
        )
    }

    // -- round trip ------------------------------------------------------------------------

    /// Regression fixture for baseline/change filtering: a written baseline reads back and
    /// matches the findings it recorded.
    #[test]
    fn a_written_baseline_reads_back_and_matches_the_findings_it_recorded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/vitest-dedup-baseline.json");
        let mut findings = vec![
            pair("src/a.test.ts", "suite > a", 10),
            pair("src/b.test.ts", "suite > b", 10),
        ];

        let update = write_baseline(&path, &findings).unwrap();
        assert_eq!(update.recorded, 2);
        assert_eq!(update.added, 2);
        assert_eq!(update.removed, 0);

        let baseline = load_baseline(&path).unwrap();
        assert_eq!(baseline.total(), 2);
        let (baselined, new) = apply_baseline(&mut findings, &baseline);
        assert_eq!((baselined, new), (2, 0));
        assert!(findings.iter().all(|finding| finding.baselined));
    }

    /// Regression fixture for baseline/change filtering: the written file is sorted
    /// deterministic and newline terminated.
    #[test]
    fn the_written_file_is_sorted_deterministic_and_newline_terminated() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("baseline.json");
        let findings = vec![
            pair("src/z.test.ts", "suite > z", 10),
            pair("src/a.test.ts", "suite > a", 10),
        ];
        write_baseline(&path, &findings).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();

        // Same findings, opposite order in: the file must be byte-identical, or a baseline would
        // churn in review every time analysis reordered anything.
        let reversed: Vec<Finding> = findings.iter().rev().cloned().collect();
        write_baseline(&path, &reversed).unwrap();
        assert_eq!(first, std::fs::read_to_string(&path).unwrap());
        assert!(first.ends_with("}\n"));

        let parsed: BaselineFile = serde_json::from_str(&first).unwrap();
        let fingerprints: Vec<&String> = parsed
            .findings
            .iter()
            .map(|entry| &entry.fingerprint)
            .collect();
        let mut sorted = fingerprints.clone();
        sorted.sort();
        assert_eq!(fingerprints, sorted, "entries must be fingerprint-sorted");
        assert_eq!(parsed.fingerprint_version, FINGERPRINT_VERSION);
        assert_eq!(parsed.schema_version, BASELINE_SCHEMA_VERSION);
    }

    /// Regression fixture for baseline/change filtering: a baseline survives a finding moving
    /// within its file.
    #[test]
    fn a_baseline_survives_a_finding_moving_within_its_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("baseline.json");
        write_baseline(&path, &[pair("src/a.test.ts", "suite > a", 10)]).unwrap();

        // Same finding, 200 lines further down: the fingerprint is location-independent, so the
        // baseline must still match it.
        let mut moved = vec![pair("src/a.test.ts", "suite > a", 210)];
        let baseline = load_baseline(&path).unwrap();
        assert_eq!(apply_baseline(&mut moved, &baseline), (1, 0));
    }

    // -- the collision case ----------------------------------------------------------------

    #[test]
    fn one_fingerprint_can_legitimately_cover_several_findings_and_is_counted_not_deduplicated() {
        // `Finding::fingerprint`'s documented collision: a run of more than two tests sharing one
        // copy-pasted title is emitted as a spanning set of pairwise findings, and two of those
        // pairs carry the identical (path, title) participant set. A baseline keyed on
        // fingerprint *uniqueness* accepts one and reports the other as new forever.
        let path = "src/copy-paste.test.ts";
        let title = "suite > renders correctly";
        let first = finding(Rule::DuplicateTest, &[(path, 10, title), (path, 20, title)]);
        let second = finding(Rule::DuplicateTest, &[(path, 30, title), (path, 40, title)]);
        assert_eq!(
            first.fingerprint(),
            second.fingerprint(),
            "sanity: this test is meaningless unless these genuinely collide"
        );
        assert_ne!(first.locations, second.locations);

        let directory = tempfile::tempdir().unwrap();
        let baseline_path = directory.path().join("baseline.json");
        let mut findings = vec![first, second];
        let update = write_baseline(&baseline_path, &findings).unwrap();
        assert_eq!(update.recorded, 2, "both occurrences must be recorded");

        let file: BaselineFile =
            serde_json::from_str(&std::fs::read_to_string(&baseline_path).unwrap()).unwrap();
        assert_eq!(file.findings.len(), 1, "one fingerprint");
        assert_eq!(file.findings[0].count, 2, "two occurrences of it");

        let baseline = load_baseline(&baseline_path).unwrap();
        assert_eq!(
            apply_baseline(&mut findings, &baseline),
            (2, 0),
            "both colliding findings must be accepted, not just the first"
        );
    }

    #[test]
    fn adding_a_third_occurrence_of_an_accepted_fingerprint_is_still_new() {
        // The other half of counting: an allowance of 2 must not silently accept a 3rd copy.
        let path = "src/copy-paste.test.ts";
        let title = "suite > renders correctly";
        let make =
            |a: u32, b: u32| finding(Rule::DuplicateTest, &[(path, a, title), (path, b, title)]);

        let directory = tempfile::tempdir().unwrap();
        let baseline_path = directory.path().join("baseline.json");
        write_baseline(&baseline_path, &[make(10, 20), make(30, 40)]).unwrap();

        let baseline = load_baseline(&baseline_path).unwrap();
        let mut grown = vec![make(10, 20), make(30, 40), make(50, 60)];
        assert_eq!(apply_baseline(&mut grown, &baseline), (2, 1));
        assert!(!grown[2].baselined, "the third copy is new debt");
    }

    // -- rejecting a baseline that cannot be trusted -----------------------------------------

    #[test]
    fn a_stale_or_malformed_baseline_is_an_error_never_a_silent_empty_baseline() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("baseline.json");

        let cases: [(&str, &str); 5] = [
            ("not json at all", "failed to parse baseline"),
            (
                r#"{"schemaVersion":99,"fingerprintVersion":"vitestDedupFingerprint/v1","findings":[]}"#,
                "schema version",
            ),
            (
                r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v0","findings":[]}"#,
                "fingerprint version",
            ),
            (
                r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v1","findings":[{"fingerprint":"vitestDedupFingerprint/v1:aaaa","count":0}]}"#,
                "0 occurrences",
            ),
            (
                r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v1","findings":[{"fingerprint":"vitestDedupFingerprint/v1:aaaa","count":1},{"fingerprint":"vitestDedupFingerprint/v1:aaaa","count":1}]}"#,
                "same fingerprint twice",
            ),
        ];
        for (content, expected) in cases {
            std::fs::write(&path, content).unwrap();
            let error = load_baseline(&path).unwrap_err();
            let text = format!("{error:#}");
            assert!(
                text.contains(expected),
                "expected {expected:?} in {text:?} for {content}"
            );
        }
    }

    /// The two bounds on an untrusted baseline file, each checked at the size that crosses it
    /// rather than comfortably past it. Both must be operational errors: a baseline whose input
    /// is unknown cannot be allowed to resolve to "suppress nothing" (a wall of apparently-new
    /// findings) or "suppress everything" (a silent pass).
    ///
    /// The entry-count file is kept under the byte limit on purpose, so it is the entry limit and
    /// not the byte limit that rejects it -- the two guards are otherwise indistinguishable from
    /// the outside, and removing the entry limit would leave this file parsing happily.
    #[test]
    fn an_oversized_or_overlong_baseline_is_rejected_by_the_bound_it_actually_crosses() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("baseline.json");

        let mut oversized = String::from("[");
        oversized.push_str(&" ".repeat(usize::try_from(MAX_BASELINE_BYTES).unwrap()));
        assert!(oversized.len() as u64 > MAX_BASELINE_BYTES);
        std::fs::write(&path, &oversized).unwrap();
        let error = format!("{:#}", load_baseline(&path).unwrap_err());
        assert!(
            error.contains(&format!("exceeds the {MAX_BASELINE_BYTES} byte limit")),
            "{error}"
        );

        let mut overlong = String::from(
            r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v1","findings":["#,
        );
        for index in 0..=MAX_BASELINE_ENTRIES {
            if index > 0 {
                overlong.push(',');
            }
            overlong.push_str(r#"{"fingerprint":"","count":1}"#);
        }
        overlong.push_str("]}");
        assert!(
            (overlong.len() as u64) < MAX_BASELINE_BYTES,
            "the entry-limit fixture must stay under the byte limit, or it proves the wrong guard"
        );
        std::fs::write(&path, &overlong).unwrap();
        let error = format!("{:#}", load_baseline(&path).unwrap_err());
        assert!(
            error.contains(&format!("past the {MAX_BASELINE_ENTRIES} entry limit")),
            "{error}"
        );
    }

    /// A file-level `fingerprintVersion` that matches says nothing about the entries under it: a
    /// three-way merge can leave a `v0` entry inside a `v1` file. Such an entry can never match a
    /// finding this build emits, so it would silently accept nothing while looking like accepted
    /// debt -- an allowance that quietly does nothing is worse than an error.
    #[test]
    fn a_baseline_entry_whose_fingerprint_is_not_a_versioned_value_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("baseline.json");
        std::fs::write(
            &path,
            r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v1","findings":[{"fingerprint":"cukeDedupFingerprint/v2:aaaa","count":1}]}"#,
        )
        .unwrap();
        let error = format!("{:#}", load_baseline(&path).unwrap_err());
        assert!(
            error.contains(&format!("not a `{FINGERPRINT_VERSION}` value")),
            "{error}"
        );
    }

    /// `is_empty` is the question "does this baseline accept anything at all", and a baseline is
    /// measured by the total instances it accepts, not by how many distinct fingerprints it
    /// lists. A `count`-blind implementation would call a file holding one fingerprint with
    /// `count: 3` a baseline of one.
    #[test]
    fn a_baseline_is_empty_only_when_it_accepts_no_instance_at_all() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("baseline.json");

        std::fs::write(
            &path,
            r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v1","findings":[]}"#,
        )
        .unwrap();
        let empty = load_baseline(&path).unwrap();
        assert!(empty.is_empty());
        assert_eq!(empty.total(), 0);
        assert!(Baseline::default().is_empty());

        write_baseline(&path, &[pair("src/a.test.ts", "suite > a", 10)]).unwrap();
        let recorded = load_baseline(&path).unwrap();
        assert!(!recorded.is_empty());
        assert_eq!(recorded.total(), 1);

        // One fingerprint, three accepted instances: the total counts instances, not keys.
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace(r#""count": 1"#, r#""count": 3"#);
        std::fs::write(&path, text).unwrap();
        let multiple = load_baseline(&path).unwrap();
        assert!(!multiple.is_empty());
        assert_eq!(multiple.total(), 3);
    }

    #[test]
    fn annotation_fields_are_sanitized_and_bounded_on_the_way_into_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("baseline.json");
        let hostile = format!("a\u{202e}b\nc{}", "x".repeat(500));
        write_baseline(&path, &[pair("src/a.test.ts", &hostile, 10)]).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains('\u{202e}'), "bidi override reached the file");
        let file: BaselineFile = serde_json::from_str(&raw).unwrap();
        for title in &file.findings[0].tests {
            assert!(!title.contains('\n'));
            assert!(title.chars().count() <= MAX_BASELINE_TITLE_CHARS);
        }
        assert_eq!(file.findings[0].rule, "duplicate-test");
    }

    #[test]
    fn rewriting_reports_what_it_added_and_removed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("baseline.json");
        write_baseline(
            &path,
            &[
                pair("src/a.test.ts", "suite > a", 10),
                pair("src/b.test.ts", "suite > b", 10),
            ],
        )
        .unwrap();

        // `b` was fixed, `c` appeared.
        let update = write_baseline(
            &path,
            &[
                pair("src/a.test.ts", "suite > a", 10),
                pair("src/c.test.ts", "suite > c", 10),
            ],
        )
        .unwrap();
        assert_eq!(update.recorded, 2);
        assert_eq!(update.added, 1);
        assert_eq!(update.removed, 1);
    }

    // -- changed-file filtering ---------------------------------------------------------------

    #[test]
    fn a_finding_is_kept_when_any_participant_is_in_a_changed_file() {
        let mut findings = vec![
            // Cross-file: only the *second* participant was touched. Keying on the primary
            // location alone would drop this, which is the half of cross-file-duplicate a
            // changed-files run most needs to see.
            finding(
                Rule::CrossFileDuplicate,
                &[
                    ("src/untouched.test.ts", 10, "a"),
                    ("src/touched.test.ts", 10, "b"),
                ],
            ),
            pair("src/elsewhere.test.ts", "suite > x", 10),
        ];
        let changed = BTreeSet::from([PathBuf::from("src/touched.test.ts")]);
        let withheld = retain_changed_findings(&mut findings, &changed);
        assert_eq!(withheld, 1);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, Rule::CrossFileDuplicate);
    }

    #[test]
    fn severity_and_inert_flags_do_not_affect_baseline_identity() {
        // A config-only change (lowering a rule's severity) must not re-earn every baseline entry.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("baseline.json");
        write_baseline(&path, &[pair("src/a.test.ts", "suite > a", 10)]).unwrap();

        let mut lowered = vec![pair("src/a.test.ts", "suite > a", 10)];
        lowered[0].severity = Severity::Warning;
        lowered[0].inert = vec![true, false];
        let baseline = load_baseline(&path).unwrap();
        assert_eq!(apply_baseline(&mut lowered, &baseline), (1, 0));
    }

    // -- git -----------------------------------------------------------------------------------

    /// Create a Git command with inherited repository environment removed for isolated fixture
    /// operations.
    fn fixture_git() -> Command {
        let mut command = Command::new("git");
        for variable in INHERITED_GIT_VARIABLES {
            command.env_remove(variable);
        }
        command
    }

    /// Initialize the fixture main branch, configure a test identity and commit its current
    /// files.
    fn init_repository(root: &Path) {
        for arguments in [
            ["init", "-q", "-b", "main"].as_slice(),
            ["config", "user.email", "test@example.com"].as_slice(),
            ["config", "user.name", "Test"].as_slice(),
            ["add", "."].as_slice(),
            ["commit", "-qm", "initial"].as_slice(),
        ] {
            assert!(fixture_git()
                .args(arguments)
                .current_dir(root)
                .status()
                .unwrap()
                .success());
        }
    }

    /// Changed files are reported relative to the analysis root, untracked files included.
    #[test]
    fn changed_files_are_relative_to_the_root_and_include_untracked_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("packages/app/src")).unwrap();
        std::fs::write(root.join("packages/app/src/tracked.test.ts"), "before").unwrap();
        std::fs::write(root.join("packages/app/src/untouched.test.ts"), "same").unwrap();
        init_repository(root);
        std::fs::write(root.join("packages/app/src/tracked.test.ts"), "after").unwrap();
        std::fs::write(root.join("packages/app/src/fresh.test.ts"), "new").unwrap();

        let scoped = root.join("packages/app");
        let changed = changed_files_since(&scoped, "HEAD").unwrap();
        assert_eq!(
            changed,
            BTreeSet::from([
                PathBuf::from("src/fresh.test.ts"),
                PathBuf::from("src/tracked.test.ts"),
            ]),
            "paths must be relative to the analysis root, matching a Finding's location path"
        );
    }

    /// A revision that does not resolve is an error naming `--changed-since`, never an empty set.
    #[test]
    fn a_missing_revision_is_an_error_not_an_empty_changed_set() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("a.test.ts"), "x").unwrap();
        init_repository(directory.path());

        for revision in ["no-such-ref", "", "  ", "--all"] {
            let error = changed_files_since(directory.path(), revision).unwrap_err();
            let text = format!("{error:#}");
            assert!(
                text.contains("--changed-since"),
                "{revision:?} produced {text:?}"
            );
        }
    }

    /// Reads `root`'s committed test files the way `--baseline-from-ref` does, as sorted
    /// `(path, content)` pairs.
    fn read_revision(root: &Path) -> Vec<(String, String)> {
        let mut sources: Vec<_> = revision_sources(root, "HEAD", &["**/*.test.ts".to_owned()], &[])
            .unwrap()
            .into_iter()
            .map(|source| {
                (
                    source.relative.to_string_lossy().into_owned(),
                    source.content,
                )
            })
            .collect();
        sources.sort();
        sources
    }

    /// The base read returns exactly the committed text of every admitted file, whatever its
    /// name, and skips files the classifier or globs decline.
    #[test]
    fn revision_reads_return_the_committed_text_of_admitted_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut files = vec![
            ("a.test.ts", "test('a', () => {});\n"),
            ("-dash.test.ts", "test('dash', () => {});\n"),
            ("with space.test.ts", "test('space', () => {});\n"),
            ("nested/b.test.ts", "test('b', () => {});\n"),
        ];
        if cfg!(unix) {
            files.push(("line\nbreak.test.ts", "test('newline', () => {});\n"));
        }
        for (path, content) in &files {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, content).unwrap();
        }
        std::fs::write(root.join("notes.md"), "not source").unwrap();
        std::fs::write(root.join("helper.ts"), "export const x = 1;").unwrap();
        init_repository(root);
        std::fs::write(root.join("a.test.ts"), "changed in the working tree").unwrap();

        let mut expected: Vec<_> = files
            .iter()
            .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
            .collect();
        expected.sort();
        assert_eq!(read_revision(root), expected);
    }

    /// An analysis root below the git root reads its own subtree with root-relative paths.
    #[test]
    fn revision_reads_resolve_paths_from_an_analysis_root_below_the_git_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("pkg")).unwrap();
        std::fs::write(root.join("pkg/x.test.ts"), "test('x', () => {});\n").unwrap();
        std::fs::write(root.join("x.test.ts"), "test('outer', () => {});\n").unwrap();
        init_repository(root);
        assert_eq!(
            read_revision(&root.join("pkg")),
            vec![("x.test.ts".to_owned(), "test('x', () => {});\n".to_owned())]
        );
    }

    /// Oversized and non-UTF-8 blobs are skipped; the other files are still read.
    #[test]
    fn revision_reads_skip_oversized_and_non_utf8_blobs() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("ok.test.ts"), "test('ok', () => {});\n").unwrap();
        std::fs::write(root.join("bytes.test.ts"), [0xff, 0xfe, 0x00]).unwrap();
        let limit = usize::try_from(MAX_REVISION_SOURCE_BYTES).unwrap();
        std::fs::write(root.join("huge.test.ts"), "x".repeat(limit + 1)).unwrap();
        init_repository(root);
        assert_eq!(
            read_revision(root),
            vec![(
                "ok.test.ts".to_owned(),
                "test('ok', () => {});\n".to_owned()
            )]
        );
    }

    /// A child that floods stderr cannot block: the drain keeps a bounded prefix and reads the
    /// rest, so the child exits and `wait()` returns. The watchdog turns a regression into a
    /// failure instead of a hung test run.
    #[cfg(unix)]
    #[test]
    fn a_flooding_stderr_is_drained_so_the_child_can_exit() {
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut child = Command::new("sh")
                .args(["-c", "head -c 300000 /dev/zero | tr '\\0' x >&2"])
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let stderr = drain_stderr(child.stderr.take().unwrap());
            let status = child.wait().unwrap();
            let _ = done.send((status.success(), stderr.join().unwrap().len()));
        });
        let (success, kept) = finished
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("the child exited instead of blocking on a full stderr pipe");
        assert!(success);
        assert_eq!(kept, usize::try_from(MAX_KEPT_STDERR_BYTES).unwrap());
    }

    /// A failed batch read names the object, and aborting reaps the git process instead of
    /// leaving it running. The watchdog turns a cleanup regression (which would otherwise block
    /// on the live child) into a failure rather than a hung test run.
    #[test]
    fn a_failed_blob_read_is_an_error_and_abort_reaps_git() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("a.test.ts"), "x").unwrap();
        init_repository(directory.path());
        let root = directory.path().to_path_buf();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BlobReader::spawn(&root).unwrap();
            let missing = "0123456789012345678901234567890123456789";
            let error = format!("{:#}", reader.read(missing).unwrap_err());
            reader.stop();
            let reaped = matches!(reader.child.try_wait(), Ok(Some(_)));
            let _ = done.send((error.contains(missing), reaped));
            let _ = reader.abort();
        });
        let (named, reaped) = finished
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("the read failed and the process was stopped without blocking");
        assert!(named, "the error names the missing object");
        assert!(reaped, "the git process has exited and been reaped");
    }

    /// The base read costs a fixed number of git processes, not one per file (P5): spawning one
    /// `cat-file` per file made `--baseline-from-ref` take seconds on ordinary repositories.
    #[test]
    fn revision_reads_spawn_a_fixed_number_of_git_processes() {
        let spawns_for = |count: usize| {
            let directory = tempfile::tempdir().unwrap();
            for index in 0..count {
                std::fs::write(
                    directory.path().join(format!("t{index}.test.ts")),
                    format!("test('{index}', () => {{}});\n"),
                )
                .unwrap();
            }
            init_repository(directory.path());
            let before = GIT_SPAWNS.with(std::cell::Cell::get);
            assert_eq!(read_revision(directory.path()).len(), count);
            GIT_SPAWNS.with(std::cell::Cell::get) - before
        };
        assert_eq!(
            spawns_for(2),
            spawns_for(30),
            "git processes must not grow with files"
        );
    }

    /// Changed-file mode outside a Git repository is an error, not an empty set.
    #[test]
    fn a_directory_outside_any_repository_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let error = changed_files_since(directory.path(), "HEAD").unwrap_err();
        assert!(
            format!("{error:#}").contains("requires a Git repository"),
            "{error:#}"
        );
    }

    #[test]
    fn a_shallow_clone_is_an_error_naming_the_fix() {
        let origin = tempfile::tempdir().unwrap();
        std::fs::write(origin.path().join("a.test.ts"), "first").unwrap();
        init_repository(origin.path());
        std::fs::write(origin.path().join("a.test.ts"), "second").unwrap();
        assert!(fixture_git()
            .args(["commit", "-qam", "second"])
            .current_dir(origin.path())
            .status()
            .unwrap()
            .success());

        let clone = tempfile::tempdir().unwrap();
        let target = clone.path().join("shallow");
        assert!(fixture_git()
            .args(["clone", "-q", "--depth", "1", "--no-local"])
            .arg(format!("file://{}", origin.path().display()))
            .arg(&target)
            .status()
            .unwrap()
            .success());

        let error = changed_files_since(&target, "HEAD").unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("shallow clone"), "{text}");
        assert!(text.contains("fetch-depth: 0"), "{text}");
    }

    /// A git invocation that fails must be an operational error that says which listing failed
    /// and carries git's own diagnosis. The alternative -- treating a failed listing as "nothing
    /// changed" -- filters every finding away and reports a clean run over a repository nobody
    /// looked at, the exact false clean `--changed-since` is documented never to produce.
    #[test]
    fn a_failing_git_listing_is_an_error_naming_the_listing_and_quoting_git() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("a.test.ts"), "x").unwrap();
        init_repository(directory.path());

        let error = git_paths(
            directory.path(),
            &["ls-files", "--no-such-option", "-z", "--"],
            "listing untracked files for --changed-since",
        )
        .unwrap_err();
        let text = format!("{error:#}");
        assert!(
            text.contains("git failed while listing untracked files for --changed-since"),
            "the message must name what was being attempted: {text}"
        );
        assert!(
            text.contains("no-such-option"),
            "git's own stderr must survive into the message: {text}"
        );
    }

    // -- the base-revision read ----------------------------------------------------------------

    /// A committed repository holding exactly `files`, parents created as needed.
    fn repository_with(files: &[(&str, &[u8])]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        for (name, bytes) in files {
            let path = directory.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        init_repository(directory.path());
        directory
    }

    /// `revision_sources` at `HEAD` with the given admission globs.
    fn base_read(root: &Path, include: &[&str], exclude: &[&str]) -> Result<Vec<RevisionSource>> {
        let owned = |globs: &[&str]| {
            globs
                .iter()
                .map(|glob| (*glob).to_owned())
                .collect::<Vec<_>>()
        };
        revision_sources(root, "HEAD", &owned(include), &owned(exclude))
    }

    /// The refusal text of a base read that must fail, as the user would see it.
    fn base_read_error(root: &Path, include: &[&str], exclude: &[&str]) -> String {
        let error = base_read(root, include, exclude).err();
        format!("{:#}", error.expect("the base read must be refused"))
    }

    /// Admission is the classifier AND the globs: a non-source file and an excluded source file
    /// are both skipped, silently, and only the admitted paths come back, relative to the root.
    #[test]
    fn a_base_read_admits_only_source_files_the_globs_allow() {
        let repository = repository_with(&[
            ("a.test.ts", b"one"),
            ("notes.md", b"not source"),
            ("skipped.test.ts", b"excluded by glob"),
            ("nested/b.test.ts", b"two"),
        ]);
        let sources = base_read(repository.path(), &["**/*.test.ts"], &["skipped.*"]).unwrap();
        let relatives: BTreeSet<_> = sources.iter().map(|s| s.relative.clone()).collect();
        assert_eq!(
            relatives,
            BTreeSet::from([
                PathBuf::from("a.test.ts"),
                PathBuf::from("nested/b.test.ts")
            ])
        );
        assert_eq!(sources.len(), 2, "no source is listed twice");
    }

    /// A blob over the byte cap and a blob that is not UTF-8 each cost a skip, never an error
    /// and never the rest of the read: the neighbouring file is still returned.
    #[test]
    fn an_oversized_or_non_utf8_base_blob_is_skipped_and_the_rest_is_read() {
        let oversized = vec![b'a'; MAX_REVISION_SOURCE_BYTES as usize + 1];
        let repository = repository_with(&[
            ("big.test.ts", oversized.as_slice()),
            ("binary.test.ts", &[0xff, 0xfe, 0xfd]),
            ("ok.test.ts", b"fine"),
        ]);
        let sources = base_read(repository.path(), &["**/*.test.ts"], &[]).unwrap();
        let [only] = &sources[..] else {
            panic!("exactly the readable file survives, got {}", sources.len());
        };
        assert_eq!(only.relative, PathBuf::from("ok.test.ts"));
        assert_eq!(only.content, "fine");
    }

    /// A tree entry whose blob cannot be read (a submodule is listed by `ls-tree -r` as a commit,
    /// not a blob) is an operational error naming the file and quoting git, not a silent gap in
    /// the base report that would make the gate more lenient than it claims.
    #[test]
    fn an_unreadable_base_entry_is_an_error_naming_the_file_and_quoting_git() {
        let repository = repository_with(&[("a.test.ts", b"x")]);
        let root = repository.path();
        let head = fixture_git()
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .unwrap();
        let commit = String::from_utf8(head.stdout).unwrap().trim().to_owned();
        for arguments in [
            vec![
                "update-index",
                "--add",
                "--cacheinfo",
                "160000",
                &commit,
                "vendor.test.ts",
            ],
            vec!["commit", "-qm", "gitlink"],
        ] {
            assert!(fixture_git()
                .args(arguments)
                .current_dir(root)
                .status()
                .unwrap()
                .success());
        }
        let text = base_read_error(root, &["**/*.test.ts"], &[]);
        assert!(
            text.contains("git failed while reading base file `vendor.test.ts`"),
            "{text}"
        );
    }

    /// Each refusal before any file is read, under the flag's own name: an invalid glob in either
    /// list, a revision that is not a commit, and a directory outside any repository.
    #[test]
    fn a_base_read_refuses_bad_input_before_reading_anything() {
        let repository = repository_with(&[("a.test.ts", b"x")]);
        let root = repository.path();
        let text = base_read_error(root, &["["], &[]);
        assert!(text.contains("invalid include pattern `[`"), "{text}");
        let text = base_read_error(root, &["**/*.test.ts"], &["{"]);
        assert!(text.contains("invalid exclude pattern `{`"), "{text}");

        let missing = revision_sources(root, "no-such-ref", &[], &[]).err();
        let text = format!(
            "{:#}",
            missing.expect("an unresolvable revision is refused")
        );
        assert!(
            text.contains("--baseline-from-ref revision `no-such-ref` does not resolve"),
            "{text}"
        );

        let outside = tempfile::tempdir().unwrap();
        let text = format!(
            "{:#}",
            ensure_not_shallow(outside.path(), "--baseline-from-ref").unwrap_err()
        );
        assert!(text.contains("could not determine whether"), "{text}");
    }

    /// A baseline whose parent directory cannot be created is a named error, not a panic and not
    /// a silent no-op: here the would-be parent is an existing regular file.
    #[test]
    fn a_baseline_that_cannot_create_its_directory_is_an_error_naming_it() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("blocker"), "a file, not a directory").unwrap();
        let path = directory.path().join("blocker/sub/baseline.json");
        let text = format!("{:#}", write_baseline(&path, &[]).unwrap_err());
        assert!(text.contains("creating baseline directory"), "{text}");
    }
}
