use assert_cmd::Command;
use std::path::Path;
use vitest_dedup::model::Rule;

const HELPER: &str = "import { test, expect } from 'vitest';\nexport function run() {\n  test('first', () => { expect('helper source marker').toBe('helper source marker'); });\n  test('second', () => { expect('helper source marker').toBe('helper source marker'); });\n}\n";

/// Writes a first-party runner with two declarations and a caller that registers them twice.
fn fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("runner.ts"), HELPER).unwrap();
    std::fs::write(
        directory.path().join("caller.test.ts"),
        "import { run } from './runner'; run(); run();",
    )
    .unwrap();
    directory
}

/// Runs the standalone analyzer with a deterministic JSON report and selected duplicate rule.
fn command(root: &Path) -> Command {
    let mut command = Command::cargo_bin("vitest-dedup").unwrap();
    command.arg(root);
    for rule in Rule::ALL {
        command.args([
            "--rule",
            &format!(
                "{}={}",
                rule.id(),
                if *rule == Rule::DuplicateTest {
                    "error"
                } else {
                    "off"
                }
            ),
        ]);
    }
    command
}

/// Reads the analyzer's JSON report from the isolated fixture root.
fn report(root: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(root.join("report.json")).unwrap()).unwrap()
}

#[test]
/// Requires helper source bytes to drive HTML snippets and inline suppression decisions.
fn imported_helper_sources_supply_html_snippets_and_source_suppressions() {
    helper_sources_drive_snippets_and_suppressions(&[]);
}

/// A helper under a subdirectory with the same file name as a root helper does not take the root
/// helper's suppression, whatever directory the analyzer runs from.
#[test]
fn the_analyzer_keeps_helper_identities_whatever_the_working_directory() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(
        root.join("runner.ts"),
        "import { test, expect } from 'vitest';\nexport function run() {\n  // vitest-dedup-disable-next-line owned unrelated\n  test('different', () => { expect(100).toBe(100); });\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("sub/runner.ts"),
        "import { test, expect } from 'vitest';\nexport function run() {\n  test('a', () => { expect(1).toBe(1); });\n  test('b', () => { expect(1).toBe(1); });\n}\n",
    )
    .unwrap();
    for caller in ["caller.test.ts", "sub/caller.test.ts"] {
        std::fs::write(root.join(caller), "import { run } from './runner'; run();").unwrap();
    }
    for directory in [root.to_path_buf(), root.join("sub")] {
        command(root)
            .current_dir(&directory)
            .args(["--reporters", "json", "--output"])
            .arg(root.join("report"))
            .assert()
            .code(1);
        let result = report(root);
        assert_eq!(
            result["findings"].as_array().unwrap().len(),
            1,
            "{directory:?}"
        );
        assert_eq!(
            result["findings"][0]["locations"][0]["path"],
            "sub/runner.ts"
        );
    }
}

/// Runs the fixture with `environment`, then again with its first test suppressed in the helper.
fn helper_sources_drive_snippets_and_suppressions(environment: &[(&str, &str)]) {
    let directory = fixture();
    let root = directory.path();
    command(root)
        .envs(environment.iter().copied())
        .args(["--reporters", "json,html", "--output"])
        .arg(root.join("report"))
        .assert()
        .code(1);
    let first = report(root);
    assert_eq!(first["testsAnalyzed"], 4);
    assert_eq!(first["findings"].as_array().unwrap().len(), 1);
    assert_eq!(first["findings"][0]["locations"][0]["path"], "runner.ts");
    assert_eq!(
        first["findings"][0]["registrationFiles"][0][0],
        "caller.test.ts"
    );
    let html = std::fs::read_to_string(root.join("report.html")).unwrap();
    assert!(
        html.contains("helper source marker"),
        "the snippet must come from the undiscovered helper source"
    );
    let suppressed = HELPER.replace(
        "  test('first'",
        "  // vitest-dedup-disable-next-line owned intentional pair\n  test('first'",
    );
    std::fs::write(root.join("runner.ts"), suppressed).unwrap();
    command(root)
        .envs(environment.iter().copied())
        .args(["--reporters", "json", "--output"])
        .arg(root.join("report"))
        .assert()
        .success();
    let second = report(root);
    assert_eq!(second["testsAnalyzed"], 4);
    assert_eq!(second["findings"].as_array().unwrap().len(), 0);
    assert_eq!(second["modes"]["suppressionsApplied"], 1);
}

#[test]
/// Requires current file baselines to retain imported declaration/caller identity.
fn imported_findings_roundtrip_through_a_current_file_baseline() {
    let directory = fixture();
    let root = directory.path();
    let baseline = root.join("baseline.json");
    command(root)
        .arg("--baseline")
        .arg(&baseline)
        .arg("--update-baseline")
        .assert()
        .success();
    command(root)
        .arg("--baseline")
        .arg(&baseline)
        .args(["--reporters", "json", "--output"])
        .arg(root.join("report"))
        .assert()
        .success();
    let result = report(root);
    assert_eq!(result["testsAnalyzed"], 4);
    assert_eq!(result["findings"].as_array().unwrap().len(), 1);
    assert_eq!(result["findings"][0]["baselined"], true);
    assert_eq!(result["modes"]["baselined"], 1);
}

#[test]
/// Keeps mixed successful/refused extraction incomplete and prevents baseline updates.
fn an_extracted_and_refused_file_remains_incomplete_and_cannot_update_a_baseline() {
    let directory = fixture();
    let root = directory.path();
    std::fs::write(
        root.join("caller.test.ts"),
        "import { run } from './runner'; run(); run(setup());",
    )
    .unwrap();
    command(root)
        .args(["--reporters", "json", "--output"])
        .arg(root.join("report"))
        .arg("--fail-on-incomplete")
        .assert()
        .code(2);
    let result = report(root);
    assert_eq!(result["testsAnalyzed"], 2);
    assert_eq!(result["findings"].as_array().unwrap().len(), 0);
    assert_eq!(result["incomplete"], true);
    assert_eq!(result["unresolvedTests"], 2);
    command(root)
        .arg("--baseline")
        .arg(root.join("baseline.json"))
        .arg("--update-baseline")
        .assert()
        .code(2);
    assert!(!root.join("baseline.json").exists());
}

/// Runs fixture Git commands without inheriting another repository's Git environment.
fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_PREFIX")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
/// Changed callers are kept, and the historical reader proves the helper's allowance.
fn revision_baselines_and_changed_callers_follow_the_helper() {
    {
        let directory = fixture();
        let root = directory.path();
        let selected_command = || command(root);
        git(root, &["init", "--quiet"]);
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "user.name=Owned fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "--quiet",
                "-m",
                "owned baseline",
            ],
        );
        std::fs::write(
            root.join("caller.test.ts"),
            "import { run } from './runner'; run();",
        )
        .unwrap();
        selected_command()
            .args(["--changed-since", "HEAD", "--reporters", "json", "--output"])
            .arg(root.join("report"))
            .assert()
            .code(1);
        let changed = report(root);
        assert_eq!(changed["testsAnalyzed"], 2);
        assert_eq!(changed["findings"].as_array().unwrap().len(), 1);
        assert_eq!(changed["findings"][0]["locations"][0]["path"], "runner.ts");
        assert_eq!(changed["modes"]["withheldUnchanged"], 0);
        let output = selected_command()
            .args([
                "--baseline-from-ref",
                "HEAD",
                "--reporters",
                "json",
                "--output",
            ])
            .arg(root.join("report"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(!String::from_utf8(output.stderr)
            .unwrap()
            .contains("baseline may be partial"));
        let baseline = report(root);
        assert_eq!(baseline["testsAnalyzed"], 2);
        assert_eq!(baseline["findings"].as_array().unwrap().len(), 1);
        assert_eq!(baseline["findings"][0]["baselined"], true);
        assert_eq!(baseline["incomplete"], false);
        assert_eq!(baseline["modes"]["baselineApplied"], true);
        assert_eq!(baseline["modes"]["baselined"], 1);
        assert_eq!(baseline["modes"]["newFindings"], 0);
        assert!(baseline["findings"][0]["fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("vitestDedupFingerprint/v2:"));
    }
}

#[test]
/// A helper finding the base revision suppressed is new once the working tree drops the directive:
/// the base pass reads the committed helper text, so it never records the finding as accepted.
fn the_analyzer_base_pass_applies_committed_helper_suppressions() {
    let directory = fixture();
    let root = directory.path();
    let suppressed = HELPER.replace(
        "  test('first'",
        "  // vitest-dedup-disable-next-line owned intentional pair\n  test('first'",
    );
    std::fs::write(root.join("runner.ts"), suppressed).unwrap();
    git(root, &["init", "--quiet"]);
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Owned fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "--quiet",
            "-m",
            "owned suppressed pair",
        ],
    );
    std::fs::write(root.join("runner.ts"), HELPER).unwrap();
    command(root)
        .args(["--baseline-from-ref", "HEAD", "--fail-on-new", "0"])
        .args(["--reporters", "json", "--output"])
        .arg(root.join("report"))
        .assert()
        .code(1);
    let result = report(root);
    assert_eq!(result["findings"].as_array().unwrap().len(), 1, "{result}");
    assert_eq!(result["findings"][0]["baselined"], false, "{result}");
}

/// Runs a caller beside a one-hook runner with only `redundant-hook` enabled; returns the report.
fn redundant_hook_report(helper_hook: &str, caller_hook: &str) -> serde_json::Value {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(
        root.join("runner.ts"),
        format!("import {{ beforeEach }} from 'vitest';\nexport function install() {{\n  beforeEach(() => {{ {helper_hook} }});\n}}\n"),
    )
    .unwrap();
    std::fs::write(
        root.join("caller.test.ts"),
        format!("import {{ beforeEach, test, expect }} from 'vitest'; import {{ install }} from './runner';\ninstall();\nbeforeEach(() => {{ {caller_hook} }});\ntest('a', () => {{ expect(1).toBe(1); }});\n"),
    )
    .unwrap();
    let mut command = Command::cargo_bin("vitest-dedup").unwrap();
    command.arg(root);
    for rule in Rule::ALL {
        let level = if *rule == Rule::RedundantHook {
            "warning"
        } else {
            "off"
        };
        command.args(["--rule", &format!("{}={level}", rule.id())]);
    }
    command
        .args(["--reporters", "json", "--output"])
        .arg(root.join("report"))
        .assert()
        .code(predicates::prelude::predicate::in_iter([0, 1]));
    report(root)
}

#[test]
/// A helper-installed hook whose only effect is an opaque global property write is not compared
/// with the caller's: the property may have an accessor or be replaced between hook executions.
fn imported_hooks_with_opaque_global_writes_are_not_compared() {
    let same = redundant_hook_report("globalThis.ready = true;", "globalThis.ready = true;");
    assert_eq!(same["testsAnalyzed"], 1);
    assert_eq!(same["incomplete"], false);
    assert!(same["findings"].as_array().unwrap().is_empty(), "{same}");
}

/// Select one rule and a deterministic JSON output for delivery controls.
fn rule_report_command(root: &Path, selected: Rule) -> Command {
    let mut command = Command::cargo_bin("vitest-dedup").unwrap();
    command.arg(root);
    for rule in Rule::ALL {
        let level = if *rule != selected {
            "off"
        } else if selected == Rule::RedundantHook {
            "warning"
        } else {
            "error"
        };
        command.args(["--rule", &format!("{}={level}", rule.id())]);
    }
    command
        .args(["--reporters", "json", "--output"])
        .arg(root.join("report"));
    command
}

/// Require successful analysis with no claims from the selected rule.
fn assert_no_findings(root: &Path, rule: Rule) {
    rule_report_command(root, rule).assert().success();
    assert!(report(root)["findings"].as_array().unwrap().is_empty());
}

#[test]
/// Two runner-owned lexical overwrites form one proven chain and retain the helper
/// locations that a developer needs to remove either declaration.
fn imported_lexical_hooks_take_part_in_redundant_hook_comparison() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(
        root.join("runner.ts"),
        "import { beforeEach } from 'vitest';\nlet ready = 0;\nexport function install() {\n  beforeEach(() => { ready = 1; });\n  beforeEach(() => { ready = 1; });\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("caller.test.ts"),
        "import { test, expect } from 'vitest'; import { install } from './runner'; install(); test('a', () => { const sentinel = 1; expect(sentinel).toBe(1); });\n",
    )
    .unwrap();
    rule_report_command(root, Rule::RedundantHook)
        .assert()
        .code(0);
    let result = report(root);
    assert_eq!(result["testsAnalyzed"], 1, "{result}");
    assert_eq!(result["findings"].as_array().unwrap().len(), 1, "{result}");
    let finding = &result["findings"].as_array().unwrap()[0];
    let paths: Vec<_> = finding["locations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|location| location["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["runner.ts", "runner.ts"], "{result}");
}

#[test]
/// Imported lifecycle setup is part of each expanded test's comparison context.
fn imported_hook_timing_prevents_a_cross_file_duplicate() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for (name, timing) in [("first", "beforeEach"), ("second", "afterEach")] {
        std::fs::write(
            root.join(format!("{name}.ts")),
            format!(
                "import {{ {timing}, test, expect }} from 'vitest'; let ready = 0; export function run() {{ {timing}(() => {{ ready = 1; }}); test('same', () => {{ expect(ready).toBe(1); }}); }}"
            ),
        )
        .unwrap();
        std::fs::write(
            root.join(format!("{name}.test.ts")),
            format!("import {{ run }} from './{name}'; run();"),
        )
        .unwrap();
    }
    assert_no_findings(root, Rule::CrossFileDuplicate);
}

#[test]
/// A caller-local test inherits lifecycle context from an imported hook runner.
fn imported_hook_timing_prevents_a_duplicate_of_a_caller_test() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(
        root.join("runner.ts"),
        "import { beforeEach, afterEach } from 'vitest'; export let ready = 0; export function before() { beforeEach(() => { ready = 1; }); } export function after() { afterEach(() => { ready = 1; }); }",
    )
    .unwrap();
    for (file, install) in [("first", "before"), ("second", "after")] {
        std::fs::write(
            root.join(format!("{file}.test.ts")),
            format!(
                "import {{ test, expect }} from 'vitest'; import {{ {install} }} from './runner'; {install}(); test('same', async () => {{ expect((await import('./runner')).ready).toBe(1); }});"
            ),
        )
        .unwrap();
    }
    assert_no_findings(root, Rule::CrossFileDuplicate);
    let second = root.join("second.test.ts");
    let source = std::fs::read_to_string(&second).unwrap();
    std::fs::write(&second, source.replace("after", "before")).unwrap();
    rule_report_command(root, Rule::CrossFileDuplicate)
        .assert()
        .code(1);
    let positive = report(root);
    assert_eq!(positive["testsAnalyzed"], 2);
    assert!(positive["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| { finding["rule"] == "cross-file-duplicate" }));
}

#[test]
/// Lexically identical writes from different helper modules never share a local symbol identity.
fn imported_hook_effects_are_scoped_to_their_helper_module() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for (name, padding) in [("first", ""), ("second", "\n\n")] {
        std::fs::write(
            root.join(format!("{name}.ts")),
            format!(
                "import {{ beforeEach }} from 'vitest';{padding}let ready = 0; export function install() {{ beforeEach(() => {{ ready = 1; }}); }}"
            ),
        )
        .unwrap();
    }
    std::fs::write(
        root.join("caller.test.ts"),
        "import { test, expect } from 'vitest'; import { install as first } from './first'; import { install as second } from './second'; first(); second(); test('a', () => { expect(1).toBe(1); });",
    )
    .unwrap();
    assert_no_findings(root, Rule::RedundantHook);
}

#[test]
/// A runner that owns every test still supplies its hook chain and caller provenance.
fn imported_only_tests_build_hook_chains_and_keep_their_caller() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(
        root.join("runner.ts"),
        "import { beforeEach, test, expect } from 'vitest'; let ready = 0; export function run() { beforeEach(() => { ready = 1; }); beforeEach(() => { ready = 1; }); test('a', () => { expect(ready).toBe(1); }); }",
    )
    .unwrap();
    for caller in ["first.test.ts", "second.test.ts"] {
        std::fs::write(root.join(caller), "import { run } from './runner'; run();").unwrap();
    }
    rule_report_command(root, Rule::RedundantHook)
        .assert()
        .code(0);
    let findings = report(root)["findings"].as_array().unwrap().clone();
    assert_eq!(findings.len(), 1, "{findings:?}");
    for participant in findings[0]["registrationFiles"].as_array().unwrap() {
        assert_eq!(
            participant,
            &serde_json::json!(["first.test.ts", "second.test.ts"])
        );
    }
}
