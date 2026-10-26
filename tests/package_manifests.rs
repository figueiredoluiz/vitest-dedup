//! Package metadata admission must preserve ordinary discovery and make required refusals visible.

use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Copies the owned package fixture used by the runtime oracle and permanent recall gate.
fn project() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/recall/framework-package-manifest-regular");
    for name in [
        "metadata.test.ts",
        "node_modules/@owned/metadata/package.json",
        "node_modules/@owned/metadata/index.js",
    ] {
        let target = directory.path().join(name);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(fixture.join(name), target).unwrap();
    }
    directory
}

/// Runs both machine reporters with a deadline so a FIFO regression cannot hang the test suite.
fn analyze(root: &Path) -> (i32, Value, Value) {
    let output = tempfile::tempdir().unwrap();
    let prefix = output.path().join("result");
    let result = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(root)
        .args([
            "--reporters",
            "json,jsonl",
            "--no-metrics",
            "--fail-on-incomplete=reads",
            "--output",
        ])
        .arg(&prefix)
        .timeout(Duration::from_secs(10))
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(
        &fs::read(prefix.with_extension("json"))
            .expect("bounded scan must finish and write its report"),
    )
    .unwrap();
    let summary = String::from_utf8(result.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|row| row["type"] == "summary")
        .unwrap();
    (
        result.status.code().expect("bounded scan must not abort"),
        json,
        summary,
    )
}

/// Asserts extraction, comparison eligibility and retained findings in both report surfaces.
fn outcome(root: &Path, tests: u64, unresolved: u64, findings: usize) -> Value {
    let (code, json, summary) = analyze(root);
    assert_eq!(json["testsAnalyzed"], tests);
    assert_eq!(summary["unresolvedTests"], unresolved, "{summary}");
    assert_eq!(
        json["findings"].as_array().unwrap().len(),
        findings,
        "{json}"
    );
    assert_eq!(json["incomplete"], unresolved > 0);
    assert_eq!(summary["incomplete"], unresolved > 0);
    assert_eq!(
        code,
        if unresolved > 0 {
            2
        } else if json["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["severity"] == "error")
        {
            1
        } else {
            0
        }
    );
    json
}

/// The owned installed manifest's location, shared by the boundary variants.
fn manifest(root: &Path) -> PathBuf {
    root.join("node_modules/@owned/metadata/package.json")
}

/// Normal packages preserve duplicates, distinguish conflicts and retain unrelated-file findings.
#[test]
fn ordinary_packages_and_independent_tests_remain_discoverable() {
    let directory = project();
    let root = directory.path();
    outcome(root, 2, 0, 1);
    let source = fs::read_to_string(root.join("metadata.test.ts")).unwrap();
    fs::write(
        root.join("metadata.test.ts"),
        source.replace(
            "second observation', () => expect(2 + 2).toBe(4)",
            "second observation', () => expect(2 + 2).toBe(5)",
        ),
    )
    .unwrap();
    let different = outcome(root, 2, 0, 1);
    assert_eq!(
        different["findings"][0]["rule"],
        "parameterization-candidate"
    );
    fs::write(root.join("metadata.test.ts"), source).unwrap();
    fs::write(root.join("independent.test.ts"), "import { test, expect } from 'vitest';\ntest('one', () => expect(3).toBe(3));\ntest('two', () => expect(3).toBe(3));\n").unwrap();
    fs::write(manifest(root), " ".repeat(1024 * 1024 + 1)).unwrap();
    outcome(root, 4, 2, 1);
}

/// An over-limit or special manifest cannot silently route through the package's index.
#[test]
fn required_refused_metadata_is_visible_and_has_no_index_fallback() {
    let directory = project();
    let root = directory.path();
    let path = manifest(root);
    let normal = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        format!("{normal}{}", " ".repeat(1024 * 1024 - normal.len())),
    )
    .unwrap();
    outcome(root, 2, 0, 1);
    for contents in [
        format!("{normal}{}", " ".repeat(1024 * 1024 + 1 - normal.len())).into_bytes(),
        vec![0xff],
    ] {
        fs::write(&path, contents).unwrap();
        let json = outcome(root, 2, 2, 0);
        let excluded = &json["excludedFiles"][0];
        assert!(
            excluded.to_string().contains("package-manifest-unreadable"),
            "{excluded}"
        );
    }
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    outcome(root, 2, 2, 0);
}

/// POSIX special files terminate promptly both during resolution and prerequisite assessment.
#[cfg(unix)]
#[test]
fn fifo_and_device_manifests_do_not_block_either_reader_path() {
    use std::os::unix::fs::symlink;
    for device in [false, true] {
        let directory = project();
        let root = directory.path();
        let path = manifest(root);
        fs::remove_file(&path).unwrap();
        if device {
            symlink("/dev/zero", &path).unwrap();
        } else {
            assert!(std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success());
        }
        outcome(root, 2, 2, 0);
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(
        root.join("plain.test.ts"),
        "test('one', () => {}); test('two', () => {});",
    )
    .unwrap();
    assert!(std::process::Command::new("mkfifo")
        .arg(root.join("package.json"))
        .status()
        .unwrap()
        .success());
    let output = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(root)
        .args(["--globals", "--no-metrics"])
        .timeout(Duration::from_secs(10))
        .output()
        .unwrap();
    assert!(
        output.status.code().is_some(),
        "prerequisite reader timed out"
    );
}
