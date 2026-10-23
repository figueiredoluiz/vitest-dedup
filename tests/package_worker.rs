//! An installed package is scanned in a worker process, so a large bundle is read and a script
//! that overflows the parser's stack refuses its package without taking the analyzer down.
//!
//! Final-outcome contract over projects built on disk at run time (`node_modules` cannot live in
//! tracked fixtures): the identical pair is reported exactly when the package that the first
//! test file calls is admitted as setup.

use assert_cmd::Command;
use std::fs;
use std::path::Path;

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A project, under Vitest globals, whose two identical tests follow a call into the installed
/// package `bundle` whose entry is `entry`.
fn project(entry: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "vitest.config.ts",
        "export default { test: { globals: true } }\n",
    );
    write(
        root,
        "node_modules/bundle/package.json",
        r#"{ "name": "bundle", "version": "1.0.0", "main": "index.js" }"#,
    );
    write(root, "node_modules/bundle/index.js", entry);
    write(
        root,
        "users.test.ts",
        "import { install } from 'bundle'\n\ninstall()\n\n\
         test('first', () => { expect(1 + 1).toBe(2) })\n\
         test('second', () => { expect(1 + 1).toBe(2) })\n",
    );
    directory
}

/// Runs the analyzer over `root`, returning its exit code and its JSONL report.
fn analyze(root: &Path) -> (Option<i32>, String) {
    let output = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(root)
        .args(["--reporters", "jsonl"])
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

/// Whether the report is a completed run over the two tests, with `unresolved` of them excluded.
fn completed(report: &str, unresolved: u32) -> bool {
    report.contains("\"type\":\"summary\"")
        && report.contains("\"testsAnalyzed\":2")
        && report.contains(&format!("\"unresolvedTests\":{unresolved}"))
}

/// Whether the report holds the duplicate-test finding for the pair.
fn reports_the_pair(report: &str) -> bool {
    report.contains("\"duplicate-test\"") && report.contains("\"type\":\"finding\"")
}

/// A flat bundle past the old size budget is read, and found to register nothing.
#[test]
fn a_large_flat_bundle_is_admitted_as_setup() {
    let entry = format!("{}exports.install = () => {{}}\n", "x();\n".repeat(10_000));
    let directory = project(&entry);
    let (code, report) = analyze(directory.path());
    assert_eq!(code, Some(1), "{report}");
    assert!(completed(&report, 0), "{report}");
    assert!(reports_the_pair(&report), "{report}");
}

/// The same bundle that registers a test is refused whatever its size.
#[test]
fn a_large_bundle_that_registers_a_test_is_refused() {
    let entry = format!(
        "{}exports.install = () => describe('a', () => {{}})\n",
        "x();\n".repeat(10_000)
    );
    let directory = project(&entry);
    let (code, report) = analyze(directory.path());
    assert_eq!(code, Some(0), "{report}");
    assert!(completed(&report, 2), "{report}");
    assert!(!reports_the_pair(&report), "{report}");
}

/// A script nested deeper than any stack survives refuses its package; the analyzer still ends
/// normally, reports the pair as not compared, and its exit code is a code, not a signal.
#[test]
fn a_script_that_overflows_the_parser_refuses_its_package_only() {
    let depth = 2_000_000;
    let entry = format!(
        "exports.install = () => {}{}0{}\n",
        "",
        "(".repeat(depth),
        ")".repeat(depth)
    );
    let directory = project(&entry);
    let (code, report) = analyze(directory.path());
    assert_eq!(code, Some(0), "the analyzer did not end normally: {report}");
    assert!(completed(&report, 2), "{report}");
    assert!(!reports_the_pair(&report), "{report}");
    assert!(report.contains("\"incomplete\":true"), "{report}");
}

/// Strengthen the owned large-bundle corpus with a worker-start refusal and a retained finding:
/// every reporter carries the failure count, while cache hits and later runs do not inflate it.
#[test]
fn owned_package_worker_failures_reach_reports_and_exit_gates() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/recall/framework-package-large-flat-bundle");
    let directory = tempfile::tempdir().unwrap();
    for (blocked, gate, code) in [
        (false, "reads", 1),
        (true, "reads", 2),
        (true, "any", 2),
        (true, "limits", 0),
        (false, "reads", 1),
    ] {
        let prefix = directory.path().join("report");
        let mut command = Command::cargo_bin("vitest-dedup").unwrap();
        command
            .arg(&fixture)
            .args(["--reporters", "jsonl,json,html,sarif"])
            .arg("--output")
            .arg(&prefix)
            .arg(format!("--fail-on-incomplete={gate}"))
            .env_remove("VITEST_DEDUP_PACKAGE_WORKER");
        if blocked {
            command.env("VITEST_DEDUP_PACKAGE_WORKER", "1");
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let read = |extension: &str| fs::read_to_string(prefix.with_extension(extension)).unwrap();
        let json: serde_json::Value = serde_json::from_str(&read("json")).unwrap();
        assert_eq!(json["testsAnalyzed"], 2);
        assert_eq!(
            json["findings"].as_array().unwrap().len(),
            usize::from(!blocked)
        );
        assert_eq!(json["incomplete"], blocked);
        let expected = if blocked {
            serde_json::json!({"nested": 1})
        } else {
            serde_json::json!({})
        };
        assert_eq!(json["incompleteCauses"]["packageWorkerFailures"], expected);
        if !blocked {
            assert_eq!(json["findings"][0]["rule"], "duplicate-test");
        }
        let records: Vec<serde_json::Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let summary = records
            .iter()
            .find(|record| record["type"] == "summary")
            .unwrap();
        assert_eq!(summary["testsAnalyzed"], 2);
        assert_eq!(summary["unresolvedTests"], if blocked { 2 } else { 0 });
        assert_eq!(
            summary["incompleteCauses"]["packageWorkerFailures"],
            expected
        );
        let sarif: serde_json::Value = serde_json::from_str(&read("sarif")).unwrap();
        assert_eq!(
            sarif["runs"][0]["invocations"][0]["properties"]["incompleteCauses"]
                ["packageWorkerFailures"],
            expected
        );
        let html = read("html");
        let island = html
            .split_once(r#"<script type="application/json" id="report-data">"#)
            .unwrap()
            .1
            .split_once("</script>")
            .unwrap()
            .0;
        let island: serde_json::Value = serde_json::from_str(island).unwrap();
        assert_eq!(
            island["incompleteCauses"]["packageWorkerFailures"],
            expected
        );
        assert_eq!(
            html.contains("1 package scan worker(s) was blocked from starting a nested worker"),
            blocked
        );
    }
    let output = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(fixture)
        .env("VITEST_DEDUP_PACKAGE_WORKER", "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("1 package scan worker(s) was blocked from starting a nested worker"));
}
