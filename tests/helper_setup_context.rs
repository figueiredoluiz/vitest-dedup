//! A setup call changes what every test in its file runs under, so files that set up differently
//! must never compare equal, and files that set up the same way must.
//!
//! Final-outcome contract over projects built at run time: two files with identical tests, each
//! calling an imported helper that registers nothing.

use std::fs;
use std::path::Path;

const TESTS: &str = "\
import { test, expect } from 'vitest'
import { setup } from '%HELPER%'

setup(%ARGUMENT%)

test('doubles', () => { expect([1, 2, 3].map((n) => n * 2)).toEqual([2, 4, 6]) })
";

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn test_file(helper: &str, argument: &str) -> String {
    TESTS
        .replace("%HELPER%", helper)
        .replace("%ARGUMENT%", argument)
}

/// The rules of the findings a run reports.
fn rules(root: &Path) -> Vec<String> {
    let stem = root.join("out/report");
    let output = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .current_dir(root)
        .arg(root)
        .args(["--reporters", "json", "--output"])
        .arg(&stem)
        .output()
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(stem.with_extension("json")).unwrap())
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(json["incomplete"], false, "{json:?}");
    assert_eq!(json["testsAnalyzed"], 2, "{json:?}");
    json["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["rule"].as_str().unwrap().to_owned())
        .collect()
}

/// Two files calling the same helper the same way are the same test under the same setup.
#[test]
fn the_same_setup_compares_equal() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "shared/setup.ts",
        "export function setup(mode?: string) { return mode }\n",
    );
    write(
        root,
        "a/doubles.test.ts",
        &test_file("../shared/setup", "'fast'"),
    );
    write(
        root,
        "b/doubles.test.ts",
        &test_file("../shared/setup", "'fast'"),
    );
    assert_eq!(rules(root), ["cross-file-duplicate"]);
}

/// A different argument is a different setup.
#[test]
fn a_different_argument_is_a_different_setup() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "shared/setup.ts",
        "export function setup(mode?: string) { return mode }\n",
    );
    write(
        root,
        "a/doubles.test.ts",
        &test_file("../shared/setup", "'fast'"),
    );
    write(
        root,
        "b/doubles.test.ts",
        &test_file("../shared/setup", "'slow'"),
    );
    assert!(rules(root).is_empty());
}

/// The same call over a different helper module is a different setup.
#[test]
fn a_different_helper_is_a_different_setup() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "a/setup.ts",
        "export function setup(mode?: string) { return mode }\n",
    );
    write(
        root,
        "b/setup.ts",
        "export function setup(mode?: string) { return [mode] }\n",
    );
    write(root, "a/doubles.test.ts", &test_file("./setup", "'fast'"));
    write(root, "b/doubles.test.ts", &test_file("./setup", "'fast'"));
    assert!(rules(root).is_empty());
}
