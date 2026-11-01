//! Duplication analysis: turns a flat list of extracted [`TestCase`]s into [`Finding`]s.
//!
//! This module is deliberately a pure function over [`crate::model`]: it never touches the
//! filesystem or a parser. That boundary is the entire point of the IR, and it is load-bearing
//! rather than cosmetic: it is what keeps the oxc bet reversible, and what makes this layer
//! testable without a parser at all -- every rule here is exercised in `#[cfg(test)]` from
//! hand-built `TestCase` values, the same fixtures the real frontend will one day produce.
//!
//! # Rules
//!
//! - [`crate::model::Rule::DuplicateTest`]: same alpha-normalized body under an equivalent
//!   [`Context`] **and** the same semantically-significant [`Modifier`]s, scoped to a single file.
//!   Two identical bodies under different `describe` blocks with no hooks are still duplicates
//!   (see `fixtures/duplicates/duplicates-different-suites-no-hooks.test.ts`); differing hook,
//!   mock, fixture, or import context is what actually blocks the comparison, per
//!   [`Context::is_equivalent`]. Cross-file matches are deliberately out of scope here -- see
//!   "File scope" below. Two `TestCase`s that share their declaration [`Location`] (every row of
//!   one `test.each`/`test.for` table) are excluded from this rule entirely -- see "One
//!   declaration, many rows" below.
//! - [`crate::model::Rule::DuplicateTestName`]: same [`TestCase::full_title`] within one file,
//!   independent of body content. Also excludes same-`Location` members, for the same reason.
//! - [`crate::model::Rule::ParameterizationCandidate`]: equal `body.structural` (same shape) but
//!   **differing** `body.alpha` (different literal values), under the same equivalent `Context`,
//!   modifiers, and file scoping `duplicate-test` requires. This is [`crate::model::Verdict::Consolidatable`],
//!   not [`crate::model::Verdict::Redundant`]: both cases are genuinely needed, so the remedy is a
//!   `test.each` table that keeps every row, never deletion. Two boundary values are
//!   `consolidatable` and not `redundant` for that reason, and there is no static distinction
//!   between an "ordinary" and a "boundary" literal to lean on anyway -- `99` and `100` straddle a
//!   business rule exactly as `0` and `1` do, so the claim has to be "propose a consolidation that
//!   preserves every case", never "detect boundary values". Equal-`alpha` members of an
//!   equal-`structural` group are `duplicate-test`'s claim, not this rule's, so groups are
//!   collapsed to one representative per distinct `alpha` before pairing -- see
//!   `representative_per_alpha`. Also excludes same-`Location` members, for the same reason as
//!   the other two rules, and additionally requires a minimum of
//!   `MIN_BEHAVIOR_EVENTS_FOR_PARAMETERIZATION` recorded behavior events, since `structural`
//!   alone can key-match two bodies with no comparable substance.
//! - [`crate::model::Rule::NearDuplicateTest`]: same equivalent `Context`, same file, differing
//!   `body.alpha`, but the **sorted multiset** of `body.behavior` events is equal. The ordered
//!   traces may differ because setup was reordered, or may agree because a supported dataflow
//!   normalization sees through two spellings of the same work. The finding message distinguishes
//!   those cases instead of claiming every match reordered statements.
//!   Multiset equality is an exact-match gate, not a similarity score:
//!   any single differing event -- a different callee, a different assertion polarity, a
//!   different literal value baked into an assertion's `value=` shape -- makes the multisets
//!   unequal, so the pair produces no finding at all. That is the `distinct` verdict -- compared
//!   and found different -- which must never be conflated with `not-compared`, the exclusion that
//!   produces no finding because nothing was ever compared; both are silent, and treating them as
//!   one is how a rule that silently analyzes nothing passes a precision gate. This is also what
//!   makes `fixtures/duplicates/near-boilerplate-gradient.test.ts` hold at every gradient without
//!   any tuning: a multiset is unaffected by how much *identical* boilerplate surrounds a
//!   differing event, but it is never equal when even one event differs, so the differing region
//!   -- however small a fraction of the body it is -- always dominates the decision instead of
//!   being diluted by shared setup. See `MIN_BEHAVIOR_EVENTS_FOR_NEAR_DUPLICATE` for the
//!   substance floor and `representative_per_alpha` for why equal-`alpha` members never reach
//!   this rule (that pair is `duplicate-test`'s claim). Known gap, not yet bridged: two bodies
//!   whose only difference is in statements that record *no* behavior event at all (for example
//!   two differently-valued, never-read local declarations) would have equal multisets despite
//!   differing `alpha`;
//!   `fixtures/duplicates/behavior-depth-truncation.test.ts`'s shallow retention pair exercises
//!   exactly that shape, deliberately, as the positive control beside the truncated pair.
//!
//!   **Clustering and non-transitive similarity.** Similarity in general is not transitive (A can
//!   resemble B and B resemble C while A and C have incompatible assertions), so a rule that
//!   clusters by "shares an edge with a similar member" would risk asserting an equivalence
//!   between A and C that was never checked, and a cluster must never imply all three members are
//!   interchangeable.
//!   This rule does not have that risk, and not by accident: its grouping key
//!   (`NearDuplicateKey`) is *exact* equality on a canonical value (path, equivalent context,
//!   equivalent modifiers, sorted `behavior` multiset), grouped through `pairs::group_and_order`'s
//!   `BTreeMap` partition -- see that module's doc comment. Two tests land in one group iff their
//!   keys are literally equal, and struct/string equality is transitive by construction, so every
//!   member of a resulting cluster is already known to be pairwise key-equal to every other member
//!   -- verified by the grouping itself, never assumed from a chain of partial matches.
//!   `fixtures/duplicates/near-non-transitive-cluster.test.ts` pins this: its A/B/C are "near" each
//!   other only in a loose, narrative sense (an extracted local, a reordered setup with an
//!   intervening mutation), but none of the three pairs shares an exact
//!   behavior multiset -- a different callee for every pair, and opposite assertion polarity for
//!   B/C -- so the rule links none of them, and the non-transitive triple it set out to stress
//!   cannot arise here. A fuzzy similarity score would not get this for free; an exact-match key
//!   does.
//! - [`crate::model::Rule::CrossFileDuplicate`]: `duplicate-test`'s own equality key (alpha body,
//!   equivalent context, equivalent modifiers) with the file-scoping restriction lifted, so
//!   groups can span every file in the run instead of one. To guarantee a pair is never reported
//!   under both rules, this rule never compares two tests in the *same* file: each equality group
//!   is collapsed to one representative per distinct file (`representative_per_file`) before
//!   pairing, so every emitted pair spans a file boundary by construction and same-file relatives
//!   stay `duplicate-test`'s to report (whether or not that rule happens to be enabled). Off by
//!   default -- duplication across unit and integration layers is frequently intentional.
//!
//! # File scope
//!
//! `duplicate-test` groups only within a single file. Cross-file matching is a *separate*,
//! off-by-default rule ([`crate::model::Rule::CrossFileDuplicate`]), because duplication across
//! unit and integration layers is frequently intentional; it belongs with the `oxc_resolver`-based
//! cross-file work (helper-body resolution, `duplicate-suite`) rather than inside this rule.
//! Grouping across files here at Error severity with no file gate would have made every unrelated
//! cross-directory coincidence a build-breaking finding -- which is why the two claims stay
//! separate rules with separate defaults, instead of one rule quietly reporting both at the
//! stricter severity.
//!
//! # One declaration, many rows
//!
//! [`crate::frontend::extract`] emits one [`TestCase`] per row of a `test.each`/`test.for` table,
//! and every row of one table shares the declaration's single call-site [`Location`] (see
//! `fixtures/each/`). When a table's rows also alpha-normalize to the same body under the same
//! `Context` -- the common case, since the row values are usually the only thing that varies --
//! naive equality-key grouping puts all N rows in one group and reports the table as a duplicate
//! of itself. That is not two copy-pasted tests; it is one declaration read N times. Both rules
//! therefore collapse each equality group down to one representative per distinct `Location`
//! before deciding whether a group is even worth reporting (see `dedup_by_declaration`) --
//! deliberately, as a statement about what "duplicate" means for this rule, not as an incidental
//! filter. `duplicate-test-name` needs this too: when a table's title pattern does not reference
//! any row data (or the table can't be statically read at all, see
//! [`crate::model::EachTable::rows`]), every row renders the identical
//! [`crate::model::Title`], which is the exact same self-duplication hazard wearing that rule's
//! clothes instead.
//!
//! # Grouping and pair emission
//!
//! Each equality-keyed rule groups test cases by its own exact key, then hands each group of 2+
//! members to `pairs::group_outcome`: groups of up to four are reported as a linear spanning set
//! of pair findings, larger groups collapse into one cluster finding. See `src/pairs.rs`
//! for the rationale (mirrors CukeDedup's `analysis::pairs`).
//!
//! The parts of that pipeline that are the *same* for every rule live in one place each, so they
//! cannot drift apart between rules: which tests are even eligible
//! (`is_body_comparison_candidate`), budget accounting and the spanning/cluster decision
//! (`emit_group_findings`), the whole per-group loop (`emit_collapsed_groups`), and `Finding`
//! assembly (`finding_over`). What stays at each rule's own site is what makes it a *different*
//! claim: its equality key, the collapse it applies to a group before certifying it, and its
//! message -- each with the reasoning for it. `subsumed-test` and `redundant-hook` share only the
//! lower layers, for reasons given on their own functions.
//!
//! # Unresolved contexts
//!
//! A test whose [`Context::unresolved`] is set (`context.is_comparable()` is `false`) is excluded
//! from every rule up front, the same way a rule turned `Off` is skipped before any work is done
//! rather than computed and discarded. An ambient fact the extractor could not read is not
//! evidence of anything, so it never anchors a finding under either rule -- and its exclusion
//! unconditionally marks [`AnalysisResult::incomplete`], since absence of a finding for it proves
//! nothing.
//!
//! # Bounded work
//!
//! [`MAX_PAIR_COMPARISONS`] caps the total "duplicate relationship" work spent turning groups into
//! findings, shared across the rules in one `analyze` call. [`MAX_HOOK_PAIR_SCANS`] separately
//! caps the candidate pairs that `redundant-hook` inspects before deduplication, and
//! [`MAX_FINDINGS`] caps how many findings are retained in the final, sorted report. A completed
//! run has the full output; hitting any limit returns a deterministic partial result and sets
//! [`AnalysisResult::incomplete`] so analysis never silently truncates.
//!
//! # Baseline and changed-file modes
//!
//! [`RunModes`] narrows what counts against the quality gate, never what is analyzed. It is
//! applied inside [`analyze_with_modes`], between rule evaluation and the capture of
//! [`AnalysisResult::had_error_finding`] / [`AnalysisResult::error_finding_locations`], because
//! those two values *are* the gate and are deliberately captured before presentation truncation.
//! Applying the modes outside this module would give a caller a report and an exit code that
//! disagreed. See `crate::modes` for what each mode means and why `--changed-since` filters
//! findings rather than restricting the corpus.

#[cfg(test)]
mod tests;

use crate::cli::ExitCode;
use crate::config::Config;
use crate::model::{
    fingerprint_hash, AnalysisResult, BlindFile, BodyDigest, CallbackForm, Context,
    DeclarationKind, EventDigest, Finding, Fingerprints, FixtureBinding, Hook, HookFingerprint,
    HookGuard, HookKind, IncompleteCauses, Location, MockedModule, ModeSummary, Modifier, Rule,
    Severity, TestCase, TestFingerprint, Title,
};
use crate::modes::{self, Baseline};
use crate::pairs::{group_and_order, group_outcome, GroupOutcome, PairBudget, SPANNING_GROUP_MAX};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The baseline and changed-file modes in force for one [`analyze`] call.
///
/// Passed *into* analysis rather than applied to its result afterwards, and that placement is the
/// whole point: [`AnalysisResult::had_error_finding`] and
/// [`AnalysisResult::error_finding_locations`] -- the quality gate's two inputs -- are captured
/// before presentation truncation, so anything that changes which findings count against the gate
/// has to happen before that capture. A caller that filtered `result.findings` after `analyze`
/// returned would have a report that said one thing and an exit code that said another, and the
/// exit code would win.
///
/// Holds borrows rather than owned values because both are read-only here and are owned by the
/// orchestrator, which also needs them to write its own diagnostics.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunModes<'a> {
    /// When set, only findings with at least one participant in this set are reported. The set is
    /// paths relative to the analysis root, the same form [`crate::model::Location::path`] uses.
    ///
    /// This never narrows what is *analyzed* -- see [`crate::modes`]'s module documentation for
    /// why analyzing only the changed files would make `cross-file-duplicate` silently unsound.
    pub changed_files: Option<&'a BTreeSet<PathBuf>>,
    /// When set, findings this baseline accounts for are marked
    /// [`Finding::baselined`](crate::model::Finding::baselined) and excluded from the gate.
    pub baseline: Option<&'a Baseline>,
    /// Inline suppression directives found in the analyzed sources (usually `&[]`). Applied
    /// before every mode below -- see [`crate::modes::apply_inline_suppressions`] for the
    /// precedence this establishes.
    pub suppressions: &'a [crate::modes::InlineSuppression],
}

/// Maximum total pairwise "duplicate relationship" work performed across every rule in one
/// [`analyze`] call, before analysis stops early and marks the result incomplete.
///
/// Charged for each relationship a rule is ready to report: one unit per emitted spanning pair,
/// or `participants.len() - 1` for a cluster (see `pairs::PairBudget`). A finding suppressed as
/// a redundant restatement costs nothing. Two million is the same order of magnitude CukeDedup
/// budgets for its (much more expensive, fuzzy) pairwise similarity work; here the per-unit cost
/// is a cheap key comparison plus a small `Finding` allocation, so this ceiling comfortably
/// covers any real monorepo while still being small enough that a pathological input (for example,
/// one file containing a million copies of the same test) fails fast and visibly instead of
/// hanging.
pub const MAX_PAIR_COMPARISONS: u64 = 2_000_000;

/// Maximum hook-pair candidates inspected by `emit_redundant_hook`.
///
/// This is separate from [`MAX_PAIR_COMPARISONS`]: the latter is charged for relationships a
/// rule is ready to report, while `redundant-hook` must first inspect the hook chain attached to
/// every test and deduplicate the matching declaration pairs it finds. Charging only while
/// emitting left that scan unbounded -- a large number of tests under a long shared hook chain
/// could do all of its quadratic work before the first budget check. This budget is charged at
/// the top of the inner loop, before even the cheap kind/body comparisons, so it bounds the work
/// it names rather than accounting for it after the fact.
pub const MAX_HOOK_PAIR_SCANS: u64 = 2_000_000;

/// Maximum number of findings retained in one [`AnalysisResult`].
///
/// A report with more than ten thousand findings is not something any consumer -- terminal,
/// JSON, or CI gate -- can usefully act on; keeping every one of them in memory and in every
/// reporter's output only makes a already-degraded run slower to look at. Exceeding this marks
/// the result incomplete rather than silently dropping findings with no signal.
pub const MAX_FINDINGS: usize = 10_000;

/// A hashable-free stand-in for the fields [`Context::is_equivalent`] actually compares.
///
/// The two are held in step by a test, not by this sentence:
/// `the_grouping_key_agrees_with_context_equivalence_on_every_pair` asserts that they answer
/// identically for every pair of a context matrix, so a field added to one and forgotten in the
/// other fails. Neither can delegate to the other -- grouping needs a hashable KEY whose
/// canonical JSON is a published record, while `is_equivalent` is a pairwise PREDICATE -- so the
/// duplication is deliberate, and that test is what makes it safe.
///
/// `Context` itself derives structural `Eq`/`Ord` (via `#[derive]` in `model.rs`), but that
/// compares `Hook::location` and every `Fingerprints` field too -- so two contexts built from
/// textually-different-but-equivalent hooks (different location, same `body.alpha`) would never
/// compare equal, defeating grouping entirely. This key instead captures exactly what
/// `is_equivalent` checks: `mocked_modules` as-is (each `MockedModule` already carries both the
/// specifier and the factory's alpha fingerprint, so two `vi.mock` calls for the same specifier
/// with different factories compare unequal, while two automocks of the same specifier -- both
/// `MockFactory::Automock` -- correctly compare equal, per `MockedModule`'s own doc comment),
/// `fixtures` as-is (each `FixtureBinding` likewise carries the fixture's name plus a
/// `FixtureValue`, with the same "two ordinary values of the same kind are equal" shape: a plain
/// `Literal` compares equal to another `Literal` with the same fingerprint), `imports` as-is
/// (the model documents `mocked_modules`/`fixtures`/`imports` as already sorted, and
/// `is_equivalent` compares them positionally, so this key must not re-sort them), plus each
/// hook's `(kind, body.alpha)` in order.
///
/// Deliberately does *not* include `Context::unresolved`: an unresolved context is filtered out
/// entirely before grouping (see `emit_duplicate_test` / `emit_duplicate_test_name`), so it never
/// reaches this key. `is_equivalent` itself stays purely structural and reflexive on purpose --
/// see `Context::unresolved`'s doc comment in `model.rs` for why "unknown never matches" is
/// enforced by an up-front filter rather than by a field that compares unequal to itself, which
/// would break the `Eq` this key's `BTreeMap` grouping relies on.
///
/// `MockFactory::Unresolved` and `FixtureValue::Unresolved` are explicit variants, not an
/// `Option::None` reused as a wildcard (`model.rs` documents this: a prior `Option<String>` shape
/// conflated "automocked"/"a plain literal" with "we could not read this" -- the same class of bug
/// `EachTable::rows` used to have before its `Unreadable` variant was anchored to the table
/// expression's own identity, see `ModifierKey`'s doc below).
/// Producing either `Unresolved` variant **must** also set `Context::unresolved` at the frontend,
/// per each variant's own doc comment, so by the time a `Context` reaches this key it has either
/// already been filtered out by `is_comparable`, or it contains no `Unresolved` value at all.
/// Equality on those variants is therefore never load-bearing here -- derived `Eq` correctly
/// treats two `Unresolved`s as equal to each other, but no such comparison ever affects a finding.
///
/// `Serialize` is derived for exactly one purpose: [`ContextKey::digest`], which publishes a hash
/// of this key without publishing the key. The struct itself stays private, so the precise set of
/// ambient facts the rules compare remains an implementation detail rather than a schema anyone
/// can depend on.
/// # Borrowed, not owned
///
/// Every field here borrows out of the `Context` it was built from, and every key type below
/// borrows the same way. The keys are build-once, compare-many values living in one
/// [`group_and_order`] map that is dropped before the next rule runs, and the `TestCase` slice
/// they point into outlives all of them -- so cloning bought nothing and cost a second copy of
/// every hook body, import list, and mock table, once per rule, at the exact moment the process
/// is already holding the whole corpus. On a 24k-test corpus that duplication was ~215 MB, most
/// of the gap between post-extraction and peak RSS.
///
/// Borrowing changes no comparison: `&str` orders as `str` does, `&[T]` as `Vec<T>` does, and
/// `Serialize` on a borrowed field emits exactly what the owned field emitted -- which is what
/// keeps [`ContextKey::digest`]'s published hash byte-identical.
/// `skip_serializing_if` predicate for [`ContextKey`]'s borrowed slice fields.
///
/// Serde passes a reference to the field, so the parameter is a double reference; `<[T]>::is_empty`
/// cannot be named directly for that.
fn slice_is_empty<T>(slice: &&[T]) -> bool {
    slice.is_empty()
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct ContextKey<'a> {
    mocked_modules: &'a [MockedModule],
    fixtures: &'a [FixtureBinding],
    imports: Vec<&'a str>,
    /// Module-scope awaited setup calls; see [`Context::module_setup`].
    ///
    /// SKIPPED WHEN EMPTY, and the skip is the contract rather than a tidiness choice. This
    /// struct's canonical JSON is published as [`crate::model::TestFingerprint::context_key`], so
    /// adding a field that always serializes moves every digest a consumer has already recorded
    /// -- including for tests that have none of what the field describes. That happened twice,
    /// silently, with `JSONL_SCHEMA_VERSION` still at "0.1": `module_setup: []` moved it from
    /// `24e7184815de5034` to `48330a74e83d00fc`, and `config_scope: null` moved it again to
    /// `5a74ce87a73231da`, all three measured on the same single-test corpus. Absent and empty are
    /// the same fact here, so skipping restores the pre-existing digest for every context that
    /// carries neither -- which is what both fields' own docs promise.
    #[serde(skip_serializing_if = "slice_is_empty")]
    module_setup: &'a [String],
    /// The per-directory config scope; see [`Context::config_scope`]. Skipped when absent, for the
    /// reason given on `module_setup`.
    #[serde(skip_serializing_if = "Option::is_none")]
    config_scope: Option<&'a str>,
    /// Each hook's `(kind, alpha text)`. The **text**, via [`Hook::alpha_source`], not
    /// [`Fingerprints::alpha`]'s digest: this struct's canonical JSON is published as
    /// [`TestFingerprint::context_key`], so what goes in here is a published-record contract, not
    /// a free choice. Equality on the text and equality on its digest agree, so grouping is
    /// unaffected either way.
    hooks: Vec<ContextHook<'a>>,
}

/// One hook of [`ContextKey`], in its published form.
///
/// Untagged on purpose, and the shape is a record contract: an unguarded hook serializes as the
/// same `(kind, alpha)` array every `contextKey` already recorded carries, so adding hook guards
/// moved nothing that exists. A hook written under a control-flow guard serializes the guard list
/// as well, because two hooks whose guards differ are different ambient facts -- a `beforeEach`
/// registered under `if (true)` and one under `if (false)` do not run for the same tests, and
/// grouping their tests together is exactly the error-severity false positive hook guards exist
/// to refuse (see [`crate::model::HookGuard`]).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(untagged, rename_all = "camelCase")]
enum ContextHook<'a> {
    Plain(HookKind, &'a str),
    Guarded {
        kind: HookKind,
        alpha: &'a str,
        guards: &'a [HookGuard],
    },
}

impl<'a> ContextHook<'a> {
    /// The published form of one hook: the plain tuple for an unguarded registration, the
    /// guard-carrying object otherwise (see the enum's own contract).
    fn of(hook: &'a crate::model::Hook) -> Self {
        if hook.guards.is_empty() {
            Self::Plain(hook.kind, &hook.alpha_source)
        } else {
            Self::Guarded {
                kind: hook.kind,
                alpha: &hook.alpha_source,
                guards: &hook.guards,
            }
        }
    }
}

impl<'a> ContextKey<'a> {
    /// Borrows the published facets of one [`Context`] -- including each hook's guard list, so
    /// a guarded registration keys differently from its unguarded twin.
    fn new(context: &'a Context) -> Self {
        Self {
            mocked_modules: &context.mocked_modules,
            fixtures: &context.fixtures,
            imports: context.imports.iter().map(|import| &**import).collect(),
            module_setup: &context.module_setup,
            config_scope: context.config_scope.as_deref(),
            hooks: context.hooks.iter().map(ContextHook::of).collect(),
        }
    }

    /// A hash of this key, for [`TestFingerprint::context_key`].
    ///
    /// Equal digests mean equal keys, which is exactly the relation the rules above test with
    /// `==`: a consumer comparing two records' `contextKey` learns the same thing `emit_*` learns
    /// from grouping on this type, and nothing else about it.
    fn digest(&self) -> String {
        fingerprint_hash(&canonical_json(self))
    }
}

/// The subset of a `TestCase`'s `modifiers` that changes what the test *does*, for `duplicate-test`.
///
/// `Modifier::Each` (table data), `SkipIf`/`RunIf` (their normalized condition source), and
/// `Unknown` (a chain this extractor version does not model, and therefore cannot rule out as
/// behavior-changing) are included: two `test.each` tables, or two opposite run conditions, are
/// different behavior even when the callback body and `Context` are byte-for-byte identical -- see
/// the `EMAILS`/`PHONES` and `skipIf(isWindows)`/`skipIf(isLinux)` examples this key exists to
/// stop merging.
///
/// `Skip`, `Only`, `Todo`, `Concurrent`, `Sequential`, and `Shuffle` are deliberately excluded:
/// every one of them changes *whether, when, or in what order* a test runs, never *what outcome
/// counts as correct*, so two tests differing only in one of these are still the same behavior
/// duplicated -- and `fixtures/duplicates/duplicates-one-skipped.test.ts` requires exactly that: a
/// `.skip` test must still be reported as a duplicate of its non-skipped twin. Including
/// `Skip`/`Todo` here would silently break that fixture by making the inert twin incomparable to
/// the live one.
///
/// `Fails` is *included*, and is the one member of that chain family that is. `.fails` inverts the
/// pass condition: the plain test passes only when its body throws nothing, its `.fails` twin
/// passes only when the body *does* throw. Two byte-identical bodies under opposite polarity
/// therefore assert opposite things and are not duplicates -- reporting them as one was an
/// Error-severity false positive, pinned by
/// `fixtures/duplicates/not-duplicates-fails-modifier.test.ts`. The distinction that puts `Fails`
/// on this side of the line and `Skip` on the other is *runs or not* versus *which result is the
/// correct one*; only the second is behavior.
///
/// Order is preserved (not sorted) and both members must match exactly: `test.each(a).skipIf(b)`
/// and `test.skipIf(b).each(a)` are treated as different keys. This is a conservative choice --
/// false negative (two behaviorally-identical tests reported as not duplicates) rather than false
/// positive -- since nothing in the IR currently proves chain order is semantically irrelevant.
///
/// `Each`'s payload, `EachTable`, derives structural equality including `rows: EachRows`, and
/// `EachRows::Unreadable` carries an identity for the table *expression* (a resolved
/// symbol, or failing that its own source shape), so two different unreadable tables no longer
/// compare equal under plain `==` the way two `rows: None` values used to. That field being
/// honest does not by itself make an unreadable table safe to compare, though: this module still
/// cannot vouch for a table it could not read the *values* of, so the frontend continues to set
/// `Context::unresolved` whenever it emits an `EachTable` it could not enumerate, and an
/// unresolved context is filtered out of every rule before grouping (`Context::is_comparable`) --
/// this key never actually receives two survived-to-comparison unreadable tables to conflate,
/// honest or not. `duplicate-each-row`'s own none-never-matches guard is therefore still doing
/// real work, not redundant: `is_comparable` is what keeps an unreadable table out of comparison,
/// and this key's now-honest equality is a second, independent reason it would fail to falsely
/// match one to another even if it somehow arrived here anyway.
///
/// `Serialize` is derived for [`ModifierKey::digest`] only, on the same terms as [`ContextKey`]'s:
/// the hash is public, the type is not.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct ModifierKey<'a>(Vec<&'a Modifier>);

impl<'a> ModifierKey<'a> {
    /// A hash of this key, for [`TestFingerprint::modifier_key`]. See [`ContextKey::digest`].
    fn digest(&self) -> String {
        fingerprint_hash(&canonical_json(self))
    }

    fn new(modifiers: &'a [Modifier]) -> Self {
        Self(
            modifiers
                .iter()
                .filter(|modifier| {
                    matches!(
                        modifier,
                        Modifier::Each(_)
                            | Modifier::SkipIf(_)
                            | Modifier::RunIf(_)
                            | Modifier::Fails
                            | Modifier::Unknown(_)
                    )
                })
                .collect(),
        )
    }
}

/// Equality key for `duplicate-test`: same file, alpha-normalized body, equivalent context, and
/// equivalent semantically-significant modifiers.
///
/// `path` scopes this rule to a single file per A7 -- see the module-level "File scope" section.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DuplicateTestKey<'a> {
    path: &'a Path,
    alpha: BodyDigest,
    context: ContextKey<'a>,
    modifiers: ModifierKey<'a>,
}

/// Equality key for `duplicate-test-name`: same file, same full title.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DuplicateNameKey<'a> {
    path: &'a Path,
    full_title: String,
}

/// Equality key for `duplicate-suite`: same file, same suite path and own title (so only a copied
/// block *under matching nesting* aggregates -- two different suites holding identical tests do
/// not), and an alpha-equal whole-block body under an equivalent context -- the same proof
/// [`DuplicateTestKey`] rests on, one level up.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DuplicateSuiteKey<'a> {
    path: &'a Path,
    suite_path: &'a [crate::model::Title],
    title: &'a crate::model::Title,
    alpha: BodyDigest,
    context: ContextKey<'a>,
}

/// Candidate key for `subsumed-test`: same file, equivalent context. Deliberately coarser than
/// the duplication keys -- subsumption is a *directional* relation between behavior event lists,
/// not an equivalence, so there is nothing narrower to key on before the pairwise check.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SubsumedKey<'a> {
    path: &'a Path,
    context: ContextKey<'a>,
    /// See [`NearDuplicateKey::callback_form`]. Subsumption is a subset relation over the same
    /// events, so it is exposed to the same blindness and needs the same partition.
    callback_form: CallbackForm,
}

/// Equality key for `parameterization-candidate`: same file, equivalent context, equivalent
/// semantically-significant modifiers, and equal `body.structural` -- everything
/// [`DuplicateTestKey`] requires except that this key deliberately uses `structural` instead of
/// `alpha`. `structural` is `alpha` with literal *values* collapsed to their *kind* (`model.rs`),
/// so two bodies keyed equal here share shape but may differ in the literals that shape carries --
/// exactly the "same test, different data" signal `test.each` exists to express. Requiring `alpha`
/// too would just re-derive `DuplicateTestKey` and could never observe a differing-alpha pair,
/// which is the entire reason this is a second key rather than a filter on the first one's groups.
///
/// `path` scopes this rule to a single file, matching `duplicate-test`'s own file scoping (see the
/// module-level "File scope" section): this rule was specified as a within-file consolidation and
/// never as a cross-file one, and the message it emits ("consolidate into one `test.each` table")
/// is only true within one file.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ParameterizationKey<'a> {
    path: &'a Path,
    structural: BodyDigest,
    context: ContextKey<'a>,
    modifiers: ModifierKey<'a>,
}

/// Minimum number of recorded [`crate::model::Fingerprints::behavior`] events a test must carry to
/// seed a `parameterization-candidate` group.
///
/// [`crate::model::Fingerprints::trivial`] already screens out genuinely empty/stub bodies (see
/// its own doc comment), but a body can be non-trivial and still carry nothing for *this* rule to
/// act on: `structural` records shape, not substance, so a bare expression statement with no
/// assertion, call, or control flow (`() => { 5 }`) would still key-match another equally bare
/// body while having nothing a `test.each` table could usefully consolidate. `behavior` is the
/// field that already distinguishes "shape" from "did something recognizable" -- it records
/// assertions, calls, awaits, and control flow (`model.rs`) -- so a floor of one recorded event is
/// the cheapest defensible substance check: it excludes the zero-behavior case above while staying
/// well below what any real assertion costs. A single `expect(f(0)).toBe(false)`-shaped body
/// records at least one event (the assertion itself), so this floor does **not** suppress the
/// boundary-value case this rule exists to report -- see
/// `fixtures/duplicates/duplicates-parameterization-boundary-values.test.ts`, which is exactly
/// that shape and is required to fire. Set any higher, this constant would start rejecting real
/// single-assertion `test.each` candidates, which is precisely the volume this rule exists to
/// find: it was the cheapest real near-duplication rule available -- `structural` was already
/// computed for other reasons, the grouping machinery already existed, and the whole rule is a
/// second grouping pass over that key -- and a rule that only fired on multi-assertion bodies
/// would quietly narrow that scope with no test able to tell the difference from "the rule
/// doesn't work".
const MIN_BEHAVIOR_EVENTS_FOR_PARAMETERIZATION: usize = 1;

/// Minimum number of recorded [`crate::model::Fingerprints::behavior`] events a test must carry to
/// seed a `near-duplicate-test` candidate under the reordered-setup transformation.
///
/// Unlike [`MIN_BEHAVIOR_EVENTS_FOR_PARAMETERIZATION`], this floor is 2, not 1: "reordered setup"
/// is meaningless for a body with fewer than two events -- there is nothing to reorder. A
/// single-event body clearing this floor would mean two syntactically different bodies produced
/// the exact same one-element multiset, which is really "these two bodies are behaviorally
/// identical apart from something this rule cannot see" rather than a reordering, so it is
/// excluded rather than risking a low-substance match. `MIN_BEHAVIOR_EVENTS_FOR_PARAMETERIZATION`
/// stays at 1 because that rule keys on `structural` (shape), where a single assertion event is
/// already the whole comparable shape; this rule keys on the behavior multiset itself, where a
/// reordering claim needs at least two elements to reorder.
pub(crate) const MIN_BEHAVIOR_EVENTS_FOR_NEAR_DUPLICATE: usize = 2;

/// Runs every enabled duplication rule over `tests` and returns a fully sorted, deterministic
/// report.
///
/// `tests` is consumed rather than borrowed because nothing about the analysis needs to hand
/// individual `TestCase`s back to the caller; `files_analyzed` and `incomplete` are carried
/// through from discovery/extraction (a frontend parse failure already means "absence of a
/// finding proves nothing" before analysis even starts) and are OR'd with whatever this module
/// discovers on its own (a rule turned `Off`, a `Context` that failed to resolve, or a
/// bounded-work limit hit).
///
/// A test whose `context.is_comparable()` is `false` is excluded from every rule up front, the
/// same way a rule turned `Off` is skipped up front rather than computed and filtered: a context
/// the extractor could not fully read is not evidence of anything, and asserting a duplicate on
/// top of an unreadable fact is exactly the false positive this product refuses to emit. Because
/// at least one test was excluded on those grounds, the result is marked incomplete regardless of
/// whether excluding it actually changed any finding -- the same "absence of a finding proves
/// nothing" principle applied to a hit work-limit, not something this function can know either
/// way without the very comparison it just refused to make.
///
/// Never reads the filesystem or a parser: every input arrives as already-extracted IR, which is
/// what lets this function be exercised entirely from hand-built `TestCase` values in tests.
/// An ordinary full run, with no baseline and no changed-file scoping. See
/// [`analyze_with_modes`] for the form the CLI actually calls.
pub fn analyze(
    tests: Vec<TestCase>,
    config: &Config,
    files_analyzed: usize,
    causes: IncompleteCauses,
) -> anyhow::Result<AnalysisResult> {
    analyze_with_modes(tests, config, files_analyzed, causes, RunModes::default())
}

/// [`analyze`], with the baseline and changed-file modes in force for this run.
///
/// The modes are applied after every rule has run but *before* the quality gate's inputs are
/// captured -- see [`RunModes`] for why that ordering is not optional, and `crate::modes` for what
/// each mode does.
pub fn analyze_with_modes(
    tests: Vec<TestCase>,
    config: &Config,
    files_analyzed: usize,
    causes: IncompleteCauses,
    modes: RunModes<'_>,
) -> anyhow::Result<AnalysisResult> {
    analyze_with_budget(
        tests,
        config,
        files_analyzed,
        causes,
        modes,
        MAX_PAIR_COMPARISONS,
    )
}

/// [`analyze`]'s body, parameterized over the pair-comparison budget.
///
/// [`analyze`] is the only real caller and always passes [`MAX_PAIR_COMPARISONS`] (2 million) --
/// this split exists purely so `analysis::tests` can drive the *actual* budget-exhaustion ->
/// `AnalysisResult::incomplete` propagation end to end, rather than only unit-testing `PairBudget`
/// in isolation, which a from-scratch test can never do against a two-million-unit real budget.
/// See `analysis/tests.rs`'s `pair_budget_exhaustion_propagates_to_analysis_result_incomplete`.
fn analyze_with_budget(
    tests: Vec<TestCase>,
    config: &Config,
    files_analyzed: usize,
    causes: IncompleteCauses,
    modes: RunModes<'_>,
    max_pair_comparisons: u64,
) -> anyhow::Result<AnalysisResult> {
    analyze_with_budgets(
        tests,
        config,
        files_analyzed,
        causes,
        modes,
        max_pair_comparisons,
        MAX_HOOK_PAIR_SCANS,
    )
}

/// [`analyze_with_budget`] with the `redundant-hook` scan ceiling exposed for adversarial tests.
/// Production always supplies [`MAX_HOOK_PAIR_SCANS`].
fn analyze_with_budgets(
    tests: Vec<TestCase>,
    config: &Config,
    files_analyzed: usize,
    causes: IncompleteCauses,
    modes: RunModes<'_>,
    max_pair_comparisons: u64,
    max_hook_pair_scans: u64,
) -> anyhow::Result<AnalysisResult> {
    let mut findings = Vec::new();
    // Counted, not merely detected. `any()` was enough for a bool, but the banner has to say
    // how many tests it is speaking for -- that number is the difference between "one odd file"
    // and "most of this corpus was never compared", and it is the cause the old banner never
    // named. Executable declarations only, matching every other test count this run reports.
    let mut causes = causes;
    causes.unresolved_contexts = tests
        .iter()
        .filter(|test| is_executable_declaration(test.kind) && !test.context.is_comparable())
        .count();
    let mut budget = PairBudget::new(max_pair_comparisons);
    let mut hook_scan_budget = PairBudget::new(max_hook_pair_scans);
    let mut parameterization_participants = BTreeSet::new();

    // `duplicate-suite` runs before every per-test rule because its output is an input to them:
    // a suite-level copy suppresses the per-test findings between the two copies, which is what
    // makes `duplicate-suite` a claim about a duplicated block reported ONCE rather than a stream
    // of per-test findings restating the same copy. The suppression itself is a post-filter
    // applied after every rule has run, so no rule's
    // emission signature has to know suites exist.
    let mut suite_twins = SuiteTwins::default();
    let duplicate_suite_severity = config.severity(Rule::DuplicateSuite);
    if duplicate_suite_severity != Severity::Off {
        causes.pair_budget_exhausted |= emit_duplicate_suite(
            &tests,
            duplicate_suite_severity,
            &mut budget,
            &mut findings,
            &mut suite_twins,
        );
    }

    let duplicate_test_severity = config.severity(Rule::DuplicateTest);
    if duplicate_test_severity != Severity::Off {
        causes.pair_budget_exhausted |=
            emit_duplicate_test(&tests, duplicate_test_severity, &mut budget, &mut findings);
    }

    let duplicate_name_severity = config.severity(Rule::DuplicateTestName);
    if duplicate_name_severity != Severity::Off {
        causes.pair_budget_exhausted |=
            emit_duplicate_test_name(&tests, duplicate_name_severity, &mut budget, &mut findings);
    }

    let parameterization_severity = config.severity(Rule::ParameterizationCandidate);
    if parameterization_severity != Severity::Off {
        causes.pair_budget_exhausted |= emit_parameterization_candidate(
            &tests,
            parameterization_severity,
            &mut budget,
            &mut findings,
            &mut parameterization_participants,
        );
    }

    let near_duplicate_severity = config.severity(Rule::NearDuplicateTest);
    if near_duplicate_severity != Severity::Off {
        causes.pair_budget_exhausted |= emit_near_duplicate_test(
            &tests,
            near_duplicate_severity,
            &mut budget,
            &mut findings,
            &parameterization_participants,
        );
    }

    let cross_file_severity = config.severity(Rule::CrossFileDuplicate);
    if cross_file_severity != Severity::Off {
        causes.pair_budget_exhausted |=
            emit_cross_file_duplicate(&tests, cross_file_severity, &mut budget, &mut findings);
    }

    let subsumed_severity = config.severity(Rule::SubsumedTest);
    if subsumed_severity != Severity::Off {
        causes.pair_budget_exhausted |=
            emit_subsumed_test(&tests, subsumed_severity, &mut budget, &mut findings);
    }

    let each_row_severity = config.severity(Rule::DuplicateEachRow);
    if each_row_severity != Severity::Off {
        causes.pair_budget_exhausted |=
            emit_duplicate_each_row(&tests, each_row_severity, &mut budget, &mut findings);
    }

    let redundant_hook_severity = config.severity(Rule::RedundantHook);
    if redundant_hook_severity != Severity::Off {
        // Two budgets, two causes. Returning one `bool` made an exhausted *scan* print "the
        // pair-comparison budget ran out", which is the misattribution this whole struct exists
        // to remove.
        let hook_limits = emit_redundant_hook(
            &tests,
            redundant_hook_severity,
            &mut hook_scan_budget,
            &mut budget,
            &mut findings,
        );
        causes.hook_scan_exhausted |= hook_limits.scan_exhausted;
        causes.pair_budget_exhausted |= hook_limits.pair_budget_exhausted;
    }

    // The suite-level suppression, applied once every rule has spoken: a per-test finding whose
    // participants sit in *different suites of one duplicated suite group* restates the suite
    // finding above it, so it goes. Findings between one copy and an unrelated test, or entirely
    // within one copy, stay -- only the cross-copy restatements are redundant reporting.
    suite_twins.suppress_cross_copy_findings(&mut findings);

    sort_findings(&mut findings);

    let finished = finish_findings(findings, causes, modes);
    let mode_summary = finished.modes;
    let causes = finished.causes;
    let had_error_finding = finished.had_error_finding;
    let error_finding_locations = finished.error_finding_locations;

    // `tests_analyzed` counts executable declarations only -- a `describe`/`suite` is a
    // container, never something that itself gets marked redundant or consolidatable, so counting
    // it here would both misreport the number in every report's summary and (more seriously)
    // inflate `threshold_passed`'s denominator, silently making a configured threshold more
    // permissive than requested on any file that groups its tests in a `describe`. Reuses
    // `is_executable_declaration` rather than adding a second notion of what counts, since that is
    // already the function every rule above uses to keep suites out of grouping in the first
    // place.
    // Counted here rather than left to the caller: `analyze` is what excludes these tests, so it
    // is the only place that knows the number without re-deriving the rule.
    // Same executable-declaration filter as `tests_analyzed` and `main.rs`'s diagnostic. Counting
    // suites here made the machine contract disagree with the human one: a dynamic `describe`
    // wrapping a single test reported `testsAnalyzed: 1` alongside `unresolvedTests: 2`, which
    // reads as more tests excluded than exist. Introduced when this field was added; the two
    // counts must use one definition of what a test is or neither can be trusted.
    let unresolved_tests = tests
        .iter()
        .filter(|test| is_executable_declaration(test.kind) && !test.context.is_comparable())
        .count();

    let executable_tests_analyzed = tests
        .iter()
        .filter(|test| is_executable_declaration(test.kind))
        .count();

    // Files in which *every* executable test was excluded, derived here for the same reason
    // `unresolved_tests` is: `analyze` is what excludes them, so it is the only place that knows
    // without re-deriving the rule, and one definition of "test" has to govern both counts.
    //
    // Separate from the corpus-wide count because the two mislead differently. A file with some
    // tests excluded still appears in the report, and its count qualifies findings a reader can
    // see. A file with *all* of them excluded produces no findings at all, so it never appears --
    // and a report that never mentions a file is indistinguishable from one that cleared it. That
    // is the `not-compared` result presenting as `distinct` at the file level, which is precisely
    // what this crate's verdict vocabulary exists to prevent.
    let blind_files = {
        let mut totals: BTreeMap<&Path, (usize, usize)> = BTreeMap::new();
        for test in tests
            .iter()
            .filter(|test| is_executable_declaration(test.kind))
        {
            let entry = totals.entry(test.registration_file()).or_default();
            entry.0 += 1;
            if !test.context.is_comparable() {
                entry.1 += 1;
            }
        }
        let mut blind: Vec<BlindFile> = totals
            .into_iter()
            .filter(|(_, (total, excluded))| *total > 0 && total == excluded)
            .map(|(path, (total, _))| BlindFile {
                path: path.to_path_buf(),
                tests: total,
            })
            .collect();
        // Worst first, then by path: a list bounded for display must not let an alphabetically
        // late file with hundreds of tests fall off the end behind a one-test file.
        blind.sort_by(|a, b| b.tests.cmp(&a.tests).then_with(|| a.path.cmp(&b.path)));
        blind
    };

    Ok(AnalysisResult {
        had_error_finding,
        error_finding_locations,
        unresolved_tests,
        blind_files,
        excluded_files: Vec::new(),
        exclusions: Default::default(),
        findings: finished.findings,
        tests_analyzed: executable_tests_analyzed,
        files_analyzed,
        incomplete_causes: causes,
        modes: mode_summary,
    })
}

/// What one analysis path's rules produced, after the run modes and the presentation cap have
/// had their say.
pub struct FinishedFindings {
    /// The findings that survive the modes and the presentation cap, in report order.
    pub findings: Vec<Finding>,
    /// The incompleteness causes the modes and the cap recorded.
    pub causes: IncompleteCauses,
    /// What each mode applied.
    pub modes: ModeSummary,
    /// Error-severity findings existed before truncation. See [`AnalysisResult::had_error_finding`].
    pub had_error_finding: bool,
    /// Distinct locations error-severity findings carried before truncation.
    pub error_finding_locations: usize,
}

/// Applies the run modes to sorted `findings` and captures the quality-gate inputs: what every
/// analysis path does after its rules have spoken, so the default engine's rules and the next
/// engine's cannot drift apart about suppressions, gate capture or the presentation cap.
///
/// Modes run after sorting (so baseline allowance is consumed in a deterministic order) and
/// before the gate inputs are captured. Inline suppressions first -- the strongest claim ("this
/// is not debt") must run before the baseline spends any allowance -- then changed-file scoping,
/// so a baseline's allowance is only spent on findings that are actually in scope.
pub fn finish_findings(
    mut findings: Vec<Finding>,
    mut causes: IncompleteCauses,
    modes: RunModes<'_>,
) -> FinishedFindings {
    let mut mode_summary = ModeSummary {
        suppressions_applied: modes::apply_inline_suppressions(&mut findings, modes.suppressions),
        ..ModeSummary::default()
    };
    if let Some(changed) = modes.changed_files {
        mode_summary.changed_scope = true;
        mode_summary.withheld_unchanged = modes::retain_changed_findings(&mut findings, changed);
        // A changed-files run that withheld something is a narrowed view of the repository, and
        // saying otherwise would be exactly the false clean bill of health this flag is most
        // likely to produce. One that withheld nothing is byte-for-byte a full run's report, so it
        // is not marked incomplete on the strength of a flag alone -- that would cry wolf on every
        // clean pull request and train people to ignore the signal.
        causes.withheld_unchanged = mode_summary.withheld_unchanged;
    }
    if let Some(baseline) = modes.baseline {
        mode_summary.baseline_applied = true;
        let (baselined, new_findings) = modes::apply_baseline(&mut findings, baseline);
        mode_summary.baselined = baselined;
        mode_summary.new_findings = new_findings;
    }

    // Captured before truncation on purpose. See `AnalysisResult::had_error_finding`.
    //
    // Baselined findings are skipped: they are recorded, accepted debt, and a gate they still
    // failed would make `--baseline` decorative. They remain in `findings` and in every report --
    // excluded from the gate, never from the output.
    let had_error_finding = findings
        .iter()
        .any(|finding| finding.severity == Severity::Error && !finding.baselined);
    // Captured here, beside `had_error_finding`, for the same reason: this is the threshold's
    // numerator, and truncation below would shrink it.
    let error_finding_locations = findings
        .iter()
        .filter(|finding| finding.severity == Severity::Error && !finding.baselined)
        .flat_map(|finding| finding.locations.iter().cloned())
        .collect::<BTreeSet<_>>()
        .len();
    if findings.len() > MAX_FINDINGS {
        // Recorded as a count rather than a flag: "the display cap dropped 4 findings" and "the
        // display cap dropped 40,000" are the same bool and very different reports.
        causes.findings_truncated = findings.len() - MAX_FINDINGS;
        findings.truncate(MAX_FINDINGS);
    }
    FinishedFindings {
        findings,
        causes,
        modes: mode_summary,
        had_error_finding,
        error_finding_locations,
    }
}

/// Whether `result`'s error-severity findings keep the project within `config`'s quality-gate
/// threshold.
///
/// The percentage is `unique test locations appearing in at least one error-severity finding`
/// divided by `tests_analyzed` (every *executable* declaration [`analyze`] was given, participating
/// or not -- see [`is_executable_declaration`]; a `describe`/`suite` is a container, never itself
/// redundant or consolidatable, so it must not inflate this denominator and understate the real
/// duplication rate), left unrounded; the gate passes when that percentage is `<=`
/// `config.threshold`. An empty project (`tests_analyzed == 0`) is defined as 0%, which passes any
/// non-negative threshold -- there is nothing to be redundant with.
///
/// A location is counted once no matter how many separate findings it appears in, per the spec's
/// "counted once each": a test that participates in three separate error-level duplicate pairs
/// still only costs one slot in the numerator.
///
/// This is exposed as a free function rather than a field on `AnalysisResult` because
/// `AnalysisResult` is defined in `model.rs`, which this crate keeps free of configuration
/// concerns (the same reason `config.rs` owns rule-id/severity string parsing instead of `Rule`).
///
/// Findings a baseline accounts for are excluded from the numerator (they are already excluded
/// from [`AnalysisResult::error_finding_locations`] when it is computed), but **not** from the
/// denominator: `tests_analyzed` is the size of the corpus, which accepting some existing debt
/// does not change. The resulting percentage is therefore "how much of this project is
/// error-level duplication nobody has signed off on", which is the number a threshold is for.
///
/// See [`exit_code`]'s doc comment for how this interacts with
/// [`AnalysisResult::had_error_finding`] at the default threshold of `0`.
pub fn threshold_passed(result: &AnalysisResult, config: &Config) -> bool {
    if result.tests_analyzed == 0 {
        return true;
    }
    // Reads the recorded pre-truncation count rather than walking `findings`, which may have been
    // shortened for presentation. Re-deriving it here is what let a truncated run pass a threshold
    // its true rate exceeded.
    let percentage = result.error_finding_locations as f64 / result.tests_analyzed as f64 * 100.0;
    percentage <= f64::from(config.threshold)
}

/// Determines the process exit code for one analysis run, per the contract [`ExitCode`] documents.
///
/// Returns [`ExitCode::QualityGateFailed`] when [`threshold_passed`] fails -- "this codebase has
/// duplication the configured policy does not allow," a normal, expected outcome of a working
/// analyzer, not a bug in it -- or, only when `config.threshold == 0`, when the run retained at
/// least one error-severity finding before presentation truncation
/// ([`AnalysisResult::had_error_finding`]).
///
/// That second condition is deliberately gated on the *default* threshold rather than checked
/// unconditionally. At `threshold == 0`, [`threshold_passed`] already fails on any error-severity
/// finding still present in the (possibly truncated) retained findings, so folding in
/// `had_error_finding` only changes the answer when truncation hid the very finding that would
/// have failed the gate -- exactly the presentation-independence `had_error_finding` exists for,
/// per its own doc comment. At a configured *non-zero* threshold, an operator has explicitly said
/// "this rate of error-severity duplication is acceptable," so a single retained Error finding
/// must not unconditionally defeat that: `--threshold 100` has to mean "never fail on this rule,"
/// not "fail on the first duplicate no matter what I asked for." Checking `had_error_finding`
/// unconditionally previously made every non-zero threshold ineffective against `duplicate-test`
/// and `duplicate-test-name`, which default to Error severity -- exactly the rules a threshold
/// exists to give an operator control over.
///
/// Otherwise returns [`ExitCode::Success`].
///
/// Does not implement `--fail-on-new`: that gate compares [`crate::model::ModeSummary::new_findings`]
/// against a limit the CLI holds, so it lives in the orchestrator alongside `--fail-on-incomplete`.
/// It is still a *quality-gate* failure ([`ExitCode::QualityGateFailed`]) -- the analyzer ran fine
/// and found more new duplication than policy allows.
///
/// Never returns [`ExitCode::OperationalError`]: that variant is reserved for the `Err` path a
/// caller takes before an `AnalysisResult` even exists (bad configuration, an unreadable path, a
/// parse failure). Conflating "the analyzer broke" with "the analyzer found duplicated tests" is
/// exactly the confusion this product's exit-code contract exists to avoid, so this function's
/// return type structurally cannot express that outcome by mistake -- callers combine it with
/// their own fallible setup instead.
pub fn exit_code(result: &AnalysisResult, config: &Config) -> ExitCode {
    // Reads the recorded outcome rather than the retained findings: truncation is a presentation
    // limit, and a quality gate that changed its answer because a report was shortened would be
    // reporting on the report instead of on the corpus. See the doc comment above for why this is
    // scoped to the default (zero) threshold rather than applied unconditionally.
    let unconditional_error_gate = config.threshold == 0 && result.had_error_finding;
    if unconditional_error_gate || !threshold_passed(result, config) {
        ExitCode::QualityGateFailed
    } else {
        ExitCode::Success
    }
}

/// Whether a declaration is an executable unit rather than a container.
///
/// `duplicate-test` and `duplicate-test-name` both describe properties of things that run. A
/// `describe`/`suite` groups other declarations and is handled by its own future rule.
///
/// Public so callers outside this module -- the orchestrator in `main.rs`, when it reports how
/// many tests had an unresolved context -- can use the same notion of "counts toward the corpus"
/// this module uses for [`AnalysisResult::tests_analyzed`], instead of recomputing (and risking
/// disagreeing with) it from `tests.len()`.
pub fn is_executable_declaration(kind: DeclarationKind) -> bool {
    match kind {
        DeclarationKind::Test | DeclarationKind::Benchmark => true,
        DeclarationKind::Suite => false,
    }
}

/// Canonical JSON encoding of `value`, the input every [`fingerprint_hash`] in this module is
/// taken over.
///
/// JSON rather than a delimiter-joined concatenation, because a normalized fingerprint string can
/// itself contain whatever delimiter a hand-rolled encoding would pick -- the collision this
/// project has already shipped once. `serde_json` writes struct fields in declaration order and
/// sequence elements in order, and nothing serialized here is a map, so the encoding is a pure
/// function of the value on every platform.
fn canonical_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .expect("ContextKey/ModifierKey/Vec<String> contain no map keys and no non-finite floats")
}

/// The three body digests every fingerprint record carries, in `(alpha, structural, behavior)`
/// order.
///
/// [`TestFingerprint`] and [`HookFingerprint`] embed the same three views of a
/// [`crate::model::Fingerprints`] because they are the same claim about a body, whether that body
/// belongs to a test or to a hook -- and they have to agree digit for digit, since a consumer
/// comparing a hook's `alpha` against a test's is entitled to assume both were hashed the same
/// way. `behavior` is hashed from its canonical JSON array, so no event's content can be confused
/// with a separator between events.
fn body_digests(body: &Fingerprints) -> (String, String, String) {
    (
        // `alpha` and `structural` were reduced to a `BodyDigest` at extraction time; the number
        // it carries as `published` is the very hash this function used to compute here, taken
        // over the same canonical JSON of the same text. `behavior` carries its own published
        // hash for the same reason, over the same JSON array of event texts.
        body.alpha.published_hex(),
        body.structural.published_hex(),
        body.behavior.published_hex(),
    )
}

/// One [`TestFingerprint`] per executable declaration in `tests`, for `--emit-test-fingerprints`.
///
/// This is the surface `scripts/check/check-fixture-isolation.mjs` needs and no reporter had: a
/// record for **every** analyzed test, including -- especially -- the tests that appear in no
/// finding, which is by definition what a `rule=none` negative fixture's participants are. A gate
/// checking that such a fixture isolates exactly the variable it claims can then compare the
/// analyzer's own normalized views for equality instead of approximating them by tokenizing
/// source.
///
/// # What is included, and why it must match `tests_analyzed`
///
/// Exactly [`is_executable_declaration`], the same filter [`AnalysisResult::tests_analyzed`] uses.
/// A `describe`/`suite` is a container: including one here would make a consumer counting records
/// disagree with the summary line in the very same stream, and "one record per analyzed test" is
/// the only statement that makes the record count checkable at all.
///
/// Tests whose context is unresolved ([`Context::is_comparable`]) **are** included, even though
/// every rule excludes them from grouping. They were analyzed, they count toward
/// `tests_analyzed`, and a gate that silently received nothing for them would read their absence
/// as "no such test" rather than "this test was excluded from comparison" -- the same
/// absence-proves-nothing failure `unresolved_tests` exists to prevent.
///
/// # Ordering
///
/// Sorted by (forward-slash path, line, column, title, alpha), so the stream is byte-identical
/// across runs and across platforms, like every reporter's output. Rows of one `.each`/`.for`
/// table share a declaration location and can share all five components; the sort is stable, so
/// they keep the frontend's deterministic unroll order.
pub fn test_fingerprints(tests: &[TestCase]) -> Vec<TestFingerprint> {
    let mut records: Vec<TestFingerprint> = tests
        .iter()
        .filter(|test| is_executable_declaration(test.kind))
        .map(|test| {
            let (alpha, structural, behavior) = body_digests(&test.body);
            TestFingerprint {
                registration_file: test.registration_file.clone(),
                location: test.location.clone(),
                title: test.full_title(),
                alpha,
                structural,
                behavior,
                trivial: test.body.trivial,
                comparable: test.context.is_comparable(),
                context_key: ContextKey::new(&test.context).digest(),
                modifier_key: ModifierKey::new(&test.modifiers).digest(),
            }
        })
        .collect();
    records.sort_by(|left, right| {
        location_order(&left.location, &right.location)
            .then(left.title.cmp(&right.title))
            .then(left.alpha.cmp(&right.alpha))
            .then(left.registration_file.cmp(&right.registration_file))
    });
    records
}

/// One [`HookFingerprint`] per distinct hook *declaration* reachable from `tests`, for
/// `--emit-test-fingerprints`.
///
/// A [`crate::model::Hook`] is copied into every test's [`Context::hooks`] chain in the scopes
/// under it, so walking `tests` and emitting one record per test visited would emit the same
/// declaration once per test in that scope. Deduplicating by [`Location`] instead is exactly
/// `emit_redundant_hook`'s own identity for a hook -- its pair map is keyed on hook `Location`,
/// never on which test's chain it walked through to find it -- so two records can never disagree
/// about what "the same hook" means. Every occurrence of one `Location` carries an identical
/// [`crate::model::Hook`] by construction (the same call expression, fingerprinted once), so which
/// occurrence supplies the stored record is arbitrary.
///
/// # Ordering
///
/// Sorted by (forward-slash path, line, column) -- the location-only prefix of
/// [`test_fingerprints`]'s sort key. A hook has no title or `.each`-row tiebreak of its own, and
/// unlike a test declaration it cannot share one `Location` with a sibling the way `.each` rows
/// share their table's call site, so location alone is already a total, deterministic order.
pub fn hook_fingerprints(tests: &[TestCase]) -> Vec<HookFingerprint> {
    let mut by_location: BTreeMap<Location, HookFingerprint> = BTreeMap::new();
    for test in tests {
        for hook in &test.context.hooks {
            by_location.entry(hook.location.clone()).or_insert_with(|| {
                let (alpha, structural, behavior) = body_digests(&hook.body);
                HookFingerprint {
                    location: hook.location.clone(),
                    kind: hook.kind,
                    alpha,
                    structural,
                    behavior,
                    trivial: hook.body.trivial,
                }
            });
        }
    }
    let mut records: Vec<HookFingerprint> = by_location.into_values().collect();
    records.sort_by(|left, right| location_order(&left.location, &right.location));
    records
}

/// Deterministic, platform-independent order for two declaration [`Location`]s: forward-slash
/// path, then start position.
///
/// This is the whole of [`hook_fingerprints`]'s sort key and the leading prefix of
/// [`test_fingerprints`]'s, which its doc comment already described that way -- now structurally
/// rather than only in prose, so the two streams cannot drift into ordering the same two paths
/// differently.
fn location_order(left: &Location, right: &Location) -> std::cmp::Ordering {
    forward_slash_key(&left.path)
        .cmp(&forward_slash_key(&right.path))
        .then(left.span.start.cmp(&right.span.start))
}

/// A path rendered with forward slashes, so record order does not depend on the host's separator.
///
/// Ordering by the raw `PathBuf` would sort `a\b.test.ts` and `a/b.test.ts` differently on Windows
/// and Linux, which would make this stream's determinism platform-dependent -- the same reason
/// every reporter normalizes separators before emitting a path.
fn forward_slash_key(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Whether `test` is a candidate for the rules that compare test *bodies*: an executable
/// declaration, carrying a real and non-trivial body, under a [`Context`] the extractor resolved
/// well enough to compare.
///
/// `duplicate-test`, `parameterization-candidate`, `near-duplicate-test`, `cross-file-duplicate`,
/// `subsumed-test` and `duplicate-each-row` all require exactly these four facts before their own
/// key or comparison says anything -- three of their doc comments already name it as
/// "`duplicate-test`'s containment rule, triviality guard, and `context.is_comparable()` filter".
/// They must not drift apart: a test admitted by one rule and refused by another would be
/// reported under one verdict while being silently absent from the rule that would have
/// contradicted it. Each rule's own *distinguishing* conditions stay at its own site.
///
/// The four facts, and why each one:
///
/// - **Executable declaration.** A `describe` is a container, not an executable unit. Grouping
///   suites alongside tests reported a duplicated `describe` twice -- once for the suite itself
///   and once for each test inside it -- and called a suite a "test" at Error severity. A
///   duplicated `describe` is instead `duplicate-suite`'s claim: a separate Warning-level rule
///   that reports the duplicated block ONCE instead of per-test ([`emit_duplicate_suite`]), so
///   suites are out of scope for every rule whose id says "test". `bench` stays in: it is an
///   executable declaration like `test`.
/// - **`has_body`.** A declaration with no callback has no body to compare.
/// - **Non-trivial body.** An empty or stub body is not evidence two tests do the same thing.
/// - **`context.is_comparable()`.** See the module-level "Unresolved contexts" section: an
///   ambient fact the extractor could not read is not evidence of anything, so it never anchors a
///   finding. `analyze` already marks the whole result incomplete for such a test, so the
///   exclusion here only has to keep it out of every group, not report anything about it.
///
/// `duplicate-test-name` deliberately does *not* use this predicate: its claim is about titles, so
/// `has_body` and body triviality are irrelevant to it and it filters on only two of the four.
fn is_body_comparison_candidate(test: &TestCase) -> bool {
    is_executable_declaration(test.kind)
        && test.has_body
        && !test.body.trivial
        && test.context.is_comparable()
}

/// Groups the body-comparison candidates among `tests` by `key` -- the shared population
/// prologue of every rule whose participants are the tests [`is_body_comparison_candidate`]
/// admits (`duplicate-test`, `subsumed-test`, `cross-file-duplicate`). Rules with additional
/// admissions still spell their own predicate rather than threading a second condition through
/// here, so this wrapper reads as exactly "the standard population" and nothing more.
fn group_body_comparison_candidates<'a, K>(
    tests: &'a [TestCase],
    key: impl Fn(&'a TestCase) -> K,
) -> Vec<Vec<usize>>
where
    K: Ord,
{
    group_and_order(tests, |test| {
        is_body_comparison_candidate(test).then(|| key(test))
    })
}

/// Groups tests for `duplicate-test` and pushes the resulting findings. Returns `true` if the
/// shared pair budget was exhausted before every group could be processed.
///
/// Admits exactly the tests [`is_body_comparison_candidate`] admits -- that predicate's doc
/// comment carries the four facts and why each one, including why suites are out of scope.
///
/// Also collapses each group down to one representative per declaration [`crate::model::Location`]
/// via [`dedup_by_declaration`] before deciding anything else about it (A1) -- see the
/// module-level "One declaration, many rows" section.
fn emit_duplicate_test(
    tests: &[TestCase],
    severity: Severity,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    let groups = group_body_comparison_candidates(tests, |test| DuplicateTestKey {
        path: test.registration_file(),
        alpha: test.body.alpha,
        context: ContextKey::new(&test.context),
        modifiers: ModifierKey::new(&test.modifiers),
    });

    // A group every member of which came from one declaration site (for example, every row of
    // one `test.each` table) collapses to a single representative: one declaration, not a
    // duplicate pair, so there is nothing to charge against the budget or report.
    emit_collapsed_groups(
        RuleFindings {
            rule: Rule::DuplicateTest,
            severity,
            message: duplicate_test_message,
        },
        tests,
        groups,
        |group| dedup_by_declaration(tests, group),
        budget,
        findings,
    )
}

/// Collapses `group` (indices into `tests`) down to at most one representative per distinct
/// declaration `Location`.
///
/// Two `TestCase`s built from the same declaration site -- most commonly every row of one
/// `test.each`/`test.for` table, which the frontend gives one shared call-site `Location` -- are
/// not a duplicate pair; they are multiple views of one declaration (see the module-level "One
/// declaration, many rows" section). `group` arrives already sorted into deterministic order by
/// [`group_and_order`] (by each member's own `Ord`, which compares `location` first since it is
/// `TestCase`'s first field), so every run of same-`Location` members is already contiguous -- a
/// single linear scan keeping only the first index seen at each location is enough, with no need
/// to compare every pair of members against each other.
fn dedup_by_declaration(tests: &[TestCase], group: &[usize]) -> Vec<usize> {
    let mut deduped: Vec<usize> = Vec::with_capacity(group.len());
    for &index in group {
        let is_new_location = match deduped.last() {
            Some(&last) => tests[last].location != tests[index].location,
            None => true,
        };
        if is_new_location {
            deduped.push(index);
        }
    }
    deduped
}

/// Groups tests for `duplicate-test-name` and pushes the resulting findings. Returns `true` if
/// the shared pair budget was exhausted before every group could be processed.
///
/// Also excludes any test with `!context.is_comparable()`, even though this rule's own definition
/// never consults `Context`: an unresolved ambient fact means the extractor could not fully read
/// this test, and that same doubt should not let it anchor a different rule's finding either.
///
/// Also collapses each group down to one representative per declaration [`crate::model::Location`]
/// via [`dedup_by_declaration`] (A1's follow-on for this rule): a `test.each` row whose title
/// pattern does not reference any row data (or whose table isn't statically readable at all)
/// renders the identical [`crate::model::Title`] for every row, sharing one declaration site --
/// without this, that is the same self-duplication A1 fixes for `duplicate-test`, just reached
/// through `full_title` instead of `body.alpha`. See the module-level "One declaration, many
/// rows" section.
fn emit_duplicate_test_name(
    tests: &[TestCase],
    severity: Severity,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    let groups = group_and_order(tests, |test| {
        // Same containment rule as `duplicate-test`. Two `describe` blocks sharing a title is a
        // real problem for `-t` filtering, but it is a suite-level finding and belongs to the
        // future `duplicate-suite` rule, not to a rule whose id says "test".
        (is_executable_declaration(test.kind) && test.context.is_comparable()).then(|| {
            DuplicateNameKey {
                path: test.registration_file(),
                full_title: test.full_title(),
            }
        })
    });

    emit_collapsed_groups(
        RuleFindings {
            rule: Rule::DuplicateTestName,
            severity,
            message: duplicate_test_name_message,
        },
        tests,
        groups,
        |group| dedup_by_declaration(tests, group),
        budget,
        findings,
    )
}

/// Groups `describe` blocks for `duplicate-suite` and pushes the resulting findings, recording
/// every duplicated suite group into `twins` so the per-test rules' cross-copy findings can be
/// suppressed afterwards.
///
/// A `describe` is extracted as a `DeclarationKind::Suite` case whose body fingerprint covers the
/// whole block -- titles, hooks, and children included -- so a wholesale copy of a suite is
/// alpha-equal to its original by the same proof `duplicate-test` rests on. The key requires the
/// same file, the same suite path, and the same own title, so two *different* suites that happen
/// to contain identical tests do not aggregate into a suite-level claim; only a copied block
/// under matching nesting does.
///
/// Excludes inert suites (`describe.skip`): a skipped copy is dead code twice over, but it never
/// runs, and reporting it at the suite level would blur "this block executes redundantly" with
/// "this block is dead". Also collapses one-per-location (`dedup_by_declaration`) so the multiple
/// instances of a `describe.each` never pair with each other.
fn emit_duplicate_suite(
    tests: &[TestCase],
    severity: Severity,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
    twins: &mut SuiteTwins,
) -> bool {
    let groups = group_and_order(tests, |test| {
        (test.kind == DeclarationKind::Suite
            && test.has_body
            && !test.body.trivial
            && !test.is_inert()
            && test.context.is_comparable())
        .then(|| DuplicateSuiteKey {
            path: test.registration_file(),
            suite_path: &test.suite_path,
            title: &test.title,
            alpha: test.body.alpha,
            context: ContextKey::new(&test.context),
        })
    });

    emit_collapsed_groups(
        RuleFindings {
            rule: Rule::DuplicateSuite,
            severity,
            message: duplicate_suite_message,
        },
        tests,
        groups,
        |group| {
            // Record the whole group's locations BEFORE dedup: every instance of the copy
            // (including `describe.each` instances sharing a location) scopes the children
            // beneath it for the suppression pass, even though only one representative per
            // location is reported. This closure receives the uncollapsed group precisely so
            // that ordering cannot be lost -- see [`emit_collapsed_groups`].
            twins.0.push(
                group
                    .iter()
                    .map(|&index| {
                        (
                            tests[index].location.clone(),
                            tests[index].registration_file().to_path_buf(),
                        )
                    })
                    .collect(),
            );
            dedup_by_declaration(tests, group)
        },
        budget,
        findings,
    )
}

/// The duplicated suite groups found by [`emit_duplicate_suite`]: one vec of member suite
/// locations per group. Consumed by [`SuiteTwins::suppress_cross_copy_findings`].
#[derive(Default)]
struct SuiteTwins(Vec<Vec<(Location, PathBuf)>>);

impl SuiteTwins {
    /// The innermost duplicated suite containing `location`, as `(group index, suite location)`.
    ///
    /// Containment is by byte span within the same file; innermost wins so a duplicated suite
    /// nested inside another duplicated suite scopes its own children, not the outer copy's.
    fn innermost_scope(&self, location: &Location, caller: &Path) -> Option<(usize, Location)> {
        let mut best: Option<(u32, usize, Location)> = None;
        for (index, group) in self.0.iter().enumerate() {
            for (suite, owner) in group {
                let contains = owner == caller
                    && suite.path == location.path
                    && suite.span.start_byte <= location.span.start_byte
                    && location.span.end_byte <= suite.span.end_byte;
                if !contains {
                    continue;
                }
                let size = suite.span.end_byte.saturating_sub(suite.span.start_byte);
                if best
                    .as_ref()
                    .is_none_or(|(smallest, _, _)| size < *smallest)
                {
                    best = Some((size, index, suite.clone()));
                }
            }
        }
        best.map(|(_, index, suite)| (index, suite))
    }

    /// Drops per-test findings that merely restate a suite-level copy: those with participants in
    /// two or more *different* suites of one duplicated group.
    ///
    /// Scoped to the four per-test rules, and only to findings whose participants span copies:
    /// a finding between one copy and unrelated code is its own fact and stays, as does any
    /// finding entirely within one copy. Cross-file findings cannot be affected -- a suite's span
    /// cannot contain a location in another file.
    fn suppress_cross_copy_findings(&self, findings: &mut Vec<Finding>) {
        if self.0.is_empty() {
            return;
        }
        findings.retain(|finding| {
            if !matches!(
                finding.rule,
                Rule::DuplicateTest
                    | Rule::DuplicateTestName
                    | Rule::ParameterizationCandidate
                    | Rule::NearDuplicateTest
            ) {
                return true;
            }
            let mut suites_per_group: std::collections::BTreeMap<usize, BTreeSet<Location>> =
                std::collections::BTreeMap::new();
            for (index, location) in finding.locations.iter().enumerate() {
                for caller in finding.registration_files_for(index) {
                    if let Some((group, suite)) = self.innermost_scope(location, caller) {
                        suites_per_group.entry(group).or_default().insert(suite);
                    }
                }
            }
            !suites_per_group.values().any(|suites| suites.len() >= 2)
        });
    }
}

/// Groups tests for `subsumed-test` and pushes the resulting findings. Returns `true` if the
/// shared pair budget was exhausted before every pair could be compared.
///
/// For every pair of executable, non-trivial tests under an equivalent context in one file, the
/// rule asks whether one body's behavior events form a **strict ordered subsequence** of the
/// other's: every step the smaller test performs, the larger performs too, in order, plus more.
/// The relation is directional and not transitive-safe to cluster, so unlike the equality-keyed
/// rules this walks candidate pairs directly rather than grouping.
///
/// Skips pairs whose `alpha` already matches (that is `duplicate-test`'s claim) and pairs with
/// equal behavior event lists (`near-duplicate-test`'s); `subsumed-test` owns exactly the strict
/// subset shape between them.
///
/// Budget semantics differ from the equality rules out of necessity: they charge per *reported
/// equivalence* (a group's spanning edges), while this rule performs real work per *candidate
/// pair*, so it charges one unit per pair compared. A corpus with one enormous same-context
/// group therefore exhausts the budget visibly (`incomplete`) instead of quietly squaring.
fn emit_subsumed_test(
    tests: &[TestCase],
    severity: Severity,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    let groups = group_body_comparison_candidates(tests, |test| SubsumedKey {
        path: test.registration_file(),
        context: ContextKey::new(&test.context),
        callback_form: test.body.callback_form,
    });

    let mut hit_limit = false;
    'groups: for group in groups {
        for left in 0..group.len() {
            for right in (left + 1)..group.len() {
                if !budget.charge(1) {
                    hit_limit = true;
                    break 'groups;
                }
                let a = &tests[group[left]];
                let b = &tests[group[right]];
                if a.body.alpha == b.body.alpha || a.body.behavior == b.body.behavior {
                    continue;
                }
                if is_strict_subsequence(a.body.behavior.events(), b.body.behavior.events()) {
                    findings.push(subsumed_pair_finding(severity, a, b));
                } else if is_strict_subsequence(b.body.behavior.events(), a.body.behavior.events())
                {
                    findings.push(subsumed_pair_finding(severity, b, a));
                }
            }
        }
    }
    hit_limit
}

/// Whether `small` reads as a strict ordered subsequence of `large`: every element of `small`
/// appears in `large`, in order, and `small` is genuinely smaller (an equal list is
/// `near-duplicate-test`'s territory, not this rule's).
fn is_strict_subsequence(small: &[EventDigest], large: &[EventDigest]) -> bool {
    if small.is_empty() || small.len() >= large.len() {
        return false;
    }
    let mut cursor = 0;
    for element in large {
        if cursor < small.len() && *element == small[cursor] {
            cursor += 1;
        }
    }
    cursor == small.len()
}

/// Groups rows-of-one-declaration for `duplicate-each-row` and pushes the resulting findings.
/// Returns `true` if the shared pair budget was exhausted.
///
/// Every row of a `test.each`/`test.for` or `describe.each`/`describe.for` table is extracted as
/// its own case sharing the declaration's one `Location` (see the module-level "One declaration,
/// many rows" section). Executable rows compare their rendered title and substituted body, which
/// models the values the callback actually observes: rows `[1, 2]` and `[1, 3]` are redundant for
/// a one-parameter callback because both bind only `1`. Suite rows instead use
/// [`TestCase::table_row_class`], assigned from exact kind-tagged row values, because a suite body
/// contains nested declarations and cannot apply test-body substitution through the whole block.
/// The class introduces no hash collision and costs one inline integer instead of retaining
/// canonical row text per case.
///
/// The unreadable-table hazard is defused here, and was from this rule's first day: a table this
/// extractor could not enumerate produces exactly one representative case whose context is
/// *unresolved*, and [`Context::is_comparable`] excludes it here, so two unresolvable tables can
/// never read as matching rows through this rule. That mattered because the IR once recorded
/// every unenumerable table identically -- `.each(getCasesA())` and `.each(getCasesB())` were
/// literally indistinguishable -- and this is the one rule that would have stepped on it. The
/// field itself now carries an identity for the table *expression* (see
/// [`crate::model::EachRows`]), but this filter is what actually keeps an unreadable table out of
/// comparison, and it is deliberately kept rather than replaced by that fix: an honest field
/// proves two unreadable tables have different provenance, not that either table's values are
/// known.
fn emit_duplicate_each_row(
    tests: &[TestCase],
    severity: Severity,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    enum RowKey {
        /// Rendered title, substituted body, and the row's own equality class -- all three parts
        /// of a three-part claim.
        ///
        /// The row class used to be omitted, on the reasoning that "different raw rows may bind
        /// the same arguments when the callback drops extra cells; those genuinely register the
        /// same case twice". The 2026-09-27 scan refuted the premise rather than the conclusion:
        /// the callback usually does NOT drop them, the substitution merely cannot see how they
        /// are read. One observed display-utility suite reads its extra cell through a REST
        /// PARAMETER (`(formatString, ...args)`), and an application framework's form-utility
        /// suite reads its whole row as one destructured object -- 17 of this rule's 41 false positives, and
        /// in both the rows differ in source while every field the rule compared agreed.
        ///
        /// Proving a cell is unread is exactly the claim that failed, so the rule no longer makes
        /// it: rows group only when their data is identical too. Nothing pinned as a true
        /// positive is lost -- all three positive fixtures in this corpus repeat a literally
        /// identical row (`[[2], [2]]`, `[['%s', 100], ['%s', 100]]`, `[[1, 2], [1, 2]]`), which
        /// is also what each negative fixture's "named weakening" turns itself into to fire.
        Executable(String, BodyDigest, Option<u32>),
        /// Suite callbacks cannot substitute their row through all nested declarations. Their
        /// exact declaration-local row equality class is the sound discriminator instead.
        Suite(Vec<Title>, u32),
    }

    let declarations = group_and_order(tests, |test| {
        let comparable_body = match test.kind {
            DeclarationKind::Suite => {
                test.has_body && !test.body.trivial && test.context.is_comparable()
            }
            DeclarationKind::Test | DeclarationKind::Benchmark => {
                is_body_comparison_candidate(test)
            }
        };
        (comparable_body && test.table_row_class.is_some())
            .then(|| (test.registration_file(), test.location.clone()))
    });

    let mut hit_limit = false;
    for declaration in declarations {
        // BTreeMap keeps the finding order deterministic across filesystems and platforms.
        let mut by_row: BTreeMap<RowKey, Vec<usize>> = BTreeMap::new();
        for &index in &declaration {
            let test = &tests[index];
            let key = match test.kind {
                DeclarationKind::Suite => RowKey::Suite(
                    test.suite_path.clone(),
                    test.table_row_class
                        .expect("declaration group admits table rows only"),
                ),
                DeclarationKind::Test | DeclarationKind::Benchmark => {
                    // S7, the title half. This rule's claim is "identical rendered title AND
                    // substituted body", and an unrendered `.each` pattern is shared by every row
                    // of the declaration BY CONSTRUCTION -- `overwrites default ${key} with %p` is
                    // the same string for row 1 and row 40 because nothing was substituted into
                    // it. Keying on it makes the title half of the claim unconditionally true,
                    // leaving the body half to carry a two-part claim alone. That is how the rule
                    // reached 14.6% precision (6 of 41, exhaustive) on the 2026-09-27 scan: the
                    // largest sub-shape, 18 of the 41, is an unfoldable `${...}` title, usually
                    // from an enclosing `for` loop -- all nine findings in one HTTP client are a
                    // single `it.each` inside `for (const key of ...)`.
                    //
                    // A row whose title did not render was never shown to the reader under the
                    // same title as its sibling, because it was never shown under a title at all.
                    // Such rows are dropped from the grouping rather than keyed differently: two
                    // of them are not "different cases", they are two cases this rule cannot
                    // speak about, and inventing a distinct key per row would say they were
                    // compared and found distinct.
                    if !test.title.is_static() {
                        continue;
                    }
                    RowKey::Executable(
                        test.full_title(),
                        test.body.alpha,
                        // `None` when the callback provably drops the cells past its fixed
                        // parameters: those rows bind the same arguments and ARE the same case,
                        // so requiring their raw data to match would discard a true positive.
                        test.row_cells_fully_observable.then(|| {
                            test.table_row_class
                                .expect("declaration group admits table rows only")
                        }),
                    )
                }
            };
            by_row.entry(key).or_default().push(index);
        }
        // The row sub-groups are already exactly the members this rule certifies -- the
        // declaration-site collapse every other rule applies would defeat the point here, since
        // sharing one declaration site is this rule's *premise* -- so the collapse is identity.
        hit_limit = emit_collapsed_groups(
            RuleFindings {
                rule: Rule::DuplicateEachRow,
                severity,
                message: duplicate_each_row_message,
            },
            tests,
            by_row.into_values().collect(),
            |members| members.to_vec(),
            budget,
            findings,
        );
        if hit_limit {
            break;
        }
    }
    hit_limit
}

/// Finds hooks whose work an ancestor hook of the same kind already performs, and pushes one
/// finding per such hook pair. Returns `true` if the budget was exhausted.
///
/// A test's [`Context::hooks`] is its full chain, outermost first, so "a hook repeats an
/// ancestor" is an intra-chain equality check: same [`HookKind`], both bodies non-trivial, and
/// alpha-equal bodies -- the same normalization `duplicate-test` compares test bodies with. The
/// same check also catches an immediate duplicate (`beforeEach(x); beforeEach(x)`) in one scope,
/// which costs every test in that scope the same double setup; the message stays neutral about
/// nesting because the chain does not record scope boundaries.
///
/// Every test under a nested suite carries the same hook pair in its chain, so pairs are
/// deduplicated by `(outer location, inner location)` before they consume the finding budget.
/// The separate scan budget is charged for every candidate pair before comparison because the
/// repeated enumeration itself is the work that needs a bound.
/// Which of [`emit_redundant_hook`]'s two budgets ran out, reported separately.
#[derive(Clone, Copy, Default)]
struct HookLimits {
    /// The candidate scan stopped before every hook chain had been inspected.
    scan_exhausted: bool,
    /// The shared finding budget stopped before every deduplicated pair had been emitted.
    pair_budget_exhausted: bool,
}

/// Whether a same-kind hook sits strictly between `outer` and `inner` in the chain and is
/// neither trivial nor a copy of the pair.
///
/// Deleting the inner hook of an alpha-equal pair is only safe while nothing runs between them
/// that could notice. This is easiest to see in teardown, where the naive argument ("the outer
/// hook runs last and wins") fails:
///
/// ```text
/// afterEach(() => { cleanup() })                                   // outer
/// describe('teardown verification', () => {
///   afterEach(() => { expect(screen.queryByText('mounted')).toBeNull() })  // middle
///   describe('component', () => {
///     afterEach(() => { cleanup() })                               // inner
/// ```
///
/// Teardown runs inner, then middle, then outer. Delete the inner `cleanup()` and the middle
/// assertion sees a mounted component and fails. The intervening hook need only OBSERVE, not
/// write, so the gate rejects on any non-equal same-kind hook regardless of what it does -- and
/// applies to all four kinds, not just teardown.
fn same_kind_observer_between(
    hooks: &[Hook],
    outer_index: usize,
    outer: &Hook,
    inner: &Hook,
) -> bool {
    hooks
        .iter()
        .skip(outer_index + 1)
        .take_while(|hook| hook.location != inner.location)
        .any(|between| {
            between.kind == outer.kind
                && !between.body.trivial
                && between.body.alpha != outer.body.alpha
        })
}

/// Pairs same-kind hooks whose bodies agree and whose pairing is not already explained by a
/// shared observer, and reports the inner one as setup the outer already performs. A guarded
/// hook never pairs with a differently-guarded one: the two do not register for the same tests.
fn emit_redundant_hook(
    tests: &[TestCase],
    severity: Severity,
    scan_budget: &mut PairBudget,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> HookLimits {
    let mut pairs: BTreeMap<(Location, Location), (HookKind, BTreeSet<PathBuf>)> = BTreeMap::new();
    let mut scan_exhausted = false;
    'tests: for test in tests
        .iter()
        .filter(|test| is_executable_declaration(test.kind) && test.context.is_comparable())
    {
        let hooks = &test.context.hooks;
        for (offset, outer) in hooks.iter().enumerate() {
            for inner in &hooks[offset + 1..] {
                if !scan_budget.charge(1) {
                    scan_exhausted = true;
                    break 'tests;
                }
                // S5: both bodies must be provably idempotent. Without this the rule told
                // users to delete a hook whose repetition WAS the behaviour -- 0 of 10 correct,
                // exhaustively, on the 2026-09-27 scan.
                // Guards are part of the comparison: a guarded hook and its unguarded twin do
                // not register for the same tests, so claiming the inner one redundant of the
                // outer would recommend deleting setup that actually runs.
                if inner.kind == outer.kind
                    && outer.idempotent
                    && inner.idempotent
                    && !outer.body.trivial
                    && !inner.body.trivial
                    && inner.body.alpha == outer.body.alpha
                    && inner.guards == outer.guards
                    && outer.location != inner.location
                    && !same_kind_observer_between(hooks, offset, outer, inner)
                {
                    // The first kind seen for a location pair wins; same-kind equality is part of
                    // the key's precondition, so a conflicting entry would be a bug upstream.
                    let (_, callers) = pairs
                        .entry((outer.location.clone(), inner.location.clone()))
                        .or_insert_with(|| (inner.kind, BTreeSet::new()));
                    if !callers.contains(test.registration_file()) {
                        callers.insert(test.registration_file().to_path_buf());
                    }
                }
            }
        }
    }

    let mut limits = HookLimits {
        scan_exhausted,
        pair_budget_exhausted: false,
    };
    for ((outer, inner), (kind, callers)) in pairs {
        if !budget.charge(1) {
            limits.pair_budget_exhausted = true;
            break;
        }
        let label = kind.label();
        let mut finding = Finding {
            identity: Default::default(),
            registration_files: Vec::new(),
            rule: Rule::RedundantHook,
            severity,
            message: format!(
                "a nested {label} repeats the work another {label} earlier in its chain already \
                 performs"
            ),
            locations: vec![outer, inner],
            titles: vec![label.to_owned(), label.to_owned()],
            // Hooks are not tests; neither side of this pair can be `.skip`/`.todo`.
            inert: vec![false, false],
            baselined: false,
        };
        let callers: Vec<PathBuf> = callers.into_iter().collect();
        finding.set_registration_files(vec![callers.clone(), callers]);
        findings.push(finding);
    }
    limits
}

/// Groups tests for `parameterization-candidate` and pushes the resulting findings. Returns `true`
/// if the shared pair budget was exhausted before every group could be processed.
///
/// Admits the tests [`is_body_comparison_candidate`] admits -- `duplicate-test`'s containment
/// rule, triviality guard, and `context.is_comparable()` filter, stated once there -- plus the
/// [`MIN_BEHAVIOR_EVENTS_FOR_PARAMETERIZATION`] substance floor documented on that constant.
///
/// A [`ParameterizationKey`] group can still contain members whose `alpha` also matches each
/// other: two tests with byte-identical bodies trivially also have equal `structural`. Reporting
/// that pair *again* here, under a different rule and a different verdict
/// ([`crate::model::Verdict::Consolidatable`] instead of [`crate::model::Verdict::Redundant`]),
/// would contradict `duplicate-test`'s own finding for the same two tests -- the spec is explicit
/// that equal `alpha` is `duplicate-test`'s claim, never this rule's. [`representative_per_alpha`]
/// collapses each group to one member per distinct `alpha` before pairing, which guarantees every
/// emitted pair differs in `alpha` by construction, without needing to inspect pairs individually.
fn emit_parameterization_candidate(
    tests: &[TestCase],
    severity: Severity,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
    emitted_participants: &mut BTreeSet<Vec<(Location, BodyDigest)>>,
) -> bool {
    let groups = group_and_order(tests, |test| {
        (is_body_comparison_candidate(test)
            && test.body.behavior.len() >= MIN_BEHAVIOR_EVENTS_FOR_PARAMETERIZATION)
            .then(|| ParameterizationKey {
                path: test.registration_file(),
                structural: test.body.structural,
                context: ContextKey::new(&test.context),
                modifiers: ModifierKey::new(&test.modifiers),
            })
    });

    emit_collapsed_groups_with_policy(
        RuleFindings {
            rule: Rule::ParameterizationCandidate,
            severity,
            message: parameterization_candidate_message,
        },
        tests,
        groups,
        |group| {
            // Same declaration-site guard `duplicate-test` needs (A1, see the module-level "One
            // declaration, many rows" section): every row of one `.each`/`.for` table shares one
            // call-site `Location`. Collapsing to one representative per location here, *before*
            // any alpha bucketing, is what stops that table's own rows from being reported as a
            // parameterization candidate of itself -- rows of one table routinely share
            // `structural` by construction (the row-specific value usually lives in the table,
            // not the callback body text), so without this guard the table would key-match itself
            // exactly the way A1 fixes for `duplicate-test`.
            let group = dedup_by_declaration(tests, group);
            // Collapsing again to one member per distinct `alpha` can leave fewer than two, which
            // `emit_collapsed_groups` then drops: every surviving member shares one `alpha` (or
            // there was only one after the dedup above), so no pair in this group differs in
            // `alpha` and this rule has nothing to say about it -- `duplicate-test` already
            // covers (or excludes, per its own rules) the equal-alpha case.
            representative_per_alpha(tests, &group)
        },
        ParticipantPolicy {
            should_emit: |_: &[usize]| true,
            on_emit: |participants: &[usize]| {
                emitted_participants.insert(participant_declarations(tests, participants));
            },
        },
        budget,
        findings,
    )
}

/// Collapses `group` (indices into `tests`, already deduped by declaration and known to share a
/// [`ParameterizationKey`]) down to one representative index per distinct `body.alpha` value.
///
/// Unlike [`dedup_by_declaration`], equal-`alpha` members are **not** guaranteed to be contiguous
/// here: `group` is ordered by each member's own `Ord` (location first), not by `alpha`, so two
/// members sharing an `alpha` value can have unrelated members between them. This tracks every
/// `alpha` seen so far in a set, rather than only comparing each member to its immediate
/// predecessor the way [`dedup_by_declaration`] safely can. Keeping the first (lowest-location)
/// occurrence of each `alpha` value preserves `group`'s existing location-sorted order, so the
/// representatives handed to [`group_outcome`] stay in the same "sorted by path, then line, then
/// column" order every other rule's findings rely on -- no separate re-sort needed.
fn representative_per_alpha(tests: &[TestCase], group: &[usize]) -> Vec<usize> {
    let mut seen_alphas: BTreeSet<BodyDigest> = BTreeSet::new();
    let mut representatives = Vec::with_capacity(group.len());
    for &index in group {
        if seen_alphas.insert(tests[index].body.alpha) {
            representatives.push(index);
        }
    }
    representatives
}

/// Equality key for `near-duplicate-test`'s reordered-setup transformation: same file, equivalent
/// context, equivalent semantically-significant modifiers, and an equal **sorted** multiset of
/// `body.behavior` events -- see the module-level doc comment for why this is an exact-match gate
/// (not a similarity score) and why that is precisely what keeps
/// `fixtures/duplicates/near-boilerplate-gradient.test.ts` distinct at every gradient level.
///
/// `path` scopes this rule to a single file, matching `duplicate-test` and
/// `parameterization-candidate`'s own file scoping (see the module-level "File scope" section):
/// the cross-file case is `cross-file-duplicate`'s separate territory -- a separate rule with its
/// own severity -- and this rule does not attempt a cross-file variant of it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NearDuplicateKey<'a> {
    path: &'a Path,
    behavior_multiset: Vec<EventDigest>,
    context: ContextKey<'a>,
    modifiers: ModifierKey<'a>,
    /// Ambient identity, exactly like `context` and `modifiers`: two bodies whose callback form
    /// changes their meaning are not doing the same work, however their events line up.
    ///
    /// `behavior` cannot carry this. An arrow and a `function` reading `this` perform the *same
    /// events* -- `this.value = 5` is one assignment either way -- so the multisets are equal and
    /// always were. What differs is which object those events act on, which is a fact about the
    /// enclosing function rather than about anything the test does. `alpha` records it (as
    /// `arrow-fn`/`function-fn`), which is why `duplicate-test` was never exposed and only the
    /// behaviour-keyed rules were.
    ///
    /// `CallbackForm::Interchangeable` for every body that reads neither `this` nor `arguments`
    /// -- the overwhelmingly common case -- so this partitions nothing that was not already
    /// distinct, and rewriting an arrow as a `function` stays the cosmetic difference this tool
    /// deliberately sees through.
    callback_form: CallbackForm,
}

/// Equality key for `cross-file-duplicate`: `duplicate-test`'s own key (alpha body, equivalent
/// context, equivalent modifiers) with `path` deliberately absent, so groups can span every file
/// in the run. See [`representative_per_file`] for how a resulting group is still kept from ever
/// reporting a same-file pair, which is what "duplicate-test`'s criteria without the file-scoping
/// restriction" and "never report a pair under both rules" both require at once.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CrossFileKey<'a> {
    alpha: BodyDigest,
    context: ContextKey<'a>,
    modifiers: ModifierKey<'a>,
}

/// Groups tests for `near-duplicate-test` and pushes the resulting findings. Returns `true` if the
/// shared pair budget was exhausted before every group could be processed.
///
/// Admits the tests [`is_body_comparison_candidate`] admits -- `duplicate-test`'s containment
/// rule, triviality guard, and `context.is_comparable()` filter, stated once there -- plus the
/// [`MIN_BEHAVIOR_EVENTS_FOR_NEAR_DUPLICATE`] substance floor documented on that constant.
///
/// Same as `parameterization-candidate` (see [`emit_parameterization_candidate`]'s doc comment): a
/// [`NearDuplicateKey`] group can contain members whose `alpha` also matches each other (the
/// group's key does not include `alpha` at all), and equal-`alpha` members are `duplicate-test`'s
/// claim, not this rule's. [`representative_per_alpha`] collapses each group to one member per
/// distinct `alpha` before pairing, for the same reason it does there.
///
/// A group already covered by `parameterization-candidate` is also suppressed here, matched on the
/// **declaration sites** of its participants and on the subset relation. Both messages propose
/// consolidating into one `test.each` table (see [`NEAR_DUPLICATE_REMEDY`]), and that rule states it
/// from a proof this one does not have -- equal `structural` -- so a second finding naming a subset
/// of the same declarations only weakens the stronger claim. Overlap alone is still not enough: a
/// group that merely shares one test is a separate pair, and therefore a separate claim.
///
/// This is a correctness guard, not a volume control, and both halves of the match are there
/// because exact-index matching measurably missed. On the 2026-09-27 public scan no
/// `near-duplicate-test` finding shared an exact participant set with a `parameterization-candidate`
/// one, which read as "the two rules reach disjoint opportunities"; a later scan of a private
/// 24,147-test corpus found **5 of 70** that were not disjoint at all (C4). One was a 3-row
/// `it.each` reported by both rules with identical `Location`s, where [`dedup_by_declaration`] had
/// handed the two rules different representative rows; the other four were strict subsets of a
/// larger parameterization group. See [`participant_declarations`] for why the key is the
/// declaration paired with `structural` rather than the declaration alone.
fn emit_near_duplicate_test(
    tests: &[TestCase],
    severity: Severity,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
    parameterization_participants: &BTreeSet<Vec<(Location, BodyDigest)>>,
) -> bool {
    let groups = group_and_order(tests, |test| {
        (is_body_comparison_candidate(test)
            && test.body.behavior.len() >= MIN_BEHAVIOR_EVENTS_FOR_NEAR_DUPLICATE)
            .then(|| {
                let mut behavior_multiset: Vec<EventDigest> = test.body.behavior.events().to_vec();
                behavior_multiset.sort_unstable();
                NearDuplicateKey {
                    path: test.registration_file(),
                    behavior_multiset,
                    context: ContextKey::new(&test.context),
                    modifiers: ModifierKey::new(&test.modifiers),
                    callback_form: test.body.callback_form,
                }
            })
    });

    emit_collapsed_groups_with_policy(
        RuleFindings {
            rule: Rule::NearDuplicateTest,
            severity,
            message: near_duplicate_test_message,
        },
        tests,
        groups,
        |group| {
            // Same declaration-site guard `duplicate-test` and `parameterization-candidate` both
            // need (A1, see the module-level "One declaration, many rows" section): every row of
            // one `.each`/`.for` table shares one call-site `Location` and typically shares one
            // behavior multiset too (the row-specific value usually lives in the table, not the
            // callback body text), so without this guard the table would key-match itself.
            let group = dedup_by_declaration(tests, group);
            // Then one member per distinct `alpha`. If that leaves fewer than two,
            // `emit_collapsed_groups` drops the group: every surviving member shares one `alpha`,
            // so no pair in it differs in `alpha` and this rule has nothing to say about it --
            // `duplicate-test` already covers that case.
            representative_per_alpha(tests, &group)
        },
        ParticipantPolicy {
            should_emit: |participants: &[usize]| {
                let declarations = participant_declarations(tests, participants);
                !parameterization_participants
                    .iter()
                    .any(|reported| is_declaration_subset(&declarations, reported))
            },
            on_emit: |_: &[usize]| {},
        },
        budget,
        findings,
    )
}

/// Retains at most one representative per registering file and declaration in a group
/// sharing a [`CrossFileKey`]. Every reported pair spans distinct callers and source sites.
///
/// Collapse these dimensions together: discarding a shared declaration first can choose a
/// caller already used by every other declaration and hide a valid cross-file comparison.
fn representative_per_file(tests: &[TestCase], group: &[usize]) -> Vec<usize> {
    let mut files = BTreeSet::new();
    let mut declarations = BTreeSet::new();
    let mut representatives = Vec::new();
    for &index in group {
        let test = &tests[index];
        if !files.contains(test.registration_file()) && !declarations.contains(&test.location) {
            files.insert(test.registration_file());
            declarations.insert(&test.location);
            representatives.push(index);
        }
    }
    // A greedy first choice can leave one representative despite a valid pair: its source
    // is also registered by another caller, while its caller owns another source. Swap that
    // first choice for those two. If either is absent, no distinct-source/caller pair exists.
    if let [first] = representatives.as_slice() {
        let first = &tests[*first];
        let other_caller = group.iter().copied().find(|&index| {
            tests[index].location == first.location
                && tests[index].registration_file() != first.registration_file()
        });
        let other_declaration = group.iter().copied().find(|&index| {
            tests[index].location != first.location
                && tests[index].registration_file() == first.registration_file()
        });
        if let (Some(left), Some(right)) = (other_caller, other_declaration) {
            representatives = vec![left, right];
            representatives.sort_unstable();
        }
    }
    representatives
}

/// Groups tests for `cross-file-duplicate` and pushes the resulting findings. Returns `true` if
/// the shared pair budget was exhausted before every group could be processed.
///
/// Admits the tests [`is_body_comparison_candidate`] admits, the same as `duplicate-test`, but
/// groups on [`CrossFileKey`] (no `path`) instead of
/// [`DuplicateTestKey`], then collapses each group to one representative per file via
/// [`representative_per_file`] before pairing, so a same-file relationship (already
/// `duplicate-test`'s to report, whether or not that rule is enabled) never becomes a
/// `cross-file-duplicate` finding too.
fn emit_cross_file_duplicate(
    tests: &[TestCase],
    severity: Severity,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    let groups = group_body_comparison_candidates(tests, |test| CrossFileKey {
        alpha: test.body.alpha,
        context: ContextKey::new(&test.context),
        modifiers: ModifierKey::new(&test.modifiers),
    });

    emit_collapsed_groups(
        RuleFindings {
            rule: Rule::CrossFileDuplicate,
            severity,
            message: cross_file_duplicate_message,
        },
        tests,
        groups,
        |group| representative_per_file(tests, group),
        budget,
        findings,
    )
}

/// Builds one rule's [`Finding`] over `members`: the shape every rule's finding shares, with one
/// `locations`, `titles` and `inert` entry per participating test, in the members' own order.
///
/// What makes each rule a *different* claim is its equality key and its message, both of which
/// stay at the rule's own site. Rendering an already-certified group as a `Finding` is the same
/// mechanical assembly for all of them, so it lives here once. Hooks are the deliberate
/// exception: [`emit_redundant_hook`] builds its `Finding` itself because its participants are
/// hook declarations rather than [`TestCase`]s, so not one of these three fields is derivable the
/// way it is here.
fn finding_over(rule: Rule, severity: Severity, members: &[&TestCase], message: String) -> Finding {
    let mut finding = Finding {
        identity: Default::default(),
        registration_files: Vec::new(),
        rule,
        severity,
        message,
        locations: members.iter().map(|test| test.location.clone()).collect(),
        titles: members.iter().map(|test| test.full_title()).collect(),
        inert: members.iter().map(|test| test.is_inert()).collect(),
        baselined: false,
    };
    if members.iter().any(|test| test.registration_file.is_some()) {
        finding.set_registration_files(
            members
                .iter()
                .map(|test| vec![test.registration_file().to_path_buf()])
                .collect(),
        );
    }
    finding
}

/// What one rule needs in order to label and word the findings it emits: its id, the severity the
/// config assigned it, and its own message function.
///
/// Bundled because the three always travel together and always come from the same rule -- a
/// finding carrying one rule's id and another rule's message would be a contradiction in terms,
/// and bundling them makes that shape awkward to construct at every emission site.
#[derive(Clone, Copy)]
struct RuleFindings {
    rule: Rule,
    severity: Severity,
    /// The rule's own wording function -- the single place each rule's claim is worded -- called
    /// with the participating tests in group order.
    message: fn(&[&TestCase]) -> String,
}

/// Charges `budget` for the equivalences one certified group asserts, then pushes the findings
/// that state them: a spanning set of pair findings, or one cluster finding, per [`group_outcome`].
///
/// A cluster finding additionally carries [`cluster_truncation_note`], which is required on every
/// one of them.
///
/// Returns `false` once the shared pair budget is exhausted. Per [`PairBudget::charge`] that means
/// the caller must stop processing *further* groups, not merely skip this one.
fn emit_group_findings<P: ParticipantCallbacks>(
    spec: RuleFindings,
    tests: &[TestCase],
    group: &[usize],
    policy: &mut P,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    let RuleFindings {
        rule,
        severity,
        message,
    } = spec;
    match group_outcome(group) {
        GroupOutcome::Spanning(pairs) => {
            let pairs: Vec<(usize, usize)> = pairs
                .into_iter()
                .filter(|&(left, right)| policy.should_emit(&[left, right]))
                .collect();
            if !budget.charge(pairs.len() as u64) {
                return false;
            }
            for (left, right) in pairs {
                policy.on_emit(&[left, right]);
                let members = [&tests[left], &tests[right]];
                findings.push(finding_over(rule, severity, &members, message(&members)));
            }
        }
        GroupOutcome::Cluster(members) => {
            if !policy.should_emit(&members) {
                return true;
            }
            if !budget.charge((members.len() - 1) as u64) {
                return false;
            }
            policy.on_emit(&members);
            let members: Vec<&TestCase> = members.iter().map(|&index| &tests[index]).collect();
            let mut message = message(&members);
            message.push_str(&cluster_truncation_note(members.len()));
            findings.push(finding_over(rule, severity, &members, message));
        }
    }
    true
}

/// Drives one equality-keyed rule's groups to findings: `collapse` reduces each group to the
/// members that rule is willing to certify as distinct participants, a group left with fewer than
/// two is dropped, and the rest are emitted. Returns `true` if the shared pair budget ran out
/// before every group was processed -- the caller's `hit_limit`.
///
/// Six rules share exactly this shape, and they are emphatically not the same claim: each groups
/// by its own equality key and each collapses for its own reason, and both of those -- with the
/// reasoning that justifies them -- stay at the call site where they can be read against that
/// rule. What is genuinely common is only the budget accounting and the drop-below-two rule, and
/// those must not drift apart between rules, which is why they are here instead.
///
/// `collapse` is `FnMut` and receives the *whole* group because one caller
/// ([`emit_duplicate_suite`]) must also record each group before reducing it; passing the full
/// group makes that ordering structural instead of a comment asking the next reader not to
/// reorder two statements.
fn emit_collapsed_groups(
    spec: RuleFindings,
    tests: &[TestCase],
    groups: Vec<Vec<usize>>,
    collapse: impl FnMut(&[usize]) -> Vec<usize>,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    emit_collapsed_groups_with_policy(
        spec,
        tests,
        groups,
        collapse,
        ParticipantPolicy {
            should_emit: |_: &[usize]| true,
            on_emit: |_: &[usize]| {},
        },
        budget,
        findings,
    )
}

/// [`emit_collapsed_groups`] with a participant-aware decision made before budget is charged.
///
/// The policy receives the exact test indices that one finding would contain. This keeps
/// expanded rows with the same declaration [`Location`] distinct and lets a caller suppress a
/// redundant finding without spending budget on a relationship it will not report.
fn emit_collapsed_groups_with_policy<P: ParticipantCallbacks>(
    spec: RuleFindings,
    tests: &[TestCase],
    groups: Vec<Vec<usize>>,
    mut collapse: impl FnMut(&[usize]) -> Vec<usize>,
    mut policy: P,
    budget: &mut PairBudget,
    findings: &mut Vec<Finding>,
) -> bool {
    for group in groups {
        let participants = collapse(&group);
        if participants.len() < 2 {
            continue;
        }
        let before = findings.len();
        if !emit_group_findings(spec, tests, &participants, &mut policy, budget, findings) {
            return true;
        }
        if group
            .iter()
            .any(|&index| tests[index].registration_file.is_some())
        {
            // Collapse suppresses duplicate declarations, not their caller provenance. Build
            // this map once so a large cluster stays linear apart from ordered-map operations.
            let mut callers: BTreeMap<&Location, BTreeSet<&Path>> = BTreeMap::new();
            for &index in &group {
                callers
                    .entry(&tests[index].location)
                    .or_default()
                    .insert(tests[index].registration_file());
            }
            for finding in &mut findings[before..] {
                let files = finding
                    .locations
                    .iter()
                    .map(|location| {
                        callers[location]
                            .iter()
                            .map(|path| path.to_path_buf())
                            .collect()
                    })
                    .collect();
                finding.set_registration_files(files);
            }
        }
    }
    false
}

/// Decides whether one exact participant set should be reported and observes it after its budget
/// charge succeeds.
struct ParticipantPolicy<ShouldEmit, OnEmit> {
    should_emit: ShouldEmit,
    on_emit: OnEmit,
}

/// The callback contract every [`ParticipantPolicy`] consumer needs, stated once: both halves
/// decide and observe on a borrowed slice of test indices. Spelling these bounds at each consumer
/// was two `where` clauses listing the same pair of HRTBs -- one construct, two places to drift.
trait ParticipantCallbacks: Sized {
    fn should_emit(&mut self, participants: &[usize]) -> bool;
    fn on_emit(&mut self, participants: &[usize]);
}

impl<ShouldEmit, OnEmit> ParticipantCallbacks for ParticipantPolicy<ShouldEmit, OnEmit>
where
    ShouldEmit: for<'a> FnMut(&'a [usize]) -> bool,
    OnEmit: for<'a> FnMut(&'a [usize]),
{
    fn should_emit(&mut self, participants: &[usize]) -> bool {
        (self.should_emit)(participants)
    }

    fn on_emit(&mut self, participants: &[usize]) {
        (self.on_emit)(participants)
    }
}

/// Canonical identity for one finding's participants: the declaration site each occupies, paired
/// with the normalized shape reported there.
///
/// This was `participant_key`, a sorted `Vec<usize>` of `tests` indices, and its doc justified the
/// indices as "distinguishing expanded table rows that share a declaration site". That is true of
/// the raw group and false of the group that reaches a finding: every consumer collapses through
/// [`dedup_by_declaration`] first, so at most one member per declaration survives and the index is
/// then nothing but an arbitrary record of which row represented it. Two rules collapsing two
/// differently-keyed groups pick different rows of one `.each` table, so the index sets differ while
/// the reported `Location`s are identical -- the miss C4 recorded.
///
/// The `Location` alone is not the identity either, and the difference is load-bearing rather than
/// defensive. `overlap_suppression_distinguishes_tests_that_share_declaration_locations` pins a
/// file where two findings occupy the same two declarations while reporting *different* tests, and
/// keying on the declaration alone would silence the second one.
///
/// `structural` separates the two cases exactly, because it is the axis both rules are defined on.
/// `parameterization-candidate` fires on EQUAL `structural` and `near-duplicate-test` on equal
/// behavior, so the pair that reaches both -- same behavior, same `structural`, differing `alpha` --
/// is precisely the pair where the surviving row is arbitrary and the two rules agree on everything
/// that is not arbitrary. Rows of one table share `structural` by construction (the row value lives
/// in the table, not the callback text), so whichever row each rule kept, the key is the same. Two
/// genuinely different tests at one declaration differ in `structural`, and stay distinguished.
///
/// ## No fixture, and why that is a limit rather than an omission
///
/// The three tests covering this are unit tests over hand-built `TestCase`s, not fixtures over real
/// Vitest source, and a fixture was attempted first. The obstacle is that `structural` equality very
/// nearly implies behavior equality in real source -- `structural` is `alpha` with literal values
/// collapsed, and it keeps callee identity -- so a structural bucket and a behavior bucket over the
/// same file normally contain the same members, and both rules then collapse to the same
/// representative. The corpus case proves the buckets CAN diverge (5 of 70 findings on a 24,147-test
/// codebase, reproduced against the built binary), but the mechanism that made them diverge there
/// was not recovered, so any fixture written now would be a guess that passes for a reason nobody
/// checked -- which is worse than no fixture. The unit tests construct the divergence directly and
/// each is revert-checked against the key it replaced.
///
/// Sorted, so the identity is independent of reporting order, and deduplicated, so it is a set.
fn participant_declarations(
    tests: &[TestCase],
    participants: &[usize],
) -> Vec<(Location, BodyDigest)> {
    let mut key: Vec<(Location, BodyDigest)> = participants
        .iter()
        .map(|&index| (tests[index].location.clone(), tests[index].body.structural))
        .collect();
    key.sort_unstable();
    key.dedup();
    key
}

/// Whether every entry of `inner` also appears in `outer`. Both are sorted and deduplicated by
/// [`participant_declarations`], so this is one forward scan rather than a nested one.
///
/// Subset rather than equality because a near-duplicate group can be a strict subset of the
/// parameterization group covering it -- four of C4's five findings were that shape. The stronger
/// claim (equal `structural`, stated by `parameterization-candidate`) already names every
/// declaration this one would, so the second finding adds nothing but a second thing to read.
///
/// Note what this still does NOT suppress: a group that merely *overlaps* one already reported.
/// Sharing one test is a different pair, and therefore a different claim.
fn is_declaration_subset(
    inner: &[(Location, BodyDigest)],
    outer: &[(Location, BodyDigest)],
) -> bool {
    if inner.len() > outer.len() {
        return false;
    }
    let mut rest = outer;
    for entry in inner {
        match rest.iter().position(|candidate| candidate == entry) {
            Some(offset) => rest = &rest[offset + 1..],
            None => return false,
        }
    }
    true
}

/// The remedy clause both [`near_duplicate_test_message`] branches end with.
///
/// One constant rather than one copy per branch: the two branches differ in what was *detected*
/// (a reordering, or a normalization that bodies differ under), never in what to do about it, and
/// a fix applied to one site and not its sibling is this repository's most repeated defect. A
/// branch that wanted its own remedy would be a different claim, and would need its own verdict.
const NEAR_DUPLICATE_REMEDY: &str = "equivalence is not proven, so the likely remedy is to \
     consolidate them into one `test.each` table or shared helper that keeps every case as its \
     own row -- delete one only if review establishes it adds no coverage";

/// Builds the `near-duplicate-test` message.
///
/// Two things have to be true of this wording at once, and the first one used to crowd out the
/// second. The rule's verdict is [`crate::model::Verdict::SimilarUncertain`]: it *suspects*, it
/// never proves, so the message must not read as a deletion order the way
/// [`duplicate_test_message`]'s "identical bodies" phrasing legitimately does -- that phrasing
/// asserts a proof this rule does not have. But the previous wording, "review before deleting
/// any", left deletion as the only outcome named, and deletion is measurably the wrong outcome to
/// put in the reader's head: on the 2026-09-27 public scan every one of this rule's 11 actionable
/// findings wanted a `test.each` table and none wanted a deletion, and an adjudicated private
/// corpus agrees (45 of 70 surviving findings, 64%, are `test.each` candidates; deletion is the
/// right action in 31.4%).
///
/// Naming consolidation here adds a remedy rather than restating
/// [`parameterization_candidate_message`]'s: not one of the 282 near-duplicate findings on that
/// scan shared an exact participant set with a `parameterization-candidate` finding, so these are
/// consolidations that rule does not reach (it keys on equal `structural`; this one keys on the
/// behavior multiset). The uncertainty stays attached to the remedy -- "likely", and deletion
/// gated on review -- instead of being traded for a second single-outcome framing, because the
/// rule's 26.7% false-positive rate on that same scan is real and the verdict is unchanged.
fn near_duplicate_test_message(members: &[&TestCase]) -> String {
    let reordered = members.first().is_some_and(|first| {
        members[1..]
            .iter()
            .any(|member| member.body.behavior.events() != first.body.behavior.events())
    });
    near_duplicate_text(members.len(), reordered)
}

/// The `near-duplicate-test` claim over `count` tests, whichever engine found them.
pub(crate) fn near_duplicate_text(count: usize, reordered: bool) -> String {
    if reordered {
        format!(
            "{count} tests execute the same behavior events with at least one different ordering \
             under an equivalent context; if the reordered statements are independent this may be \
             redundant coverage, but {NEAR_DUPLICATE_REMEDY}"
        )
    } else {
        format!(
            "{count} tests execute the same behavior events in the same order under an equivalent \
             context, but their normalized bodies differ; a supported normalization may explain \
             the similarity, but {NEAR_DUPLICATE_REMEDY}"
        )
    }
}

/// Builds the `cross-file-duplicate` message, naming how many distinct files the finding spans so
/// a reader is not surprised the participants are not siblings in one file.
fn cross_file_duplicate_message(members: &[&TestCase]) -> String {
    let files: BTreeSet<&Path> = members
        .iter()
        .map(|test| test.registration_file())
        .collect();
    format!(
        "{} tests across {} files have identical bodies under an equivalent context",
        members.len(),
        files.len()
    )
}

/// Builds the `parameterization-candidate` message.
///
/// Wording is deliberate -- the message is the remedy a reader acts on, so it has to match the
/// verdict exactly (see [`crate::model::Verdict::Consolidatable`]): the remedy this rule proposes
/// is merging into one `test.each` table that keeps every one of these cases as its own row, never
/// deleting one. Some pairs this rule reports are deliberate boundary coverage (`0` vs `1`) that
/// happen to share `structural`; a message that said "duplicate" or "remove" would read as license
/// to delete the very case that would have caught a boundary bug -- the exact confusion that comes
/// from letting one word, "duplicate", stand for three claims with three different remedies.
/// Compare [`duplicate_test_message`], whose "identical bodies" phrasing is correct for
/// `Verdict::Redundant` but would be wrong here.
fn parameterization_candidate_message(members: &[&TestCase]) -> String {
    format!(
        "{} tests share identical structure but differ only in literal values; consolidate into \
         one `test.each` table that keeps every case as its own row",
        members.len()
    )
}

/// Builds the `duplicate-test` message, calling out any inert (`.skip`/`.todo`) participant by
/// full title so a reader does not mistake "still duplicates" for "these both run".
fn duplicate_test_message(members: &[&TestCase]) -> String {
    let mut message = format!(
        "{} tests have identical bodies under an equivalent context",
        members.len()
    );
    let inert: Vec<String> = members
        .iter()
        .filter(|test| test.is_inert())
        .map(|test| test.full_title())
        .collect();
    if !inert.is_empty() {
        message.push_str(&format!(
            "; inert (skip/todo) participant(s), still counted as duplicates: {}",
            inert.join(", ")
        ));
    }
    message
}

fn duplicate_suite_message(members: &[&TestCase]) -> String {
    format!(
        "{} suites have identical bodies under an equivalent context; per-test findings between \
         the copies are suppressed, not repeated",
        members.len()
    )
}

/// Builds the one `subsumed-test` finding shape.
///
/// This rule's participants are *ordered* -- the smaller test first, the test that subsumes it
/// second -- and its message names both behavior-event counts, so unlike the equality-keyed rules
/// it cannot be worded from the member list alone and has no cluster form. It shares
/// [`finding_over`]'s assembly and nothing above it.
fn subsumed_pair_finding(severity: Severity, subsumed: &TestCase, subsumer: &TestCase) -> Finding {
    let members = [subsumed, subsumer];
    let message = format!(
        "one test's {} behavior events are a strict ordered subset of the other's {}; the \
         smaller test may add no coverage, but only review can confirm the larger test's \
         extra observations make it worthless",
        subsumed.body.behavior.len(),
        subsumer.body.behavior.len()
    );
    finding_over(Rule::SubsumedTest, severity, &members, message)
}

fn duplicate_each_row_message(members: &[&TestCase]) -> String {
    match members[0].kind {
        DeclarationKind::Suite => format!(
            "{} rows of one describe.each/describe.for table repeat the same resolved row values; \
             deleting one row loses no coverage",
            members.len()
        ),
        DeclarationKind::Test | DeclarationKind::Benchmark => format!(
            "{} rows of one parameterized test or benchmark declaration are the same case \
             (identical rendered title and substituted body); deleting one row loses no coverage",
            members.len()
        ),
    }
}

fn duplicate_test_name_message(members: &[&TestCase]) -> String {
    format!(
        "{} tests in this file share the full title \"{}\"",
        members.len(),
        members[0].full_title()
    )
}

/// The explicit truncation signal required on every cluster finding: names the member count and
/// how many pairwise findings were not emitted individually.
pub(crate) fn cluster_truncation_note(member_count: usize) -> String {
    let full_pairs = member_count * member_count.saturating_sub(1) / 2;
    format!(
        "; {member_count} tests form one cluster, reported as a single finding instead of \
         {full_pairs} pairwise findings (spanning-set reporting stops at {SPANNING_GROUP_MAX} members)"
    )
}

/// Sorts findings by path, then line, then column (via each finding's `locations`, which is
/// already location-ordered within itself by construction), with rule id and message as final
/// tie-breakers so two findings that legitimately start at the same location still land in a
/// fixed order.
pub(crate) fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|left, right| {
        left.locations
            .cmp(&right.locations)
            .then_with(|| left.rule.cmp(&right.rule))
            .then_with(|| left.message.cmp(&right.message))
    });
}
