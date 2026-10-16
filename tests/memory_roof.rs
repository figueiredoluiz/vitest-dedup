//! The memory roof narrows the corpus at file granularity and changes nothing else.

use assert_cmd::Command;
use std::path::Path;

/// Files in the generated tree, and tests per file: about 2 MB retained at ~500 bytes per test,
/// so a 1 MiB budget stops part way.
const FILES: usize = 10;
const TESTS_PER_FILE: usize = 400;

/// Writes `files` generated test files, named so discovery reads them in index order.
fn write_tree(root: &Path, files: std::ops::Range<usize>) {
    for file in files {
        let tests: String = (0..TESTS_PER_FILE)
            .map(|test| {
                format!(
                    "test('case {file} {test}', () => {{ expect(add({}, 1)).toBe({}); }});\n",
                    test % 10,
                    test % 10 + 1
                )
            })
            .collect();
        std::fs::write(
            root.join(format!("f{file}.test.ts")),
            format!(
                "import {{ test, expect }} from 'vitest';\nimport {{ add }} from './add';\n{tests}"
            ),
        )
        .unwrap();
    }
}

/// What one analyzer run produced.
struct Run {
    code: i32,
    report: serde_json::Value,
    stderr: String,
}

/// Runs the analyzer on `root` with `flags` and the JSON reporter (plus any `reporters`).
fn analyze_with(root: &Path, reporters: &str, flags: &[&str]) -> Run {
    let output = tempfile::tempdir().unwrap();
    let prefix = output.path().join("report");
    let result = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(root)
        .args(["--reporters", reporters, "--no-metrics", "--output"])
        .arg(&prefix)
        .args(flags)
        .output()
        .unwrap();
    let report = std::fs::read_to_string(prefix.with_extension("json")).unwrap();
    Run {
        code: result.status.code().unwrap(),
        report: serde_json::from_str(&report).unwrap(),
        stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
    }
}

/// Runs the analyzer on `root` with `flags` and the JSON reporter.
fn analyze(root: &Path, flags: &[&str]) -> (i32, serde_json::Value) {
    let run = analyze_with(root, "json", flags);
    (run.code, run.report)
}

/// Runs a fixture Git command without inheriting a hook's repository variables.
fn git(root: &Path, args: &[&str]) {
    let mut command = std::process::Command::new("git");
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
        "GIT_CEILING_DIRECTORIES",
    ] {
        command.env_remove(variable);
    }
    let status = command.current_dir(root).args(args).status().unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A roof-truncated run reports exactly what a run over the files it read reports, plus the
/// incompleteness that says so; `=limits` gates on it and `=reads` does not.
#[test]
fn a_truncated_run_equals_a_run_over_the_files_it_read() {
    let full = tempfile::tempdir().unwrap();
    write_tree(full.path(), 0..FILES);
    let run = analyze_with(full.path(), "json", &["--max-memory", "1"]);
    assert!(
        run.stderr.contains("reached the memory budget (1 MiB)"),
        "{}",
        run.stderr
    );
    let truncated = run.report;
    let analyzed = truncated["filesAnalyzed"].as_u64().unwrap() as usize;
    let skipped = truncated["incompleteCauses"]["filesOverMemoryBudget"]
        .as_u64()
        .unwrap() as usize;
    assert!(
        analyzed > 0 && skipped > 0,
        "{analyzed} analyzed, {skipped} skipped"
    );
    assert_eq!(analyzed + skipped, FILES);
    assert_eq!(truncated["incomplete"], true);

    let subset = tempfile::tempdir().unwrap();
    write_tree(subset.path(), 0..analyzed);
    let (_, exact) = analyze(subset.path(), &[]);
    assert_eq!(exact["incomplete"], false);
    for field in ["findings", "testsAnalyzed", "filesAnalyzed"] {
        assert_eq!(truncated[field], exact[field], "{field}");
    }
    assert!(!exact["findings"].as_array().unwrap().is_empty());

    // The same budget over exactly those files crosses it on the last one: nothing is skipped.
    let last = analyze_with(subset.path(), "json", &["--max-memory", "1"]);
    assert_eq!(last.report["incomplete"], false);
    assert!(!last.stderr.contains("memory budget"), "{}", last.stderr);

    assert_eq!(
        analyze(
            full.path(),
            &["--max-memory", "1", "--fail-on-incomplete=limits"]
        )
        .0,
        2
    );
    assert_ne!(
        analyze(
            full.path(),
            &["--max-memory", "1", "--fail-on-incomplete=reads"]
        )
        .0,
        2
    );
}

/// Without a cap, or with the default budget, the generated tree is read in full.
#[test]
fn the_default_budget_and_no_cap_read_everything() {
    let full = tempfile::tempdir().unwrap();
    write_tree(full.path(), 0..FILES);
    for flags in [&[][..], &["--no-memory-cap"][..]] {
        let (_, report) = analyze(full.path(), flags);
        assert_eq!(report["filesAnalyzed"], FILES, "{flags:?}");
        assert_eq!(report["incomplete"], false, "{flags:?}");
    }
}

/// Source text kept for the HTML report counts against the budget: files whose tests retain
/// little but whose text is large are read in full for JSON and stopped part way for HTML.
#[test]
fn source_retained_for_html_counts_against_the_budget() {
    let root = tempfile::tempdir().unwrap();
    let padding = format!("// {}\n", "x".repeat(200_000));
    for file in 0..10 {
        std::fs::write(
            root.path().join(format!("f{file}.test.ts")),
            format!("import {{ test, expect }} from 'vitest';\n{padding}test('case {file}', () => {{ expect({file}).toBe({file}); }});\n"),
        )
        .unwrap();
    }
    let json = analyze_with(root.path(), "json", &["--max-memory", "1"]);
    assert_eq!(json.report["incomplete"], false, "{}", json.stderr);
    let html = analyze_with(root.path(), "json,html", &["--max-memory", "1"]);
    assert!(
        html.report["incompleteCauses"]["filesOverMemoryBudget"]
            .as_u64()
            .unwrap()
            > 0,
        "{}",
        html.stderr
    );
}

/// A test file that is also an imported runner's helper is retained once for HTML, so it is
/// charged once, whichever is read first: with a double charge the file after both would be left
/// unread.
#[test]
fn a_test_file_retained_as_a_helper_is_charged_once() {
    // (runner module, caller): the caller sorts first in the first case, the runner in the second.
    for (runner, caller) in [("b", "a"), ("a", "b")] {
        let root = tempfile::tempdir().unwrap();
        let padding = format!("// {}\n", "x".repeat(600_000));
        std::fs::write(
            root.path().join(format!("{caller}.test.ts")),
            format!("import {{ run }} from './{runner}.test';\nrun();\n"),
        )
        .unwrap();
        std::fs::write(
            root.path().join(format!("{runner}.test.ts")),
            format!("import {{ test, expect }} from 'vitest';\n{padding}export function run() {{ test('runner', () => {{ expect(1).toBe(1); }}); }}\n"),
        )
        .unwrap();
        std::fs::write(
            root.path().join("c.test.ts"),
            "import { test, expect } from 'vitest';\ntest('last', () => { expect(2).toBe(2); });\n",
        )
        .unwrap();
        let html = analyze_with(root.path(), "json,html", &["--max-memory", "1"]);
        assert_eq!(html.report["filesAnalyzed"], 3, "{runner}: {}", html.stderr);
        assert_eq!(html.report["testsAnalyzed"], 2, "{runner}: {}", html.stderr);
    }
}

/// `--baseline-from-ref` applies the budget to the base revision too, and says the baseline may be
/// partial.
#[test]
fn the_base_revision_pass_has_the_same_roof() {
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), 0..FILES);
    git(root.path(), &["init", "-q", "-b", "main"]);
    git(root.path(), &["config", "user.email", "test@example.com"]);
    git(root.path(), &["config", "user.name", "Test"]);
    git(root.path(), &["add", "."]);
    git(root.path(), &["commit", "-qm", "initial"]);
    let run = analyze_with(
        root.path(),
        "json",
        &["--baseline-from-ref", "HEAD", "--max-memory", "1"],
    );
    assert!(
        run.stderr
            .contains("the base revision reached the memory budget"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("baseline may be partial"),
        "{}",
        run.stderr
    );
}
