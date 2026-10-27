use super::*;

/// Exhaustion at the final entry must remain visible through every project-list reader.
#[test]
fn oversized_globs_degrade_arrays_workspaces_and_glob_sync() {
    let pattern = format!("p[{}]g", "k".repeat(MAX_PROJECT_GLOB_BYTES));
    for (file, source) in [
        ("vitest.config.ts", format!("export default {{ test: {{ projects: ['{pattern}'] }} }}")),
        ("vitest.workspace.ts", format!("export default ['{pattern}']")),
        ("vitest.config.ts", format!("import {{ glob }} from 'glob'; export default {{ test: {{ projects: glob.sync('{pattern}') }} }}")),
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), file, &source);
        let set = resolve_projects(directory.path());
        assert_unresolved_with_diagnostic(&set, "glob compilation budget");
        assert!(set.truncated && set.config_found, "{set:?}");
    }
}

/// An unresolved later entry can override a previously resolved project's membership.
#[test]
fn compilation_refusal_cannot_leave_earlier_governance_certain() {
    let directory = tempfile::tempdir().unwrap();
    let pattern = format!("p[{}]g", "k".repeat(MAX_PROJECT_GLOB_BYTES));
    write(directory.path(), "vitest.config.ts", &format!("export default {{ test: {{ projects: [{{ test: {{ include: ['**/*.test.ts'] }} }}, '{pattern}'] }} }}"));
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 1);
    assert!(set.truncated && set.has_unresolved_entries);
    assert!(matches!(
        set.governing_project(&directory.path().join("scope.test.ts")),
        ProjectGovernance::Unknown { .. }
    ));
}

/// Full and prefix matchers consume one shared budget, including invalid patterns.
#[test]
fn compilation_charges_are_cumulative_and_refusal_is_sticky() {
    let root = tempfile::tempdir().unwrap();
    let mut budget = Budget {
        compile_bytes: MAX_GLOB_COMPILE_BYTES - 4,
        ..Budget::default()
    };
    let first = expand_project_path(root.path(), "*[", &mut budget);
    assert!(first.paths.is_empty());
    assert_eq!(budget.compile_bytes, MAX_GLOB_COMPILE_BYTES - 2);
    expand_project_path(root.path(), "*?", &mut budget);
    assert_eq!(budget.compile_bytes, MAX_GLOB_COMPILE_BYTES);
    assert!(!budget.compilation_refused);
    expand_project_path(root.path(), "*", &mut budget);
    assert!(budget.compilation_refused);
    assert_eq!(budget.compile_bytes, MAX_GLOB_COMPILE_BYTES);
    assert!(!budget.charge_compilation(0));
}

/// Exact UTF-8 byte and cumulative-prefix boundaries are admitted, the next byte refused.
#[test]
fn pattern_and_prefix_boundaries_are_checked_before_compilation() {
    let root = tempfile::tempdir().unwrap();
    let pattern = format!("p[{}]g", "k".repeat(MAX_PROJECT_GLOB_BYTES - 4));
    let mut budget = Budget::default();
    expand_project_path(root.path(), &pattern, &mut budget);
    assert!(!budget.compilation_refused);
    expand_project_path(root.path(), &(pattern + "?"), &mut budget);
    assert!(budget.compilation_refused);
    let mut unicode_budget = Budget::default();
    expand_project_path(
        root.path(),
        &format!("{}*", "é".repeat(MAX_PROJECT_GLOB_BYTES / 2)),
        &mut unicode_budget,
    );
    assert!(unicode_budget.compilation_refused);
    assert_eq!(unicode_budget.compile_bytes, 0);
    let pattern = "é/*/file";
    let cost = 2 + 4;
    for remaining in [cost, cost - 1] {
        let mut budget = Budget {
            compile_bytes: MAX_GLOB_COMPILE_BYTES - remaining,
            ..Budget::default()
        };
        let matchers = project_directory_matchers(pattern, &mut budget);
        assert_eq!(matchers.len(), if remaining == cost { 2 } else { 0 });
        assert_eq!(budget.compilation_refused, remaining < cost);
        assert!(budget.compile_bytes <= MAX_GLOB_COMPILE_BYTES);
    }
}

/// Ordinary walks need one full matcher regardless of the number of directory prefixes.
#[test]
fn walks_without_symlinks_do_not_compile_prefixes() {
    let root = tempfile::tempdir().unwrap();
    write_glob_monorepo(root.path(), 2);
    for pattern in [
        "packages/*/vitest.config.ts".to_owned(),
        format!("missing/{}*", "a/".repeat(512)),
    ] {
        let mut budget = Budget::default();
        let expanded = expand_project_path(root.path(), &pattern, &mut budget);
        assert_eq!(
            expanded.paths.len(),
            if pattern.starts_with("packages") {
                2
            } else {
                0
            }
        );
        assert_eq!(budget.compile_bytes, pattern.len());
        assert!(!budget.exhausted());
    }
}

/// Cached prefix matching preserves wildcard, brace, globstar and Unicode link containment.
#[cfg(unix)]
#[test]
fn lazy_prefix_matchers_preserve_external_link_refusals() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("é")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("other")).unwrap();
    let internal = root.path().join("inside");
    fs::create_dir(&internal).unwrap();
    std::os::unix::fs::symlink(&internal, root.path().join("local")).unwrap();
    for (pattern, expected) in [
        ("*/child/*", 2),
        ("**/child/*", 2),
        ("{é,other}/child/*", 2),
        ("é*/child/*", 1),
        ("chosen*/child/*", 0),
    ] {
        let mut budget = Budget::default();
        let expanded = expand_project_path(root.path(), pattern, &mut budget);
        assert!(expanded.paths.is_empty());
        assert_eq!(expanded.outside, expected, "{pattern}");
        let prefix_bytes: usize = pattern.match_indices('/').map(|(offset, _)| offset).sum();
        assert_eq!(budget.compile_bytes, pattern.len() + prefix_bytes);
        assert!(!budget.exhausted());
    }
}

/// Prefix work refusal at the last array entry marks the entire project map incomplete.
#[cfg(unix)]
#[test]
fn prefix_compilation_refusal_reaches_project_map() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
    let pattern = format!("*/{}*", "a/".repeat(1024));
    write(
        root.path(),
        "vitest.config.ts",
        &format!("export default {{ test: {{ projects: ['{pattern}'] }} }}"),
    );
    let set = resolve_projects(root.path());
    assert_unresolved_with_diagnostic(&set, "glob compilation budget");
    assert!(set.truncated);
}
