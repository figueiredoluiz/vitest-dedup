//! A run that cannot resolve imports because a file or directory it needs is absent says so.
//!
//! Final-outcome contract: the terminal banner, the JSON/JSONL/SARIF/HTML reports and the exit
//! code under `--fail-on-incomplete`, over projects built on disk at run time (a `node_modules`
//! directory cannot live in tracked fixtures).

use std::fs;
use std::path::Path;

const DUPLICATE_PAIR: &str = "\
import { test, expect } from 'vitest'
import { a } from './helpers/a'
import { b } from './helpers/b'

test('first', () => { expect(a('dashboard')).toBe(9) })
test('second', () => { expect(b('dashboard')).toBe(9) })
";

/// Writes `contents` under `root`, creating parent directories.
fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A project whose two tests are a duplicate pair when every import resolves.
fn project() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "helpers/a.ts",
        "export const a = (x: string) => x.length\n",
    );
    write(
        root,
        "helpers/b.ts",
        "export const b = (x: string) => x.length\n",
    );
    write(root, "twin.test.ts", DUPLICATE_PAIR);
    directory
}

/// A project whose tests import a declared package, so `node_modules` decides what resolves.
fn package_project(lockfile: Option<&str>, installed: Option<bool>) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{ "name": "app", "devDependencies": { "left-pad": "1.0.0" } }"#,
    );
    write(
        root,
        "twin.test.ts",
        "import { test, expect } from 'vitest'\nimport pad from 'left-pad'\n\
         test('first', () => { expect(pad('a')).toBe('a') })\n\
         test('second', () => { expect(pad('a')).toBe('a') })\n",
    );
    if let Some(lockfile) = lockfile {
        write(root, lockfile, "");
    }
    match installed {
        Some(true) => write(
            root,
            "node_modules/left-pad/index.js",
            "module.exports = (s) => s\n",
        ),
        Some(false) => fs::create_dir_all(root.join("node_modules")).unwrap(),
        None => {}
    }
    directory
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    json: serde_json::Value,
}

/// Runs the analyzer over `root` with the terminal and JSON reporters and collects the outcome.
fn run(root: &Path, extra: &[&str]) -> Run {
    let stem = root.join("out/report");
    let output = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .current_dir(root)
        .arg(root)
        .args(["--reporters", "terminal,json", "--output"])
        .arg(&stem)
        .args(extra)
        .output()
        .unwrap();
    let json = serde_json::from_slice(&fs::read(stem.with_extension("json")).unwrap()).unwrap();
    Run {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        json,
    }
}

/// The `missingPrerequisites` entries of a run.
fn prerequisites(run: &Run) -> &Vec<serde_json::Value> {
    run.json["incompleteCauses"]["missingPrerequisites"]
        .as_array()
        .expect("incompleteCauses.missingPrerequisites is an array")
}

/// A run that reports no missing prerequisite and is complete.
fn assert_clean(run: &Run) {
    assert!(prerequisites(run).is_empty(), "{:?}", run.json);
    assert_eq!(run.json["incomplete"], false, "{:?}", run.json);
    assert!(!run.stdout.contains("is missing"), "{}", run.stdout);
}

/// An absent `.nuxt` extends target is named with `nuxt prepare` and the imports it left unresolved.
#[test]
#[cfg_attr(
    windows,
    ignore = "tsconfig discovery from a canonical importer does not work on Windows yet"
)]
fn absent_extends_target_is_named_with_its_generator() {
    let directory = project();
    let root = directory.path();
    write(
        root,
        "tsconfig.json",
        "{\n  // generated\n  \"extends\": \"./.nuxt/tsconfig.json\"\n}\n",
    );
    write(
        root,
        "package.json",
        r#"{ "devDependencies": { "nuxt": "3.0.0" } }"#,
    );

    let report = run(root, &[]);

    assert_eq!(report.json["testsAnalyzed"], 2);
    assert_eq!(
        report.json["findings"].as_array().unwrap().len(),
        0,
        "{:?}",
        report.json
    );
    assert_eq!(report.json["incomplete"], true);
    let entries = prerequisites(&report);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["kind"], "tsconfig-extends");
    assert_eq!(entries[0]["path"], ".nuxt/tsconfig.json");
    assert_eq!(entries[0]["fix"], "nuxt prepare");
    assert_eq!(entries[0]["unresolvedImports"], 3);
    assert!(
        report
            .stdout
            .contains("INCOMPLETE: some tests could not be compared"),
        "{}",
        report.stdout
    );
    assert!(
        report.stdout.contains(
            "`.nuxt/tsconfig.json` is missing: a tsconfig extends it for path aliases and \
             compiler options, and without it no import under that tsconfig resolves \
             (3 import(s) stayed unresolved). Run `nuxt prepare`."
        ),
        "{}",
        report.stdout
    );
    assert_eq!(report.code, Some(0), "{}", report.stderr);
}

/// The same project with the target present: no notice, and the duplicate is found.
#[test]
#[cfg_attr(
    windows,
    ignore = "tsconfig discovery from a canonical importer does not work on Windows yet"
)]
fn present_extends_target_is_silent_and_the_duplicate_is_found() {
    let directory = project();
    let root = directory.path();
    write(
        root,
        "tsconfig.json",
        r#"{ "extends": "./.nuxt/tsconfig.json" }"#,
    );
    write(
        root,
        "package.json",
        r#"{ "devDependencies": { "nuxt": "3.0.0" } }"#,
    );
    write(root, ".nuxt/tsconfig.json", "{}");

    let report = run(root, &[]);

    assert_eq!(
        report.json["findings"].as_array().unwrap().len(),
        1,
        "{:?}",
        report.json
    );
    assert_clean(&report);
}

/// An absent target with no known generator says to generate or restore it.
#[test]
#[cfg_attr(
    windows,
    ignore = "tsconfig discovery from a canonical importer does not work on Windows yet"
)]
fn absent_extends_target_without_a_known_generator_says_generate_or_restore() {
    let directory = project();
    let root = directory.path();
    write(
        root,
        "tsconfig.json",
        r#"{ "extends": "./config/base.json" }"#,
    );

    let report = run(root, &[]);

    let entries = prerequisites(&report);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["path"], "config/base.json");
    assert_eq!(entries[0]["fix"], "generate or restore this file");
    assert!(
        report.stdout.contains("Generate or restore this file."),
        "{}",
        report.stdout
    );
    assert!(!report.stdout.contains("nuxt prepare"), "{}", report.stdout);
}

/// `nuxt prepare` needs a `.nuxt` path, not only a `nuxt` dependency.
#[test]
#[cfg_attr(
    windows,
    ignore = "tsconfig discovery from a canonical importer does not work on Windows yet"
)]
fn nuxt_generator_is_named_only_for_a_nuxt_generated_path() {
    let directory = project();
    let root = directory.path();
    write(
        root,
        "tsconfig.json",
        r#"{ "extends": "./config/base.json" }"#,
    );
    write(
        root,
        "package.json",
        r#"{ "devDependencies": { "nuxt": "3.0.0" } }"#,
    );

    let report = run(root, &[]);

    assert_eq!(
        prerequisites(&report)[0]["fix"],
        "generate or restore this file"
    );
}

/// A `nuxt.config.*` file is enough evidence for `nuxt prepare`.
#[test]
#[cfg_attr(
    windows,
    ignore = "tsconfig discovery from a canonical importer does not work on Windows yet"
)]
fn nuxt_config_file_names_the_generator_without_a_dependency() {
    let directory = project();
    let root = directory.path();
    write(
        root,
        "tsconfig.json",
        r#"{ "extends": "./.nuxt/tsconfig.json" }"#,
    );
    write(root, "nuxt.config.ts", "export default {}\n");

    let report = run(root, &[]);

    assert_eq!(prerequisites(&report)[0]["fix"], "nuxt prepare");
}

/// A target that exists but is invalid is reported as unreadable, not absent.
#[test]
#[cfg_attr(
    windows,
    ignore = "tsconfig discovery from a canonical importer does not work on Windows yet"
)]
fn unreadable_extends_target_is_reported_as_unreadable() {
    let directory = project();
    let root = directory.path();
    write(root, "tsconfig.json", r#"{ "extends": "./base.json" }"#);
    write(root, "base.json", "{ \"compilerOptions\": ");

    let report = run(root, &[]);

    let entries = prerequisites(&report);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["kind"], "tsconfig-unreadable");
    assert_eq!(entries[0]["path"], "base.json");
    assert_eq!(report.json["incomplete"], true);
    assert!(
        report
            .stdout
            .contains("`base.json` exists but could not be read as a tsconfig"),
        "{}",
        report.stdout
    );
}

/// Negative control: a tsconfig without `extends` produces no notice.
#[test]
fn tsconfig_without_extends_never_alarms() {
    let directory = project();
    let root = directory.path();
    write(
        root,
        "tsconfig.json",
        r#"{ "compilerOptions": { "strict": true } }"#,
    );

    let report = run(root, &[]);

    assert_eq!(report.json["findings"].as_array().unwrap().len(), 1);
    assert_clean(&report);
}

/// Each lockfile kind implies its install command in the field and the sentence.
#[test]
fn missing_node_modules_names_the_install_command_the_lockfile_implies() {
    for (lockfile, command) in [
        ("pnpm-lock.yaml", "pnpm install"),
        ("package-lock.json", "npm ci"),
        ("yarn.lock", "yarn install"),
        ("bun.lock", "bun install"),
    ] {
        let directory = package_project(Some(lockfile), None);
        let report = run(directory.path(), &[]);

        let entries = prerequisites(&report);
        assert_eq!(entries.len(), 1, "{lockfile}: {entries:?}");
        assert_eq!(entries[0]["kind"], "node-modules");
        assert_eq!(entries[0]["path"], "node_modules");
        assert_eq!(entries[0]["fix"], command);
        assert_eq!(entries[0]["unresolvedImports"], 1);
        assert_eq!(report.json["incomplete"], true);
        assert!(
            report.stdout.contains(&format!(
                "`node_modules` is missing or empty although package.json lists dependencies: \
                 imports of installed packages cannot be resolved (1 import(s) stayed \
                 unresolved). Run `{command}`."
            )),
            "{lockfile}: {}",
            report.stdout
        );
    }
}

/// Without a lockfile no package manager is named.
#[test]
fn missing_node_modules_without_a_lockfile_names_no_manager() {
    let directory = package_project(None, None);
    let report = run(directory.path(), &[]);

    assert_eq!(prerequisites(&report)[0]["fix"], "install the dependencies");
    assert!(
        report.stdout.contains("Install the dependencies."),
        "{}",
        report.stdout
    );
}

/// An empty `node_modules` directory is treated as missing.
#[test]
fn empty_node_modules_counts_as_missing() {
    let directory = package_project(Some("pnpm-lock.yaml"), Some(false));
    let report = run(directory.path(), &[]);

    assert_eq!(prerequisites(&report).len(), 1, "{:?}", report.json);
}

/// An installed package resolves: no notice, and the duplicate is found.
#[test]
fn installed_node_modules_is_silent() {
    let directory = package_project(Some("pnpm-lock.yaml"), Some(true));
    let report = run(directory.path(), &[]);

    assert_eq!(
        report.json["findings"].as_array().unwrap().len(),
        1,
        "{:?}",
        report.json
    );
    assert_clean(&report);
}

/// Node resolves upward, so an ancestor `node_modules` satisfies the project.
#[test]
fn node_modules_in_an_ancestor_directory_counts_as_installed() {
    let parent = package_project(Some("pnpm-lock.yaml"), Some(true));
    let child = parent.path().join("packages/app");
    fs::create_dir_all(&child).unwrap();
    write(
        &child,
        "package.json",
        r#"{ "dependencies": { "left-pad": "1.0.0" } }"#,
    );
    fs::copy(
        parent.path().join("twin.test.ts"),
        child.join("twin.test.ts"),
    )
    .unwrap();
    // The ancestor installs a different package, so `left-pad` fails to resolve and only the
    // ancestor's non-empty `node_modules` keeps the run from being reported.
    fs::remove_dir_all(parent.path().join("node_modules/left-pad")).unwrap();
    write(
        parent.path(),
        "node_modules/other/index.js",
        "module.exports = 1\n",
    );

    let report = run(&child, &[]);

    assert_eq!(
        report.json["findings"].as_array().unwrap().len(),
        1,
        "{:?}",
        report.json
    );
    assert_clean(&report);
}

/// Negative control: no listed dependencies, no notice.
#[test]
fn project_without_dependencies_never_alarms_about_node_modules() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "package.json", r#"{ "name": "lib" }"#);
    write(
        root,
        "twin.test.ts",
        "import { test, expect } from 'vitest'\nimport pad from 'left-pad'\n\
         test('first', () => { expect(pad('a')).toBe('a') })\n\
         test('second', () => { expect(pad('a')).toBe('a') })\n",
    );

    let report = run(root, &[]);

    assert_clean(&report);
}

/// Negative control: listed dependencies and no unresolved package import, no notice.
#[test]
fn dependencies_with_only_relative_imports_never_alarm() {
    let directory = project();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{ "devDependencies": { "vitest": "3.0.0" } }"#,
    );
    write(root, "pnpm-lock.yaml", "");

    let report = run(root, &[]);

    assert_eq!(
        report.json["findings"].as_array().unwrap().len(),
        1,
        "{:?}",
        report.json
    );
    assert_clean(&report);
}

/// JSON, JSONL, SARIF and HTML carry the entry, with `schemaVersion` unchanged.
#[test]
fn every_reporter_carries_the_missing_prerequisite() {
    let directory = package_project(Some("pnpm-lock.yaml"), None);
    let root = directory.path();
    let stem = root.join("all/report");
    let output = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .current_dir(root)
        .arg(root)
        .args(["--reporters", "json,sarif,html", "--output"])
        .arg(&stem)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(stem.with_extension("json")).unwrap()).unwrap();
    assert_eq!(json["schemaVersion"], "0.1");
    let jsonl = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .current_dir(root)
        .arg(root)
        .args(["--reporters", "jsonl"])
        .output()
        .unwrap();
    let jsonl = String::from_utf8(jsonl.stdout).unwrap();
    let summary: serde_json::Value = jsonl
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|record| record["type"] == "summary")
        .unwrap();
    assert_eq!(summary["schemaVersion"], "0.1");
    assert_eq!(
        summary["incompleteCauses"]["missingPrerequisites"][0]["fix"],
        "pnpm install"
    );
    let sarif: serde_json::Value =
        serde_json::from_slice(&fs::read(stem.with_extension("sarif")).unwrap()).unwrap();
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["properties"]["incompleteCauses"]
            ["missingPrerequisites"][0]["kind"],
        "node-modules"
    );
    let html = fs::read_to_string(stem.with_extension("html")).unwrap();
    // The sentence is built from the cause by `reasons()` and is not part of the JSON island.
    assert!(
        html.contains("is missing or empty although package.json lists dependencies"),
        "html banner carries the sentence"
    );
    assert!(
        html.contains("Run `pnpm install`."),
        "html banner carries the fix"
    );
}

/// A missing prerequisite is a `reads` cause, not a `limits` one.
#[test]
fn fail_on_incomplete_reads_fails_on_a_missing_prerequisite() {
    let directory = package_project(Some("pnpm-lock.yaml"), None);
    let failing = run(directory.path(), &["--fail-on-incomplete=reads"]);
    assert_eq!(failing.code, Some(2), "{}", failing.stderr);
    let limits = run(directory.path(), &["--fail-on-incomplete=limits"]);
    assert_eq!(
        limits.code,
        Some(1),
        "the quality gate, not the incomplete gate: {}",
        limits.stderr
    );
}

/// Node core modules need no installation, so importing them alone never counts as a missing package.
#[test]
fn node_core_imports_alone_never_alarm() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{ "devDependencies": { "left-pad": "1.0.0" } }"#,
    );
    write(
        root,
        "twin.test.ts",
        "import { test, expect } from 'vitest'\nimport { join } from 'path'\nimport fs from 'fs/promises'\n\
         test('first', () => { expect(join('a')).toBe('a'); void fs })\n\
         test('second', () => { expect(join('b')).toBe('b'); void fs })\n",
    );

    assert_clean(&run(root, &[]));
}

/// The nearest `package.json` above the analyzed directory governs it.
#[test]
fn dependencies_listed_in_a_parent_manifest_are_honoured() {
    let parent = package_project(Some("pnpm-lock.yaml"), None);
    let child = parent.path().join("tests");
    fs::create_dir_all(&child).unwrap();
    fs::rename(
        parent.path().join("twin.test.ts"),
        child.join("twin.test.ts"),
    )
    .unwrap();

    let report = run(&child, &[]);

    assert_eq!(
        prerequisites(&report)[0]["kind"],
        "node-modules",
        "{:?}",
        report.json
    );
}
