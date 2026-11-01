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

/// A run of the analyzer over `directory` under `engine`, with `args`, as `(exit, stdout, stderr)`.
fn analyze(
    directory: &std::path::Path,
    engine: Option<&str>,
    args: &[&str],
) -> (i32, String, String) {
    let mut command = assert_cmd::Command::cargo_bin("vitest-dedup").unwrap();
    command
        .arg(directory)
        .args(args)
        .env_remove("VITEST_DEDUP_ENGINE");
    if let Some(engine) = engine {
        command.env("VITEST_DEDUP_ENGINE", engine);
    }
    let output = command.output().unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// A test titled `title`, whose body all of a file's copies share.
fn copy(title: &str) -> String {
    format!("test('{title}', () => {{ expect(1).toBe(1); expect(2).toBe(2); }});\n")
}

/// The source of a test file holding `tests`.
fn source(tests: &[&str]) -> String {
    let body: String = tests.iter().map(|title| copy(title)).collect();
    format!("import {{ test, expect }} from 'vitest';\n{body}")
}

/// The duplicate-test records of a JSONL report, and how many of them are baselined.
fn duplicates(stdout: &str) -> (usize, usize) {
    let records: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .filter(|record: &serde_json::Value| record["rule"] == "duplicate-test")
        .collect();
    let baselined = records.iter().filter(|record| record["baselined"] == true);
    (records.len(), baselined.count())
}

/// Recorded findings stop counting under the next engine, a new one still does, and each carries
/// the next engine's identity.
#[test]
fn the_next_engine_records_and_applies_a_baseline() {
    let directory = tempfile::tempdir().unwrap();
    let test_file = directory.path().join("a.test.ts");
    std::fs::write(&test_file, source(&["one", "two"])).unwrap();
    let path = directory.path().join("baseline.json");
    let path = path.to_str().unwrap();
    let args = ["--reporters", "jsonl", "--baseline", path];
    let (exit, _, stderr) = analyze(
        directory.path(),
        Some("next"),
        &[&args[..], &["--update-baseline"]].concat(),
    );
    assert_eq!(exit, 0, "{stderr}");
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(written["fingerprintVersion"], "vitestDedupFingerprint/v2");
    let (exit, stdout, stderr) = analyze(directory.path(), Some("next"), &args);
    assert_eq!(exit, 0, "{stderr}");
    assert_eq!(duplicates(&stdout), (1, 1));
    assert!(stdout.contains("vitestDedupFingerprint/v2:"), "{stdout}");
    std::fs::write(&test_file, source(&["one", "two", "three"])).unwrap();
    let (exit, stdout, _) = analyze(directory.path(), Some("next"), &args);
    assert_eq!(exit, 1);
    assert_eq!(duplicates(&stdout), (2, 1));
}

/// Equal fingerprints are counted: accepting one of two copies still reports the other.
#[test]
fn a_collision_under_the_next_engine_is_counted() {
    let directory = tempfile::tempdir().unwrap();
    let test_file = directory.path().join("a.test.ts");
    std::fs::write(&test_file, source(&["same", "same"])).unwrap();
    let path = directory.path().join("baseline.json");
    let path = path.to_str().unwrap();
    let args = ["--reporters", "jsonl", "--baseline", path];
    analyze(
        directory.path(),
        Some("next"),
        &[&args[..], &["--update-baseline"]].concat(),
    );
    std::fs::write(&test_file, source(&["same", "same", "same"])).unwrap();
    let (exit, stdout, _) = analyze(directory.path(), Some("next"), &args);
    assert_eq!(exit, 1);
    assert_eq!(duplicates(&stdout), (2, 1));
}

/// Each engine refuses the other's file and names the way out, which also works: a half-matching
/// baseline would read as unexplained new findings.
#[test]
fn each_engine_refuses_the_other_engines_baseline() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("a.test.ts"), source(&["one", "two"])).unwrap();
    let path = directory.path().join("baseline.json");
    let path = path.to_str().unwrap();
    let args = ["--baseline", path];
    let update = [&args[..], &["--update-baseline"]].concat();
    for (writer, reader, written, wanted) in [
        (None, Some("next"), "v1", "v2"),
        (Some("next"), None, "v2", "v1"),
    ] {
        assert_eq!(analyze(directory.path(), writer, &update).0, 0);
        let (exit, _, stderr) = analyze(directory.path(), reader, &args);
        assert_eq!(exit, 2, "{stderr}");
        assert!(
            stderr.contains(&format!("vitestDedupFingerprint/{written}")),
            "{stderr}"
        );
        assert!(
            stderr.contains(&format!("vitestDedupFingerprint/{wanted}")),
            "{stderr}"
        );
        assert!(stderr.contains("--update-baseline"), "{stderr}");
        assert_eq!(analyze(directory.path(), reader, &update).0, 0);
        assert_eq!(analyze(directory.path(), reader, &args).0, 0);
        assert_eq!(analyze(directory.path(), writer, &update).0, 0);
    }
}

/// A file whose header claims the next engine's identity must hold only its entries, and malformed
/// input stays a refusal, never a pass.
#[test]
fn the_next_engine_refuses_a_malformed_baseline() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("a.test.ts"), source(&["one", "two"])).unwrap();
    let path = directory.path().join("baseline.json");
    let file = |fingerprint: &str, count: u32| {
        format!(
            r#"{{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v2","findings":[{{"fingerprint":"{fingerprint}","count":{count},"rule":"r","tests":[]}}]}}"#
        )
    };
    for (body, reason) in [
        (
            file("vitestDedupFingerprint/v1:00", 1),
            "not a `vitestDedupFingerprint/v2`",
        ),
        (
            file("vitestDedupFingerprint/v2:00", 0),
            "accepts 0 occurrences",
        ),
        ("{".to_owned(), "failed to parse"),
    ] {
        std::fs::write(&path, body).unwrap();
        let (exit, _, stderr) = analyze(
            directory.path(),
            Some("next"),
            &["--baseline", path.to_str().unwrap()],
        );
        assert_eq!(exit, 2, "{stderr}");
        assert!(stderr.contains(reason), "{stderr}");
    }
}

/// A mode that needs a base revision stays refused.
#[test]
fn the_next_engine_still_refuses_a_base_revision() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("a.test.ts"), source(&["one"])).unwrap();
    for flag in ["--baseline-from-ref", "--changed-since"] {
        let (exit, _, stderr) = analyze(directory.path(), Some("next"), &[flag, "HEAD"]);
        assert_eq!(exit, 2, "{stderr}");
        assert!(stderr.contains("reads no base revision"), "{stderr}");
    }
}
