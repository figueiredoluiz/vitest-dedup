//! Generated table admission must preserve useful bindings and report dependent proof failures.

use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::Duration;

const VALID_TABLE: &str = "export { useFixtureClock } from '../composables/use-fixture-clock';\n";

/// Copies the owned flat-table fixture, using either supported lookup route.
fn project(named_import: bool) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/recall/framework-auto-import-flat-table");
    for relative in [
        ".nuxt/imports.d.ts",
        "composables/use-fixture-clock.ts",
        "setup.ts",
        "clock.test.ts",
    ] {
        let destination = root.join(relative);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(fixture.join(relative), destination).unwrap();
    }
    if named_import {
        let setup = fs::read_to_string(root.join("setup.ts")).unwrap();
        fs::write(
            root.join("setup.ts"),
            format!("import {{ useFixtureClock }} from '#imports';\n{setup}"),
        )
        .unwrap();
    }
    directory
}

/// Runs the CLI under a deadline and returns both report formats and the process exit code.
fn analyze(root: &Path, gate: Option<&str>) -> (i32, Value, Value) {
    let output_dir = tempfile::tempdir().unwrap();
    let prefix = output_dir.path().join("report");
    let mut command = Command::cargo_bin("vitest-dedup").unwrap();
    command
        .arg(root)
        .args(["--reporters", "json,jsonl", "--no-metrics", "--output"])
        .arg(&prefix)
        .timeout(Duration::from_secs(30));
    if let Some(gate) = gate {
        command.arg(format!("--fail-on-incomplete={gate}"));
    }
    let output = command.output().unwrap();
    let code = output.status.code().expect("analyzer must not abort");
    let json =
        serde_json::from_str(&fs::read_to_string(prefix.with_extension("json")).unwrap()).unwrap();
    let records: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let summary = records
        .into_iter()
        .find(|record| record["type"] == "summary")
        .expect("every run must report its coverage");
    (code, json, summary)
}

/// Pins extraction, eligibility and findings before checking detailed report contents.
fn assert_outcome(json: &Value, summary: &Value, tests: u64, unresolved: u64, findings: usize) {
    assert_eq!(json["testsAnalyzed"], tests, "{json}");
    assert_eq!(summary["unresolvedTests"], unresolved, "{summary}");
    assert_eq!(json["incomplete"], unresolved != 0, "{json}");
    assert_eq!(summary["incomplete"], unresolved != 0, "{summary}");
    let actual = json["findings"].as_array().unwrap();
    assert_eq!(actual.len(), findings, "{actual:?}");
    assert!(actual
        .iter()
        .all(|finding| finding["rule"] == "duplicate-test"));
}

/// Thousands of flat exports retain the referenced final binding, without widening matching.
#[test]
fn large_flat_tables_retain_equivalent_and_distinct_pairs() {
    for named_import in [false, true] {
        let directory = project(named_import);
        let root = directory.path();
        let (code, json, summary) = analyze(root, Some("reads"));
        assert_eq!(code, 1);
        assert_outcome(&json, &summary, 2, 0, 1);
        fs::write(
            root.join(".nuxt/imports.d.ts"),
            format!(
                "export const unused = {}0{};\n{VALID_TABLE}",
                "(".repeat(256),
                ")".repeat(256)
            ),
        )
        .unwrap();
        let (_, json, summary) = analyze(root, None);
        assert_outcome(&json, &summary, 2, 0, 1);
        let test = root.join("clock.test.ts");
        let source = fs::read_to_string(&test).unwrap();
        fs::write(
            &test,
            source.replacen(
                "expect([3, 1, 2].sort()).toEqual([1, 2, 3])",
                "expect(Math.round(1.2)).toBe(1)",
                1,
            ),
        )
        .unwrap();
        let (code, json, summary) = analyze(root, None);
        assert_eq!(code, 0);
        assert_outcome(&json, &summary, 2, 0, 0);
    }
}

/// Every admission/read failure reaches the same conservative outcome on both lookup routes.
#[test]
fn refused_tables_exclude_dependent_tests_and_reach_exit_gates() {
    let depth_fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/recall/framework-auto-import-table-depth/.nuxt/imports.d.ts");
    let cases = [
        fs::read(depth_fixture).unwrap(),
        format!("/* {} */\n{VALID_TABLE}", "!".repeat(36_865)).into_bytes(),
        format!("{VALID_TABLE}{}", " ".repeat(8 * 1024 * 1024)).into_bytes(),
        b"export { from ;;; {{{".to_vec(),
        vec![0xff],
        b"export { test as useFixtureClock } from 'vitest';".to_vec(),
        b"export type { useFixtureClock } from '../composables/use-fixture-clock';".to_vec(),
    ];
    for named_import in [false, true] {
        let directory = project(named_import);
        let root = directory.path();
        let table = root.join(".nuxt/imports.d.ts");
        for (case, source) in cases.iter().enumerate() {
            fs::write(&table, source).unwrap();
            let (code, json, summary) = analyze(root, Some("reads"));
            assert_eq!(code, 2);
            assert_outcome(&json, &summary, 2, 2, 0);
            let reason = if named_import {
                "helper-setup-effects"
            } else {
                "helper-effects-unproved"
            };
            assert_eq!(
                json["excludedFiles"][0]["reason"], reason,
                "named import: {named_import}, case: {case}, report: {json}"
            );
            assert_eq!(json["exclusions"], summary["exclusions"]);
        }
        fs::remove_file(table).unwrap();
        let (code, json, summary) = analyze(root, Some("any"));
        assert_eq!(code, 2);
        assert_outcome(&json, &summary, 2, 2, 0);
    }
}

/// A failed nearest table never borrows bindings from a valid ancestor or taints unrelated tests.
#[test]
fn nearest_refusal_preserves_independent_findings() {
    let directory = project(false);
    let root = directory.path();
    let nested = root.join("nested");
    fs::create_dir_all(nested.join(".nuxt")).unwrap();
    fs::rename(root.join("clock.test.ts"), nested.join("clock.test.ts")).unwrap();
    fs::rename(root.join("setup.ts"), nested.join("setup.ts")).unwrap();
    fs::write(nested.join(".nuxt/imports.d.ts"), "export { from ;;; {{{").unwrap();
    fs::write(
        nested.join("independent.test.ts"),
        "import { expect, test } from 'vitest';\n\
         test('independent a', () => { expect(10 + 20).toBe(30) });\n\
         test('independent b', () => { expect(10 + 20).toBe(30) });\n",
    )
    .unwrap();
    let (code, json, summary) = analyze(root, None);
    assert_eq!(code, 1);
    assert_outcome(&json, &summary, 4, 2, 1);
    fs::remove_file(nested.join("clock.test.ts")).unwrap();
    let (code, json, summary) = analyze(root, Some("reads"));
    assert_eq!(code, 1);
    assert_outcome(&json, &summary, 2, 0, 1);
}
