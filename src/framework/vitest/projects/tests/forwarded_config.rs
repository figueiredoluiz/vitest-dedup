use super::*;

/// A root config listing `pkg/a`, whose own config is `forward`, beside the given shared files.
fn forwarding_repo(forward: &str, shared: &[(&str, &str)]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default { test: { projects: ['pkg/a/vitest.config.ts'] } }",
    );
    write(directory.path(), "pkg/a/vitest.config.ts", forward);
    for (name, source) in shared {
        write(directory.path(), name, source);
    }
    directory
}

/// Resolves `root`'s single project with nothing unresolved and `globals` enabled.
fn resolves_enabled(root: &Path) -> ProjectSet {
    let set = resolve_projects(root);
    assert!(!set.has_unresolved_entries, "{:?}", set.diagnostics);
    assert!(matches!(
        single_project(&set).globals,
        GlobalsDetection::Enabled { .. }
    ));
    set
}

const SHARED: (&str, &str) = (
    "vitest.shared.ts",
    "import { defineConfig } from 'vitest/config'\nexport default defineConfig({ test: { globals: true, include: ['src/**/*.test.ts'] } })",
);

/// trpc's and tldraw's spellings, and a named forward: each project is the forwarded config,
/// rooted at its own directory.
#[test]
fn a_forwarded_config_is_the_project_config() {
    for forward in [
        "export { default } from '../../vitest.shared.ts';",
        "export { default } from '../../vitest.shared'",
        "/// <reference types=\"vitest\" />\nimport shared from '../../vitest.shared.ts'\n\nexport default shared\n",
        "import type { UserConfig } from 'vitest/config'\nimport { default as shared } from '../../vitest.shared.ts'\nexport default shared;",
    ] {
        let directory = forwarding_repo(forward, &[SHARED]);
        let set = resolves_enabled(directory.path());
        let file = directory.path().join("pkg/a/src/x.test.ts");
        assert!(
            matches!(set.governing_project(&file), ProjectGovernance::Governed(_)),
            "{forward}: include resolves against the forwarding file's directory"
        );
        assert!(matches!(
            set.governing_project(&directory.path().join("src/x.test.ts")),
            ProjectGovernance::NotIncluded
        ));
    }
}

/// `export { name as default }` selects the named export, as a named merge base does.
#[test]
fn a_named_forward_reads_the_named_export() {
    let directory = forwarding_repo(
        "export { node as default } from '../../presets'",
        &[(
            "presets.ts",
            "export const node = { test: { globals: true } }\nexport default { test: {} }",
        )],
    );
    resolves_enabled(directory.path());
}

/// Anything that could change the config between import and export, or that is not a relative
/// forward, refuses as before.
#[test]
fn a_forward_with_anything_else_stays_unresolved() {
    for forward in [
        "import shared from '../../vitest.shared.ts'\nshared.test.globals = false\nexport default shared",
        "import shared from '../../vitest.shared.ts'\nObject.assign(shared, {})\nexport default shared",
        "import shared from '../../vitest.shared.ts'\nmutate(shared)\nexport default shared",
        "import '../../mutate'\nexport { default } from '../../vitest.shared.ts'",
        "import { runtimeValue } from '../../mutate'\nexport { default } from '../../vitest.shared.ts'",
        // The same module object, changed through a second binding before it is forwarded.
        "import shared from '../../vitest.shared.ts'\nshared.test.globals = false\nexport { default } from '../../vitest.shared.ts'",
        "import shared from '../../vitest.shared.ts'\nexport default shared\nexport const other = 1",
        "import shared from '../../vitest.shared.ts'\nexport default other",
        "export { default } from 'vitest-shared-config'",
        "export { default as base } from '../../vitest.shared.ts'",
        "export * from '../../vitest.shared.ts'",
        "export { default } from '../../missing.ts'",
        "export { default } from '../../vitest.shared.ts'\nconsole.log(1)",
    ] {
        let directory = forwarding_repo(forward, &[SHARED]);
        let set = resolve_projects(directory.path());
        assert!(set.has_unresolved_entries, "{forward}: {set:?}");
        assert!(
            !matches!(single_project(&set).globals, GlobalsDetection::Enabled { .. }),
            "{forward}"
        );
    }
}

/// A forward to a file outside the analysis root is refused and named.
#[test]
fn a_forward_outside_the_root_is_reported() {
    let outer = tempfile::tempdir().unwrap();
    write(outer.path(), SHARED.0, SHARED.1);
    let root = outer.path().join("repo");
    write(
        &root,
        "vitest.config.ts",
        "export default { test: { projects: ['pkg/a/vitest.config.ts'] } }",
    );
    write(
        &root,
        "pkg/a/vitest.config.ts",
        "export { default } from '../../../vitest.shared.ts'",
    );
    let set = resolve_projects(&root);
    assert!(set.has_unresolved_entries, "{set:?}");
    assert!(
        set.diagnostics
            .iter()
            .any(|note| note.contains("forwarded config")),
        "{:?}",
        set.diagnostics
    );
    assert!(matches!(
        single_project(&set).globals,
        GlobalsDetection::Unknown { .. }
    ));
}

/// A forward cycle is refused; a merge base that forwards resolves through it.
#[test]
fn forwards_chain_like_merge_bases() {
    let directory = forwarding_repo(
        "export { default } from './b'",
        &[("pkg/a/b.ts", "export { default } from './vitest.config'")],
    );
    let set = resolve_projects(directory.path());
    assert!(set.has_unresolved_entries, "{set:?}");
    let directory = forwarding_repo(
        "import { mergeConfig } from 'vitest/config'\nimport base from './forward'\nexport default mergeConfig(base, { test: {} })",
        &[SHARED, ("pkg/a/forward.ts", "export { default } from '../../vitest.shared.ts'")],
    );
    resolves_enabled(directory.path());
}

/// Discovery's comparison scope reads a forward as `mergeConfig(target, {})`.
#[test]
fn a_forward_has_the_scope_of_an_empty_merge() {
    let directory = forwarding_repo(
        "export { default } from '../../vitest.shared.ts'",
        &[SHARED],
    );
    write(
        directory.path(),
        "pkg/b/vitest.config.ts",
        "import { mergeConfig } from 'vitest/config'\nimport base from '../../vitest.shared.ts'\nexport default mergeConfig(base, {})",
    );
    let forward = resolved_config_test_scope(&directory.path().join("pkg/a/vitest.config.ts"));
    let merge = resolved_config_test_scope(&directory.path().join("pkg/b/vitest.config.ts"));
    assert!(forward.as_ref().is_ok_and(Option::is_some), "{forward:?}");
    assert_eq!(forward, merge);
    write(
        directory.path(),
        "pkg/a/vitest.config.ts",
        "import shared from '../../vitest.shared.ts'\nshared.test = {}\nexport default shared",
    );
    assert!(resolved_config_test_scope(&directory.path().join("pkg/a/vitest.config.ts")).is_err());
}
