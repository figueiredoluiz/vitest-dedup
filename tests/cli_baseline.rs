//! CLI baseline output-path and positional-path contracts.

use vitest_dedup::modes::load_baseline;

/// A bare baseline filename uses the caller's working directory without changing global cwd.
#[test]
fn a_bare_baseline_filename_is_written_in_the_cli_working_directory() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("a.test.ts"),
        "import {test, expect} from 'vitest'; test('a', () => expect(1).toBe(1));\n",
    )
    .unwrap();
    let output = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(directory.path())
        .args(["--baseline", "baseline.json", "--update-baseline"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        load_baseline(&directory.path().join("baseline.json"))
            .unwrap()
            .total(),
        0
    );
}

const DUPLICATES: &str = "import {test, expect} from 'vitest';\n\
    test('a', () => { expect(1).toBe(1); });\n\
    test('b', () => { expect(1).toBe(1); });\n";

/// Runs the binary in `cwd` with `args` and returns the JSON report it wrote to `out.json`.
fn run_with_report(cwd: &std::path::Path, args: &[&str]) -> Option<serde_json::Value> {
    let report = cwd.join("out.json");
    let _ = std::fs::remove_file(&report);
    assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .current_dir(cwd)
        .args(["--reporters", "json", "--output", "out"])
        .args(args)
        .output()
        .unwrap();
    let text = std::fs::read_to_string(report).ok()?;
    Some(serde_json::from_str(&text).unwrap())
}

/// The GitHub Action passes its `path` input after `--`, so a flag-shaped path names a directory
/// and cannot rewrite the baseline.
#[test]
fn a_flag_shaped_path_after_the_delimiter_cannot_update_the_baseline() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let target = cwd.join("--update-baseline");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("a.test.ts"), DUPLICATES).unwrap();
    let seed = cwd.join("seed");
    std::fs::create_dir(&seed).unwrap();
    std::fs::write(
        seed.join("a.test.ts"),
        "import {test, expect} from 'vitest'; test('a', () => expect(1).toBe(1));\n",
    )
    .unwrap();
    run_with_report(
        cwd,
        &["--baseline", "base.json", "--update-baseline", "seed"],
    )
    .unwrap();
    let seeded = std::fs::read(cwd.join("base.json")).unwrap();

    let report = run_with_report(cwd, &["--baseline", "base.json", "--", "--update-baseline"])
        .expect("the named directory is analyzed");
    assert_eq!(report["filesAnalyzed"], 1);
    assert!(!report["findings"].as_array().unwrap().is_empty());
    assert_eq!(std::fs::read(cwd.join("base.json")).unwrap(), seeded);

    // Without the delimiter the same word is the flag and rewrites the baseline.
    run_with_report(cwd, &["--baseline", "base.json", "--update-baseline"]);
    assert_ne!(std::fs::read(cwd.join("base.json")).unwrap(), seeded);
}

/// After `--`, `check` is a directory, not the subcommand.
#[test]
fn check_after_the_delimiter_is_a_directory() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    for name in ["check", "other"] {
        std::fs::create_dir(cwd.join(name)).unwrap();
        std::fs::write(cwd.join(name).join("a.test.ts"), DUPLICATES).unwrap();
    }
    let report = run_with_report(cwd, &["--", "check"]).expect("the named directory is analyzed");
    assert_eq!(report["filesAnalyzed"], 1);
}
