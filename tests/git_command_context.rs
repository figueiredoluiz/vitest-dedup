//! Git-backed mode outcomes retain owned findings under repository and process settings.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SOURCE: &str =
    include_str!("../fixtures/recall/duplicate-test-git-mode-retention/scope.test.ts");

/// Run an owned fixture's Git operation with a predictable repository and commit identity.
fn git(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(key);
        }
    }
    let output = command
        .current_dir(root)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Commit the permanent owned source before applying per-test settings or worktree changes.
fn repository() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("scope.test.ts"), SOURCE).unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "fixture"]);
    temp
}

/// Capture one CLI invocation and its report outside the analyzed repository.
fn analyze(root: &Path, flag: &str, env: &[(&str, String)]) -> (Output, Option<serde_json::Value>) {
    let directory = tempfile::tempdir().unwrap();
    let prefix = directory.path().join("report");
    let mut command = assert_cmd::Command::cargo_bin("vitest-dedup").unwrap();
    command
        .arg(root)
        .args([flag, "HEAD", "--reporters", "json", "--output"])
        .arg(&prefix);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    let report = std::fs::read_to_string(prefix.with_extension("json"))
        .ok()
        .map(|s| serde_json::from_str(&s).unwrap());
    (output, report)
}

/// Verify extraction eligibility before the required finding and baseline metadata.
fn assert_retained(output: &Output, report: Option<serde_json::Value>, baseline: bool) {
    assert_eq!(
        output.status.code(),
        Some(if baseline { 0 } else { 1 }),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report.expect("successful analysis writes its report");
    assert_eq!(report["filesAnalyzed"], 1);
    assert_eq!(report["testsAnalyzed"], 2);
    assert_eq!(report["incomplete"], false);
    assert_eq!(report["findings"].as_array().unwrap().len(), 1);
    assert_eq!(report["findings"][0]["rule"], "duplicate-test");
    if baseline {
        assert_eq!(report["modes"]["baselined"], 1);
        assert_eq!(report["modes"]["newFindings"], 0);
    } else {
        assert_eq!(report["modes"]["withheldUnchanged"], 0);
    }
}

/// Construct a harmless command whose effect is entirely within the owned fixture.
fn marker(root: &Path) -> (PathBuf, String) {
    let path = root.join(".git/called");
    let quoted = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\\''");
    (path, format!("printf called > '{quoted}'; cat"))
}

/// Local monitor settings neither execute nor hide the owned duplicate in changed-file mode.
#[test]
fn changed_findings_survive_monitor_settings_and_unchanged_findings_stay_withheld() {
    let repo = repository();
    let root = repo.path();
    let (called, command) = marker(root);
    git(root, &["config", "core.fsmonitor", &command]);
    let (output, report) = analyze(root, "--changed-since", &[]);
    assert_eq!(output.status.code(), Some(0));
    let report = report.unwrap();
    assert_eq!(report["testsAnalyzed"], 2);
    assert_eq!(report["findings"].as_array().unwrap().len(), 0);
    assert_eq!(report["modes"]["withheldUnchanged"], 1);
    assert_eq!(report["incomplete"], true);
    assert!(!called.exists());
    std::fs::write(
        root.join("scope.test.ts"),
        format!("{SOURCE}\n// changed\n"),
    )
    .unwrap();
    let (output, report) = analyze(root, "--changed-since", &[]);
    assert_retained(&output, report, false);
    assert!(!called.exists());
}

/// Caller Git settings cannot redirect roots, inject configuration, force shallow state or write traces.
#[test]
fn inherited_git_settings_do_not_redirect_either_mode() {
    let repo = repository();
    let other = repository();
    let root = repo.path();
    let (called, command) = marker(root);
    let shallow = root.join(".git/other-shallow");
    std::fs::write(&shallow, git(root, &["rev-parse", "HEAD"]).stdout).unwrap();
    let config = root.join(".git/inherited-config");
    std::fs::write(&config, "[core]\n bare = true\n").unwrap();
    let cases = [
        vec![
            (
                "GIT_DIR",
                other.path().join(".git").to_string_lossy().into_owned(),
            ),
            ("GIT_WORK_TREE", other.path().to_string_lossy().into_owned()),
            (
                "GIT_INDEX_FILE",
                other
                    .path()
                    .join(".git/index")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ],
        vec![
            ("GIT_CONFIG_COUNT", "1".into()),
            ("GIT_CONFIG_KEY_0", "core.fsmonitor".into()),
            ("GIT_CONFIG_VALUE_0", command),
        ],
        vec![("GIT_CONFIG_PARAMETERS", "malformed".into())],
        vec![("GIT_SHALLOW_FILE", shallow.to_string_lossy().into_owned())],
        vec![("GIT_CONFIG_GLOBAL", config.to_string_lossy().into_owned())],
        vec![("GIT_TRACE", called.to_string_lossy().into_owned())],
    ];
    std::fs::write(
        root.join("scope.test.ts"),
        format!("{SOURCE}\n// changed\n"),
    )
    .unwrap();
    for env in cases {
        for flag in ["--changed-since", "--baseline-from-ref"] {
            let (output, report) = analyze(root, flag, &env);
            assert_retained(&output, report, flag == "--baseline-from-ref");
            assert!(!called.exists(), "{env:?}");
        }
    }
}

/// Required conversions are operational errors, never executed or presented as clean partial reports.
#[test]
fn needed_conversions_fail_but_historical_sources_and_unused_drivers_remain_supported() {
    for setting in ["clean", "process"] {
        let repo = repository();
        let root = repo.path();
        let (called, command) = marker(root);
        git(
            root,
            &["config", &format!("filter.owned.{setting}"), &command],
        );
        std::fs::write(
            root.join("scope.test.ts"),
            format!("{SOURCE}\n// changed\n"),
        )
        .unwrap();
        let (output, report) = analyze(root, "--changed-since", &[]);
        assert_retained(&output, report, false);
        std::fs::write(root.join(".gitattributes"), "*.ts filter=owned\n").unwrap();
        let (output, report) = analyze(root, "--changed-since", &[]);
        assert_eq!(output.status.code(), Some(2));
        assert!(report.is_none());
        assert!(String::from_utf8_lossy(&output.stderr).contains("clean filter 'owned' failed"));
        assert!(!called.exists());
        let (output, report) = analyze(root, "--baseline-from-ref", &[]);
        assert_retained(&output, report, true);
        assert!(!called.exists());
    }
}

/// An unavailable historical blob cannot be mistaken for a complete, empty baseline.
#[test]
fn an_unavailable_local_blob_is_an_operational_error() {
    let repo = repository();
    let root = repo.path();
    let output = git(root, &["rev-parse", "HEAD:scope.test.ts"]);
    let blob = String::from_utf8(output.stdout).unwrap();
    let blob = blob.trim();
    std::fs::remove_file(root.join(".git/objects").join(&blob[..2]).join(&blob[2..])).unwrap();
    git(
        root,
        &["config", "remote.origin.url", "ext::invalid-command"],
    );
    git(root, &["config", "remote.origin.promisor", "true"]);
    let (output, report) = analyze(root, "--baseline-from-ref", &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(report.is_none());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("could not read the size of base file")
    );
}

/// Git without environment-pair support still refuses drivers or rejects the required capability.
#[cfg(unix)]
#[test]
fn older_git_interface_keeps_selection_and_conversion_refusal() {
    use std::os::unix::fs::PermissionsExt;
    let resolved = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(resolved.status.success());
    let real_git = String::from_utf8(resolved.stdout).unwrap();
    let shim = tempfile::tempdir().unwrap();
    let script = shim.path().join("git");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nfor arg do\ncase \"$arg\" in --config-env=*) echo 'unsupported config-env' >&2; exit 129;; esac\ndone\nunset GIT_CONFIG_COUNT\nexec '{}' \"$@\"\n",
            real_git.trim().replace('\'', "'\\''")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths = vec![shim.path().to_path_buf()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let env = [(
        "PATH",
        std::env::join_paths(paths).unwrap().into_string().unwrap(),
    )];
    let repo = repository();
    let root = repo.path();
    std::fs::write(
        root.join("scope.test.ts"),
        format!("{SOURCE}\n// changed\n"),
    )
    .unwrap();
    let (output, report) = analyze(root, "--changed-since", &env);
    assert_retained(&output, report, false);
    let (called, command) = marker(root);
    for driver in ["owned", "Own.ed=one"] {
        git(
            root,
            &["config", &format!("filter.{driver}.clean"), &command],
        );
        std::fs::write(
            root.join(".gitattributes"),
            format!("*.ts filter={driver}\n"),
        )
        .unwrap();
        let (output, report) = analyze(root, "--changed-since", &env);
        assert_eq!(output.status.code(), Some(2));
        assert!(report.is_none());
        assert!(!called.exists());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains(if driver == "owned" {
                "clean filter 'owned' failed"
            } else {
                "unsupported config-env"
            }),
            "{error}"
        );
    }
}
