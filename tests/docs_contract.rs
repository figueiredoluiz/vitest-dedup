//! Documentation-contract tests.
//!
//! The blind spot this file closes: two review rounds found documentation stating things the code
//! did not do (`--fail-on-incomplete` documented as exit 2 while returning 1; a reporter list
//! naming two of four). A human reviewer catches that class of bug by reading prose and code side
//! by side; a test suite reads neither, unless something makes the prose itself an assertion.
//!
//! Every test here **parses documentation at runtime** (`include_str!`, never a hand-copied
//! table) and drives either the real `vitest-dedup` binary or the library's public reporters
//! against what it parsed. Two consequences follow from that design, on purpose:
//!
//! - If a table is reformatted so the parser can no longer find it, [`parse_table`] panics naming
//!   what it expected, rather than silently returning nothing and passing. A parser that returns
//!   `vec![]` on a reformat would make the failure mode this file exists to close reappear as
//!   quietly as the drift itself.
//! - If documentation and behaviour actually disagree, the relevant test fails. That is the
//!   intended outcome, not a bug in this file -- see the report below for what currently fails.

use assert_cmd::Command;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use vitest_dedup::cli::ParsedArgs;
use vitest_dedup::config;
use vitest_dedup::model::{
    AnalysisResult, Finding, IncompleteCauses, Location, ModeSummary, Position, Rule, Severity,
    Span,
};
use vitest_dedup::reporters::{JsonReporter, Reporter};

const README: &str = include_str!("../README.md");
const RULES: &str = include_str!("../docs/rules.md");
/// The shipped Agent Skill. It is a *third* place the remedy is written down, and the one an
/// agent actually acts on -- the S10 sweep updated the message, `full_description` and the README
/// and left this stale, which is the sibling-site defect the sweep itself was about.
const AGENT_SKILL: &str = include_str!("../skills/vitest-dedup/SKILL.md");
const REPORTS_DOC: &str = include_str!("../docs/reports.md");
const JSON_REPORTER_SOURCE: &str = include_str!("../src/reporters/json.rs");
const VITEST_SURFACE_DOC: &str = include_str!("../docs/vitest-surface.md");

// ---------------------------------------------------------------------------------------------
// Generic markdown-table parsing. Shared by every table-backed contract below so there is exactly
// one place that knows what a GFM table looks like, instead of one hand-rolled parser per table.
// ---------------------------------------------------------------------------------------------

/// Splits one markdown source line into its table cells, or `None` if the line is not a table
/// row at all (does not start with `|` once trimmed).
fn split_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') {
        return None;
    }
    let inner = trimmed.trim_start_matches('|').trim_end_matches('|');
    Some(inner.split('|').map(str::to_owned).collect())
}

/// A GFM header-separator row (`| --- | ---: | :--- |`): every cell contains only `-`, `:`, and
/// whitespace, and at least one dash.
fn is_separator_row(line: &str) -> bool {
    match split_row(line) {
        Some(cells) if !cells.is_empty() => cells.iter().all(|cell| {
            let trimmed = cell.trim();
            !trimmed.is_empty() && trimmed.chars().all(|ch| matches!(ch, '-' | ':'))
        }),
        _ => false,
    }
}

/// Strips the markdown formatting a table cell is likely to carry (code spans, emphasis) and
/// surrounding whitespace, so a parsed cell reads the same whether or not the author wrapped it
/// in backticks.
fn clean_cell(cell: &str) -> String {
    cell.replace(['`', '*'], "").trim().to_owned()
}

/// Finds the first markdown table in `markdown` whose header row's cells equal `header`
/// (case-insensitively, after [`clean_cell`]), and returns its body rows, each cell cleaned.
///
/// Panics -- naming the header it was looking for and the document it searched -- if no such
/// table is found, or if it is found but has zero body rows. This is the mechanism that makes a
/// reformatted table a loud test failure instead of a silently-empty, passing one.
fn parse_table(markdown: &str, header: &[&str], doc_name: &str) -> Vec<Vec<String>> {
    let lines: Vec<&str> = markdown.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let Some(cells) = split_row(line) else {
            continue;
        };
        let cleaned: Vec<String> = cells.iter().map(|c| clean_cell(c)).collect();
        if cleaned.len() != header.len() {
            continue;
        }
        let is_match = cleaned
            .iter()
            .zip(header)
            .all(|(cell, expected)| cell.eq_ignore_ascii_case(expected));
        if !is_match {
            continue;
        }
        let Some(separator) = lines.get(i + 1) else {
            continue;
        };
        if !is_separator_row(separator) {
            continue;
        }
        let mut rows = Vec::new();
        let mut j = i + 2;
        while let Some(row) = lines.get(j).and_then(|l| split_row(l)) {
            rows.push(row.iter().map(|c| clean_cell(c)).collect());
            j += 1;
        }
        assert!(
            !rows.is_empty(),
            "found the {header:?} table header in {doc_name} but it has no body rows -- \
             a table with a header and no rows almost certainly means the parser stopped in \
             the wrong place, not that the table is genuinely empty"
        );
        return rows;
    }
    panic!(
        "could not find a markdown table with header {header:?} in {doc_name}. Either the table \
         was reformatted in a way this parser does not recognize -- fix parse_table in \
         tests/docs_contract.rs -- or the table itself was removed, in which case the contract it \
         pinned needs a maintainer decision, not a silent pass."
    );
}

// ---------------------------------------------------------------------------------------------
// Shared fixtures. Built in a TempDir rather than read from `fixtures/`, deliberately: these
// contracts must keep holding no matter how the fixture corpus is edited, so nothing here may
// depend on a tree whose contents change independently of this file.
// ---------------------------------------------------------------------------------------------

/// Write distinct test bodies for documentation examples that require a clean analysis result.
fn write_clean_fixture(dir: &Path) {
    fs::write(
        dir.join("clean.test.ts"),
        r#"
import { test, expect } from 'vitest'

test('adds one and one', () => {
  expect(1 + 1).toBe(2)
})

test('multiplies two and three', () => {
  expect(2 * 3).toBe(6)
})
"#,
    )
    .unwrap();
}

/// Two tests with byte-identical bodies and different titles: a textbook `duplicate-test` (Error
/// by default), and nothing else -- titles differ, so `duplicate-test-name` must not also fire.
fn write_duplicate_fixture(dir: &Path) {
    fs::write(
        dir.join("dup.test.ts"),
        r#"
import { test, expect } from 'vitest'

test('adds one and one', () => {
  expect(1 + 1).toBe(2)
})

test('adds one and one again', () => {
  expect(1 + 1).toBe(2)
})
"#,
    )
    .unwrap();
}

/// Two tests sharing a full title with different bodies: a textbook `duplicate-test-name`
/// (Warning by default) with bodies too different to also trip `duplicate-test`.
/// Two tests with identical structure differing only in literal values -- a `test.each` table
/// waiting to happen, and the `parameterization-candidate` trigger.
///
/// Note the bodies must be non-trivial: a recognized stub is excluded from comparison, so a
/// fixture built from `expect(true).toBe(true)` would produce no finding and silently check
/// nothing.
fn write_parameterization_fixture(dir: &Path) {
    fs::write(
        dir.join("parameterization.test.ts"),
        r#"
import { test, expect } from 'vitest'
import { area } from './geometry'

test('area of a 2x3 rectangle', () => {
  expect(area(2, 3)).toBe(6)
})

test('area of a 4x5 rectangle', () => {
  expect(area(4, 5)).toBe(20)
})
"#,
    )
    .unwrap();
}

/// Two tests whose independent setup calls run in opposite order before an identical assertion:
/// same multiset of behavior events, different sequence, differing alpha body -- the
/// reordered-setup transformation `near-duplicate-test` bridges (`src/analysis.rs`'s module doc).
fn write_near_duplicate_fixture(dir: &Path) {
    fs::write(
        dir.join("near-duplicate.test.ts"),
        r#"
import { test, expect } from 'vitest'
import { configureThing, seedThing, isReady } from './setup'

test('configures then seeds', () => {
  configureThing({ mode: 'test' })
  seedThing({ count: 3 })
  expect(isReady()).toBe(true)
})

test('seeds then configures', () => {
  seedThing({ count: 3 })
  configureThing({ mode: 'test' })
  expect(isReady()).toBe(true)
})
"#,
    )
    .unwrap();
}

/// The same alpha-equal body under an equivalent context, split across two files -- the exact
/// `duplicate-test` criteria `cross-file-duplicate` reports without the file-scoping restriction
/// (`src/analysis.rs`'s module doc). `duplicate-test` itself is file-scoped, so this pair produces
/// nothing under it; only `cross-file-duplicate` has anything to say about it, which since that
/// rule's default became `Warning` makes this fixture's finding observable in a default run.
fn write_cross_file_duplicate_fixture(dir: &Path) {
    fs::write(
        dir.join("cross-file-a.test.ts"),
        r#"
import { test, expect } from 'vitest'

test('adds one and one in file a', () => {
  expect(1 + 1).toBe(2)
})
"#,
    )
    .unwrap();
    fs::write(
        dir.join("cross-file-b.test.ts"),
        r#"
import { test, expect } from 'vitest'

test('adds one and one in file b', () => {
  expect(1 + 1).toBe(2)
})
"#,
    )
    .unwrap();
}

/// Write a shorter/longer behavior pair for the documented subsumed-test rule.
fn write_subsumed_fixture(dir: &Path) {
    fs::write(
        dir.join("subsumed.test.ts"),
        r#"
import { test, expect } from 'vitest'

test('submits the order', () => {
  const order = buildOrder()
  order.submit()
  expect(order.status).toBe('sent')
})

test('submits and confirms the order', () => {
  const order = buildOrder()
  order.submit()
  expect(order.status).toBe('sent')
  order.confirm()
  expect(order.confirmed).toBe(true)
})
"#,
    )
    .unwrap();
}

/// Write matching suite fixtures for the duplicate-suite documentation contract.
fn write_duplicate_suite_fixture(dir: &Path) {
    fs::write(
        dir.join("suite.test.ts"),
        r#"
import { describe, test, expect } from 'vitest'

describe('checkout', () => {
  test('shows the total', () => {
    const cart = buildCart()
    cart.refresh()
    expect(cart.total).toBe(90)
  })
})

describe('checkout', () => {
  test('shows the total', () => {
    const cart = buildCart()
    cart.refresh()
    expect(cart.total).toBe(90)
  })
})
"#,
    )
    .unwrap();
}

/// Write a parameterized test with repeated literal rows for the duplicate-each-row example.
fn write_duplicate_each_row_fixture(dir: &Path) {
    fs::write(
        dir.join("each-row.test.ts"),
        r#"
import { test, expect } from 'vitest'

test.each([[2], [2]])('doubles %i', (input) => {
  expect(input * 2).toBeGreaterThan(0)
})
"#,
    )
    .unwrap();
}

/// The hook body is Vitest's own mock reset, not an ordinary `connect()`/`seed()` pair.
///
/// Since S5 `redundant-hook` reports only pairs whose bodies are provably IDEMPOTENT, and
/// `connect('primary'); db.seed()` is exactly the shape that made the rule 0-of-10 on the
/// 2026-09-27 scan: repeating it is not redundant, it is what the test does. This fixture's job
/// is to make the rule emit ONE finding so the documented default severity can be read off a
/// real report, so it uses a body the rule can legitimately report.
fn write_redundant_hook_fixture(dir: &Path) {
    fs::write(
        dir.join("hook.test.ts"),
        r#"
import { describe, test, expect, beforeEach, vi } from 'vitest'

describe('outer', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.unstubAllGlobals()
  })

  test('monthly totals', () => {
    const rows = 4
    expect(rows).toBe(4)
  })

  describe('inner', () => {
    beforeEach(() => {
      vi.clearAllMocks()
      vi.unstubAllGlobals()
    })

    test('regional totals', () => {
      const regions = ['north', 'south']
      expect(regions).toHaveLength(2)
    })
  })
})
"#,
    )
    .unwrap();
}

/// Write same-title tests with different bodies for the duplicate-name documentation example.
fn write_name_collision_fixture(dir: &Path) {
    fs::write(
        dir.join("name-collision.test.ts"),
        r#"
import { test, expect } from 'vitest'

test('does something important', () => {
  expect(1 + 1).toBe(2)
})

test('does something important', () => {
  expect('a' + 'b').toBe('ab')
})
"#,
    )
    .unwrap();
}

/// A `with` statement is a construct the extractor deliberately never recurses into --
/// `src/frontend/extract.rs`'s `defect1_with_statement_is_reported_incomplete_not_silently_skipped`
/// pins the same fact -- making it a reliable, minimal way to produce a degraded (`incomplete`)
/// run without touching the shared `fixtures/` tree.
fn write_degraded_fixture(dir: &Path) {
    fs::write(
        dir.join("degraded.test.ts"),
        r#"
import { test } from 'vitest'

with (globalThis) {
  test('hidden inside a with block', () => {})
}
"#,
    )
    .unwrap();
}

// =================================================================================================
// Contract 1: the exit-code table in README.md.
//
// Pinned because `--fail-on-incomplete` was documented as exit 2 and returned exit 1 (section 5b's
// own motivating example). The table is parsed once per test rather than cached, since each test
// must independently survive the table being reformatted or reordered.
// =================================================================================================

/// Parse numeric exit codes and meanings from the README contract table.
fn exit_code_table() -> Vec<(i32, String)> {
    parse_table(README, &["Code", "Meaning"], "README.md")
        .into_iter()
        .map(|row| {
            let code: i32 = row[0].trim().parse().unwrap_or_else(|_| {
                panic!("README's exit-code table has a non-numeric code in row {row:?}")
            });
            (code, row[1].clone())
        })
        .collect()
}

/// Looks up the documented meaning for `code`, failing loudly (naming the missing code) if the
/// table no longer documents it -- a code silently dropped from the table is exactly as much a
/// contract violation as one whose behaviour changed.
fn documented_meaning(codes: &[(i32, String)], code: i32) -> String {
    codes
        .iter()
        .find(|(c, _)| *c == code)
        .unwrap_or_else(|| panic!("README's exit-code table does not document exit code {code}"))
        .1
        .clone()
}

/// Regression fixture for public documentation: exit code 0 on a clean corpus.
#[test]
fn exit_code_0_on_a_clean_corpus() {
    let codes = exit_code_table();
    documented_meaning(&codes, 0);

    let dir = tempfile::tempdir().unwrap();
    write_clean_fixture(dir.path());
    let mut cmd = Command::cargo_bin("vitest-dedup").unwrap();
    cmd.arg(dir.path());
    cmd.assert().code(0);
}

/// Regression fixture for public documentation: exit code 1 on an error severity finding at the
/// default threshold.
#[test]
fn exit_code_1_on_an_error_severity_finding_at_the_default_threshold() {
    let codes = exit_code_table();
    documented_meaning(&codes, 1);

    let dir = tempfile::tempdir().unwrap();
    write_duplicate_fixture(dir.path());
    let mut cmd = Command::cargo_bin("vitest-dedup").unwrap();
    cmd.arg(dir.path());
    cmd.assert().code(1);
}

#[test]
fn exit_code_0_when_a_nonzero_threshold_permits_the_error_rate() {
    let codes = exit_code_table();
    let meaning = documented_meaning(&codes, 0);
    assert!(
        meaning.to_lowercase().contains("threshold"),
        "README's exit-0 row should describe the threshold rule this test exercises; parsed: \
         {meaning:?}"
    );

    let dir = tempfile::tempdir().unwrap();
    write_duplicate_fixture(dir.path());
    let mut cmd = Command::cargo_bin("vitest-dedup").unwrap();
    cmd.arg(dir.path());
    cmd.args(["--threshold", "100"]);
    cmd.assert().code(0);
}

#[test]
fn exit_code_2_on_a_bad_config_path() {
    let codes = exit_code_table();
    let meaning = documented_meaning(&codes, 2);
    assert!(
        meaning.to_lowercase().contains("config"),
        "README's exit-2 row should mention configuration; parsed: {meaning:?}"
    );

    let dir = tempfile::tempdir().unwrap();
    write_clean_fixture(dir.path());
    let mut cmd = Command::cargo_bin("vitest-dedup").unwrap();
    cmd.arg(dir.path());
    cmd.args(["--config", "does-not-exist.json"]);
    cmd.assert().code(2);
}

#[test]
fn exit_code_2_on_fail_on_incomplete_with_a_degraded_run() {
    let codes = exit_code_table();
    let meaning = documented_meaning(&codes, 2);
    assert!(
        meaning.contains("--fail-on-incomplete"),
        "README's exit-2 row should name --fail-on-incomplete; parsed: {meaning:?}"
    );

    let dir = tempfile::tempdir().unwrap();
    write_degraded_fixture(dir.path());
    let mut cmd = Command::cargo_bin("vitest-dedup").unwrap();
    cmd.arg(dir.path());
    cmd.arg("--fail-on-incomplete");
    cmd.assert().code(2);
}

// =================================================================================================
// Contract 2: the reporter table in docs/reports.md.
//
// Pinned because the motivating review findings include a reporter list naming two of four --
// exactly the shape of bug "each output path was tested in isolation" (section 5c) also produces:
// nobody re-checks the whole table once one reporter changes.
// =================================================================================================

/// Parse reporter names/destinations from the public report documentation table.
fn reporter_table() -> Vec<(String, String)> {
    parse_table(
        REPORTS_DOC,
        &["Reporter", "Destination", "Best for"],
        "docs/reports.md",
    )
    .into_iter()
    .map(|row| (row[0].clone(), row[1].clone()))
    .collect()
}

#[test]
fn every_documented_reporter_is_accepted_by_the_reporters_flag() {
    for (name, _destination) in reporter_table() {
        ParsedArgs::parse_from(["vitest-dedup", "--reporters", &name]).unwrap_or_else(|error| {
            panic!(
                "docs/reports.md documents `{name}` as a reporter, but \
                 `--reporters {name}` was rejected: {error}"
            )
        });
    }
}

#[test]
fn stdout_reporters_write_to_stdout_and_create_no_file_report() {
    for (name, destination) in reporter_table() {
        if !destination.eq_ignore_ascii_case("stdout") {
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        write_duplicate_fixture(dir.path());
        let mut cmd = Command::cargo_bin("vitest-dedup").unwrap();
        cmd.arg(dir.path());
        cmd.args(["--reporters", &name]);
        let output = cmd.output().unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            !stdout.trim().is_empty(),
            "docs/reports.md documents `{name}`'s destination as stdout, but nothing was \
             written to stdout"
        );
        assert!(
            !dir.path().join("reports").exists(),
            "docs/reports.md documents `{name}` as stdout-only, but running it also created a \
             `reports/` directory (the default file-reporter location)"
        );
    }
}

#[test]
fn file_reporters_write_a_file_and_never_dump_report_content_to_stdout() {
    for (name, destination) in reporter_table() {
        if !destination.eq_ignore_ascii_case("file") {
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        write_duplicate_fixture(dir.path());
        let prefix = dir.path().join("out");
        let mut cmd = Command::cargo_bin("vitest-dedup").unwrap();
        cmd.arg(dir.path());
        cmd.args(["--reporters", &name, "--output"]);
        cmd.arg(&prefix);
        let output = cmd.output().unwrap();

        // Both documented file reporters today happen to be named after their own file
        // extension (`json` -> `.json`, `html` -> `.html`), matching `ReporterKind::extension`
        // in src/cli.rs. If a future file reporter breaks that naming coincidence, this
        // assertion is the one to update alongside it.
        let report_path = prefix.with_extension(&name);
        assert!(
            report_path.exists(),
            "docs/reports.md documents `{name}`'s destination as a file, but {} was never \
             created",
            report_path.display()
        );
        let report_contents = fs::read_to_string(&report_path).unwrap();
        assert!(
            !report_contents.trim().is_empty(),
            "{} was created but is empty",
            report_path.display()
        );

        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            !stdout.contains("adds one and one"),
            "`{name}` is documented as file-only, but analyzed test content leaked onto \
             stdout: {stdout:?}"
        );
    }
}

/// docs/reports.md states the terminal/JSONL exclusion; the binary rejects the pair with exit 2.
#[test]
fn terminal_and_jsonl_mutual_exclusion_is_documented_and_enforced() {
    // The prose claim itself, checked so a rewording that silently drops the claim is visible --
    // this is not a table, so it cannot go through parse_table, but it must not be able to
    // disappear from the doc without this test noticing.
    assert!(
        REPORTS_DOC.contains("cannot be selected together"),
        "docs/reports.md no longer states the terminal/jsonl mutual exclusion in the wording \
         this test looks for; update the wording check alongside the doc"
    );

    let dir = tempfile::tempdir().unwrap();
    write_clean_fixture(dir.path());
    let mut cmd = Command::cargo_bin("vitest-dedup").unwrap();
    cmd.arg(dir.path());
    cmd.args(["--reporters", "terminal,jsonl"]);
    cmd.assert().failure().code(2);
}

// ================================================================================================
// Contract 3: the rule table in docs/rules.md.
//
// Each documented default must equal the rule's real default, and the rule set must equal Rule::ALL.
// ================================================================================================

/// Rows of the docs/rules.md rule table as (rule, default, meaning) strings.
fn rule_table() -> Vec<(String, String, String)> {
    parse_table(RULES, &["Rule", "Default", "Meaning"], "docs/rules.md")
        .into_iter()
        .map(|row| (row[0].clone(), row[1].clone(), row[2].clone()))
        .collect()
}

/// Every rule id documented in docs/rules.md parses in the config API and the `--rule` flag.
#[test]
fn every_documented_rule_id_is_accepted_by_the_rule_flag() {
    for (id, _default, _meaning) in rule_table() {
        config::parse_rule_id(&id).unwrap_or_else(|error| {
            panic!("docs/rules.md documents rule `{id}`, but config::parse_rule_id rejected it: {error}")
        });
        ParsedArgs::parse_from(["vitest-dedup", "--rule", &format!("{id}=warning")])
            .unwrap_or_else(|error| {
                panic!(
                    "docs/rules.md documents rule `{id}`, but `--rule {id}=warning` was rejected: \
                     {error}"
                )
            });
    }
}

/// The `--rule` flag rejects an id that no rule table documents.
#[test]
fn rule_flag_rejects_an_id_no_table_documents() {
    assert!(
        ParsedArgs::parse_from(["vitest-dedup", "--rule", "not-a-real-rule=warning"]).is_err(),
        "`--rule` must reject an unknown rule id rather than silently accepting it"
    );
}

/// The docs/rules.md table lists exactly the rules in `Rule::ALL`: none missing, none extra.
#[test]
fn documented_rule_table_enumerates_every_rule_the_binary_implements() {
    let documented: BTreeSet<String> = rule_table().into_iter().map(|(id, _, _)| id).collect();
    let implemented: BTreeSet<String> = Rule::ALL.iter().map(|rule| rule.id().to_owned()).collect();
    assert_eq!(
        documented, implemented,
        "docs/rules.md's rule table (left) and Rule::ALL in src/model.rs (right) disagree about which \
         rules exist; see the two sets above for exactly which id(s) are undocumented or stale"
    );
}

#[test]
/// Each documented default severity must match the rule's real default; see docs/rules.md.
fn documented_default_severities_are_observed_in_a_real_report() {
    for (id, default_severity, _meaning) in rule_table() {
        let expected = default_severity.to_lowercase();
        config::parse_severity(&expected).unwrap_or_else(|error| {
            panic!(
                "docs/rules.md gives rule `{id}` a default severity of `{default_severity}`, which is \
                 not a valid severity spelling: {error}"
            )
        });

        let dir = tempfile::tempdir().unwrap();
        match id.as_str() {
            "duplicate-test" => write_duplicate_fixture(dir.path()),
            "duplicate-test-name" => write_name_collision_fixture(dir.path()),
            "parameterization-candidate" => write_parameterization_fixture(dir.path()),
            "near-duplicate-test" => write_near_duplicate_fixture(dir.path()),
            "cross-file-duplicate" => write_cross_file_duplicate_fixture(dir.path()),
            "subsumed-test" => write_subsumed_fixture(dir.path()),
            "duplicate-suite" => write_duplicate_suite_fixture(dir.path()),
            "duplicate-each-row" => write_duplicate_each_row_fixture(dir.path()),
            "redundant-hook" => write_redundant_hook_fixture(dir.path()),
            other => panic!(
                "docs/rules.md documents rule `{other}` but tests/docs_contract.rs does not know how \
                 to trigger it -- add a fixture+case above so this rule's documented default \
                 severity stays pinned rather than silently unchecked"
            ),
        }

        let prefix = dir.path().join("out");
        let mut cmd = Command::cargo_bin("vitest-dedup").unwrap();
        cmd.arg(dir.path());
        cmd.args(["--reporters", "json", "--output"]);
        cmd.arg(&prefix);
        let _ = cmd.output().unwrap();
        let report_path = prefix.with_extension("json");
        let contents = fs::read_to_string(&report_path).unwrap_or_else(|error| {
            panic!(
                "expected a JSON report at {}: {error}",
                report_path.display()
            )
        });
        let report: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let findings = report["findings"].as_array().expect("findings is an array");

        // `off` is not a severity any finding is ever reported at (`Severity` has no such
        // variant a `Finding` carries) -- it means the rule does no work at all under the
        // default config, so "at least one finding at the documented severity" is unobservable
        // by construction for these rules. Assert the two halves separately instead: the default
        // run above must contain zero findings for this rule id (the "off" half), and the same
        // fixture with the rule explicitly turned on must produce at least one (proving the
        // fixture genuinely triggers the rule, rather than the zero above being a fixture defect
        // wearing this contract's clothes).
        if expected == "off" {
            assert!(
                findings.iter().all(|finding| finding["rule"] != id),
                "docs/rules.md documents `{id}`'s default severity as `off`, but a default-config real \
                 report still contains a `{id}` finding: {report}"
            );

            let prefix_on = dir.path().join("out-on");
            let mut cmd_on = Command::cargo_bin("vitest-dedup").unwrap();
            cmd_on.arg(dir.path());
            cmd_on.args(["--reporters", "json", "--output"]);
            cmd_on.arg(&prefix_on);
            cmd_on.args(["--rule", &format!("{id}=warning")]);
            let _ = cmd_on.output().unwrap();
            let report_on_path = prefix_on.with_extension("json");
            let contents_on = fs::read_to_string(&report_on_path).unwrap_or_else(|error| {
                panic!(
                    "expected a JSON report at {}: {error}",
                    report_on_path.display()
                )
            });
            let report_on: serde_json::Value = serde_json::from_str(&contents_on).unwrap();
            let findings_on = report_on["findings"]
                .as_array()
                .expect("findings is an array");
            assert!(
                findings_on.iter().any(|finding| finding["rule"] == id),
                "expected the `{id}` fixture to produce at least one `{id}` finding once \
                 explicitly enabled via --rule; report was: {report_on}"
            );
            continue;
        }

        let matching: Vec<&serde_json::Value> = findings
            .iter()
            .filter(|finding| finding["rule"] == id)
            .collect();
        assert!(
            !matching.is_empty(),
            "expected the `{id}` fixture to produce at least one `{id}` finding; report was: \
             {report}"
        );
        for finding in matching {
            assert_eq!(
                finding["severity"], expected,
                "docs/rules.md documents `{id}`'s default severity as `{default_severity}`, but a real \
                 report (default config, no overrides) says `{}`",
                finding["severity"]
            );
        }
    }
}

/// The near-duplicate remedy must match in the finding message, Rule::full_description and docs/rules.md prose.
#[test]
fn near_duplicate_remedy_prose_matches_the_message_the_binary_emits() {
    const DELETION_FRAMING: &str = "before deleting";

    let dir = tempfile::tempdir().unwrap();
    write_near_duplicate_fixture(dir.path());
    let prefix = dir.path().join("out");
    let mut cmd = Command::cargo_bin("vitest-dedup").unwrap();
    cmd.arg(dir.path());
    cmd.args(["--reporters", "json", "--output"]);
    cmd.arg(&prefix);
    let _ = cmd.output().unwrap();
    let report_path = prefix.with_extension("json");
    let contents = fs::read_to_string(&report_path).unwrap_or_else(|error| {
        panic!(
            "expected a JSON report at {}: {error}",
            report_path.display()
        )
    });
    let report: serde_json::Value = serde_json::from_str(&contents).unwrap();
    let message = report["findings"]
        .as_array()
        .expect("findings is an array")
        .iter()
        .find(|finding| finding["rule"] == "near-duplicate-test")
        .and_then(|finding| finding["message"].as_str())
        .unwrap_or_else(|| {
            panic!(
                "the near-duplicate fixture must produce a `near-duplicate-test` finding to read \
                 a message from; report was: {report}"
            )
        })
        .to_owned();

    assert!(
        message.contains("consolidate"),
        "the `near-duplicate-test` message must name consolidation as the remedy: {message}"
    );
    assert!(
        !message.contains(DELETION_FRAMING),
        "the `near-duplicate-test` message must not frame deletion as the decision: {message}"
    );

    let full_description = Rule::NearDuplicateTest.full_description();
    assert!(
        full_description.to_lowercase().contains("consolidat"),
        "the message proposes consolidation, so `Rule::full_description` (which a SARIF consumer \
         reads without a result in hand) must too: {full_description}"
    );
    assert!(
        !full_description.contains(DELETION_FRAMING),
        "`Rule::full_description` still frames the decision as deletion: {full_description}"
    );

    // The Agent Skill bullet is sliced out alone, so a neighbouring bullet cannot supply the remedy.
    let lines: Vec<&str> = AGENT_SKILL.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with("- `near-duplicate-test`"))
        .expect(
            "the Agent Skill must carry a `near-duplicate-test` remediation bullet; without it \
             this contract has nothing to check",
        );
    let end = start
        + 1
        + lines[start + 1..]
            .iter()
            .position(|line| line.starts_with("- "))
            .unwrap_or(lines.len() - start - 1);
    let bullet = lines[start..end].join("\n");
    assert!(
        bullet.to_lowercase().contains("consolidat"),
        "the Agent Skill's `near-duplicate-test` bullet is what an agent acts on, so it must \
         propose the same remedy the binary emits: {bullet}"
    );

    // Bullet-level slice: a block search lets a neighbouring bullet supply the remedy word.
    let prose: Vec<String> = RULES
        .split("\n\n")
        .filter(|block| !block.trim_start().starts_with('|'))
        .flat_map(|block| block.split("\n- ").map(str::to_owned).collect::<Vec<_>>())
        .filter(|bullet| bullet.contains("`near-duplicate-test`"))
        .collect();
    assert!(
        !prose.is_empty(),
        "no docs/rules.md prose bullet outside the rule table mentions `near-duplicate-test`; either the \
         remedy paragraph was deleted or this contract's parse no longer finds it -- both are \
         failures, not a pass"
    );
    assert!(
        prose
            .iter()
            .any(|block| block.to_lowercase().contains("consolidat")),
        "the message proposes consolidation, but no docs/rules.md prose bullet about \
         `near-duplicate-test` says so; blocks found: {prose:#?}"
    );
    for block in &prose {
        assert!(
            !block.contains(DELETION_FRAMING),
            "a docs/rules.md prose bullet still frames `near-duplicate-test` as a deletion decision: \
             {block}"
        );
    }
}

// =================================================================================================
// Contract 4: the JSON schema field list in src/reporters/json.rs's doc comment.
//
// Pinned because the doc comment's worked example previously omitted `inert` even though it is
// always serialized -- a documented example that omits a field a consumer depends on. Checked at
// every nesting level the example shows (report, finding, location), not only the top level,
// because that historical omission was at the finding level, not the top level.
// =================================================================================================

/// Extracts the first fenced ` ```json ` block from a `//!` doc comment in `source`, stripping the
/// comment prefix from each line so the result parses as bare JSON.
fn extract_doc_json_example(source: &str, doc_name: &str) -> serde_json::Value {
    let mut in_block = false;
    let mut buffer = String::new();
    for line in source.lines() {
        let Some(content) = line.trim_start().strip_prefix("//!") else {
            if in_block {
                break; // Left the doc comment mid-block: malformed, stop rather than guess.
            }
            continue;
        };
        let content = content.strip_prefix(' ').unwrap_or(content);
        if content.trim() == "```json" {
            in_block = true;
            continue;
        }
        if in_block && content.trim() == "```" {
            return serde_json::from_str(&buffer).unwrap_or_else(|error| {
                panic!(
                    "found a ```json block in {doc_name}'s doc comment, but it does not parse \
                     as JSON: {error}\n---\n{buffer}"
                )
            });
        }
        if in_block {
            buffer.push_str(content);
            buffer.push('\n');
        }
    }
    panic!(
        "could not find a fenced ```json code block in {doc_name}'s doc comment; if the schema \
         example was reformatted or removed, this contract can no longer verify the field list \
         it is meant to pin"
    );
}

/// Return sorted JSON object keys, panicking with context when the value is not an object.
fn sorted_object_keys(value: &serde_json::Value, what: &str) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .unwrap_or_else(|| panic!("expected {what} to be a JSON object, got {value}"))
        .keys()
        .cloned()
        .collect();
    keys.sort_unstable();
    keys
}

/// Build a representative analysis result for public report-schema assertions.
fn sample_analysis_result() -> AnalysisResult {
    let location = |path: &str, line: u32, column: u32| Location {
        path: PathBuf::from(path),
        span: Span {
            start: Position { line, column },
            end: Position {
                line,
                column: column + 5,
            },
            start_byte: 0,
            end_byte: 5,
        },
    };
    AnalysisResult {
        findings: vec![Finding {
            registration_files: Vec::new(),
            rule: Rule::DuplicateTest,
            severity: Severity::Error,
            message: "identical bodies under an equivalent context".to_owned(),
            locations: vec![
                location("src/a.test.ts", 10, 3),
                location("src/b.test.ts", 20, 5),
            ],
            titles: vec!["suite > a".to_owned(), "suite > b".to_owned()],
            inert: vec![false, false],
            baselined: false,
        }],
        tests_analyzed: 4,
        files_analyzed: 2,
        incomplete_causes: IncompleteCauses::default(),
        had_error_finding: true,
        error_finding_locations: 0,
        unresolved_tests: 0,
        blind_files: Vec::new(),
        modes: ModeSummary::default(),
    }
}

#[test]
fn json_schema_field_lists_match_the_documented_example_at_every_nesting_level() {
    let doc = extract_doc_json_example(JSON_REPORTER_SOURCE, "src/reporters/json.rs");

    let doc_top_level = sorted_object_keys(&doc, "the doc example's top level");
    let doc_finding = &doc["findings"][0];
    let doc_finding_keys = sorted_object_keys(doc_finding, "the doc example's finding object");
    let doc_location = &doc_finding["locations"][0];
    let doc_location_keys = sorted_object_keys(doc_location, "the doc example's location object");

    let mut buffer = Vec::new();
    JsonReporter::new()
        .write_report(&sample_analysis_result(), &mut buffer)
        .unwrap();
    let real: serde_json::Value = serde_json::from_slice(&buffer).unwrap();

    assert_eq!(
        doc_top_level,
        sorted_object_keys(&real, "a real report"),
        "src/reporters/json.rs's doc comment and its real output disagree about the report's \
         top-level fields"
    );
    assert_eq!(
        doc_finding_keys,
        sorted_object_keys(&real["findings"][0], "a real finding"),
        "src/reporters/json.rs's doc comment and its real output disagree about a finding \
         object's fields (this is the level at which `inert` was previously omitted from the \
         documented example)"
    );
    assert_eq!(
        doc_location_keys,
        sorted_object_keys(&real["findings"][0]["locations"][0], "a real location"),
        "src/reporters/json.rs's doc comment and its real output disagree about a location \
         object's fields"
    );
}

// =================================================================================================
// Contract 5: deficiency claims carry a checked-against-code marker.
//
// Pinned because five of one review round's eight findings were the same shape: prose describing
// a defect or gap the implementation no longer has (a superseded hazard entry, two fixture
// header comments, a fixture-index row, and an action.yml/README pair). Nothing short of a human
// re-reading every hazard paragraph against the source ever catches that -- which is exactly what
// let it recur. This contract cannot tell whether a deficiency claim is *true* (that still needs a
// human or the corpus gate); it only makes an unreviewed one impossible to leave lying around
// silently, by requiring an explicit `[status: current]` marker on any sentence that asserts one.
//
// Scope is deliberately narrower than "every doc in the repo": `fixtures/duplicates/` is excluded
// because that corpus is continuously re-proven against the real binary by
// `scripts/check/check-duplicates-corpus.mjs` -- a stale deficiency claim there fails execution,
// not just a missing marker, so adding this check there would just duplicate an existing signal
// (and requires touching a directory this task does not own). The two places genuinely exposed --
// `fixtures/edge/` and friends (demonstrative fixtures nothing executes a verdict against) and
// `docs/vitest-surface.md`'s prose hazard list -- are exactly where all five findings actually
// lived, and where a false claim can otherwise sit unreviewed indefinitely.
//
// The phrase list is intentionally a subset of the obvious candidates: "silently" was tried and
// dropped because two *correct*, still-current hazards (`.sequential`'s removal, non-literal
// `.each` tables) use it to describe what a bad implementation would do, not what this one
// currently does -- it would have forced markers onto true statements, which is the false-positive
// noise this mechanism must stay under to avoid being disabled or ignored.
// =================================================================================================

const DEFICIENCY_PHRASES: &[&str] = &[
    "extractor gap",
    "not recognized",
    "false duplicate",
    "this version does not",
];

const CURRENT_STATUS_MARKER: &str = "[status: current]";

/// Lowercases and strips markdown emphasis (`` ` `` and `*`) and hyphens, so a phrase written as
/// `` **false-`duplicate-test`** `` or `KNOWN EXTRACTOR GAP` is found the same as its plain-text
/// spelling.
fn normalize_for_phrase_match(text: &str) -> String {
    text.to_lowercase().replace(['`', '*', '-'], " ")
}

/// Returns the first deficiency phrase found in `normalized` (already run through
/// [`normalize_for_phrase_match`]), or `None`.
fn find_deficiency_phrase(normalized: &str) -> Option<&'static str> {
    DEFICIENCY_PHRASES
        .iter()
        .find(|phrase| normalized.contains(*phrase))
        .copied()
}

/// A unit of prose to check: `location` names it for the failure message, `text` is the content to
/// scan.
struct DeficiencySite {
    location: String,
    text: String,
}

/// Recursively collects every `.ts` file under `root`, except any directory named `duplicates`
/// directly under `fixtures/` (see this section's module doc for why that corpus is out of scope).
fn fixture_ts_files_excluding_duplicates(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    /// Collect TypeScript fixture paths recursively while excluding directories named
    /// duplicates.
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("could not read directory {}: {error}", dir.display()))
        {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) == Some("duplicates") {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("ts") {
                out.push(path);
            }
        }
    }
    walk(root, &mut out);
    out
}

/// The leading contiguous run of `//` comment lines at the top of a fixture file -- its header
/// comment. Stops at the first line (blank or code) that is not a `//` line.
fn fixture_header_comment(path: &Path) -> String {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
    let mut header = String::new();
    for line in content.lines() {
        if line.trim_start().starts_with("//") {
            header.push_str(line);
            header.push('\n');
        } else {
            break;
        }
    }
    header
}

/// True if `line` opens a top-level numbered list item (`"11. ..."`, no leading indentation) --
/// as opposed to an indented continuation line belonging to the previous item.
fn is_top_level_numbered_item(line: &str) -> bool {
    if line.starts_with(char::is_whitespace) {
        return false;
    }
    let mut chars = line.chars();
    let mut saw_digit = false;
    loop {
        match chars.next() {
            Some(c) if c.is_ascii_digit() => saw_digit = true,
            Some('.') => break,
            _ => return false,
        }
    }
    saw_digit && chars.next() == Some(' ')
}

/// Splits `docs/vitest-surface.md`'s "Extraction hazards, ranked" numbered list into one
/// `DeficiencySite` per item (starting line number folded into `location`), by walking lines from
/// the section header to the next `## ` heading or `---` divider.
///
/// Panics naming what it expected if the section header cannot be found or the list turns out to
/// have zero items -- the same "loud failure on reformat" discipline [`parse_table`] uses, so a
/// heading rename doesn't silently stop this contract from checking anything.
fn hazard_sites(doc: &str) -> Vec<DeficiencySite> {
    const HEADER: &str = "## Extraction hazards, ranked";
    let lines: Vec<&str> = doc.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == HEADER)
        .unwrap_or_else(|| {
            panic!(
                "could not find the \"{HEADER}\" section in docs/vitest-surface.md -- either it \
                 was renamed (update hazard_sites in tests/docs_contract.rs) or removed (in which \
                 case this contract's target moved and needs a maintainer decision)"
            )
        });

    let mut sites = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") || trimmed == "---" {
            break;
        }
        if is_top_level_numbered_item(line) {
            if let Some((start_line, text)) = current.take() {
                sites.push(DeficiencySite {
                    location: format!("docs/vitest-surface.md:{start_line}"),
                    text,
                });
            }
            current = Some((index + 1, line.to_string()));
        } else if let Some((_, text)) = current.as_mut() {
            text.push('\n');
            text.push_str(line);
        }
    }
    if let Some((start_line, text)) = current.take() {
        sites.push(DeficiencySite {
            location: format!("docs/vitest-surface.md:{start_line}"),
            text,
        });
    }

    assert!(
        !sites.is_empty(),
        "found the \"{HEADER}\" heading but parsed zero numbered items -- the list's format \
         probably changed; update is_top_level_numbered_item/hazard_sites in \
         tests/docs_contract.rs"
    );
    sites
}

#[test]
fn deficiency_claims_carry_a_checked_against_code_marker() {
    let mut sites: Vec<DeficiencySite> = hazard_sites(VITEST_SURFACE_DOC);

    let fixtures_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    for path in fixture_ts_files_excluding_duplicates(&fixtures_root) {
        let text = fixture_header_comment(&path);
        if text.is_empty() {
            continue;
        }
        sites.push(DeficiencySite {
            location: path.display().to_string(),
            text,
        });
    }

    let unmarked: Vec<String> = sites
        .iter()
        .filter_map(|site| {
            let normalized = normalize_for_phrase_match(&site.text);
            let phrase = find_deficiency_phrase(&normalized)?;
            if normalized.contains(&normalize_for_phrase_match(CURRENT_STATUS_MARKER)) {
                return None;
            }
            Some(format!(
                "  {}: asserts a deficiency (matched {phrase:?}) without a `{CURRENT_STATUS_MARKER}` \
                 marker",
                site.location
            ))
        })
        .collect();

    assert!(
        unmarked.is_empty(),
        "found deficiency claim(s) with no `{CURRENT_STATUS_MARKER}` marker proving someone \
         checked them against the current implementation -- this is the exact failure mode \
         behind five stale-documentation review findings (prose describing a defect or gap the \
         code no longer has). Either the claim is still true (add `{CURRENT_STATUS_MARKER}` plus \
         a one-line justification once verified against the code) or it is stale (fix the prose \
         instead):\n{}",
        unmarked.join("\n")
    );
}
