//! One adversarial matrix, crossed over every output surface.
//!
//! The blind spot this file closes is adversarial input applied *unevenly*: hostile input was
//! tested thoroughly in the HTML reporter, partially in JSON and JSONL, and not at all on stderr
//! diagnostics or the stdout path echo -- because each output path was tested in isolation, by
//! whoever built it, so the same bidi override the HTML reporter carefully strips could be printed
//! raw to a terminal one function away. **The property under test is uniformity**, which a
//! per-surface test cannot express; this file expresses it as one table of hostile values
//! crossed with every injection site and every output surface, driven through the real
//! `vitest-dedup` binary end to end (never the library reporters directly), matching how
//! `docs/reports/safety.md`'s output-safety contract is actually consumed.
//!
//! # Dimensions
//!
//! - **Hazards** ([`HAZARDS`]): seventeen values -- fifteen hostile, two legitimate-Unicode
//!   controls, one of them a set of scalars one step outside each stripped span. Each
//!   carries a [`Category`] describing what the output-safety contract in `docs/reports/safety.md`
//!   promises for it, which is what turns "does it survive" into a falsifiable expectation instead
//!   of a guess per cell.
//! - **Injection sites** ([`Site`]): a test title, a `describe` title, and a filename -- the three
//!   places repository-controlled text enters the pipeline.
//! - **Output surfaces** ([`Surface`]): terminal stdout, JSONL stdout, the JSON report file, the
//!   HTML report file, the SARIF report file, and stderr diagnostics.
//!
//! # How this fails closed on a new surface
//!
//! Two functions match over real, exhaustive enums with **no wildcard arm**:
//!
//! - [`surface_for_reporter`] matches over [`vitest_dedup::cli::ReporterKind`] itself -- the actual
//!   production enum, not a copy of it. If a maintainer adds a new reporter anywhere in
//!   `src/cli.rs`, this match stops being exhaustive and **the crate fails to compile** until
//!   someone decides which [`Surface`] that reporter is and teaches this file about it -- exactly
//!   what happened when `ReporterKind::Sarif` was added and this file gained `Surface::SarifFile`.
//!   A new reporter can therefore never go unlisted in the matrix; the compiler enforces it.
//! - [`expected_survive`] matches over `(Category, Surface)` with every arm spelled out. Adding a
//!   [`Surface`] variant (stderr diagnostics is not backed by a `ReporterKind` and only this match
//!   knows about it) makes every arm of this function non-exhaustive, which is again a compile
//!   error, not a silently-skipped case.
//!
//! Together these two matches are the enforcement mechanism, not a decorative check next to it:
//! there is no separate "list of surfaces" that could drift from what the matrix actually runs,
//! because the thing that decides what each surface *means* is the thing the compiler audits.
//!
//! This is not enforcement over [`Site`] or [`Category`]: those are this file's own vocabulary, not
//! read from the crate, so nothing forces a new hostile-value category or injection site to be
//! added. That is an honest limitation, not an oversight -- the crate has no typed enumeration of
//! "hazard category" or "injection site" for a match to be exhaustive over.
//!
//! # Skipped combinations
//!
//! - **Test/describe title x stderr diagnostics**, for every hazard: skipped explicitly, not
//!   silently. `src/main.rs`'s diagnostics are built exclusively from file paths and counts (a
//!   skip reason, a parse-diagnostic count, an unresolved-test tally) -- grep the crate and no
//!   `eprintln!` ever interpolates a test or `describe` title. Stderr is only reachable through the
//!   filename injection site.
//! - **Filename site for seven hazards, on non-Unix targets**: newline, carriage return, the ANSI
//!   escape, and the four literal lookalikes that contain `<`, `>`, or `:` cannot legally appear in
//!   a filename on Windows. Those `#[test]` functions are `#[cfg(unix)]`-gated below and say so at
//!   the call site, matching the project's own practical guidance rather than letting the suite
//!   fail on a platform where the file could never have been created.

use assert_cmd::Command;
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use vitest_dedup::cli::ReporterKind;

// =================================================================================================
// The matrix's three typed dimensions.
// =================================================================================================

/// What the output-safety contract (`docs/reports/safety.md`) promises for a hazard's raw content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    /// Stripped by every sanitizer, unconditionally: bidirectional overrides, invisible
    /// formatting characters, and other control characters (the ANSI escape byte among them).
    /// Must not appear anywhere, in any surface, decoded or raw -- there is no representation in
    /// which this content is meant to survive.
    AlwaysStripped,
    /// Structural whitespace (`\n`, `\r`): `src/reporters/shared.rs`'s `sanitize` strips it for
    /// the terminal reporter and for stderr (both go through `terminal_safe`, which never keeps
    /// whitespace), because a raw newline there would forge an extra report line. Every
    /// structured/escaped surface (JSON, JSONL, the HTML JSON island) preserves it as *data*
    /// instead, since JSON's own escaping represents it without letting it forge new structure.
    StructuralWhitespace,
    /// Ordinary printable text with no stripping rule at all: the fingerprint-delimiter lookalike,
    /// `</script>`, `<!--`, and the workflow-command lookalike. Every reporter's sanitizer only
    /// strips control/ANSI/bidi/invisible characters, never plain text, so this survives
    /// byte-for-byte everywhere as inert data -- except the HTML file must not let it survive
    /// *unescaped* in the raw page when it contains an HTML-special character, which is checked
    /// separately (see [`assert_html_raw_escaping`]).
    LiteralPassthrough,
    /// Legitimate non-ASCII content (CJK, accented Latin) that must survive unmangled everywhere.
    /// The positive control: a matrix that strips everything would pass every hostile-value test
    /// above while silently failing this one.
    MustSurvive,
}

/// The three places repository-controlled text enters the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Site {
    /// A `test(...)`/`it(...)` title.
    TestTitle,
    /// A `describe(...)` title, joined into `TestCase::full_title` with `" > "`.
    DescribeTitle,
    /// The test file's own filename.
    Filename,
}

/// Every output surface this binary has. See the module doc for how adding a variant here is
/// caught at compile time rather than silently skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    TerminalStdout,
    JsonlStdout,
    JsonFile,
    HtmlFile,
    SarifFile,
    /// Not backed by a [`ReporterKind`] -- diagnostics printed by `src/main.rs` outside any
    /// reporter (a skip reason, a parse-diagnostic tally, an unresolved-context count).
    StderrDiagnostics,
}

/// Maps a real, implemented reporter onto the surface that exercises it.
///
/// Exhaustive over [`ReporterKind`] itself with no wildcard arm -- see the module doc's "How this
/// fails closed" section. This is the load-bearing function for the file's central claim: it does
/// not merely describe today's reporters, it is the reason a new one can never go unacknowledged
/// -- as `ReporterKind::Sarif` could not when it was added.
fn surface_for_reporter(kind: ReporterKind) -> Surface {
    match kind {
        ReporterKind::Terminal => Surface::TerminalStdout,
        ReporterKind::Jsonl => Surface::JsonlStdout,
        ReporterKind::Json => Surface::JsonFile,
        ReporterKind::Html => Surface::HtmlFile,
        ReporterKind::Sarif => Surface::SarifFile,
    }
}

/// Whether a hazard of the given [`Category`] is expected to survive (appear, in some decoded
/// form) on the given [`Surface`].
///
/// Exhaustive over `(Category, Surface)` with no wildcard arm on either axis -- adding a
/// [`Category`] or a [`Surface`] variant makes this stop compiling, which is exactly the second
/// half of the "fails closed" mechanism described in the module doc. `HtmlFile`'s entry here
/// describes only the *decoded* value (read back out of the JSON island); whether the same content
/// is also allowed to appear **unescaped** in the raw page bytes is a separate, stricter question
/// answered by [`assert_html_raw_escaping`], because HTML is the one surface where "present as
/// data" and "safe to render" are not the same thing.
fn expected_survive(category: Category, surface: Surface) -> bool {
    match category {
        Category::AlwaysStripped => match surface {
            Surface::TerminalStdout
            | Surface::StderrDiagnostics
            | Surface::JsonlStdout
            | Surface::JsonFile
            | Surface::HtmlFile
            | Surface::SarifFile => false,
        },
        Category::StructuralWhitespace => match surface {
            Surface::TerminalStdout | Surface::StderrDiagnostics => false,
            Surface::JsonlStdout | Surface::JsonFile | Surface::HtmlFile | Surface::SarifFile => {
                true
            }
        },
        Category::LiteralPassthrough => match surface {
            Surface::TerminalStdout
            | Surface::StderrDiagnostics
            | Surface::JsonlStdout
            | Surface::JsonFile
            | Surface::HtmlFile
            | Surface::SarifFile => true,
        },
        Category::MustSurvive => match surface {
            Surface::TerminalStdout
            | Surface::StderrDiagnostics
            | Surface::JsonlStdout
            | Surface::JsonFile
            | Surface::HtmlFile
            | Surface::SarifFile => true,
        },
    }
}

// =================================================================================================
// The hazard table.
// =================================================================================================

/// One row of the adversarial matrix: a hostile (or, for `legitimate_unicode`, deliberately benign)
/// value plus everything needed to embed it at any [`Site`] and know what to expect back.
struct Hazard {
    /// A short, filename- and JS-identifier-safe label, also used to build a unique marker so a
    /// search for this hazard's content can never accidentally match unrelated report structure
    /// (a bare `"\n"` search would match every other line break in the output; a search for
    /// `"advnewlinebegin\nadvnewlineend"` cannot).
    id: &'static str,
    /// The exact scalar content the analyzer extracts, before any sanitizer runs.
    decoded: &'static str,
    /// How to spell `decoded` inside a double-quoted JS string literal so the generated fixture
    /// source stays syntactically valid. Identical to `decoded` for every hazard whose content is
    /// legal unescaped inside a JS string; a JS escape sequence for the three that are not (a raw
    /// newline, carriage return, or ESC byte cannot appear literally in a single-line JS string).
    js_source_spelling: &'static str,
    category: Category,
    /// Whether `decoded` can legally appear in a filename on every platform this suite runs on,
    /// including Windows. `false` means the filename-site tests for this hazard are
    /// `#[cfg(unix)]`-gated; see the module doc's "Skipped combinations" section.
    filename_legal_everywhere: bool,
}

const HAZARDS: &[Hazard] = &[
    Hazard {
        id: "bidioverride",
        decoded: "\u{202e}",
        js_source_spelling: "\u{202e}",
        category: Category::AlwaysStripped,
        filename_legal_everywhere: true,
    },
    Hazard {
        id: "zerowidthspace",
        decoded: "\u{200b}",
        js_source_spelling: "\u{200b}",
        category: Category::AlwaysStripped,
        filename_legal_everywhere: true,
    },
    Hazard {
        id: "unicodetag",
        decoded: "\u{e0001}",
        js_source_spelling: r"\u{e0001}",
        category: Category::AlwaysStripped,
        filename_legal_everywhere: true,
    },
    Hazard {
        id: "lineseparator",
        decoded: "\u{2028}",
        js_source_spelling: r" ",
        category: Category::AlwaysStripped,
        filename_legal_everywhere: true,
    },
    Hazard {
        id: "paragraphseparator",
        decoded: "\u{2029}",
        js_source_spelling: r" ",
        category: Category::AlwaysStripped,
        filename_legal_everywhere: true,
    },
    Hazard {
        // Arabic number sign, a `Cf` in the Basic Multilingual Plane outside the old ranges.
        id: "arabicnumbersign",
        decoded: "\u{0600}",
        js_source_spelling: r"؀",
        category: Category::AlwaysStripped,
        filename_legal_everywhere: true,
    },
    Hazard {
        id: "mongolianvowelseparator",
        decoded: "\u{180e}",
        js_source_spelling: r"᠎",
        category: Category::AlwaysStripped,
        filename_legal_everywhere: true,
    },
    Hazard {
        // A `Cf` outside the Basic Multilingual Plane and outside the Tags block.
        id: "musicalbeam",
        decoded: "\u{1d173}",
        js_source_spelling: r"\u{1d173}",
        category: Category::AlwaysStripped,
        filename_legal_everywhere: true,
    },
    Hazard {
        id: "newline",
        decoded: "\n",
        js_source_spelling: r"\n",
        category: Category::StructuralWhitespace,
        filename_legal_everywhere: false, // control character, illegal in a Windows filename
    },
    Hazard {
        id: "carriagereturn",
        decoded: "\r",
        js_source_spelling: r"\r",
        category: Category::StructuralWhitespace,
        filename_legal_everywhere: false, // control character, illegal in a Windows filename
    },
    Hazard {
        id: "ansiescape",
        decoded: "\u{1b}[31m",
        js_source_spelling: r"\x1b[31m",
        category: Category::AlwaysStripped,
        filename_legal_everywhere: false, // control character, illegal in a Windows filename
    },
    Hazard {
        id: "scriptclose",
        decoded: "</script>",
        js_source_spelling: "</script>",
        category: Category::LiteralPassthrough,
        filename_legal_everywhere: false, // '<' and '>' are reserved on Windows
    },
    Hazard {
        id: "commentopen",
        decoded: "<!--",
        js_source_spelling: "<!--",
        category: Category::LiteralPassthrough,
        filename_legal_everywhere: false, // '<' is reserved on Windows
    },
    Hazard {
        id: "fingerprintdelimiter",
        // Imitates the fingerprint canonicalization's own delimiter grammar (`<kind:raw>`-shaped).
        // This is here because a fix tested only against the bug it fixed is not tested: the
        // literal-kind encoding closed `true == 'true'` and opened a new collision, where a string
        // containing the delimiter syntax imitates a structure boundary and `['x> <string:y']`
        // fingerprints the same as `['x', 'y']` -- an Error-severity false duplicate created *by*
        // a false-duplicate fix. The value below is what the new encoding made possible, not the
        // bug the fix originally closed.
        decoded: "x> <string:y",
        js_source_spelling: "x> <string:y",
        category: Category::LiteralPassthrough,
        filename_legal_everywhere: false, // '<', '>', and ':' are reserved on Windows
    },
    Hazard {
        id: "workflowcommand",
        decoded: "::error::",
        js_source_spelling: "::error::",
        category: Category::LiteralPassthrough,
        filename_legal_everywhere: false, // ':' is reserved on Windows
    },
    Hazard {
        id: "legitimateunicode",
        decoded: "café-日本語のタイトル",
        js_source_spelling: "café-日本語のタイトル",
        category: Category::MustSurvive,
        filename_legal_everywhere: true,
    },
    Hazard {
        // Each character sits one scalar outside a stripped span, or is a variation selector
        // (`Mn`): U+2027 below the separators, U+202F after the bidi overrides, U+205F before the
        // invisible-operator block, U+FE0F, and U+E0100 after the tags. A stripper that widened
        // by one scalar, or by category, would drop one of them.
        id: "edgeunicode",
        decoded: "a\u{2027}b\u{202f}c\u{205f}d\u{fe0f}e\u{e0100}f",
        js_source_spelling: r"a‧b c d️e\u{e0100}f",
        category: Category::MustSurvive,
        filename_legal_everywhere: true,
    },
];

/// Look up a named hostile-input fixture, panicking when the hazard inventory lacks it.
fn hazard(id: &str) -> &'static Hazard {
    HAZARDS
        .iter()
        .find(|hazard| hazard.id == id)
        .unwrap_or_else(|| panic!("no hazard named {id:?} in HAZARDS"))
}

/// A marker-wrapped needle unique to `hazard`, so a raw-text search can never accidentally match
/// unrelated content the reporters legitimately emit (the page's own `</script>` closing tags, a
/// pretty-printer's own newlines, the terminal report's own line structure).
fn needle_text(hazard: &Hazard) -> String {
    format!("adv{}begin{}adv{}end", hazard.id, hazard.decoded, hazard.id)
}

/// Wrap a hazard's JavaScript source spelling in recognizable title markers.
fn title_source_text(hazard: &Hazard) -> String {
    format!(
        "adv{}begin{}adv{}end",
        hazard.id, hazard.js_source_spelling, hazard.id
    )
}

// =================================================================================================
// Fixture construction.
//
// Every fixture is exactly two tests sharing one full title with deliberately different bodies
// (different matcher, different expression shape) so that `duplicate-test-name` fires alone --
// `duplicate-test` needs identical bodies, and `parameterization-candidate` needs identical
// *structure* differing only in a literal leaf, and these bodies satisfy neither. That keeps every
// surface's output down to exactly one finding, which is what makes the JSONL line-count check
// (`assert_jsonl_field`) a hardcoded expectation rather than a fragile guess.
// =================================================================================================

const BODY_A: &str = "expect(1).toBe(1)";
const BODY_B: &str =
    "const values = [1, 2, 3]\n    expect(values.filter((value) => value > 1)).toEqual([2, 3])";

/// Builds a project with the hazard embedded in a test title (fixed filename, fixed describe-free
/// structure).
fn build_test_title_project(hazard: &Hazard) -> (TempDir, String) {
    let dir = tempfile::tempdir().expect("create tempdir");
    let title = title_source_text(hazard);
    let source = format!(
        "import {{ test, expect }} from 'vitest'\n\n\
         test(\"{title}\", () => {{\n  {BODY_A}\n}})\n\n\
         test(\"{title}\", () => {{\n  {BODY_B}\n}})\n"
    );
    fs::write(dir.path().join("case.test.ts"), source).expect("write fixture");
    (dir, needle_text(hazard))
}

/// Builds a project with the hazard embedded in a `describe` title wrapping a plain inner test
/// title, so `TestCase::full_title`'s `" > "` join is what carries the hazard into the report.
fn build_describe_title_project(hazard: &Hazard) -> (TempDir, String) {
    let dir = tempfile::tempdir().expect("create tempdir");
    let title = title_source_text(hazard);
    let source = format!(
        "import {{ describe, test, expect }} from 'vitest'\n\n\
         describe(\"{title}\", () => {{\n  test(\"case\", () => {{\n    {BODY_A}\n  }})\n}})\n\n\
         describe(\"{title}\", () => {{\n  test(\"case\", () => {{\n    {BODY_B}\n  }})\n}})\n"
    );
    fs::write(dir.path().join("case.test.ts"), source).expect("write fixture");
    (dir, needle_text(hazard))
}

/// Builds a project whose *filename* carries the hazard, with ordinary content that produces one
/// `duplicate-test-name` finding referencing that path.
fn build_filename_project(hazard: &Hazard) -> (TempDir, String) {
    let dir = tempfile::tempdir().expect("create tempdir");
    let needle = needle_text(hazard);
    let filename = format!("{needle}.test.ts");
    let source = format!(
        "import {{ test, expect }} from 'vitest'\n\n\
         test(\"case\", () => {{\n  {BODY_A}\n}})\n\n\
         test(\"case\", () => {{\n  {BODY_B}\n}})\n"
    );
    fs::write(dir.path().join(&filename), source).expect("write fixture");
    (dir, needle)
}

/// Builds a project whose filename carries the hazard and whose *content* is deliberately invalid
/// UTF-8, so `src/main.rs`'s `read_bounded` fails and the filename reaches stderr through the
/// "skipping {path}: {error}" diagnostic -- the only path by which a filename (never a title)
/// reaches stderr at all.
fn build_filename_stderr_project(hazard: &Hazard) -> (TempDir, String) {
    let dir = tempfile::tempdir().expect("create tempdir");
    let needle = needle_text(hazard);
    let filename = format!("{needle}.test.ts");
    let mut bytes = b"// not valid utf-8 below: ".to_vec();
    bytes.push(0xff); // a lone continuation byte is never valid UTF-8 at any position
    fs::write(dir.path().join(&filename), bytes).expect("write fixture");
    (dir, needle)
}

// =================================================================================================
// Driving the real binary and extracting the field each site populates.
// =================================================================================================

struct NonStderrOutputs {
    terminal_stdout: String,
    jsonl_stdout: String,
    json_file: String,
    html_file: String,
    sarif_file: String,
}

/// Runs the five non-stderr reporters against `dir` via three invocations of the real binary
/// (`terminal` and `jsonl` cannot be combined; `json,html,sarif` share one `--output` run), reading
/// file reporters back off disk exactly as a real consumer would.
fn run_non_stderr(dir: &Path) -> NonStderrOutputs {
    let terminal = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(dir)
        .args(["--reporters", "terminal"])
        .output()
        .expect("run vitest-dedup (terminal)");
    let jsonl = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(dir)
        .args(["--reporters", "jsonl"])
        .output()
        .expect("run vitest-dedup (jsonl)");

    let prefix = dir.join("out").join("rep");
    Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(dir)
        .args(["--reporters", "json,html,sarif", "--output"])
        .arg(&prefix)
        .output()
        .expect("run vitest-dedup (json,html,sarif)");

    NonStderrOutputs {
        terminal_stdout: String::from_utf8(terminal.stdout).expect("terminal stdout is UTF-8"),
        jsonl_stdout: String::from_utf8(jsonl.stdout).expect("jsonl stdout is UTF-8"),
        json_file: fs::read_to_string(prefix.with_extension("json")).expect("read json report"),
        html_file: fs::read_to_string(prefix.with_extension("html")).expect("read html report"),
        sarif_file: fs::read_to_string(prefix.with_extension("sarif")).expect("read sarif report"),
    }
}

/// Run the CLI with the terminal reporter and return decoded stderr for sanitization checks.
fn run_stderr_only(dir: &Path) -> String {
    let output = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(dir)
        .args(["--reporters", "terminal"])
        .output()
        .expect("run vitest-dedup");
    String::from_utf8(output.stderr).expect("stderr is UTF-8")
}

/// Extracts the field a `Site` populates (the title, or the participant path) out of the single
/// finding in a parsed JSON report document (`{"findings": [...]}` shape: the JSON file and the
/// HTML JSON island both use it).
fn extract_field(report: &serde_json::Value, site: Site) -> String {
    let finding = &report["findings"][0];
    let field = if site == Site::Filename {
        &finding["locations"][0]["path"]
    } else {
        &finding["titles"][0]
    };
    field
        .as_str()
        .unwrap_or_else(|| panic!("expected a string field in finding: {finding}"))
        .to_owned()
}

/// Same extraction for a JSONL finding record, whose fields sit at the top level rather than under
/// `findings[0]`.
fn extract_field_flat(record: &serde_json::Value, site: Site) -> String {
    let field = if site == Site::Filename {
        &record["locations"][0]["path"]
    } else {
        &record["titles"][0]
    };
    field
        .as_str()
        .unwrap_or_else(|| panic!("expected a string field in record: {record}"))
        .to_owned()
}

/// Extracts the field a `Site` populates from the first `results[]` entry of a parsed SARIF
/// document. Unlike the JSON/JSONL schemas, SARIF has no top-level `titles` array parallel to
/// `locations` -- `src/reporters/sarif.rs` instead attaches each participant's title as that
/// location's own `message.text` (see that module's "Per-participant title and inert flag" doc
/// section), so the title-site extraction reads the primary location's message rather than a
/// sibling array.
fn extract_field_sarif(report: &serde_json::Value, site: Site) -> String {
    let result = &report["runs"][0]["results"][0];
    let field = if site == Site::Filename {
        &result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
    } else {
        &result["locations"][0]["message"]["text"]
    };
    field
        .as_str()
        .unwrap_or_else(|| panic!("expected a string field in sarif result: {result}"))
        .to_owned()
}

/// Parses `jsonl_stdout` as exactly one finding line followed by exactly one summary line --
/// hardcoded, not a fragile guess, because every fixture in this file is built (see the "Fixture
/// construction" section) to produce exactly one `duplicate-test-name` finding. A third line here
/// would mean either the fixture produced more findings than intended, or -- the case this file
/// exists to catch -- a raw newline inside hazard content forged an extra record boundary instead
/// of being escaped as data.
fn parse_single_finding_jsonl(jsonl_stdout: &str) -> serde_json::Value {
    // `str::lines` splits on LF only, but ECMAScript and several log tools also split on these two,
    // and `serde_json` writes them unescaped: none may reach the stream.
    assert!(
        !jsonl_stdout.contains(['\u{2028}', '\u{2029}']),
        "a Unicode line separator reached the jsonl stream: {jsonl_stdout:?}"
    );
    let mut lines = jsonl_stdout.lines();
    let finding_line = lines
        .next()
        .unwrap_or_else(|| panic!("expected a finding line in jsonl output, got none"));
    let record: serde_json::Value =
        serde_json::from_str(finding_line).expect("finding line is valid standalone JSON");
    assert_eq!(record["type"], "finding");
    let summary_line = lines
        .next()
        .unwrap_or_else(|| panic!("expected a summary line after the finding line"));
    serde_json::from_str::<serde_json::Value>(summary_line)
        .expect("summary line is valid standalone JSON");
    assert!(
        lines.next().is_none(),
        "expected exactly one finding line and one summary line, but jsonl output had more: {jsonl_stdout:?}"
    );
    record
}

/// Extracts the raw text of the `report-data` JSON island from a rendered HTML page, mirroring
/// `src/reporters/html.rs`'s own tests. Isolating this region matters: the page's *own* static
/// markup legitimately contains `</script>` (its real closing tags) and other boilerplate that
/// would make a whole-file substring search meaningless.
fn html_island_text(html: &str) -> &str {
    let start_marker = r#"<script type="application/json" id="report-data">"#;
    let start = html
        .find(start_marker)
        .unwrap_or_else(|| panic!("could not find the report-data script tag in HTML output"))
        + start_marker.len();
    let end = start
        + html[start..]
            .find("</script>")
            .unwrap_or_else(|| panic!("report-data script tag was never closed"));
    &html[start..end]
}

// =================================================================================================
// Per-surface assertions.
// =================================================================================================

/// The core check every cell reduces to: does `needle` appear in `haystack`, and does that match
/// [`expected_survive`] for this hazard's category and surface?
fn assert_cell(hazard: &Hazard, site: Site, surface: Surface, haystack: &str, needle: &str) {
    let expected = expected_survive(hazard.category, surface);
    let found = haystack.contains(needle);
    assert_eq!(
        found, expected,
        "hazard={:?} site={site:?} surface={surface:?}: expected survive={expected} but \
         found={found} (needle={needle:?})",
        hazard.id
    );
}

/// The HTML file's extra, stricter question: is `needle` present **unescaped**, in the raw page
/// bytes, in a form that could actually terminate a `<script>` element or open markup? This is
/// independent of whether the content survives as *data* (checked separately via the JSON island
/// through the ordinary `assert_cell` path), because HTML is the one surface where those two
/// questions have different answers: `report_safe` and `json_for_html` both preserve the character
/// data but re-spell any of `< > & " '` so the raw bytes never carry them unescaped.
fn assert_html_raw_escaping(hazard: &Hazard, needle: &str, html_raw: &str) {
    let needs_escaping = needle
        .chars()
        .any(|character| matches!(character, '<' | '>' | '&' | '"' | '\''));
    let decoded_survives = expected_survive(hazard.category, Surface::HtmlFile);
    let expected_raw_survive = decoded_survives && !needs_escaping;
    let raw_survives = html_raw.contains(needle);
    assert_eq!(
        raw_survives, expected_raw_survive,
        "hazard={:?} html raw-bytes check: expected raw survive={expected_raw_survive} but \
         found={raw_survives} (needle={needle:?}); a hazard containing '<', '>', '&', '\"', or \
         '\\'' must never appear unescaped in the raw HTML file even when it is allowed to \
         survive as data in the JSON island",
        hazard.id
    );
}

/// Runs every non-stderr surface for one (hazard, site) cell and asserts all four.
fn check_non_stderr_cells(hazard: &Hazard, site: Site, dir: &Path, needle: &str) {
    let outputs = run_non_stderr(dir);

    assert_cell(
        hazard,
        site,
        Surface::TerminalStdout,
        &outputs.terminal_stdout,
        needle,
    );

    let jsonl_record = parse_single_finding_jsonl(&outputs.jsonl_stdout);
    assert_cell(
        hazard,
        site,
        Surface::JsonlStdout,
        &extract_field_flat(&jsonl_record, site),
        needle,
    );

    let json_report: serde_json::Value =
        serde_json::from_str(&outputs.json_file).expect("json report is valid JSON");
    assert_cell(
        hazard,
        site,
        Surface::JsonFile,
        &extract_field(&json_report, site),
        needle,
    );

    let island = html_island_text(&outputs.html_file);
    let html_report: serde_json::Value =
        serde_json::from_str(island).expect("HTML report-data island is valid JSON");
    assert_cell(
        hazard,
        site,
        Surface::HtmlFile,
        &extract_field(&html_report, site),
        needle,
    );
    assert_html_raw_escaping(hazard, needle, &outputs.html_file);

    let sarif_report: serde_json::Value =
        serde_json::from_str(&outputs.sarif_file).expect("sarif report is valid JSON");
    assert_cell(
        hazard,
        site,
        Surface::SarifFile,
        &extract_field_sarif(&sarif_report, site),
        needle,
    );
}

/// Build the selected hostile-input site and check its non-stderr reporting surfaces.
fn run_cell(hazard: &Hazard, site: Site) {
    let (dir, needle) = match site {
        Site::TestTitle => build_test_title_project(hazard),
        Site::DescribeTitle => build_describe_title_project(hazard),
        Site::Filename => build_filename_project(hazard),
    };
    check_non_stderr_cells(hazard, site, dir.path(), &needle);
}

/// The filename-site stderr cell: see the module doc's "Skipped combinations" section for why
/// `TestTitle` and `DescribeTitle` have no stderr counterpart.
fn run_filename_stderr_cell(hazard: &Hazard) {
    let (dir, needle) = build_filename_stderr_project(hazard);
    let stderr = run_stderr_only(dir.path());
    assert_cell(
        hazard,
        Site::Filename,
        Surface::StderrDiagnostics,
        &stderr,
        &needle,
    );
}

// =================================================================================================
// A meta-test tying `surface_for_reporter` to the reporters this build actually advertises, so the
// "fails closed" claim in the module doc is itself exercised rather than only asserted in prose.
// =================================================================================================

/// Cross-checks every hazard's `filename_legal_everywhere` flag against its own `decoded` content,
/// so the flag (which drives whether this file even attempts to write a hostile filename) cannot
/// silently drift from reality the way `docs/reports/safety.md`'s output-safety contract is not allowed
/// to drift from the reporters' actual behavior. Reserved characters cover both Windows'
/// restrictions (`< > : " \ | ? *`) and the one restriction every platform shares: `/` as a path
/// separator, which is what made `</script>` unusable as a filename fragment at all (see the
/// comment on the `scriptclose` hazard below) rather than merely Windows-unsafe.
#[test]
fn filename_unsafe_hazards_are_consistently_flagged() {
    for hazard in HAZARDS {
        let has_reserved_character = hazard.decoded.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '/' | '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*'
                )
        });
        assert_eq!(
            hazard.filename_legal_everywhere,
            !has_reserved_character,
            "hazard {:?}: filename_legal_everywhere={} but decoded={:?} {} a reserved or control \
             character -- the flag and the content disagree",
            hazard.id,
            hazard.filename_legal_everywhere,
            hazard.decoded,
            if has_reserved_character {
                "contains"
            } else {
                "does not contain"
            }
        );
    }
}

#[test]
fn every_reporter_kind_maps_to_a_surface() {
    // `ReporterKind` is not iterable (no `EnumIter` in this crate's dependency set, and this file
    // owns no `Cargo.toml` change to add one), so this list is this test's own enumeration, kept
    // deliberately small and separate from the enforcement mechanism itself: even if this list
    // were left stale, `surface_for_reporter`'s match staying exhaustive over the *real*
    // `ReporterKind` is what actually catches a new variant, at compile time, independent of
    // whether anyone remembered to extend this list too.
    for kind in [
        ReporterKind::Terminal,
        ReporterKind::Jsonl,
        ReporterKind::Json,
        ReporterKind::Html,
        ReporterKind::Sarif,
    ] {
        let _ = surface_for_reporter(kind);
    }
}

// =================================================================================================
// The matrix, one #[test] per (hazard, site) cell group so `cargo test`'s pass count is a real,
// legible measure of coverage rather than one opaque function hiding forty-odd assertions.
// =================================================================================================

macro_rules! title_site_tests {
    ($hazard_id:expr, $title_fn:ident, $describe_fn:ident) => {
        #[test]
        fn $title_fn() {
            run_cell(hazard($hazard_id), Site::TestTitle);
        }

        #[test]
        fn $describe_fn() {
            run_cell(hazard($hazard_id), Site::DescribeTitle);
        }
    };
}

macro_rules! filename_site_tests {
    ($hazard_id:expr, $filename_fn:ident, $stderr_fn:ident) => {
        #[test]
        fn $filename_fn() {
            run_cell(hazard($hazard_id), Site::Filename);
        }

        #[test]
        fn $stderr_fn() {
            run_filename_stderr_cell(hazard($hazard_id));
        }
    };
}

// --- bidi override --------------------------------------------------------------------------
title_site_tests!(
    "bidioverride",
    bidi_override_in_test_title,
    bidi_override_in_describe_title
);
filename_site_tests!(
    "bidioverride",
    bidi_override_in_filename,
    bidi_override_filename_reaches_stderr
);

// --- zero-width space ------------------------------------------------------------------------
title_site_tests!(
    "zerowidthspace",
    zero_width_space_in_test_title,
    zero_width_space_in_describe_title
);
filename_site_tests!(
    "zerowidthspace",
    zero_width_space_in_filename,
    zero_width_space_filename_reaches_stderr
);

// --- Unicode tag character (U+E0001) ---------------------------------------------------------
title_site_tests!(
    "unicodetag",
    unicode_tag_in_test_title,
    unicode_tag_in_describe_title
);
filename_site_tests!(
    "unicodetag",
    unicode_tag_in_filename,
    unicode_tag_filename_reaches_stderr
);

// --- line and paragraph separators, other format characters --------------------------------------
title_site_tests!(
    "lineseparator",
    line_separator_in_test_title,
    line_separator_in_describe_title
);
filename_site_tests!(
    "lineseparator",
    line_separator_in_filename,
    line_separator_filename_reaches_stderr
);
title_site_tests!(
    "paragraphseparator",
    paragraph_separator_in_test_title,
    paragraph_separator_in_describe_title
);
filename_site_tests!(
    "paragraphseparator",
    paragraph_separator_in_filename,
    paragraph_separator_filename_reaches_stderr
);
title_site_tests!(
    "arabicnumbersign",
    arabic_number_sign_in_test_title,
    arabic_number_sign_in_describe_title
);
filename_site_tests!(
    "arabicnumbersign",
    arabic_number_sign_in_filename,
    arabic_number_sign_filename_reaches_stderr
);
title_site_tests!(
    "mongolianvowelseparator",
    mongolian_vowel_separator_in_test_title,
    mongolian_vowel_separator_in_describe_title
);
filename_site_tests!(
    "mongolianvowelseparator",
    mongolian_vowel_separator_in_filename,
    mongolian_vowel_separator_filename_reaches_stderr
);
title_site_tests!(
    "musicalbeam",
    musical_beam_in_test_title,
    musical_beam_in_describe_title
);
filename_site_tests!(
    "musicalbeam",
    musical_beam_in_filename,
    musical_beam_filename_reaches_stderr
);
title_site_tests!(
    "edgeunicode",
    edge_unicode_survives_in_test_title,
    edge_unicode_survives_in_describe_title
);
filename_site_tests!(
    "edgeunicode",
    edge_unicode_survives_in_filename,
    edge_unicode_survives_in_stderr
);

// --- newline ------------------------------------------------------------------------------------
title_site_tests!("newline", newline_in_test_title, newline_in_describe_title);
// Filename site: a raw newline byte is creatable in a filename on macOS/Linux but not Windows
// (the practical constraint the task description names explicitly), so these are Unix-only.
#[cfg(unix)]
filename_site_tests!(
    "newline",
    newline_in_filename,
    newline_filename_reaches_stderr
);

// --- carriage return ------------------------------------------------------------------------------------
title_site_tests!(
    "carriagereturn",
    carriage_return_in_test_title,
    carriage_return_in_describe_title
);
#[cfg(unix)]
filename_site_tests!(
    "carriagereturn",
    carriage_return_in_filename,
    carriage_return_filename_reaches_stderr
);

// --- ANSI escape --------------------------------------------------------------------------------
title_site_tests!(
    "ansiescape",
    ansi_escape_in_test_title,
    ansi_escape_in_describe_title
);
// Filename site: the raw ESC byte is a control character, creatable in a filename on macOS/Linux
// but not Windows -- same constraint as newline/carriage-return above.
#[cfg(unix)]
filename_site_tests!(
    "ansiescape",
    ansi_escape_in_filename,
    ansi_escape_filename_reaches_stderr
);

// --- </script> lookalike --------------------------------------------------------------------------
title_site_tests!(
    "scriptclose",
    script_close_in_test_title,
    script_close_in_describe_title
);
// Filename site: skipped on *every* platform, not just Windows. `</script>` contains a literal
// '/', which `Path::join` treats as a path separator on Unix -- `dir.join("adv...</script>...")`
// would create a nested, mostly-nonexistent directory instead of a single hostile filename. This
// is exactly the "a path separator in a filename" impossible combination the task description
// calls out by name; '<' and '>' would additionally make it illegal on Windows even if the slash
// were not a problem.

// --- <!-- lookalike -----------------------------------------------------------------------------
title_site_tests!(
    "commentopen",
    comment_open_in_test_title,
    comment_open_in_describe_title
);
// Filename site: '<' is reserved on Windows.
#[cfg(unix)]
filename_site_tests!(
    "commentopen",
    comment_open_in_filename,
    comment_open_filename_reaches_stderr
);

// --- fingerprint-delimiter-grammar lookalike -----------------------------------------------------
title_site_tests!(
    "fingerprintdelimiter",
    fingerprint_delimiter_in_test_title,
    fingerprint_delimiter_in_describe_title
);
// Filename site: '<', '>', and ':' are all reserved on Windows.
#[cfg(unix)]
filename_site_tests!(
    "fingerprintdelimiter",
    fingerprint_delimiter_in_filename,
    fingerprint_delimiter_filename_reaches_stderr
);

// --- workflow-command lookalike -------------------------------------------------------------------
title_site_tests!(
    "workflowcommand",
    workflow_command_in_test_title,
    workflow_command_in_describe_title
);
// Filename site: ':' is reserved on Windows (drive letters, alternate data streams).
#[cfg(unix)]
filename_site_tests!(
    "workflowcommand",
    workflow_command_in_filename,
    workflow_command_filename_reaches_stderr
);

// --- legitimate Unicode: the positive control ------------------------------------------------
title_site_tests!(
    "legitimateunicode",
    legitimate_unicode_survives_in_test_title,
    legitimate_unicode_survives_in_describe_title
);
filename_site_tests!(
    "legitimateunicode",
    legitimate_unicode_survives_in_filename,
    legitimate_unicode_survives_in_stderr
);

// ---------------------------------------------------------------------------
// The invariant that makes workflow-command passthrough safe
// ---------------------------------------------------------------------------

/// Untrusted content must never begin a physical line of stdout or stderr.
///
/// This matrix showed that `::error::` and similar workflow-command syntax pass through the
/// terminal and stderr surfaces unmodified. That is not currently exploitable, because GitHub
/// Actions only parses a workflow command when `::` starts a line, and every site that
/// interpolates a title or a path is preceded by fixed structural text.
///
/// But that safety was an accident of formatting, not a property anyone had stated or checked. A
/// reporter reflowing its lines, or a consumer stripping the indent (`jq -r`, a log-processing
/// step), would silently remove it.
///
/// This test makes it an invariant. It does not assert that `::error::` is stripped — the
/// output-safety contract in `docs/reports/safety.md` deliberately does not promise that, since the text
/// is legitimate content in a test title. It asserts the weaker, sufficient, and actually-relied-on
/// property: whatever the content, it never lands at column zero.
#[test]
fn untrusted_content_never_begins_a_line_of_terminal_or_stderr_output() {
    let dir = tempfile::tempdir().unwrap();
    // A title that is itself a workflow command, with no leading whitespace of its own.
    fs::write(
        dir.path().join("hostile.test.ts"),
        "import { test, expect } from 'vitest'\n\
         import { f } from './f'\n\
         test('::error::forged', () => { expect(f(1)).toBe(1) })\n\
         test('::error::forged again', () => { expect(f(1)).toBe(1) })\n",
    )
    .unwrap();

    let output = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(dir.path())
        .output()
        .unwrap();

    for (stream, bytes) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
        let text = String::from_utf8_lossy(bytes);
        for (index, line) in text.lines().enumerate() {
            assert!(
                !line.starts_with("::"),
                "{stream} line {index} begins with `::`, which GitHub Actions parses as a \
                 workflow command: {line:?}. Untrusted content must never reach column zero; if a \
                 report format changed, this invariant needs restoring or the output-safety \
                 contract in docs/reports/safety.md needs to promise stripping instead."
            );
        }
    }
}
