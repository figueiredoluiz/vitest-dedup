//! SARIF 2.1.0 reporter, for GitHub code scanning and any other SARIF consumer.
//!
//! ```text
//! {
//!   "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
//!   "version": "2.1.0",
//!   "runs": [{
//!     "tool": { "driver": { "name": "VitestDedup", "rules": [ ... one per Rule::ALL ... ] } },
//!     "invocations": [{ "executionSuccessful": true, "properties": { "incomplete": false, ... } }],
//!     "results": [{
//!       "ruleId": "duplicate-test",
//!       "level": "error",
//!       "message": { "text": "2 tests have identical bodies under an equivalent context" },
//!       "locations": [{ "physicalLocation": { ... }, "message": { "text": "suite > a" } }],
//!       "relatedLocations": [{ "physicalLocation": { ... }, "message": { "text": "suite > b" } }],
//!       "partialFingerprints": { "vitestDedupFingerprint/v1": "vitestDedupFingerprint/v1:0123456789abcdef" },
//!       "properties": { "verdict": "redundant", "inert": [false, false], "baselined": false }
//!     }]
//!   }]
//! }
//! ```
//!
//! # Rules come from `Rule::ALL`, never hand-written
//!
//! `driver.rules[]` is built by mapping over [`Rule::ALL`] -- the same enumeration point
//! configuration and every other reporter uses -- so a rule added there appears here
//! automatically instead of needing a second, easily-forgotten registration.
//!
//! # `Severity::Off`
//!
//! An off rule (`subsumed-test` by default) never produces a finding: `analysis.rs` skips
//! computing it entirely rather than computing and filtering, so `Severity::Off` can never appear
//! on a [`Finding`] and therefore never appears in `results[].level`. It still describes a rule
//! this tool *implements*, though, so it still gets a `driver.rules[]` entry -- with
//! `defaultConfiguration.enabled: false` as the authoritative "does not run by default" signal.
//! `defaultConfiguration.level` still needs some value (SARIF has no "off" level), so it reuses
//! the same mapping as an active `Severity::Off` finding would get if configuration ever produced
//! one: [`sarif_level`] maps `Off` to `"note"`, matching CukeDedup's own choice for the identical
//! case (`cuke-dedup/src/reporters/sarif.rs`). `enabled: false` is what a consumer should actually
//! key on; `level: "note"` here is a placeholder that avoids omitting a field SARIF expects,
//! never a claim about what severity a user's override would produce.
//!
//! # `Verdict`
//!
//! SARIF has no concept matching [`Rule::verdict`] (what a finding claims, and therefore which
//! remedy is correct -- `docs/reports.md#verdict-and-fingerprint`). Dropping it would leave a
//! SARIF consumer to hardcode the same rule-to-remedy table `json`/`jsonl` exist to spare a
//! consumer from re-deriving. It surfaces instead in `results[].properties.verdict`, SARIF's
//! documented extension point for tool-specific data, serialized by the same closed enum
//! [`crate::model::Verdict`] derives elsewhere -- never free text, so it carries none of the
//! injection surface `message` and the location `message`s below do.
//!
//! # `partialFingerprints` and its collision behavior
//!
//! Keyed by [`FINGERPRINT_VERSION`] (`vitestDedupFingerprint/v1`) rather than CukeDedup's
//! `cukeDedupFingerprint/v2` -- a distinct name for a distinct derivation, so the two tools'
//! fingerprints can never be mistaken for each other if their reports are ever merged. The value
//! is exactly [`Finding::fingerprint`]'s output, unmodified: that method's doc comment is explicit
//! that two genuinely different findings can legitimately share a fingerprint (a spanning set of
//! pairwise findings drawn from one run of more than two identically-titled tests), and SARIF's
//! own spec is explicit that `partialFingerprints` values are not guaranteed unique either within
//! a run or across runs. Emitting anything *more* unique here -- folding in a location or a result
//! index, say -- would misrepresent the identity this tool actually computed as a stronger
//! guarantee than it makes. A consumer that needs to split two colliding results already has
//! `locations` to do it with.
//!
//! # Per-participant title and inert flag
//!
//! `json`/`jsonl` expose `titles`/`inert` as top-level arrays parallel to `locations`, per
//! `docs/reports/findings.md`'s "machine formats expose structured fields" rule. SARIF has no array-of-
//! strings extension point at the result level that keeps that parallel structure, but every
//! `location` in `locations`/`relatedLocations` already carries its own optional `message`, which
//! is exactly a per-location piece of text -- so each participant's title lands in *that*
//! location's `message.text`, sanitized the same way `message` and `locations[].path` are
//! everywhere else, and its inert flag lands in that location's `properties.inert`. A consumer
//! reading one `location` gets both facts about that participant without cross-indexing two
//! parallel arrays.
//!
//! # Run-level context
//!
//! `invocations[0].properties.incomplete` mirrors [`AnalysisResult::incomplete`]: per
//! `docs/incomplete-runs.md`, "the absence of a finding proves nothing" when a run is incomplete, and a
//! SARIF consumer has no other way to learn that short of comparing finding counts across runs.
//! `executionSuccessful` is always `true` here because this reporter only ever runs after the
//! analysis pipeline has already completed -- an operational failure exits before any reporter is
//! invoked, so this field cannot usefully vary and is not a proxy for `incomplete`, which is a
//! coverage fact, not a crash.
//!
//! `invocations[0].properties.incompleteCauses` carries [`crate::model::IncompleteCauses`] as the
//! `json`/`jsonl` reports serialize it, on a complete run too (all false/zero). `incomplete` alone
//! cannot tell "the analyzer was undersized for this corpus" from "twelve files failed to read",
//! which call for opposite responses.
//!
//! `invocations[0].properties.modes` mirrors [`crate::model::ModeSummary`], for the same reason:
//! `withheldUnchanged` counts findings this log deliberately does not contain, and a consumer that
//! only read `results[]` would have no way to tell a `--changed-since` run's narrowed view from a
//! clean repository. It sits alongside `incomplete` (which such a run also sets) rather than
//! replacing it.
//!
//! # Baselined results
//!
//! A finding a `--baseline` file accounts for is emitted with a SARIF `suppressions` entry of
//! `kind: "external"` -- SARIF's own vocabulary for "suppressed by something outside the tool",
//! which is exactly what a committed baseline is. A consumer that understands suppressions (GitHub
//! code scanning among them) then stops alerting on it without this tool having to drop the result
//! from the log, which is the same "reported but not gating" shape every other reporter uses.
//! `properties.baselined` carries the same fact as a plain boolean for consumers that do not read
//! `suppressions`. Every result carries the array -- empty when the finding is not baselined --
//! because SARIF reads an absent `suppressions` as "suppression state unknown", which this
//! reporter never is.
//!
//! # Output safety and determinism
//!
//! Every interpolated string -- `message.text`, a location's `message.text`, and
//! `artifactLocation.uri` -- goes through [`report_safe`]/[`report_safe_path`], identically to the
//! `json` reporter (`docs/reports/safety.md`'s output-safety contract, applied "by every reporter"),
//! and `artifactLocation.uri` is then encoded as an RFC 3986 URI reference ([`uri_reference`]):
//! SARIF requires that form, a consumer resolves it by percent-decoding, and the sanitized path
//! verbatim would let a `#` in a filename truncate the URI into a fragment while a literal `%41`
//! decodes to `A` -- the resolved URI naming a different file than the finding points at.
//! `verdict` and `partialFingerprints` are not, for the same reason `json` does not sanitize them:
//! both are closed, tool-defined values, never text copied out of the analyzed repository.
//! `serde_json` is built with the `preserve_order` feature crate-wide (`Cargo.toml`), so the
//! `serde_json::json!` object literals below serialize their keys in the order written here --
//! this reporter's determinism (byte-identical output for the same [`AnalysisResult`]) follows
//! from `AnalysisResult::findings` already being sorted before any reporter sees it, not from
//! anything specific to this file.

use super::shared::{report_registration_files, report_safe, report_safe_path};
use super::Reporter;
use crate::model::{
    AnalysisResult, Finding, Location, Rule, Severity, Verdict, FINGERPRINT_VERSION,
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::Write;

/// The schema this reporter's output validates against. Not embedded to make a network call --
/// GitHub's own SARIF ingestion resolves `$schema` the same way -- but so a consumer that does
/// fetch it, or a human comparing this file to the spec, is pointed at the exact same document
/// this reporter was authored and manually checked against (see this module's tests and the
/// PR/commit description for how that check was run: the schema was fetched from this URL and
/// validated offline with a JSON Schema validator, not vendored into the crate).
const SARIF_SCHEMA_URI: &str = "https://json.schemastore.org/sarif-2.1.0.json";

/// This tool's identity in `tool.driver`, kept alongside the reporter that is its only user
/// rather than in `Cargo.toml` lookup, since a driver name is SARIF vocabulary, not a Cargo
/// concept, and the two are not obligated to match.
const TOOL_NAME: &str = "VitestDedup";

/// Renders an [`AnalysisResult`] as a SARIF 2.1.0 log.
#[derive(Debug, Clone, Copy, Default)]
pub struct SarifReporter;

impl SarifReporter {
    /// Creates the reporter, matching the `::new()` convention every other reporter in this crate
    /// uses at its call site.
    pub fn new() -> Self {
        Self
    }
}

impl Reporter for SarifReporter {
    /// Render the analysis as pretty SARIF JSON and attach context to serialization or writer
    /// failures.
    fn write_report(&self, result: &AnalysisResult, writer: &mut dyn Write) -> Result<()> {
        let document = render(result);
        serde_json::to_writer_pretty(writer, &document).context("failed to serialize SARIF report")
    }
}

/// Maps this tool's severity to a SARIF result/rule level. See the module doc's "`Severity::Off`"
/// section for why `Off` maps to `"note"` even though it can never actually back a `results[]`
/// entry.
fn sarif_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Off => "note",
    }
}

/// Builds the complete SARIF document for `result`.
fn render(result: &AnalysisResult) -> Value {
    json!({
        "$schema": SARIF_SCHEMA_URI,
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": driver() },
            // Columns in this crate's `Position` are UTF-16 code units (`src/model.rs`'s own
            // doc comment: "matching editor and SARIF conventions") -- which is also SARIF's
            // default when `columnKind` is omitted, but stated explicitly here so the choice is
            // self-documenting rather than relying on a reader already knowing the SARIF default.
            "columnKind": "utf16CodeUnits",
            "invocations": [invocation(result)],
            "results": result.findings.iter().map(sarif_result).collect::<Vec<_>>(),
        }]
    })
}

/// `tool.driver`: this tool's identity plus every rule it implements, from [`Rule::ALL`].
fn driver() -> Value {
    json!({
        "name": TOOL_NAME,
        "semanticVersion": env!("CARGO_PKG_VERSION"),
        "informationUri": "https://github.com/figueiredoluiz/vitest-dedup",
        "rules": Rule::ALL.iter().map(|rule| sarif_rule(*rule)).collect::<Vec<_>>(),
    })
}

/// One `driver.rules[]` entry. No `helpUri`: there is no per-rule documentation anchor to point
/// at today (`README.md`'s rule table is a plain GFM table -- GitHub does not anchor individual
/// rows, only headings), and fabricating one that resolves to the whole README, or to nothing,
/// would misrepresent a link as rule-specific documentation when it is not. Omitting the field
/// entirely is the honest choice; add it the day a real per-rule anchor exists.
fn sarif_rule(rule: Rule) -> Value {
    let default_severity = rule.default_severity();
    json!({
        "id": rule.id(),
        "name": rule.id(),
        "shortDescription": { "text": rule.short_description() },
        "fullDescription": { "text": rule.full_description() },
        "defaultConfiguration": {
            "level": sarif_level(default_severity),
            "enabled": default_severity != Severity::Off,
        },
    })
}

/// `invocations[0]`: run-level context a SARIF consumer cannot recover from `results[]` alone.
/// See the module doc's "Run-level context" section.
fn invocation(result: &AnalysisResult) -> Value {
    json!({
        "executionSuccessful": true,
        "properties": {
            "incomplete": result.incomplete(),
            // Serialized from the struct, so a new cause appears without a second registration.
            "incompleteCauses": result.incomplete_causes,
            "testsAnalyzed": result.tests_analyzed,
            "filesAnalyzed": result.files_analyzed,
            "unresolvedTests": result.unresolved_tests,
            "modes": result.modes,
        },
    })
}

/// One `results[]` entry for `finding`.
///
/// # Panics
///
/// Never in practice: [`Finding::locations`]'s doc comment guarantees at least two participants,
/// enforced by every code path that constructs one (`analysis.rs`). A `Finding` violating that
/// invariant is a bug upstream of this reporter, not something this function should paper over by
/// emitting a malformed SARIF result.
fn sarif_result(finding: &Finding) -> Value {
    let mut locations = finding
        .locations
        .iter()
        .zip(&finding.titles)
        .zip(&finding.inert)
        .map(|((location, title), inert)| sarif_location(location, title, *inert));
    let primary = locations
        .next()
        .expect("a Finding always carries at least two locations");
    let related: Vec<Value> = locations.collect();

    // SARIF distinguishes three states here, and this reporter always knows which one applies:
    // an *absent* `suppressions` means the suppression state is unknown, an *empty* array means
    // definitely not suppressed, and a populated one names why. Emitting the array only for
    // baselined results would claim ignorance about every ordinary finding, so it is always
    // present and simply empty when the finding is not baselined.
    let suppressions = if finding.baselined {
        json!([{
            "kind": "external",
            "justification": "Recorded in this repository's VitestDedup baseline.",
        }])
    } else {
        json!([])
    };
    let mut result = json!({
        "ruleId": finding.rule.id(),
        "level": sarif_level(finding.severity),
        "message": { "text": report_safe(&finding.message) },
        "locations": [primary],
        "relatedLocations": related,
        "partialFingerprints": { FINGERPRINT_VERSION: finding.fingerprint() },
        "suppressions": suppressions,
        "properties": {
            "verdict": verdict_id(finding.rule.verdict()),
            "inert": finding.inert,
            "baselined": finding.baselined,
        },
    });
    if !finding.registration_files.is_empty() {
        result["properties"]["registrationFiles"] = json!(report_registration_files(finding));
    }
    result
}

/// The hex digits percent-encoding spells its bytes with. Upper-case, as RFC 3986 §2.1 permits
/// and every consumer decodes case-insensitively.
const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// A path as an RFC 3986 URI reference, which is what SARIF 2.1.0 requires of
/// `artifactLocation.uri` and what consumers (GitHub code scanning among them) resolve by
/// percent-decoding.
///
/// Every UTF-8 byte that is neither unreserved (`A-Z a-z 0-9 - . _ ~`) nor the `/` separator is
/// percent-encoded, so `#` cannot become a fragment (`%23`), `[`/`]`/space become legal
/// (`%5B`/`%5D`/`%20`), non-ASCII travels as its UTF-8 triples, and a literal `%` in a filename
/// re-encodes as `%25` -- a file genuinely named `a%41.test.ts` emits `a%2541.test.ts`, which
/// decodes back to `a%41.test.ts` instead of collapsing to `aA.test.ts`. Sub-delims (`+ & = ; ,
/// $`) are legal path characters and are encoded anyway: unreserved-plus-separator is the
/// simplest rule that is valid for every consumer. Ordinary clean paths are unchanged, so the
/// other reporters' plain-string path contract and this URI form stay byte-identical wherever
/// the path never needed encoding.
fn uri_reference(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &byte in path.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    out
}

/// One participant location, with its title and inert flag attached the way the module doc's
/// "Per-participant title and inert flag" section describes.
fn sarif_location(location: &Location, title: &str, inert: bool) -> Value {
    json!({
        "physicalLocation": {
            // The sanitized, forward-slash, analysis-root-relative path every other reporter
            // emits as a plain string (`json.rs`'s `JsonLocation::path`) -- encoded into the
            // URI-reference form only SARIF requires here, because a consumer that resolves
            // `uri` decodes it, and the other reporters' consumers read it as a plain string.
            "artifactLocation": { "uri": uri_reference(&report_safe_path(&location.path)) },
            "region": {
                "startLine": location.span.start.line,
                "startColumn": location.span.start.column,
                "endLine": location.span.end.line,
                "endColumn": location.span.end.column,
            },
        },
        "message": { "text": report_safe(title) },
        "properties": { "inert": inert },
    })
}

/// [`Verdict`]'s stable, kebab-case wire name, reusing its `Serialize` impl (the same one `json`
/// emits through) rather than a second hand-written match -- so a variant renamed or added there
/// cannot silently diverge from what this reporter emits.
fn verdict_id(verdict: Verdict) -> Value {
    serde_json::to_value(verdict).expect("Verdict serializes to a JSON string infallibly")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{IncompleteCauses, Rule, Severity};
    use crate::reporters::test_support::{
        all_fired_causes, append_third_participant, cross_file_result,
    };
    use std::path::PathBuf;

    /// Build a cross-file finding fixture for SARIF rule, result and location assertions.
    fn sample_result() -> AnalysisResult {
        cross_file_result("identical bodies under an equivalent context")
    }

    /// Serialize through the SARIF reporter and parse the document, panicking if either step
    /// fails.
    fn render_value(result: &AnalysisResult) -> Value {
        let mut buffer = Vec::new();
        SarifReporter::new()
            .write_report(result, &mut buffer)
            .expect("writing to a Vec<u8> never fails");
        serde_json::from_slice(&buffer).expect("reporter must emit valid JSON")
    }

    /// Check SARIF output: round trips through serde json and carries the required top level
    /// sarif fields.
    #[test]
    fn round_trips_through_serde_json_and_carries_the_required_top_level_sarif_fields() {
        let value = render_value(&sample_result());
        assert_eq!(value["version"], "2.1.0");
        assert!(value["$schema"].as_str().unwrap().contains("sarif-2.1.0"));
        let run = &value["runs"][0];
        assert_eq!(run["tool"]["driver"]["name"], "VitestDedup");
        assert!(run["tool"]["driver"]["rules"].is_array());
        assert!(run["invocations"].is_array());
        assert!(run["results"].is_array());
    }

    /// Check SARIF output: rule list matches rule all exactly.
    #[test]
    fn rule_list_matches_rule_all_exactly() {
        let value = render_value(&sample_result());
        let rules = value["runs"][0]["tool"]["driver"]["rules"]
            .as_array()
            .unwrap();
        assert_eq!(rules.len(), Rule::ALL.len());
        let emitted_ids: std::collections::BTreeSet<&str> = rules
            .iter()
            .map(|rule| rule["id"].as_str().unwrap())
            .collect();
        let expected_ids: std::collections::BTreeSet<&str> =
            Rule::ALL.iter().map(Rule::id).collect();
        assert_eq!(emitted_ids, expected_ids);
    }

    /// Check SARIF output: every rule entry carries a level a description and no help uri.
    #[test]
    fn every_rule_entry_carries_a_level_a_description_and_no_help_uri() {
        let value = render_value(&sample_result());
        for rule in value["runs"][0]["tool"]["driver"]["rules"]
            .as_array()
            .unwrap()
        {
            assert!(rule["shortDescription"]["text"].as_str().is_some());
            assert!(rule["fullDescription"]["text"].as_str().is_some());
            assert!(rule["defaultConfiguration"]["level"].as_str().is_some());
            assert!(rule["defaultConfiguration"]["enabled"].is_boolean());
            assert!(
                rule.get("helpUri").is_none(),
                "no per-rule documentation anchor exists yet; helpUri must stay omitted, not \
                 fabricated"
            );
        }
    }

    /// Check SARIF output: off by default rule is listed disabled and never backs a result.
    #[test]
    fn off_by_default_rule_is_listed_disabled_and_never_backs_a_result() {
        let value = render_value(&sample_result());
        let rules = value["runs"][0]["tool"]["driver"]["rules"]
            .as_array()
            .unwrap();
        // `subsumed-test` is the rule that ships Off (`model.rs`'s severity table). It is found by
        // id rather than by scanning for whichever rule happens to be disabled, so that a default
        // flip fails this assertion and forces the pin to be re-aimed deliberately -- the way
        // `cross-file-duplicate`, which this test used to name, did when it became a Warning.
        let disabled = rules
            .iter()
            .find(|rule| rule["id"] == "subsumed-test")
            .expect("subsumed-test must still be listed even though it is off by default");
        assert_eq!(disabled["defaultConfiguration"]["enabled"], false);
        assert_eq!(disabled["defaultConfiguration"]["level"], "note");

        for result in value["runs"][0]["results"].as_array().unwrap() {
            assert_ne!(
                result["level"], "note",
                "Severity::Off can never back an emitted finding, so no result should carry the \
                 placeholder level reserved for an off rule's driver entry"
            );
        }
    }

    /// Check SARIF output: severity maps to the documented sarif level.
    #[test]
    fn severity_maps_to_the_documented_sarif_level() {
        let mut result = sample_result();
        result.findings[0].severity = Severity::Error;
        assert_eq!(
            render_value(&result)["runs"][0]["results"][0]["level"],
            "error"
        );
        result.findings[0].severity = Severity::Warning;
        assert_eq!(
            render_value(&result)["runs"][0]["results"][0]["level"],
            "warning"
        );
    }

    /// Check SARIF output: verdict is emitted in properties for every rule.
    #[test]
    fn verdict_is_emitted_in_properties_for_every_rule() {
        for rule in Rule::ALL {
            let mut result = sample_result();
            result.findings[0].rule = *rule;
            let value = render_value(&result);
            let expected = match rule.verdict() {
                Verdict::Redundant => "redundant",
                Verdict::Consolidatable => "consolidatable",
                Verdict::NameCollision => "name-collision",
                Verdict::SimilarUncertain => "similar-uncertain",
            };
            assert_eq!(
                value["runs"][0]["results"][0]["properties"]["verdict"], expected,
                "rule {rule:?} did not surface its verdict in properties"
            );
        }
    }

    /// Check SARIF output: fingerprint is keyed by the versioned label and matches the model
    /// method.
    #[test]
    fn fingerprint_is_keyed_by_the_versioned_label_and_matches_the_model_method() {
        let result = sample_result();
        let value = render_value(&result);
        let fingerprints = &value["runs"][0]["results"][0]["partialFingerprints"];
        assert_eq!(
            fingerprints[FINGERPRINT_VERSION],
            result.findings[0].fingerprint()
        );
        // Exactly one key: this reporter never adds a stronger, location-based disambiguator
        // than Finding::fingerprint() itself provides.
        assert_eq!(fingerprints.as_object().unwrap().len(), 1);
    }

    /// Check SARIF output: primary location and related locations split first from rest.
    #[test]
    fn primary_location_and_related_locations_split_first_from_rest() {
        let mut result = sample_result();
        append_third_participant(&mut result);
        result.findings[0].inert.push(false);
        let value = render_value(&result);
        let sarif_result = &value["runs"][0]["results"][0];
        assert_eq!(sarif_result["locations"].as_array().unwrap().len(), 1);
        assert_eq!(
            sarif_result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "src/a.test.ts"
        );
        assert_eq!(sarif_result["locations"][0]["message"]["text"], "suite > a");

        let related = sarif_result["relatedLocations"].as_array().unwrap();
        assert_eq!(related.len(), 2);
        assert_eq!(
            related[0]["physicalLocation"]["artifactLocation"]["uri"],
            "src/b.test.ts"
        );
        assert_eq!(related[1]["message"]["text"], "suite > c");
    }

    /// Check SARIF output: region uses the finding's span.
    #[test]
    fn region_uses_the_finding_s_span() {
        let value = render_value(&sample_result());
        let region = &value["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region["startLine"], 10);
        assert_eq!(region["startColumn"], 3);
        assert_eq!(region["endLine"], 10);
        assert_eq!(region["endColumn"], 8);
    }

    /// Check SARIF output: inert flag is attached to its own location.
    #[test]
    fn inert_flag_is_attached_to_its_own_location() {
        let mut result = sample_result();
        result.findings[0].inert = vec![false, true];
        let value = render_value(&result);
        let sarif_result = &value["runs"][0]["results"][0];
        assert_eq!(sarif_result["locations"][0]["properties"]["inert"], false);
        assert_eq!(
            sarif_result["relatedLocations"][0]["properties"]["inert"],
            true
        );
        assert_eq!(
            sarif_result["properties"]["inert"],
            serde_json::json!([false, true])
        );
    }

    /// Check SARIF output: incomplete run is visible at the invocation level.
    #[test]
    fn incomplete_run_is_visible_at_the_invocation_level() {
        let mut result = sample_result();
        result.incomplete_causes.pair_budget_exhausted = true;
        let value = render_value(&result);
        assert_eq!(
            value["runs"][0]["invocations"][0]["properties"]["incomplete"],
            true
        );
        assert_eq!(
            value["runs"][0]["invocations"][0]["executionSuccessful"],
            true
        );
    }

    /// Check SARIF output: the invocation carries every incompleteness cause, keyed and valued
    /// exactly as the struct serializes. The expectation is derived from `IncompleteCauses`
    /// itself, so a cause the invocation omitted -- or a hand-built key list that dropped one --
    /// fails here rather than shipping silently, which is how this property came to omit the
    /// causes entirely.
    #[test]
    fn invocation_carries_every_incompleteness_cause() {
        let mut result = sample_result();
        let causes = all_fired_causes();
        result.incomplete_causes = causes.clone();
        let value = render_value(&result);
        let emitted = &value["runs"][0]["invocations"][0]["properties"]["incompleteCauses"];
        let expected = serde_json::to_value(&causes).unwrap();
        assert_eq!(
            emitted, &expected,
            "every cause must carry its key and its value"
        );
    }

    /// Check SARIF output: a complete run still carries the causes object, all false and zero --
    /// the same unconditional emission `json`/`jsonl` practice, so a consumer never has to
    /// distinguish "no causes" from "causes not reported".
    #[test]
    fn complete_run_still_carries_the_causes_object() {
        let value = render_value(&sample_result());
        let properties = &value["runs"][0]["invocations"][0]["properties"];
        assert_eq!(properties["incomplete"], false);
        assert_eq!(
            properties["incompleteCauses"],
            serde_json::to_value(IncompleteCauses::default()).unwrap()
        );
    }

    /// Percent-decode a URI reference the way a consumer resolves it, so the tests can assert
    /// the round trip `uri_reference` exists to preserve.
    fn decode_uri_reference(uri: &str) -> String {
        let bytes = uri.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' && index + 2 < bytes.len() {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                if let (Some(high), Some(low)) = (high, low) {
                    out.push((high * 16 + low) as u8);
                    index += 3;
                    continue;
                }
            }
            out.push(bytes[index]);
            index += 1;
        }
        String::from_utf8(out).expect("uri_reference only emits valid UTF-8")
    }

    /// `artifactLocation.uri` is a URI reference: separators survive, reserved and non-ASCII
    /// bytes percent-encode, an existing percent sequence in a FILENAME is data (re-encoded,
    /// never decoded), and percent-decoding the result yields the input path exactly.
    #[test]
    fn uri_percent_encodes_reserved_bytes_and_preserves_separators() {
        let cases = [
            // Ordinary paths are byte-identical to the plain-string form every other reporter
            // emits.
            ("src/a.test.ts", "src/a.test.ts"),
            ("a/b-c_d.e~f.ts", "a/b-c_d.e~f.ts"),
            // The URI-hostile shape: brackets illegal in URIs, a space, a fragment delimiter,
            // and a filename that already spells a percent sequence.
            (
                "src/[id]/a b#c%41.test.ts",
                "src/%5Bid%5D/a%20b%23c%2541.test.ts",
            ),
            // Non-ASCII travels as UTF-8 percent triples.
            (
                "héllo/ünïcode.test.ts",
                "h%C3%A9llo/%C3%BCn%C3%AFcode.test.ts",
            ),
            (
                " casos/日本語.test.ts",
                "%20casos/%E6%97%A5%E6%9C%AC%E8%AA%9E.test.ts",
            ),
            // Sub-delims are legal path characters and encode anyway (the contract's rule).
            ("a+b&c=d.test.ts", "a%2Bb%26c%3Dd.test.ts"),
            ("m,n;o$g.test.ts", "m%2Cn%3Bo%24g.test.ts"),
            // A first-segment colon must not introduce a scheme; query syntax is path data.
            ("scheme:name?.test.ts", "scheme%3Aname%3F.test.ts"),
        ];
        for (path, expected) in cases {
            assert_eq!(uri_reference(path), expected, "encoding {path:?}");
            assert_eq!(
                decode_uri_reference(expected),
                path,
                "decoding must return the path exactly: {expected:?}"
            );
        }
    }

    /// A finding whose path carries the URI-hostile shape: the emitted uri encodes it, and the
    /// region and fingerprint beside it are untouched by the encoding.
    #[test]
    fn hostile_path_encodes_in_uri_and_leaves_region_and_fingerprint_alone() {
        let mut result = sample_result();
        result.findings[0].locations[0].path = PathBuf::from("src/[id]/a b#c%41.test.ts");
        result.findings[0].locations[1].path = PathBuf::from("src/日本語.test.ts");
        let value = render_value(&result);
        assert_eq!(
            value["runs"][0]["results"][0]["relatedLocations"][0]["physicalLocation"]
                ["artifactLocation"]["uri"],
            "src/%E6%97%A5%E6%9C%AC%E8%AA%9E.test.ts"
        );
        let physical = &value["runs"][0]["results"][0]["locations"][0]["physicalLocation"];
        assert_eq!(
            physical["artifactLocation"]["uri"],
            "src/%5Bid%5D/a%20b%23c%2541.test.ts"
        );
        // The fixture's first participant stands at line 10, column 3, five columns wide;
        // asserting the exact numbers proves the encoding did not touch the region.
        let region = physical["region"].as_object().unwrap();
        assert_eq!(region["startLine"], 10);
        assert_eq!(region["startColumn"], 3);
        assert_eq!(region["endLine"], 10);
        assert_eq!(region["endColumn"], 8);
        let fingerprints = value["runs"][0]["results"][0]["partialFingerprints"]
            .as_object()
            .unwrap();
        assert_eq!(
            fingerprints[FINGERPRINT_VERSION],
            result.findings[0].fingerprint()
        );
    }

    /// Check SARIF output: hostile title and path are sanitized in message and uri.
    #[test]
    fn hostile_title_and_path_are_sanitized_in_message_and_uri() {
        let mut result = sample_result();
        result.findings[0].titles[0] =
            "line one\nline two\x1b[31mred\x1b[0m\u{200b}hidden".to_string();
        result.findings[0].locations[0].path =
            PathBuf::from("src\\safe\u{202e}gnisaeler-tset\u{202c}evil.test.ts");
        result.findings[0].message = "1 tests\x1b[31m".to_string();
        let value = render_value(&result);

        let title_text = value["runs"][0]["results"][0]["locations"][0]["message"]["text"]
            .as_str()
            .unwrap();
        assert!(!title_text.contains('\u{1b}'));
        assert!(!title_text.contains('\u{200b}'));

        let uri = value["runs"][0]["results"][0]["locations"][0]["physicalLocation"]
            ["artifactLocation"]["uri"]
            .as_str()
            .unwrap();
        assert!(!uri.contains('\u{202e}'));
        assert!(!uri.contains('\u{202c}'));
        assert!(!uri.contains('\\'));
        assert_eq!(
            decode_uri_reference(uri),
            "src/safegnisaeler-tsetevil.test.ts"
        );

        let message_text = value["runs"][0]["results"][0]["message"]["text"]
            .as_str()
            .unwrap();
        assert!(!message_text.contains('\u{1b}'));
    }

    /// Check SARIF output: output is byte identical across two renders of the same result.
    #[test]
    fn output_is_byte_identical_across_two_renders_of_the_same_result() {
        let result = sample_result();
        let mut first = Vec::new();
        let mut second = Vec::new();
        SarifReporter::new()
            .write_report(&result, &mut first)
            .unwrap();
        SarifReporter::new()
            .write_report(&result, &mut second)
            .unwrap();
        assert_eq!(first, second);
    }

    /// Check SARIF output: zero findings still produces a valid document with a full rule list.
    #[test]
    fn zero_findings_still_produces_a_valid_document_with_a_full_rule_list() {
        let value = render_value(&AnalysisResult::default());
        assert_eq!(value["runs"][0]["results"].as_array().unwrap().len(), 0);
        assert_eq!(
            value["runs"][0]["tool"]["driver"]["rules"]
                .as_array()
                .unwrap()
                .len(),
            Rule::ALL.len()
        );
    }
}
