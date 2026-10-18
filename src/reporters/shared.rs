//! Helpers shared by the terminal and JSON reporters.
//!
//! A test title or finding message is copied verbatim from source the analyzer never executes,
//! so it is attacker-controlled the same way a filename or HTTP header is: it can contain
//! control characters, raw ANSI escapes, or invisible Unicode formatting characters chosen to
//! rewrite how the report displays rather than what it says. Every string that reaches a report
//! passes through one of the sanitizers below first, so a malicious title can misinform a reader
//! but cannot forge extra report lines or hide text in an invisible/reordering character.

use crate::model::{AnalysisResult, Finding, IncompleteCauses, Location, ModeSummary, Severity};
use serde::Serialize;
use std::path::Path;

/// Strips characters that could rewrite or misrepresent the report when it is displayed.
///
/// `keep_whitespace` preserves `\n`, `\r`, and `\t` for formats that can represent them safely
/// (JSON escapes them in the encoded string); the terminal reporter cannot allow them through at
/// all, because a literal newline inside a title would let that title forge extra report lines.
fn sanitize(value: &str, keep_whitespace: bool) -> String {
    value
        .chars()
        .filter(|character| {
            let allowed_control = keep_whitespace && matches!(*character, '\n' | '\r' | '\t');
            (!character.is_control() || allowed_control)
                && !matches!(
                    *character,
                    '\u{00ad}'
                        | '\u{061c}'
                        | '\u{200b}'..='\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        // U+2060..=U+206F: the full "General Punctuation" invisible-formatting
                        // block, not just its isolates (U+2066..=U+2069). U+206A..=U+206F are the
                        // deprecated-but-still-live bidi/shaping controls (inhibit/activate
                        // symmetric swapping, Arabic form shaping, national digit shapes) — none
                        // of them are `char::is_control()`, so leaving the range short at U+2069
                        // let them pass straight through into a rendered report.
                        | '\u{2060}'..='\u{206f}'
                        | '\u{feff}'
                        | '\u{e0000}'..='\u{e007f}'
                )
        })
        .collect()
}

/// Sanitizes text for the terminal reporter, where even a bare newline would forge a report
/// line and a bare ESC byte could trigger a terminal escape sequence.
///
/// `pub(crate)` rather than `pub(super)`: this is the same hazard for any raw diagnostic text a
/// caller outside this module prints straight to stdout/stderr (a repository-controlled filename,
/// an error's context, an echoed `--output` path) — the reporters do not have a monopoly on
/// needing it, so callers elsewhere in the crate reuse this instead of re-implementing sanitation.
pub fn terminal_safe(value: &str) -> String {
    sanitize(value, false)
}

/// Sanitizes text for the JSON reporter, which can safely represent intentional line breaks
/// because they are escaped inside a JSON string rather than written raw.
pub(super) fn report_safe(value: &str) -> String {
    sanitize(value, true)
}

/// Renders a path with forward slashes regardless of the host platform's separator.
///
/// A `Location::path` is a relative, analyzer-owned path recorded by the frontend, not a
/// platform-specific display path, so the JSON schema fixes its separator. Without this, the
/// same repository analyzed on Windows and on Linux would produce two different JSON reports.
pub(super) fn forward_slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Renders `path` with forward slashes and then strips it the same way [`report_safe`] strips a
/// title or message.
///
/// A `Location::path` is copied out of the analyzed repository exactly like a test title — it is
/// a filename someone else chose, not something the analyzer generated — so it is attacker
/// controlled in the same way. A bidi override or invisible formatting character embedded in a
/// filename can visually reorder a rendered line just as effectively as one embedded in a title,
/// which is precisely the hazard `docs/reports/safety.md`'s output-safety contract exists to rule out for
/// "all interpolated text." Every reporter must therefore run paths through this before emitting
/// them, not just `forward_slash_path` on its own.
///
/// Sanitization runs strictly *after* separator normalization: `forward_slash_path` only ever
/// turns `\` into `/`, a transformation that cannot introduce a control character, an ANSI escape,
/// or a bidi/invisible codepoint, so doing it first and sanitizing the result can never let
/// normalization reintroduce something the sanitizer just removed. Doing it in the other order
/// would require re-checking that invariant every time `forward_slash_path` changed.
///
/// # Strip, not escape — and the same choice for every reporter
///
/// A path is an identifier a consumer may feed back to a filesystem, not prose, so stripping a
/// character from it — unlike from a title — can leave a string that no longer resolves to the
/// file that produced it. Escaping (e.g. spelling a bidi override out as the six-character
/// sequence `\u{202e}` instead of the raw codepoint) would preserve that round trip instead. This
/// function strips anyway, uniformly across the JSON, JSONL, and HTML
/// reporters (including the HTML report's embedded JSON island), for two reasons:
///
/// 1. The hazard is specifically about *rendering*. A bidi override's entire effect is on how a
///    line displays; it does nothing when a program opens the path as a byte string. Every
///    consumer of these reports eventually puts the path in front of a person — someone reading
///    the JSON file directly, `jsonl` piped through `jq` onto a terminal, the HTML page itself, or
///    an agent that echoes a finding's path back into a chat transcript — so the display hazard
///    applies everywhere these reports go, even the ones nominally "for machines."
/// 2. Nothing in this crate writes to, deletes, or renames a file using a path read back out of a
///    report. The sanitized string is a report artifact, not a handle the tool itself dereferences,
///    so a sanitized path failing to round-trip back to the original file has no destructive
///    effect here — only the display benefit of not being able to lie about which file a finding
///    points at.
///
/// This keeps paths and titles handled identically rather than picking strip for one and escape
/// for the other, which would leave two different safety stories for the same class of input.
pub(super) fn report_safe_path(path: &Path) -> String {
    report_safe(&forward_slash_path(path))
}

/// Optional caller provenance, sanitized uniformly across structured finding reports.
pub(super) fn report_registration_files(finding: &Finding) -> Vec<Vec<String>> {
    finding
        .registration_files
        .iter()
        .map(|files| files.iter().map(|path| report_safe_path(path)).collect())
        .collect()
}

/// The lowercase, report-stable spelling of a severity, matching `Rule::id`'s convention of
/// stable kebab-case identifiers rather than `Debug`'s capitalized variant names.
pub(super) fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Off => "off",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

/// Finding counts by severity, used to render report summaries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SeverityCounts {
    pub(super) errors: usize,
    pub(super) warnings: usize,
    pub(super) off: usize,
}

impl SeverityCounts {
    /// Tallies `findings` by severity in a single pass.
    pub(super) fn count(findings: &[Finding]) -> Self {
        let mut counts = Self::default();
        for finding in findings {
            match finding.severity {
                Severity::Error => counts.errors += 1,
                Severity::Warning => counts.warnings += 1,
                Severity::Off => counts.off += 1,
            }
        }
        counts
    }

    /// Total findings across all severities.
    pub(super) fn total(&self) -> usize {
        self.errors + self.warnings + self.off
    }
}

/// The run-summary counters every JSON-carrying reporter repeats verbatim: what was analyzed,
/// what stayed unresolved, and what `--baseline`/`--changed-since` did.
///
/// Both the JSON document and the `jsonl` summary line carry exactly these fields, flattened in
/// place, so the two formats cannot disagree about a counter the way their two hand-maintained
/// field lists could.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SummaryCounters {
    tests_analyzed: usize,
    files_analyzed: usize,
    unresolved_tests: usize,
    /// How many files had *every* executable test excluded, so they produced no findings and
    /// appear nowhere else in the report. A count, not a list: the names are analyzed-repository
    /// text, and the terminal reporter is where they are surfaced (sanitized) for a human. A gate
    /// acts on the number.
    blind_files: usize,
    incomplete: bool,
    /// Which causes made `incomplete` true, and how much each accounts for.
    ///
    /// `incomplete` on its own tells a consumer that absence of a finding proves nothing but not
    /// what to do about it; these counters separate "the analyzer was undersized for this corpus"
    /// from "twelve files failed to read", which call for opposite responses. See
    /// [`IncompleteCauses`].
    incomplete_causes: IncompleteCauses,
    /// What `--baseline` and `--changed-since` did to this run. Serialized from
    /// [`ModeSummary`] directly: it is a closed, tool-owned struct of counters and booleans, with
    /// no analyzed-repository text in it, so it needs no sanitization.
    modes: ModeSummary,
}

impl SummaryCounters {
    /// Copy analysis counts, incompleteness causes and mode counters into the shared report
    /// summary.
    pub(super) fn from_result(result: &AnalysisResult) -> Self {
        Self {
            tests_analyzed: result.tests_analyzed,
            files_analyzed: result.files_analyzed,
            unresolved_tests: result.unresolved_tests,
            blind_files: result.blind_files.len(),
            incomplete: result.incomplete(),
            incomplete_causes: result.incomplete_causes.clone(),
            modes: result.modes,
        }
    }
}

/// One participating location in a JSON-carrying report, shared by the JSON document and the
/// `jsonl` finding record so the two formats never disagree about what a location contains.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct JsonLocation {
    path: String,
    line: u32,
    column: u32,
    end_line: u32,
    end_column: u32,
}

impl From<&Location> for JsonLocation {
    /// Convert a source location to report fields, sanitizing its path while preserving
    /// line/column endpoints.
    fn from(location: &Location) -> Self {
        Self {
            // `report_safe_path` normalizes separators *then* strips control/ANSI/bidi/invisible
            // characters -- see its doc comment for why a path gets the same treatment as a title
            // here rather than being emitted raw.
            path: report_safe_path(&location.path),
            line: location.span.start.line,
            column: location.span.start.column,
            end_line: location.span.end.line,
            end_column: location.span.end.column,
        }
    }
}

/// Groups findings by their first participant's file, in path order -- the layout both the
/// terminal and HTML reporters render under each rule heading.
///
/// A finding is documented to always carry at least two locations; the `unwrap_or_default`
/// fallback only guards a hand-built value (e.g. in a test) that violates that invariant.
pub(super) fn findings_by_file<'a>(
    findings: &[&'a Finding],
) -> std::collections::BTreeMap<std::path::PathBuf, Vec<&'a Finding>> {
    let mut by_file: std::collections::BTreeMap<std::path::PathBuf, Vec<&'a Finding>> =
        std::collections::BTreeMap::new();
    for finding in findings {
        let file = finding
            .locations
            .first()
            .map(|location| location.path.clone())
            .unwrap_or_default();
        by_file.entry(file).or_default().push(finding);
    }
    by_file
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check shared report fields: terminal safe strips newlines carriage returns tabs and
    /// escapes.
    #[test]
    fn terminal_safe_strips_newlines_carriage_returns_tabs_and_escapes() {
        let input = "line one\nline two\r\x1b[31mred\x1b[0m\ttabbed";
        let sanitized = terminal_safe(input);
        assert!(!sanitized.contains('\n'));
        assert!(!sanitized.contains('\r'));
        assert!(!sanitized.contains('\t'));
        assert!(!sanitized.contains('\u{1b}'));
        // The bracket/letter text after ESC is inert without the ESC byte, so it may remain.
        assert_eq!(sanitized, "line oneline two[31mred[0mtabbed");
    }

    /// Check shared report fields: report safe keeps intentional whitespace but strips escapes
    /// and invisibles.
    #[test]
    fn report_safe_keeps_intentional_whitespace_but_strips_escapes_and_invisibles() {
        let input = "line one\nline two\tend\x1b[31mred\x1b[0m\u{200b}hidden";
        let sanitized = report_safe(input);
        assert!(sanitized.contains('\n'));
        assert!(sanitized.contains('\t'));
        assert!(!sanitized.contains('\u{1b}'));
        assert!(!sanitized.contains('\u{200b}'));
    }

    /// Check shared report fields: forward slash path normalizes backslashes on every platform.
    #[test]
    fn forward_slash_path_normalizes_backslashes_on_every_platform() {
        // `Path` treats `\` as an ordinary byte on non-Windows hosts, so this exercises the
        // string-level replacement rather than relying on the host's own separator handling —
        // the whole point is that the JSON schema must not vary by build platform.
        let path = Path::new("src\\module\\file.test.ts");
        assert_eq!(forward_slash_path(path), "src/module/file.test.ts");
    }

    /// Check shared report fields: forward slash path is a no op for already forward slashed
    /// paths.
    #[test]
    fn forward_slash_path_is_a_no_op_for_already_forward_slashed_paths() {
        let path = Path::new("src/module/file.test.ts");
        assert_eq!(forward_slash_path(path), "src/module/file.test.ts");
    }

    /// Check shared report fields: report safe path normalizes separators and strips bidi zero
    /// width and control characters.
    #[test]
    fn report_safe_path_normalizes_separators_and_strips_bidi_zero_width_and_control_characters() {
        let bidi = Path::new("src\\safe\u{202e}gnisaeler-tset\u{202c}evil.test.ts");
        let sanitized = report_safe_path(bidi);
        assert!(!sanitized.contains('\u{202e}'));
        assert!(!sanitized.contains('\u{202c}'));
        assert!(!sanitized.contains('\\'));

        let zero_width = Path::new("src\\a\u{200b}.test.ts");
        assert!(!report_safe_path(zero_width).contains('\u{200b}'));

        let control = Path::new("src\\a\u{7}.test.ts");
        assert!(!report_safe_path(control).contains('\u{7}'));
    }

    /// Check shared report fields: report safe strips deprecated bidi and shaping controls
    /// u206a through u206f.
    #[test]
    fn report_safe_strips_deprecated_bidi_and_shaping_controls_u206a_through_u206f() {
        // U+206A..=U+206F (inhibit/activate symmetric swapping, inhibit/activate Arabic form
        // shaping, national digit shapes, nominal digit shapes) are deprecated but still live
        // formatting controls. None of them are `char::is_control()`, so a sanitizer range that
        // stopped at U+2069 (the last bidi isolate) let every one of these six codepoints through
        // unchanged. Each must be stripped individually, not just as a lucky side effect of a
        // wider range that happens to include them.
        for codepoint in 0x206a_u32..=0x206f {
            let character = char::from_u32(codepoint).expect("206a..=206f are valid scalars");
            let input = format!("src/report{character}title.test.ts");
            let sanitized = report_safe(&input);
            assert!(
                !sanitized.contains(character),
                "U+{codepoint:04X} was not stripped by report_safe"
            );
        }
    }

    /// Check shared report fields: report safe path strips deprecated bidi and shaping controls
    /// u206a through u206f.
    #[test]
    fn report_safe_path_strips_deprecated_bidi_and_shaping_controls_u206a_through_u206f() {
        for codepoint in 0x206a_u32..=0x206f {
            let character = char::from_u32(codepoint).expect("206a..=206f are valid scalars");
            let raw = format!("src/a{character}.test.ts");
            let path = Path::new(&raw);
            let sanitized = report_safe_path(path);
            assert!(
                !sanitized.contains(character),
                "U+{codepoint:04X} was not stripped by report_safe_path"
            );
        }
    }

    /// Check shared report fields: report safe path preserves legitimate unicode filenames.
    #[test]
    fn report_safe_path_preserves_legitimate_unicode_filenames() {
        // The whole point of stripping only control/ANSI/bidi/invisible characters rather than
        // every non-ASCII codepoint: real filenames in CJK scripts or accented Latin must survive
        // untouched.
        let path = Path::new("src/tëst-ünïcödé/日本語のタイトル.test.ts");
        assert_eq!(
            report_safe_path(path),
            "src/tëst-ünïcödé/日本語のタイトル.test.ts"
        );
    }

    /// Check shared report fields: severity label is lowercase and stable.
    #[test]
    fn severity_label_is_lowercase_and_stable() {
        assert_eq!(severity_label(Severity::Error), "error");
        assert_eq!(severity_label(Severity::Warning), "warning");
        assert_eq!(severity_label(Severity::Off), "off");
    }
}
