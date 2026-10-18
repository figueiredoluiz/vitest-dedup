//! Gitignore-aware discovery of Vitest test files.
//!
//! This module never reads a byte of a file's contents beyond its metadata: classification is
//! extension-based and existence-based only. Parsing is `frontend`'s job, and keeping this module
//! ignorant of parsing is what lets `frontend` be swapped without touching discovery.
//!
//! # In-source testing (`includeSource` and `benchmark.includeSource`)
//!
//! `docs/vitest-surface.md`, "In-source testing" and "Default file globs": a project can
//! additionally scan ordinary application source
//! files for `if (import.meta.vitest) { ... }` blocks, governed by `test.includeSource` or
//! `test.benchmark.includeSource` (both default to `[]`, i.e. opt-in). These are *additive* to the
//! normal test and benchmark file passes -- a file can be an in-source candidate without looking
//! like a test or benchmark file at all -- so [`discover`] decides inclusion for each walked file
//! by checking every independent channel. Any one is enough to include the file, and since all
//! checks happen once per walked file before it is ever pushed into [`DiscoveredFiles::files`],
//! a file matching both can never be pushed twice.
//!
//! The single filesystem walk here already visits every file in the tree regardless of
//! in-source setting (it has to, to evaluate ordinary file includes), so supporting the feature
//! adds no second walk. What it must avoid is the *per-file* cost of matching against in-source globs when the
//! setting is absent (the common case): `prepare_in_source_candidates` returns an empty `Vec`
//! whenever every resolved project's two in-source fields have the Vitest default of `Known([])`, and an
//! empty `Vec` costs one no-op iteration per file, not a glob match.
//!
//! A project whose in-source include or corresponding exclude could not be read
//! (`GlobPatterns::Unknown`) is not guessed at: that channel contributes no candidates, a
//! diagnostic is recorded, and the run is marked incomplete, exactly like unreadable ordinary
//! discovery globs elsewhere in this module.

use crate::config::{compile_glob_set, Config};
use crate::vitest_config::GlobalsDetection;
use crate::vitest_projects::{GlobPatterns, Project, ProjectGovernance, ProjectSet};
use anyhow::Result;
use globset::GlobSet;
use ignore::WalkBuilder;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Custom ignore-file name, checked with the same negation/precedence semantics as `.gitignore`.
///
/// Lets a repository exclude generated or fixture test files from analysis without touching its
/// real `.gitignore` (fixtures are often deliberately *not* git-ignored, since they need to ship).
const VITEST_DEDUP_IGNORE_FILENAME: &str = ".vitest-dedupignore";

/// Per-file size limit before a discovered file is skipped rather than analyzed.
///
/// 8 MiB comfortably covers even large generated test files while bounding worst-case memory use
/// for the parser stage, which must hold the full source plus an AST arena at once.
pub const MAX_SOURCE_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Reads a file, refusing anything past `limit` without allocating the whole of it.
///
/// `Ok(None)` means the file exceeds the limit; callers report that as a skip rather than an
/// error, because an oversized file is a coverage gap, not a broken run. Lives here beside
/// [`MAX_SOURCE_FILE_BYTES`] -- the size ceiling and the bounded read that enforces it are one
/// concern -- and serves every caller that must not trust a path to be small (test-file
/// ingestion, `--baseline` files).
pub fn read_bounded(path: &Path, limit: u64) -> std::io::Result<Option<String>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut buffer = Vec::new();
    // `limit + 1` is enough to prove the file is over the limit without reading all of a huge one.
    file.take(limit.saturating_add(1))
        .read_to_end(&mut buffer)?;
    if buffer.len() as u64 > limit {
        return Ok(None);
    }
    String::from_utf8(buffer)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Default include patterns, matching Vitest's own `include` defaults.
///
/// Vitest documents its default as the extglob pattern `**/*.{test,spec}.?(c|m)[jt]s?(x)`, but
/// `globset` -- the matcher used everywhere in this project, the same choice CukeDedup made --
/// does not support extglob optional-groups (`?(...)`). Rather than approximate that syntax, this
/// enumerates the concrete extensions the optional groups expand to in practice: `js`, `jsx`,
/// `ts`, `tsx`, and their `.mjs`/`.cjs`/`.mts`/`.cts` module-flavor forms. This intentionally
/// drops combinations the extglob technically allows but nobody writes, such as `.mjsx`/`.ctsx`.
pub const DEFAULT_INCLUDE_GLOBS: &[&str] = &[
    "**/*.test.js",
    "**/*.test.jsx",
    "**/*.test.ts",
    "**/*.test.tsx",
    "**/*.test.mjs",
    "**/*.test.mts",
    "**/*.test.cjs",
    "**/*.test.cts",
    "**/*.spec.js",
    "**/*.spec.jsx",
    "**/*.spec.ts",
    "**/*.spec.tsx",
    "**/*.spec.mjs",
    "**/*.spec.mts",
    "**/*.spec.cjs",
    "**/*.spec.cts",
];

/// Default type-test include patterns, matching Vitest's `typecheck.include` default.
///
/// Vitest documents `['**/*.{test,spec}-d.?(c|m)[jt]s?(x)']` and consults it only when
/// `typecheck.enabled` is on (default off), so this list gates nothing on its own. Enumerated
/// rather than extglobbed for the reason [`DEFAULT_INCLUDE_GLOBS`] gives: `globset` does not
/// implement Vitest's optional groups, and this drops only the combinations nobody writes
/// (`.test-d.mjsx` and friends).
pub const DEFAULT_TYPECHECK_INCLUDE_GLOBS: &[&str] = &[
    "**/*.test-d.js",
    "**/*.test-d.jsx",
    "**/*.test-d.ts",
    "**/*.test-d.tsx",
    "**/*.test-d.mjs",
    "**/*.test-d.mts",
    "**/*.test-d.cjs",
    "**/*.test-d.cts",
    "**/*.spec-d.js",
    "**/*.spec-d.jsx",
    "**/*.spec-d.ts",
    "**/*.spec-d.tsx",
    "**/*.spec-d.mjs",
    "**/*.spec-d.mts",
    "**/*.spec-d.cjs",
    "**/*.spec-d.cts",
];

/// Default benchmark include patterns, matching Vitest's `benchmark.include` default.
///
/// Vitest documents `**/*.{bench,benchmark}.?(c|m)[jt]s?(x)`. As with
/// [`DEFAULT_INCLUDE_GLOBS`], enumerate the practical extensions because `globset` does not
/// implement the optional extglob groups Vitest accepts.
pub const DEFAULT_BENCHMARK_INCLUDE_GLOBS: &[&str] = &[
    "**/*.bench.js",
    "**/*.bench.jsx",
    "**/*.bench.ts",
    "**/*.bench.tsx",
    "**/*.bench.mjs",
    "**/*.bench.mts",
    "**/*.bench.cjs",
    "**/*.bench.cts",
    "**/*.benchmark.js",
    "**/*.benchmark.jsx",
    "**/*.benchmark.ts",
    "**/*.benchmark.tsx",
    "**/*.benchmark.mjs",
    "**/*.benchmark.mts",
    "**/*.benchmark.cjs",
    "**/*.benchmark.cts",
];

/// Vitest's `benchmark.exclude` defaults, expressed as directory globs for `globset`.
pub const DEFAULT_BENCHMARK_EXCLUDE_GLOBS: &[&str] = &[
    "**/node_modules/**",
    "**/dist/**",
    "**/.idea/**",
    "**/.git/**",
    "**/.cache/**",
];

/// Default exclude patterns, applied in addition to `.gitignore` / `.vitest-dedupignore`.
pub const DEFAULT_EXCLUDE_GLOBS: &[&str] = &[
    "**/node_modules/**",
    "**/dist/**",
    "**/build/**",
    "**/coverage/**",
    "**/.idea/**",
    "**/.git/**",
    "**/.cache/**",
    "**/.output/**",
    "**/.temp/**",
];

/// The language family selected from a source file's extension.
///
/// This is the same four-way split oxc's own `SourceType` exposes (JS vs. JSX vs. TS vs. TSX):
/// the frontend module needs exactly this information to build the `SourceType` it hands to
/// `oxc_parser`. Module-system information (ESM vs. CJS vs. ambiguous) is orthogonal and carried
/// separately in [`ModuleFlavor`], since combining them would multiply this enum by four for no
/// benefit -- oxc keys parsing mode on language and module-ness independently too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum SourceLanguage {
    /// `.js`, `.mjs`, or `.cjs`.
    JavaScript,
    /// `.jsx`.
    Jsx,
    /// `.ts`, `.mts`, or `.cts`.
    TypeScript,
    /// `.tsx`.
    Tsx,
}

/// Whether a file's extension pins its module system, independent of the nearest
/// `package.json`'s `"type"` field.
///
/// `.mjs`/`.mts` are unconditionally ESM and `.cjs`/`.cts` are unconditionally CommonJS
/// regardless of `package.json`. A bare `.js`/`.jsx`/`.ts`/`.tsx` extension is ambiguous: settling
/// it requires reading `package.json`, which is module-resolution work that belongs in
/// `frontend` (or a future `vitest_config.rs`), not in file discovery. Carrying `Ambiguous`
/// through explicitly means the frontend never has to re-derive "does the extension already know
/// the answer?" from the raw extension string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ModuleFlavor {
    /// The extension alone does not determine the module system.
    Ambiguous,
    /// `.mjs` / `.mts` -- always ECMAScript modules.
    ForcedEsm,
    /// `.cjs` / `.cts` -- always CommonJS.
    ForcedCjs,
}

/// A discovered test source file and the parser inputs it will need.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SourceFile {
    /// Absolute path to the file.
    pub path: PathBuf,
    /// Language family selected from the extension.
    pub language: SourceLanguage,
    /// Module-system commitment implied by the extension alone.
    pub module_flavor: ModuleFlavor,
    /// The nearest enclosing `vitest.config.*`/`vite.config.*` BELOW the analysis root, as its
    /// own `test` object's normalized text -- the scope this file actually runs under.
    ///
    /// S6 mechanism 2. A config beside a test directory governs how those tests run, and the
    /// scan read none of them, so two byte-identical files under `environment: 'node'` and
    /// `environment: 'happy-dom'` shared one context and `cross-file-duplicate` linked them.
    ///
    /// `None` means "nothing nearer than the analysis root governs this file", which is the
    /// ordinary case and leaves the context exactly as it was. `Some(Err(reason))` means a
    /// config IS there and could not be read -- the caller must fail closed on that rather than
    /// treat it as absent, because a config that exists and is unreadable is precisely where
    /// guessing is wrong.
    pub config_scope: Option<Result<String, String>>,
    /// The `globals` value of the project whose `includeSource` or `benchmark.includeSource` glob
    /// is why this file was discovered, when that is knowable.
    ///
    /// This exists to fix a regression: an in-source-testing candidate (`docs/vitest-surface.md`
    /// "In-source testing", admitted via `prepare_in_source_candidates`) is ordinary application
    /// source, not a regular test or benchmark file, so it may match no project's ordinary
    /// include globs. Left to
    /// [`crate::vitest_projects::ProjectSet::globals_for`], which resolves a file against
    /// regular file channels only, such a file resolves to no governing project at all and falls
    /// back to the repository-wide answer -- in a `test.projects` monorepo that fallback is
    /// often `Unknown`, and `Unknown` becomes `Disabled` downstream, so an in-source test in a
    /// `globals: true` project yields zero tests and makes the run fail operationally. Populating
    /// this field with the admitting project's own `globals` right here, while discovery still knows
    /// which project matched, lets `main.rs` use it directly instead of re-deriving governance
    /// from path globs a second time.
    ///
    /// `None` for a file discovered through an ordinary test/benchmark file path (including one
    /// that also happens to match an `includeSource` glob for the *same* file under a different
    /// code path than the one that actually admitted it -- see [`discover`]'s in-source-match
    /// check, which always runs first) or when no [`ProjectSet`] was supplied at all; callers
    /// fall back to [`crate::vitest_projects::ProjectSet::globals_for`] in that case, exactly as
    /// before this field existed. When `Some`, the value may itself be
    /// [`GlobalsDetection::Unknown`] for either of two reasons -- the admitting project's own
    /// `globals` could not be read, or `crate::vitest_projects::ProjectSet::ambiguous_after`
    /// found that a later, unresolved `test.projects` entry might match this file too and could
    /// out-rank the admitting project under "later occurrence wins" -- and a caller must not
    /// silently substitute the repository-wide fallback for either case: the uncertainty is real
    /// and has to reach the run, not be guessed away.
    pub project_globals: Option<GlobalsDetection>,
}

impl SourceFile {
    /// Builds a source file with no per-directory config scope and no attributing project --
    /// the shape for a file read outside the ordinary discovery walk, currently the
    /// `--baseline-from-ref` base revision (whose tree is read through git objects, not the
    /// filesystem, so no config is re-read for it; see `modes::revision_sources`).
    pub fn without_project_context(
        path: PathBuf,
        language: SourceLanguage,
        module_flavor: ModuleFlavor,
    ) -> Self {
        Self {
            path,
            language,
            module_flavor,
            config_scope: None,
            project_globals: None,
        }
    }
}

impl PartialOrd for SourceFile {
    /// Delegate partial comparison to the total source-file ordering.
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SourceFile {
    /// Ordered by `(path, language, module_flavor)` only, deliberately excluding
    /// `project_globals`: [`GlobalsDetection`] carries no `Ord` impl (it is a resolution outcome,
    /// not a sortable value), and `path` alone already makes every discovered file's ordering
    /// unique, so this reproduces the exact ordering the derived `Ord` gave before this field was
    /// added.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.path, self.language, self.module_flavor).cmp(&(
            &other.path,
            other.language,
            other.module_flavor,
        ))
    }
}

/// Why a matched file was not included in [`DiscoveredFiles::files`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum SkipReason {
    /// The file exceeded [`MAX_SOURCE_FILE_BYTES`] and was never read.
    TooLarge {
        /// The file's actual size in bytes.
        size_bytes: u64,
    },
    /// The extension did not map to a supported [`SourceLanguage`].
    ///
    /// Reached only when configuration widens `include` beyond the built-in defaults (which are
    /// already extension-scoped to supported languages) to something like `**/*.spec.*`.
    UnsupportedLanguage,
    /// A [`ProjectSet`] was resolved for this run, but no project governs this file -- either no
    /// resolved project's test or benchmark include globs match it (or the corresponding exclude
    /// does), or
    /// governance was [`ProjectGovernance::Unknown`] and the built-in default globs used as a
    /// fallback (see [`discover`]'s docs) do not match it either.
    ///
    /// This is the project-scoping fix: under `globals: true`, a `.cy.ts`/`.spec.js`
    /// (Mocha/Jasmine) file
    /// sitting outside the governing project's `include` is skipped here rather than analyzed as
    /// an indistinguishable ambient `describe`/`it`.
    NotInVitestProject {
        /// Human-readable explanation of why no project claims this file.
        reason: String,
    },
}

impl std::fmt::Display for SkipReason {
    /// Human-readable, because these reach a user's terminal.
    ///
    /// The orchestrator previously interpolated the `Debug` rendering, so a report explaining why
    /// a file was skipped read `NotInVitestProject { reason: "..." }` -- struct syntax leaking into
    /// output meant for a person.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { size_bytes } => {
                write!(
                    formatter,
                    "exceeds the {MAX_SOURCE_FILE_BYTES} byte limit ({size_bytes} bytes)"
                )
            }
            Self::UnsupportedLanguage => {
                formatter.write_str("not a supported JavaScript or TypeScript source file")
            }
            Self::NotInVitestProject { reason } => formatter.write_str(reason),
        }
    }
}

/// A file that matched the include/exclude configuration but was not analyzed.
///
/// Skips are reported, not silently dropped: a report that is silent about a skipped file cannot
/// be trusted to have actually looked at it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub struct SkippedFile {
    /// Absolute path to the skipped file.
    pub path: PathBuf,
    /// Why it was skipped.
    pub reason: SkipReason,
}

/// Deterministically ordered files discovered for one analysis run.
///
/// `files` and `skipped` are always sorted by path before this is returned, regardless of the
/// order the filesystem happened to yield entries in -- reports must be reproducible across runs
/// and across operating systems.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiscoveredFiles {
    /// Test source files to analyze, sorted by path.
    pub files: Vec<SourceFile>,
    /// Matched files that were not analyzed, sorted by path.
    pub skipped: Vec<SkippedFile>,
    /// Non-fatal filesystem traversal errors collected during the walk.
    pub errors: Vec<String>,
    /// Set when a skip or traversal error means the absence of a finding proves nothing.
    ///
    /// Mirrors `model::AnalysisResult::incomplete`: an oversized file might have contained a
    /// duplicate this run never saw, so a clean result is a weaker claim than usual.
    ///
    /// This is the aggregate. It says nothing about *which* of the causes below fired, and a
    /// caller that reports it as one specific cause will be wrong most of the time -- which is
    /// exactly what happened: `main` mapped it to "directory traversal failed" and printed that
    /// for a file that was merely too large.
    pub incomplete: bool,
    /// The filesystem walk itself failed somewhere: the file list may be short.
    ///
    /// The only cause that means *files are missing from this run entirely*. Every other cause
    /// below names a file that was found and then set aside, which is a different and much
    /// smaller claim.
    pub traversal_failed: bool,
    /// A resolved Vitest project's configuration could not be read well enough to scope
    /// discovery: an unknown test/benchmark include, exclude, or in-source pattern set, or a glob
    /// that failed to compile.
    ///
    /// Files here were attributed conservatively rather than guessed at, so a file that should
    /// have been analyzed may have been set aside.
    pub project_config_degraded: bool,
}

/// Walks `config.root`, applying include/exclude globs and ignore-file rules, and classifies
/// every matched file for the frontend.
///
/// `project_set`, when given, additionally scopes discovery to the resolved Vitest project
/// configuration: a file that clears `config.include`/`config.exclude` is *also* required to be
/// admitted by either that project's test or benchmark discovery channel. Passing `None`
/// reproduces today's behavior exactly -- this module never decides on
/// its own whether project configuration should be consulted, or when it gets read; that is the
/// caller's call (`main.rs` wires it from [`crate::vitest_projects::resolve_projects`]).
///
/// A file no known project includes is genuinely not a Vitest test file and is recorded in
/// [`DiscoveredFiles::skipped`] with [`SkipReason::NotInVitestProject`] rather than dropped.
/// [`ProjectGovernance::Unknown`] (a project's own test or benchmark discovery globs could not be read) is
/// never treated as exclusion: it falls back to matching this file against the built-in
/// the combined built-in test/benchmark includes and tool excludes instead, so an unreadable project config
/// degrades to the same permissive defaults a repository with no config at all gets, rather than
/// silently discovering nothing -- the silent-zero failure this project has hit before.
pub fn discover(config: &Config, project_set: Option<&ProjectSet>) -> Result<DiscoveredFiles> {
    let include = compile_glob_set(&config.include, "include")?;
    let exclude = compile_glob_set(&config.exclude, "exclude")?;
    // Per-DIRECTORY, not per-file: a directory of 200 tests reads its config once. Without this
    // the walk would re-read and re-mask the same config for every file under it.
    let mut config_scopes: HashMap<PathBuf, Option<Result<String, String>>> = HashMap::new();
    let default_project_globs = match project_set {
        Some(_) => Some((
            compile_glob_set(&owned_default_include(), "include")?,
            compile_glob_set(&owned_default_exclude(), "exclude")?,
        )),
        None => None,
    };
    let mut result = DiscoveredFiles::default();
    // Empty whenever no resolved project sets a non-empty test or benchmark `includeSource` (the
    // common case, and Vitest's own default) -- see the module docs. A `None` `project_set` never has in-source
    // candidates either: nothing here reads project configuration on its own.
    let in_source_candidates = project_set
        .map(|project_set| prepare_in_source_candidates(project_set, &mut result))
        .unwrap_or_default();

    let root = config.root.clone();
    let walk_excludes = exclude.clone();
    let mut builder = WalkBuilder::new(&config.root);
    builder
        .standard_filters(true)
        .add_custom_ignore_filename(VITEST_DEDUP_IGNORE_FILENAME)
        .require_git(false)
        // Symlinks are classified as their own file type rather than followed, so a symlink loop
        // is inert by construction -- there is no recursive descent to loop through.
        .follow_links(false)
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            let relative = entry.path().strip_prefix(&root).unwrap_or(entry.path());
            let directory_form = format!("{}/", relative.to_string_lossy().replace('\\', "/"));
            // Pruning an excluded directory here, before the walker descends into it, is not
            // just an optimization: it also means a broken or permission-denied subtree under
            // e.g. node_modules never surfaces as a traversal error.
            !walk_excludes.is_match(relative) && !walk_excludes.is_match(directory_form)
        });
    let walker = builder.build();

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                result.errors.push(format!(
                    "failed while walking {}: {error}",
                    config.root.display()
                ));
                // A directory we could not read might have hidden a duplicate.
                result.incomplete = true;
                result.traversal_failed = true;
                continue;
            }
        };
        match entry.file_type() {
            Some(file_type) if file_type.is_file() => {}
            _ => continue,
        }
        let path = entry.path().to_path_buf();
        let relative = path.strip_prefix(&config.root).unwrap_or(&path);
        // The global exclude always applies -- both the user and the defaults mean it as a filter.
        if exclude.is_match(relative) {
            continue;
        }
        let globally_included = include.is_match(relative);
        // In-source candidates are additive (`docs/vitest-surface.md`, "In-source testing"): a
        // file can win
        // inclusion here even though it fails every check below, because it is not a "test file"
        // in the ordinary sense at all -- it is application source with embedded tests. Checking
        // this first, and skipping the whole ordinary-inclusion decision when it matches, is what
        // keeps a file matching *both* `include` and `includeSource` from being considered twice:
        // either path leads to the same single push below, never two.
        //
        // When more than one project's `includeSource` matches, the *last* one in declaration
        // order wins -- the same tie-break `ProjectSet::governing_project` applies to ordinary
        // `include`/`exclude` matches, chosen for consistency with that precedent.
        let in_source_project = in_source_candidates
            .iter()
            .rfind(|candidate| candidate.matches(&path));
        let in_source_match = in_source_project.is_some();
        // Carries the admitting project's `globals` forward onto the pushed `SourceFile` (see its
        // doc comment): this is the fix for the regression this task exists to close. A file
        // admitted only through `include`/`exclude` leaves this `None`, so `main.rs` falls back to
        // `ProjectSet::globals_for` exactly as it did before this field existed.
        //
        // Resolving the admitting candidate's own globs (above) is not the end of it, exactly as
        // it is not for `ProjectSet::governing_project`'s ordinary `include`/`exclude` decision: a
        // later, unresolved `test.projects` entry might declare an `includeSource` glob that also
        // matches this file, in which case Vitest's documented "later occurrence wins" would pick
        // it instead, taking its own (unknown) `globals` with it. `ProjectSet::ambiguous_after` is
        // the exact machinery `governing_project` uses for that same question, reused here rather
        // than re-derived, so the two paths cannot drift apart again the way Copilot review #9
        // found them to have (the ordinary path was fixed to fail closed on this; this one was
        // not). The file is still discovered either way -- dropping it would be a worse failure
        // than an uncertain `globals` answer -- but an `Unknown` here reaches `main.rs` through
        // the same channel `SourceFile::project_globals` already uses for an unreadable `globals`,
        // so the run fails closed instead of silently trusting a match that might not be the real
        // governing one.
        let project_globals = in_source_project.map(|candidate| {
            match project_set
                .and_then(|project_set| project_set.ambiguous_after(&path, candidate.project))
            {
                Some(reason) => GlobalsDetection::Unknown { reason },
                None => candidate.project.globals.clone(),
            }
        });

        if !in_source_match {
            match project_set {
                Some(project_set) => {
                    match project_scope(
                        project_set,
                        &path,
                        relative,
                        default_project_globs.as_ref(),
                    ) {
                        // A governing project may *widen* the built-in defaults: a project declaring
                        // `test.include: ['src/**/*.ts']` knows its layout better than our guess at
                        // what a test file looks like. It may not widen an explicit user `include`,
                        // which is an instruction rather than a guess.
                        ProjectScope::Include => {
                            if !globally_included && config.include_is_explicit {
                                continue;
                            }
                        }
                        // Included by the fallback rather than by the project, so the run can no
                        // longer prove it stayed in scope. An unreadable project config does not
                        // resurrect a file the defaults already reject.
                        ProjectScope::IncludeDegraded { reason } => {
                            if !globally_included {
                                continue;
                            }
                            result.incomplete = true;
                            result.project_config_degraded = true;
                            // S12: relative like every other reported path. `reason` is
                            // already built from `display_path`, so an absolute prefix
                            // here printed one line carrying both spellings of the same
                            // file.
                            result
                                .errors
                                .push(format!("{}: {reason}", relative.display()));
                        }
                        ProjectScope::Skip { reason, incomplete } => {
                            // Only recorded for a file that looked like a test to begin with.
                            // Reporting every source file in the repository as "skipped" would bury
                            // the few skips that mean something.
                            if globally_included {
                                result.skipped.push(SkippedFile {
                                    path: path.clone(),
                                    reason: SkipReason::NotInVitestProject { reason },
                                });
                                if incomplete {
                                    result.incomplete = true;
                                    result.project_config_degraded = true;
                                }
                            }
                            continue;
                        }
                    }
                }
                None => {
                    if !globally_included {
                        continue;
                    }
                }
            }
        }

        // Not covered by any test, and deliberately so. `entry.metadata()` fails only when the
        // walk can see a path it then cannot stat -- a file unlinked between the two calls, or a
        // permission change mid-walk. A dangling symlink does NOT produce it (the walker does not
        // follow links, so this stats the link itself and succeeds), and racing the filesystem is
        // not a test this suite should contain. Left uncovered and named rather than reached by
        // manufacturing a path.
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                result.errors.push(format!(
                    "failed to read metadata for {}: {error}",
                    path.display()
                ));
                result.incomplete = true;
                result.traversal_failed = true;
                continue;
            }
        };
        if metadata.len() > MAX_SOURCE_FILE_BYTES {
            result.skipped.push(SkippedFile {
                path,
                reason: SkipReason::TooLarge {
                    size_bytes: metadata.len(),
                },
            });
            result.incomplete = true;
            continue;
        }

        match classify_source_file(&path) {
            Some((language, module_flavor)) => {
                let config_scope = nearest_config_scope(&path, &config.root, &mut config_scopes);
                result.files.push(SourceFile {
                    path,
                    language,
                    module_flavor,
                    config_scope,
                    project_globals,
                })
            }
            None => result.skipped.push(SkippedFile {
                path,
                reason: SkipReason::UnsupportedLanguage,
            }),
        }
    }

    result.files.sort();
    result.skipped.sort();
    result.errors.sort();
    Ok(result)
}

/// Classifies a path by extension into a `(language, module flavor)` pair, or `None` when the
/// extension is not a supported Vitest test source.
///
/// Exposed publicly so a future cross-file resolver (`oxc_resolver`) can classify a resolved
/// helper path the same way discovery classifies a walked one, without duplicating this table.
pub fn classify_source_file(path: &Path) -> Option<(SourceLanguage, ModuleFlavor)> {
    let name = path.file_name()?.to_str()?;
    // `.d.ts` (and its `.mts`/`.cts` siblings) nominally end in a recognized extension, but a
    // type-declaration file is never a test source. A suffix check alone would misclassify it,
    // so this is rejected before the extension switch below ever sees it.
    if name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts") {
        return None;
    }
    let extension = path.extension()?.to_str()?;
    match extension {
        "js" => Some((SourceLanguage::JavaScript, ModuleFlavor::Ambiguous)),
        "mjs" => Some((SourceLanguage::JavaScript, ModuleFlavor::ForcedEsm)),
        "cjs" => Some((SourceLanguage::JavaScript, ModuleFlavor::ForcedCjs)),
        "jsx" => Some((SourceLanguage::Jsx, ModuleFlavor::Ambiguous)),
        "ts" => Some((SourceLanguage::TypeScript, ModuleFlavor::Ambiguous)),
        "mts" => Some((SourceLanguage::TypeScript, ModuleFlavor::ForcedEsm)),
        "cts" => Some((SourceLanguage::TypeScript, ModuleFlavor::ForcedCjs)),
        "tsx" => Some((SourceLanguage::Tsx, ModuleFlavor::Ambiguous)),
        _ => None,
    }
}

/// Return owned test and benchmark inclusion globs used by default discovery.
fn owned_default_include() -> Vec<String> {
    DEFAULT_INCLUDE_GLOBS
        .iter()
        .chain(DEFAULT_BENCHMARK_INCLUDE_GLOBS)
        .map(|pattern| (*pattern).to_owned())
        .collect()
}

/// Return owned copies of the default discovery exclusion patterns.
fn owned_default_exclude() -> Vec<String> {
    DEFAULT_EXCLUDE_GLOBS
        .iter()
        .map(|pattern| (*pattern).to_owned())
        .collect()
}

/// Whether a file that already cleared `config.include`/`config.exclude` should also be
/// discovered under the resolved Vitest project configuration, and if not, why.
enum ProjectScope {
    /// Included because the governing project's own globs could not be read, and the built-in
    /// defaults matched. Carries why, so the run can say what it could not establish.
    IncludeDegraded { reason: String },
    /// A project governs this file (or governance could not be pinned down, but the built-in
    /// default globs -- used as the fallback -- match it anyway).
    Include,
    /// No project claims this file, so it is not a Vitest test file under the resolved
    /// configuration.
    Skip {
        /// Human-readable explanation, propagated into [`SkipReason::NotInVitestProject`].
        reason: String,
        /// Whether this skip means the run's completeness is now in question -- set only when
        /// governance was genuinely [`ProjectGovernance::Unknown`] and even the default-glob
        /// fallback could not confirm the file belongs, since that is the one case where a real
        /// Vitest test might have been missed rather than correctly recognized as out of scope.
        incomplete: bool,
    },
}

/// Decides [`ProjectScope`] for one file already known to satisfy `config.include`/`exclude`.
///
/// `default_globs` is `Some` exactly when `project_set` is `Some` (see [`discover`]) and is used
/// only for [`ProjectGovernance::Unknown`], to avoid dropping a file just because a project's own
/// test or benchmark discovery globs could not be read.
fn project_scope(
    project_set: &ProjectSet,
    absolute_path: &Path,
    relative_path: &Path,
    default_globs: Option<&(GlobSet, GlobSet)>,
) -> ProjectScope {
    match project_set.governing_project(absolute_path) {
        ProjectGovernance::Governed(_) => ProjectScope::Include,
        ProjectGovernance::NotIncluded => ProjectScope::Skip {
            reason: "no resolved Vitest project's test or benchmark include globs match this file \
                      (or the corresponding exclude globs do)"
                .to_owned(),
            incomplete: false,
        },
        ProjectGovernance::Unknown { reason } => {
            let (default_include, default_exclude) = default_globs
                .expect("default_globs is always Some when project_set is Some (see `discover`)");
            if default_include.is_match(relative_path) && !default_exclude.is_match(relative_path) {
                // Included by the fallback, not by the project. The project's own globs may well
                // have excluded this file, so analyzing it may be analyzing something out of
                // scope -- and a finding on an out-of-scope file is as wrong as a missed one.
                // Including it is still the right call (dropping files on unreadable config is the
                // silent-zero failure), but the run is no longer provably complete.
                ProjectScope::IncludeDegraded { reason }
            } else {
                ProjectScope::Skip {
                    reason: format!(
                        "{reason}; falling back to the built-in default include/exclude globs, \
                         which also do not match this file"
                    ),
                    incomplete: true,
                }
            }
        }
    }
}

/// One project's compiled `includeSource`/`exclude` glob sets, ready to test a file against
/// without recompiling a `GlobSet` per file (`compile_glob_set` is not free, and the walk visits
/// every file in the tree).
struct InSourceCandidate<'a> {
    project: &'a Project,
    include_source: GlobSet,
    exclude: GlobSet,
}

impl InSourceCandidate<'_> {
    /// Whether `absolute_path` is an in-source-testing candidate under this project: inside the
    /// project's `base_dir`, matching its `includeSource` glob, and not matching its `exclude`
    /// glob -- the same base-dir-relative resolution [`crate::vitest_projects::Project::label`]'s
    /// sibling `match_file` uses for ordinary `include`/`exclude`, since `includeSource` is
    /// resolved relative to the same root (`docs/vitest-surface.md`, "In-source testing":
    /// "resolved relative to `root`").
    fn matches(&self, absolute_path: &Path) -> bool {
        let Ok(relative) = absolute_path.strip_prefix(&self.project.base_dir) else {
            return false;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        self.include_source.is_match(&relative) && !self.exclude.is_match(&relative)
    }
}

/// Builds the compiled in-source candidate list for both `test.includeSource` and
/// `benchmark.includeSource` in every resolved project.
fn prepare_in_source_candidates<'a>(
    project_set: &'a ProjectSet,
    result: &mut DiscoveredFiles,
) -> Vec<InSourceCandidate<'a>> {
    let mut candidates = Vec::new();
    for project in &project_set.projects {
        prepare_in_source_channel(
            project,
            &project.include_source,
            &project.exclude,
            "includeSource",
            "exclude",
            result,
            &mut candidates,
        );
        prepare_in_source_channel(
            project,
            &project.benchmark_include_source,
            &project.benchmark_exclude,
            "benchmark.includeSource",
            "benchmark.exclude",
            result,
            &mut candidates,
        );
    }
    candidates
}

/// Prepare a project's in-source candidate channel, recording degradation for unknown or
/// invalid glob configuration.
#[allow(clippy::too_many_arguments)]
fn prepare_in_source_channel<'a>(
    project: &'a Project,
    include_patterns: &GlobPatterns,
    exclude_patterns: &GlobPatterns,
    include_name: &str,
    exclude_name: &str,
    result: &mut DiscoveredFiles,
    candidates: &mut Vec<InSourceCandidate<'a>>,
) {
    let patterns = match include_patterns {
        GlobPatterns::Known(patterns) if patterns.is_empty() => return,
        GlobPatterns::Known(patterns) => patterns,
        GlobPatterns::Unknown { reason } => {
            result.incomplete = true;
            result.project_config_degraded = true;
            result.errors.push(format!(
                "project {}'s `{include_name}` could not be read ({reason}); this project's \
                 in-source declarations may not be discovered",
                project.label()
            ));
            return;
        }
    };
    let include_source = match compile_glob_set(patterns, include_name) {
        Ok(set) => set,
        Err(error) => {
            result.incomplete = true;
            result.project_config_degraded = true;
            result.errors.push(format!(
                "project {}'s `{include_name}` patterns failed to compile ({error}); this \
                     project's in-source declarations may not be discovered",
                project.label()
            ));
            return;
        }
    };
    let exclude = match exclude_patterns {
        GlobPatterns::Known(patterns) => match compile_glob_set(patterns, exclude_name) {
            Ok(set) => set,
            Err(error) => {
                result.incomplete = true;
                result.project_config_degraded = true;
                result.errors.push(format!(
                    "project {}'s `{exclude_name}` patterns failed to compile ({error}); this \
                         project's in-source declarations may not be discovered",
                    project.label()
                ));
                return;
            }
        },
        GlobPatterns::Unknown { reason } => {
            result.incomplete = true;
            result.project_config_degraded = true;
            result.errors.push(format!(
                "project {}'s `{include_name}` is known but its `{exclude_name}` could not be \
                     read ({reason}); this project's in-source declarations may not be discovered",
                project.label()
            ));
            return;
        }
    };
    candidates.push(InSourceCandidate {
        project,
        include_source,
        exclude,
    });
}

/// The nearest `vitest.config.*`/`vite.config.*` strictly below `root` that governs `path`, read
/// as its `test` object's normalized text. See [`SourceFile::config_scope`].
///
/// Walks up from the file's own directory and stops AT the analysis root: the root's own config
/// is already applied to every file by the ordinary path, so including it here would say nothing
/// and would make every file in a flat repository carry the same redundant scope.
fn nearest_config_scope(
    path: &Path,
    root: &Path,
    cache: &mut HashMap<PathBuf, Option<Result<String, String>>>,
) -> Option<Result<String, String>> {
    let mut dir = path.parent()?;
    loop {
        if dir == root || !dir.starts_with(root) {
            return None;
        }
        if let Some(cached) = cache.get(dir) {
            if cached.is_some() {
                return cached.clone();
            }
        } else {
            let found = crate::vitest_config::find_project_config(dir)
                .map(|config| crate::vitest_projects::resolved_config_test_scope(&config));
            // A config with no `test` object says nothing about how tests run, so it is not a
            // scope -- keep walking up rather than stopping at it.
            let scope = match found {
                Some(Ok(None)) | None => None,
                Some(Ok(Some(text))) => Some(Ok(text)),
                Some(Err(reason)) => Some(Err(reason)),
            };
            cache.insert(dir.to_path_buf(), scope.clone());
            if scope.is_some() {
                return scope;
            }
        }
        dir = dir.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ConfigOverrides};
    use std::fs;

    /// Whether a discovered file sits directly inside `directory`, asked separator-agnostically.
    ///
    /// Two tests matched `format!("/{sub}/")` against `path.to_string_lossy()`, which is a real
    /// filesystem path: on Windows its separator is `\`, so the needle never matched and both
    /// failed with "must be discovered" — the file was found, the test could not recognize it.
    /// The analyzer itself was fine, which is the giveaway that this was only ever a test-side
    /// assumption. `AGENTS.md` already warns that CI runs windows-latest.
    ///
    /// Written once and shared so the assumption cannot come back in only one of the two places,
    /// and normalizing the way [`crate::vitest_config::display_path`] does rather than inventing a
    /// second convention.
    fn in_directory(file: &SourceFile, directory: &str) -> bool {
        file.path
            .to_string_lossy()
            .replace('\\', "/")
            .contains(&format!("/{directory}/"))
    }

    /// `in_directory` recognizes a Windows path, which is the whole reason it exists.
    ///
    /// This assertion carries the fix on its own: the two callers run on a real temp directory, so
    /// on macOS and Linux they pass with or without the normalization and prove nothing about the
    /// platform that was failing. A hand-built backslash path is the only way to pin it from a
    /// machine that is not Windows — without this, the next person to inline the `contains` back
    /// into a caller gets a green suite locally and a red one on CI.
    #[test]
    fn in_directory_recognizes_both_path_separators() {
        let file = |path: &str| SourceFile {
            path: PathBuf::from(path),
            language: SourceLanguage::TypeScript,
            module_flavor: ModuleFlavor::Ambiguous,
            config_scope: None,
            project_globals: None,
        };

        assert!(
            in_directory(&file(r"D:\a\repo\tmp\node\a.test.ts"), "node"),
            "a Windows path spells the separator `\\`, and matching `/node/` against it is how two \
             tests failed on windows-latest while passing everywhere else"
        );
        assert!(in_directory(&file("/tmp/repo/node/a.test.ts"), "node"));
        assert!(
            !in_directory(&file(r"D:\a\repo\tmp\other\a.test.ts"), "node"),
            "and it still has to be able to say no"
        );
    }

    /// Every `SkipReason` renders the sentence a user actually reads on stderr.
    ///
    /// These are the only explanation a reader gets for a file that was found and then set aside,
    /// so a wrong or empty one turns a reported skip into an unexplained absence.
    #[test]
    fn every_skip_reason_explains_itself() {
        assert_eq!(
            SkipReason::TooLarge {
                size_bytes: 9_000_000
            }
            .to_string(),
            format!("exceeds the {MAX_SOURCE_FILE_BYTES} byte limit (9000000 bytes)"),
        );
        assert_eq!(
            SkipReason::UnsupportedLanguage.to_string(),
            "not a supported JavaScript or TypeScript source file",
        );
        // `NotInVitestProject` forwards the resolver's own wording rather than restating it.
        assert_eq!(
            SkipReason::NotInVitestProject {
                reason: "no project governs it".to_owned()
            }
            .to_string(),
            "no project governs it",
        );
    }

    /// Load default configuration for a discovery fixture root, panicking on invalid fixture
    /// configuration.
    fn load(root: &Path) -> Config {
        Config::load(root, ConfigOverrides::default()).unwrap()
    }

    /// One discovery scenario: a fresh tempdir holding `files` (relative path, contents),
    /// parent directories created as needed; the config `load`ed from that root with its
    /// `include` optionally widened past the tool defaults, then `discover`ed -- project-scoped
    /// when the scenario says so, plain otherwise.
    ///
    /// Most tests in this module are exactly this scaffold plus one assertion about what came
    /// back; the helper exists so each test's interesting part -- what it asserts -- is all that
    /// remains at the call site. The tempdir is kept alive in the returned struct because some
    /// tests re-read it after discovery, and `root` is the config's own (canonicalized) root,
    /// which is what discovered paths are relative to.
    struct Scenario {
        _directory: tempfile::TempDir,
        root: std::path::PathBuf,
        files: DiscoveredFiles,
    }

    impl Scenario {
        /// Return this discovery scenario's root-relative file paths in discovery order.
        fn paths(&self) -> Vec<String> {
            relative_paths(&self.files.files, &self.root)
        }

        /// Return the requested discovered file from this scenario, panicking when the fixture
        /// did not discover it.
        fn find(&self, relative: &str) -> &SourceFile {
            find_file(&self.files, &self.root, relative)
        }
    }

    /// Write temporary fixture files, optionally override includes, and return a plain or
    /// project-scoped discovery scenario.
    fn scenario(files: &[(&str, &str)], include: &[&str], project_scoped: bool) -> Scenario {
        let directory = tempfile::tempdir().unwrap();
        for (path, contents) in files {
            let full = directory.path().join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&full, contents).unwrap();
        }
        let mut config = load(directory.path());
        if !include.is_empty() {
            config.include = include.iter().map(|glob| (*glob).to_owned()).collect();
        }
        let project_set = crate::vitest_projects::resolve_projects(&config.root);
        let files = discover(&config, project_scoped.then_some(&project_set)).unwrap();
        Scenario {
            _directory: directory,
            root: config.root,
            files,
        }
    }

    /// [`scenario`] reduced to its most common assertion: the discovered files' root-relative
    /// paths, in discovery order.
    fn scenario_paths(
        files: &[(&str, &str)],
        include: &[&str],
        project_scoped: bool,
    ) -> Vec<String> {
        scenario(files, include, project_scoped).paths()
    }

    /// Render discovered file paths relative to the fixture root with normalized separators.
    fn relative_paths(files: &[SourceFile], root: &Path) -> Vec<String> {
        files
            .iter()
            .map(|file| {
                file.path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    /// A CommonJS per-directory config is UNREADABLE, not "no config".
    ///
    /// `config_test_scope` returned `Ok(None)` for `ConfigObject::NoExport`, so
    /// `nearest_config_scope` kept walking and the file compared as if the analysis root governed
    /// it -- silently ignoring the environment. `.cjs` is in `CONFIG_EXTENSIONS`, so
    /// `module.exports = { test: { environment: … } }` is a real shape that reached this branch,
    /// and it brought back the very false positive the scope exists to remove. Reproduced against
    /// the release binary before the fix; found in review.
    #[test]
    fn a_commonjs_config_is_unreadable_rather_than_absent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("cjs")).unwrap();
        fs::write(root.join("cjs").join("a.test.ts"), "test('t', () => {});").unwrap();
        fs::write(
            root.join("cjs").join("vitest.config.cjs"),
            "module.exports = { test: { environment: 'happy-dom' } }",
        )
        .unwrap();

        let discovered = discover(&load(root), None).unwrap();
        let scope = discovered
            .files
            .iter()
            .find(|file| in_directory(file, "cjs"))
            .expect("cjs/a.test.ts must be discovered")
            .config_scope
            .clone();
        assert!(
            matches!(scope, Some(Err(_))),
            "a config whose `export default` this scan cannot recognize must fail closed, not read \
             as though no config governed the file: {scope:?}"
        );
    }

    /// S6 mechanism 2: a config below the analysis root is found, read, and attached per file.
    ///
    /// Reverting the attachment left every `cargo test` green -- only the recall gate caught it --
    /// so this pins the discovery half directly. Three states, because the third is the one that
    /// must NOT be mistaken for the first: absent, present-and-readable, present-and-unreadable.
    #[test]
    fn a_config_below_the_analysis_root_is_attached_to_the_files_it_governs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (sub, config) in [
            (
                "node",
                Some("export default { test: { environment: 'node' } }"),
            ),
            (
                "happy",
                Some("export default { test: { environment: 'happy-dom' } }"),
            ),
            ("plain", None),
        ] {
            fs::create_dir_all(root.join(sub)).unwrap();
            fs::write(root.join(sub).join("a.test.ts"), "test('t', () => {});").unwrap();
            if let Some(text) = config {
                fs::write(root.join(sub).join("vitest.config.ts"), text).unwrap();
            }
        }
        // A config that exists and cannot be read: an unterminated string leaves the scan unable
        // to tell code from text past that point.
        fs::create_dir_all(root.join("broken")).unwrap();
        fs::write(
            root.join("broken").join("a.test.ts"),
            "test('t', () => {});",
        )
        .unwrap();
        fs::write(
            root.join("broken").join("vitest.config.ts"),
            "export default { test: { environment: 'node} }",
        )
        .unwrap();

        let discovered = discover(&load(root), None).unwrap();
        let scope_of = |sub: &str| {
            discovered
                .files
                .iter()
                .find(|file| in_directory(file, sub))
                .unwrap_or_else(|| panic!("{sub}/a.test.ts must be discovered"))
                .config_scope
                .clone()
        };

        let node = scope_of("node");
        let happy = scope_of("happy");
        assert!(matches!(node, Some(Ok(_))), "readable config: {node:?}");
        assert_ne!(node, happy, "different environments are different scopes");
        assert_eq!(
            scope_of("plain"),
            None,
            "no config below the root means the root governs, exactly as before this field"
        );
        assert!(
            matches!(scope_of("broken"), Some(Err(_))),
            "a config that EXISTS and cannot be read is not the same as no config -- callers \
             fail closed on it"
        );
    }

    /// Check file discovery: discovers default vitest test and spec files but ignores git
    /// excludes.
    #[test]
    fn discovers_default_vitest_test_and_spec_files_but_ignores_git_excludes() {
        assert_eq!(
            scenario_paths(
                &[
                    (".gitignore", "dist/\n"),
                    ("src/a.test.ts", ""),
                    ("src/b.spec.tsx", ""),
                    ("src/helper.ts", ""),
                    ("dist/c.test.js", ""),
                ],
                &[],
                false,
            ),
            vec!["src/a.test.ts", "src/b.spec.tsx"]
        );
    }

    /// Check file discovery: discovers default vitest bench and benchmark files.
    #[test]
    fn discovers_default_vitest_bench_and_benchmark_files() {
        assert_eq!(
            scenario_paths(
                &[
                    ("perf/a.bench.ts", ""),
                    ("perf/b.benchmark.mjs", ""),
                    ("perf/helper.ts", ""),
                ],
                &[],
                false,
            ),
            vec!["perf/a.bench.ts", "perf/b.benchmark.mjs"]
        );
    }

    /// Check file discovery: benchmark include and exclude are independent from test file
    /// globs.
    #[test]
    fn benchmark_include_and_exclude_are_independent_from_test_file_globs() {
        assert_eq!(
            scenario_paths(
                &[
                    (
                        "vitest.config.ts",
                        r#"export default {
                test: {
                    include: ['tests/**/*.test.ts'],
                    benchmark: {
                        include: ['perf/**/*.perf.ts'],
                        exclude: ['perf/skip/**'],
                    },
                },
            };"#,
                    ),
                    ("tests/a.test.ts", ""),
                    ("perf/a.perf.ts", ""),
                    ("perf/skip/b.perf.ts", ""),
                    ("perf/not-in-custom.bench.ts", ""),
                ],
                &[],
                true,
            ),
            vec!["perf/a.perf.ts", "tests/a.test.ts"]
        );
    }

    /// Check file discovery: custom test include does not erase the default benchmark include.
    #[test]
    fn custom_test_include_does_not_erase_the_default_benchmark_include() {
        assert_eq!(
            scenario_paths(
                &[
                    (
                        "vitest.config.ts",
                        "export default { test: { include: ['tests/**/*.test.ts'] } };",
                    ),
                    ("tests/a.test.ts", ""),
                    ("perf/a.bench.ts", ""),
                ],
                &[],
                true,
            ),
            vec!["perf/a.bench.ts", "tests/a.test.ts"]
        );
    }

    /// Check file discovery: unreadable benchmark include falls back visibly for default
    /// benchmark files.
    #[test]
    fn unreadable_benchmark_include_falls_back_visibly_for_default_benchmark_files() {
        let scenario = scenario(
            &[
                (
                    "vitest.config.ts",
                    "export default { test: { benchmark: { include: makePatterns() } } };",
                ),
                ("perf/a.bench.ts", ""),
            ],
            &[],
            true,
        );
        assert_eq!(scenario.paths(), vec!["perf/a.bench.ts"]);
        assert!(scenario.files.incomplete);
        assert!(scenario.files.project_config_degraded);
        assert!(
            scenario
                .files
                .errors
                .iter()
                .any(|error| error.contains("benchmark.include")),
            "expected benchmark.include diagnostic, got {:#?}",
            scenario.files.errors
        );
    }

    /// Check file discovery: benchmark include source is additive and uses benchmark exclude.
    #[test]
    fn benchmark_include_source_is_additive_and_uses_benchmark_exclude() {
        assert_eq!(
            scenario_paths(
                &[
                    (
                        "vitest.config.ts",
                        r#"export default {
                test: {
                    benchmark: {
                        includeSource: ['src/**/*.ts'],
                        exclude: ['src/skip/**'],
                    },
                },
            };"#,
                    ),
                    ("src/perf.ts", ""),
                    ("src/skip/perf.ts", ""),
                ],
                &[],
                true,
            ),
            vec!["src/perf.ts"]
        );
    }

    /// Check file discovery: built in excludes prune node modules and friends.
    #[test]
    fn built_in_excludes_prune_node_modules_and_friends() {
        assert_eq!(
            scenario_paths(
                &[
                    ("node_modules/pkg/x.test.ts", ""),
                    ("build/x.test.ts", ""),
                    ("coverage/x.test.ts", ""),
                    (".cache/x.test.ts", ""),
                    ("kept.test.ts", ""),
                ],
                &[],
                false,
            ),
            vec!["kept.test.ts"]
        );
    }

    /// Check file discovery: vitest dedupignore supports negation like gitignore.
    #[test]
    fn vitest_dedupignore_supports_negation_like_gitignore() {
        // A negation for a path nested inside an *ignored directory* famously cannot re-include
        // it under real gitignore semantics (git never descends into the pruned directory to
        // evaluate the negation at all), and the `ignore` crate matches that behavior exactly.
        // So this exercises negation the way it actually works: a file-level pattern followed by
        // a file-level negation, both at the same directory level.
        assert_eq!(
            scenario_paths(
                &[
                    (
                        ".vitest-dedupignore",
                        "*.generated.test.ts\n!keep.generated.test.ts\n",
                    ),
                    ("drop.generated.test.ts", ""),
                    ("keep.generated.test.ts", ""),
                ],
                &[],
                false,
            ),
            vec!["keep.generated.test.ts"]
        );
    }

    /// Check file discovery: discovery order is independent of filesystem traversal order.
    #[test]
    fn discovery_order_is_independent_of_filesystem_traversal_order() {
        // Create in reverse alphabetical order, and across sibling directories, so an
        // implementation that forgot to sort would very likely fail this test. `scenario`
        // writes in list order, which is the reverse-alphabetical order named here.
        assert_eq!(
            scenario_paths(
                &[
                    ("z/a.test.ts", "// z"),
                    ("y/a.test.ts", "// y"),
                    ("x/a.test.ts", "// x"),
                    ("w/a.test.ts", "// w"),
                ],
                &[],
                false,
            ),
            vec!["w/a.test.ts", "x/a.test.ts", "y/a.test.ts", "z/a.test.ts"]
        );
    }

    /// Check file discovery: file at exactly the size limit is kept and one byte over is
    /// skipped.
    #[test]
    fn file_at_exactly_the_size_limit_is_kept_and_one_byte_over_is_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let at_limit = vec![b'a'; MAX_SOURCE_FILE_BYTES as usize];
        let over_limit = vec![b'a'; MAX_SOURCE_FILE_BYTES as usize + 1];
        fs::write(directory.path().join("at-limit.test.ts"), &at_limit).unwrap();
        fs::write(directory.path().join("over-limit.test.ts"), &over_limit).unwrap();
        let config = load(directory.path());

        let files = discover(&config, None).unwrap();
        assert_eq!(
            relative_paths(&files.files, &config.root),
            vec!["at-limit.test.ts"]
        );
        assert_eq!(files.skipped.len(), 1);
        assert!(files.skipped[0].path.ends_with("over-limit.test.ts"));
        assert!(matches!(
            files.skipped[0].reason,
            SkipReason::TooLarge { size_bytes } if size_bytes == MAX_SOURCE_FILE_BYTES + 1
        ));
        assert!(files.incomplete);
    }

    /// Check file discovery: unsupported extensions are rejected explicitly not guessed at.
    #[test]
    fn unsupported_extensions_are_rejected_explicitly_not_guessed_at() {
        assert_eq!(classify_source_file(Path::new("legacy.test.coffee")), None);
        assert_eq!(classify_source_file(Path::new("types.test.d.ts")), None);
        assert_eq!(classify_source_file(Path::new("types.d.mts")), None);
        assert_eq!(classify_source_file(Path::new("types.d.cts")), None);
    }

    /// Check file discovery: widened include reaches the unsupported language skip path.
    #[test]
    fn widened_include_reaches_the_unsupported_language_skip_path() {
        // The built-in defaults are already extension-scoped, so nothing unsupported can reach
        // classification through them. Widening `include` exercises the skip path the way a
        // project's own `.vitest-dedup.json` could.
        let scenario = scenario(
            &[("legacy.test.coffee", ""), ("kept.test.ts", "")],
            &["**/*.test.*"],
            false,
        );
        assert_eq!(scenario.paths(), vec!["kept.test.ts"]);
        assert_eq!(scenario.files.skipped.len(), 1);
        assert!(scenario.files.skipped[0]
            .path
            .ends_with("legacy.test.coffee"));
        assert_eq!(
            scenario.files.skipped[0].reason,
            SkipReason::UnsupportedLanguage
        );
        // Rejecting an unsupported extension is a deliberate classification decision, not a
        // sign that analysis missed something -- unlike an oversized file, it must not mark the
        // run incomplete.
        assert!(!scenario.files.incomplete);
    }

    /// Check file discovery: symlink loops do not hang or error discovery.
    #[cfg(unix)]
    #[test]
    fn symlink_loops_do_not_hang_or_error_discovery() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("loop")).unwrap();
        fs::write(directory.path().join("loop/a.test.ts"), "").unwrap();
        // A symlink back to an ancestor directory: if this were ever followed, walking it would
        // recurse forever. Created before discovery runs, which is why this test keeps its own
        // scaffold rather than [`scenario`].
        symlink(directory.path(), directory.path().join("loop/self")).unwrap();
        let config = load(directory.path());
        let discovered = discover(&config, None).unwrap();
        assert_eq!(
            relative_paths(&discovered.files, &config.root),
            vec!["loop/a.test.ts"]
        );
    }

    /// Check file discovery: traversal errors are collected and mark the run incomplete.
    #[cfg(unix)]
    #[test]
    fn traversal_errors_are_collected_and_mark_the_run_incomplete() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("visible.test.ts"), "").unwrap();
        let locked = directory.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("hidden.test.ts"), "").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let config = load(directory.path());

        let files = discover(&config, None).unwrap();

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            relative_paths(&files.files, &config.root),
            vec!["visible.test.ts"]
        );
        assert!(!files.errors.is_empty());
        assert!(files.incomplete);
    }

    /// Check file discovery: module flavor reflects forced esm and cjs extensions.
    #[test]
    fn module_flavor_reflects_forced_esm_and_cjs_extensions() {
        assert_eq!(
            classify_source_file(Path::new("a.test.mjs")),
            Some((SourceLanguage::JavaScript, ModuleFlavor::ForcedEsm))
        );
        assert_eq!(
            classify_source_file(Path::new("a.test.cjs")),
            Some((SourceLanguage::JavaScript, ModuleFlavor::ForcedCjs))
        );
        assert_eq!(
            classify_source_file(Path::new("a.test.mts")),
            Some((SourceLanguage::TypeScript, ModuleFlavor::ForcedEsm))
        );
        assert_eq!(
            classify_source_file(Path::new("a.test.cts")),
            Some((SourceLanguage::TypeScript, ModuleFlavor::ForcedCjs))
        );
        assert_eq!(
            classify_source_file(Path::new("a.test.ts")),
            Some((SourceLanguage::TypeScript, ModuleFlavor::Ambiguous))
        );
    }

    // -- Project scoping ----------------------------------------------------------------------

    use crate::vitest_projects::{resolve_projects, GlobPatterns};

    /// Check file discovery: project scoping skips a cypress file outside include and keeps a
    /// test file inside it.
    #[test]
    fn project_scoping_skips_a_cypress_file_outside_include_and_keeps_a_test_file_inside_it() {
        // Under `globals: true`, a `.cy.ts` file's ambient `describe`/`it` is statically
        // indistinguishable from Vitest's own. The sound fix is respecting the governing
        // project's own `include` globs rather than guessing from the extension.
        // The tool's own include is widened (see [`scenario`]'s `include` parameter) so both
        // files clear that gate -- the project's resolved `include` is what should do the real
        // scoping being tested here.
        let scenario = scenario(
            &[
                (
                    "vitest.config.ts",
                    r#"export default defineConfig({
                test: { globals: true, include: ['src/**/*.test.ts'] },
            });"#,
                ),
                ("src/component.cy.ts", ""),
                ("src/component.test.ts", ""),
            ],
            &["**/*.ts"],
            true,
        );

        assert_eq!(scenario.paths(), vec!["src/component.test.ts"]);
        let skipped = scenario
            .files
            .skipped
            .iter()
            .find(|file| file.path.ends_with("component.cy.ts"))
            .expect("component.cy.ts must be recorded as skipped, not silently dropped");
        assert!(matches!(
            skipped.reason,
            SkipReason::NotInVitestProject { .. }
        ));
        // No project including this file is a deliberate classification, not a sign analysis
        // missed something.
        assert!(!scenario.files.incomplete);
    }

    /// Check file discovery: unreadable project globs fall back to defaults instead of
    /// discovering nothing.
    #[test]
    fn unreadable_project_globs_fall_back_to_defaults_instead_of_discovering_nothing() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("vitest.config.ts"),
            // `include` set to a call expression -- not an array literal this scan can read --
            // is exactly the "globs cannot be read" case `GlobPatterns::Unknown` exists for.
            "export default defineConfig({ test: { include: someHelper() } });",
        )
        .unwrap();
        fs::write(directory.path().join("a.test.ts"), "").unwrap();
        let config = load(directory.path());
        let project_set = resolve_projects(&config.root);
        // Sanity: the project's own `include` really is unresolved, not merely absent.
        assert!(matches!(
            project_set.projects[0].include,
            GlobPatterns::Unknown { .. }
        ));

        let files = discover(&config, Some(&project_set)).unwrap();

        // Dropping this file because the project's own `include` could not be read would
        // recreate the silent-zero failure this project has hit before -- falling back to the
        // built-in default globs keeps it discovered.
        assert_eq!(
            relative_paths(&files.files, &config.root),
            vec!["a.test.ts"]
        );
        assert!(files.skipped.is_empty());
    }

    /// Check file discovery: passing no project set reproduces today's behavior exactly.
    #[test]
    fn passing_no_project_set_reproduces_todays_behavior_exactly() {
        // With no `ProjectSet` supplied, project scoping never runs at all: every file that
        // clears the tool's own `include`/`exclude` is discovered, exactly as before this
        // feature existed -- discovery never decides on its own to start reading project
        // configuration.
        let scenario = scenario(&[("a.test.ts", ""), ("b.cy.ts", "")], &["**/*.ts"], false);
        assert_eq!(scenario.paths(), vec!["a.test.ts", "b.cy.ts"]);
        assert!(scenario.files.skipped.is_empty());
    }

    // -- Type tests (`typecheck.include`) ----------------------------------------------------

    /// C3: a `typecheck`-enabled project discovers its type-test files. Before this channel
    /// existed, `.test-d.ts` matched neither the tool's own defaults nor any project glob, so a
    /// directory of type tests extracted zero tests -- honestly flagged, but analyzed not at all.
    #[test]
    fn typecheck_enabled_discovers_type_test_files() {
        let scenario = scenario(
            &[
                (
                    "vitest.config.ts",
                    r#"export default defineConfig({
                test: { typecheck: { enabled: true, include: ['**/*.test-d.ts'] } },
            });"#,
                ),
                ("types/parse.test-d.ts", ""),
                ("types/run.a.test.ts", ""),
            ],
            &[],
            true,
        );
        assert_eq!(
            scenario.paths(),
            vec!["types/parse.test-d.ts", "types/run.a.test.ts"],
            "the typecheck channel admits the type-test file alongside the ordinary one"
        );
        assert!(!scenario.files.incomplete);
    }

    /// `typecheck.include` defaults to the `*.test-d`/`*.spec-d` family when `enabled` is on and
    /// `include` is not written, exactly as `include` defaults to the `*.test` family.
    #[test]
    fn typecheck_enabled_without_include_uses_the_default_type_test_globs() {
        let scenario = scenario(
            &[
                (
                    "vitest.config.ts",
                    "export default defineConfig({ test: { typecheck: { enabled: true } } });",
                ),
                ("types/a.test-d.ts", ""),
                ("types/b.spec-d.ts", ""),
                ("types/c.test-d.cts", ""),
                // A `-d` in the wrong position is not a type test: `.testd.ts` matches no
                // typecheck default and must not be swept in by the channel.
                ("types/d.testd.ts", ""),
            ],
            &[],
            true,
        );
        assert_eq!(
            scenario.paths(),
            vec![
                "types/a.test-d.ts",
                "types/b.spec-d.ts",
                "types/c.test-d.cts"
            ]
        );
    }

    /// The gate comes first: with `enabled` off (Vitest's default, and the default inherited when
    /// the key is absent), the same `.test-d.ts` file is not a test file under the project, and
    /// `typecheck.include` -- written or default -- decides nothing.
    #[test]
    fn typecheck_disabled_ignores_the_include_entirely() {
        for config in [
            "export default defineConfig({ test: {} });",
            "export default defineConfig({ test: { typecheck: { enabled: false, include: \
             ['**/*.test-d.ts'] } } });",
        ] {
            let scenario = scenario(
                &[
                    ("vitest.config.ts", config),
                    ("types/parse.test-d.ts", ""),
                    ("types/run.a.test.ts", ""),
                ],
                &[],
                true,
            );
            assert_eq!(
                scenario.paths(),
                vec!["types/run.a.test.ts"],
                "type tests are not test files while typecheck is off: {config}"
            );
        }
    }

    /// The test channel's own naming contract, in all three states a project can be in.
    ///
    /// `.test.*` and `.spec.*` are the built-in conventions, and anything else -- `.unit.ts` is the
    /// common third -- is a test file only when a config says so. All three halves were verified
    /// against the release binary and all three already held; what was missing was an assertion, and
    /// specifically the REPLACEMENT one: `a_custom_typecheck_include_replaces_the_defaults` pins
    /// that for the typecheck channel, and `custom_test_include_does_not_erase_the_default_benchmark_include`
    /// pins that a custom test include leaves the BENCHMARK channel alone, but nothing said that a
    /// custom test include stops a default-matching `.test.ts` from being discovered.
    ///
    /// That is the half a well-meaning change breaks. Making `include` additive would look
    /// generous and would make this tool analyze files Vitest does not run -- every finding in them
    /// a false positive about code the project never executes as a test.
    #[test]
    fn test_file_naming_follows_the_config_and_defaults_to_test_and_spec() {
        // No config: both built-in conventions, and nothing else. Unscoped on purpose — with no
        // config there is no project to resolve, so running this project-scoped would let the
        // absence of a project set decide the result instead of the globs.
        let defaults = scenario(
            &[("a.test.ts", ""), ("b.spec.ts", ""), ("c.unit.ts", "")],
            &[],
            false,
        );
        assert_eq!(
            defaults.paths(),
            vec!["a.test.ts", "b.spec.ts"],
            "`.test.*` and `.spec.*` are the built-in conventions; `.unit.ts` is not one until a \
             config says it is"
        );

        // A config naming its own convention: that one, and the defaults no longer apply.
        let custom = scenario(
            &[
                (
                    "vitest.config.ts",
                    "export default defineConfig({ test: { include: ['**/*.unit.ts'] } });",
                ),
                ("a.test.ts", ""),
                ("b.spec.ts", ""),
                ("c.unit.ts", ""),
            ],
            &[],
            true,
        );
        assert_eq!(
            custom.paths(),
            vec!["c.unit.ts"],
            "a declared `include` is an instruction, not an addition -- Vitest would run only \
             these, and analyzing more means reporting findings in files that are not tests"
        );

        // A config that exists but says nothing about `include` keeps the defaults.
        let silent = scenario(
            &[
                (
                    "vitest.config.ts",
                    "export default defineConfig({ test: { environment: 'node' } });",
                ),
                ("a.test.ts", ""),
                ("c.unit.ts", ""),
            ],
            &[],
            true,
        );
        assert_eq!(
            silent.paths(),
            vec!["a.test.ts"],
            "the presence of a config is not itself an instruction about naming"
        );
    }

    /// A custom `typecheck.include` is an instruction: the defaults do not also apply, so a
    /// project naming `**/*.spec-d.ts` does not sweep in `.test-d.ts`.
    #[test]
    fn a_custom_typecheck_include_replaces_the_defaults() {
        let scenario = scenario(
            &[
                (
                    "vitest.config.ts",
                    r#"export default defineConfig({
                test: { typecheck: { enabled: true, include: ['**/*.spec-d.ts'] } },
            });"#,
                ),
                ("types/a.test-d.ts", ""),
                ("types/b.spec-d.ts", ""),
            ],
            &[],
            true,
        );
        assert_eq!(scenario.paths(), vec!["types/b.spec-d.ts"]);
    }

    /// An `enabled` this scan cannot read must not silently OPEN the channel: the type-test file
    /// is not guessed in through a gate that could not be read, and the ordinary test file's own
    /// channel is unaffected.
    ///
    /// The run's flags are asserted too, as the recorded shape of a known limitation rather than
    /// a claim: an unreadable gate over files that do not match the built-in defaults stays
    /// silent here, exactly as an unreadable `include` already does --
    /// `an_unreadable_projects_entry_alone_does_not_raise_discoverys_degraded_flag` pins that
    /// discovery's degraded flag means "a file fell through to default matching" and that
    /// map-level uncertainty is the CLI boundary's to report. Widening this arm to carry the
    /// gate case too was tried and reverted: it double-flagged every unresolved-`projects`
    /// repository.
    #[test]
    fn an_unreadable_typecheck_enabled_never_guesses_the_channel_open() {
        let scenario = scenario(
            &[
                (
                    "vitest.config.ts",
                    r#"export default defineConfig({
                test: { typecheck: { enabled: isCi(), include: ['**/*.test-d.ts'] } },
            });"#,
                ),
                ("types/parse.test-d.ts", ""),
                ("types/run.a.test.ts", ""),
            ],
            &[],
            true,
        );
        assert_eq!(scenario.paths(), vec!["types/run.a.test.ts"]);
        assert!(!scenario.files.incomplete);
    }

    /// The whole `typecheck` value being unreadable, which is a different arm from an unreadable
    /// `enabled` inside a readable one.
    ///
    /// A config sharing its typecheck settings through a variable — `typecheck: shared` — cannot be
    /// read as an object literal by a scan that executes nothing. The channel must stay CLOSED:
    /// guessing it open would analyze type tests the project may not run, and guessing at
    /// `include` would pick files the real config never names. The ordinary test channel is
    /// untouched either way, which is the half that says the refusal is narrow.
    #[test]
    fn an_unreadable_typecheck_object_leaves_the_channel_closed() {
        let scenario = scenario(
            &[
                (
                    "vitest.config.ts",
                    "export default defineConfig({ test: { typecheck: sharedTypecheck } });",
                ),
                ("types/parse.test-d.ts", ""),
                ("types/run.a.test.ts", ""),
            ],
            &[],
            true,
        );
        assert_eq!(
            scenario.paths(),
            vec!["types/run.a.test.ts"],
            "an unreadable `typecheck` admits no type tests and costs the test channel nothing"
        );
    }

    // -- In-source testing (`includeSource`) -------------------------------------------------

    /// Check file discovery: include source widens discovery to a matching non test source
    /// file.
    #[test]
    fn include_source_widens_discovery_to_a_matching_non_test_source_file() {
        // docs/vitest-surface.md, "In-source testing": `src/thing.ts` is ordinary application
        // source, not a test
        // file, and does not match the tool's own `include` (no `.test.`/`.spec.` in the name).
        // It must still be discovered because `includeSource` names it explicitly.
        let scenario = scenario(
            &[
                (
                    "vitest.config.ts",
                    r#"export default defineConfig({
                test: { includeSource: ['src/**/*.ts'] },
            });"#,
                ),
                ("src/thing.ts", ""),
            ],
            &[],
            true,
        );
        assert_eq!(scenario.paths(), vec!["src/thing.ts"]);
        assert!(!scenario.files.incomplete);
    }

    /// Check file discovery: absent include source does not discover the same source file.
    #[test]
    fn absent_include_source_does_not_discover_the_same_source_file() {
        // The same project, minus `includeSource`: `includeSource` defaults to `[]`, so
        // `src/thing.ts` must not be discovered.
        assert!(scenario_paths(
            &[
                (
                    "vitest.config.ts",
                    "export default defineConfig({ test: {} });",
                ),
                ("src/thing.ts", ""),
            ],
            &[],
            true,
        )
        .is_empty());
    }

    /// Check file discovery: project exclude still wins over a matching include source glob.
    #[test]
    fn project_exclude_still_wins_over_a_matching_include_source_glob() {
        assert_eq!(
            scenario_paths(
                &[
                    (
                        "vitest.config.ts",
                        r#"export default defineConfig({
                test: {
                    includeSource: ['src/**/*.ts'],
                    exclude: ['src/excluded.ts'],
                },
            });"#,
                    ),
                    ("src/thing.ts", ""),
                    ("src/excluded.ts", ""),
                ],
                &[],
                true,
            ),
            vec!["src/thing.ts"]
        );
    }

    /// Check file discovery: a file matching both include and include source is discovered
    /// exactly once.
    #[test]
    fn a_file_matching_both_include_and_include_source_is_discovered_exactly_once() {
        // `src/thing.test.ts` uses the ordinary Vitest naming convention, so it also matches
        // the tool's own default `include`.
        assert_eq!(
            scenario_paths(
                &[
                    (
                        "vitest.config.ts",
                        r#"export default defineConfig({
                test: { includeSource: ['src/**/*.ts'] },
            });"#,
                    ),
                    ("src/thing.test.ts", ""),
                ],
                &[],
                true,
            ),
            vec!["src/thing.test.ts"]
        );
    }

    /// Check file discovery: unreadable include source marks the run incomplete with a
    /// diagnostic naming it.
    #[test]
    fn unreadable_include_source_marks_the_run_incomplete_with_a_diagnostic_naming_it() {
        // `includeSource` set to a call expression is not a literal array this scan can read --
        // the same "cannot handle soundly" shape as an unreadable `include`/`exclude`. Rather than
        // guess, this must show up as an explicit incompleteness signal naming the setting.
        let scenario = scenario(
            &[
                (
                    "vitest.config.ts",
                    "export default defineConfig({ test: { includeSource: someHelper() } });",
                ),
                ("src/thing.ts", ""),
            ],
            &[],
            true,
        );

        // Not guessed into the result -- an unreadable `includeSource` never invents a candidate.
        assert!(scenario.paths().is_empty());
        assert!(scenario.files.incomplete);
        assert!(
            scenario
                .files
                .errors
                .iter()
                .any(|error| error.contains("includeSource")),
            "expected a diagnostic naming `includeSource`, got {:#?}",
            scenario.files.errors
        );
    }

    // -- Carrying the admitting project's `globals` forward (regression fix) ----------------

    /// Find a root-relative fixture path in discovery results, panicking with the full result
    /// when it is absent.
    fn find_file<'a>(files: &'a DiscoveredFiles, root: &Path, relative: &str) -> &'a SourceFile {
        files
            .files
            .iter()
            .find(|file| file.path.strip_prefix(root).unwrap() == Path::new(relative))
            .unwrap_or_else(|| panic!("{relative} was not discovered; got {files:#?}"))
    }

    /// Asserts that the file a project-scoped scenario discovers carries a specific
    /// `project_globals` shape. The four callers differ only in the config's `globals`
    /// expression and which file is discovered; each keeps its own name and comment, which is
    /// where its behavioral claim lives.
    fn assert_carried_globals(
        config: &str,
        discovered: &str,
        shape: fn(&Option<GlobalsDetection>) -> bool,
    ) {
        let scenario = scenario(&[("vitest.config.ts", config), (discovered, "")], &[], true);
        let file = scenario.find(discovered);
        assert!(
            shape(&file.project_globals),
            "unexpected project_globals on {discovered}: {:?}",
            file.project_globals
        );
    }

    /// Check file discovery: in source file in a globals true project carries enabled globals
    /// onto the source file.
    #[test]
    fn in_source_file_in_a_globals_true_project_carries_enabled_globals_onto_the_source_file() {
        // The regression this task exists to fix: an in-source candidate never matches any
        // project's `include`, so a caller resolving `globals` from path globs alone
        // (`ProjectSet::globals_for`) finds no governing project. Discovery must carry the
        // admitting project's own `globals` forward instead, so a `globals: true` project's
        // in-source tests are analyzed as such rather than falling back to a repo-wide answer.
        assert_carried_globals(
            "export default defineConfig({ test: { globals: true, includeSource: ['src/**/*.ts'] } });",
            "src/thing.ts",
            |globals| matches!(globals, Some(GlobalsDetection::Enabled { .. })),
        );
    }

    /// Check file discovery: in source file in a globals false project carries disabled globals
    /// onto the source file.
    #[test]
    fn in_source_file_in_a_globals_false_project_carries_disabled_globals_onto_the_source_file() {
        assert_carried_globals(
            "export default defineConfig({ test: { globals: false, includeSource: ['src/**/*.ts'] } });",
            "src/thing.ts",
            |globals| matches!(globals, Some(GlobalsDetection::Disabled { .. })),
        );
    }

    /// Check file discovery: a file matching both include and include source carries the same
    /// projects globals.
    #[test]
    fn a_file_matching_both_include_and_include_source_carries_the_same_projects_globals() {
        // Consistency requirement: whichever path admits the file, the `globals` answer carried
        // forward must be the one project that actually governs it -- not `None` just because
        // the ordinary `include`-scoping branch was skipped in favor of the in-source one.
        // `src/thing.test.ts` uses the ordinary Vitest naming convention, so it also matches
        // the tool's own default `include`.
        assert_carried_globals(
            "export default defineConfig({ test: { globals: true, includeSource: ['src/**/*.ts'] } });",
            "src/thing.test.ts",
            |globals| matches!(globals, Some(GlobalsDetection::Enabled { .. })),
        );
    }

    /// Check file discovery: in source file whose projects globals is unknown carries that
    /// uncertainty forward.
    #[test]
    fn in_source_file_whose_projects_globals_is_unknown_carries_that_uncertainty_forward() {
        // The admitting project's own `globals` could not be read. A caller must not treat this
        // the same as "no governing project" (which falls back to a repo-wide guess) -- the
        // uncertainty belongs to a specific project and has to reach the run as such.
        assert_carried_globals(
            "export default defineConfig({ test: { globals: someHelper(), includeSource: ['src/**/*.ts'] } });",
            "src/thing.ts",
            |globals| matches!(globals, Some(GlobalsDetection::Unknown { .. })),
        );
    }

    /// An `includeSource` glob that does not compile degrades the run rather than being ignored.
    ///
    /// In-source tests live inside ordinary application files, so the pattern naming them is the
    /// only thing that opens those files. If it cannot compile, they are never read -- and
    /// silence about them would be silence about tests that exist. Driven through `discover` so
    /// the real configuration path is what is under test, not a hand-built `Project`.
    #[test]
    fn an_include_source_glob_that_does_not_compile_degrades_the_run() {
        let scenario = scenario(
            &[
                (
                    "vitest.config.ts",
                    // `[` opens a character class that is never closed.
                    r#"export default defineConfig({ test: { includeSource: ["src/**/[.ts"] } });"#,
                ),
                ("src/thing.ts", ""),
            ],
            &[],
            true,
        );

        assert!(
            scenario.files.incomplete,
            "absence of a finding must stop proving anything: {:?}",
            scenario.files.errors
        );
        assert!(
            scenario.files.project_config_degraded,
            "and the cause is named as the project's configuration, not a failed traversal"
        );
        assert!(
            scenario
                .files
                .errors
                .iter()
                .any(|error| error.contains("includeSource")),
            "the failing pattern's role reaches the reader: {:?}",
            scenario.files.errors
        );
    }

    /// `exclude` is compiled separately from `includeSource`, and fails the same way.
    ///
    /// Its own arm, a dozen lines further down: a fix applied to one and not the other is this
    /// repository's most repeated defect, so both are pinned.
    #[test]
    fn an_exclude_glob_that_does_not_compile_degrades_the_run() {
        let scenario = scenario(
            &[
                (
                    "vitest.config.ts",
                    r#"export default defineConfig({ test: { includeSource: ["src/**/*.ts"], exclude: ["**/["] } });"#,
                ),
                ("src/thing.ts", ""),
            ],
            &[],
            true,
        );

        assert!(
            scenario.files.project_config_degraded,
            "{:?}",
            scenario.files.errors
        );
        assert!(
            scenario
                .files
                .errors
                .iter()
                .any(|error| error.contains("exclude")),
            "the failing pattern's role reaches the reader: {:?}",
            scenario.files.errors
        );
    }

    /// The control. Without it the assertions above would hold just as well if every run with an
    /// `includeSource` were marked incomplete.
    #[test]
    fn a_compiling_include_source_glob_degrades_nothing() {
        let scenario = scenario(
            &[
                (
                    "vitest.config.ts",
                    r#"export default defineConfig({ test: { includeSource: ["src/**/*.ts"] } });"#,
                ),
                ("src/thing.ts", ""),
            ],
            &[],
            true,
        );

        assert!(
            !scenario.files.project_config_degraded,
            "{:?}",
            scenario.files.errors
        );
        assert!(
            scenario.files.errors.is_empty(),
            "{:?}",
            scenario.files.errors
        );
    }

    /// Check file discovery: ordinary test file is not given a project globals override.
    #[test]
    fn ordinary_test_file_is_not_given_a_project_globals_override() {
        // Ordinary files must resolve exactly as they did before this field existed: discovery
        // leaves `project_globals` at `None` for anything admitted through the ordinary
        // `include`/`exclude` path, so `main.rs` keeps using `ProjectSet::globals_for` for them.
        assert_carried_globals(
            "export default defineConfig({ test: { globals: true } });",
            "src/thing.test.ts",
            |globals| globals.is_none(),
        );
    }

    // -- Copilot review #9: sharing the ambiguity decision with `governing_project` ---------

    /// Check file discovery: in source admission is ambiguous when an unresolved entry is
    /// declared after the match.
    #[test]
    fn in_source_admission_is_ambiguous_when_an_unresolved_entry_is_declared_after_the_match() {
        // `missing-project-dir/*` never resolves to anything, so it is an unresolved
        // `test.projects` entry -- declared *after* the project whose `includeSource` matches
        // `src/thing.ts`. Under "later occurrence wins" that unresolved entry could itself have
        // matched (with an unknown `globals`) and out-ranked the known match, exactly the
        // ambiguity `ProjectSet::governing_project` already fails closed on for ordinary
        // `include`/`exclude`. The bug this test guards against: the in-source fast path used to
        // resolve `in_source_candidates.iter().rfind(...)` directly and admit the known match
        // without ever asking this question.
        let scenario = in_source_admission_with_unresolved_entry(false);

        // Dropping the file because admission is ambiguous would be a worse failure than an
        // uncertain `globals` answer -- it must still be discovered.
        let file = scenario.find("src/thing.ts");
        // The uncertainty must reach the run through the same channel an unreadable `globals`
        // already uses, so `main.rs` fails closed instead of silently trusting a match that
        // might not be the real governing project.
        assert!(
            matches!(file.project_globals, Some(GlobalsDetection::Unknown { .. })),
            "a later unresolved entry could out-rank the match; expected Some(Unknown), got {:?}",
            file.project_globals
        );
    }

    /// [`in_source_admission_with_unresolved_entry`]'s two callers differ only in which
    /// `test.projects` entry comes first: the known match, or the unresolved entry that could
    /// out-rank it under "later occurrence wins". That ordering IS the test, so it is the
    /// parameter.
    fn in_source_admission_with_unresolved_entry(unresolved_entry_first: bool) -> Scenario {
        let known = "{ test: { name: 'known', globals: true, includeSource: ['src/**/*.ts'] } }";
        let unresolved = "'missing-project-dir/*'";
        let (first, second) = if unresolved_entry_first {
            (unresolved, known)
        } else {
            (known, unresolved)
        };
        let scenario = scenario(
            &[
                (
                    "vitest.config.ts",
                    &format!("export default defineConfig({{ test: {{ projects: [{first}, {second}] }} }});"),
                ),
                ("src/thing.ts", ""),
            ],
            &[],
            true,
        );
        // The scenario's precondition: `missing-project-dir/*` really is an unresolved entry,
        // so the ordering is what decides the outcome, not some other failure to resolve.
        assert!(crate::vitest_projects::resolve_projects(&scenario.root).has_unresolved_entries);
        scenario
    }

    /// Check file discovery: in source admission is not degraded when the unresolved entry is
    /// declared before the match.
    #[test]
    fn in_source_admission_is_not_degraded_when_the_unresolved_entry_is_declared_before_the_match()
    {
        // The same unresolved entry, but declared *before* the known match this time: "later
        // occurrence wins" already settled in the known project's favor regardless of what that
        // earlier, unresolved entry was, so this must not be over-corrected into `Unknown` too.
        let scenario = in_source_admission_with_unresolved_entry(true);

        let file = scenario.find("src/thing.ts");
        assert!(
            matches!(file.project_globals, Some(GlobalsDetection::Enabled { .. })),
            "the unresolved entry is declared before the match it cannot out-rank; expected \
             Some(Enabled), got {:?}",
            file.project_globals
        );
    }

    /// Check file discovery: ordinary include admitted files are unaffected by the shared
    /// ambiguity check.
    #[test]
    fn ordinary_include_admitted_files_are_unaffected_by_the_shared_ambiguity_check() {
        // No project here sets `includeSource`, so the in-source fast path never runs at all --
        // this guards against the shared `ProjectSet::ambiguous_after` machinery leaking into
        // `project_globals` for a file admitted purely through the ordinary `include`/`exclude`
        // path, which must stay `None` regardless of any unresolved `test.projects` entry.
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("shared")).unwrap();
        fs::write(
            directory.path().join("vitest.config.ts"),
            r#"export default defineConfig({
                test: {
                    projects: [
                        { test: { name: 'known', include: ['shared/**/*.test.ts'] } },
                        'missing-project-dir/*',
                    ],
                },
            });"#,
        )
        .unwrap();
        fs::write(directory.path().join("shared/a.test.ts"), "").unwrap();
        let config = load(directory.path());
        let project_set = resolve_projects(&config.root);
        assert!(project_set.has_unresolved_entries);

        let files = discover(&config, Some(&project_set)).unwrap();

        let file = find_file(&files, &config.root, "shared/a.test.ts");
        assert_eq!(file.project_globals, None);
    }
}
