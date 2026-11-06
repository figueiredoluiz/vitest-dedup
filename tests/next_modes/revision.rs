//! Historical outcomes use committed dependencies and the same v2 finalizer.
use super::{git, run, summary};
use serde_json::Value;
use std::path::Path;

const CALLER: &str =
    include_str!("../../fixtures/recall/duplicate-test-revision-helper-isolation/run.test.ts");
const LEFT: &str =
    include_str!("../../fixtures/recall/duplicate-test-revision-helper-isolation/left.ts");
const RIGHT: &str =
    include_str!("../../fixtures/recall/duplicate-test-revision-helper-isolation/right.ts");

/// Create the permanent owned source as a disposable repository with a local commit identity.
fn project(caller: &str, files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("run.test.ts"), caller).unwrap();
    for (name, source) in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["config", "user.name", "Fixture"]);
    git(
        dir.path(),
        &["config", "user.email", "fixture@example.invalid"],
    );
    git(dir.path(), &["add", "-f", "."]);
    git(
        dir.path(),
        &[
            "commit",
            "-qm",
            "test(fixture): establish historical sources",
        ],
    );
    dir
}

/// Invoke the reference mode with strict new-finding accounting and return its complete stream.
fn baseline(root: &Path, flags: &[&str]) -> (i32, Vec<Value>, String) {
    let mut args = vec!["--baseline-from-ref", "HEAD", "--fail-on-new", "0"];
    args.extend_from_slice(flags);
    run(root, &args)
}

/// Count extraction and every retained finding before judging acceptance of a historical pair.
fn assert_modes(records: &[Value], tests: u64, accepted: u64, new: u64) {
    let summary = summary(records, tests);
    assert_eq!(summary["modes"]["baselineApplied"], true);
    assert_eq!(summary["modes"]["baselined"], accepted);
    assert_eq!(summary["modes"]["newFindings"], new);
    assert_eq!(
        records.iter().filter(|r| r["type"] == "finding").count() as u64,
        accepted + new
    );
    assert_eq!(
        records
            .iter()
            .filter(|r| r["type"] == "finding" && r["baselined"] == true)
            .count() as u64,
        accepted
    );
    for finding in records.iter().filter(|r| r["type"] == "finding") {
        assert!(finding["fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("vitestDedupFingerprint/v3:"));
    }
}

/// A helper-only edit creates a genuinely new pair without losing the historical plain pair.
#[test]
fn historical_helpers_do_not_read_current_source() {
    let dir = project(CALLER, &[("left.ts", LEFT), ("right.ts", RIGHT)]);
    std::fs::write(dir.path().join("right.ts"), LEFT).unwrap();
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 1, "{stderr}");
    assert_modes(&records, 4, 1, 1);
    let finding = records.iter().find(|r| r["type"] == "finding").unwrap();
    assert_eq!(finding["rule"], "duplicate-test");
    assert!(!stderr.contains("baseline may be partial"), "{stderr}");

    let (code, records, stderr) = baseline(dir.path(), &["--changed-since", "HEAD"]);
    // Only a non-test helper changed: current changed-files mode still scopes by finding paths.
    assert_eq!(code, 0, "{stderr}");
    assert_modes(&records, 4, 0, 0);
    assert_eq!(summary(&records, 4)["modes"]["withheldUnchanged"], 2);
}

/// A nested runner's historical child cannot borrow the current child's newly matching body.
#[test]
fn historical_nested_runner_children_keep_their_committed_source() {
    let caller = include_str!("../../fixtures/recall/runner-nested-forwarded-capture/run.test.ts");
    let runner = include_str!("../../fixtures/recall/runner-nested-forwarded-capture/runner.ts");
    let distinct =
        include_str!("../../fixtures/recall/runner-nested-forwarded-capture/leaf-distinct.ts");
    let matching = include_str!("../../fixtures/recall/runner-nested-forwarded-capture/leaf.ts");
    let dir = project(caller, &[("runner.ts", runner), ("leaf.ts", distinct)]);
    std::fs::write(dir.path().join("leaf.ts"), matching).unwrap();
    for changed in [false, true] {
        let mut flags = vec![
            "--rule",
            "parameterization-candidate=off",
            "--rule",
            "near-duplicate-test=off",
        ];
        if changed {
            flags.extend(["--changed-since", "HEAD"]);
        }
        let (code, records, stderr) = baseline(dir.path(), &flags);
        assert_eq!(code, 1, "changed={changed}: {stderr}");
        assert_modes(&records, 3, 0, 1);
        assert_eq!(summary(&records, 3)["unresolvedTests"], 0);
        assert_eq!(summary(&records, 3)["incomplete"], false);
        assert!(!stderr.contains("baseline may be partial"), "{stderr}");
        let finding = records
            .iter()
            .find(|record| record["type"] == "finding")
            .unwrap();
        assert_eq!(finding["rule"], "duplicate-test");
        assert_eq!(finding["locations"].as_array().unwrap().len(), 2);
        assert_eq!(
            finding["registrationFiles"],
            serde_json::json!([["run.test.ts"], ["run.test.ts"]])
        );
    }
}

/// Removing a historical positive in current helpers cannot spend unrelated baseline allowance.
#[test]
fn historical_equal_helpers_keep_their_positive() {
    let dir = project(CALLER, &[("left.ts", LEFT), ("right.ts", LEFT)]);
    std::fs::write(dir.path().join("right.ts"), RIGHT).unwrap();
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 0, "{stderr}");
    assert_modes(&records, 4, 1, 0);
    assert!(
        stderr.contains("2 finding(s) in the base revision"),
        "{stderr}"
    );
}

/// Re-export targets and package entry metadata both belong to the requested revision.
#[test]
fn historical_reexports_and_package_main_are_isolated() {
    for package in [false, true] {
        let caller = CALLER.replace("'./right'", "'./entry'");
        let entry = if package {
            "entry/package.json"
        } else {
            "entry.ts"
        };
        let old = if package {
            "{\"main\":\"../right.ts\"}"
        } else {
            "export {run} from './right';"
        };
        let changed = if package {
            "{\"main\":\"../left.ts\"}"
        } else {
            "export {run} from './left';"
        };
        let dir = project(
            &caller,
            &[("left.ts", LEFT), ("right.ts", RIGHT), (entry, old)],
        );
        std::fs::write(dir.path().join(entry), changed).unwrap();
        let (code, records, stderr) = baseline(dir.path(), &[]);
        assert_eq!(code, 1, "package={package}: {stderr}");
        assert_modes(&records, 4, 1, 1);
    }
}

/// Historical alias mappings apply even when current config and helper files disagree.
#[test]
fn historical_tsconfig_uses_its_own_aliases() {
    const ALIAS_CALLER: &str =
        include_str!("../../fixtures/recall/duplicate-test-revision-tsconfig-paths/run.test.ts");
    const CURRENT: &str = include_str!(
        "../../fixtures/recall/duplicate-test-revision-tsconfig-paths/current.source.ts"
    );
    const CONFIG: &str =
        include_str!("../../fixtures/recall/duplicate-test-revision-tsconfig-paths/tsconfig.json");
    for same_target in [false, true] {
        let config = if same_target {
            CONFIG.replace("right.ts", "left.ts")
        } else {
            CONFIG.to_owned()
        };
        let dir = project(
            ALIAS_CALLER,
            &[
                ("left.ts", LEFT),
                ("right.ts", RIGHT),
                ("tsconfig.json", &config),
            ],
        );
        std::fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();
        std::fs::write(dir.path().join("run.test.ts"), CURRENT).unwrap();
        std::fs::remove_file(dir.path().join("right.ts")).unwrap();
        let (code, records, stderr) = baseline(dir.path(), &[]);
        assert_eq!(code, i32::from(!same_target), "{stderr}");
        assert_modes(
            &records,
            4,
            if same_target { 2 } else { 1 },
            u64::from(!same_target),
        );
    }
}

/// A reference suppression cannot be replaced with the present caller's missing directive.
#[test]
fn historical_suppressions_precede_reference_allowance() {
    let suppressed = CALLER.replace(
        "test('plain two'",
        "// vitest-dedup-disable-next-line intentional duplicate\ntest('plain two'",
    );
    let dir = project(&suppressed, &[("left.ts", LEFT), ("right.ts", RIGHT)]);
    std::fs::write(dir.path().join("run.test.ts"), CALLER).unwrap();
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 1, "{stderr}");
    assert_modes(&records, 4, 0, 1);
}

/// An unproved committed invocation withholds scope allowance instead of borrowing working hooks.
#[test]
fn historical_inventory_keeps_its_own_uncertainty() {
    let caller =
        include_str!("../../fixtures/recall/runner-unproved-scope-retention/unsafe.test.ts");
    let helper = include_str!("../../fixtures/recall/runner-unproved-scope-retention/helper.ts");
    let dir = project(caller, &[("helper.ts", helper)]);
    std::fs::write(
        dir.path().join("helper.ts"),
        "import {beforeEach} from 'vitest';export function setup(){beforeEach(()=>{})}",
    )
    .unwrap();
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 1, "{stderr}");
    assert_modes(&records, 2, 0, 1);
    assert_eq!(summary(&records, 2)["incomplete"], false);
    assert!(stderr.contains("baseline may be partial"), "{stderr}");
}

/// A malformed committed resolver config cannot be repaired by its present valid replacement.
#[test]
fn historical_config_errors_do_not_fall_back_to_current_files() {
    let dir = project(
        CALLER,
        &[
            ("left.ts", LEFT),
            ("right.ts", RIGHT),
            ("tsconfig.json", "{broken config"),
        ],
    );
    std::fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 2, "{stderr}");
    assert!(records.is_empty());
    assert!(
        stderr.contains("base revision source access failed"),
        "{stderr}"
    );
}

/// A nested analysis root resolves aliases through the historical repository ancestor config.
#[test]
fn historical_ancestor_config_is_used_for_nested_projects() {
    let dir = project(
        "test('unused',()=>{})",
        &[
            (
                "nested/run.test.ts",
                &CALLER
                    .replace("'./left'", "'@left'")
                    .replace("'./right'", "'@right'"),
            ),
            ("nested/left.ts", LEFT),
            ("nested/right.ts", RIGHT),
            (
                "tsconfig.json",
                r#"{"compilerOptions":{"baseUrl":".","paths":{"@left":["nested/left.ts"],"@right":["nested/left.ts"]}}}"#,
            ),
        ],
    );
    let path = dir.path().join("tsconfig.json");
    let changed = std::fs::read_to_string(&path)
        .unwrap()
        .replace("nested/left.ts", "nested/right.ts");
    std::fs::write(path, changed).unwrap();
    std::fs::write(
        dir.path().join("nested/run.test.ts"),
        include_str!(
            "../../fixtures/recall/duplicate-test-revision-tsconfig-paths/current.source.ts"
        ),
    )
    .unwrap();
    let (code, records, stderr) = baseline(&dir.path().join("nested"), &[]);
    assert_eq!(code, 0, "{stderr}");
    assert_modes(&records, 4, 2, 0);
}

/// The base's actual invalid helper bytes cannot be replaced by a valid present-day helper.
#[test]
fn invalid_historical_helper_text_refuses_the_baseline() {
    let dir = project(CALLER, &[("left.ts", LEFT), ("right.ts", RIGHT)]);
    std::fs::write(dir.path().join("right.ts"), [255]).unwrap();
    git(dir.path(), &["add", "."]);
    git(
        dir.path(),
        &[
            "commit",
            "-qm",
            "test(fixture): record invalid historical helper",
        ],
    );
    std::fs::write(dir.path().join("right.ts"), LEFT).unwrap();
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 2, "{stderr}");
    assert!(records.is_empty());
    assert!(stderr.contains("not valid UTF-8"), "{stderr}");
}

/// A selected root symlink is not treated as committed JavaScript, even with a live target.
#[cfg(unix)]
#[test]
fn historical_symlinks_are_never_followed() {
    for root in [false, true] {
        let dir = project(CALLER, &[("left.ts", LEFT), ("right.ts", RIGHT)]);
        let name = if root { "linked.test.ts" } else { "right.ts" };
        if !root {
            std::fs::remove_file(dir.path().join(name)).unwrap();
        }
        std::os::unix::fs::symlink("left.ts", dir.path().join(name)).unwrap();
        git(dir.path(), &["add", "."]);
        git(
            dir.path(),
            &["commit", "-qm", "test(fixture): record historical symlink"],
        );
        std::fs::remove_file(dir.path().join(name)).unwrap();
        std::fs::write(dir.path().join(name), if root { CALLER } else { LEFT }).unwrap();
        let (code, records, stderr) = baseline(dir.path(), &[]);
        assert_eq!(code, 2, "root={root}: {stderr}");
        assert!(records.is_empty());
        assert!(stderr.contains("not a regular file"), "{stderr}");
    }
}

/// Bare-import identity uses historical physical module identity, not present symlink targets.
#[cfg(unix)]
#[test]
fn historical_bare_import_scope_uses_the_same_source_authority() {
    let caller =
        "import {test,expect} from 'vitest'; import './side'; test('a',()=>expect(1).toBe(1));";
    let dir = project(
        "",
        &[
            ("a/a.test.ts", caller),
            ("b/b.test.ts", caller),
            ("a/side.ts", "console.log('a');"),
            ("b/side.ts", "console.log('b');"),
        ],
    );
    std::fs::remove_file(dir.path().join("b/side.ts")).unwrap();
    std::os::unix::fs::symlink("../a/side.ts", dir.path().join("b/side.ts")).unwrap();
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 1, "{stderr}");
    assert_modes(&records, 2, 0, 1);
}

/// An absolute extends target outside the committed namespace cannot supply live configuration.
#[test]
fn historical_config_cannot_escape_to_a_live_file() {
    let external = tempfile::tempdir().unwrap();
    let target = external.path().join("live.json");
    std::fs::write(&target, "{}").unwrap();
    let old = serde_json::json!({"extends":target}).to_string();
    let dir = project(
        CALLER,
        &[
            ("left.ts", LEFT),
            ("right.ts", RIGHT),
            ("tsconfig.json", &old),
        ],
    );
    std::fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 2, "{stderr}");
    assert!(records.is_empty());
    assert!(
        stderr.contains("base revision source access failed"),
        "{stderr}"
    );
}

/// Plumbing neither invokes repository callbacks nor changes source, index, config or refs.
#[cfg(unix)]
#[test]
fn historical_source_access_does_not_execute_callbacks_or_mutate_git() {
    let dir = project(
        CALLER,
        &[
            ("left.ts", LEFT),
            ("right.ts", RIGHT),
            (".gitattributes", "*.ts filter=probe\n"),
        ],
    );
    let marker = dir.path().join("callback-ran");
    let script = dir.path().join("callback");
    std::fs::write(&script, "#!/bin/sh\ntouch callback-ran\ncat\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let command = script.to_str().unwrap();
    for name in [
        "core.fsmonitor",
        "filter.probe.clean",
        "filter.probe.smudge",
    ] {
        git(dir.path(), &["config", name, command]);
    }
    git(dir.path(), &["config", "core.hooksPath", command]);
    let paths = [
        ".git/index",
        ".git/config",
        ".git/HEAD",
        "left.ts",
        "right.ts",
        "run.test.ts",
    ];
    let before = paths.map(|path| std::fs::read(dir.path().join(path)).unwrap());
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 0, "{stderr}");
    assert_modes(&records, 4, 1, 0);
    assert!(!marker.exists());
    for (path, bytes) in paths.into_iter().zip(before) {
        assert_eq!(
            std::fs::read(dir.path().join(path)).unwrap(),
            bytes,
            "{path}"
        );
    }
}

/// Files that exist only in the present cannot satisfy a missing historical import.
#[test]
fn historical_missing_helpers_do_not_borrow_new_files() {
    let dir = project(CALLER, &[("left.ts", LEFT)]);
    std::fs::write(dir.path().join("right.ts"), LEFT).unwrap();
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 1, "{stderr}");
    assert_modes(&records, 4, 1, 1);
}

/// Gitlinks are not source files or directories and cannot trigger external submodule access.
#[test]
fn historical_gitlinks_are_refused_without_loading_a_submodule() {
    for root in [false, true] {
        let dir = project(CALLER, &[("left.ts", LEFT), ("right.ts", RIGHT)]);
        let commit = git(dir.path(), &["rev-parse", "HEAD"]);
        let commit = String::from_utf8(commit.stdout).unwrap();
        let name = if root { "linked.test.ts" } else { "right.ts" };
        let entry = format!("160000,{},{}", commit.trim(), name);
        git(
            dir.path(),
            &["update-index", "--add", "--cacheinfo", &entry],
        );
        git(
            dir.path(),
            &["commit", "-qm", "test(fixture): record historical gitlink"],
        );
        std::fs::write(dir.path().join(name), if root { CALLER } else { LEFT }).unwrap();
        let (code, records, stderr) = baseline(dir.path(), &[]);
        assert_eq!(code, 2, "root={root}: {stderr}");
        assert!(records.is_empty());
        assert!(
            stderr.contains(if root {
                "not a blob"
            } else {
                "not a regular file"
            }),
            "{stderr}"
        );
    }
}

/// Missing package config degrades the base without accepting opaque historical identities.
#[test]
fn missing_historical_package_config_is_partial_without_allowance() {
    const CONFIG: &str =
        include_str!("../../fixtures/recall/duplicate-test-revision-tsconfig-gap/tsconfig.json");
    for config in [
        CONFIG,
        "{\"extends\":[\"./local.json\"]}",
        "{\"extends\":[\"@tsconfig/node20/tsconfig.json\",\"./plain.json\"]}",
    ] {
        let dir = project(
            CALLER,
            &[
                ("left.ts", LEFT),
                ("right.ts", RIGHT),
                ("tsconfig.json", config),
                ("local.json", CONFIG),
                ("plain.json", "{}"),
            ],
        );
        let pkg = dir.path().join("node_modules/@tsconfig/node20");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("package.json"),
            "{\"name\":\"@tsconfig/node20\",\"version\":\"0.0.0\"}",
        )
        .unwrap();
        std::fs::write(pkg.join("tsconfig.json"), "{}").unwrap();
        std::fs::write(dir.path().join("right.ts"), LEFT).unwrap();
        let (code, records, stderr) = baseline(dir.path(), &[]);
        assert_eq!(code, 1, "{stderr}");
        assert_modes(&records, 4, 0, 2);
        assert_eq!(summary(&records, 4)["incomplete"], false);
        assert!(stderr.contains("baseline may be partial"), "{stderr}");
        assert!(
            stderr.contains("withholding all revision baseline allowance"),
            "{stderr}"
        );
    }
}

/// A committed package config remains ordinary historical authority and keeps its positive pair.
#[test]
fn committed_package_config_retains_reference_allowance() {
    let config =
        include_str!("../../fixtures/recall/duplicate-test-revision-tsconfig-gap/tsconfig.json");
    let dir = project(
        CALLER,
        &[
            ("left.ts", LEFT),
            ("right.ts", RIGHT),
            ("tsconfig.json", config),
            ("node_modules/@tsconfig/node20/tsconfig.json", "{}"),
            (
                "node_modules/@tsconfig/node20/package.json",
                "{\"name\":\"@tsconfig/node20\",\"version\":\"0.0.0\"}",
            ),
        ],
    );
    std::fs::write(dir.path().join("right.ts"), LEFT).unwrap();
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 1, "{stderr}");
    assert_modes(&records, 4, 1, 1);
    assert!(!stderr.contains("baseline may be partial"), "{stderr}");
}

/// Existing bad package config and missing relative config remain operational refusals.
#[test]
fn invalid_committed_config_is_not_misclassified_as_package_absence() {
    for (config, files) in [
        ("{\"extends\":\"./missing.json\"}", vec![]),
        ("{\"extends\":\"#missing\"}", vec![]),
        (
            "{\"extends\":\"@tsconfig/node20/tsconfig.json\"}",
            vec![("node_modules/@tsconfig/node20/tsconfig.json", "{bad json")],
        ),
    ] {
        let mut sources = vec![
            ("left.ts", LEFT),
            ("right.ts", RIGHT),
            ("tsconfig.json", config),
        ];
        sources.extend(files);
        let dir = project(CALLER, &sources);
        std::fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();
        let (code, records, stderr) = baseline(dir.path(), &[]);
        assert_eq!(code, 2, "{stderr}");
        assert!(records.is_empty());
        assert!(
            stderr.contains("base revision source access failed"),
            "{stderr}"
        );
    }
}

/// A recorded package-directory target with no default config degrades without host lookup.
#[test]
fn missing_default_config_in_a_committed_package_is_partial() {
    let dir = project(
        CALLER,
        &[
            ("left.ts", LEFT),
            ("right.ts", RIGHT),
            ("tsconfig.json", "{\"extends\":\"./node_modules/pkg\"}"),
            (
                "node_modules/pkg/package.json",
                "{\"name\":\"pkg\",\"version\":\"0.0.0\"}",
            ),
        ],
    );
    std::fs::write(dir.path().join("node_modules/pkg/tsconfig.json"), "{}").unwrap();
    std::fs::write(dir.path().join("right.ts"), LEFT).unwrap();
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 1, "{stderr}");
    assert_modes(&records, 4, 0, 2);
    assert!(stderr.contains("baseline may be partial"), "{stderr}");
}

/// Missing unrelated objects must not turn a complete owned baseline into an operational error.
#[test]
fn unrelated_missing_blob_preserves_baseline_allowance() {
    let dir = project(
        CALLER,
        &[
            ("left.ts", LEFT),
            ("right.ts", RIGHT),
            ("unrelated.bin", "an unrelated committed blob"),
        ],
    );
    let oid = git(dir.path(), &["rev-parse", "HEAD:unrelated.bin"]);
    let oid = String::from_utf8(oid.stdout).unwrap();
    let oid = oid.trim();
    std::fs::remove_file(
        dir.path()
            .join(".git/objects")
            .join(&oid[..2])
            .join(&oid[2..]),
    )
    .unwrap();
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 0, "{stderr}");
    assert_modes(&records, 4, 1, 0);
    assert!(!stderr.contains("baseline may be partial"), "{stderr}");
}

/// A missing committed helper remains fatal and identifies unavailable local Git bytes.
#[test]
fn needed_missing_blob_is_a_clear_operational_error() {
    let dir = project(CALLER, &[("left.ts", LEFT), ("right.ts", RIGHT)]);
    let oid = git(dir.path(), &["rev-parse", "HEAD:right.ts"]);
    let oid = String::from_utf8(oid.stdout).unwrap();
    let oid = oid.trim();
    std::fs::remove_file(
        dir.path()
            .join(".git/objects")
            .join(&oid[..2])
            .join(&oid[2..]),
    )
    .unwrap();
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 2, "{stderr}");
    assert!(records.is_empty());
    assert!(
        stderr.contains("not available in local Git objects"),
        "{stderr}"
    );
}

/// Workspace links absent from Git cannot silently look like a complete historical pass.
#[cfg(unix)]
#[test]
fn missing_workspace_link_is_partial_without_historical_allowance() {
    let caller =
        include_str!("../../fixtures/recall/duplicate-test-revision-workspace/workspace.source.ts");
    let package = include_str!(
        "../../fixtures/recall/duplicate-test-revision-workspace/packages/runner/package.json"
    );
    let helper = include_str!(
        "../../fixtures/recall/duplicate-test-revision-workspace/packages/runner/index.ts"
    );
    for subpath in [false, true] {
        let caller = if subpath {
            caller.replace("@acme/test-utils'", "@acme/test-utils/api'")
        } else {
            caller.to_owned()
        };
        let package = if subpath {
            package.replace(
                "\"./index.ts\"",
                "{\".\":\"./index.ts\",\"./api\":\"./index.ts\"}",
            )
        } else {
            package.to_owned()
        };
        let dir = project(
            &caller,
            &[
                (
                    "package.json",
                    "{\"private\":true,\"workspaces\":[\"packages/*\"]}",
                ),
                ("packages/runner/package.json", &package),
                ("packages/runner/index.ts", helper),
            ],
        );
        let link = dir.path().join("node_modules/@acme");
        std::fs::create_dir_all(&link).unwrap();
        std::os::unix::fs::symlink(dir.path().join("packages/runner"), link.join("test-utils"))
            .unwrap();
        let (code, records, stderr) = baseline(dir.path(), &[]);
        assert_eq!(code, 1, "{stderr}");
        assert_modes(&records, 2, 0, 1);
        assert!(stderr.contains("baseline may be partial"), "{stderr}");
        assert!(
            stderr.contains("withholding all revision baseline allowance"),
            "{stderr}"
        );
    }
}

/// Explicit committed project paths keep their historical positive without a workspace link.
#[test]
fn committed_project_runner_keeps_reference_allowance() {
    let caller =
        include_str!("../../fixtures/recall/duplicate-test-revision-workspace/workspace.source.ts")
            .replace("@acme/test-utils", "./packages/runner");
    let dir = project(
        &caller,
        &[
            (
                "packages/runner/package.json",
                "{\"name\":\"@acme/test-utils\",\"main\":\"./index.ts\"}",
            ),
            (
                "packages/runner/index.ts",
                "export {test,expect} from 'vitest';",
            ),
        ],
    );
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 0, "{stderr}");
    assert_modes(&records, 2, 1, 0);
    assert!(!stderr.contains("baseline may be partial"), "{stderr}");
}

/// Ordinary opaque dependencies must not invalidate unrelated baseline positives.
#[test]
fn missing_external_package_without_project_name_keeps_allowance() {
    let caller="import {test,expect} from 'vitest';import {unused} from 'external-lib';test('one',()=>expect(1).toBe(1));test('two',()=>expect(1).toBe(1));";
    let dir = project(caller, &[("package.json", "{\"name\":\"owned-project\"}")]);
    let (code, records, stderr) = baseline(dir.path(), &[]);
    assert_eq!(code, 0, "{stderr}");
    assert_modes(&records, 2, 1, 0);
    assert!(!stderr.contains("baseline may be partial"), "{stderr}");
}

/// An unqueryable filename outside a nested analysis root is not an indexing prerequisite.
#[cfg(unix)]
#[test]
fn unrelated_non_utf8_entry_does_not_abort_nested_baseline() {
    use std::io::Write;
    use std::process::Stdio;
    let dir=project("",&[("nested/run.test.ts","import {test,expect} from 'vitest';test('a',()=>expect(1).toBe(1));test('b',()=>expect(1).toBe(1));")]);
    let blob = git(dir.path(), &["rev-parse", "HEAD:run.test.ts"]);
    let blob = String::from_utf8(blob.stdout).unwrap();
    let mut tree = git(dir.path(), &["ls-tree", "-z", "HEAD"]).stdout;
    tree.extend_from_slice(format!("100644 blob {}\t", blob.trim()).as_bytes());
    tree.extend_from_slice(b"\xff.bin\0");
    let mut child = super::git_command(dir.path())
        .args(["mktree", "-z"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&tree).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8(output.stdout).unwrap();
    git(dir.path(), &["read-tree", tree.trim()]);
    git(
        dir.path(),
        &[
            "commit",
            "-qm",
            "test(fixture): include unrelated non-UTF-8 Git entry",
        ],
    );
    let (code, records, stderr) = baseline(&dir.path().join("nested"), &[]);
    assert_eq!(code, 0, "{stderr}");
    assert_modes(&records, 2, 1, 0);
}
