//! `--baseline-from-ref` admits base-revision files with the same globs as the working tree.

use assert_cmd::Command;
use std::path::Path;

/// Two tests with identical bodies, so every copy of the file duplicates every other.
const SOURCE: &str = "import { test, expect } from 'vitest';\n\
    test('adds', () => { expect(1 + 1).toBe(2); });\n\
    test('adds again', () => { expect(1 + 1).toBe(2); });\n";

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

/// On an unchanged commit, a baseline taken from that commit accepts every finding the working
/// tree reports and adds none: the base pass read exactly the files the working-tree run did. A
/// `*` that crossed directories in the base pass's exclude, or an unstripped `./`, would leave
/// working-tree findings unbaselined.
#[test]
fn an_unchanged_commit_is_its_own_complete_baseline() {
    for config in [
        r#"{"include":["src/*.test.ts"]}"#,
        r#"{"include":["./src/**/*.test.ts"]}"#,
        r#"{"include":["src/**/*.test.ts"],"exclude":["src/*.test.ts"]}"#,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for name in ["src/a.test.ts", "src/deep/b.test.ts"] {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, SOURCE).unwrap();
        }
        std::fs::write(root.join(".vitest-dedup.json"), config).unwrap();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.email", "test@example.com"]);
        git(root, &["config", "user.name", "Test"]);
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "initial"]);

        let output = tempfile::tempdir().unwrap();
        let prefix = output.path().join("report");
        Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(root)
            .args([
                "--baseline-from-ref",
                "HEAD",
                "--reporters",
                "json",
                "--output",
            ])
            .arg(&prefix)
            .output()
            .unwrap();
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(prefix.with_extension("json")).unwrap())
                .unwrap();
        let findings = report["findings"].as_array().unwrap().len();
        assert!(findings > 0, "{config}: the fixture must produce findings");
        assert_eq!(report["modes"]["baselined"], findings, "{config}");
        assert_eq!(report["modes"]["newFindings"], 0, "{config}");
    }
}
