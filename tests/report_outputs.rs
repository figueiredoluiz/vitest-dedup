//! Atomic publication contracts for CLI file reports and baselines.

use vitest_dedup::modes;

const DUPLICATE_PAIR: &str = r#"
    import { test, expect } from 'vitest';

    test('adds numbers', () => {
        const total = 1 + 1;
        expect(total).toBe(2);
    });

    test('adds numbers, again', () => {
        const total = 1 + 1;
        expect(total).toBe(2);
    });
"#;

/// Existing reports are replaced, nested parents are created, and bare stems remain valid.
#[test]
fn file_reports_replace_existing_files_and_accept_bare_relative_stems() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(root.join("math.test.ts"), DUPLICATE_PAIR).unwrap();
    for stem in ["report", "nested/reports/report"] {
        for repeat in 0..2 {
            let output = assert_cmd::Command::cargo_bin("vitest-dedup")
                .unwrap()
                .current_dir(root)
                .arg(root)
                .args(["--reporters", "json,html,sarif", "--output", stem])
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(1),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let prefix = root.join(stem);
            let report = read_json_report(&prefix);
            assert_eq!(report["testsAnalyzed"], 2);
            assert_eq!(report["findings"].as_array().unwrap().len(), 1);
            assert!(prefix.with_extension("html").is_file());
            let sarif: serde_json::Value =
                serde_json::from_slice(&std::fs::read(prefix.with_extension("sarif")).unwrap())
                    .unwrap();
            assert_eq!(sarif["runs"][0]["results"].as_array().unwrap().len(), 1);
            if repeat == 0 {
                for extension in ["json", "html", "sarif"] {
                    std::fs::write(prefix.with_extension(extension), "previous report").unwrap();
                }
            }
        }
    }
}

/// Every file reporter and baseline updates replace final symlinks without touching victims.
#[cfg(unix)]
#[test]
fn file_outputs_replace_symlinks_without_following_them() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(root.join("math.test.ts"), DUPLICATE_PAIR).unwrap();
    for kind in ["json", "html", "sarif", "baseline"] {
        for dangling in [false, true] {
            let victim = root.join(format!("{kind}-{dangling}-victim"));
            if !dangling {
                std::fs::write(&victim, "ORIGINAL").unwrap();
            }
            let prefix = root.join(format!("{kind}-{dangling}"));
            let path = prefix.with_extension("json");
            let path = if kind == "baseline" {
                path
            } else {
                prefix.with_extension(kind)
            };
            std::os::unix::fs::symlink(&victim, &path).unwrap();
            let (code, _, stderr) = if kind == "baseline" {
                run_cli(
                    root,
                    &["--baseline", path.to_str().unwrap(), "--update-baseline"],
                )
            } else {
                run_cli(
                    root,
                    &["--reporters", kind, "--output", prefix.to_str().unwrap()],
                )
            };
            assert_eq!(
                code,
                Some(if kind == "baseline" { 0 } else { 1 }),
                "{kind}: {stderr}"
            );
            assert!(
                !path.symlink_metadata().unwrap().file_type().is_symlink(),
                "{kind}"
            );
            if dangling {
                assert!(!victim.exists(), "{kind}");
            } else {
                assert_eq!(
                    std::fs::read_to_string(&victim).unwrap(),
                    "ORIGINAL",
                    "{kind}"
                );
            }
            let body = std::fs::read_to_string(&path).unwrap();
            match kind {
                "json" => {
                    let report: serde_json::Value = serde_json::from_str(&body).unwrap();
                    assert_eq!(report["testsAnalyzed"], 2);
                    assert_eq!(report["findings"].as_array().unwrap().len(), 1);
                }
                "baseline" => assert_eq!(modes::load_baseline(&path).unwrap().total(), 1),
                "sarif" => {
                    let report: serde_json::Value = serde_json::from_str(&body).unwrap();
                    assert_eq!(report["runs"][0]["results"].as_array().unwrap().len(), 1);
                }
                "html" => assert!(body.to_ascii_lowercase().contains("<!doctype html>")),
                _ => unreachable!(),
            }
        }
    }
}

/// Runs the CLI against an owned analysis root and returns its operational outcome.
fn run_cli(root: &std::path::Path, args: &[&str]) -> (Option<i32>, String, String) {
    let output = assert_cmd::Command::cargo_bin("vitest-dedup")
        .expect("binary")
        .arg(root)
        .args(args)
        .output()
        .expect("run");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Reads the structured report emitted for a filename stem.
fn read_json_report(prefix: &std::path::Path) -> serde_json::Value {
    let path = prefix.with_extension("json");
    serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display())),
    )
    .expect("the JSON reporter emits valid JSON")
}
