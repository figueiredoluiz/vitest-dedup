//! Per-project resolution of Vitest configuration.
//!
//! [`crate::vitest_config::detect_globals`] deliberately answers
//! the `globals` question for a *single*-project repository and returns
//! [`crate::vitest_config::GlobalsDetection::Unknown`] the moment it sees `test.projects` or a
//! `vitest.workspace.*` file, because an entry can set `extends: false` and diverge from the root.
//! Measured against the pinned external corpus: all four of its large repositories hit exactly
//! that path, so the single-project detector helps least where it is needed most.
//!
//! This module resolves configuration **per project** instead of per repository: it parses
//! `test.projects` entries and legacy `vitest.workspace.*` files, resolves each project's own
//! `globals`, test discovery globs, and benchmark discovery globs, and exposes a lookup from an
//! analyzed file path to the project that governs it. That lookup is also what keeps another
//! runner's files out of the analysis: a project's resolved admission globs are the only sound
//! way to tell whether a `.cy.ts` file
//! sitting outside them should be analyzed as a Vitest test at all under `globals: true`, rather
//! than guessing from its extension. Cypress, Mocha and Jasmine ambient globals have no import to
//! anchor to, so the governing project's globs are the only signal available.
//!
//! # Never executes the config
//!
//! Same guarantee as `vitest_config.rs`, extended rather than relaxed: every value here is read by
//! masking comments/strings and walking brace/bracket structure, never by evaluating JavaScript.
//! `glob.sync(...)` with a NON-literal pattern, a spread, a ternary, or any other computed value
//! is treated as unreadable, not approximated.
//!
//! Two literal shapes are read rather than refused (B3), both narrowing to exactly what a literal
//! already carries -- neither executes anything:
//!
//! - `test.projects: glob.sync('<literal>')` (directly, or through one `const` hop), with `glob`
//!   verified imported from the `glob` package: the pattern is a string literal, and its
//!   expansion is the same budgeted filesystem walk a literal string entry already takes.
//! - `export default mergeConfig(<imported base>, <literal>)`: explicit arrays concatenate
//!   base first, scalar overrides win, and nullish overrides preserve the base. Defaults apply
//!   after merging. Measured before these two: 575 of 575 files undetermined on one large
//!   React monorepo (both shapes stack there), 3 of 199 on another; after: 0 and 0.
//!
//! # Fail closed
//!
//! A project whose `globals` or discovery globs cannot be determined resolves that field to
//! [`crate::vitest_config::GlobalsDetection::Unknown`] / [`GlobPatterns::Unknown`], never to a
//! guess -- the same reasoning `vitest_config.rs` already documents at length: a wrong `Enabled`
//! invents findings in a Jest/Playwright file, a wrong `Disabled` silently analyzes nothing. The
//! same principle extends to *membership*: [`ProjectSet::governing_project`] returns
//! [`ProjectGovernance::Unknown`], not [`ProjectGovernance::NotIncluded`], whenever an unresolved
//! `test.projects` entry might plausibly have claimed the file, or the matching project's own
//! test or benchmark include/exclude channel could not be read. This holds even when a *known*
//! project also matches: an
//! unresolved entry declared later in `test.projects` could still be the one Vitest's "later
//! occurrence wins" rule (below) actually picks, so a known match only stands as `Governed` once
//! every unresolved entry is known to have been declared *before* it.
//!
//! # Precedence when a file matches more than one project
//!
//! Vitest's own behavior here is `UNVERIFIED` (`docs/vitest-surface.md`, "Projects / workspace":
//! "UNVERIFIED whether
//! Vitest allows/dedupes a file matched by two projects"). This module picks the **last** project
//! in declaration order whose test or benchmark discovery channel is known and matches -- the same "later
//! occurrence wins" rule `vitest_config.rs` already applies to a duplicated object key
//! (`duplicate_globals_key_uses_the_last_occurrence`), chosen for consistency with that precedent
//! rather than from any confirmed Vitest semantics. If the last matching project's own
//! relevant discovery channel is itself unreadable, the file resolves to `Unknown` rather than silently
//! falling back to an earlier match.
//!
//! # Bounding the work
//!
//! Two independent bounds keep a pathological or hostile config from turning this into unbounded
//! work: [`MAX_PROJECTS`] caps the number of projects a single repository resolves into (real
//! corpora observed top out under 30, the largest at ~26), and
//! `MAX_GLOB_WALK_ENTRIES` caps the total filesystem entries visited while expanding every
//! glob-shaped `projects` string (e.g. `'packages/*'`) across the whole resolution. Either limit
//! being hit sets [`ProjectSet::truncated`] and [`ProjectSet::has_unresolved_entries`] rather than
//! silently returning a partial-but-unflagged answer.
//!
//! # Known limitations
//!
//! - **One level of `extends`-by-string.** `extends: './vitest.config.ts'` (a real observed shape)
//!   is resolved by loading that file's own `test` object once; if *that* file itself sets
//!   `extends` or `test.projects`, the chain is not followed further and the entry resolves to
//!   `Unknown` for whatever it did not set itself.
//! - **Glob negation is not resolved.** `'!packages/excluded'` / `'packages/!(business)'`
//!   (`docs/vitest-surface.md`, "Projects / workspace") need cross-entry set-difference semantics
//!   this scan does not
//!   attempt; such an entry is recorded as unresolved.
//! - **A workspace-file entry has no root `test` object to inherit from.** `extends: true`/absent
//!   on a `vitest.workspace.*` entry resolves to `Unknown` for any field the entry does not set
//!   itself, since this scan does not also load a sibling root `vitest.config.*` as a base for it.
//! - **A referenced project file's own `test.projects` is not recursed into**, but the rest of its
//!   `test` object IS read. Nothing here follows a nested `projects` list, so the file's other
//!   fields can be taken at face value without any risk of looping -- which is what a reference to
//!   it is asking for in the first place. Two real shapes depend on this: one CSS engine lists
//!   its own `vitest.config.ts` among its `projects` (Vitest reads that as "my own test config is
//!   one of the projects"), and one composable library's inline entry says
//!   `extends: './vitest.config.ts'`. Refusing such a file in full cost that CSS engine 102 of its
//!   104 files an undetermined globals mode.
//! - **Discovery glob semantics mirror `crate::config::compile_glob_set`** (`globset` with
//!   literal separators), not Vitest's own `tinyglobby`/micromatch matching -- close enough for
//!   `*`/`**`/`{a,b}` (everything the real corpora above actually use) but not a certified match
//!   for every extglob form Vitest accepts.

use crate::config::compile_glob_set;
use crate::confine::{confine, Confinement};
use crate::discovery::{
    DEFAULT_BENCHMARK_EXCLUDE_GLOBS, DEFAULT_BENCHMARK_INCLUDE_GLOBS, DEFAULT_EXCLUDE_GLOBS,
    DEFAULT_INCLUDE_GLOBS, DEFAULT_TYPECHECK_INCLUDE_GLOBS,
};
use crate::vitest_config::{
    boolean_property, display_path, find_direct_key_value_starts_precise,
    find_export_default_value_start, find_matching_delim, find_project_config, find_workspace_file,
    is_ident_char, literal_bool_at, locate_config_object,
    locate_delimited_literal_or_call_argument, locate_object_literal_or_call_argument,
    locate_test_object, mask_non_code, property_present, read_bounded, skip_trivia, ConfigObject,
    DelimitedLiteral, GlobalsDetection, TestObject,
};
use globset::GlobBuilder;
use ignore::WalkBuilder;
use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Upper bound on the number of projects a single repository resolves into.
///
/// Real corpora measured against this scan sit well under this (the largest at ~26 -- see the
/// module-level docs). It exists to keep a config referencing thousands of
/// projects, or a glob that happens to match a huge tree, from becoming unbounded work rather than
/// to accommodate any observed real repository.
pub const MAX_PROJECTS: usize = 256;

/// Upper bound on filesystem entries visited, in total across every glob-shaped `projects` string
/// entry resolved for one repository, while expanding those globs into concrete directories/files.
const MAX_GLOB_WALK_ENTRIES: usize = 20_000;

/// Depth bound applied to a glob-shaped `projects` entry whose pattern contains `**`, where the
/// pattern's own segment count can't otherwise bound the walk.
const MAX_GLOB_WALK_DEPTH: usize = 12;

/// A project's resolved `include` or `exclude` glob list.
///
/// Mirrors [`GlobalsDetection`]'s two-outcomes-plus-unknown shape for the same reason: "present and
/// read" and "could not be read" must stay distinguishable so a caller never treats an unreadable
/// `include` as an empty one (which would silently exclude every file) or as the Vitest default
/// (which could wrongly include a file the real config excludes).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GlobPatterns {
    /// The glob list as written -- a literal array of quoted string literals.
    Known(Vec<String>),
    /// Present but not a literal array of plain quoted strings (a variable, a spread, a computed
    /// entry, `glob.sync(...)`, ...), or absent with no base to inherit a value from.
    Unknown {
        /// Human-readable explanation, mirroring [`GlobalsDetection::Unknown`]'s `reason`.
        reason: String,
    },
}

/// Whether a project's `test.typecheck` configuration admits type-test files (`*.test-d.ts`)
/// into discovery at all.
///
/// Vitest runs type tests only under `typecheck.enabled` (default **off**) -- the files named by
/// `typecheck.include` are not test files at all until that switch is on, so the channel is gated
/// by an admission decision before any glob is consulted. Three outcomes, mirroring
/// [`GlobalsDetection`]'s shape for the same reason: on, off, and could-not-tell -- because
/// guessing either way on an unreadable `enabled` would either analyze files Vitest never runs or
/// silently drop a directory's whole type-test suite, and both are wrong answers this enum exists
/// to make unrepresentable.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TypecheckAdmission {
    /// `typecheck.enabled: true` (or an inherited `true`): `typecheck.include` names test files.
    Enabled,
    /// Absent, `false`, or inherited false -- Vitest's default. Type-test files are not test
    /// files under this project.
    Disabled,
    /// `typecheck.enabled` is present but not a readable boolean literal, or an inheritance this
    /// scan could not resolve. A caller must fail closed: the file MIGHT be a type test.
    Unknown {
        /// Human-readable explanation, mirroring [`GlobPatterns::Unknown`]'s `reason`.
        reason: String,
    },
}

/// One resolved Vitest project: an inline `test.projects` entry, a project referenced by a glob or
/// path, the sole implicit project of a repository with no `test.projects` at all, or an entry from
/// a legacy `vitest.workspace.*` file.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Project {
    /// The project's `test.name`, when it is a literal string. `None` when absent or non-literal.
    pub name: Option<String>,
    /// Directory discovery globs are resolved relative to: the analysis root for an inline
    /// entry (and for a repository with no `test.projects` at all), or the referenced config
    /// file's own directory for a file-based entry.
    pub base_dir: PathBuf,
    /// The project's own configuration file, when it has one distinct from the file that declared
    /// it (a referenced `vitest.config.*`/`vite.config.*`). `None` for an inline entry, whose
    /// settings live inside [`Project::defined_in`] instead.
    pub config_path: Option<PathBuf>,
    /// The file whose `test.projects` array (or `vitest.workspace.*` default export) declared this
    /// project -- always populated, unlike `config_path`.
    pub defined_in: PathBuf,
    /// Whether this project's effective configuration sets `test.globals` to `true`.
    pub globals: GlobalsDetection,
    /// This project's effective `test.include` globs.
    pub include: GlobPatterns,
    /// This project's effective `test.exclude` globs.
    pub exclude: GlobPatterns,
    /// This project's effective `test.includeSource` globs -- in-source-testing candidates
    /// (`docs/vitest-surface.md`, "In-source testing"), resolved and inherited exactly like
    /// `include`/`exclude` but
    /// defaulting to `Known(vec![])` rather than Vitest's test-file defaults: `includeSource` is
    /// `[]` unless a project opts in, and an empty list must resolve as a real, confident empty
    /// answer rather than `Unknown` -- otherwise every ordinary project without in-source testing
    /// would look uncertain about a feature it simply isn't using.
    pub include_source: GlobPatterns,
    /// This project's effective `test.benchmark.include` globs.
    pub benchmark_include: GlobPatterns,
    /// This project's effective `test.benchmark.exclude` globs.
    pub benchmark_exclude: GlobPatterns,
    /// This project's effective `test.benchmark.includeSource` globs.
    pub benchmark_include_source: GlobPatterns,
    /// Whether this project's type-test channel is open; see [`TypecheckAdmission`]. `include`
    /// and `typecheck.include` are separate lists in Vitest, and only files the ordinary channels
    /// already admit are analyzed while this is `Disabled`.
    pub typecheck_enabled: TypecheckAdmission,
    /// This project's effective `test.typecheck.include` globs. Consulted only when
    /// [`Project::typecheck_enabled`] is [`TypecheckAdmission::Enabled`].
    pub typecheck_include: GlobPatterns,
    /// This project's effective `test.typecheck.exclude` globs, independent of `test.exclude`
    /// the same way `benchmark.exclude` is.
    pub typecheck_exclude: GlobPatterns,
    /// This project's position within the `test.projects`/workspace array that declared it (0 for
    /// the implicit single project of a repository with no such array). Not exposed publicly --
    /// used only by [`ProjectSet::governing_project`] to tell whether an unresolved sibling entry
    /// was declared *after* this project and could therefore plausibly out-rank it under "later
    /// occurrence wins" (see the module docs), the same way declaration order already breaks ties
    /// between two known matching projects.
    declared_at: usize,
}

impl Project {
    /// Human-readable identifier for diagnostics: the project's `test.name` if set, otherwise the
    /// file whose `test.projects` array (or workspace default export) declared it.
    pub fn label(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| self.defined_in.display().to_string())
    }

    /// A project whose config-carried fields are copied out of `effective` -- the seven-field
    /// tail every resolution site used to repeat, so a field added to [`EffectiveTestFields`] is
    /// carried through here once rather than at each construction site that can drift.
    fn from_effective(
        name: Option<String>,
        base_dir: PathBuf,
        config_path: Option<PathBuf>,
        defined_in: &Path,
        effective: EffectiveTestFields,
        declared_at: usize,
    ) -> Self {
        Self {
            name,
            base_dir,
            config_path,
            defined_in: defined_in.to_path_buf(),
            globals: effective.globals,
            include: effective.include,
            exclude: effective.exclude,
            include_source: effective.include_source,
            benchmark_include: effective.benchmark_include,
            benchmark_exclude: effective.benchmark_exclude,
            benchmark_include_source: effective.benchmark_include_source,
            typecheck_enabled: effective.typecheck_enabled,
            typecheck_include: effective.typecheck_include,
            typecheck_exclude: effective.typecheck_exclude,
            declared_at,
        }
    }
}

/// Every project resolved for one repository, plus enough bookkeeping to answer file lookups
/// soundly even when some part of the configuration could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProjectSet {
    /// The analysis root this set was resolved for.
    pub root: PathBuf,
    /// Every project resolved, in declaration order (see the module docs for why order matters:
    /// it is the tie-break for a file matched by more than one project).
    pub projects: Vec<Project>,
    /// Human-readable notes about anything this scan could not fully resolve (a malformed entry, a
    /// glob that matched nothing, a truncation), for surfacing as warnings.
    pub diagnostics: Vec<String>,
    /// Set when at least one `test.projects`/workspace entry could not be resolved into a known
    /// [`Project`] at all (as opposed to a `Project` whose own fields are individually `Unknown`).
    /// Forces [`ProjectSet::governing_project`] to answer `Unknown` rather than `NotIncluded` for a
    /// file no *known* project claims, since the unresolved entry might have claimed it.
    pub has_unresolved_entries: bool,
    /// Set when [`MAX_PROJECTS`] or `MAX_GLOB_WALK_ENTRIES` was hit before every entry could be
    /// processed.
    pub truncated: bool,
    /// Whether a Vitest/Vite configuration file (or legacy `vitest.workspace.*`) was found at
    /// `root` at all.
    ///
    /// `projects` being empty is ambiguous on its own: it means one of three different things --
    /// (1) no config file exists, the ordinary shape of a plain library, where nothing was missed;
    /// (2) a config file exists and explicitly declares `test.projects: []`, which is also nothing
    /// missed, just an explicit empty answer; or (3) a config file exists but its project list (or
    /// the file itself) could not be read, which *is* something missed. `config_found` answers
    /// "does a config exist at all", so a caller can tell case (1) apart from (2)/(3); telling (2)
    /// and (3) apart from each other is already possible via `has_unresolved_entries`. This exists
    /// so callers do not have to re-derive "no config" from `diagnostics` prose or from
    /// `projects.is_empty() && !truncated`, which conflates (1) with (3).
    pub config_found: bool,
    /// The greatest [`Project::declared_at`] among any `test.projects`/workspace entry that could
    /// not be resolved into a known [`Project`] at all -- `None` when every entry resolved (even if
    /// [`ProjectSet::has_unresolved_entries`] is set for some other reason, e.g. a whole-array read
    /// failure with no positional entries to speak of). Lets [`ProjectSet::governing_project`] tell
    /// an unresolved entry declared *before* a known match (harmless: "later wins" already settled
    /// in the known match's favor) apart from one declared *after* it (which could still win),
    /// rather than treating every `has_unresolved_entries` config as uniformly unknowable.
    max_unresolved_position: Option<usize>,
}

/// Which project, if any, governs a given file under a [`ProjectSet`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectGovernance<'a> {
    /// Exactly one project -- the last matching one in declaration order, see the module docs --
    /// is known to include this file and not exclude it.
    Governed(&'a Project),
    /// No project is known to include this file, and no unresolved entry could plausibly have
    /// claimed it either: under the resolved configuration, Vitest would not treat this as a test
    /// file at all.
    NotIncluded,
    /// Could not be determined: either an unresolved `test.projects` entry might govern this file,
    /// or the project that would otherwise match has an unreadable `include`/`exclude`.
    Unknown {
        /// Human-readable explanation.
        reason: String,
    },
}

/// The `globals` answer for a file, from [`ProjectSet::governed_globals_for`].
///
/// `ProjectSet::globals_for` answers the same question but collapses two very different
/// non-answers into one `GlobalsDetection::Unknown`: "no project governs this file" (ordinary --
/// a caller's own repository-wide fallback is the right answer) and "a project governs this file,
/// or might, but the answer could not be determined" (a caller must fail closed rather than
/// guess, on pain of silently dropping a `globals: true` project's ambient tests). A caller that
/// needs to tell those apart -- rather than applying one fallback to both -- wants this enum
/// instead of `GlobalsDetection`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GovernedGlobals {
    /// No project governs this file, not even ambiguously (mirrors
    /// [`ProjectGovernance::NotIncluded`]). A caller's repository-wide fallback is the right
    /// answer here, not a guess.
    NotIncluded,
    /// A concrete answer: either the file's governing project sets `globals: true`, or governance
    /// itself was ambiguous but every project that could plausibly govern this file agreed on
    /// `true` (see `ProjectSet::unanimous_globals`).
    Enabled,
    /// As [`GovernedGlobals::Enabled`], for `globals: false`.
    Disabled,
    /// A project governs this file, or might, but the answer could not be determined. Unlike
    /// `NotIncluded`, a caller must fail closed here rather than fall back to a repository-wide
    /// answer.
    Unknown {
        /// Human-readable explanation.
        reason: String,
    },
}

impl ProjectSet {
    /// The set produced by a scan that read a real config and resolved its entries, however
    /// incompletely. Both `test.projects` and the standalone workspace file end in exactly this
    /// shape; keeping one constructor is what stops the two tails drifting apart, and is why
    /// threading a new field (`truncated`, 2026-09-25) touches one place rather than two.
    ///
    /// `config_found` is always `true` here: a scan that found no config returns
    /// [`unresolved_set`] instead, which is the only other way to build a set with no projects.
    fn resolved(
        root: &Path,
        projects: Vec<Project>,
        diagnostics: Vec<String>,
        has_unresolved_entries: bool,
        truncated: bool,
        max_unresolved_position: Option<usize>,
    ) -> Self {
        Self {
            root: root.to_path_buf(),
            projects,
            diagnostics,
            has_unresolved_entries,
            truncated,
            config_found: true,
            max_unresolved_position,
        }
    }

    /// Resolves which project, if any, governs `file` (expected to be an absolute path, matching
    /// every other path in this crate).
    pub fn governing_project(&self, file: &Path) -> ProjectGovernance<'_> {
        let mut winner: Option<(&Project, Option<String>)> = None;
        for project in &self.projects {
            match match_file(project, file) {
                FileMatch::No => continue,
                FileMatch::Yes => winner = Some((project, None)),
                FileMatch::Maybe(reason) => winner = Some((project, Some(reason))),
            }
        }
        match winner {
            // A known match still is not the end of it: `governing_project` picks the *last*
            // matching project in declaration order (see the module docs), and an unresolved
            // sibling entry declared after this project might itself have matched and won,
            // taking its own (unknown) `globals` with it. `ambiguous_after` is exactly that
            // check -- shared with in-source-testing candidate admission (`discovery.rs`), so
            // this decision is made in one place. Only an unresolved entry declared *before*
            // this project is provably harmless, since "later wins" already settled in this
            // project's favor regardless of what that earlier entry was.
            Some((project, None)) => match self.ambiguous_after(file, project) {
                Some(reason) => ProjectGovernance::Unknown { reason },
                None => ProjectGovernance::Governed(project),
            },
            Some((project, Some(why))) => ProjectGovernance::Unknown {
                reason: format!(
                    "{} matches project {}, whose own `include`/`exclude` could not be fully \
                     read ({why})",
                    display_path(file, &self.root),
                    project_label(project)
                ),
            },
            None if self.has_unresolved_entries => ProjectGovernance::Unknown {
                reason: format!(
                    "{} is not matched by any resolved project, but at least one `test.projects` \
                     entry could not be resolved and might govern it",
                    display_path(file, &self.root)
                ),
            },
            None => ProjectGovernance::NotIncluded,
        }
    }

    /// Whether the `test.projects`/workspace entry that resolved `file` into `project` could
    /// still be out-ranked by a later entry this scan could not resolve, under "later occurrence
    /// wins" (see the module docs' "Precedence"/"Fail closed" sections) -- and if so, why.
    ///
    /// This is the exact machinery [`ProjectSet::governing_project`] uses for its own known-match
    /// ambiguity check, factored out so [`crate::discovery`]'s `includeSource` in-source-testing
    /// fast path can ask the identical question about the project that admits a candidate file,
    /// rather than maintaining a second, parallel version of it. Copilot review #9 found exactly
    /// that drift: the ordinary `include`/`exclude` path was fixed to fail closed on an unresolved
    /// later entry, while the `includeSource` path kept resolving the admitting project directly
    /// (`in_source_candidates.iter().rfind(...)`) and never consulted this question at all.
    ///
    /// Only an unresolved entry declared *after* `project` can matter here: one declared *before*
    /// it is provably harmless, since "later wins" already settled in `project`'s favor regardless
    /// of what that earlier, unresolved entry was.
    pub(crate) fn ambiguous_after(&self, file: &Path, project: &Project) -> Option<String> {
        if self
            .max_unresolved_position
            .is_some_and(|position| position > project.declared_at)
        {
            Some(format!(
                "{} matches project {}, but a later `test.projects` entry could not be resolved \
                 and might match this file too, in which case it would govern instead",
                display_path(file, &self.root),
                project_label(project)
            ))
        } else {
            None
        }
    }

    /// Convenience wrapper around [`ProjectSet::governing_project`] for a caller that only wants
    /// the `globals` answer and already knows (or does not care) whether `file` is included at
    /// all. `NotIncluded` still yields `Unknown` here, not `Disabled`: "not a test file under this
    /// config" and "a test file with `globals: false`" are different claims, and collapsing them
    /// would let a caller that only checks `globals_for` silently analyze an excluded file.
    ///
    /// This also collapses "no project governs this file" and "a project governs this file, or
    /// might, but the answer could not be determined" into the same `Unknown`, even though a
    /// caller that fails closed on `Unknown` should treat them very differently -- the first case
    /// is ordinary and should fall back to a repository-wide answer, the second must not guess.
    /// A caller that needs to tell them apart wants [`ProjectSet::governed_globals_for`] instead.
    pub fn globals_for(&self, file: &Path) -> GlobalsDetection {
        match self.governing_project(file) {
            ProjectGovernance::Governed(project) => project.globals.clone(),
            ProjectGovernance::NotIncluded => GlobalsDetection::Unknown {
                reason: format!(
                    "{} is not matched by any resolved project's `include` globs",
                    display_path(file, &self.root)
                ),
            },
            // Ambiguity only matters when it changes the answer.
            //
            // A project whose `include`/`exclude` cannot be fully read leaves it uncertain *which*
            // project governs a file -- but if every project that could plausibly govern it agrees
            // on `globals`, the uncertainty is not load-bearing and reporting `Unknown` throws away
            // a fact we actually have.
            //
            // This is not a weakening of the fail-closed rule: when the candidates disagree, or
            // when any of them is itself `Unknown`, the answer stays `Unknown`. Measured on
            // a Vue framework core, where all five projects set `globals: true` and one `exclude` array
            // spreads `configDefaults.exclude`, this is the difference between analyzing 121 tests
            // and analyzing all 3,479.
            ProjectGovernance::Unknown { reason } => self
                .unanimous_globals()
                .unwrap_or(GlobalsDetection::Unknown { reason }),
        }
    }

    /// Like [`ProjectSet::globals_for`], but keeps "no project governs this file" separate from
    /// "a project governs this file, or might, but the answer could not be determined" -- see
    /// [`GovernedGlobals`]'s doc comment for why a caller would want that distinction.
    pub fn governed_globals_for(&self, file: &Path) -> GovernedGlobals {
        match self.governing_project(file) {
            ProjectGovernance::Governed(project) => match &project.globals {
                GlobalsDetection::Enabled { .. } => GovernedGlobals::Enabled,
                GlobalsDetection::Disabled { .. } => GovernedGlobals::Disabled,
                GlobalsDetection::Unknown { reason } => GovernedGlobals::Unknown {
                    reason: reason.clone(),
                },
            },
            ProjectGovernance::NotIncluded => GovernedGlobals::NotIncluded,
            // Same `unanimous_globals` shortcut `globals_for` applies, just reported through the
            // richer enum instead of being funneled back into `GlobalsDetection::Unknown`.
            ProjectGovernance::Unknown { reason } => match self.unanimous_globals() {
                Some(GlobalsDetection::Enabled { .. }) => GovernedGlobals::Enabled,
                Some(GlobalsDetection::Disabled { .. }) => GovernedGlobals::Disabled,
                Some(GlobalsDetection::Unknown { .. }) | None => {
                    GovernedGlobals::Unknown { reason }
                }
            },
        }
    }

    /// The `globals` value every resolved project agrees on, if they all agree and all are known.
    ///
    /// Returns `None` when there are no projects, when any project's own detection is `Unknown`,
    /// or when two projects disagree -- each of which is a case where a file's governing project
    /// genuinely determines the answer.
    fn unanimous_globals(&self) -> Option<GlobalsDetection> {
        let mut agreed: Option<bool> = None;
        for project in &self.projects {
            let enabled = match &project.globals {
                GlobalsDetection::Enabled { .. } => true,
                GlobalsDetection::Disabled { .. } => false,
                _ => return None,
            };
            match agreed {
                None => agreed = Some(enabled),
                Some(previous) if previous == enabled => {}
                Some(_) => return None,
            }
        }
        let enabled = agreed?;
        let config = self.projects.first()?.defined_in.clone();
        Some(if enabled {
            GlobalsDetection::Enabled { config }
        } else {
            GlobalsDetection::Disabled { config }
        })
    }
}

/// Return the configured project name, falling back to the path of the defining config.
fn project_label(project: &Project) -> String {
    project
        .name
        .clone()
        .unwrap_or_else(|| project.defined_in.display().to_string())
}

#[derive(Debug, PartialEq, Eq)]
enum FileMatch {
    Yes,
    No,
    Maybe(String),
}

/// Combine test, benchmark and enabled typecheck glob matches; preserve uncertainty and reject
/// paths outside the project base.
fn match_file(project: &Project, file: &Path) -> FileMatch {
    // A file outside `base_dir` entirely cannot be one of this project's files -- its `include`/
    // `exclude` globs are resolved relative to `base_dir`, so falling back to matching the file's
    // full absolute path here (as an earlier version of this function did) would let a project's
    // default globs (`**/*.test.ts`, matching almost anything) falsely claim a file that actually
    // belongs to an unrelated sibling project elsewhere in the tree.
    let Ok(relative) = file.strip_prefix(&project.base_dir) else {
        return FileMatch::No;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");

    let test = match_glob_channel(
        project,
        &relative,
        &project.include,
        &project.exclude,
        "include",
        "exclude",
    );
    let benchmark = match_glob_channel(
        project,
        &relative,
        &project.benchmark_include,
        &project.benchmark_exclude,
        "benchmark.include",
        "benchmark.exclude",
    );
    // The type-test channel, gated by its own admission switch BEFORE any glob is consulted:
    // while `typecheck.enabled` is off, `typecheck.include`'s defaults match `.test-d.ts` files
    // that Vitest would not run as tests at all, so the glob alone must never decide here.
    let typecheck = match &project.typecheck_enabled {
        TypecheckAdmission::Enabled => match_glob_channel(
            project,
            &relative,
            &project.typecheck_include,
            &project.typecheck_exclude,
            "typecheck.include",
            "typecheck.exclude",
        ),
        TypecheckAdmission::Disabled => FileMatch::No,
        // Not `No`: the file might be admitted through a gate this scan could not read, which is
        // exactly what `Maybe` exists to say. This makes every file under the project ambiguous,
        // not only `.test-d.ts` ones -- fail-closed, and only reachable when a config writes a
        // non-literal `enabled` or inherits one unresolvably.
        TypecheckAdmission::Unknown { reason } => FileMatch::Maybe(reason.clone()),
    };
    match (test, benchmark, typecheck) {
        (FileMatch::Yes, _, _) | (_, FileMatch::Yes, _) | (_, _, FileMatch::Yes) => FileMatch::Yes,
        (FileMatch::No, FileMatch::No, FileMatch::No) => FileMatch::No,
        (FileMatch::Maybe(reason), FileMatch::No, FileMatch::No)
        | (FileMatch::No, FileMatch::Maybe(reason), FileMatch::No)
        | (FileMatch::No, FileMatch::No, FileMatch::Maybe(reason)) => FileMatch::Maybe(reason),
        (FileMatch::Maybe(a), FileMatch::Maybe(b), FileMatch::No)
        | (FileMatch::Maybe(a), FileMatch::No, FileMatch::Maybe(b))
        | (FileMatch::No, FileMatch::Maybe(a), FileMatch::Maybe(b)) => {
            FileMatch::Maybe(format!("{a}; {b}"))
        }
        (FileMatch::Maybe(a), FileMatch::Maybe(b), FileMatch::Maybe(c)) => {
            FileMatch::Maybe(format!("{a}; {b}; {c}"))
        }
    }
}

/// Match one include/exclude channel against a relative path, returning uncertainty for unknown
/// or invalid patterns.
fn match_glob_channel(
    project: &Project,
    relative: &str,
    include: &GlobPatterns,
    exclude: &GlobPatterns,
    include_name: &str,
    exclude_name: &str,
) -> FileMatch {
    let included = match include {
        GlobPatterns::Known(patterns) => match compile_glob_set(patterns, include_name) {
            Ok(set) => set.is_match(relative),
            Err(_) => {
                return FileMatch::Maybe(format!(
                    "project {}'s `{include_name}` patterns failed to compile",
                    project_label(project)
                ))
            }
        },
        GlobPatterns::Unknown { reason } => return FileMatch::Maybe(reason.clone()),
    };
    if !included {
        return FileMatch::No;
    }
    let excluded = match exclude {
        GlobPatterns::Known(patterns) => match compile_glob_set(patterns, exclude_name) {
            Ok(set) => set.is_match(relative),
            Err(_) => {
                return FileMatch::Maybe(format!(
                    "project {}'s `{exclude_name}` patterns failed to compile",
                    project_label(project)
                ))
            }
        },
        GlobPatterns::Unknown { reason } => return FileMatch::Maybe(reason.clone()),
    };
    if excluded {
        FileMatch::No
    } else {
        FileMatch::Yes
    }
}

/// Resolves every project for the Vitest repository rooted at `root`.
///
/// See the module documentation for precedence, bounds, and known limitations. Reads at most
/// [`MAX_PROJECTS`] projects' worth of configuration files and never executes any of them.
pub fn resolve_projects(root: &Path) -> ProjectSet {
    let mut budget = Budget::default();

    if let Some(workspace) = find_workspace_file(root) {
        return resolve_from_workspace_file(root, &workspace, &mut budget);
    }

    let Some(config_path) = find_project_config(root) else {
        return unresolved_set(
            root,
            vec![format!(
                "no vitest.config.* or vite.config.* found at {}",
                root.display()
            )],
            false,
        );
    };

    let (masked, original) = match read_masked_source(&config_path, root) {
        Ok(views) => views,
        Err(reason) => return unresolved_set(root, vec![reason], true),
    };

    let config_object = match locate_config_config_object(&masked, &original, &config_path) {
        ConfigObject::Found(open, close) => (open, close),
        ConfigObject::NoExport => {
            return unresolved_set(
                root,
                vec![format!(
                    "{} has no `export default` this scan recognizes",
                    display_path(&config_path, root)
                )],
                true,
            )
        }
        ConfigObject::NonLiteral => {
            return unresolved_set(
                root,
                vec![format!(
                    "{}'s default export is not an object literal this scan can read",
                    display_path(&config_path, root)
                )],
                true,
            )
        }
        ConfigObject::Malformed => {
            return unresolved_set(
                root,
                vec![format!(
                    "{}'s default export starts an object literal that never closes",
                    display_path(&config_path, root)
                )],
                true,
            )
        }
    };
    let is_merge = find_export_default_value_start(&masked).is_some_and(|value_start| {
        merge_config_overrides_literal(&masked, &original, value_start).is_some()
    });
    let merged = is_merge
        .then(|| resolve_standalone_config_within(root, &config_path))
        .flatten();
    let base_refusal = merged.as_ref().and_then(|fields| fields.refusal.clone());
    let merged_fields = merged.map(|fields| (fields.name, fields.effective));
    if is_merge && merged_fields.is_none() {
        return unresolved_set(
            root,
            vec![format!(
                "{}'s merged default export could not be resolved",
                display_path(&config_path, root)
            )],
            true,
        );
    }

    let mut set = match locate_test_object(&masked, config_object.0 + 1..config_object.1) {
        TestObject::Absent => match merged_fields {
            Some((name, effective)) => {
                single_project_set_from_effective(root, &config_path, name, effective)
            }
            None => single_project_set(root, &config_path, &masked, &original, None),
        },
        TestObject::NonLiteral | TestObject::Malformed => unresolved_set(
            root,
            vec![format!(
                "{} sets `test` to something this scan cannot read",
                display_path(&config_path, root)
            )],
            true,
        ),
        TestObject::Found(open, close) => {
            let body = open + 1..close;
            if property_present(&masked, body.clone(), "projects") {
                resolve_projects_array(
                    root,
                    &config_path,
                    &masked,
                    &original,
                    body,
                    merged_fields.as_ref().map(|(_, effective)| effective),
                    &mut budget,
                )
            } else if let Some((name, effective)) = merged_fields {
                single_project_set_from_effective(root, &config_path, name, effective)
            } else {
                single_project_set(root, &config_path, &masked, &original, Some(body))
            }
        }
    };
    // Recorded here rather than left in the merged fields: an inline project that sets
    // everything itself, or `extends: false`, discards them and the refusal would vanish.
    if let Some(reason) = base_refusal {
        set.diagnostics.push(reason);
        set.has_unresolved_entries = true;
    }
    set
}

/// Reads `path` and returns `(masked, original)` character vectors for the scanners, or the one
/// diagnostic explaining why the file cannot be scanned at all.
///
/// Every entry point that scans a config file needs both views and has to refuse the same two
/// ways: the file could not be read within the size bound ([`read_bounded`]), or [`mask_non_code`]
/// found an unterminated string, template literal, or comment -- in which case every bracket
/// position in the file is suspect and no depth-tracking scan over it means anything. Stating that
/// pair once is what keeps a new entry point from accepting a file the existing ones reject.
fn read_masked_source(path: &Path, root: &Path) -> Result<(Vec<char>, Vec<char>), String> {
    // The analysis root's own configuration file can be a symlink out of the tree too.
    if matches!(confine(root, path), Confinement::Outside) {
        return Err(format!(
            "{} resolves outside the analysis root and was not read",
            display_path(path, root)
        ));
    }
    let source = read_bounded(path)?;
    let Some(masked) = mask_non_code(&source) else {
        return Err(format!(
            "{} contains an unterminated string, template literal, or comment",
            display_path(path, root)
        ));
    };
    Ok((masked, source.chars().collect()))
}

/// Build an empty project set with diagnostics, marking unresolved entries only when a config
/// was found.
fn unresolved_set(root: &Path, diagnostics: Vec<String>, config_found: bool) -> ProjectSet {
    ProjectSet {
        root: root.to_path_buf(),
        projects: Vec::new(),
        diagnostics,
        // `has_unresolved_entries` only means something when a config genuinely declared a
        // `test.projects`/workspace entry this scan could not resolve -- that is exactly the
        // `config_found: true` callers of this function (an unreadable config, a malformed `test`
        // object, an unresolvable `projects` value). The one `config_found: false` caller is a
        // repository with no Vitest/Vite config at all: the ordinary case for a plain library,
        // where there is no entry to have missed. Coupling this to `config_found` keeps that
        // caller from forcing every file's `ProjectSet::governing_project` to `Unknown` instead of
        // `NotIncluded` -- which would make `governed_globals_for` fail closed on every file in
        // every config-less repository, exactly the false alarm a caller must not produce.
        has_unresolved_entries: config_found,
        truncated: false,
        config_found,
        max_unresolved_position: None,
    }
}

/// Return owned copies of the built-in test inclusion globs.
fn default_include() -> GlobPatterns {
    GlobPatterns::Known(
        DEFAULT_INCLUDE_GLOBS
            .iter()
            .map(|pattern| (*pattern).to_owned())
            .collect(),
    )
}

/// Return owned copies of the built-in test exclusion globs.
fn default_exclude() -> GlobPatterns {
    GlobPatterns::Known(
        DEFAULT_EXCLUDE_GLOBS
            .iter()
            .map(|pattern| (*pattern).to_owned())
            .collect(),
    )
}

/// Vitest's own default for `test.includeSource`: an empty list, i.e. in-source testing is
/// entirely opt-in (`docs/vitest-surface.md`, "In-source testing"). Unlike
/// [`default_include`]/[`default_exclude`],
/// there is no built-in constant to mirror here -- the empty `Vec` *is* the default.
fn default_include_source() -> GlobPatterns {
    GlobPatterns::Known(Vec::new())
}

/// Return owned copies of the built-in benchmark inclusion globs.
fn default_benchmark_include() -> GlobPatterns {
    GlobPatterns::Known(
        DEFAULT_BENCHMARK_INCLUDE_GLOBS
            .iter()
            .map(|pattern| (*pattern).to_owned())
            .collect(),
    )
}

/// Return owned copies of the built-in benchmark exclusion globs.
fn default_benchmark_exclude() -> GlobPatterns {
    GlobPatterns::Known(
        DEFAULT_BENCHMARK_EXCLUDE_GLOBS
            .iter()
            .map(|pattern| (*pattern).to_owned())
            .collect(),
    )
}

/// Return an empty benchmark in-source inclusion list; in-source admission is opt-in.
fn default_benchmark_include_source() -> GlobPatterns {
    GlobPatterns::Known(Vec::new())
}

/// Vitest's own default for `test.typecheck.include`. Only consulted while
/// `typecheck.enabled` is on -- see [`TypecheckAdmission`].
fn default_typecheck_include() -> GlobPatterns {
    GlobPatterns::Known(
        DEFAULT_TYPECHECK_INCLUDE_GLOBS
            .iter()
            .map(|pattern| (*pattern).to_owned())
            .collect(),
    )
}

/// Builds the single implicit project for a repository whose config has no `test.projects` array.
fn single_project_set(
    root: &Path,
    config_path: &Path,
    masked: &[char],
    original: &[char],
    body: Option<Range<usize>>,
) -> ProjectSet {
    let fields = match body {
        Some(body) => RawTestFields::read(masked, original, body),
        None => RawTestFields::empty(),
    };
    let effective = resolve_effective(&fields, &ExtendsMode::NoInherit, None, config_path);
    let project = Project {
        name: fields.name,
        base_dir: root.to_path_buf(),
        config_path: Some(config_path.to_path_buf()),
        defined_in: config_path.to_path_buf(),
        globals: effective.globals,
        include: effective.include,
        exclude: effective.exclude,
        include_source: effective.include_source,
        benchmark_include: effective.benchmark_include,
        benchmark_exclude: effective.benchmark_exclude,
        benchmark_include_source: effective.benchmark_include_source,
        typecheck_enabled: effective.typecheck_enabled,
        typecheck_include: effective.typecheck_include,
        typecheck_exclude: effective.typecheck_exclude,
        declared_at: 0,
    };
    ProjectSet {
        root: root.to_path_buf(),
        projects: vec![project],
        diagnostics: Vec::new(),
        has_unresolved_entries: false,
        truncated: false,
        config_found: true,
        max_unresolved_position: None,
    }
}

/// Build a resolved single-project set from the root config and its effective test fields.
fn single_project_set_from_effective(
    root: &Path,
    config_path: &Path,
    name: Option<String>,
    effective: EffectiveTestFields,
) -> ProjectSet {
    ProjectSet::resolved(
        root,
        vec![Project::from_effective(
            name,
            root.to_path_buf(),
            Some(config_path.to_path_buf()),
            config_path,
            effective,
            0,
        )],
        Vec::new(),
        false,
        false,
        None,
    )
}

/// Fields read directly from one `test: { ... }` (or inline-project) object, *before* inheritance.
/// `None` means the key is absent -- fall through to a base or a Vitest default; `Some` means the
/// key is present, whether or not its value could actually be read.
struct RawTestFields {
    globals: Option<Option<bool>>,
    include: Option<GlobPatterns>,
    exclude: Option<GlobPatterns>,
    include_source: Option<GlobPatterns>,
    benchmark_include: Option<GlobPatterns>,
    benchmark_exclude: Option<GlobPatterns>,
    benchmark_include_source: Option<GlobPatterns>,
    /// `typecheck.enabled` -- `None` absent, `Some(None)` present but unreadable, the same
    /// two-layer shape `globals` uses.
    typecheck_enabled: Option<Option<bool>>,
    typecheck_include: Option<GlobPatterns>,
    typecheck_exclude: Option<GlobPatterns>,
    name: Option<String>,
    /// The entry's own `root:` or `dir:`, verbatim, before it is joined to the analysis root.
    ///
    /// Two spellings, one effect *for this scan*: both say "this project's files live under
    /// here", which is exactly what `base_dir` means. Vitest distinguishes them (a project
    /// `root` relocates the whole project; `test.dir` only moves where test files are searched),
    /// and that distinction is invisible to an analyzer that reads no plugins, resolves no
    /// aliases and runs no code -- every use `base_dir` is put to here is include/exclude
    /// resolution, where the two agree. Recorded rather than silently conflated.
    ///
    /// `docs/vitest-surface.md` documents neither option, so the shape honoured here is the one
    /// four separate specifications pin -- Rust rows R18/R19 and the `duplicate-test-projects-root`
    /// / `duplicate-test-projects-dir` recall cases -- namely both keys nested inside the entry's
    /// `test` object. An entry-level `root:` (a Vite option, a sibling of `test:` rather than a
    /// member) is NOT read, because nothing measured says what the scanned repositories write,
    /// and inventing a second spelling would widen what a project claims on a guess.
    base: Option<String>,
}

impl RawTestFields {
    /// Initialize raw test fields with every property absent, before inheritance or defaults
    /// apply.
    fn empty() -> Self {
        Self {
            globals: None,
            include: None,
            exclude: None,
            include_source: None,
            benchmark_include: None,
            benchmark_exclude: None,
            benchmark_include_source: None,
            typecheck_enabled: None,
            typecheck_include: None,
            typecheck_exclude: None,
            name: None,
            base: None,
        }
    }

    /// Read the modeled scalar and glob fields from the supplied test-object range without
    /// executing code.
    fn read(masked: &[char], original: &[char], range: Range<usize>) -> Self {
        let (benchmark_include, benchmark_exclude, benchmark_include_source) =
            read_benchmark_fields(masked, original, range.clone());
        let (typecheck_enabled, typecheck_include, typecheck_exclude) =
            read_typecheck_fields(masked, original, range.clone());
        Self {
            globals: boolean_property(masked, range.clone(), "globals"),
            include: read_string_array_property(masked, original, range.clone(), "include"),
            exclude: read_string_array_property(masked, original, range.clone(), "exclude"),
            include_source: read_string_array_property(
                masked,
                original,
                range.clone(),
                "includeSource",
            ),
            benchmark_include,
            benchmark_exclude,
            benchmark_include_source,
            typecheck_enabled,
            typecheck_include,
            typecheck_exclude,
            name: read_string_property(masked, original, range.clone(), "name"),
            // `root` first: where an entry writes both, the project root is the stronger claim,
            // and `dir` is documented as relative to it (so with both present and equal -- the
            // only shape this scan can resolve without composing them -- either answer is the
            // same one).
            base: read_string_property(masked, original, range.clone(), "root")
                .or_else(|| read_string_property(masked, original, range, "dir")),
        }
    }
}

/// Read the nested benchmark inclusion, exclusion and in-source fields, preserving absent or
/// unknown values.
fn read_benchmark_fields(
    masked: &[char],
    original: &[char],
    range: Range<usize>,
) -> (
    Option<GlobPatterns>,
    Option<GlobPatterns>,
    Option<GlobPatterns>,
) {
    let Some(&value_start) =
        find_direct_key_value_starts_precise(masked, original, range, "benchmark").last()
    else {
        return (None, None, None);
    };
    let body = match locate_object_literal_or_call_argument(masked, value_start) {
        ConfigObject::Found(open, close) => open + 1..close,
        ConfigObject::Malformed => {
            let unknown = || GlobPatterns::Unknown {
                reason: "`benchmark`'s object literal never reaches a matching `}`".to_owned(),
            };
            return (Some(unknown()), Some(unknown()), Some(unknown()));
        }
        ConfigObject::NonLiteral | ConfigObject::NoExport => {
            let unknown = || GlobPatterns::Unknown {
                reason: "`benchmark` is set to something other than an object literal".to_owned(),
            };
            return (Some(unknown()), Some(unknown()), Some(unknown()));
        }
    };
    (
        read_benchmark_array_property(masked, original, body.clone(), "include"),
        read_benchmark_array_property(masked, original, body.clone(), "exclude"),
        read_benchmark_array_property(masked, original, body, "includeSource"),
    )
}

/// Read a benchmark glob-array property with diagnostics naming its nested config path.
fn read_benchmark_array_property(
    masked: &[char],
    original: &[char],
    range: Range<usize>,
    key: &str,
) -> Option<GlobPatterns> {
    read_nested_array_property(masked, original, range, key, "benchmark")
}

/// Reads `test.typecheck`'s three keys: `enabled` (a boolean), `include`, and `exclude`.
///
/// Same shape as [`read_benchmark_fields`] over the same kind of nested object, with one more
/// key that is not a glob list: `enabled` decides whether the other two name test files at all,
/// so it is read through the same [`boolean_property`] `globals` uses and carried with the same
/// two-layer `Option` shape ("absent" and "present but unreadable" must stay distinguishable).
/// A `typecheck` object that is not a readable object literal makes all three unreadable
/// together, because a channel whose gate cannot be read is not a channel this scan may open or
/// close on a guess.
fn read_typecheck_fields(
    masked: &[char],
    original: &[char],
    range: Range<usize>,
) -> (
    Option<Option<bool>>,
    Option<GlobPatterns>,
    Option<GlobPatterns>,
) {
    let Some(&value_start) =
        find_direct_key_value_starts_precise(masked, original, range, "typecheck").last()
    else {
        return (None, None, None);
    };
    let body = match locate_object_literal_or_call_argument(masked, value_start) {
        ConfigObject::Found(open, close) => open + 1..close,
        ConfigObject::Malformed | ConfigObject::NonLiteral | ConfigObject::NoExport => {
            let unknown = || GlobPatterns::Unknown {
                reason: "`typecheck` is set to something this scan cannot read as an object \
                         literal"
                    .to_owned(),
            };
            return (Some(None), Some(unknown()), Some(unknown()));
        }
    };
    (
        boolean_property(masked, body.clone(), "enabled"),
        read_typecheck_array_property(masked, original, body.clone(), "include"),
        read_typecheck_array_property(masked, original, body, "exclude"),
    )
}

/// Read a typecheck glob-array property with diagnostics naming its nested config path.
fn read_typecheck_array_property(
    masked: &[char],
    original: &[char],
    range: Range<usize>,
    key: &str,
) -> Option<GlobPatterns> {
    read_nested_array_property(masked, original, range, key, "typecheck")
}

/// [`read_benchmark_array_property`]/[`read_typecheck_array_property`]'s shared body: a key read
/// inside a nested config object, with diagnostics naming the nested path. One function because
/// the two readers are the same reader -- `benchmark.include` and `typecheck.include` differ in
/// nothing but the prefix their diagnostics carry, and the duplication gate holds production at
/// 0.00%.
fn read_nested_array_property(
    masked: &[char],
    original: &[char],
    range: Range<usize>,
    key: &str,
    parent: &str,
) -> Option<GlobPatterns> {
    let diagnostic_key = format!("{parent}.{key}");
    read_string_array_property_with_diagnostic_key(masked, original, range, key, &diagnostic_key)
}

/// A project's globals/include/exclude after inheritance has been applied -- what
/// [`Project::globals`]/[`Project::include`]/[`Project::exclude`] are built from directly.
#[derive(Clone)]
struct EffectiveTestFields {
    globals: GlobalsDetection,
    include: GlobPatterns,
    exclude: GlobPatterns,
    include_source: GlobPatterns,
    benchmark_include: GlobPatterns,
    benchmark_exclude: GlobPatterns,
    benchmark_include_source: GlobPatterns,
    typecheck_enabled: TypecheckAdmission,
    typecheck_include: GlobPatterns,
    typecheck_exclude: GlobPatterns,
}

/// How a project entry relates to a base configuration it might inherit unset fields from.
enum ExtendsMode {
    /// `extends: true`, or absent (the v5 default for an inline `test.projects` entry).
    Inherit,
    /// `extends: false`: Vitest's own defaults apply to anything this entry does not set itself.
    NoInherit,
    /// `extends` is present but not a boolean or a resolvable string path -- or is a string this
    /// scan could not load and parse. Any field the entry does not set itself is `Unknown`.
    Unresolvable(String),
}

/// Combines `own` (this project's own direct fields) with `mode`/`base` (its inheritance) into the
/// effective values a [`Project`] carries. `config_path` attributes a literal `globals`/default
/// value to the file that set it, matching how [`GlobalsDetection`] is used everywhere else.
fn resolve_effective(
    own: &RawTestFields,
    mode: &ExtendsMode,
    base: Option<&EffectiveTestFields>,
    config_path: &Path,
) -> EffectiveTestFields {
    let globals = match own.globals {
        Some(Some(true)) => GlobalsDetection::Enabled {
            config: config_path.to_path_buf(),
        },
        Some(Some(false)) => GlobalsDetection::Disabled {
            config: config_path.to_path_buf(),
        },
        Some(None) => GlobalsDetection::Unknown {
            reason: format!(
                "{} sets `test.globals` to a non-literal expression",
                config_path.display()
            ),
        },
        None => match mode {
            ExtendsMode::NoInherit => GlobalsDetection::Disabled {
                config: config_path.to_path_buf(),
            },
            ExtendsMode::Inherit => {
                base.map(|base| base.globals.clone())
                    .unwrap_or_else(|| GlobalsDetection::Unknown {
                        reason: format!(
                            "{} does not set `test.globals` and there is no base configuration to \
                         inherit it from",
                            config_path.display()
                        ),
                    })
            }
            ExtendsMode::Unresolvable(reason) => GlobalsDetection::Unknown {
                reason: reason.clone(),
            },
        },
    };

    let include = resolve_glob_field(
        &own.include,
        mode,
        base.map(|base| &base.include),
        DEFAULT_INCLUDE_GLOBS,
        config_path,
        "include",
    );
    let exclude = resolve_glob_field(
        &own.exclude,
        mode,
        base.map(|base| &base.exclude),
        DEFAULT_EXCLUDE_GLOBS,
        config_path,
        "exclude",
    );
    // Vitest's own default is `[]` -- no built-in constant to pass here, unlike `include`/
    // `exclude`, because the empty list *is* the default (see `default_include_source`).
    let include_source = resolve_glob_field(
        &own.include_source,
        mode,
        base.map(|base| &base.include_source),
        &[],
        config_path,
        "includeSource",
    );
    let benchmark_include = resolve_glob_field(
        &own.benchmark_include,
        mode,
        base.map(|base| &base.benchmark_include),
        DEFAULT_BENCHMARK_INCLUDE_GLOBS,
        config_path,
        "benchmark.include",
    );
    let benchmark_exclude = resolve_glob_field(
        &own.benchmark_exclude,
        mode,
        base.map(|base| &base.benchmark_exclude),
        DEFAULT_BENCHMARK_EXCLUDE_GLOBS,
        config_path,
        "benchmark.exclude",
    );
    let benchmark_include_source = resolve_glob_field(
        &own.benchmark_include_source,
        mode,
        base.map(|base| &base.benchmark_include_source),
        &[],
        config_path,
        "benchmark.includeSource",
    );

    // `typecheck.enabled` resolves with the boolean's own tri-state, not `resolve_glob_field`:
    // it is the channel's gate, not a pattern list, so "could not read the gate" is an admission
    // of Unknown rather than a glob-set Unknown that only fires when a file actually matches.
    let typecheck_enabled = match own.typecheck_enabled {
        Some(Some(true)) => TypecheckAdmission::Enabled,
        Some(Some(false)) => TypecheckAdmission::Disabled,
        Some(None) => TypecheckAdmission::Unknown {
            reason: format!(
                "{} sets `test.typecheck.enabled` to a non-literal expression",
                config_path.display()
            ),
        },
        None => match mode {
            // Vitest's default is off, and an inheritance that resolves carries the base's own
            // answer; the two `mode` arms that fall back to a definite `Disabled` (rather than
            // Unknown) are the ones whose defaults Vitest states outright.
            ExtendsMode::NoInherit => TypecheckAdmission::Disabled,
            ExtendsMode::Inherit => match base {
                Some(base) => base.typecheck_enabled.clone(),
                None => TypecheckAdmission::Unknown {
                    reason: format!(
                        "{} does not set `test.typecheck.enabled` and there is no base \
                         configuration to inherit it from",
                        config_path.display()
                    ),
                },
            },
            ExtendsMode::Unresolvable(reason) => TypecheckAdmission::Unknown {
                reason: reason.clone(),
            },
        },
    };
    let typecheck_include = resolve_glob_field(
        &own.typecheck_include,
        mode,
        base.map(|base| &base.typecheck_include),
        DEFAULT_TYPECHECK_INCLUDE_GLOBS,
        config_path,
        "typecheck.include",
    );
    let typecheck_exclude = resolve_glob_field(
        &own.typecheck_exclude,
        mode,
        base.map(|base| &base.typecheck_exclude),
        DEFAULT_EXCLUDE_GLOBS,
        config_path,
        "typecheck.exclude",
    );

    EffectiveTestFields {
        globals,
        include,
        exclude,
        include_source,
        benchmark_include,
        benchmark_exclude,
        benchmark_include_source,
        typecheck_enabled,
        typecheck_include,
        typecheck_exclude,
    }
}

/// Prefer explicit globs; otherwise select defaults, inherited values or an unknown state
/// according to extends mode.
#[allow(clippy::too_many_arguments)]
fn resolve_glob_field(
    own: &Option<GlobPatterns>,
    mode: &ExtendsMode,
    base: Option<&GlobPatterns>,
    defaults: &[&str],
    config_path: &Path,
    field: &str,
) -> GlobPatterns {
    match own {
        Some(patterns) => patterns.clone(),
        None => match mode {
            ExtendsMode::NoInherit => GlobPatterns::Known(
                defaults
                    .iter()
                    .map(|pattern| (*pattern).to_owned())
                    .collect(),
            ),
            ExtendsMode::Inherit => base.cloned().unwrap_or_else(|| GlobPatterns::Unknown {
                reason: format!(
                    "{} does not set `test.{field}` and there is no base configuration to \
                     inherit it from",
                    config_path.display()
                ),
            }),
            ExtendsMode::Unresolvable(reason) => GlobPatterns::Unknown {
                reason: reason.clone(),
            },
        },
    }
}

/// Finds the textual extent of a property value starting at `start`: everything up to the next
/// depth-zero comma, or `limit`. Used to bound a scalar value (`extends`, `name`) the same way
/// [`split_top_level`] bounds each element of an array.
fn value_extent(masked: &[char], start: usize, limit: usize) -> Range<usize> {
    let mut depth: i32 = 0;
    let mut i = start;
    while i < limit {
        match masked[i] {
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => depth -= 1,
            ',' if depth == 0 => break,
            _ => {}
        }
        i += 1;
    }
    start..i
}

/// Splits `range` on depth-zero commas, dropping empty elements (an empty array, or a trailing
/// comma) -- "empty" meaning [`skip_trivia`] over `original` reaches the element's own end, so a
/// plain string-literal element is never mistaken for one. Depth tracks all of `{[(`/`}])`
/// together, which is safe once [`crate::vitest_config::mask_non_code`] has blanked every bracket
/// that appeared inside a string or comment -- every bracket left in `masked` is real code.
fn split_top_level(original: &[char], masked: &[char], range: Range<usize>) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    let mut depth: i32 = 0;
    let mut start = range.start;
    let mut i = range.start;
    while i < range.end {
        match masked[i] {
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => depth -= 1,
            ',' if depth == 0 => {
                result.push(start..i);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < range.end {
        result.push(start..range.end);
    }
    result
        .into_iter()
        .filter(|element| skip_trivia(original, element.start, element.end) < element.end)
        .collect()
}

/// Reads a plain single- or double-quoted string literal occupying `range` (after skipping leading
/// trivia), returning `None` for anything else -- a variable, a template literal, string
/// concatenation, trailing content (other than trivia) after the closing quote, or an escape
/// sequence this function does not decode (see below).
///
/// Decodes the standard JavaScript escapes this scan actually expects to see in a glob/name string:
/// `\n`, `\t`, `\r`, `\0`, `\\`, `\'`, `\"`, `\xNN`, `\uXXXX`, and `\u{...}`. Any other escape --
/// `\v`, `\b`, an octal escape, a malformed/unterminated `\u`/`\x`, or a `\u{...}` codepoint that is
/// not a valid `char` (e.g. a lone surrogate half or a value past `U+10FFFF`) -- makes the whole
/// literal unreadable (`None`), which every caller already treats as "not a plain string literal"
/// and reports as unresolved. This is deliberate: an earlier version of this function reduced any
/// unrecognized `\x` to the bare `x` (so `"tests/tests/**/*.ts"` silently became
/// `tests/u0074ests/**/*.ts`, a *different, valid-looking* glob), which is exactly the
/// wrong-but-plausible failure this module's fail-closed rule exists to prevent -- refusing to
/// decode is safer than guessing.
///
/// Reads from `original`, not `masked`: [`crate::vitest_config::mask_non_code`] blanks a string's
/// *contents* (quotes included) to keep keyword scans from matching text inside it, which is
/// exactly the information this function needs back. Trivia is skipped with [`skip_trivia`] rather
/// than a masked-whitespace scan for the same reason `find_direct_key_value_starts_precise` exists:
/// a masked scan cannot tell "still leading trivia" from "already inside this very string."
/// The identifier of a `...name` spread element, or `None` for anything else.
fn read_spread_identifier(original: &[char], range: Range<usize>) -> Option<String> {
    let mut i = skip_trivia(original, range.start, range.end);
    for expected in ['.', '.', '.'] {
        if i >= range.end || original[i] != expected {
            return None;
        }
        i += 1;
    }
    i = skip_trivia(original, i, range.end);
    let start = i;
    while i < range.end && is_ident_char(original[i]) {
        i += 1;
    }
    if i == start {
        return None;
    }
    let name: String = original[start..i].iter().collect();
    // Only a bare `...name`; `...a.b` or `...f()` is not a constant this can substitute.
    (skip_trivia(original, i, range.end) >= range.end).then_some(name)
}

/// Decode a complete supported single- or double-quoted string in the range; return None for
/// unsupported or incomplete forms.
fn read_string_literal(original: &[char], range: Range<usize>) -> Option<String> {
    let mut i = skip_trivia(original, range.start, range.end);
    if i >= range.end {
        return None;
    }
    let quote = original[i];
    if quote != '\'' && quote != '"' {
        return None;
    }
    i += 1;
    let mut content = String::new();
    let mut closed = false;
    while i < range.end {
        let c = original[i];
        if c == '\\' && i + 1 < range.end {
            match original[i + 1] {
                'n' => {
                    content.push('\n');
                    i += 2;
                }
                't' => {
                    content.push('\t');
                    i += 2;
                }
                'r' => {
                    content.push('\r');
                    i += 2;
                }
                // `\0` is NUL only when no digit follows it. `\01` is a *legacy octal* escape
                // (JavaScript reads it as U+0001), so decoding the `0` alone here would yield
                // NUL followed by a literal `1` -- a wrong-but-plausible value of exactly the
                // kind this function refuses to produce. Octal is not decoded; it is refused.
                '0' if !matches!(original.get(i + 2), Some(next) if next.is_ascii_digit()) => {
                    content.push('\0');
                    i += 2;
                }
                '\\' => {
                    content.push('\\');
                    i += 2;
                }
                '\'' => {
                    content.push('\'');
                    i += 2;
                }
                '"' => {
                    content.push('"');
                    i += 2;
                }
                'x' => {
                    let ch = decode_hex_escape(original, i + 2, range.end, 2)?;
                    content.push(ch);
                    i += 4;
                }
                'u' if original.get(i + 2) == Some(&'{') => {
                    let digits_start = i + 3;
                    let mut j = digits_start;
                    while j < range.end && original[j] != '}' {
                        j += 1;
                    }
                    if j >= range.end || j == digits_start {
                        // Unterminated, or `\u{}` with no digits at all.
                        return None;
                    }
                    let ch =
                        decode_hex_escape(original, digits_start, range.end, j - digits_start)?;
                    content.push(ch);
                    i = j + 1;
                }
                'u' => {
                    let ch = decode_hex_escape(original, i + 2, range.end, 4)?;
                    content.push(ch);
                    i += 6;
                }
                // Any other escape (`\v`, `\b`, an octal digit, ...) is not decoded -- reject the
                // whole literal rather than silently dropping the backslash. See the function docs.
                _ => return None,
            }
            continue;
        }
        if c == quote {
            i += 1;
            closed = true;
            break;
        }
        content.push(c);
        i += 1;
    }
    if !closed {
        return None;
    }
    if skip_trivia(original, i, range.end) != range.end {
        return None;
    }
    Some(content)
}

/// Decodes exactly `digit_count` hex digits starting at `start` (bounded by `limit`) into the
/// `char` they name, for the `\xNN`/`\uXXXX`/`\u{...}` cases of [`read_string_literal`]. `None` on
/// too few digits remaining, a non-hex digit, or a codepoint that is not a valid Rust `char` (a
/// lone UTF-16 surrogate half, or a value past `U+10FFFF`) -- all of which mean "cannot be decoded
/// soundly," never "decode as something close enough."
fn decode_hex_escape(
    original: &[char],
    start: usize,
    limit: usize,
    digit_count: usize,
) -> Option<char> {
    if start + digit_count > limit {
        return None;
    }
    let hex: String = original[start..start + digit_count].iter().collect();
    let code = u32::from_str_radix(&hex, 16).ok()?;
    char::from_u32(code)
}

/// Reads `key`'s value within `range` as a literal array of plain quoted strings. `None` if `key`
/// is absent; `Some(GlobPatterns::Unknown)` if present but not an array literal, or containing any
/// element that is not a plain quoted string.
fn read_string_array_property(
    masked: &[char],
    original: &[char],
    range: Range<usize>,
    key: &str,
) -> Option<GlobPatterns> {
    read_string_array_property_with_diagnostic_key(masked, original, range, key, key)
}

/// Whether `name` is imported from Vitest's own config entry point in this file.
///
/// Verified, not assumed from the spelling. A project's own `defaultExclude` would be a different
/// value, and this tool's whole complaint about name heuristics is that they cannot tell the two
/// apart. The scan is over the file's full text because that is what the caller already holds.
fn is_vitest_config_import(masked: &[char], original: &[char], name: &str) -> bool {
    is_imported_from_package(masked, original, name, &["vitest/config", "vitest"])
}

/// [`is_vitest_config_import`]'s check for any package: `name` is imported in this file from a
/// specifier whose module part is one of `packages`. The Vitest wrapper keeps its exact
/// behavior; the `glob.sync` project-list reader (B3) reuses the machinery to verify its `glob`
/// identifier really comes from the `glob` package before believing a call shape.
fn is_imported_from_package(
    masked: &[char],
    original: &[char],
    name: &str,
    packages: &[&str],
) -> bool {
    top_level_keyword_positions(masked, "import")
        .into_iter()
        .any(|start| {
            let tail = start + "import".len();
            let Some(from) = top_level_keyword_positions(&masked[tail..], "from")
                .first()
                .copied()
            else {
                return false;
            };
            let from = tail + from;
            let clause: String = masked[tail..from].iter().collect();
            if clause.trim_start().starts_with("type ")
                || clause.contains(';')
                || clause.contains("import")
                || !clause.contains('{')
                || !clause
                    .split(['{', '}', ','])
                    .any(|part| part.trim() == name)
            {
                return false;
            }
            import_specifier(masked, original, from + "from".len())
                .is_some_and(|specifier| packages.contains(&specifier.as_str()))
        })
}

/// Keyword positions in character coordinates, outside nested delimiters and masked non-code.
fn top_level_keyword_positions(masked: &[char], keyword: &str) -> Vec<usize> {
    let token: Vec<char> = keyword.chars().collect();
    let mut depth = 0i32;
    let mut positions = Vec::new();
    for (at, ch) in masked.iter().enumerate() {
        if depth == 0
            && masked[at..].starts_with(&token)
            && (at == 0 || !is_ident_char(masked[at - 1]))
            && !masked
                .get(at + token.len())
                .is_some_and(|ch| is_ident_char(*ch))
        {
            positions.push(at);
        }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    positions
}

/// Read a supported string module specifier after an import from-clause.
fn import_specifier(masked: &[char], original: &[char], after_from: usize) -> Option<String> {
    let start = skip_trivia(original, after_from, original.len());
    let end = masked[start..]
        .iter()
        .position(|ch| matches!(ch, ';' | '\n'))
        .map_or(original.len(), |offset| start + offset);
    read_string_literal(original, start..end)
}

/// Read the last direct array property, distinguishing absence from unknown values in diagnostics.
/// Expand supported optional extglobs and substitute verified Vitest defaultInclude/defaultExclude
/// spreads; unsupported forms remain unknown.
fn read_string_array_property_with_diagnostic_key(
    masked: &[char],
    original: &[char],
    range: Range<usize>,
    key: &str,
    diagnostic_key: &str,
) -> Option<GlobPatterns> {
    let value_start = *find_direct_key_value_starts_precise(masked, original, range, key).last()?;
    if value_start >= masked.len() || masked[value_start] != '[' {
        return Some(GlobPatterns::Unknown {
            reason: format!("`{diagnostic_key}` is set to something other than an array literal"),
        });
    }
    let Some(close) = find_matching_delim(masked, value_start, '[', ']') else {
        return Some(GlobPatterns::Unknown {
            reason: format!("`{diagnostic_key}`'s array literal never reaches a matching `]`"),
        });
    };
    let elements = split_top_level(original, masked, value_start + 1..close);
    let mut values = Vec::with_capacity(elements.len());
    for element in elements {
        match read_string_literal(original, element.clone()) {
            Some(value) => match expand_extglob_optional_groups(&value) {
                // `?(a|b)` groups (if any) are expanded here, once, into concrete alternatives --
                // every consumer of `GlobPatterns::Known` (`match_file`, `discovery.rs`'s
                // in-source candidate compilation) then compiles plain, globset-safe patterns and
                // never has to know extglob syntax existed. See the function's own docs for why
                // this cannot be left to `globset` itself.
                Ok(expanded) => values.extend(expanded),
                Err(reason) => {
                    return Some(GlobPatterns::Unknown {
                        reason: format!("`{diagnostic_key}`: {reason}"),
                    })
                }
            },
            None => {
                // `exclude: [...defaultExclude, '**/svelte-scoped/**']` is a very common Vitest
                // idiom, and `defaultExclude` is Vitest's own export -- the exact list
                // `DEFAULT_EXCLUDE_GLOBS` already mirrors from `docs/vitest-surface.md`.
                // Substituting it is reading Vitest's API surface, the same thing this tool does
                // for `test`, `describe` and `vi`; it is not guessing at a user value. The import
                // is VERIFIED, so a project's own binding of that name does not qualify.
                //
                // Measured: without this, one CSS engine's root project `exclude` is `Unknown`, which
                // makes every file matching it carry an undetermined globals mode -- 100 of its
                // 104 files.
                if let Some(spread) = read_spread_identifier(original, element.clone()) {
                    let known: Option<&[&str]> = match spread.as_str() {
                        "defaultExclude" => Some(DEFAULT_EXCLUDE_GLOBS),
                        "defaultInclude" => Some(DEFAULT_INCLUDE_GLOBS),
                        _ => None,
                    };
                    if let Some(globs) = known {
                        if is_vitest_config_import(masked, original, &spread) {
                            values.extend(globs.iter().map(|glob| (*glob).to_owned()));
                            continue;
                        }
                    }
                }
                return Some(GlobPatterns::Unknown {
                    reason: format!(
                        "`{diagnostic_key}` contains an element that is not a plain quoted string literal"
                    ),
                });
            }
        }
    }
    Some(GlobPatterns::Known(values))
}

/// Upper bound on the number of concrete patterns one `?(a|b)`-bearing glob element expands into.
///
/// Real corpora need at most a couple of small optional groups (Vitest's own default `include`
/// shape, `?(c|m)[jt]s?(x)`, has two). This exists to keep a pathological pattern with many groups
/// from becoming unbounded work, the same reasoning `MAX_PROJECTS`/`MAX_GLOB_WALK_ENTRIES` apply
/// elsewhere in this module -- a pattern that would exceed it is treated as unsupported (`Unknown`
/// with a diagnostic) rather than silently truncated.
const MAX_EXTGLOB_EXPANSIONS: usize = 64;

/// Expands every extglob `?(a|b|...)` "optional group" in `pattern` into the cross product of
/// concrete, `globset`-compatible alternatives (one substituted pattern string per combination,
/// including the zero-occurrence case) -- e.g. `?(c|m)[jt]s?(x)` becomes `js`, `jsx`, `ts`, `tsx`,
/// `cjs`, `cjsx`, `mjs`, `mjsx`, `cts`, `ctsx`, `mts`, `mtsx`.
///
/// `globset` accepts `?(...)`/`+(...)`/`@(...)`/`!(...)` extglob syntax in a pattern *without
/// error* and then matches nothing sensible against real paths -- unlike an unsupported glob
/// syntax that errors and gets reported, a pattern that silently matches nothing is
/// indistinguishable from a project that genuinely has no files there. `discovery.rs`'s
/// `DEFAULT_INCLUDE_GLOBS` already hand-enumerates this exact gap for Vitest's own built-in
/// default (`**/*.{test,spec}.?(c|m)[jt]s?(x)`); this function gives a project's own `include`/
/// `exclude`/`includeSource` the same treatment instead of silently discovering nothing whenever a
/// real config spells its `include` out explicitly (as one observed `runtime` project does).
///
/// Only `?(...)` is translated. `+(...)` (one-or-more), `@(...)` (exactly-one), and `*(...)`
/// (zero-or-more) are structurally different -- a partial, guessed translation of any of them
/// would be exactly the silent-wrong-match failure this function exists to prevent -- so a pattern
/// containing one of those, an unterminated or nested `?(...)` group, or a `?(...)` group that
/// would expand past [`MAX_EXTGLOB_EXPANSIONS`], is rejected with `Err` naming the pattern rather
/// than guessed at.
fn expand_extglob_optional_groups(pattern: &str) -> Result<Vec<String>, String> {
    const INTRODUCERS: [char; 5] = ['?', '+', '@', '!', '*'];
    if !pattern.contains('(') || !INTRODUCERS.iter().any(|c| pattern.contains(*c)) {
        // Cheap short-circuit: the overwhelming common case is a pattern with no extglob syntax
        // at all, and this must stay cheap for it.
        return Ok(vec![pattern.to_owned()]);
    }

    let chars: Vec<char> = pattern.chars().collect();
    // Each entry: the group's `[start, end)` span in `chars` (introducer through closing `)`) and
    // its alternatives, including `""` for the zero-occurrence case `?(...)` itself represents.
    let mut groups: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let introducer = chars[i];
        if !(INTRODUCERS.contains(&introducer) && i + 1 < chars.len() && chars[i + 1] == '(') {
            i += 1;
            continue;
        }
        let mut depth = 1;
        let mut j = i + 2;
        while j < chars.len() && depth > 0 {
            match chars[j] {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        if depth != 0 {
            return Err(format!(
                "`{pattern}` has an extglob group (`{introducer}(`) that never reaches a \
                 matching `)`"
            ));
        }
        if introducer != '?' {
            return Err(format!(
                "`{pattern}` uses `{introducer}(...)`, an extglob form this scan does not \
                 translate"
            ));
        }
        let inner: String = chars[i + 2..j].iter().collect();
        if inner.contains('(') {
            return Err(format!(
                "`{pattern}` has a nested extglob group this scan does not translate"
            ));
        }
        let mut alternatives = vec![String::new()];
        for alternative in inner.split('|') {
            if alternative.is_empty() {
                return Err(format!(
                    "`{pattern}` has an empty alternative in a `?(...)` group"
                ));
            }
            alternatives.push(alternative.to_owned());
        }
        groups.push((i, j + 1, alternatives));
        i = j + 1;
    }

    if groups.is_empty() {
        // Every extglob-introducer character present turned out to not actually start a `(...)`
        // group (e.g. a literal `?` matching a single path character) -- nothing to expand.
        return Ok(vec![pattern.to_owned()]);
    }

    let total_expansions: usize = groups.iter().map(|(_, _, alts)| alts.len()).product();
    if total_expansions > MAX_EXTGLOB_EXPANSIONS {
        return Err(format!(
            "`{pattern}` would expand into {total_expansions} concrete alternatives, more than \
             this scan translates soundly"
        ));
    }

    let mut results = vec![String::new()];
    let mut cursor = 0;
    for (start, end, alternatives) in &groups {
        let literal_prefix: String = chars[cursor..*start].iter().collect();
        let mut next = Vec::with_capacity(results.len() * alternatives.len());
        for existing in &results {
            for alternative in alternatives {
                next.push(format!("{existing}{literal_prefix}{alternative}"));
            }
        }
        results = next;
        cursor = *end;
    }
    let literal_suffix: String = chars[cursor..].iter().collect();
    for result in &mut results {
        result.push_str(&literal_suffix);
    }
    Ok(results)
}

/// Reads `key`'s value within `range` as a single plain quoted string, e.g. `test.name`.
fn read_string_property(
    masked: &[char],
    original: &[char],
    range: Range<usize>,
    key: &str,
) -> Option<String> {
    let value_start =
        *find_direct_key_value_starts_precise(masked, original, range.clone(), key).last()?;
    read_string_literal(original, value_extent(masked, value_start, range.end))
}

/// Bounds filesystem work shared across every glob expansion performed while resolving one
/// repository's projects.
#[derive(Default)]
struct Budget {
    entries_visited: usize,
}

impl Budget {
    /// Report whether the directory-entry walk has reached its configured budget.
    fn exhausted(&self) -> bool {
        self.entries_visited >= MAX_GLOB_WALK_ENTRIES
    }
}

/// Records that the `test.projects`/workspace entry at `declared_at` could not be resolved into a
/// known [`Project`], widening `position` to `declared_at` if it is not already at least that far.
/// Only the maximum is kept: [`ProjectSet::governing_project`] only ever needs to know whether
/// *some* unresolved entry was declared after a given known match, and the greatest position
/// already implies every smaller one.
fn note_unresolved_position(position: &mut Option<usize>, declared_at: usize) {
    *position = Some(position.map_or(declared_at, |current| current.max(declared_at)));
}

/// The patterns a comma alternation in `pattern` stands for, or `None` when it holds none, or too
/// many to enumerate.
fn brace_alternatives(pattern: &str) -> Option<Vec<String>> {
    const MAX_ALTERNATIVES: usize = 64;
    let open = pattern.find('{')?;
    let mut depth = 0usize;
    let mut close = None;
    let mut cuts = vec![open];
    for (offset, ch) in pattern[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + offset);
                    break;
                }
            }
            ',' if depth == 1 => cuts.push(open + offset),
            _ => {}
        }
    }
    let close = close?;
    if cuts.len() < 2 {
        return None;
    }
    cuts.push(close);
    let (head, tail) = (&pattern[..open], &pattern[close + 1..]);
    let mut all = Vec::new();
    for pair in cuts.windows(2) {
        let candidate = format!("{head}{}{tail}", &pattern[pair[0] + 1..pair[1]]);
        match brace_alternatives(&candidate) {
            Some(nested) => all.extend(nested),
            None => all.push(candidate),
        }
        if all.len() > MAX_ALTERNATIVES {
            return None;
        }
    }
    Some(all)
}

/// Whether `pattern` names a location outside its own directory without consulting the
/// filesystem: an absolute path or a `..` segment.
fn leaves_the_tree_lexically(pattern: &str) -> bool {
    Path::new(pattern).is_absolute()
        || pattern.starts_with('/')
        || pattern.split('/').any(|segment| segment == "..")
}

/// Recognize path segments containing a supported glob metacharacter.
fn looks_like_glob(segment: &str) -> bool {
    segment.contains(['*', '?', '[', '{'])
}

/// What one `test.projects` string entry expanded to.
struct ExpandedProjectPaths {
    /// Concrete project configuration files under the analysis root, sorted.
    paths: Vec<PathBuf>,
    /// Matches (or a glob's own start directory) that resolve outside the analysis root. None of
    /// them was read or walked.
    outside: usize,
}

/// Resolves one `test.projects` string entry (a directory glob, an explicit config-file glob, or a
/// literal path) into concrete project configuration file paths, sorted for determinism regardless
/// of filesystem traversal order.
///
/// Every path is confined to `root` after canonicalization: a `..` segment, an absolute path or a
/// symlink that leaves the tree is counted in [`ExpandedProjectPaths::outside`] and never read.
fn expand_project_path(root: &Path, pattern: &str, budget: &mut Budget) -> ExpandedProjectPaths {
    let mut expanded = ExpandedProjectPaths {
        paths: Vec::new(),
        outside: 0,
    };
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);

    // The walk starts from the pattern's literal prefix, so an alternative like `{pkg,../other}`
    // that leaves the tree would never be visited, let alone judged. An alternation holding one
    // is split into its alternatives, each expanded and confined on its own.
    if let Some(alternatives) = brace_alternatives(pattern)
        .filter(|all| all.iter().any(|one| leaves_the_tree_lexically(one)))
    {
        let mut paths = BTreeSet::new();
        for alternative in alternatives {
            let part = expand_project_path(root, &alternative, budget);
            expanded.outside += part.outside;
            paths.extend(part.paths);
        }
        expanded.paths = paths.into_iter().collect();
        return expanded;
    }

    if !looks_like_glob(pattern) {
        let candidate = root.join(pattern);
        if matches!(confine(root, &candidate), Confinement::Outside) {
            expanded.outside += 1;
            return expanded;
        }
        let config = if candidate.is_file() {
            Some(candidate)
        } else if candidate.is_dir() {
            find_project_config(&candidate)
        } else {
            None
        };
        // A directory's config can itself be a symlink out of the tree.
        match config {
            Some(config) if matches!(confine(root, &config), Confinement::Outside) => {
                expanded.outside += 1;
            }
            Some(config) => expanded.paths.push(config),
            None => {}
        }
        return expanded;
    }

    let Ok(matcher) = GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
    else {
        return expanded;
    };

    let prefix_segments: Vec<&str> = pattern
        .split('/')
        .take_while(|segment| !looks_like_glob(segment))
        .collect();
    // The walk does not follow links, so a symlinked directory in the middle of the pattern is
    // never entered. Vitest follows it, and it may hold a project; these match the directory
    // prefixes of the pattern so such a link is still counted when it leaves the tree.
    let directory_matchers: Vec<_> = (1..pattern.split('/').count())
        .filter_map(|keep| {
            let prefix = pattern.split('/').take(keep).collect::<Vec<_>>().join("/");
            GlobBuilder::new(&prefix)
                .literal_separator(true)
                .build()
                .ok()
                .map(|glob| glob.compile_matcher())
        })
        .collect();
    let start_dir = root.join(prefix_segments.join("/"));
    if matches!(confine(root, &start_dir), Confinement::Outside) {
        // Not even listed: enumerating a directory outside the tree is itself a read.
        expanded.outside += 1;
        return expanded;
    }
    if !start_dir.is_dir() {
        return expanded;
    }

    let mut builder = WalkBuilder::new(&start_dir);
    builder
        .standard_filters(true)
        .follow_links(false)
        .require_git(false);
    if pattern.contains("**") {
        builder.max_depth(Some(MAX_GLOB_WALK_DEPTH));
    } else {
        let remaining = pattern
            .split('/')
            .count()
            .saturating_sub(prefix_segments.len());
        builder.max_depth(Some(remaining.max(1)));
    }

    let mut matches = BTreeSet::new();
    for entry in builder.build() {
        if budget.exhausted() {
            break;
        }
        budget.entries_visited += 1;
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(path);
        let relative = relative.to_string_lossy().replace('\\', "/");
        if !matcher.is_match(&relative) {
            if entry.file_type().is_some_and(|kind| kind.is_symlink())
                && directory_matchers.iter().any(|dir| dir.is_match(&relative))
                && path.is_dir()
                && matches!(confine(root, path), Confinement::Outside)
            {
                expanded.outside += 1;
            }
            continue;
        }
        let config = match entry.file_type() {
            Some(file_type) if file_type.is_file() => Some(path.to_path_buf()),
            Some(file_type) if file_type.is_dir() => find_project_config(path),
            // Links are not followed, so a link inside the tree is not a second project. One that
            // leaves it is still reported, so a refused entry is never silent.
            Some(file_type) if file_type.is_symlink() => {
                if matches!(confine(root, path), Confinement::Outside) {
                    expanded.outside += 1;
                }
                None
            }
            _ => None,
        };
        match config {
            Some(config) if matches!(confine(root, &config), Confinement::Outside) => {
                expanded.outside += 1;
            }
            Some(config) => {
                matches.insert(config);
            }
            None => {}
        }
    }
    expanded.paths = matches.into_iter().collect();
    expanded
}

/// Locates a standalone config without following nested `test.projects` or string `extends`.
/// `mergeConfig` exports are checked before the generic reader: the base must be an imported
/// identifier and the override an entire literal or a verified `defineConfig`/`defineProject` wrapper.
/// Explicit arrays concatenate before defaults, scalar overrides win, and nullish overrides
/// preserve the base. Missing bases leave discovery arrays
/// unknown, while cycles and excessive depth refuse the chain.
fn locate_config_config_object(masked: &[char], original: &[char], path: &Path) -> ConfigObject {
    let Some(value_start) = find_export_default_value_start(masked) else {
        return ConfigObject::NoExport;
    };
    if !merge_config_export_shape(masked, original, value_start) {
        return locate_config_object(masked);
    }
    // A merge must never fall through to the generic reader's first-argument shortcut.
    let Some(overrides) = merge_config_overrides_literal(masked, original, value_start) else {
        return ConfigObject::NonLiteral;
    };
    // The merge cannot be performed here -- this function returns positions, the merge needs the
    // base FILE. Callers that reach this arm re-resolve through `resolve_standalone_config` on
    // the base path; the positions this returns are the OVERRIDES object's, whose `test` body is
    // what `RawTestFields::read` should read.
    let _ = path;
    ConfigObject::Found(overrides.0, overrides.1)
}

/// Recognize a mergeConfig identifier prefix at the export value; argument and import
/// validation happen separately.
fn merge_config_export_shape(masked: &[char], original: &[char], value_start: usize) -> bool {
    let callee_start = skip_trivia(original, value_start, original.len());
    let merge_callee: Vec<char> = "mergeConfig".chars().collect();
    masked[callee_start..].starts_with(&merge_callee)
        && !masked
            .get(callee_start + merge_callee.len())
            .is_some_and(|c| is_ident_char(*c))
}

/// The override literal of a verified `mergeConfig(<identifier>, <literal>)` export.
/// [`merge_config_base_path`] resolves the first argument's imported file.
fn merge_config_overrides_literal(
    masked: &[char],
    original: &[char],
    value_start: usize,
) -> Option<(usize, usize)> {
    let callee = "mergeConfig";
    if !is_imported_from_package(
        masked,
        original,
        callee,
        &["vitest/config", "vitest", "vite"],
    ) {
        return None;
    }
    let callee_start = skip_trivia(original, value_start, original.len());
    if !masked[callee_start..].starts_with(&callee.chars().collect::<Vec<_>>()) {
        return None;
    }
    let after = callee_start + callee.len();
    if masked.get(after).is_some_and(|c| is_ident_char(*c)) {
        return None;
    }
    let open_paren = skip_trivia(original, after, original.len());
    if masked.get(open_paren) != Some(&'(') {
        return None;
    }
    let close_paren = find_matching_delim(masked, open_paren, '(', ')')?;
    let arguments = split_top_level(original, masked, open_paren + 1..close_paren);
    if arguments.len() != 2 {
        return None;
    }
    // Both arguments and the complete export expression must match the supported shape.
    if !merge_config_export_ends_after_optional_object_assertion(original, close_paren + 1) {
        return None;
    }
    read_bare_identifier(original, arguments[0].clone())?;
    static_config_object_literal(masked, original, arguments[1].clone())
}

/// Accepts the runtime-neutral assertion observed on package configs, and nothing broader.
///
/// A general TypeScript type parser does not belong in this textual config reader. The observed
/// shape is exactly `mergeConfig(...) as object`; admitting an arbitrary suffix here would bring
/// back the object-prefix bug fixed in PR #34, where a readable prefix hid a conflicting tail.
fn merge_config_export_ends_after_optional_object_assertion(
    original: &[char],
    after_expression: usize,
) -> bool {
    let mut end = skip_trivia(original, after_expression, original.len());
    if original.get(end) == Some(&';') || end == original.len() {
        return true;
    }

    for word in ["as", "object"] {
        let token: Vec<char> = word.chars().collect();
        if !original[end..].starts_with(&token)
            || original
                .get(end + token.len())
                .is_some_and(|ch| is_ident_char(*ch))
        {
            return false;
        }
        end = skip_trivia(original, end + token.len(), original.len());
    }
    original.get(end) == Some(&';') || end == original.len()
}

/// Only an entire object literal or a verified identity config/project wrapper is static.
fn static_config_object_literal(
    masked: &[char],
    original: &[char],
    value: Range<usize>,
) -> Option<(usize, usize)> {
    let start = skip_trivia(original, value.start, value.end);
    let (open, limit) = if masked.get(start) == Some(&'{') {
        (start, value.end)
    } else {
        let mut end = start;
        while end < value.end && is_ident_char(masked[end]) {
            end += 1;
        }
        let name: String = masked[start..end].iter().collect();
        let valid = match name.as_str() {
            "defineProject" => is_vitest_config_import(masked, original, &name),
            "defineConfig" => is_imported_from_package(
                masked,
                original,
                &name,
                &["vitest/config", "vitest", "vite"],
            ),
            _ => false,
        };
        if !valid {
            return None;
        }
        let callee: Vec<char> = name.chars().collect();
        let paren = skip_trivia(original, start + callee.len(), value.end);
        if masked.get(paren) != Some(&'(') {
            return None;
        }
        let close = find_matching_delim(masked, paren, '(', ')')?;
        if close >= value.end || skip_trivia(original, close + 1, value.end) != value.end {
            return None;
        }
        (skip_trivia(original, paren + 1, close), close)
    };
    if masked.get(open) != Some(&'{') {
        return None;
    }
    let close = find_matching_delim(masked, open, '{', '}')?;
    if close >= limit || skip_trivia(original, close + 1, limit) != limit {
        return None;
    }
    Some((open, close))
}

/// The imported file and selected export supplying a merge base.
struct ImportedConfigBase {
    path: PathBuf,
    named_export: Option<String>,
}

/// Identify a supported default or named import for the local binding; return None when the
/// clause cannot establish it.
fn imported_config_binding(clause: &str, local_name: &str) -> Option<Option<String>> {
    if clause.contains(';') || clause.contains("import") || clause.trim_start().starts_with("type ")
    {
        return None;
    }
    let clause = clause.trim();
    let first = clause.split(',').next()?.trim();
    if first == local_name {
        return Some(None);
    }
    let open = clause.find('{')?;
    let close = clause.rfind('}')?;
    if close <= open {
        return None;
    }
    for entry in clause[open + 1..close].split(',') {
        let tokens: Vec<_> = entry.split_whitespace().collect();
        let exported = match tokens.as_slice() {
            [name] if *name == local_name => *name,
            [name, "as", local] if *local == local_name => *name,
            _ => continue,
        };
        return Some((exported != "default").then(|| exported.to_owned()));
    }
    None
}

/// Resolve the imported base of a verified mergeConfig export, retaining any selected named
/// export.
///
/// With a `root`, a base file that resolves outside it is refused: `Some(Err(reason))`, never
/// read. `None` keeps its meaning of "no statically resolvable base".
fn merge_config_base_path(
    root: Option<&Path>,
    masked: &[char],
    original: &[char],
    config_path: &Path,
) -> Option<Result<ImportedConfigBase, String>> {
    let value_start = find_export_default_value_start(masked)?;
    merge_config_overrides_literal(masked, original, value_start)?;
    let callee_start = skip_trivia(original, value_start, original.len());
    let open_paren = skip_trivia(original, callee_start + "mergeConfig".len(), original.len());
    let close_paren = find_matching_delim(masked, open_paren, '(', ')')?;
    let arguments = split_top_level(original, masked, open_paren + 1..close_paren);
    let name = read_bare_identifier(original, arguments.first()?.clone())?;
    let (literal, named_export) = top_level_keyword_positions(masked, "import")
        .into_iter()
        .find_map(|start| {
            let tail = start + "import".len();
            let from = tail
                + top_level_keyword_positions(&masked[tail..], "from")
                    .first()
                    .copied()?;
            let clause: String = masked[tail..from].iter().collect();
            let named_export = imported_config_binding(&clause, &name)?;
            Some((
                import_specifier(masked, original, from + "from".len())?,
                named_export,
            ))
        })?;
    if !literal.starts_with('.') {
        return None;
    }
    let base_dir = config_path.parent()?;
    let mut candidates = vec![
        literal.clone(),
        format!("{literal}.ts"),
        format!("{literal}.mts"),
        format!("{literal}.cts"),
        format!("{literal}.js"),
        format!("{literal}.mjs"),
        format!("{literal}.cjs"),
    ];
    let source_extension = match Path::new(&literal).extension().and_then(|ext| ext.to_str()) {
        Some("js") => Some("ts"),
        Some("mjs") => Some("mts"),
        Some("cjs") => Some("cts"),
        _ => None,
    };
    if let Some(extension) = source_extension {
        let mut source_path = PathBuf::from(&literal);
        source_path.set_extension(extension);
        candidates.push(source_path.to_string_lossy().into_owned());
    }
    for candidate in candidates {
        let path = base_dir.join(&candidate);
        if path.is_file() {
            if let Some(root) =
                root.filter(|root| matches!(confine(root, &path), Confinement::Outside))
            {
                return Some(Err(format!(
                    "{}'s mergeConfig base `{literal}` resolves outside the analysis root and \
                     was not read",
                    display_path(config_path, root)
                )));
            }
            return Some(Ok(ImportedConfigBase { path, named_export }));
        }
    }
    // No candidate exists, but a base spelled outside the tree is refused all the same, as an
    // `extends` is: nothing was read, and the base's contributions are unknown.
    if let Some(root) = root.filter(|root| {
        matches!(
            confine(root, &base_dir.join(&literal)),
            Confinement::Outside
        )
    }) {
        return Some(Err(format!(
            "{}'s mergeConfig base `{literal}` resolves outside the analysis root and was not \
             read",
            display_path(config_path, root)
        )));
    }
    None
}

const MAX_MERGE_CONFIG_DEPTH: usize = 32;

struct StandaloneConfigFields {
    name: Option<String>,
    effective: EffectiveTestFields,
    specified: RawTestFields,
    /// Why a `mergeConfig` base in the chain was refused as outside the analysis root. Carried
    /// beside `effective` because a consumer that sets every field itself discards the `Unknown`
    /// values that record it there.
    refusal: Option<String>,
}

/// Recognize a direct nullish property value, refusing a potentially shadowed undefined
/// binding.
fn nullish_property(masked: &[char], original: &[char], body: Range<usize>, key: &str) -> bool {
    let Some(&start) =
        find_direct_key_value_starts_precise(masked, original, body.clone(), key).last()
    else {
        return false;
    };
    let value: String = masked[value_extent(masked, start, body.end)]
        .iter()
        .collect();
    match value.trim() {
        "null" | "void 0" => true,
        "undefined" => {
            // A local binding can replace the global undefined value. Refuse uncertain scope.
            if !top_level_keyword_positions(masked, "undefined").is_empty() {
                return false;
            }
            for keyword in ["import", "const", "let", "var"] {
                for at in top_level_keyword_positions(masked, keyword) {
                    let tail = at + keyword.len();
                    let end = if keyword == "import" {
                        let Some(from) = top_level_keyword_positions(&masked[tail..], "from")
                            .first()
                            .copied()
                        else {
                            return false;
                        };
                        tail + from
                    } else {
                        masked[tail..]
                            .iter()
                            .position(|ch| *ch == ';')
                            .map_or(masked.len(), |n| tail + n)
                    };
                    let clause: String = masked[tail..end].iter().collect();
                    if clause
                        .split(|ch: char| !is_ident_char(ch))
                        .any(|word| word == "undefined")
                    {
                        return false;
                    }
                }
            }
            true
        }
        _ => false,
    }
}

/// Return a direct property's object body only when it occupies its entire value expression.
fn complete_property_object(
    masked: &[char],
    original: &[char],
    body: Range<usize>,
    key: &str,
) -> Option<Range<usize>> {
    let start =
        *find_direct_key_value_starts_precise(masked, original, body.clone(), key).last()?;
    if masked.get(start) != Some(&'{') {
        return None;
    }
    let close = find_matching_delim(masked, start, '{', '}')?;
    let end = value_extent(masked, start, body.end).end;
    (skip_trivia(original, close + 1, end) == end).then_some(start + 1..close)
}

/// Validate complete nested groups before computing inheritance. Only the merging view skips
/// nullish values; standalone effective fields retain their conservative unreadable state.
fn normalize_merge_fields(
    fields: &mut RawTestFields,
    masked: &[char],
    original: &[char],
    body: Range<usize>,
    skip_nulls: bool,
) {
    if skip_nulls && nullish_property(masked, original, body.clone(), "globals") {
        fields.globals = None;
    }
    if skip_nulls && nullish_property(masked, original, body.clone(), "include") {
        fields.include = None;
    }
    if skip_nulls && nullish_property(masked, original, body.clone(), "exclude") {
        fields.exclude = None;
    }
    if skip_nulls && nullish_property(masked, original, body.clone(), "includeSource") {
        fields.include_source = None;
    }
    for parent in ["benchmark", "typecheck"] {
        let null_group = nullish_property(masked, original, body.clone(), parent);
        let nested = complete_property_object(masked, original, body.clone(), parent);
        if !null_group && nested.is_none() {
            let present =
                !find_direct_key_value_starts_precise(masked, original, body.clone(), parent)
                    .is_empty();
            if present {
                let unknown = || {
                    Some(GlobPatterns::Unknown {
                        reason: format!("`{parent}` is not a complete static object literal"),
                    })
                };
                if parent == "benchmark" {
                    fields.benchmark_include = unknown();
                    fields.benchmark_exclude = unknown();
                    fields.benchmark_include_source = unknown();
                } else {
                    fields.typecheck_enabled = Some(None);
                    fields.typecheck_include = unknown();
                    fields.typecheck_exclude = unknown();
                }
            }
        }
        let skipped = |key| {
            skip_nulls
                && (null_group
                    || nested.as_ref().is_some_and(|range| {
                        nullish_property(masked, original, range.clone(), key)
                    }))
        };
        if parent == "benchmark" {
            if skipped("include") {
                fields.benchmark_include = None;
            }
            if skipped("exclude") {
                fields.benchmark_exclude = None;
            }
            if skipped("includeSource") {
                fields.benchmark_include_source = None;
            }
        } else {
            if skipped("enabled") {
                fields.typecheck_enabled = None;
            }
            if skipped("include") {
                fields.typecheck_include = None;
            }
            if skipped("exclude") {
                fields.typecheck_exclude = None;
            }
        }
    }
}

/// Concatenate known base and override arrays in that order, preserving absence and propagating
/// unknown bases or values.
fn merge_config_array(
    own: Option<GlobPatterns>,
    base: Option<GlobPatterns>,
    unresolved_base: bool,
) -> Option<GlobPatterns> {
    if unresolved_base {
        return Some(GlobPatterns::Unknown {
            reason: "mergeConfig base arrays cannot be determined".to_owned(),
        });
    }
    match (base, own) {
        (None, own) => own,
        (base, None) => base,
        (Some(GlobPatterns::Known(mut base)), Some(GlobPatterns::Known(own))) => {
            base.extend(own);
            Some(GlobPatterns::Known(base))
        }
        (Some(GlobPatterns::Unknown { reason }), _)
        | (_, Some(GlobPatterns::Unknown { reason })) => Some(GlobPatterns::Unknown { reason }),
    }
}

/// Merge all modeled glob-array channels from the base into the override fields.
fn merge_config_arrays(own: &mut RawTestFields, base: RawTestFields, unresolved: bool) {
    own.include = merge_config_array(own.include.take(), base.include, unresolved);
    own.exclude = merge_config_array(own.exclude.take(), base.exclude, unresolved);
    own.include_source =
        merge_config_array(own.include_source.take(), base.include_source, unresolved);
    own.benchmark_include = merge_config_array(
        own.benchmark_include.take(),
        base.benchmark_include,
        unresolved,
    );
    own.benchmark_exclude = merge_config_array(
        own.benchmark_exclude.take(),
        base.benchmark_exclude,
        unresolved,
    );
    own.benchmark_include_source = merge_config_array(
        own.benchmark_include_source.take(),
        base.benchmark_include_source,
        unresolved,
    );
    own.typecheck_include = merge_config_array(
        own.typecheck_include.take(),
        base.typecheck_include,
        unresolved,
    );
    own.typecheck_exclude = merge_config_array(
        own.typecheck_exclude.take(),
        base.typecheck_exclude,
        unresolved,
    );
}

/// Read one direct exported const, selecting its literal rather than a different default export.
/// Other export forms and references that could mutate the binding remain unresolved.
fn named_config_literal(masked: &[char], original: &[char], name: &str) -> Option<(usize, usize)> {
    let token: Vec<char> = name.chars().collect();
    // Escaped identifiers can reference the binding without spelling its literal name.
    // Strings and comments are masked, so their backslashes do not cause refusal.
    if token.is_empty() || masked.contains(&'\\') {
        return None;
    }
    let uses = masked
        .windows(token.len())
        .enumerate()
        .filter(|(at, chars)| {
            *chars == token
                && (*at == 0 || !is_ident_char(masked[*at - 1]))
                && !masked
                    .get(*at + token.len())
                    .is_some_and(|ch| is_ident_char(*ch))
        })
        .count();
    if uses != 1 {
        return None;
    }
    for export in top_level_keyword_positions(masked, "export") {
        let declaration = skip_trivia(original, export + "export".len(), original.len());
        if !masked[declaration..].starts_with(&['c', 'o', 'n', 's', 't']) {
            continue;
        }
        let start = skip_trivia(original, declaration + "const".len(), original.len());
        if !masked[start..].starts_with(&token)
            || masked
                .get(start + token.len())
                .is_some_and(|ch| is_ident_char(*ch))
        {
            continue;
        }
        let value = find_const_initializer(masked, original, name, original.len())?;
        return static_config_object_literal(masked, original, value);
    }
    None
}

/// Resolve a standalone config to its project name, effective fields and any refused
/// `mergeConfig` base, returning None when resolution fails. Every file the `mergeConfig` chain
/// reads must lie under `root`.
fn resolve_standalone_config_within(root: &Path, path: &Path) -> Option<StandaloneConfigFields> {
    let mut refused = false;
    resolve_standalone_config_bounded(Some(root), path, None, &mut Vec::new(), &mut refused)
}

/// [`resolve_standalone_config_within`] without a root to confine the chain to.
#[cfg(test)]
fn resolve_standalone_config(path: &Path) -> Option<(Option<String>, EffectiveTestFields)> {
    let mut refused = false;
    let fields =
        resolve_standalone_config_bounded(None, path, None, &mut Vec::new(), &mut refused)?;
    Some((fields.name, fields.effective))
}

/// Resolves the comparison scope of a standalone config without executing it.
///
/// Unlike [`crate::vitest_config::config_test_scope`], this path follows the statically verified
/// `mergeConfig(imported_base, literal_override)` form already owned by this module. Both sides'
/// raw `test` object text participate in the identity. That deliberately over-separates
/// formatting-only differences, but it cannot merge two configurations that differ in a field
/// this analyzer does not otherwise model.
///
/// Every file the `mergeConfig` chain reads must lie under `root`; one that does not is an `Err`.
pub(crate) fn resolved_config_test_scope_within(
    root: &Path,
    path: &Path,
) -> Result<Option<String>, String> {
    resolve_config_test_scope_bounded(Some(root), path, None, &mut Vec::new())
}

/// [`resolved_config_test_scope_within`] without a root to confine the chain to.
#[cfg(test)]
pub(crate) fn resolved_config_test_scope(path: &Path) -> Result<Option<String>, String> {
    resolve_config_test_scope_bounded(None, path, None, &mut Vec::new())
}

/// Resolve comparison scope while tracking canonical config paths; return an error for cycles,
/// excess depth or unresolved paths.
fn resolve_config_test_scope_bounded(
    root: Option<&Path>,
    path: &Path,
    export: Option<&str>,
    active: &mut Vec<PathBuf>,
) -> Result<Option<String>, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
    if active.len() >= MAX_MERGE_CONFIG_DEPTH || active.contains(&canonical) {
        return Err(format!(
            "{} has a cyclic or excessively deep config chain",
            path.display()
        ));
    }
    active.push(canonical);
    let result = resolve_config_test_scope_inner(root, path, export, active);
    active.pop();
    result
}

/// Read a bounded config and derive its comparison scope, following supported mergeConfig bases
/// and reporting unsupported forms.
fn resolve_config_test_scope_inner(
    root: Option<&Path>,
    path: &Path,
    export: Option<&str>,
    active: &mut Vec<PathBuf>,
) -> Result<Option<String>, String> {
    let source = read_bounded(path)?;
    let masked = mask_non_code(&source).ok_or_else(|| {
        format!(
            "{} contains an unterminated string, template literal, or comment",
            path.display()
        )
    })?;
    let original: Vec<char> = source.chars().collect();
    let (config_object, is_merge, merge_base) = if let Some(name) = export {
        (
            named_config_literal(&masked, &original, name)
                .ok_or_else(|| format!("{}'s `{name}` export cannot be read", path.display()))?,
            false,
            None,
        )
    } else {
        let value_start = find_export_default_value_start(&masked)
            .ok_or_else(|| format!("{} has no `export default`", path.display()))?;
        if !merge_config_export_shape(&masked, &original, value_start) {
            return crate::vitest_config::config_test_scope(path);
        }
        let is_merge = merge_config_overrides_literal(&masked, &original, value_start).is_some();
        if !is_merge {
            return Err(format!(
                "{}'s mergeConfig default export cannot be read",
                path.display()
            ));
        }
        let object = match locate_config_config_object(&masked, &original, path) {
            ConfigObject::Found(open, close) => (open, close),
            _ => {
                return Err(format!(
                    "{}'s default export cannot be read",
                    path.display()
                ))
            }
        };
        let merge_base = is_merge
            .then(|| merge_config_base_path(root, &masked, &original, path))
            .flatten();
        (object, is_merge, merge_base)
    };

    let body = config_object.0 + 1..config_object.1;
    let own_scope = match locate_test_object(&masked, body.clone()) {
        TestObject::Absent => None,
        TestObject::NonLiteral
            if is_merge && nullish_property(&masked, &original, body.clone(), "test") =>
        {
            None
        }
        TestObject::Found(_, _) => {
            let range = complete_property_object(&masked, &original, body, "test")
                .ok_or_else(|| format!("{}'s `test` value cannot be read", path.display()))?;
            Some(original[range].iter().collect::<String>())
        }
        TestObject::NonLiteral | TestObject::Malformed => {
            return Err(format!("{}'s `test` value cannot be read", path.display()))
        }
    };

    if !is_merge {
        return Ok(own_scope);
    }
    let base = merge_base
        .ok_or_else(|| format!("{}'s mergeConfig base cannot be resolved", path.display()))??;
    let base_scope =
        resolve_config_test_scope_bounded(root, &base.path, base.named_export.as_deref(), active)?;
    if base_scope.is_none() && own_scope.is_none() {
        return Ok(None);
    }
    Ok(Some(format!(
        "merge(base:{}:{},override:{}:{})",
        base_scope.as_ref().map_or(0, String::len),
        base_scope.as_deref().unwrap_or(""),
        own_scope.as_ref().map_or(0, String::len),
        own_scope.as_deref().unwrap_or("")
    )))
}

/// Resolve config fields with canonical-path cycle/depth guards, marking the refusal flag when
/// either guard is reached.
fn resolve_standalone_config_bounded(
    root: Option<&Path>,
    path: &Path,
    export: Option<&str>,
    active: &mut Vec<PathBuf>,
    refused: &mut bool,
) -> Option<StandaloneConfigFields> {
    let canonical = path.canonicalize().ok()?;
    if active.len() >= MAX_MERGE_CONFIG_DEPTH || active.contains(&canonical) {
        *refused = true;
        return None;
    }
    active.push(canonical);
    let result = resolve_standalone_config_inner(root, path, export, active, refused);
    active.pop();
    result
}

/// Read and resolve supported standalone config fields and inheritance without executing the
/// config.
fn resolve_standalone_config_inner(
    root: Option<&Path>,
    path: &Path,
    export: Option<&str>,
    active: &mut Vec<PathBuf>,
    refused: &mut bool,
) -> Option<StandaloneConfigFields> {
    if !path.is_file() {
        return None;
    }
    let source = read_bounded(path).ok()?;
    let masked = mask_non_code(&source)?;
    let original: Vec<char> = source.chars().collect();
    let (config_object, is_merge, merge_base) = if let Some(name) = export {
        (named_config_literal(&masked, &original, name)?, false, None)
    } else {
        let config_object = match locate_config_config_object(&masked, &original, path) {
            ConfigObject::Found(open, close) => (open, close),
            _ => return None,
        };
        let value_start = find_export_default_value_start(&masked)?;
        (
            config_object,
            merge_config_overrides_literal(&masked, &original, value_start).is_some(),
            merge_config_base_path(root, &masked, &original, path),
        )
    };
    let body = config_object.0 + 1..config_object.1;
    let test_object = locate_test_object(&masked, body.clone());
    let test_body = match test_object {
        TestObject::Absent => None,
        TestObject::NonLiteral
            if is_merge && nullish_property(&masked, &original, body.clone(), "test") =>
        {
            None
        }
        TestObject::NonLiteral | TestObject::Malformed => return None,
        TestObject::Found(_, _) => {
            Some(complete_property_object(&masked, &original, body, "test")?)
        }
    };
    let mut fields = test_body
        .as_ref()
        .map_or_else(RawTestFields::empty, |body| {
            RawTestFields::read(&masked, &original, body.clone())
        });
    let name_present = test_body.as_ref().is_some_and(|body| {
        property_present(&masked, body.clone(), "name")
            && !(is_merge && nullish_property(&masked, &original, body.clone(), "name"))
    });
    if is_merge {
        if let Some(body) = &test_body {
            normalize_merge_fields(&mut fields, &masked, &original, body.clone(), true);
        }
        let mut base_refusal = None;
        let resolved_base = match merge_base {
            Some(Ok(base)) => {
                let resolved = resolve_standalone_config_bounded(
                    root,
                    &base.path,
                    base.named_export.as_deref(),
                    active,
                    refused,
                );
                if resolved.is_none() && (*refused || base.named_export.is_some()) {
                    return None;
                }
                // A base that exists and cannot be read contributes unknown fields. Left only in
                // those fields, it vanishes when a project sets them itself or `extends: false`.
                if resolved.is_none() {
                    base_refusal = root.map_or_else(
                        || Some(format!("{} could not be read", base.path.display())),
                        |root| {
                            Some(format!(
                                "{}, the mergeConfig base of {}, could not be read",
                                display_path(&base.path, root),
                                display_path(path, root)
                            ))
                        },
                    );
                }
                resolved
            }
            Some(Err(reason)) => {
                base_refusal = Some(reason);
                None
            }
            None => None,
        };
        if let Some(base) = resolved_base {
            let name = if name_present {
                fields.name.clone()
            } else {
                base.name
            };
            merge_config_arrays(&mut fields, base.specified, false);
            let effective =
                resolve_effective(&fields, &ExtendsMode::Inherit, Some(&base.effective), path);
            return Some(StandaloneConfigFields {
                name,
                effective,
                specified: fields,
                refusal: base.refusal,
            });
        }
        merge_config_arrays(&mut fields, RawTestFields::empty(), true);
        let mode = ExtendsMode::Unresolvable(base_refusal.clone().unwrap_or_else(|| {
            format!("mergeConfig base in {} cannot be resolved", path.display())
        }));
        let effective = resolve_effective(&fields, &mode, None, path);
        return Some(StandaloneConfigFields {
            name: fields.name.clone(),
            effective,
            specified: fields,
            refusal: base_refusal,
        });
    }
    if let Some(body) = &test_body {
        normalize_merge_fields(&mut fields, &masked, &original, body.clone(), false);
    }
    let effective = if test_body.is_none() {
        EffectiveTestFields {
            globals: GlobalsDetection::Disabled {
                config: path.to_path_buf(),
            },
            include: default_include(),
            exclude: default_exclude(),
            include_source: default_include_source(),
            benchmark_include: default_benchmark_include(),
            benchmark_exclude: default_benchmark_exclude(),
            benchmark_include_source: default_benchmark_include_source(),
            typecheck_enabled: TypecheckAdmission::Disabled,
            typecheck_include: default_typecheck_include(),
            typecheck_exclude: default_exclude(),
        }
    } else {
        resolve_effective(&fields, &ExtendsMode::NoInherit, None, path)
    };
    // Null base values do not contribute an array prefix, although their effective standalone
    // value may remain unknown. Preserve that distinction for any importing merge.
    if let Some(body) = &test_body {
        normalize_merge_fields(&mut fields, &masked, &original, body.clone(), true);
    }
    Some(StandaloneConfigFields {
        name: fields.name.clone(),
        effective,
        specified: fields,
        refusal: None,
    })
}

/// A [`Project`] nothing could be resolved about, every field family `Unknown` for one shared
/// `reason`.
///
/// The four field families each carry the reason separately because each is reported separately,
/// but when a project's configuration could not be read at all there is only ever *one* reason,
/// and the four must not drift into disagreeing about it -- a project reported as `Unknown`
/// include but `Unknown` exclude "for a different reason" would be describing a state that cannot
/// happen. Callers supply only the two facts that genuinely differ between them: which directory
/// the project would have resolved globs against, and which config file (if any) it was.
fn unresolved_project(
    base_dir: PathBuf,
    config_path: Option<PathBuf>,
    defined_in: &Path,
    declared_at: usize,
    reason: String,
) -> Project {
    Project {
        name: None,
        base_dir,
        config_path,
        defined_in: defined_in.to_path_buf(),
        globals: GlobalsDetection::Unknown {
            reason: reason.clone(),
        },
        include: GlobPatterns::Unknown {
            reason: reason.clone(),
        },
        exclude: GlobPatterns::Unknown {
            reason: reason.clone(),
        },
        include_source: GlobPatterns::Unknown {
            reason: reason.clone(),
        },
        benchmark_include: GlobPatterns::Unknown {
            reason: reason.clone(),
        },
        benchmark_exclude: GlobPatterns::Unknown {
            reason: reason.clone(),
        },
        benchmark_include_source: GlobPatterns::Unknown {
            reason: reason.clone(),
        },
        typecheck_enabled: TypecheckAdmission::Unknown {
            reason: reason.clone(),
        },
        typecheck_include: GlobPatterns::Unknown {
            reason: reason.clone(),
        },
        typecheck_exclude: GlobPatterns::Unknown { reason },
        declared_at,
    }
}

/// Build a project from a config file, retaining unknown effective fields when static
/// resolution fails.
///
/// A refused `mergeConfig` base is recorded in `refusal`, as for [`build_inline_project`].
fn build_file_project(
    root: &Path,
    path: &Path,
    defined_in: &Path,
    declared_at: usize,
    refusal: &mut Option<String>,
) -> Project {
    let base_dir = path.parent().unwrap_or(root).to_path_buf();
    match resolve_standalone_config_within(root, path) {
        Some(fields) => {
            *refusal = fields.refusal;
            Project::from_effective(
                fields.name,
                base_dir,
                Some(path.to_path_buf()),
                defined_in,
                fields.effective,
                declared_at,
            )
        }
        None => {
            let reason = format!(
                "{} could not be read as a standalone project configuration (unreadable, \
                 malformed, or its `test` value is not an object literal)",
                path.display()
            );
            unresolved_project(
                base_dir,
                Some(path.to_path_buf()),
                defined_in,
                declared_at,
                reason,
            )
        }
    }
}

/// Resolves the base an `extends: '<path>'` string points at, other than the common
/// self-referencing case the caller already handles directly.
fn resolve_extends_target(root: &Path, candidate: &Path) -> Option<StandaloneConfigFields> {
    resolve_standalone_config_within(root, candidate)
}

/// Return the canonical path when available, otherwise preserve the supplied path.
fn canonical_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Builds one [`Project`] from an inline `test.projects`/workspace-array object entry
/// (`{ extends?, test: { ... } }`).
///
/// An `extends` that is refused, cannot be loaded, or is not a boolean or string literal is
/// recorded in `refusal`, besides reaching the fields it would have supplied: a project that sets
/// every such field itself would otherwise hide that its base was not read.
// Every parameter is a distinct piece of context with no natural grouping, as for
// `resolve_project_entry` below.
#[allow(clippy::too_many_arguments)]
fn build_inline_project(
    root: &Path,
    defining_config: &Path,
    masked: &[char],
    original: &[char],
    object_interior: Range<usize>,
    default_base: &EffectiveTestFields,
    declared_at: usize,
    refusal: &mut Option<String>,
) -> Project {
    let extends_pos =
        find_direct_key_value_starts_precise(masked, original, object_interior.clone(), "extends")
            .last()
            .copied();

    let (mode, base) = match extends_pos {
        None => (ExtendsMode::Inherit, default_base.clone()),
        Some(pos) => {
            let extent = value_extent(masked, pos, object_interior.end);
            match literal_bool_at(masked, pos, extent.end) {
                Some(true) => (ExtendsMode::Inherit, default_base.clone()),
                Some(false) => (ExtendsMode::NoInherit, default_base.clone()),
                None => match read_string_literal(original, extent) {
                    Some(target) => {
                        let defining_dir = defining_config.parent().unwrap_or(root);
                        let candidate = canonical_or_self(&defining_dir.join(&target));
                        if candidate == canonical_or_self(defining_config) {
                            (ExtendsMode::Inherit, default_base.clone())
                        } else if matches!(confine(root, &candidate), Confinement::Outside) {
                            let reason = format!(
                                "{} extends `{target}`, which resolves outside the analysis \
                                 root and was not read",
                                display_path(defining_config, root)
                            );
                            *refusal = Some(reason.clone());
                            (ExtendsMode::Unresolvable(reason), default_base.clone())
                        } else {
                            match resolve_extends_target(root, &candidate) {
                                Some(fields) => {
                                    *refusal = fields.refusal;
                                    (ExtendsMode::Inherit, fields.effective)
                                }
                                None => {
                                    let reason = format!(
                                        "{} extends `{target}`, which this scan could not load \
                                         and parse as a base configuration",
                                        display_path(defining_config, root)
                                    );
                                    *refusal = Some(reason.clone());
                                    (ExtendsMode::Unresolvable(reason), default_base.clone())
                                }
                            }
                        }
                    }
                    None => {
                        let reason = format!(
                            "{} sets `extends` to something other than a boolean or a string \
                             literal",
                            display_path(defining_config, root)
                        );
                        *refusal = Some(reason.clone());
                        (ExtendsMode::Unresolvable(reason), default_base.clone())
                    }
                },
            }
        }
    };

    let own_test_body = match locate_test_object(masked, object_interior) {
        TestObject::Found(open, close) => Some(open + 1..close),
        TestObject::Absent => None,
        TestObject::NonLiteral | TestObject::Malformed => {
            let reason = format!(
                "a `test.projects` entry in {} sets `test` to something this scan cannot read",
                display_path(defining_config, root)
            );
            return unresolved_project(
                root.to_path_buf(),
                None,
                defining_config,
                declared_at,
                reason,
            );
        }
    };

    let own_fields = match own_test_body {
        Some(body) => RawTestFields::read(masked, original, body),
        None => RawTestFields::empty(),
    };

    let effective = resolve_effective(&own_fields, &mode, Some(&base), defining_config);

    // S3 sub-mechanisms 1 and 2. An entry declaring `root:`/`dir:` is scoped to that
    // directory, and `base_dir` is precisely the thing `match_file` resolves this project's
    // include/exclude globs against -- so one assignment fixes both halves of the measured
    // defect at once:
    //
    // - An observed router package's shape: `root: './pkg'` with NO `include`, inheriting the
    //   defaults.
    //   Left at the analysis root, the inherited defaults ran repository-wide and swept in
    //   files the project never claimed. Over-inclusion is the observable half, and it is
    //   what the recall case measures (4 tests where 2 are correct).
    // - `nuxt/ui`'s shape: `dir: './pkg'` with `include: ['src/**/*.spec.ts']`. Matched from
    //   the analysis root that glob never reaches `pkg/src/`, so the project extracted
    //   nothing at all.
    //
    // A declared directory that resolves on disk is canonicalized: a value like
    // `./pkg/../pkg` resolves inside the analysis root, but `Path` comparisons are lexical, so
    // an unresolved `..` component could never `strip_prefix`-match the paths discovery reports
    // (which are built from the config's own canonicalized root) -- the project would silently
    // claim no files while the run stayed complete. Canonicalizing also matches how `root`
    // itself was canonicalized, so both sides of the later comparisons carry the same form.
    //
    // A directory that does NOT resolve keeps the lexical join, and `match_file`'s
    // `strip_prefix` fails closed on it exactly as before: a project whose declared directory
    // cannot be resolved is out of scope for this run either way, and this scan does not guess
    // which files such a path might have named. An absolute value joins to itself
    // (`Path::join`'s documented behavior), so it is handled by the same two arms.
    //
    // A single leading `./` is stripped for the same reason `compile_glob_set` and
    // `expand_project_path` strip theirs (S1): config authors write the prefix the way shell
    // users do, and `Path::join` keeps it as a `.` component that no later `strip_prefix`
    // matches.
    let base_dir = match own_fields.base.as_deref() {
        Some(declared) => {
            let joined = root.join(declared.strip_prefix("./").unwrap_or(declared));
            match confine(root, &joined) {
                Confinement::Inside(canonical) => canonical,
                Confinement::Missing(_) => joined,
                Confinement::Outside => {
                    let reason = format!(
                        "a `test.projects` entry in {} declares `root`/`dir` `{declared}`, which \
                         resolves outside the analysis root",
                        display_path(defining_config, root)
                    );
                    return unresolved_project(
                        root.to_path_buf(),
                        None,
                        defining_config,
                        declared_at,
                        reason,
                    );
                }
            }
        }
        None => root.to_path_buf(),
    };
    Project::from_effective(
        own_fields.name,
        base_dir,
        None,
        defining_config,
        effective,
        declared_at,
    )
}

/// Resolves one element of a `test.projects` (or workspace-file) array into zero or more
/// [`Project`]s -- zero when a glob string matches nothing, more than one when it expands to
/// several project directories/files.
#[allow(clippy::too_many_arguments)]
fn resolve_project_entry(
    root: &Path,
    defining_config: &Path,
    masked: &[char],
    original: &[char],
    element: Range<usize>,
    declared_at: usize,
    base: &EffectiveTestFields,
    budget: &mut Budget,
    diagnostics: &mut Vec<String>,
    projects: &mut Vec<Project>,
    has_unresolved: &mut bool,
    truncated: &mut bool,
    max_unresolved_position: &mut Option<usize>,
) {
    let i = skip_trivia(original, element.start, element.end);
    if i >= element.end {
        return;
    }

    if masked[i] == '{' || is_ident_char(masked[i]) {
        match locate_object_literal_or_call_argument(masked, i) {
            ConfigObject::Found(open, close) => {
                let mut refusal = None;
                projects.push(build_inline_project(
                    root,
                    defining_config,
                    masked,
                    original,
                    open + 1..close,
                    base,
                    declared_at,
                    &mut refusal,
                ));
                if let Some(reason) = refusal {
                    diagnostics.push(reason);
                    *has_unresolved = true;
                    note_unresolved_position(max_unresolved_position, declared_at);
                }
                return;
            }
            ConfigObject::Malformed => {
                diagnostics.push(format!(
                    "a `test.projects` entry in {} starts an object literal that never closes",
                    display_path(defining_config, root)
                ));
                *has_unresolved = true;
                note_unresolved_position(max_unresolved_position, declared_at);
                return;
            }
            ConfigObject::NoExport | ConfigObject::NonLiteral => {
                // Fall through: might still be a bare identifier that happens to start with a
                // letter but isn't a string literal either (handled by the final catch-all below).
            }
        }
    }

    if let Some(path_str) = read_string_literal(original, element) {
        if path_str.starts_with('!') || path_str.contains("!(") {
            diagnostics.push(format!(
                "a `test.projects` entry in {} uses glob negation (`{path_str}`), which this \
                 scan does not resolve",
                display_path(defining_config, root)
            ));
            *has_unresolved = true;
            note_unresolved_position(max_unresolved_position, declared_at);
            return;
        }
        let expanded = expand_project_path(root, &path_str, budget);
        if expanded.outside > 0 {
            diagnostics.push(format!(
                "a `test.projects` entry in {} (`{path_str}`) resolves outside the analysis \
                 root and was not read",
                display_path(defining_config, root)
            ));
            *has_unresolved = true;
            note_unresolved_position(max_unresolved_position, declared_at);
        }
        let resolved = expanded.paths;
        if resolved.is_empty() {
            if expanded.outside > 0 {
                return;
            }
            diagnostics.push(format!(
                "a `test.projects` entry in {} (`{path_str}`) matched no project configuration \
                 file",
                display_path(defining_config, root)
            ));
            // A literal path or glob that matches nothing is exactly as uncertain as any other
            // unresolved entry: it might have been intended to scope files this scan now leaves
            // ungoverned entirely, or -- via `declared_at` -- to out-rank a later-declared project
            // this same array does resolve. Silently leaving `has_unresolved` false here previously
            // let a config containing only such an entry look fully resolved while discovery
            // skipped every file it meant to cover.
            *has_unresolved = true;
            note_unresolved_position(max_unresolved_position, declared_at);
            return;
        }
        for path in resolved {
            if projects.len() >= MAX_PROJECTS {
                *has_unresolved = true;
                // `MAX_PROJECTS` was hit before every entry could be processed, which is exactly
                // what `truncated` documents. Only the per-entry loops above used to set it, so a
                // config whose single glob entry overflowed the cap reported `truncated: false`
                // -- and `main` then printed "the project map is incomplete" WITHOUT the
                // "(truncated at the project limit)" clause, dropping the one part a reader can
                // act on. Reproduced before fixing: 261 packages behind `projects: ['packages/*']`
                // gave `projects=256, has_unresolved=true, truncated=false`.
                *truncated = true;
                note_unresolved_position(max_unresolved_position, declared_at);
                return;
            }
            let mut refusal = None;
            projects.push(build_file_project(
                root,
                &path,
                defining_config,
                declared_at,
                &mut refusal,
            ));
            if let Some(reason) = refusal {
                diagnostics.push(reason);
                *has_unresolved = true;
                note_unresolved_position(max_unresolved_position, declared_at);
            }
        }
        return;
    }

    diagnostics.push(format!(
        "a `test.projects` entry in {} is neither a string literal nor an inline object this \
         scan can read",
        display_path(defining_config, root)
    ));
    note_unresolved_position(max_unresolved_position, declared_at);
    *has_unresolved = true;
}

/// Reads the `glob.sync('<literal>')` project-list shape at `value_start`, or `None`.
///
/// Accepts the value written directly as the call (`test: { projects: glob.sync('...') }`) or
/// through ONE const-binding hop (`const entries = glob.sync('...'); test: { projects: entries }`)
/// -- the two spellings measured on real corpora. The identifier must be `glob`, verified
/// imported from the `glob` package (the same discipline `is_vitest_config_import` applies to
/// Vitest's own exports), and the argument must be a single quoted string literal: a variable
/// pattern is a runtime value this scan cannot know, and it refuses exactly as before.
///
/// Returns the literal pattern text, for [`expand_project_path`] -- the same budgeted walk a
/// literal string entry takes, which is what makes this reading rather than executing.
fn read_glob_sync_literal(
    masked: &[char],
    original: &[char],
    value: Range<usize>,
) -> Option<String> {
    let value_start = value.start;
    let direct = glob_sync_argument(masked, original, value_start, value.clone());
    let pattern = match direct {
        Some(pattern) => pattern,
        None => {
            // One hop: the value must be a bare identifier, resolved to a `const NAME = ...`
            // initializer earlier in the file. Top-level only -- a binding inside a function
            // body is scoped to a call this scan never makes.
            let name = read_bare_identifier(original, value.clone())?;
            let initializer = find_const_initializer(masked, original, &name, value_start)?;
            glob_sync_argument(masked, original, initializer.start, initializer.clone())?
        }
    };
    if !is_imported_from_package(masked, original, "glob", &["glob"]) {
        return None;
    }
    Some(pattern)
}

/// The argument of a `glob.sync('<literal>')` call whose callee starts at `start`, or `None`.
fn glob_sync_argument(
    masked: &[char],
    original: &[char],
    start: usize,
    limit: Range<usize>,
) -> Option<String> {
    let text: String = masked[start..limit.end.min(masked.len())].iter().collect();
    let trimmed = text.trim_start();
    let callee_offset = start + (text.len() - trimmed.len());
    if !trimmed.starts_with("glob") {
        return None;
    }
    // `glob` must be its own token, followed by `.sync` and `(`.
    let after = &trimmed["glob".len()..];
    if !after.starts_with(".sync") {
        return None;
    }
    let open_paren = callee_offset + "glob".len() + ".sync".len();
    if masked.get(open_paren) != Some(&'(') {
        return None;
    }
    // The call's matching `)`, so the argument range handed to `read_string_literal` ends at it:
    // that function refuses any non-trivia after the closing quote, and a range running to
    // `limit.end` includes the `)` itself -- the probe-measured reason a first version of this
    // returned None on exactly this shape.
    let close_paren = find_matching_delim(masked, open_paren, '(', ')')?;
    // The single argument must be a quoted string literal followed (after trivia) by `)`.
    if close_paren >= limit.end || skip_trivia(original, close_paren + 1, limit.end) != limit.end {
        return None;
    }
    read_string_literal(original, open_paren + 1..close_paren)
}

/// A bare identifier occupying `range`, if nothing but trivia follows it on that line of input.
fn read_bare_identifier(original: &[char], limit: Range<usize>) -> Option<String> {
    let start = skip_trivia(original, limit.start, limit.end);
    let mut end = start;
    while end < limit.end && is_ident_char(original[end]) {
        end += 1;
    }
    if end == start {
        return None;
    }
    let name: String = original[start..end].iter().collect();
    // A keyword or member access is not a binding reference.
    if matches!(name.as_str(), "true" | "false" | "null" | "undefined") {
        return None;
    }
    if skip_trivia(original, end, limit.end) != limit.end {
        return None;
    }
    Some(name)
}

/// The value range of `const <name> = <initializer>` declared at top level in this file, or
/// `None`. Structural, over the masked view: every `const` token is checked for being its own
/// token and followed by the name and `=`.
fn find_const_initializer(
    masked: &[char],
    original: &[char],
    name: &str,
    use_at: usize,
) -> Option<Range<usize>> {
    for at in top_level_keyword_positions(masked, "const")
        .into_iter()
        .filter(|at| *at < use_at)
    {
        let after = at + "const".len();
        let ident_start = skip_trivia(original, after, use_at);
        let mut ident_end = ident_start;
        while ident_end < original.len() && is_ident_char(original[ident_end]) {
            ident_end += 1;
        }
        let ident: String = original[ident_start..ident_end].iter().collect();
        if ident != name {
            continue;
        }
        let eq = skip_trivia(original, ident_end, original.len());
        if masked.get(eq) != Some(&'=') {
            continue;
        }
        let init_start = skip_trivia(original, eq + 1, original.len());
        // The initializer runs to the end of the statement: the next `;` or newline at depth 0.
        let mut depth = 0i32;
        let mut end = init_start;
        while end < original.len() {
            match masked[end] {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ';' if depth == 0 => break,
                '\n' if depth == 0 => break,
                _ => {}
            }
            end += 1;
        }
        return Some(init_start..end);
    }
    None
}

/// Resolve the root config projects value into a project set, retaining diagnostics and
/// unresolved entries.
fn resolve_projects_array(
    root: &Path,
    config_path: &Path,
    masked: &[char],
    original: &[char],
    test_body: Range<usize>,
    base_effective: Option<&EffectiveTestFields>,
    budget: &mut Budget,
) -> ProjectSet {
    let mut diagnostics = Vec::new();
    let value_start =
        *find_direct_key_value_starts_precise(masked, original, test_body.clone(), "projects")
            .last()
            .expect("caller already confirmed `projects` is present");

    if value_start >= masked.len() || masked[value_start] != '[' {
        // B3: one computed shape is read rather than refused -- `glob.sync('<literal>')`, the
        // shape a large React monorepo's root config uses to declare its project list (575 of
        // 575 files undetermined before this, because the glob runs at config-evaluation time).
        // Reading it is NOT executing the config: the pattern is a string LITERAL, the `glob`
        // identifier is verified imported from the `glob` package, and the expansion is the
        // same budgeted filesystem walk a literal string entry already takes. Anything else --
        // a variable pattern, a different callee, an unverified `glob`, more than one binding
        // hop -- keeps today's refusal.
        if let Some(pattern) = read_glob_sync_literal(
            masked,
            original,
            value_extent(masked, value_start, test_body.end),
        ) {
            let expanded = expand_project_path(root, &pattern, budget);
            let outside = expanded.outside > 0;
            let mut refused = false;
            if outside {
                diagnostics.push(format!(
                    "{} sets `test.projects` to `glob.sync('{pattern}')`, which resolves \
                     outside the analysis root and was not read",
                    display_path(config_path, root)
                ));
            }
            let resolved = expanded.paths;
            if resolved.is_empty() {
                if !outside {
                    diagnostics.push(format!(
                        "{} sets `test.projects` to `glob.sync('{pattern}')`, which matched no \
                         project configuration file",
                        display_path(config_path, root)
                    ));
                }
                let mut set = unresolved_set(root, diagnostics, true);
                set.truncated = budget.exhausted();
                return set;
            }
            let mut projects = Vec::with_capacity(resolved.len());
            for path in resolved {
                if projects.len() >= MAX_PROJECTS {
                    diagnostics.push(format!(
                        "stopped resolving {}'s `glob.sync` project list at the {}-project cap; \
                         remaining matches were not processed",
                        display_path(config_path, root),
                        MAX_PROJECTS
                    ));
                    let mut set =
                        ProjectSet::resolved(root, projects, diagnostics, true, true, Some(1));
                    set.config_found = true;
                    return set;
                }
                let mut refusal = None;
                projects.push(build_file_project(
                    root,
                    &path,
                    config_path,
                    0,
                    &mut refusal,
                ));
                if let Some(reason) = refusal {
                    diagnostics.push(reason);
                    refused = true;
                }
            }
            // All matches originate at entry 0. An incomplete dynamic list can hide a later
            // matching project, so its unresolved position must out-rank every retained match.
            let exhausted = budget.exhausted();
            if exhausted {
                diagnostics.push(
                    "glob project-list filesystem budget exhausted; matches may be incomplete"
                        .to_owned(),
                );
            }
            let mut set = ProjectSet::resolved(
                root,
                projects,
                diagnostics,
                exhausted || outside || refused,
                exhausted,
                (exhausted || outside || refused).then_some(1),
            );
            set.config_found = true;
            return set;
        }
        diagnostics.push(format!(
            "{} sets `test.projects` to something other than an array literal; cannot \
             enumerate projects",
            display_path(config_path, root)
        ));
        return unresolved_set(root, diagnostics, true);
    }
    let Some(close) = find_matching_delim(masked, value_start, '[', ']') else {
        diagnostics.push(format!(
            "{}'s `test.projects` array literal never reaches a matching `]`",
            display_path(config_path, root)
        ));
        return unresolved_set(root, diagnostics, true);
    };

    // The base inline entries inherit under `extends: true`/absent: the root's own direct fields,
    // with nothing further to inherit (Vitest defaults fill any gap it leaves).
    let resolved_base = base_effective.cloned().unwrap_or_else(|| {
        let base_fields = RawTestFields::read(masked, original, test_body);
        resolve_effective(&base_fields, &ExtendsMode::NoInherit, None, config_path)
    });

    let elements = split_top_level(original, masked, value_start + 1..close);
    resolve_elements(
        root,
        config_path,
        &format!("{}'s `test.projects`", display_path(config_path, root)),
        masked,
        original,
        elements,
        &resolved_base,
        budget,
    )
}

/// Drives one projects-array resolution loop: the shared stop condition (the project-count or
/// discovery-budget ceiling), the diagnostic it emits, and the per-element handoff to
/// [`resolve_project_entry`]. The two array sources -- a config's inline `test.projects` and a
/// workspace file's default export -- differ only in which file declared the array, what base
/// fields entries inherit, and how the stop diagnostic names the array (`stop_subject`);
/// everything else about enumerating them is this loop, and it was two copies that could drift.
///
/// One collaborator per parameter and no natural grouping among them -- the same rationale
/// `resolve_project_entry` carries this allow for; this driver forwards that function's
/// own parameter family plus the two facts that vary between its two call sites.
#[allow(clippy::too_many_arguments)]
fn resolve_elements(
    root: &Path,
    defining_config: &Path,
    stop_subject: &str,
    masked: &[char],
    original: &[char],
    elements: Vec<Range<usize>>,
    base: &EffectiveTestFields,
    budget: &mut Budget,
) -> ProjectSet {
    let mut diagnostics = Vec::new();
    let mut projects = Vec::new();
    let mut has_unresolved = false;
    let mut truncated = false;
    let mut max_unresolved_position = None;
    for (declared_at, element) in elements.into_iter().enumerate() {
        if projects.len() >= MAX_PROJECTS || budget.exhausted() {
            diagnostics.push(format!(
                "stopped resolving {stop_subject} after {} projects / {} filesystem \
                 entries visited; remaining entries were not processed",
                projects.len(),
                budget.entries_visited
            ));
            has_unresolved = true;
            truncated = true;
            // Every element from here on (this one included) went unprocessed -- any of them
            // could plausibly govern a file that also matches an already-resolved project.
            note_unresolved_position(&mut max_unresolved_position, declared_at);
            break;
        }
        resolve_project_entry(
            root,
            defining_config,
            masked,
            original,
            element,
            declared_at,
            base,
            budget,
            &mut diagnostics,
            &mut projects,
            &mut has_unresolved,
            &mut truncated,
            &mut max_unresolved_position,
        );
    }

    ProjectSet::resolved(
        root,
        projects,
        diagnostics,
        has_unresolved,
        truncated,
        max_unresolved_position,
    )
}

/// Locates the array literal a legacy `vitest.workspace.*` file's `export default` resolves to:
/// [`locate_delimited_literal_or_call_argument`] for `[ ... ]` (a bare array, or a single wrapping
/// call like `defineWorkspace([ ... ])`).
///
/// Collapses `Malformed` into `None` alongside `NonLiteral`. Unlike the object case, which reports
/// an unclosed `{` as its own [`ConfigObject::Malformed`] outcome, this caller's single diagnostic
/// ("not an array literal this scan can read") covers both, so the distinction has nowhere to go.
fn locate_array_literal_or_call_argument(masked: &[char], start: usize) -> Option<(usize, usize)> {
    match locate_delimited_literal_or_call_argument(masked, start, '[', ']') {
        DelimitedLiteral::Found(open, close) => Some((open, close)),
        DelimitedLiteral::Malformed | DelimitedLiteral::NonLiteral => None,
    }
}

/// Resolve a standalone workspace file through the shared project-entry resolver, retaining
/// static-read diagnostics.
fn resolve_from_workspace_file(
    root: &Path,
    workspace_path: &Path,
    budget: &mut Budget,
) -> ProjectSet {
    let (masked, original) = match read_masked_source(workspace_path, root) {
        Ok(views) => views,
        Err(reason) => return unresolved_set(root, vec![reason], true),
    };

    let Some(value_start) = find_export_default_value_start(&masked) else {
        return unresolved_set(
            root,
            vec![format!(
                "{} has no `export default` this scan recognizes",
                display_path(workspace_path, root)
            )],
            true,
        );
    };
    let Some((open, close)) = locate_array_literal_or_call_argument(&masked, value_start) else {
        return unresolved_set(
            root,
            vec![format!(
                "{}'s default export is not an array literal this scan can read (e.g. \
                 `defineWorkspace([ ... ])`)",
                display_path(workspace_path, root)
            )],
            true,
        );
    };

    // A workspace-file entry has no root `test` object to inherit from -- see the module-level
    // "Known limitations".
    let no_base = EffectiveTestFields {
        globals: GlobalsDetection::Unknown {
            reason: format!(
                "{} is a workspace file; there is no root `test` configuration for an \
                 inheriting entry to extend",
                display_path(workspace_path, root)
            ),
        },
        include: GlobPatterns::Unknown {
            reason: "no base configuration to inherit `include` from".to_owned(),
        },
        exclude: GlobPatterns::Unknown {
            reason: "no base configuration to inherit `exclude` from".to_owned(),
        },
        include_source: GlobPatterns::Unknown {
            reason: "no base configuration to inherit `includeSource` from".to_owned(),
        },
        benchmark_include: GlobPatterns::Unknown {
            reason: "no base configuration to inherit `benchmark.include` from".to_owned(),
        },
        benchmark_exclude: GlobPatterns::Unknown {
            reason: "no base configuration to inherit `benchmark.exclude` from".to_owned(),
        },
        benchmark_include_source: GlobPatterns::Unknown {
            reason: "no base configuration to inherit `benchmark.includeSource` from".to_owned(),
        },
        typecheck_enabled: TypecheckAdmission::Unknown {
            reason: "no base configuration to inherit `typecheck.enabled` from".to_owned(),
        },
        typecheck_include: GlobPatterns::Unknown {
            reason: "no base configuration to inherit `typecheck.include` from".to_owned(),
        },
        typecheck_exclude: GlobPatterns::Unknown {
            reason: "no base configuration to inherit `typecheck.exclude` from".to_owned(),
        },
    };

    let elements = split_top_level(&original, &masked, open + 1..close);
    resolve_elements(
        root,
        workspace_path,
        &display_path(workspace_path, root),
        &masked,
        &original,
        elements,
        &no_base,
        budget,
    )
}

#[cfg(test)]
mod tests;
