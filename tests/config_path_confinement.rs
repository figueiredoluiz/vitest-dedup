//! Paths named by Vitest and TypeScript configuration stay inside the analysis root.
//!
//! Each scenario runs the binary over a root that has a sibling directory outside it. The
//! configuration either points at the sibling (refused: the run is incomplete and says why) or at a
//! directory inside the root (the control: complete, no refusal). The two differ in one path, so a
//! fix that refused everything, or that stopped reading configuration, fails the control.

use std::fs;
use std::path::{Path, PathBuf};

const TESTS: &str = "import { expect, test } from 'vitest'\n\
test('adds two numbers', () => { expect(1 + 2).toBe(3) })\n\
test('sums the same pair again', () => { expect(1 + 2).toBe(3) })\n";

const REFUSAL: &str = "outside the analysis root";

/// What the binary reported for one run.
struct Outcome {
    incomplete: bool,
    tests: u64,
    stderr: String,
    /// The raw summary record, so a failed assertion shows the causes the run reported.
    summary: String,
    /// Every record the run wrote, for the same reason.
    records: String,
}

/// A root with a sibling `outside` directory, both under one temporary directory.
struct Layout {
    _guard: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
}

/// Creates the layout with the duplicate-pair test file in the root.
fn layout() -> Layout {
    let guard = tempfile::tempdir().unwrap();
    let root = guard.path().join("root");
    let outside = guard.path().join("outside");
    write(&root, "arith.test.ts", TESTS);
    fs::create_dir_all(&outside).unwrap();
    Layout {
        _guard: guard,
        root,
        outside,
    }
}

/// Moves the duplicate-pair test file from the root into `pkg/`, where a project scoped to `pkg`
/// governs it.
fn move_tests_into_pkg(layout: &Layout) {
    fs::remove_file(layout.root.join("arith.test.ts")).unwrap();
    write(&layout.root, "pkg/arith.test.ts", TESTS);
}

/// Writes `contents` to `name` under `directory`, creating parents.
fn write(directory: &Path, name: &str, contents: &str) {
    let path = directory.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Runs the analyzer over `root` and reads the summary record.
fn run(root: &Path) -> Outcome {
    let output = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(root)
        .args(["--reporters", "jsonl", "--no-metrics"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let summary = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|record| record["type"] == "summary")
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    Outcome {
        incomplete: summary["incomplete"].as_bool().unwrap(),
        tests: summary["testsAnalyzed"].as_u64().unwrap(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        summary: summary.to_string(),
        records: stdout.into_owned(),
    }
}

/// The run refused a path: incomplete, the tests still extracted, and the reason on stderr.
fn assert_refused(outcome: &Outcome, why: &str) {
    assert!(
        outcome.incomplete,
        "{why}: expected an incomplete run\n{}\nsummary: {}\nrecords:\n{}",
        outcome.stderr, outcome.summary, outcome.records
    );
    assert_eq!(
        outcome.tests, 2,
        "{why}: {}\nsummary: {}",
        outcome.stderr, outcome.summary
    );
    assert!(
        outcome.stderr.contains(REFUSAL),
        "{why}: no refusal on stderr\n{}",
        outcome.stderr
    );
}

/// The run read its configuration: complete, no refusal anywhere.
fn assert_kept(outcome: &Outcome, why: &str) {
    assert!(
        !outcome.incomplete,
        "{why}: expected a complete run\n{}\nsummary: {}",
        outcome.stderr, outcome.summary
    );
    assert_eq!(
        outcome.tests, 2,
        "{why}: {}\nsummary: {}",
        outcome.stderr, outcome.summary
    );
    assert!(
        !outcome.stderr.contains(REFUSAL),
        "{why}: {}",
        outcome.stderr
    );
}

/// Creates a symlink to a file or directory, or reports why not, so a caller can skip visibly.
fn try_symlink(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let created = if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
    match created {
        Ok(()) => true,
        Err(error) => {
            eprintln!("skipped: cannot create a symlink here: {error}");
            false
        }
    }
}

/// `path` spelled for a JavaScript string literal.
fn js(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

const PROJECT: &str = "export default { test: { name: 'pkg' } };\n";

/// A `test.projects` entry is confined, whether written with `..`, absolutely, or as a glob.
#[test]
fn projects_entries_outside_the_root_are_refused() {
    let layout = layout();
    move_tests_into_pkg(&layout);
    write(&layout.outside, "vitest.config.ts", PROJECT);
    write(&layout.root, "pkg/vitest.config.ts", PROJECT);
    let absolute = js(&layout.outside);
    for entry in [
        "../outside",
        absolute.as_str(),
        "../out*",
        "../outside/vitest.config.ts",
    ] {
        write(
            &layout.root,
            "vitest.config.ts",
            &format!("export default {{ test: {{ projects: ['{entry}'] }} }};\n"),
        );
        assert_refused(&run(&layout.root), entry);
    }
    for entry in ["pkg", "./pkg", "pk*"] {
        write(
            &layout.root,
            "vitest.config.ts",
            &format!("export default {{ test: {{ projects: ['{entry}'] }} }};\n"),
        );
        assert_kept(&run(&layout.root), entry);
    }
}

/// A symlinked project directory is judged by where it points.
#[test]
fn projects_entries_through_symlinks_are_judged_by_their_target() {
    let layout = layout();
    move_tests_into_pkg(&layout);
    write(&layout.outside, "vitest.config.ts", PROJECT);
    write(&layout.root, "pkg/vitest.config.ts", PROJECT);
    if !try_symlink(&layout.outside, &layout.root.join("escape")) {
        return;
    }
    assert!(try_symlink(
        &layout.root.join("pkg"),
        &layout.root.join("alias")
    ));
    write(
        &layout.root,
        "vitest.config.ts",
        "export default { test: { projects: ['escape'] } };\n",
    );
    assert_refused(&run(&layout.root), "symlink out");
    write(
        &layout.root,
        "vitest.config.ts",
        "export default { test: { projects: ['alias'] } };\n",
    );
    // Files are discovered under their real path `pkg/`, which a project spelled `alias/` does not
    // claim, so the run may be incomplete; what must not happen is a refusal.
    let outcome = run(&layout.root);
    assert!(!outcome.stderr.contains(REFUSAL), "{}", outcome.stderr);
}

/// A root reached through a symlink resolves its inside entries.
#[test]
fn a_root_reached_through_a_symlink_keeps_its_inside_paths() {
    let layout = layout();
    move_tests_into_pkg(&layout);
    write(&layout.root, "pkg/vitest.config.ts", PROJECT);
    let alias = layout.root.parent().unwrap().join("alias-root");
    if !try_symlink(&layout.root, &alias) {
        return;
    }
    // A canonical Windows path carries a `\\?\` prefix, whose `?` reads as a glob metacharacter,
    // so the absolute spelling is only tried elsewhere.
    let absolute = (!cfg!(windows)).then(|| js(&layout.root.canonicalize().unwrap().join("pkg")));
    let entries: Vec<String> = ["pkg".to_owned()].into_iter().chain(absolute).collect();
    for entry in &entries {
        write(
            &layout.root,
            "vitest.config.ts",
            &format!("export default {{ test: {{ projects: ['{entry}'] }} }};\n"),
        );
        assert_kept(&run(&alias), entry);
    }
}

/// An inline project's string `extends` is confined; one folding back inside is kept.
#[test]
fn extends_outside_the_root_is_refused() {
    let layout = layout();
    write(
        &layout.outside,
        "base.ts",
        "export default { test: { globals: true } };\n",
    );
    write(
        &layout.root,
        "shared/base.ts",
        "export default { test: { globals: true } };\n",
    );
    let absolute = js(&layout.outside.join("base.ts"));
    for target in ["../outside/base.ts", absolute.as_str()] {
        write(
            &layout.root,
            "vitest.config.ts",
            &format!(
                "export default {{ test: {{ projects: [{{ extends: '{target}', test: {{ name: 'x' }} }}] }} }};\n"
            ),
        );
        assert_refused(&run(&layout.root), target);
    }
    for target in ["./shared/base.ts", "./shared/../shared/base.ts"] {
        write(
            &layout.root,
            "vitest.config.ts",
            &format!(
                "export default {{ test: {{ projects: [{{ extends: '{target}', test: {{ name: 'x' }} }}] }} }};\n"
            ),
        );
        assert_kept(&run(&layout.root), target);
    }
}

/// A refused `mergeConfig` base is reported whatever consumes the merged fields: a project that
/// sets everything itself, or `extends: false`, discards the inherited values the refusal was
/// first recorded in.
#[test]
fn an_outside_merge_base_is_reported_even_when_its_fields_are_discarded() {
    let layout = layout();
    move_tests_into_pkg(&layout);
    let base = "export default { test: { globals: true } };\n";
    write(&layout.outside, "base.ts", base);
    write(&layout.root, "shared/base.ts", base);
    let merged = |specifier: &str| {
        format!(
            "import {{ mergeConfig }} from 'vitest/config';\nimport base from '{specifier}';\nexport default mergeConfig(base, "
        )
    };
    for (specifier, refused) in [("../outside/base", true), ("./shared/base", false)] {
        let _ = fs::remove_file(layout.root.join("pkg/vitest.config.ts"));
        let head = merged(specifier);
        // The root config's own base, with an inline project that discards what it inherited.
        write(
            &layout.root,
            "vitest.config.ts",
            &format!(
                "{head}{{ test: {{ projects: [{{ extends: false, test: {{ {EXPLICIT_SETTINGS} }} }}] }} }});\n"
            ),
        );
        let outcome = run(&layout.root);
        assert_eq!(
            outcome.stderr.contains(REFUSAL),
            refused,
            "root config, {specifier}\n{}",
            outcome.stderr
        );
        assert!(outcome.incomplete || !refused, "root config, {specifier}");
        // A project config file whose own base is refused, and the project sets every field.
        let head = merged(&format!("../{specifier}"));
        write(
            &layout.root,
            "pkg/vitest.config.ts",
            &format!("{head}{{ test: {{ {EXPLICIT_SETTINGS} }} }});\n"),
        );
        write(
            &layout.root,
            "vitest.config.ts",
            "export default { test: { projects: ['./pkg/vitest.config.ts'] } };\n",
        );
        let outcome = run(&layout.root);
        assert_eq!(
            outcome.stderr.contains(REFUSAL),
            refused,
            "project file, {specifier}\n{}",
            outcome.stderr
        );
        // The same file found by a `glob.sync` project list.
        write(
            &layout.root,
            "vitest.config.ts",
            "import { glob } from 'glob';\nexport default { test: { projects: glob.sync('./pkg/vitest.config.ts') } };\n",
        );
        let outcome = run(&layout.root);
        assert_eq!(
            outcome.stderr.contains(REFUSAL),
            refused,
            "glob project list, {specifier}\n{}",
            outcome.stderr
        );
        // The same file as an `extends` target.
        write(
            &layout.root,
            "vitest.config.ts",
            &format!(
                "export default {{ test: {{ projects: [{{ extends: './pkg/vitest.config.ts', test: {{ {EXPLICIT_SETTINGS} }} }}] }} }};\n"
            ),
        );
        let outcome = run(&layout.root);
        assert_eq!(
            outcome.stderr.contains(REFUSAL),
            refused,
            "extends target, {specifier}\n{}",
            outcome.stderr
        );
    }
}

/// A nested configuration with a refused base is reported even when the tests it governs are
/// never emitted: the report names the refusal, not only the tests it excluded.
#[test]
fn a_nested_config_with_an_outside_base_is_reported_without_any_test_to_exclude() {
    let layout = layout();
    write(
        &layout.outside,
        "base.ts",
        "export default { test: { environment: 'node' } };\n",
    );
    write(
        &layout.root,
        "pkg/vitest.config.ts",
        "import { mergeConfig } from 'vitest/config';\nimport base from '../../outside/base';\nexport default mergeConfig(base, { test: {} });\n",
    );
    write(
        &layout.root,
        "pkg/empty.test.ts",
        "import { test } from 'vitest';\ntest.each([])('empty', () => {});\n",
    );
    let outcome = run(&layout.root);
    assert!(outcome.incomplete, "{}", outcome.stderr);
    assert!(
        outcome.stderr.contains(REFUSAL),
        "no refusal on stderr\n{}",
        outcome.stderr
    );
}

/// A `mergeConfig` base import is confined; one folding back inside is kept.
#[test]
fn merge_config_imports_outside_the_root_are_refused() {
    let layout = layout();
    let base = "export default { test: { include: ['**/*.test.ts'] } };\n";
    write(&layout.outside, "base.ts", base);
    write(&layout.root, "shared/base.ts", base);
    for specifier in ["../outside/base", "./shared/../../outside/base"] {
        write(
            &layout.root,
            "vitest.config.ts",
            &format!(
                "import {{ mergeConfig }} from 'vitest/config';\nimport base from '{specifier}';\nexport default mergeConfig(base, {{ test: {{ name: 'merged' }} }});\n"
            ),
        );
        assert_refused(&run(&layout.root), specifier);
    }
    for specifier in ["./shared/base", "./shared/../shared/base"] {
        write(
            &layout.root,
            "vitest.config.ts",
            &format!(
                "import {{ mergeConfig }} from 'vitest/config';\nimport base from '{specifier}';\nexport default mergeConfig(base, {{ test: {{ name: 'merged' }} }});\n"
            ),
        );
        assert_kept(&run(&layout.root), specifier);
    }
}

/// An inline project's `root`/`dir` is confined; one folding back inside is kept.
#[test]
fn project_root_and_dir_outside_the_root_are_refused() {
    let layout = layout();
    move_tests_into_pkg(&layout);
    write(&layout.outside, "keep.txt", "");
    let absolute = js(&layout.outside);
    for key in ["root", "dir"] {
        for declared in ["../outside", absolute.as_str()] {
            write(
                &layout.root,
                "vitest.config.ts",
                &format!(
                    "export default {{ test: {{ projects: [{{ test: {{ name: 'p', {key}: '{declared}' }} }}] }} }};\n"
                ),
            );
            assert_refused(&run(&layout.root), declared);
        }
        for declared in ["./pkg", "./pkg/../pkg"] {
            write(
                &layout.root,
                "vitest.config.ts",
                &format!(
                    "export default {{ test: {{ projects: [{{ test: {{ name: 'p', {key}: '{declared}' }} }}] }} }};\n"
                ),
            );
            assert_kept(&run(&layout.root), declared);
        }
    }
}

/// A tsconfig `paths` alias is confined; one that stays inside resolves its helpers.
#[test]
fn tsconfig_paths_outside_the_root_are_refused() {
    let layout = layout();
    fs::remove_file(layout.root.join("arith.test.ts")).unwrap();
    let helper = "import { expect } from 'vitest'\nexport function check(value: number) {\n  const doubled = value * 2\n  expect(doubled).toBe(6)\n}\n";
    write(&layout.outside, "check.ts", helper);
    write(&layout.root, "helpers/check.ts", helper);
    let tests = "import { test } from 'vitest'\nimport { check } from '@h/check'\n\
test('doubles three', () => { check(3) })\ntest('doubles three again', () => { check(3) })\n";
    write(&layout.root, "doubles.test.ts", tests);
    let config = |target: &str| {
        format!(
            "{{ \"compilerOptions\": {{ \"baseUrl\": \".\", \"paths\": {{ \"@h/*\": [\"{target}\"] }} }} }}\n"
        )
    };
    write(&layout.root, "tsconfig.json", &config("../outside/*"));
    // The binary canonicalizes its root, which is a verbatim `\\?\` path on Windows, and the
    // resolver does not apply tsconfig `paths` from such a path at all (see
    // `tsconfig_alias_is_judged_from_canonical_paths`): the alias never resolves there, so nothing
    // outside is read and there is nothing to refuse.
    if !cfg!(windows) {
        assert_refused(&run(&layout.root), "tsconfig paths out");
    }
    write(&layout.root, "tsconfig.json", &config("helpers/*"));
    assert_kept(&run(&layout.root), "tsconfig paths in");
}

/// A relative import that leaves the root is not configuration and keeps resolving.
#[test]
fn a_relative_import_outside_the_root_is_not_refused() {
    let layout = layout();
    fs::remove_file(layout.root.join("arith.test.ts")).unwrap();
    write(
        &layout.outside,
        "check.ts",
        "import { expect } from 'vitest'\nexport function check(value: number) {\n  expect(value * 2).toBe(6)\n}\n",
    );
    write(
        &layout.root,
        "doubles.test.ts",
        "import { test } from 'vitest'\nimport { check } from '../outside/check'\n\
test('doubles three', () => { check(3) })\ntest('doubles three again', () => { check(3) })\n",
    );
    assert_kept(&run(&layout.root), "relative import");
}

/// The analysis root's own configuration file is judged by where it points: a symlink out of the
/// tree is refused, one to a file inside is read.
#[test]
fn a_root_configuration_symlinked_out_of_the_tree_is_refused() {
    let layout = layout();
    // An outside configuration that would take effect if it were read: it narrows `include` so the
    // root's tests are no longer discovered.
    write(
        &layout.outside,
        "base.ts",
        "export default { test: { include: ['**/*.nomatch.ts'] } };\n",
    );
    write(
        &layout.root,
        "shared.config.ts",
        "export default { test: { environment: 'node' } };\n",
    );
    let config = layout.root.join("vitest.config.ts");
    if !try_symlink(&layout.outside.join("base.ts"), &config) {
        return;
    }
    assert_refused(&run(&layout.root), "root config symlinked out");

    fs::remove_file(&config).unwrap();
    assert!(try_symlink(&layout.root.join("shared.config.ts"), &config));
    assert_kept(&run(&layout.root), "root config symlinked within the tree");
}

/// The legacy workspace file is confined the same way as the root configuration.
#[test]
fn a_workspace_file_symlinked_out_of_the_tree_is_refused() {
    let layout = layout();
    write(&layout.outside, "workspace.ts", "export default ['pkg'];\n");
    if !try_symlink(
        &layout.outside.join("workspace.ts"),
        &layout.root.join("vitest.workspace.ts"),
    ) {
        return;
    }
    assert_refused(&run(&layout.root), "workspace file symlinked out");
}

/// A glob that also matches a symlink out of the tree reports it, even when another match resolves.
#[test]
fn a_glob_match_that_is_a_symlink_out_of_the_tree_is_reported() {
    let layout = layout();
    move_tests_into_pkg(&layout);
    write(&layout.outside, "vitest.config.ts", PROJECT);
    write(&layout.root, "pkg/vitest.config.ts", PROJECT);
    if !try_symlink(&layout.outside, &layout.root.join("pkz")) {
        return;
    }
    write(
        &layout.root,
        "vitest.config.ts",
        "export default { test: { projects: ['pk*'] } };\n",
    );
    assert_refused(&run(&layout.root), "glob matching a symlink out");
}

/// Every setting an inline project can inherit, spelled out so nothing depends on the base file.
const EXPLICIT_SETTINGS: &str = "name: 'p', globals: false, environment: 'node', \
include: ['**/*.test.ts'], exclude: [], includeSource: [], \
benchmark: { include: [], exclude: [], includeSource: [] }, typecheck: { enabled: false }";

/// A refused `extends` is reported whatever the project sets itself: the settings that would have
/// come from the refused file being explicit must not make the refusal vanish.
#[test]
fn an_outside_extends_is_reported_even_when_the_project_sets_everything_itself() {
    let layout = layout();
    write(
        &layout.outside,
        "base.ts",
        "export default { test: { globals: true } };\n",
    );
    write(
        &layout.root,
        "shared/base.ts",
        "export default { test: { globals: true } };\n",
    );
    for (target, refused) in [("../outside/base.ts", true), ("./shared/base.ts", false)] {
        write(
            &layout.root,
            "vitest.config.ts",
            &format!(
                "export default {{ test: {{ projects: [{{ extends: '{target}', test: {{ {EXPLICIT_SETTINGS} }} }}] }} }};\n"
            ),
        );
        let outcome = run(&layout.root);
        if refused {
            assert_refused(&outcome, target);
        } else {
            assert_kept(&outcome, target);
        }
    }
}
