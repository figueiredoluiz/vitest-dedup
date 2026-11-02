use super::*;

/// A root config listing `pkg/*` projects through `glob.sync` with `options` as its second
/// argument, and a `globals: true` project in each of `pkg/a` and `pkg/b`.
fn two_projects(options: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        &format!(
            "import {{ glob }} from 'glob'\nconst entries = glob.sync('./pkg/*/vitest.config.ts'{options})\nexport default defineConfig({{ test: {{ projects: entries }} }});"
        ),
    );
    for name in ["a", "b"] {
        write(
            directory.path(),
            &format!("pkg/{name}/vitest.config.ts"),
            "export default defineConfig({ test: { globals: true } });",
        );
    }
    directory
}

/// The base directories of the resolved projects, relative to `root`.
fn project_dirs(set: &ProjectSet, root: &Path) -> Vec<String> {
    let mut dirs: Vec<String> = set
        .projects
        .iter()
        .map(|project| {
            project
                .base_dir
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    dirs.sort();
    dirs
}

/// tldraw's spelling: one `const` hop, a brace pattern and a string `ignore`.
#[test]
fn glob_sync_with_a_literal_ignore_resolves() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "import { glob } from 'glob'\nimport { defineConfig } from 'vitest/config'\nconst vitestPackages = glob.sync('{apps,packages}/**/vitest.config.ts', {\n\tignore: '**/node_modules/**',\n})\nexport default defineConfig({ test: { projects: vitestPackages } })",
    );
    write(
        directory.path(),
        "packages/tldraw/vitest.config.ts",
        "export default defineConfig({ test: { globals: true } });",
    );
    let set = resolve_projects(directory.path());
    assert!(!set.has_unresolved_entries, "{:?}", set.diagnostics);
    let project = single_project(&set);
    assert!(matches!(project.globals, GlobalsDetection::Enabled { .. }));
}

/// An ignored config is not a project, and a file only it would include is not a test file.
#[test]
fn an_ignored_project_governs_nothing() {
    for options in [
        ", { ignore: 'pkg/b/**' }",
        ", { ignore: ['pkg/b/**'] }",
        ", { ignore: './pkg/b/**', }",
    ] {
        let directory = two_projects(options);
        let set = resolve_projects(directory.path());
        assert!(
            !set.has_unresolved_entries,
            "{options}: {:?}",
            set.diagnostics
        );
        assert_eq!(project_dirs(&set, directory.path()), ["pkg/a"], "{options}");
        assert!(
            matches!(
                set.governing_project(&directory.path().join("pkg/b/x.test.ts")),
                ProjectGovernance::NotIncluded
            ),
            "{options}"
        );
    }
    let directory = two_projects("");
    let set = resolve_projects(directory.path());
    assert_eq!(project_dirs(&set, directory.path()), ["pkg/a", "pkg/b"]);
}

/// glob 13.0.6 drops a matched directory named without `/**`, but not a file under it.
#[test]
fn an_ignore_without_a_globstar_drops_a_directory_match_only() {
    let directory = two_projects("");
    write(
        directory.path(),
        "vitest.config.ts",
        "import { glob } from 'glob'\nexport default { test: { projects: glob.sync('pkg/*', { ignore: 'pkg/b' }) } }",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(project_dirs(&set, directory.path()), ["pkg/a"]);
    // A trailing `/**` drops the directory it names as well.
    write(
        directory.path(),
        "vitest.config.ts",
        "import { glob } from 'glob'\nexport default { test: { projects: glob.sync('pkg/*', { ignore: 'pkg/b/**' }) } }",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(project_dirs(&set, directory.path()), ["pkg/a"]);
    let directory = two_projects(", { ignore: 'pkg/b' }");
    let set = resolve_projects(directory.path());
    assert_eq!(project_dirs(&set, directory.path()), ["pkg/a", "pkg/b"]);
    let directory = two_projects(", { ignore: 'pkg/b/vitest.config.ts' }");
    let set = resolve_projects(directory.path());
    assert_eq!(project_dirs(&set, directory.path()), ["pkg/a"]);
}

/// Every option but a literal `ignore` is a runtime setting this scan cannot know.
#[test]
fn other_glob_sync_options_stay_unresolved() {
    for options in [
        ", { ignore: patterns }",
        ", { ignore: ['pkg/b/**', other] }",
        ", { ignore: `pkg/b/**` }",
        ", { ignore: 'pkg/b/**', cwd: 'pkg' }",
        ", { dot: true }",
        ", { ...options }",
        ", { ignore }",
        ", { 'ignore': 'pkg/b/**' }",
        ", { ignore: 'pkg/b/**', ignore: 'pkg/a/**' }",
        ", { ignore: '!pkg/b/**' }",
        ", { ignore: 'pkg/+(b)/**' }",
        ", options",
        ", { ignore: 'pkg/b/**' }, extra",
    ] {
        let directory = two_projects(options);
        let set = resolve_projects(directory.path());
        assert!(set.has_unresolved_entries, "{options}: {set:?}");
        assert!(set.projects.is_empty(), "{options}");
    }
}

/// An `ignore` pattern that does not compile refuses the list and says why.
#[test]
fn an_uncompilable_ignore_pattern_is_reported() {
    let directory = two_projects(", { ignore: 'pkg/[b/**' }");
    let set = resolve_projects(directory.path());
    assert_unresolved_with_diagnostic(&set, "`ignore` pattern cannot be compiled");
}
