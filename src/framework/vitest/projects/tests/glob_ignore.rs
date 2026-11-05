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

/// glob 13.0.6 reads each brace alternative as its own pattern, so each `/**` alternative drops
/// the directory it names: `{pkg/b/**,pkg/c/**}` and `{./pkg/b/**,pkg/x}` both leave `pkg/a`.
#[test]
fn each_brace_alternative_drops_its_own_directory() {
    let directory = two_projects("");
    write(
        directory.path(),
        "pkg/c/vitest.config.ts",
        "export default defineConfig({ test: { globals: true } });",
    );
    for (ignore, expected) in [
        ("{pkg/b/**,pkg/c/**}", vec!["pkg/a"]),
        ("{./pkg/b/**,pkg/x}", vec!["pkg/a", "pkg/c"]),
        ("pkg/{b,c}/**", vec!["pkg/a"]),
    ] {
        write(
            directory.path(),
            "vitest.config.ts",
            &format!("import {{ glob }} from 'glob'\nexport default {{ test: {{ projects: glob.sync('pkg/*', {{ ignore: '{ignore}' }}) }} }}"),
        );
        let set = resolve_projects(directory.path());
        assert!(
            !set.has_unresolved_entries,
            "{ignore}: {:?}",
            set.diagnostics
        );
        assert_eq!(project_dirs(&set, directory.path()), expected, "{ignore}");
    }
}

/// An `ignore` alternative is read only in a spelling whose glob 13.0.6 meaning the matcher
/// reproduces. Each refused one is measured to differ: an absolute path is matched from glob's
/// working directory, `{a..b}` is a range, `{b}` is literal, `pkg/b/` and `pkg/./b/**` drop
/// `pkg/b`, `pkg//b/**` collapses the empty segment, a backslash escapes, `pkg/b/**/**` drops
/// `pkg/b`, and `?` or a non-ASCII letter matches per character (`pkg/?/**` drops `pkg/é`).
#[test]
fn an_ignore_in_an_unread_spelling_stays_unresolved() {
    let directory = two_projects("");
    let absolute = directory
        .path()
        .join("pkg/b")
        .to_string_lossy()
        .replace('\\', "/");
    for ignore in [
        format!("{absolute}/**"),
        "{pkg/x,/pkg/b/**}".to_owned(),
        "../pkg/b/**".to_owned(),
        "pkg/{a..b}/**".to_owned(),
        "pkg/{b}/**".to_owned(),
        "pkg/b/".to_owned(),
        "pkg/./b/**".to_owned(),
        "pkg//b/**".to_owned(),
        "pkg/b**/x".to_owned(),
        "pkg/\\\\b/**".to_owned(),
        "C:/pkg/b/**".to_owned(),
        "pkg/b/**/**".to_owned(),
        "pkg/?/**".to_owned(),
        "pkg/[b]/**".to_owned(),
        "pkg/[b/**".to_owned(),
        "pkg/\u{e9}/**".to_owned(),
        "{!pkg/b/**,pkg/x}".to_owned(),
    ] {
        write(
            directory.path(),
            "vitest.config.ts",
            &format!("import {{ glob }} from 'glob'\nexport default {{ test: {{ projects: glob.sync('pkg/*', {{ ignore: '{ignore}' }}) }} }}"),
        );
        let set = resolve_projects(directory.path());
        assert!(set.has_unresolved_entries, "{ignore}: {set:?}");
        assert!(set.projects.is_empty(), "{ignore}");
    }
}

/// glob 13.0.6 ignores without regard to case by default on macOS and Windows, and with it
/// elsewhere: `pkg/B/**` drops `pkg/b` on a Mac.
#[test]
fn an_ignore_matches_case_as_glob_does_on_this_platform() {
    let directory = two_projects(", { ignore: 'pkg/B/**' }");
    let set = resolve_projects(directory.path());
    assert!(!set.has_unresolved_entries, "{:?}", set.diagnostics);
    let expected: &[&str] = if cfg!(any(target_os = "macos", windows)) {
        &["pkg/a"]
    } else {
        &["pkg/a", "pkg/b"]
    };
    assert_eq!(project_dirs(&set, directory.path()), expected);
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

/// An `ignore` pattern past the compilation budget refuses the list as truncated, and a brace
/// with more alternatives than are enumerated refuses it too.
#[test]
fn an_ignore_past_the_budget_stays_unresolved() {
    let long = format!("pkg/{}/**", "0".repeat(MAX_PROJECT_GLOB_BYTES));
    let set = resolve_projects(two_projects(&format!(", {{ ignore: '{long}' }}")).path());
    assert_unresolved_with_diagnostic(&set, "compilation budget exhausted");
    assert!(set.truncated);
    let many: Vec<String> = (0..65).map(|n| n.to_string()).collect();
    let braces = format!("pkg/{{{}}}/**", many.join(","));
    let set = resolve_projects(two_projects(&format!(", {{ ignore: '{braces}' }}")).path());
    assert!(set.has_unresolved_entries, "{set:?}");
    assert!(set.projects.is_empty());
}

/// A directory `x/**` ignores is not walked, so it cannot spend the entry budget that the
/// projects outside it need.
#[test]
fn an_ignored_directory_is_not_walked() {
    let directory = two_projects("");
    for n in 0..40 {
        write(directory.path(), &format!("pkg/b/fixtures/f{n}.ts"), "");
    }
    let set = resolve_with_spent_budget(
        directory.path(),
        "import { glob } from 'glob'\nexport default { test: { projects: glob.sync('pkg/**/vitest.config.ts', { ignore: 'pkg/b/**' }) } }",
        MAX_GLOB_WALK_ENTRIES - 20,
    );
    assert!(!set.has_unresolved_entries, "{:?}", set.diagnostics);
    assert_eq!(project_dirs(&set, directory.path()), ["pkg/a"]);
}

/// A literal pattern is dropped like a match: glob 13.0.6 returns nothing for
/// `glob.sync('pkg/b', { ignore: 'pkg/b/**' })`.
#[test]
fn a_literal_pattern_its_ignore_names_matches_nothing() {
    let directory = two_projects("");
    for (ignore, expected) in [("pkg/b/**", vec![]), ("pkg/a/**", vec!["pkg/b"])] {
        write(
            directory.path(),
            "vitest.config.ts",
            &format!("import {{ glob }} from 'glob'\nexport default {{ test: {{ projects: glob.sync('pkg/b', {{ ignore: '{ignore}' }}) }} }}"),
        );
        let set = resolve_projects(directory.path());
        assert_eq!(project_dirs(&set, directory.path()), expected, "{ignore}");
    }
}

/// Every ASCII character an ignore alternative refuses is named in the configuration guide's
/// globals section, so a newly refused character cannot go undocumented.
#[test]
fn every_refused_ignore_character_is_documented() {
    let guide = include_str!("../../../../../docs/configuration.md");
    let start = guide
        .find("### Globals")
        .expect("the guide has a globals section");
    let section = &guide[start..];
    let section = &section[..section[3..]
        .find("\n#")
        .map_or(section.len(), |end| end + 3)];
    let refused: Vec<char> = (b'!'..=b'~')
        .map(char::from)
        .filter(|c| !ignore_spelling_is_read(&format!("pkg/a{c}b")))
        .collect();
    assert!(refused.len() >= 10, "only {refused:?} are refused");
    for c in refused {
        let named = match c {
            '\\' => "backslash".to_owned(),
            '[' | ']' => "`[...]`".to_owned(),
            '{' | '}' => "brace".to_owned(),
            c => format!("`{c}`"),
        };
        assert!(
            section.contains(&named),
            "{c:?} is refused but the guide does not name {named}"
        );
    }
}
