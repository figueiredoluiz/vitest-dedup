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
