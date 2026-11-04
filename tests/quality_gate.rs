//! The quality gate counts eligible runtime cases: rows apart, each case once, placeholders out.

use serde_json::Value;
use std::path::Path;

/// One run over `files` written to a fresh project, with every report written beside it.
struct Run {
    code: Option<i32>,
    json: Value,
    jsonl_summary: Value,
    sarif: Value,
    html: String,
}

/// Writes `files` (path, source) under `root`.
fn write(root: &Path, files: &[(&str, &str)]) {
    for (path, source) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
}

/// Runs the binary over `root` with `flags`, writing json, sarif and html, then again for jsonl.
fn run_on(root: &Path, flags: &[&str]) -> Run {
    let output = tempfile::tempdir().unwrap();
    let prefix = output.path().join("report");
    let files = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(root)
        .args(["--reporters", "json,sarif,html", "--no-metrics", "--output"])
        .arg(&prefix)
        .args(flags)
        .output()
        .unwrap();
    let stream = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(root)
        .args(["--reporters", "jsonl", "--no-metrics"])
        .args(flags)
        .output()
        .unwrap();
    assert_eq!(files.status.code(), stream.status.code());
    let read = |extension| std::fs::read_to_string(prefix.with_extension(extension)).unwrap();
    let jsonl_summary = String::from_utf8(stream.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|record| record["type"] == "summary")
        .unwrap();
    Run {
        code: files.status.code(),
        json: serde_json::from_str(&read("json")).unwrap(),
        jsonl_summary,
        sarif: serde_json::from_str(&read("sarif")).unwrap(),
        html: read("html"),
    }
}

/// [`run_on`] over a fresh project holding `files`.
fn run(files: &[(&str, &str)], flags: &[&str]) -> Run {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), files);
    run_on(directory.path(), flags)
}

/// `(errorCases, eligibleCases, percentage)` of the JSON report, after checking every other
/// report publishes the same three figures.
fn gate(run: &Run) -> (u64, u64, Option<f64>) {
    let gate = &run.json["qualityGate"];
    assert_eq!(&run.jsonl_summary["qualityGate"], gate);
    assert_eq!(
        &run.sarif["runs"][0]["invocations"][0]["properties"]["qualityGate"],
        gate
    );
    assert!(
        run.html.contains(&format!("\"qualityGate\":{gate}")),
        "the HTML data island must carry {gate}"
    );
    (
        gate["errorCases"].as_u64().unwrap(),
        gate["eligibleCases"].as_u64().unwrap(),
        gate["percentage"].as_f64(),
    )
}

const PAIR: &str = "import { test, expect } from 'vitest'
test('one', () => { expect(1).toBe(1) })
test('two', () => { expect(1).toBe(1) })
test('three', () => { expect(3).toBe(4) })
test('four', () => { expect(5).toBe(6) })
";

/// Two duplicate tests in four: 50%, failing one point under it and passing at it.
#[test]
fn a_duplicate_pair_is_two_cases_of_four_at_the_threshold_boundary() {
    for (threshold, code) in [("49", 1), ("50", 0)] {
        let run = run(&[("a.test.ts", PAIR)], &["--threshold", threshold]);
        assert_eq!(gate(&run), (2, 4, Some(50.0)));
        assert_eq!(run.code, Some(code), "threshold {threshold}");
    }
}

/// Every duplicated row of one table is its own case; rows that differ are none.
#[test]
fn each_duplicated_row_counts_as_its_own_case() {
    let rows = |values: &str| {
        format!(
            "import {{ test, expect }} from 'vitest'
test.each([{values}])('row %s', (v) => {{ expect(v).toBe(1) }})
test('plain', () => {{ expect(2).toBe(2) }})
"
        )
    };
    let flags = ["--rule", "duplicate-each-row=error", "--threshold", "100"];
    let duplicated = run(&[("a.test.ts", &rows("[1], [1], [1]"))], &flags);
    assert_eq!(gate(&duplicated), (3, 4, Some(75.0)));
    let distinct = run(&[("a.test.ts", &rows("[1], [2], [3]"))], &flags);
    assert_eq!(gate(&distinct).0, 0);
}

/// A case two error findings name counts once.
#[test]
fn a_case_named_by_two_findings_counts_once() {
    let run = run(
        &[(
            "a.test.ts",
            "import { test, expect } from 'vitest'
test('same', () => { expect(1).toBe(1) })
test('same', () => { expect(1).toBe(1) })
test('other', () => { expect(2).toBe(3) })
",
        )],
        &["--rule", "duplicate-test-name=error", "--threshold", "100"],
    );
    let errors = run.json["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["severity"] == "error")
        .count();
    assert_eq!(errors, 2, "both rules report the pair at error");
    assert_eq!(gate(&run), (2, 3, Some(2.0 / 3.0 * 100.0)));
}

/// A copied block counts the cases it covers, not its two blocks.
#[test]
fn a_suite_copy_counts_its_children_not_its_blocks() {
    let run = run(
        &[(
            "a.test.ts",
            "import { describe, test, expect } from 'vitest'
describe('copy', () => { test('a', () => { expect(1).toBe(1) }); test('b', () => { expect(2).toBe(2) }) })
describe('copy', () => { test('a', () => { expect(1).toBe(1) }); test('b', () => { expect(2).toBe(2) }) })
",
        )],
        &["--threshold", "100"],
    );
    assert_eq!(run.json["findings"][0]["rule"], "duplicate-suite");
    assert_eq!(gate(&run), (4, 4, Some(100.0)));
}

/// Benchmarks, placeholders for what a module load or wrapper registers, and bodyless todos leave
/// the denominator; a skipped test with a callback stays, and every case stays analyzed.
#[test]
fn placeholders_benchmarks_and_bodyless_todos_are_not_eligible() {
    let run = run(
        &[
            ("a.test.ts", PAIR),
            (
                "b.bench.ts",
                "import { bench } from 'vitest'\nbench('fast', () => { Math.sqrt(2) })\n",
            ),
            ("c.test.ts", "import './loads'\n"),
            (
                "loads.ts",
                "import { test, expect } from 'vitest'\ntest('at load', () => { expect(3).toBe(3) })\n",
            ),
            (
                "d.test.ts",
                "import { register } from './helper'\nregister()\n",
            ),
            (
                "helper.ts",
                "import { describe, test, expect } from 'vitest'\nexport function register() { describe('s', () => { test('t', () => { expect(7).toBe(7) }) }) }\n",
            ),
            (
                "todo.test.ts",
                "import { test, expect } from 'vitest'\ntest.todo('later')\ntest.skip('skipped', () => { expect(4).toBe(4) })\n",
            ),
        ],
        &["--threshold", "100"],
    );
    assert_eq!(run.json["testsAnalyzed"], 9);
    assert_eq!(gate(&run), (2, 5, Some(40.0)));
}

/// With nothing eligible there is no rate, and the threshold passes.
#[test]
fn no_eligible_case_publishes_no_rate() {
    let run = run(
        &[(
            "b.bench.ts",
            "import { bench } from 'vitest'\nbench('fast', () => { Math.sqrt(2) })\n",
        )],
        &[],
    );
    assert_eq!(gate(&run), (0, 0, None));
    assert_eq!(run.code, Some(0));
}

/// Accepted debt leaves the numerator and stays in the denominator.
#[test]
fn a_baseline_removes_its_cases_from_the_numerator_only() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), &[("a.test.ts", PAIR)]);
    let baseline = directory.path().join("baseline.json");
    let baseline = baseline.to_str().unwrap();
    assert_eq!(
        run_on(
            directory.path(),
            &["--baseline", baseline, "--update-baseline"]
        )
        .code,
        Some(0)
    );
    write(
        directory.path(),
        &[(
            "b.test.ts",
            "import { test, expect } from 'vitest'
test('five', () => { expect(8).toBe(8) })
test('six', () => { expect(8).toBe(8) })
",
        )],
    );
    let run = run_on(
        directory.path(),
        &["--baseline", baseline, "--threshold", "100"],
    );
    assert_eq!(gate(&run), (2, 6, Some(2.0 / 6.0 * 100.0)));
}

/// `--changed-since` withholds findings in unchanged files, never cases from the denominator.
#[test]
fn changed_scope_keeps_the_full_denominator() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, &[("a.test.ts", PAIR)]);
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .current_dir(root)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.test"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "--quiet"]);
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "base"]);
    write(
        root,
        &[(
            "b.test.ts",
            "import { test, expect } from 'vitest'\ntest('five', () => { expect(9).toBe(10) })\n",
        )],
    );
    let run = run_on(root, &["--changed-since", "HEAD", "--threshold", "100"]);
    assert!(run.json["modes"]["withheldUnchanged"].as_u64().unwrap() > 0);
    assert_eq!(gate(&run), (0, 5, Some(0.0)));
}

/// Two copied tables count every matched row, not one representative per table, and the row order
/// inside them does not change the rate.
#[test]
fn copied_tables_count_every_matched_row_whatever_the_order() {
    let tables = |left: &str, right: &str| {
        format!(
            "import {{ test, expect }} from 'vitest'
test.each([{left}])('first %s', (v) => {{ expect(v).toBeGreaterThan(0) }})
test.each([{right}])('second %s', (v) => {{ expect(v).toBeGreaterThan(0) }})
"
        )
    };
    let copied = run(
        &[("a.test.ts", &tables("[1], [2], [3]", "[1], [2], [3]"))],
        &["--threshold", "100"],
    );
    assert_eq!(gate(&copied), (6, 6, Some(100.0)));
    let flags = ["--rule", "duplicate-each-row=error", "--threshold", "100"];
    let ordered = run(
        &[("a.test.ts", &tables("[1], [1], [2]", "[1], [1], [2]"))],
        &flags,
    );
    let reordered = run(
        &[("a.test.ts", &tables("[2], [1], [1]", "[1], [1], [2]"))],
        &flags,
    );
    assert_eq!(gate(&ordered), gate(&reordered));
    assert_eq!(gate(&ordered).0, 6);
}

/// A bodyless todo stays out of the denominator when another refusal is recorded for it first.
#[test]
fn a_bodyless_todo_stays_ineligible_under_another_refusal() {
    let run = run(
        &[
            (
                "a.test.ts",
                "import { test, expect } from 'vitest'
test('one', () => { expect(1).toBe(1) })
test('two', () => { expect(1).toBe(1) })
",
            ),
            (
                "todo.test.ts",
                "import { test, vi } from 'vitest'
vi.doMock('./dep')
test.todo('first')
test.todo('second')
test.todo('third')
",
            ),
        ],
        &["--threshold", "50"],
    );
    assert_eq!(gate(&run), (2, 2, Some(100.0)));
    assert_eq!(run.code, Some(1));
}

/// Every row of two tables whose rows all match counts, though one finding names one row of each.
#[test]
fn matching_tables_count_every_row_behind_their_representatives() {
    let run = run(
        &[(
            "a.test.ts",
            "import { test, expect } from 'vitest'
test.each([[1], [1], [1]])('first %s', (v) => { expect(v).toBe(1) })
test.each([[1], [1], [1]])('second %s', (v) => { expect(v).toBe(1) })
",
        )],
        &["--threshold", "100"],
    );
    assert_eq!(gate(&run), (6, 6, Some(100.0)));
}

/// A new row pair counts its own rows, not the rows of a baselined pair beside it.
#[test]
fn a_new_row_pair_counts_only_its_own_rows() {
    let directory = tempfile::tempdir().unwrap();
    let rows = |values: &str| {
        format!(
            "import {{ test, expect }} from 'vitest'\ntest.each([{values}])('row %s', (v) => {{ expect(v).toBe(1) }})\n"
        )
    };
    write(directory.path(), &[("a.test.ts", &rows("[1], [1]"))]);
    let baseline = directory.path().join("baseline.json");
    let baseline = baseline.to_str().unwrap();
    let flags = ["--rule", "duplicate-each-row=error"];
    let update = [&flags[..], &["--baseline", baseline, "--update-baseline"]].concat();
    assert_eq!(run_on(directory.path(), &update).code, Some(0));
    write(directory.path(), &[("a.test.ts", &rows("[1], [1], [1]"))]);
    let gated = [&flags[..], &["--baseline", baseline, "--threshold", "70"]].concat();
    let run = run_on(directory.path(), &gated);
    assert_eq!(gate(&run), (2, 3, Some(2.0 / 3.0 * 100.0)));
    assert_eq!(run.code, Some(0));
}

/// A subsumed table pair counts every row pair, not only the first.
#[test]
fn a_subsumed_table_pair_counts_every_row() {
    let run = run(
        &[(
            "a.test.ts",
            "import { test, expect } from 'vitest'
test.each([[1], [2], [3]])('smaller %s', (v) => { expect(v).toBeGreaterThan(0) })
test.each([[1], [2], [3]])('larger %s', (v) => { expect(v).toBeGreaterThan(0); expect(v).toBeLessThan(9) })
",
        )],
        &["--rule", "subsumed-test=error", "--threshold", "100"],
    );
    assert!(run.json["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["rule"] == "subsumed-test"));
    assert_eq!(gate(&run), (6, 6, Some(100.0)));
}

/// A suite copy at error stands for the tests inside it when no child rule reports them.
#[test]
fn a_suite_copy_counts_its_tests_without_child_findings() {
    let run = run(
        &[(
            "a.test.ts",
            "import { describe, test, expect } from 'vitest'
describe('copy', () => { test.each([[1], [2], [3]])('row %s', (v) => { expect(v).toBeGreaterThan(0) }) })
describe('copy', () => { test.each([[1], [2], [3]])('row %s', (v) => { expect(v).toBeGreaterThan(0) }) })
",
        )],
        &[
            "--rule",
            "duplicate-suite=error",
            "--rule",
            "duplicate-test=off",
            "--rule",
            "duplicate-test-name=off",
            "--rule",
            "parameterization-candidate=off",
            "--threshold",
            "100",
        ],
    );
    assert_eq!(run.json["findings"][0]["rule"], "duplicate-suite");
    assert!(run.json["findings"][0].get("covers").is_none());
    assert_eq!(gate(&run), (6, 6, Some(100.0)));
}

/// A redundant hook stays a finding at its severity and fails a zero-threshold run, but it is not
/// a test case: it adds nothing to the duplicated cases.
#[test]
fn a_redundant_hook_is_a_finding_but_no_case() {
    let source = "import { describe, test, expect, beforeEach } from 'vitest'
let region = ''
describe('outer', () => {
  beforeEach(() => { region = 'primary' })
  test('monthly', () => { expect(4).toBe(4) })
  describe('inner', () => {
    beforeEach(() => { region = 'primary' })
    test('regional', () => { expect(['north']).toHaveLength(1) })
  })
})
";
    for (threshold, code) in [("0", 1), ("1", 0)] {
        let run = run(
            &[("a.test.ts", source)],
            &["--rule", "redundant-hook=error", "--threshold", threshold],
        );
        let findings = run.json["findings"].as_array().unwrap();
        assert!(findings
            .iter()
            .any(|f| f["rule"] == "redundant-hook" && f["severity"] == "error"));
        assert_eq!(gate(&run), (0, 2, Some(0.0)));
        assert_eq!(run.code, Some(code), "threshold {threshold}");
    }
}

/// A suite copy at error counts every test inside its copies: suppressing one child finding
/// leaves the count where it was.
#[test]
fn a_suite_at_error_counts_its_tests_whatever_its_children_report() {
    let block = |suppressed: bool| {
        format!(
            "describe('copy', () => {{
  test.each([[1], [2]])('row %s', (v) => {{ expect(v).toBeGreaterThan(0) }})
{}  test('plain', () => {{ expect(2).toBe(2) }})
}})
",
            if suppressed {
                "  // vitest-dedup-disable-next-line intentional\n"
            } else {
                ""
            }
        )
    };
    for suppressed in [false, true] {
        let source = format!(
            "import {{ describe, test, expect }} from 'vitest'\n{}{}",
            block(false),
            block(suppressed)
        );
        let run = run(
            &[("a.test.ts", &source)],
            &[
                "--rule",
                "duplicate-suite=error",
                "--rule",
                "duplicate-test=error",
                "--rule",
                "duplicate-test-name=off",
                "--rule",
                "parameterization-candidate=off",
                "--threshold",
                "100",
            ],
        );
        let suite = &run.json["findings"][0];
        assert_eq!(suite["rule"], "duplicate-suite");
        assert_eq!(suite["severity"], "error");
        let covered = suite["covers"]["locations"].as_array().unwrap().len();
        assert_eq!(
            covered,
            if suppressed { 2 } else { 4 },
            "suppressed {suppressed}"
        );
        assert_eq!(gate(&run), (6, 6, Some(100.0)), "suppressed {suppressed}");
    }
}
