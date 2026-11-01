//! Next-engine shell modes preserve full analysis while narrowing only final findings.
use serde_json::Value;
use std::path::Path;
use std::process::Command;

/// Invoke the freshly built standalone binary and parse its complete JSONL stream.
fn run(root: &Path, flags: &[&str]) -> (i32, Vec<Value>, String) {
    let output = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(root)
        .args(["--reporters", "jsonl", "--no-metrics", "--no-memory-cap"])
        .args(flags)
        .env("VITEST_DEDUP_ENGINE", "next")
        .output()
        .unwrap();
    let records = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (
        output.status.code().unwrap(),
        records,
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// Run a fixture operation independently of inherited Git repository/configuration overrides.
fn git(root: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    let output = command.current_dir(root).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Inherited hook/config settings cannot redirect fixture writes into the calling repository.
#[test]
fn git_fixture_ignores_inherited_repository_environment() {
    const CHILD_ROOT: &str = "VITEST_DEDUP_TEST_GIT_FIXTURE_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = Path::new(&root);
        git(root, &["init", "-q"]);
        git(root, &["config", "user.name", "Inner fixture"]);
        git(root, &["config", "user.email", "inner@example.invalid"]);
        std::fs::write(root.join("owned.txt"), "inner fixture").unwrap();
        git(root, &["add", "owned.txt"]);
        git(
            root,
            &["commit", "-qm", "test(fixture): record isolated source"],
        );
        return;
    }

    let outer = tempfile::tempdir().unwrap();
    let inner = tempfile::tempdir().unwrap();
    git(outer.path(), &["init", "-q"]);
    git(outer.path(), &["config", "user.name", "Outer fixture"]);
    git(
        outer.path(),
        &["config", "user.email", "outer@example.invalid"],
    );
    std::fs::write(outer.path().join("keep.txt"), "outer fixture").unwrap();
    git(outer.path(), &["add", "keep.txt"]);
    git(
        outer.path(),
        &["commit", "-qm", "test(fixture): preserve caller source"],
    );
    let directory = outer.path().join(".git");
    let snapshots: Vec<_> = ["config", "index", "HEAD"]
        .map(|name| (name, std::fs::read(directory.join(name)).unwrap()))
        .into();
    let head = git(outer.path(), &["rev-parse", "HEAD"]).stdout;
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "git_fixture_ignores_inherited_repository_environment",
            "--nocapture",
        ])
        .env(CHILD_ROOT, inner.path())
        .env("GIT_DIR", &directory)
        .env("GIT_WORK_TREE", outer.path())
        .env("GIT_INDEX_FILE", directory.join("index"))
        .env("GIT_CONFIG", directory.join("config"))
        .output()
        .unwrap();
    for (name, bytes) in snapshots {
        assert_eq!(
            std::fs::read(directory.join(name)).unwrap(),
            bytes,
            "caller {name} changed"
        );
    }
    assert_eq!(git(outer.path(), &["rev-parse", "HEAD"]).stdout, head);
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(git(inner.path(), &["ls-files"]).stdout)
            .unwrap()
            .trim(),
        "owned.txt"
    );
    assert_eq!(
        String::from_utf8(git(inner.path(), &["rev-list", "--count", "HEAD"]).stdout)
            .unwrap()
            .trim(),
        "1"
    );
}

/// The summary exists exactly once and counts every analyzed test before findings are inspected.
fn summary(records: &[Value], tests: u64) -> &Value {
    let summaries: Vec<_> = records.iter().filter(|r| r["type"] == "summary").collect();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0]["testsAnalyzed"], tests);
    summaries[0]
}

/// New memory fields are exhaustive and the default engine keeps its previous record shape.
#[test]
fn debug_memory_accounts_for_next_populations() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.test.ts"),"import {test,expect} from 'vitest';test('a',()=>expect(1).toBe(1));test('b',()=>expect(1).toBe(1));").unwrap();
    let (code, records, stderr) = run(dir.path(), &["--debug-memory"]);
    assert_eq!(code, 1, "{stderr}");
    summary(&records, 2);
    let memory: Vec<_> = records.iter().filter(|r| r["type"] == "memory").collect();
    assert_eq!(memory.len(), 1);
    let memory = memory[0];
    let total = memory["retainedBytes"].as_u64().unwrap();
    assert!(total > 0);
    assert_eq!(memory["fields"]["engine"], total);
    assert_eq!(
        memory["engineFields"]
            .as_object()
            .unwrap()
            .values()
            .map(|n| n.as_u64().unwrap())
            .sum::<u64>(),
        total
    );
    assert_eq!(memory["bytesPerTest"], total / 2);
    assert!(memory["engineMemoryNotice"]
        .as_str()
        .unwrap()
        .contains("private hash-table"));
    assert_eq!(
        records
            .iter()
            .filter(|r| r["rule"] == "duplicate-test")
            .count(),
        1
    );
}

/// An unchanged twin remains analyzed when the other participant changes; wholly unchanged
/// findings are withheld visibly rather than presented as a complete clean result.
#[test]
fn changed_files_filter_findings_after_full_analysis() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.test.ts", "b.test.ts"] {
        std::fs::write(
            dir.path().join(name),
            "import {test,expect} from 'vitest';test('same',()=>expect(1).toBe(1));",
        )
        .unwrap();
    }
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["config", "user.name", "Fixture"]);
    git(
        dir.path(),
        &["config", "user.email", "fixture@example.invalid"],
    );
    git(dir.path(), &["add", "."]);
    git(
        dir.path(),
        &["commit", "-q", "-m", "test(fixture): establish base"],
    );
    let (code, records, stderr) = run(
        dir.path(),
        &["--changed-since", "HEAD", "--fail-on-incomplete"],
    );
    assert_eq!(code, 2, "{stderr}");
    let sum = summary(&records, 2);
    assert_eq!(sum["incomplete"], true);
    assert_eq!(sum["modes"]["withheldUnchanged"], 1);
    assert_eq!(records.iter().filter(|r| r["type"] == "finding").count(), 0);
    let path = dir.path().join("b.test.ts");
    let mut source = std::fs::read_to_string(&path).unwrap();
    source.push_str("\n// changed\n");
    std::fs::write(path, source).unwrap();
    let (code, records, stderr) = run(dir.path(), &["--changed-since", "HEAD"]);
    assert_eq!(code, 0, "{stderr}");
    let sum = summary(&records, 2);
    assert_eq!(sum["modes"]["withheldUnchanged"], 0);
    assert_eq!(sum["incomplete"], false);
    assert_eq!(
        records
            .iter()
            .filter(|r| r["rule"] == "cross-file-duplicate")
            .count(),
        1
    );
    let (code, _, stderr) = run(dir.path(), &["--changed-since", "not-a-revision"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("not-a-revision"));
}

/// Versioned file baselines compose with changed-file filtering and inline suppression in order.
#[test]
fn next_file_baselines_compose_with_changed_scope_and_suppression() {
    let dir = tempfile::tempdir().unwrap();
    let source = "import {test,expect} from 'vitest';\ntest('same',()=>expect(1).toBe(1));\n";
    for name in ["a.test.ts", "b.test.ts"] {
        std::fs::write(dir.path().join(name), source).unwrap();
    }
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["config", "user.name", "Fixture"]);
    git(
        dir.path(),
        &["config", "user.email", "fixture@example.invalid"],
    );
    git(dir.path(), &["add", "."]);
    git(
        dir.path(),
        &[
            "commit",
            "-q",
            "-m",
            "test(fixture): establish baseline source",
        ],
    );
    let baseline = dir.path().join("baseline.json");
    let path = baseline.to_str().unwrap();
    let (code, records, stderr) = run(dir.path(), &["--baseline", path, "--update-baseline"]);
    assert_eq!(code, 0, "{stderr}");
    summary(&records, 2);
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(&baseline).unwrap()).unwrap();
    assert_eq!(stored["fingerprintVersion"], "vitestDedupFingerprint/v2");
    let (code, records, stderr) = run(dir.path(), &["--baseline", path, "--changed-since", "HEAD"]);
    assert_eq!(code, 0, "{stderr}");
    let sum = summary(&records, 2);
    assert_eq!(sum["modes"]["withheldUnchanged"], 1);
    assert_eq!(sum["modes"]["baselined"], 0);
    assert_eq!(sum["incomplete"], true);
    let changed = dir.path().join("b.test.ts");
    std::fs::write(&changed, format!("{source}// changed\n")).unwrap();
    let (code, records, stderr) = run(
        dir.path(),
        &[
            "--baseline",
            path,
            "--changed-since",
            "HEAD",
            "--fail-on-new",
            "0",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let sum = summary(&records, 2);
    assert_eq!(sum["modes"]["withheldUnchanged"], 0);
    assert_eq!(sum["modes"]["baselined"], 1);
    assert_eq!(sum["modes"]["newFindings"], 0);
    assert_eq!(sum["incomplete"], false);
    std::fs::write(
        &changed,
        source.replace(
            "test('same'",
            "// vitest-dedup-disable-next-line intentional duplicate\ntest('same'",
        ),
    )
    .unwrap();
    let (code, records, stderr) = run(dir.path(), &["--baseline", path, "--changed-since", "HEAD"]);
    assert_eq!(code, 0, "{stderr}");
    let sum = summary(&records, 2);
    assert_eq!(sum["modes"]["suppressionsApplied"], 1);
    assert_eq!(sum["modes"]["baselined"], 0);
    assert_eq!(records.iter().filter(|r| r["type"] == "finding").count(), 0);
}
