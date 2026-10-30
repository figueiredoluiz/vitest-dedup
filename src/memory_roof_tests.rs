//! Deterministic end-to-end memory admission controls, independent of host process residency.

use super::*;
use std::ffi::OsString;
use std::path::Path;

/// Run the real pipeline while controlling only the OS residency reading.
fn analyze(
    root: &Path,
    reporters: &str,
    flags: &[&str],
    resident: impl FnMut() -> Option<u64>,
) -> (ExitCode, serde_json::Value) {
    analyze_on(root, reporters, flags, resident, None)
}

/// [`analyze`] on the engine given: the next engine, or `None` for the current core.
fn analyze_on(
    root: &Path,
    reporters: &str,
    flags: &[&str],
    resident: impl FnMut() -> Option<u64>,
    engine: Option<vitest_dedup::corpus::Engine>,
) -> (ExitCode, serde_json::Value) {
    let output = tempfile::tempdir().unwrap();
    let prefix = output.path().join("report");
    let mut args = vec![
        OsString::from("vitest-dedup"),
        root.as_os_str().to_owned(),
        OsString::from("--reporters"),
        OsString::from(reporters),
        OsString::from("--no-metrics"),
        OsString::from("--output"),
        prefix.as_os_str().to_owned(),
    ];
    args.extend(flags.iter().map(OsString::from));
    let code = run_with_engine(ParsedArgs::parse_from(args).unwrap(), resident, engine).unwrap();
    let report = std::fs::read_to_string(prefix.with_extension("json")).unwrap();
    (code, serde_json::from_str(&report).unwrap())
}

/// Write one owned nontrivial duplicate-retention test per file in discovery order.
fn write_tree(root: &Path, files: usize) {
    for file in 0..files {
        std::fs::write(
            root.join(format!("{file}.test.ts")),
            format!("import {{test,expect}} from 'vitest'; test('case {file}',()=>{{expect(1).toBe(1)}});"),
        ).unwrap();
    }
}

/// RSS alone selects a prefix, whose findings equal an uncapped run over exactly that prefix.
#[test]
fn rss_stops_only_at_a_whole_file_boundary() {
    let full = tempfile::tempdir().unwrap();
    write_tree(full.path(), 4);
    let mut samples = 0;
    let (code, truncated) = analyze(
        full.path(),
        "json",
        &["--max-memory", "64", "--fail-on-incomplete=limits"],
        || {
            samples += 1;
            Some(if samples == 2 {
                96 * 1024 * 1024 + 1
            } else {
                0
            })
        },
    );
    assert_eq!(samples, 2);
    assert_eq!(code, ExitCode::OperationalError);
    assert_eq!(truncated["filesAnalyzed"], 2);
    assert_eq!(truncated["testsAnalyzed"], 2);
    assert_eq!(truncated["incompleteCauses"]["filesOverMemoryBudget"], 2);
    assert_eq!(truncated["incomplete"], true);
    let prefix = tempfile::tempdir().unwrap();
    write_tree(prefix.path(), 2);
    let (_, exact) = analyze(prefix.path(), "json", &["--no-memory-cap"], || {
        panic!("uncapped sampling")
    });
    assert_eq!(exact["incomplete"], false);
    for field in ["findings", "testsAnalyzed", "filesAnalyzed"] {
        assert_eq!(truncated[field], exact[field], "{field}");
    }
    assert!(!exact["findings"].as_array().unwrap().is_empty());
    let (_, complete) = analyze(full.path(), "json", &["--max-memory", "64"], || None);
    assert_eq!(complete["filesAnalyzed"], 4);
    assert_eq!(complete["incomplete"], false);
}

/// Exact-threshold residency admits all files; a high final boundary omits nothing.
#[test]
fn equal_residency_and_a_last_file_crossing_do_not_claim_omissions() {
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), 2);
    let (_, equal) = analyze(root.path(), "json", &["--max-memory", "64"], || {
        Some(96 * 1024 * 1024)
    });
    assert_eq!(equal["filesAnalyzed"], 2);
    assert_eq!(equal["incomplete"], false);
    let prefix = tempfile::tempdir().unwrap();
    write_tree(prefix.path(), 1);
    let (_, last) = analyze(prefix.path(), "json", &["--max-memory", "64"], || {
        Some(u64::MAX)
    });
    assert_eq!(last["filesAnalyzed"], 1);
    assert_eq!(last["incomplete"], false);
}

/// HTML source accounting is still enforced even when residency cannot be measured.
#[test]
fn source_retained_for_html_counts_against_the_budget() {
    let root = tempfile::tempdir().unwrap();
    let padding = format!("// {}\n", "x".repeat(200_000));
    for file in 0..10 {
        std::fs::write(
            root.path().join(format!("f{file}.test.ts")),
            format!("import {{ test, expect }} from 'vitest';\n{padding}test('case {file}', () => {{ expect({file}).toBe({file}); }});\n"),
        ).unwrap();
    }
    // Both engines: the next one charges the source it retains as the current core does.
    for next in [false, true] {
        let engine = || next.then(vitest_dedup::corpus::Engine::default);
        let (_, json) = analyze_on(
            root.path(),
            "json",
            &["--max-memory", "1"],
            || None,
            engine(),
        );
        assert_eq!(json["incomplete"], false, "{json}");
        let (_, html) = analyze_on(
            root.path(),
            "json,html",
            &["--max-memory", "1"],
            || None,
            engine(),
        );
        assert!(
            html["incompleteCauses"]["filesOverMemoryBudget"]
                .as_u64()
                .unwrap()
                > 0,
            "{next}: {html}"
        );
    }
}

/// Imported runner source retained for HTML is charged once whichever file sorts first.
#[test]
fn a_test_file_retained_as_a_helper_is_charged_once() {
    for (runner, caller) in [("b", "a"), ("a", "b")] {
        let root = tempfile::tempdir().unwrap();
        let padding = format!("// {}\n", "x".repeat(600_000));
        std::fs::write(
            root.path().join(format!("{caller}.test.ts")),
            format!("import {{ run }} from './{runner}.test';\nrun();\n"),
        )
        .unwrap();
        std::fs::write(root.path().join(format!("{runner}.test.ts")), format!("import {{ test, expect }} from 'vitest';\n{padding}export function run() {{ test('runner', () => {{ expect(1).toBe(1); }}); }}\n")).unwrap();
        std::fs::write(
            root.path().join("c.test.ts"),
            "import { test, expect } from 'vitest';\ntest('last', () => { expect(2).toBe(2); });\n",
        )
        .unwrap();
        let (_, html) = analyze(root.path(), "json,html", &["--max-memory", "1"], || None);
        assert_eq!(html["filesAnalyzed"], 3, "{runner}: {html}");
        assert_eq!(html["testsAnalyzed"], 2, "{runner}: {html}");
    }
}

/// The historical pass samples RSS too; a partial baseline must not accept unseen findings.
#[test]
fn rss_stops_the_historical_pass_before_it_accepts_unseen_duplicates() {
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), 4);
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "test@example.com"],
        &["config", "user.name", "Test"],
        &["add", "."],
        &["commit", "-qm", "test(memory): initialize owned fixture"],
    ] {
        let output = std::process::Command::new("git")
            .env_clear()
            .envs(std::env::vars_os().filter(|(key, _)| !key.to_string_lossy().starts_with("GIT_")))
            .current_dir(root.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let flags = ["--max-memory", "64", "--baseline-from-ref", "HEAD"];
    let mut samples = 0;
    let (_, partial) = analyze(root.path(), "json", &flags, || {
        samples += 1;
        Some(if samples == 1 { u64::MAX } else { 0 })
    });
    assert_eq!(samples, 4, "one base file plus three current boundaries");
    assert_eq!(partial["filesAnalyzed"], 4);
    assert_eq!(partial["testsAnalyzed"], 4);
    let new_findings = partial["findings"].as_array().unwrap();
    assert!(!new_findings.is_empty());
    assert!(new_findings
        .iter()
        .all(|finding| finding["baselined"] == false));
    let (_, complete) = analyze(root.path(), "json", &flags, || None);
    let accepted = complete["findings"].as_array().unwrap();
    assert_eq!(accepted.len(), new_findings.len());
    assert!(accepted.iter().all(|finding| finding["baselined"] == true));
    // Residency from the historical pass remains part of the current process ceiling.
    let mut persistent_samples = 0;
    let (code, resident) = analyze(
        root.path(),
        "json",
        &[
            "--max-memory",
            "64",
            "--baseline-from-ref",
            "HEAD",
            "--fail-on-incomplete=limits",
        ],
        || {
            persistent_samples += 1;
            Some(96 * 1024 * 1024 + 1)
        },
    );
    assert_eq!(persistent_samples, 2, "one boundary in each pass");
    assert_eq!(code, ExitCode::OperationalError);
    assert_eq!(resident["filesAnalyzed"], 1);
    assert_eq!(resident["testsAnalyzed"], 1);
    assert_eq!(resident["incomplete"], true);
    assert_eq!(resident["incompleteCauses"]["filesOverMemoryBudget"], 3);
    let prefix = tempfile::tempdir().unwrap();
    write_tree(prefix.path(), 1);
    let (_, exact) = analyze(prefix.path(), "json", &["--no-memory-cap"], || {
        panic!("uncapped sampling")
    });
    assert_eq!(resident["findings"], exact["findings"]);
}
