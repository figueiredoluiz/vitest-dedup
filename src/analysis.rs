//! What every rule's findings pass through before they are reported: the run modes (inline
//! suppressions, baseline, changed-file scope), the shared work budgets, the presentation cap,
//! report order, the quality-gate threshold and the exit code.
//!
//! The rules themselves live in [`crate::rules`], over the records [`crate::corpus`] builds. A
//! finding is captured for the gate (`had_error_finding`, `error_cases`) before the
//! presentation cap truncates the list, so a capped report can never pass a gate the full list
//! fails.

use crate::cli::ExitCode;
use crate::config::Config;
use crate::model::{
    AnalysisResult, DeclarationKind, Finding, IncompleteCauses, ModeSummary, Severity,
};
use crate::modes::{self, Baseline};
use crate::pairs::SPANNING_GROUP_MAX;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// The baseline and changed-file modes in force for one run.
///
/// Passed *into* analysis rather than applied to its result afterwards, and that placement is the
/// whole point: [`AnalysisResult::had_error_finding`] and
/// [`AnalysisResult::error_cases`] -- the quality gate's two inputs -- are captured
/// before presentation truncation, so anything that changes which findings count against the gate
/// has to happen before that capture. A caller that filtered `result.findings` afterwards would have
/// a report that said one thing and an exit code that said another, and the exit code would win.
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

/// Maximum total pairwise "duplicate relationship" work performed across every rule in one run,
/// before analysis stops early and marks the result incomplete.
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

/// Maximum hook-pair candidates inspected by `redundant-hook`.
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

/// Minimum number of recorded behavior events a test must carry to seed a `near-duplicate-test`
/// candidate under the reordered-setup transformation.
///
/// Unlike `MIN_BEHAVIOR_EVENTS_FOR_PARAMETERIZATION`, this floor is 2, not 1: "reordered setup"
/// is meaningless for a body with fewer than two events -- there is nothing to reorder. A
/// single-event body clearing this floor would mean two syntactically different bodies produced
/// the exact same one-element multiset, which is really "these two bodies are behaviorally
/// identical apart from something this rule cannot see" rather than a reordering, so it is
/// excluded rather than risking a low-substance match. `MIN_BEHAVIOR_EVENTS_FOR_PARAMETERIZATION`
/// stays at 1 because that rule keys on `structural` (shape), where a single assertion event is
/// already the whole comparable shape; this rule keys on the behavior multiset itself, where a
/// reordering claim needs at least two elements to reorder.
pub(crate) const MIN_BEHAVIOR_EVENTS_FOR_NEAR_DUPLICATE: usize = 2;

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
    pub error_cases: usize,
}

/// Applies the run modes to sorted `findings` and captures the quality-gate inputs: what every
/// analysis path does after its rules have spoken, so no rule can drift apart from the others about
/// suppressions, gate capture or the presentation cap.
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
    // A suite finding counts at the severity and locations of the children it covers.
    let had_error_finding = findings
        .iter()
        .any(|finding| finding.gate_severity() == Severity::Error && !finding.baselined);
    // Captured here, beside `had_error_finding`, for the same reason: this is the threshold's
    // numerator, and truncation below would shrink it.
    // A case is its declaration and row, so two rows of one table count twice and one case named
    // by two findings counts once.
    let error_cases = findings
        .iter()
        .filter(|finding| !finding.baselined)
        .flat_map(|finding| finding.gate_cases(Severity::Error))
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
        error_cases,
    }
}

/// Whether `result`'s error-severity findings keep the project within `config`'s quality-gate
/// threshold.
///
/// The percentage is [`gate_percentage`]: distinct runtime cases (declaration and table row) in at
/// least one error-severity finding, over the eligible cases
/// ([`AnalysisResult::eligible_cases`]), left unrounded; the gate passes when it is `<=`
/// `config.threshold`. A run with no eligible case has no rate and passes any threshold -- there
/// is nothing to be redundant with.
///
/// A case is counted once no matter how many separate findings name it, and every row of a table
/// is its own case.
///
/// This is exposed as a free function rather than a field on `AnalysisResult` because
/// `AnalysisResult` is defined in `model.rs`, which this crate keeps free of configuration
/// concerns (the same reason `config.rs` owns rule-id/severity string parsing instead of `Rule`).
///
/// Findings a baseline accounts for are excluded from the numerator (they are already excluded
/// from [`AnalysisResult::error_cases`] when it is computed), but **not** from the
/// denominator: the eligible cases are the size of the corpus, which accepting some existing debt
/// does not change. The resulting percentage is therefore "how much of this project is
/// error-level duplication nobody has signed off on", which is the number a threshold is for.
///
/// See [`exit_code`]'s doc comment for how this interacts with
/// [`AnalysisResult::had_error_finding`] at the default threshold of `0`.
pub fn threshold_passed(result: &AnalysisResult, config: &Config) -> bool {
    // Reads the recorded pre-truncation count rather than walking `findings`, which may have been
    // shortened for presentation. Re-deriving it here is what let a truncated run pass a threshold
    // its true rate exceeded.
    gate_percentage(result).is_none_or(|percentage| percentage <= f64::from(config.threshold))
}

/// The error-level duplication rate the quality gate reads: distinct error cases over eligible
/// cases, as a percentage. `None` when no case is eligible, so no rate exists.
pub fn gate_percentage(result: &AnalysisResult) -> Option<f64> {
    (result.eligible_cases > 0)
        .then(|| result.error_cases as f64 / result.eligible_cases as f64 * 100.0)
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

/// The remedy clause both [`near_duplicate_text`] branches end with.
///
/// One constant rather than one copy per branch: the two branches differ in what was *detected*
/// (a reordering, or a normalization that bodies differ under), never in what to do about it, and
/// a fix applied to one site and not its sibling is this repository's most repeated defect. A
/// branch that wanted its own remedy would be a different claim, and would need its own verdict.
const NEAR_DUPLICATE_REMEDY: &str = "equivalence is not proven, so the likely remedy is to \
     consolidate them into one `test.each` table or shared helper that keeps every case as its \
     own row -- delete one only if review establishes it adds no coverage";

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
