//! Stable, documented JSON report schema.
//!
//! ```json
//! {
//!   "schemaVersion": "0.1",
//!   "testsAnalyzed": 42,
//!   "filesAnalyzed": 5,
//!   "incomplete": true,
//!   "incompleteCauses": {
//!     "discoveryFailed": false,
//!     "projectConfigDegraded": false,
//!     "nothingRead": false,
//!     "filesTooLarge": 0,
//!     "filesUnreadable": 0,
//!     "filesPartiallyRead": 0,
//!     "filesGlobalsUndetermined": 0,
//!     "unresolvedContexts": 3,
//!     "pairBudgetExhausted": false,
//!     "hookScanExhausted": false,
//!     "filesOverMemoryBudget": 0,
//!     "findingsTruncated": 0,
//!     "withheldUnchanged": 0,
//!     "missingPrerequisites": []
//!   },
//!   "unresolvedTests": 3,
//!   "blindFiles": 1,
//!   "exclusions": { "files": 1, "tests": 3, "omitted": 0, "reasons": [
//!     { "reason": "helper-setup-effects", "files": 1, "tests": 3 }
//!   ] },
//!   "modes": {
//!     "baselineApplied": false,
//!     "baselined": 0,
//!     "newFindings": 0,
//!     "changedScope": false,
//!     "withheldUnchanged": 0
//!   },
//!   "excludedFiles": [
//!     {
//!       "path": "src/cart.test.ts",
//!       "reason": "helper-setup-effects",
//!       "otherReasons": [],
//!       "detail": "a called helper's body was not analyzed (the imported runner has unproved setup or fixture effects); any test/describe declarations inside it were not counted",
//!       "testsExcluded": 3,
//!       "testsTotal": 3,
//!       "everyTestExcluded": true
//!     }
//!   ],
//!   "findings": [
//!     {
//!       "rule": "duplicate-test",
//!       "severity": "error",
//!       "verdict": "redundant",
//!       "message": "identical bodies under an equivalent context",
//!       "locations": [
//!         { "path": "src/a.test.ts", "line": 10, "column": 3, "endLine": 10, "endColumn": 8 },
//!         { "path": "src/b.test.ts", "line": 20, "column": 3, "endLine": 20, "endColumn": 8 }
//!       ],
//!       "titles": ["suite > a", "suite > b"],
//!       "inert": [false, false],
//!       "baselined": false,
//!       "fingerprint": "vitestDedupFingerprint/v1:0123456789abcdef"
//!     }
//!   ]
//! }
//! ```
//!
//! Field names are camelCase throughout; `rule` and `severity` reuse the same stable kebab-case
//! identifiers `Rule::id` and the terminal reporter use, so the two reports never disagree about
//! what to call something. `path` is always forward-slash separated regardless of the host
//! platform, because this is a data interchange format, not a display path. `inert` is always
//! serialized, parallel to `locations` and `titles`: a per-participant `.skip`/`.todo` flag so a
//! consumer can tell which side of a duplicate is dead code without parsing prose out of
//! `message`.
//!
//! `excludedFiles` lists every file excluded from comparison in whole or in part, sorted by path:
//! files with excluded tests, files read only in part, and files never read. `reason` is the first
//! reason recorded for the file, a closed [`crate::model::ExclusionReason`] code; `otherReasons`
//! holds the rest in code order; `detail` is the sentence the terminal prints. `testsTotal` is `0`
//! for a file that was not parsed (its count is unknown), and `everyTestExcluded` matches
//! membership in the `blindFiles` count. The list is capped at
//! [`crate::exclusions::MAX_EXCLUDED_FILES`], keeping the files with the most excluded tests;
//! `exclusions` counts every file, so `exclusions.files - len(excludedFiles)` equals
//! `exclusions.omitted`. The list is empty on a run that excluded nothing. HTML and SARIF carry
//! `incompleteCauses` only.
//!
//! `verdict` is [`crate::model::Rule::verdict`]'s output (`redundant` / `consolidatable` /
//! `similar-uncertain` / `name-collision`): what the finding claims, and therefore what remedy is
//! correct, without a consumer having to hardcode a rule-to-remedy table of its own. It is not run
//! through [`report_safe`]/[`report_safe_path`](super::shared::report_safe_path) like `message`, `titles`, and `locations[].path`:
//! `Verdict` is a closed enum this tool defines and serializes by variant name, never text copied
//! out of the analyzed repository, so it carries none of the injection surface those sanitizers
//! exist for.
//!
//! `fingerprint` is [`crate::model::Finding::fingerprint`]'s output: a location-independent
//! identity for the finding, used as-is by `sarif`'s `partialFingerprints`
//! (`src/reporters/sarif.rs`) and by the baseline format (`src/modes.rs`). See that method's doc
//! comment for the derivation and for why two genuinely different findings can legitimately share
//! one — and `crate::modes` for why the baseline counts occurrences rather than assuming
//! uniqueness.
//!
//! `baselined` says a `--baseline` file accounts for this finding, so it does not affect the exit
//! code. It is a per-finding flag rather than an omission: a consumer that wants only new
//! duplication filters on it, and a consumer that wants the repository's full picture — including
//! the debt already signed off — still gets it without a second run.
//!
//! `modes` reports what `--baseline` and `--changed-since` did to the run as a whole
//! ([`crate::model::ModeSummary`]). `withheldUnchanged` in particular cannot be recovered from
//! `findings`: it counts findings this report does *not* contain, and it is why `incomplete` may
//! be true on a changed-files run that otherwise looks clean.

use super::shared::{
    report_registration_files, report_safe, JsonExcludedFile, JsonLocation, SummaryCounters,
};
use super::Reporter;
use crate::model::{AnalysisResult, Finding, Rule, Severity, Verdict};
use anyhow::Result;
use serde::Serialize;
use std::io::Write;

/// Schema version embedded in every JSON report.
///
/// Bump this when a field is removed or its meaning changes; adding a new optional field does
/// not require a bump. Consumers should tolerate unrecognized fields.
pub const JSON_SCHEMA_VERSION: &str = "0.1";

/// Renders an [`AnalysisResult`] as the documented JSON report.
#[derive(Debug, Clone, Copy)]
pub struct JsonReporter {
    /// When `true` (the default), findings are pretty-printed with two-space indentation. Either
    /// form parses to the identical structure — this only affects human readability of the raw
    /// file, never the schema.
    pub pretty: bool,
}

impl JsonReporter {
    /// Creates a reporter that pretty-prints its output, the friendlier default for a report a
    /// human might open directly rather than only pipe to another tool.
    pub fn new() -> Self {
        Self { pretty: true }
    }
}

impl Default for JsonReporter {
    /// Construct the default pretty-printed JSON reporter.
    fn default() -> Self {
        Self::new()
    }
}

impl Reporter for JsonReporter {
    /// Serialize the analysis result as pretty or compact JSON, propagating serialization and
    /// writer errors.
    fn write_report(&self, result: &AnalysisResult, writer: &mut dyn Write) -> Result<()> {
        let report = JsonReport::from(result);
        if self.pretty {
            serde_json::to_writer_pretty(writer, &report)?;
        } else {
            serde_json::to_writer(writer, &report)?;
        }
        Ok(())
    }
}

/// The top-level JSON report document.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonReport {
    schema_version: &'static str,
    #[serde(flatten)]
    summary: SummaryCounters,
    excluded_files: Vec<JsonExcludedFile>,
    findings: Vec<JsonFinding>,
}

impl From<&AnalysisResult> for JsonReport {
    /// Build the versioned JSON document from the analysis summary and findings.
    fn from(result: &AnalysisResult) -> Self {
        Self {
            schema_version: JSON_SCHEMA_VERSION,
            summary: SummaryCounters::from_result(result),
            excluded_files: result
                .excluded_files
                .iter()
                .map(JsonExcludedFile::from)
                .collect(),
            findings: result.findings.iter().map(JsonFinding::from).collect(),
        }
    }
}

/// One finding in the JSON report.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonFinding {
    rule: Rule,
    severity: Severity,
    /// What the finding claims, and therefore what remedy is correct. See the module doc for why
    /// this is not sanitized the way `message`/`titles`/`locations[].path` are.
    verdict: Verdict,
    message: String,
    locations: Vec<JsonLocation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    registration_files: Vec<Vec<String>>,
    titles: Vec<String>,
    /// Per-participant inert (`.skip`/`.todo`) flags, parallel to `locations` and `titles`.
    ///
    /// The message also names inert participants in prose, for humans. This field is the machine
    /// answer, so an integration never has to parse that prose — and cannot break when it is
    /// reworded.
    inert: Vec<bool>,
    /// Whether a `--baseline` file accounts for this finding, and it therefore does not affect
    /// the exit code. See the module doc for why it is flagged rather than omitted.
    baselined: bool,
    /// A location-independent identity for this finding. See
    /// [`crate::model::Finding::fingerprint`].
    fingerprint: String,
}

impl From<&Finding> for JsonFinding {
    /// Convert a finding to its JSON fields, sanitizing report text and retaining structured
    /// provenance.
    fn from(finding: &Finding) -> Self {
        Self {
            rule: finding.rule,
            severity: finding.severity,
            verdict: finding.rule.verdict(),
            message: report_safe(&finding.message),
            locations: finding.locations.iter().map(JsonLocation::from).collect(),
            registration_files: report_registration_files(finding),
            titles: finding
                .titles
                .iter()
                .map(|title| report_safe(title))
                .collect(),
            inert: finding.inert.clone(),
            baselined: finding.baselined,
            fingerprint: finding.fingerprint(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reporters::test_support::{append_third_participant, cross_file_result};
    use serde_json::Value;
    use std::path::PathBuf;

    /// The first location's path of the first finding (or record) -- the field every
    /// hostile-path test reads back.
    fn first_path(value: &serde_json::Value) -> &str {
        value["findings"][0]["locations"][0]["path"]
            .as_str()
            .unwrap()
    }

    /// Build a representative cross-file duplicate result for JSON reporter assertions.
    fn sample_result() -> AnalysisResult {
        cross_file_result("identical bodies under an equivalent context")
    }

    /// Render the result through the JSON reporter and parse it, panicking on serialization or
    /// JSON errors.
    fn render_value(result: &AnalysisResult) -> Value {
        let mut buffer = Vec::new();
        JsonReporter::new()
            .write_report(result, &mut buffer)
            .expect("writing to a Vec<u8> never fails");
        serde_json::from_slice(&buffer).expect("reporter must emit valid JSON")
    }

    /// Verify inert flags are emitted per participant.
    #[test]
    fn inert_flags_are_emitted_per_participant() {
        let mut result = sample_result();
        result.findings[0].inert = vec![false, true];
        let value = render_value(&result);
        assert_eq!(
            value["findings"][0]["inert"],
            serde_json::json!([false, true])
        );
    }

    /// Verify round trips through serde json value with exact structure.
    #[test]
    fn round_trips_through_serde_json_value_with_exact_structure() {
        let value = render_value(&sample_result());

        assert_eq!(value["schemaVersion"], "0.1");
        assert_eq!(value["testsAnalyzed"], 4);
        assert_eq!(value["filesAnalyzed"], 2);
        assert_eq!(value["incomplete"], false);

        let findings = value["findings"].as_array().expect("findings is an array");
        assert_eq!(findings.len(), 1);

        let finding = &findings[0];
        assert_eq!(finding["rule"], "duplicate-test");
        assert_eq!(finding["severity"], "error");
        assert_eq!(
            finding["message"],
            "identical bodies under an equivalent context"
        );

        let locations = finding["locations"]
            .as_array()
            .expect("locations is an array");
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0]["path"], "src/a.test.ts");
        assert_eq!(locations[0]["line"], 10);
        assert_eq!(locations[0]["column"], 3);
        assert_eq!(locations[0]["endLine"], 10);
        assert_eq!(locations[0]["endColumn"], 8);
        assert_eq!(locations[1]["path"], "src/b.test.ts");

        let titles = finding["titles"].as_array().expect("titles is an array");
        assert_eq!(titles[0], "suite > a");
        assert_eq!(titles[1], "suite > b");

        // Exact top-level key set: catches accidental field additions or renames the schema
        // doc comment does not mention.
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "blindFiles",
                "excludedFiles",
                "exclusions",
                "filesAnalyzed",
                "findings",
                "incomplete",
                "incompleteCauses",
                "modes",
                "schemaVersion",
                "testsAnalyzed",
                "unresolvedTests"
            ]
        );
    }

    /// Verify finding key set matches the documented schema including inert.
    #[test]
    fn finding_key_set_matches_the_documented_schema_including_inert() {
        // R4: the module doc's schema example previously omitted `inert`, even though it is
        // always serialized. Pin the finding object's exact key set so the doc comment and the
        // real output can never drift apart again.
        let value = render_value(&sample_result());
        let mut keys: Vec<&str> = value["findings"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "baselined",
                "fingerprint",
                "inert",
                "locations",
                "message",
                "rule",
                "severity",
                "titles",
                "verdict",
            ]
        );
    }

    /// Verify zero findings serializes a clear empty array not null.
    #[test]
    fn zero_findings_serializes_a_clear_empty_array_not_null() {
        let value = render_value(&AnalysisResult::default());
        assert_eq!(value["findings"].as_array().unwrap().len(), 0);
        assert_eq!(value["testsAnalyzed"], 0);
        assert_eq!(value["filesAnalyzed"], 0);
        assert_eq!(value["incomplete"], false);
    }

    /// Verify incomplete flag round trips.
    #[test]
    fn incomplete_flag_round_trips() {
        let mut result = sample_result();
        result.incomplete_causes.pair_budget_exhausted = true;
        let value = render_value(&result);
        assert_eq!(value["incomplete"], true);
    }

    /// Verify finding with more than two locations serializes all of them.
    #[test]
    fn finding_with_more_than_two_locations_serializes_all_of_them() {
        let mut result = sample_result();
        append_third_participant(&mut result);
        let value = render_value(&result);
        let locations = value["findings"][0]["locations"].as_array().unwrap();
        assert_eq!(locations.len(), 3);
        assert_eq!(locations[2]["path"], "src/c.test.ts");
        assert_eq!(value["findings"][0]["titles"][2], "suite > c");
    }

    /// Verify very long title is preserved in full.
    #[test]
    fn very_long_title_is_preserved_in_full() {
        let mut result = sample_result();
        let long_title = "x".repeat(10_000);
        result.findings[0].titles[0] = long_title.clone();
        let value = render_value(&result);
        assert_eq!(
            value["findings"][0]["titles"][0].as_str().unwrap().len(),
            10_000
        );
    }

    /// Verify title with newlines and ansi escapes stays valid json with escapes stripped.
    #[test]
    fn title_with_newlines_and_ansi_escapes_stays_valid_json_with_escapes_stripped() {
        let mut result = sample_result();
        result.findings[0].titles[0] = "line one\nline two\x1b[31mred\x1b[0m".to_string();
        let value = render_value(&result);
        let title = value["findings"][0]["titles"][0].as_str().unwrap();
        assert!(!title.contains('\u{1b}'));
        // Structured output can safely keep an intentional line break; it is JSON-escaped, not
        // written raw, so it cannot forge a new record the way it could in the terminal report.
        assert!(title.contains('\n'));
    }

    /// Verify non ascii path and title round trip byte for byte.
    #[test]
    fn non_ascii_path_and_title_round_trip_byte_for_byte() {
        let mut result = sample_result();
        result.findings[0].locations[0].path = PathBuf::from("src/tëst-ünïcödé/a.test.ts");
        result.findings[0].titles[0] = "suite > 日本語のタイトル".to_string();
        let value = render_value(&result);
        assert_eq!(
            value["findings"][0]["locations"][0]["path"],
            "src/tëst-ünïcödé/a.test.ts"
        );
        assert_eq!(
            value["findings"][0]["titles"][0],
            "suite > 日本語のタイトル"
        );
    }

    /// Verify windows style backslashes are normalized to forward slashes.
    #[test]
    fn windows_style_backslashes_are_normalized_to_forward_slashes() {
        let mut result = sample_result();
        result.findings[0].locations[0].path = PathBuf::from("src\\nested\\a.test.ts");
        let value = render_value(&result);
        assert_eq!(
            value["findings"][0]["locations"][0]["path"],
            "src/nested/a.test.ts"
        );
    }

    /// Strip bidirectional formatting controls from JSON finding locations.
    #[test]
    fn path_with_bidi_override_is_stripped() {
        let mut result = sample_result();
        result.findings[0].locations[0].path =
            PathBuf::from("src/safe\u{202e}gnisaeler-tset\u{202c}evil.test.ts");
        let value = render_value(&result);
        let path = first_path(&value);
        assert!(!path.contains('\u{202e}'));
        assert!(!path.contains('\u{202c}'));
    }

    /// Keep invisible zero-width text out of JSON document source locations.
    #[test]
    fn path_with_zero_width_character_is_stripped() {
        let mut result = sample_result();
        result.findings[0].locations[0].path = PathBuf::from("src/a\u{200b}.test.ts");
        let value = render_value(&result);
        let path = first_path(&value);
        assert!(!path.contains('\u{200b}'));
    }

    /// Remove terminal control bytes from the JSON finding location field.
    #[test]
    fn path_with_control_character_is_stripped() {
        let mut result = sample_result();
        result.findings[0].locations[0].path = PathBuf::from("src/a\u{7}.test.ts");
        let value = render_value(&result);
        let path = first_path(&value);
        assert!(!path.contains('\u{7}'));
    }

    /// Verify compact mode produces the same structure as pretty mode.
    #[test]
    fn compact_mode_produces_the_same_structure_as_pretty_mode() {
        let mut buffer = Vec::new();
        JsonReporter { pretty: false }
            .write_report(&sample_result(), &mut buffer)
            .unwrap();
        let compact: Value = serde_json::from_slice(&buffer).unwrap();
        let pretty = render_value(&sample_result());
        assert_eq!(compact, pretty);
    }

    /// Verify verdict matches the rule that produced the finding.
    #[test]
    fn verdict_matches_the_rule_that_produced_the_finding() {
        let mut result = sample_result();
        result.findings[0].rule = Rule::ParameterizationCandidate;
        let value = render_value(&result);
        assert_eq!(value["findings"][0]["verdict"], "consolidatable");
    }

    /// Verify verdict is emitted for every rule.
    #[test]
    fn verdict_is_emitted_for_every_rule() {
        for rule in crate::model::Rule::ALL {
            let mut result = sample_result();
            result.findings[0].rule = *rule;
            let value = render_value(&result);
            let expected = match rule.verdict() {
                crate::model::Verdict::Redundant => "redundant",
                crate::model::Verdict::Consolidatable => "consolidatable",
                crate::model::Verdict::NameCollision => "name-collision",
                crate::model::Verdict::SimilarUncertain => "similar-uncertain",
            };
            assert_eq!(
                value["findings"][0]["verdict"], expected,
                "rule {:?} did not serialize its verdict correctly",
                rule
            );
        }
    }

    /// Verify fingerprint is emitted and matches the model method.
    #[test]
    fn fingerprint_is_emitted_and_matches_the_model_method() {
        let result = sample_result();
        let value = render_value(&result);
        assert_eq!(
            value["findings"][0]["fingerprint"],
            result.findings[0].fingerprint()
        );
    }

    /// Verify fingerprint is stable across two renders of the same result.
    #[test]
    fn fingerprint_is_stable_across_two_renders_of_the_same_result() {
        let result = sample_result();
        let first = render_value(&result)["findings"][0]["fingerprint"].clone();
        let second = render_value(&result)["findings"][0]["fingerprint"].clone();
        assert_eq!(first, second);
    }
}
