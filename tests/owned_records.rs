//! End-to-end owned diagnostic records and their independence from findings.
use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use tempfile::tempdir;

/// Run the real executable with the owned engine, returning every typed JSONL record.
fn records(source: &str, fingerprints: bool) -> Vec<Value> {
    records_for_files(&[("case.test.ts", source)], fingerprints)
}

/// Exercise caller and declaration files in one owned analysis root.
fn records_for_files(files: &[(&str, &str)], fingerprints: bool) -> Vec<Value> {
    let dir = tempdir().unwrap();
    for (path, source) in files {
        fs::write(dir.path().join(path), source).unwrap();
    }
    let mut cmd = Command::cargo_bin("vitest-dedup").unwrap();
    cmd.arg(dir.path())
        .args(["--reporters", "jsonl", "--no-metrics"]);
    if fingerprints {
        cmd.arg("--emit-test-fingerprints");
    }
    let output = cmd.output().unwrap();
    assert!(matches!(output.status.code(), Some(0..=2)));
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// A lone hook is a declaration even though the redundancy planner needs at least two.
#[test]
fn owned_records_enumerate_tests_and_single_hooks_without_changing_findings() {
    let source = "import { beforeEach, test, expect } from 'vitest';\nbeforeEach(() => reset());\ntest('a', () => expect(work(1)).toBe(2));\ntest('b', () => expect(work(1)).toBe(2));\n";
    let plain = records(source, false);
    let all = records(source, true);
    assert_eq!(all.last().unwrap()["testsAnalyzed"], 2);
    assert_eq!(all.iter().filter(|r| r["type"] == "test").count(), 2);
    assert_eq!(all.iter().filter(|r| r["type"] == "hook").count(), 1);
    assert_eq!(all.iter().filter(|r| r["type"] == "site").count(), 3);
    assert_eq!(all.iter().filter(|r| r["type"] == "finding").count(), 1);
    let tests: Vec<_> = all.iter().filter(|r| r["type"] == "test").collect();
    assert_eq!(tests[0]["schemaVersion"], "0.2");
    assert_eq!(tests[0]["engine"], "owned");
    assert_eq!(
        tests[0]["comparisonKeys"]["duplicate"],
        tests[1]["comparisonKeys"]["duplicate"]
    );
    assert!(tests[0]["views"]["body"].is_string());
    assert!(tests[0].get("alpha").is_none());
    let stripped: Vec<_> = all
        .into_iter()
        .filter(|r| !matches!(r["type"].as_str(), Some("test" | "hook" | "site")))
        .collect();
    assert_eq!(plain, stripped);
}

/// An exclusion supplies no equality proof; zero hashes must not launder refusal into equality.
#[test]
fn owned_records_preserve_exclusions_without_fake_keys() {
    let all = records(
        "import { test } from 'vitest';\ntest('empty', () => {});\ntest('todo');\n",
        true,
    );
    assert_eq!(all.last().unwrap()["testsAnalyzed"], 2);
    let tests: Vec<_> = all.iter().filter(|r| r["type"] == "test").collect();
    assert_eq!(tests.len(), 2);
    for test in tests {
        assert_eq!(test["comparable"], false);
        assert!(test["exclusion"].is_string());
        assert!(test["comparisonKeys"]["duplicate"].is_null());
        assert!(test["views"].is_null());
    }
}

/// Separate context, modifier and body dimensions remain independently inspectable.
#[test]
fn owned_records_expose_independent_body_context_and_modifier_evidence() {
    let all = records("import { test, describe, beforeEach, expect } from 'vitest';\ntest('plain', () => expect(work(1)).toBe(2));\ntest.fails('fails', () => expect(work(1)).toBe(2));\ndescribe('suite', () => { beforeEach(() => reset()); test('context', () => expect(work(1)).toBe(2)); });\n", true);
    assert_eq!(all.last().unwrap()["testsAnalyzed"], 3);
    let tests: Vec<_> = all.iter().filter(|r| r["type"] == "test").collect();
    assert_eq!(tests.len(), 3);
    assert_eq!(tests[0]["views"]["body"], tests[1]["views"]["body"]);
    assert_eq!(tests[0]["views"]["context"], tests[1]["views"]["context"]);
    assert_ne!(
        tests[0]["views"]["modifiers"],
        tests[1]["views"]["modifiers"]
    );
    assert_eq!(tests[0]["views"]["body"], tests[2]["views"]["body"]);
    assert_ne!(tests[0]["views"]["context"], tests[2]["views"]["context"]);
    assert_ne!(
        tests[0]["comparisonKeys"]["duplicate"],
        tests[2]["comparisonKeys"]["duplicate"]
    );
}

/// Expanded rows share a declaration and site while retaining separate execution records.
#[test]
fn owned_table_rows_keep_execution_counts_separate_from_site_counts() {
    let all = records("import { test, expect } from 'vitest';\ntest.each([1, 1])('row', value => expect(value).toBe(1));\n", true);
    assert_eq!(all.last().unwrap()["testsAnalyzed"], 2);
    let tests: Vec<_> = all.iter().filter(|r| r["type"] == "test").collect();
    assert_eq!(tests.len(), 2);
    assert_eq!(all.iter().filter(|r| r["type"] == "site").count(), 1);
    assert_eq!(tests[0]["row"], 0);
    assert_eq!(tests[1]["row"], 1);
    assert_eq!(tests[0]["line"], tests[1]["line"]);
    assert_eq!(
        tests[0]["comparisonKeys"]["rowClass"],
        tests[1]["comparisonKeys"]["rowClass"]
    );
    assert!(all
        .iter()
        .any(|r| r["type"] == "finding" && r["rule"] == "duplicate-each-row"));
}

/// Imported executions retain their shared declaration and distinct caller registrations.
#[test]
fn owned_runner_records_preserve_declaration_and_registration_ownership() {
    let files = [
        ("case.test.ts", "import { test, expect } from 'vitest';\nimport { register } from './runner';\nregister('a'); register('b');\ntest('plain-a', () => expect(work(1)).toBe(2));\ntest('plain-b', () => expect(work(1)).toBe(2));\n"),
        ("runner.ts", "import { test, expect } from 'vitest';\nexport function register(label) { test(label, () => expect(work(2)).toBe(3)); }\n"),
    ];
    let all = records_for_files(&files, true);
    assert_eq!(all.last().unwrap()["testsAnalyzed"], 4);
    let tests: Vec<_> = all.iter().filter(|r| r["type"] == "test").collect();
    assert_eq!(tests.len(), 4);
    let imported: Vec<_> = tests.iter().filter(|r| r["path"] == "runner.ts").collect();
    assert_eq!(imported.len(), 2);
    assert_eq!(imported[0]["registrationFile"], "case.test.ts");
    assert_eq!(imported[1]["registrationFile"], "case.test.ts");
    assert_ne!(imported[0]["title"], imported[1]["title"]);
    assert_eq!(imported[0]["line"], imported[1]["line"]);
    assert!(all
        .iter()
        .any(|r| r["type"] == "finding" && r["rule"] == "duplicate-test"));
    let ordinary: Vec<_> = all
        .into_iter()
        .filter(|r| !matches!(r["type"].as_str(), Some("test" | "hook" | "site")))
        .collect();
    assert_eq!(ordinary, records_for_files(&files, false));
}

/// Cross-file ambient evidence includes the module-load context used by the owned rules.
#[test]
fn owned_context_views_include_module_load_effects() {
    let files = [
        ("a.test.ts", "import { test, expect, vi } from 'vitest'; vi.mock('./dep', () => ({ limit: 1 })); test('a', () => expect(work(1)).toBe(2)); test('a twin', () => expect(work(1)).toBe(2));"),
        ("b.test.ts", "import { test, expect, vi } from 'vitest'; vi.mock('./dep', () => ({ limit: 2 })); test('b', () => expect(work(1)).toBe(2));"),
    ];
    let all = records_for_files(&files, true);
    assert_eq!(all.last().unwrap()["testsAnalyzed"], 3);
    let tests: Vec<_> = all.iter().filter(|r| r["type"] == "test").collect();
    assert_eq!(tests.len(), 3);
    let a = tests.iter().find(|r| r["title"] == "a").unwrap();
    let b = tests.iter().find(|r| r["title"] == "b").unwrap();
    assert_eq!(a["views"]["body"], b["views"]["body"]);
    assert_ne!(
        a["comparisonKeys"]["crossFile"],
        b["comparisonKeys"]["crossFile"]
    );
    assert_ne!(a["views"]["context"], b["views"]["context"]);
    assert!(all
        .iter()
        .any(|r| r["type"] == "finding" && r["rule"] == "duplicate-test"));
    assert!(!all
        .iter()
        .any(|r| r["type"] == "finding" && r["rule"] == "cross-file-duplicate"));
}

/// The permanent accumulator corpus retains display identity even when its tests are excluded.
#[test]
fn excluded_records_preserve_known_suite_titles_and_positive_controls() {
    let files = [
        (
            "accumulator.test.ts",
            include_str!("../fixtures/duplicates/not-redundant-hook-accumulator.test.ts"),
        ),
        (
            "safe.test.ts",
            include_str!("../fixtures/recall/runner-unproved-scope-retention/safe.test.ts"),
        ),
    ];
    let all = records_for_files(&files, true);
    assert_eq!(all.last().unwrap()["testsAnalyzed"], 4);
    let tests: Vec<_> = all.iter().filter(|r| r["type"] == "test").collect();
    assert_eq!(tests.len(), 4);
    let excluded: Vec<_> = tests
        .iter()
        .filter(|r| r["path"] == "accumulator.test.ts")
        .collect();
    assert_eq!(excluded.len(), 2);
    assert!(excluded
        .iter()
        .any(|r| r["title"] == "nested > accumulates the nested hook too"));
    assert!(excluded
        .iter()
        .all(|r| r["comparable"] == false && r["comparisonKeys"]["duplicate"].is_null()));
    assert_eq!(tests.iter().filter(|r| r["comparable"] == true).count(), 2);
    assert!(all
        .iter()
        .any(|r| r["type"] == "finding" && r["rule"] == "duplicate-test"));
}

/// Early refusal siblings retain known nested titles and row rendering without name proof.
#[test]
fn excluded_records_keep_nested_scalar_and_row_display_titles() {
    let all = records("import { describe, test, expect } from 'vitest'; const shared = () => expect(1).toBe(1); describe('outer', () => { describe('inner', () => { test('empty', () => {}); test('todo'); test('named', shared); test.each([1, 2])('row %s', () => {}); }); });", true);
    assert_eq!(all.last().unwrap()["testsAnalyzed"], 5);
    let tests: Vec<_> = all.iter().filter(|r| r["type"] == "test").collect();
    assert_eq!(tests.len(), 5);
    for title in [
        "outer > inner > empty",
        "outer > inner > todo",
        "outer > inner > named",
        "outer > inner > row 1",
        "outer > inner > row 2",
    ] {
        let record = tests
            .iter()
            .find(|r| r["title"] == title)
            .unwrap_or_else(|| panic!("missing {title}: {tests:?}"));
        assert_eq!(record["comparable"], false);
        assert!(record["comparisonKeys"]["name"].is_null());
        assert!(record["comparisonKeys"]["duplicate"].is_null());
    }
}

/// An unknown suite prefix cannot become a fabricated qualified title on an excluded case.
#[test]
fn excluded_records_do_not_invent_an_unknown_suite_title() {
    let all = records("import { describe, test } from 'vitest'; describe(dynamicTitle, () => { test('empty', () => {}); });", true);
    assert_eq!(all.last().unwrap()["testsAnalyzed"], 1);
    let tests: Vec<_> = all.iter().filter(|r| r["type"] == "test").collect();
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0]["title"], "empty");
    assert_eq!(tests[0]["comparable"], false);
    assert!(tests[0]["comparisonKeys"]["name"].is_null());
}
