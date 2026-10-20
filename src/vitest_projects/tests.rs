use super::*;
use std::fs;

/// Asserts the resolution landed in the unresolved state with `needle` among its
/// diagnostics -- the ending every "this array cannot be enumerated soundly" test has.
fn assert_unresolved_with_diagnostic(set: &ProjectSet, needle: &str) {
    assert!(
        set.projects.is_empty(),
        "expected no resolved projects: {set:?}"
    );
    assert!(
        set.has_unresolved_entries,
        "expected the set to say so: {set:?}"
    );
    assert!(
        set.diagnostics.iter().any(|note| note.contains(needle)),
        "expected a diagnostic containing {needle:?}: {:?}",
        set.diagnostics
    );
}

/// The reason a single resolved project's `include` is `Unknown`, asserting the singleton
/// and the `Unknown`-ness so the caller asserts only its own reason text.
fn unknown_include_reason(set: &ProjectSet) -> &str {
    assert_eq!(set.projects.len(), 1, "{set:?}");
    match &set.projects[0].include {
        GlobPatterns::Unknown { reason } => reason,
        other => panic!("expected Unknown include, got {other:?}"),
    }
}

/// `match_file` with the result asserted by value -- the form every project-scoping test
/// ends in. `why` stays at the call site; it is the test's claim.
fn assert_file_match(project: &Project, file: &Path, expected: FileMatch, why: &str) {
    assert_eq!(match_file(project, file), expected, "{why}");
}

/// The one project a test expects to have resolved, asserting the singleton.
fn single_project(set: &ProjectSet) -> &Project {
    assert_eq!(set.projects.len(), 1, "{set:?}");
    &set.projects[0]
}

/// Asserts that both globals views of `file` -- the per-file lookup and the governed
/// lookup that fails closed on ambiguity -- read as `Unknown`: the ending every ambiguous
/// multi-project case has.
fn assert_both_globals_views_unknown(set: &ProjectSet, file: &Path) {
    assert!(matches!(
        set.globals_for(file),
        GlobalsDetection::Unknown { .. }
    ));
    assert!(matches!(
        set.governed_globals_for(file),
        GovernedGlobals::Unknown { .. }
    ));
}

/// Writes `count` package configs under `packages/` plus the root `test.projects` glob that
/// references them -- the monorepo shape the project-cap tests size up and down.
fn write_glob_monorepo(root: &Path, count: usize) {
    for index in 0..count {
        write(
            root,
            &format!("packages/p{index}/vitest.config.ts"),
            "export default defineConfig({ test: { globals: true } });",
        );
    }
    write(
        root,
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: ['packages/*'] } });",
    );
}

/// The minimal discovery config over `root`: empty explicit globs, no rules, no threshold --
/// what the tests that drive `discover` against a resolved project set need.
fn discovery_config(root: &Path) -> crate::config::Config {
    crate::config::Config {
        root: root.to_path_buf(),
        include: Vec::new(),
        include_is_explicit: false,
        exclude: Vec::new(),
        rules: Default::default(),
        threshold: 0,
        source: "test".to_owned(),
    }
}

/// The root-config refusal contract every "cannot read" shape shares: no projects, the
/// config was found, unresolved, a diagnostic containing `needle`, and `name` in every
/// failure message so a table row that regresses names itself.
fn assert_root_config_refused(set: &ProjectSet, name: &str, needle: &str) {
    assert!(set.projects.is_empty(), "{name}: {:?}", set.projects);
    assert!(set.config_found, "{name}");
    assert!(set.has_unresolved_entries, "{name}");
    assert!(
        set.diagnostics.iter().any(|note| note.contains(needle)),
        "{name}: {:?}",
        set.diagnostics
    );
}

/// The inherited-defaults tail: a project entry that reached Vitest's own defaults reads
/// `globals` disabled with the default include/exclude globs and a confidently-empty
/// `includeSource`.
fn assert_inherited_defaults(project: &Project) {
    assert!(matches!(project.globals, GlobalsDetection::Disabled { .. }));
    assert_eq!(project.include, owned(DEFAULT_INCLUDE_GLOBS));
    assert_eq!(project.exclude, owned(DEFAULT_EXCLUDE_GLOBS));
    assert_eq!(project.include_source, GlobPatterns::Known(Vec::new()));
}

/// Resolves and returns the one project, asserting its `test.name` and that its `globals`
/// reads Enabled -- the "extends resolved and inherited" ending two tests share.
fn assert_single_named_enabled<'a>(set: &'a ProjectSet, name: &str) -> &'a Project {
    let project = single_project(set);
    assert_eq!(project.name.as_deref(), Some(name));
    assert!(matches!(project.globals, GlobalsDetection::Enabled { .. }));
    project
}

/// Write fixture contents beneath the root, creating parent directories and panicking on
/// filesystem errors.
fn write(root: &Path, name: &str, contents: &str) {
    if let Some(parent) = root.join(name).parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(root.join(name), contents).unwrap();
}

/// Create and return a temporary config fixture with the requested file and source.
fn review_fixture(path: &str, source: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), path, source);
    directory
}

/// Return references to all eight modeled glob-array channels in a stable order for merge
/// assertions.
fn merge_array_channels(fields: &EffectiveTestFields) -> [&GlobPatterns; 8] {
    [
        &fields.include,
        &fields.exclude,
        &fields.include_source,
        &fields.benchmark_include,
        &fields.benchmark_exclude,
        &fields.benchmark_include_source,
        &fields.typecheck_include,
        &fields.typecheck_exclude,
    ]
}

/// Build a test-object literal assigning the pattern to every modeled glob-array channel,
/// with globals and typecheck.enabled both true.
fn merge_all_arrays(pattern: &str) -> String {
    format!("{{ globals: true, include: ['{pattern}'], exclude: ['{pattern}'], includeSource: ['{pattern}'], benchmark: {{ include: ['{pattern}'], exclude: ['{pattern}'], includeSource: ['{pattern}'] }}, typecheck: {{ enabled: true, include: ['{pattern}'], exclude: ['{pattern}'] }} }}")
}

/// Write base/override config fixtures and return their resolved project name and effective
/// fields.
fn merged_review_fields(
    root: &Path,
    base: &str,
    own: &str,
) -> (Option<String>, EffectiveTestFields) {
    write(
        root,
        "base.ts",
        &format!("export default {{ test: {base} }};"),
    );
    write(root, "merged.ts", &format!("import {{ mergeConfig }} from 'vitest/config'; import base from './base'; export default mergeConfig(base, {{ test: {own} }});"));
    resolve_standalone_config(&root.join("merged.ts")).unwrap()
}

/// Verify merge arrays apply defaults only after explicit merging.
#[test]
fn review_merge_arrays_apply_defaults_only_after_explicit_merging() {
    let directory = tempfile::tempdir().unwrap();
    let all = merge_all_arrays("base/**");
    let empty = merge_all_arrays("empty").replace("['empty']", "[]");
    let null_base = merge_all_arrays("empty").replace("['empty']", "null");
    for (base, own, expected) in [
        ("{}", all.as_str(), "base/**"),
        (all.as_str(), empty.as_str(), "base/**"),
        (null_base.as_str(), all.as_str(), "base/**"),
    ] {
        let (_, fields) = merged_review_fields(directory.path(), base, own);
        for patterns in merge_array_channels(&fields) {
            assert_eq!(patterns, &GlobPatterns::Known(vec![expected.to_owned()]));
        }
    }
    write(directory.path(), "base.ts", "export default { test: {} };");
    write(directory.path(), "middle.ts", &format!("import {{ mergeConfig }} from 'vitest/config'; import base from './base'; export default mergeConfig(base, {{ test: {all} }});"));
    write(directory.path(), "merged.ts", &format!("import {{ mergeConfig }} from 'vitest/config'; import base from './middle'; export default mergeConfig(base, {{ test: {empty} }});"));
    let (_, fields) = resolve_standalone_config(&directory.path().join("merged.ts")).unwrap();
    for patterns in merge_array_channels(&fields) {
        assert_eq!(patterns, &GlobPatterns::Known(vec!["base/**".to_owned()]));
    }
}

/// Verify merge nullish overrides preserve arrays groups scalars and name.
#[test]
fn review_merge_nullish_overrides_preserve_arrays_groups_scalars_and_name() {
    let directory = tempfile::tempdir().unwrap();
    let all = merge_all_arrays("base/**").replacen("{", "{ name: 'base-name',", 1);
    for nullish in ["null", "undefined", "void 0"] {
        let own = merge_all_arrays("empty")
            .replace("['empty']", nullish)
            .replace("true", nullish)
            .replacen("{", &format!("{{ name: {nullish},"), 1);
        let (name, fields) = merged_review_fields(directory.path(), &all, &own);
        assert_eq!(name.as_deref(), Some("base-name"));
        assert!(matches!(fields.globals, GlobalsDetection::Enabled { .. }));
        assert!(matches!(
            fields.typecheck_enabled,
            TypecheckAdmission::Enabled
        ));
        for patterns in merge_array_channels(&fields) {
            assert_eq!(patterns, &GlobPatterns::Known(vec!["base/**".to_owned()]));
        }
        for own in [
            nullish.to_owned(),
            format!("{{ benchmark: {nullish}, typecheck: {nullish} }}"),
        ] {
            let (_, fields) = merged_review_fields(directory.path(), &all, &own);
            for patterns in merge_array_channels(&fields) {
                assert_eq!(patterns, &GlobPatterns::Known(vec!["base/**".to_owned()]));
            }
        }
    }
    let (_, fields) = merged_review_fields(
        directory.path(),
        "{ typecheck: { enabled: true } && { enabled: false } }",
        "{}",
    );
    assert!(matches!(
        fields.typecheck_enabled,
        TypecheckAdmission::Unknown { .. }
    ));
    write(directory.path(), "merged.ts", "import { mergeConfig } from 'vitest/config'; import base from './base'; export default mergeConfig(base, { test: { globals: null } && { globals: false } });");
    assert!(resolve_standalone_config(&directory.path().join("merged.ts")).is_none());
    for operator in ["&&", "||", "??"] {
        let own = format!("{{ benchmark: {{ include: null }} {operator} {{ include: ['own/**'] }}, typecheck: {{ enabled: null }} {operator} {{ enabled: false }} }}");
        let (_, fields) = merged_review_fields(directory.path(), &all, &own);
        assert!(
            matches!(fields.benchmark_include, GlobPatterns::Unknown { .. }),
            "{own}"
        );
        assert!(
            matches!(fields.typecheck_enabled, TypecheckAdmission::Unknown { .. }),
            "{own}"
        );
    }
    write(directory.path(), "merged.ts", "import { mergeConfig } from 'vitest/config'; import base from './base'; const unrelated = 1; let other = 2; var third = 3; export default mergeConfig(base, { test: { include: undefined } });");
    let (_, fields) = resolve_standalone_config(&directory.path().join("merged.ts")).unwrap();
    assert_eq!(
        fields.include,
        GlobPatterns::Known(vec!["base/**".to_owned()])
    );
    for declaration in [
        "const undefined = ['other/**'];",
        "let undefined = ['other/**'];",
        "var undefined = ['other/**'];",
        "import { other as undefined } from './other';",
        "function undefined() {}",
    ] {
        write(directory.path(), "merged.ts", &format!("import {{ mergeConfig }} from 'vitest/config'; import base from './base'; {declaration} export default mergeConfig(base, {{ test: {{ include: undefined }} }});"));
        let (_, fields) = resolve_standalone_config(&directory.path().join("merged.ts")).unwrap();
        assert!(
            matches!(fields.include, GlobPatterns::Unknown { .. }),
            "{declaration}"
        );
    }
}

/// Verify merge unknown arrays cannot be replaced by known overrides.
#[test]
fn review_merge_unknown_arrays_cannot_be_replaced_by_known_overrides() {
    let directory = tempfile::tempdir().unwrap();
    for (base, own) in [
        ("{ include: makePatterns() }", "{ include: ['own/**'] }"),
        ("{ include: ['base/**'] }", "{ include: makePatterns() }"),
        ("{ include: makePatterns() }", "{}"),
        ("{}", "{ include: makePatterns() }"),
    ] {
        let (_, fields) = merged_review_fields(directory.path(), base, own);
        assert!(matches!(fields.include, GlobPatterns::Unknown { .. }));
    }
    write(directory.path(), "merged.ts", "import { mergeConfig } from 'vitest/config'; import base from './missing'; export default mergeConfig(base, { test: { globals: true, include: ['own/**'] } });");
    let (_, fields) = resolve_standalone_config(&directory.path().join("merged.ts")).unwrap();
    assert!(matches!(fields.globals, GlobalsDetection::Enabled { .. }));
    for patterns in merge_array_channels(&fields) {
        assert!(matches!(patterns, GlobPatterns::Unknown { .. }));
    }
}

/// Verify named import variants require the selected static export.
#[test]
fn review_named_import_variants_require_the_selected_static_export() {
    let directory = tempfile::tempdir().unwrap();
    for header in [
        "import { preset as base } from './base';",
        "import { preset } from './base';",
        "import unused, { preset as base } from './base';",
        "// 日本🙂\nimport { other, /* binding */ preset as base }\nfrom './base';",
    ] {
        let local = if header.contains("{ preset }") {
            "preset"
        } else {
            "base"
        };
        for literal in [
            "{ test: { name: 'selected', globals: true } }",
            "defineConfig({ test: { name: 'selected', globals: true } })",
            "defineProject({ test: { name: 'selected', globals: true } })",
        ] {
            write(directory.path(), "base.ts", &format!("import {{ defineConfig, defineProject }} from 'vitest/config'; export const other = {{}}; export const preset = {literal}; export default {{ test: {{ globals: false }} }};"));
            write(directory.path(), "merged.ts", &format!("import {{ mergeConfig }} from 'vitest/config'; {header} export default mergeConfig({local}, {{}});"));
            let (name, fields) =
                resolve_standalone_config(&directory.path().join("merged.ts")).unwrap();
            assert_eq!(name.as_deref(), Some("selected"));
            assert!(matches!(fields.globals, GlobalsDetection::Enabled { .. }));
        }
    }
    write(
        directory.path(),
        "base.ts",
        "export default { test: { globals: true } };",
    );
    write(directory.path(), "merged.ts", "import { mergeConfig } from 'vitest/config'; import { default as base } from './base'; export default mergeConfig(base, {});");
    assert!(matches!(
        resolve_standalone_config(&directory.path().join("merged.ts"))
            .unwrap()
            .1
            .globals,
        GlobalsDetection::Enabled { .. }
    ));
    for source in [
        "export default { test: { globals: true } };",
        "export const wrong = { test: { globals: true } };",
        "export let preset = { test: { globals: true } };",
        "const preset = { test: { globals: true } }; export { preset };",
        "export const preset = makeConfig();",
        "import { defineConfig } from 'vitest/config'; export const preset = defineConfig;",
        "import { defineConfig } from 'vitest/config'; export const preset = defineConfig(null);",
        "export const preset = { test: { globals: true } } || other;",
        "export const preset = unknown({ test: { globals: true } });",
        "// export const preset = { test: { globals: true } };",
        "function f() { const preset = { test: { globals: true } }; }",
        "export const preset = { test: { globals: true } }; preset.test.globals = false;",
        r"export const preset = { test: { globals: true } }; p\u0072eset.test.globals = false;",
    ] {
        write(directory.path(), "base.ts", source);
        write(directory.path(), "merged.ts", "import { mergeConfig } from 'vitest/config'; import { preset as base } from './base'; export default mergeConfig(base, {});");
        assert!(
            resolve_standalone_config(&directory.path().join("merged.ts")).is_none(),
            "{source}"
        );
    }
    for clause in [
        "type { preset as base }",
        "{ type preset as base }",
        "{ preset as another }",
        "{ unused, wrong }",
        "* as base",
    ] {
        assert!(
            imported_config_binding(clause, "base").is_none(),
            "{clause}"
        );
    }
    assert!(imported_config_binding("} {base", "base").is_none());
}

/// Verify merge arrays preserve every explicit base channel.
#[test]
fn review_merge_arrays_preserve_every_explicit_base_channel() {
    let directory = review_fixture(
        "base.ts",
        &format!(
            "export default {{ test: {} }};",
            merge_all_arrays("base/**")
        ),
    );
    write(directory.path(), "merged.ts", &format!("import {{ mergeConfig }} from 'vitest/config'; import base from './base'; export default mergeConfig(base, {{ test: {} }});", merge_all_arrays("pkg/**")));
    let (_, fields) = resolve_standalone_config(&directory.path().join("merged.ts")).unwrap();
    for patterns in merge_array_channels(&fields) {
        assert_eq!(
            patterns,
            &GlobPatterns::Known(vec!["base/**".to_owned(), "pkg/**".to_owned()])
        );
    }
}

/// Verify named merge base selects named export over default.
#[test]
fn review_named_merge_base_selects_named_export_over_default() {
    let directory = review_fixture("base.ts", "export const preset = { test: { name: 'named', globals: true, include: ['named/**'] } }; export default { test: { name: 'default', globals: false, include: ['wrong/**'] } };");
    write(directory.path(), "merged.ts", "import { mergeConfig } from 'vitest/config'; import { preset as base } from './base'; export default mergeConfig(base, {});");
    let (name, fields) = resolve_standalone_config(&directory.path().join("merged.ts")).unwrap();
    assert_eq!(name.as_deref(), Some("named"));
    assert!(matches!(fields.globals, GlobalsDetection::Enabled { .. }));
    assert_eq!(
        fields.include,
        GlobPatterns::Known(vec!["named/**".to_owned()])
    );
}

/// Verify merge base prefers the exact imported path.
#[test]
fn summary_merge_base_prefers_the_exact_imported_path() {
    let directory = review_fixture(
        "preset.js",
        "export default { test: { globals: false, name: 'exact-base' } };",
    );
    write(
        directory.path(),
        "preset.js.ts",
        "export default { test: { globals: true, name: 'suffix-base' } };",
    );
    write(directory.path(), "merged.ts", "import { mergeConfig } from 'vitest/config'; import base from './preset.js'; export default mergeConfig(base, {});");
    let (name, fields) = resolve_standalone_config(&directory.path().join("merged.ts")).unwrap();
    assert_eq!(name.as_deref(), Some("exact-base"));
    assert!(matches!(fields.globals, GlobalsDetection::Disabled { .. }));
}

/// Verify glob import requires the package root.
#[test]
fn summary_glob_import_requires_the_package_root() {
    let directory = review_fixture(
        "pkg/a/vitest.config.ts",
        "export default { test: { globals: true } }",
    );
    for (package, resolved) in [("glob", true), ("glob/join", false)] {
        write(directory.path(), "vitest.config.ts", &format!("import {{ glob }} from '{package}'; export default {{ test: {{ projects: glob.sync('./pkg/*') }} }};"));
        let set = resolve_projects(directory.path());
        assert_eq!(set.projects.len(), usize::from(resolved), "{package}");
        assert_eq!(set.has_unresolved_entries, !resolved, "{package}");
        if resolved {
            assert!(matches!(
                single_project(&set).globals,
                GlobalsDetection::Enabled { .. }
            ));
        }
    }
}

/// Verify merge literal base is refused.
#[test]
fn followup_merge_literal_base_is_refused() {
    let directory = review_fixture("merged.ts", "import { mergeConfig } from 'vitest/config'; export default mergeConfig({ test: { globals: false } }, { test: { globals: true } });");
    assert!(resolve_standalone_config(&directory.path().join("merged.ts")).is_none());
}

/// Verify merge override requires a complete verified literal.
#[test]
fn followup_merge_override_requires_a_complete_verified_literal() {
    let directory = review_fixture("base.ts", "export default { test: { globals: false } }");
    for value in [
        "makeDynamic({ test: { globals: true } })",
        "helpers.defineProject({ test: { globals: true } })",
        "defineProject({ test: { globals: true } })",
        "{ test: { globals: true } } || other",
        "defineProject({ test: { globals: true } }) || other",
        "defineProject({ test: { globals: true } }, other)",
    ] {
        write(directory.path(), "merged.ts", &format!("import {{ mergeConfig }} from 'vitest/config'; import base from './base'; export default mergeConfig(base, {value});"));
        assert!(
            resolve_standalone_config(&directory.path().join("merged.ts")).is_none(),
            "{value}"
        );
    }
}

/// Verify merge wrapper provenance and export boundaries.
#[test]
fn followup_merge_wrapper_provenance_and_export_boundaries() {
    let directory = review_fixture(
        "base.ts",
        "export default { test: { globals: false, name: 'base-name' } }",
    );
    for (header, expression, accepted) in [
        ("import { defineConfig } from 'vitest/config';", "mergeConfig(base, defineConfig({ test: { globals: true } }))", true),
        ("import { defineConfig } from 'vite';", "mergeConfig(base, defineConfig({ test: { globals: true } }))", true),
        ("import { defineConfig } from './helpers';", "mergeConfig(base, defineConfig({ test: { globals: true } }))", false),
        ("import { defineConfig } from 'vitest/config';", "mergeConfig(base, defineConfig(() => ({ test: { globals: true } })))", false),
        ("import { defineConfig } from 'vitest/config';", "mergeConfig(base, defineConfig({ test: { globals: true } }) || other)", false),
        ("import { defineProject } from 'vitest/config';", "mergeConfig(base, defineProject({ test: { globals: true } }))", true),
        ("import { defineProject } from 'vitest';", "mergeConfig(base, defineProject(/* before */ { test: { globals: true } } /* after */))", true),
        ("import { defineProject } from './helpers';", "mergeConfig(base, defineProject({ test: { globals: true } }))", false),
        ("import type { defineProject } from 'vitest/config';", "mergeConfig(base, defineProject({ test: { globals: true } }))", false),
        ("import { other as defineProject } from 'vitest/config';", "mergeConfig(base, defineProject({ test: { globals: true } }))", false),
        ("import { defineProject } from 'vitest/config';", "mergeConfig(base, defineProject({ test: { globals: true } }) || other)", false),
        ("", "mergeConfig(base, { test: { globals: true } }) || other", false),
        ("", "mergeConfig(base, { test: { globals: true } }).test", false),
    ] {
        write(directory.path(), "merged.ts", &format!("import {{ mergeConfig }} from 'vitest/config'; import base from './base'; {header} export default {expression};"));
        let result = resolve_standalone_config(&directory.path().join("merged.ts"));
        assert_eq!(result.is_some(), accepted, "{header} {expression}");
        if let Some((name, fields)) = result {
            assert_eq!(name.as_deref(), Some("base-name"));
            assert!(matches!(fields.globals, GlobalsDetection::Enabled { .. }));
        }
    }
}

/// Verify merge name inherits and own name wins.
#[test]
fn followup_merge_name_inherits_and_own_name_wins() {
    let directory = review_fixture(
        "base.ts",
        "export default { test: { globals: false, name: 'base-name' } }",
    );
    for (value, expected) in [
        ("{ test: {} }", Some("base-name")),
        ("{}", Some("base-name")),
        ("{ test: { name: 'own-name' } }", Some("own-name")),
        ("{ test: { name: dynamicName } }", None),
    ] {
        write(directory.path(), "merged.ts", &format!("import {{ mergeConfig }} from 'vitest/config'; import base from './base'; export default mergeConfig(base, {value});"));
        let (name, _) = resolve_standalone_config(&directory.path().join("merged.ts")).unwrap();
        assert_eq!(name.as_deref(), expected, "{value}");
    }
}

/// Verify unicode before imports preserves glob and merge resolution.
#[test]
fn review_unicode_before_imports_preserves_glob_and_merge_resolution() {
    let directory = review_fixture("base.ts", "export default { test: { globals: true } }");
    let prefix = format!("// {}\n", "日本🙂".repeat(80));
    for source in [
        "import { glob } from 'glob'\nexport default { test: { projects: glob.sync('./pkg/*') } }",
        "import { glob } from 'glob'\nconst entries = glob.sync('./pkg/*');\nexport default { test: { projects: entries } }",
    ] {
        write(directory.path(), "vitest.config.ts", &format!("{prefix}{source}"));
        write(directory.path(), "pkg/a/vitest.config.ts", &format!("{prefix}import {{ mergeConfig }} from 'vitest/config'\nimport base from '../../base'\nexport default mergeConfig(base, {{ test: {{}} }})"));
        let set = resolve_projects(directory.path());
        let project = single_project(&set);
        assert!(matches!(project.globals, GlobalsDetection::Enabled { .. }));
        assert!(!set.has_unresolved_entries, "{:?}", set.diagnostics);
    }
}

/// Verify merge config accepts vite and vitest exports.
#[test]
fn review_merge_config_accepts_vite_and_vitest_exports() {
    let directory = review_fixture("base.ts", "export default { test: { globals: true } }");
    for package in ["vitest/config", "vitest", "vite"] {
        write(directory.path(), "merged.ts", &format!("import {{ mergeConfig }} from '{package}'; import base from './base'; export default mergeConfig(base, {{}});"));
        let result = resolve_standalone_config(&directory.path().join("merged.ts"));
        assert!(
            matches!(
                result,
                Some((
                    _,
                    EffectiveTestFields {
                        globals: GlobalsDetection::Enabled { .. },
                        ..
                    }
                ))
            ),
            "{package}"
        );
    }
}

/// Verify merge config requires a verified import.
#[test]
fn review_merge_config_requires_a_verified_import() {
    let directory = review_fixture("base.ts", "export default { test: { globals: true } }");
    for header in [
        "",
        "import { mergeConfig } from './other'\n",
        "import type { mergeConfig } from 'vitest/config'\n",
        "import { other as mergeConfig } from 'vitest/config'\n",
        "// import { mergeConfig } from 'vitest/config'\n",
        "function mergeConfig(a, b) { return {} }\n",
    ] {
        write(
            directory.path(),
            "vitest.config.ts",
            "export default { test: { projects: ['./pkg'] } }",
        );
        write(directory.path(), "pkg/vitest.config.ts", &format!("{header}import base from '../base'\nexport default mergeConfig(base, {{ test: {{ globals: true }} }})"));
        let set = resolve_projects(directory.path());
        assert!(
            matches!(
                single_project(&set).globals,
                GlobalsDetection::Unknown { .. }
            ),
            "{header}"
        );
    }
}

/// Verify const hop requires an earlier top level binding.
#[test]
fn review_const_hop_requires_an_earlier_top_level_binding() {
    let directory = review_fixture("pkg/a/vitest.config.ts", "export default { test: {} }");
    for expression in [
        "export default { test: { projects: entries } };\nconst entries = glob.sync('./pkg/*');",
        "function f() { const entries = glob.sync('./pkg/*'); }\nexport default { test: { projects: entries } };",
        "const entries = glob.sync('./pkg/*');\nexport default { test: { projects: entries || [] } };",
    ] {
        write(directory.path(), "vitest.config.ts", &format!("import {{ glob }} from 'glob'\n{expression}"));
        let set = resolve_projects(directory.path());
        assert!(set.has_unresolved_entries, "{expression}");
    }
}

/// Verify glob call must be the complete value.
#[test]
fn review_glob_call_must_be_the_complete_value() {
    let directory = review_fixture("pkg/a/vitest.config.ts", "export default { test: {} }");
    for suffix in [" ? [] : other", ".map(x => x)", " || []", " + []"] {
        for expression in [format!("export default {{ test: {{ projects: glob.sync('./pkg/*'){suffix} }} }}"), format!("const entries = glob.sync('./pkg/*'){suffix};\nexport default {{ test: {{ projects: entries }} }}")] {
            write(directory.path(), "vitest.config.ts", &format!("import {{ glob }} from 'glob'\n{expression}"));
            let set = resolve_projects(directory.path());
            assert!(set.has_unresolved_entries, "{expression}");
        }
    }
}

/// Verify merge cycles and empty overrides.
#[test]
fn review_merge_cycles_and_empty_overrides() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "a.ts", "import { mergeConfig } from 'vitest/config'\nimport base from './b'\nexport default mergeConfig(base, {})");
    write(directory.path(), "b.ts", "import { mergeConfig } from 'vitest/config'\nimport base from './a'\nexport default mergeConfig(base, {})");
    assert!(resolve_standalone_config(&directory.path().join("a.ts")).is_none());
    write(
        directory.path(),
        "b.ts",
        "export default { test: { globals: true } }",
    );
    let (_, fields) = resolve_standalone_config(&directory.path().join("a.ts")).unwrap();
    assert!(matches!(fields.globals, GlobalsDetection::Enabled { .. }));
    write(directory.path(), "a.ts", "import { mergeConfig } from 'vitest/config'\nimport base from './missing'\nexport default mergeConfig(base, { test: { globals: true } })");
    let (_, fields) = resolve_standalone_config(&directory.path().join("a.ts")).unwrap();
    assert!(matches!(fields.globals, GlobalsDetection::Enabled { .. }));
    assert!(matches!(fields.include, GlobPatterns::Unknown { .. }));
}

/// Verify glob budget exhaustion reports truncation.
#[test]
fn review_glob_budget_exhaustion_reports_truncation() {
    let directory = tempfile::tempdir().unwrap();
    let source =
        "import { glob } from 'glob'\nexport default { test: { projects: glob.sync('./pkg/*') } }";
    let masked = mask_non_code(source).unwrap();
    let original: Vec<char> = source.chars().collect();
    let ConfigObject::Found(open, close) = locate_config_object(&masked) else {
        panic!("literal config")
    };
    let TestObject::Found(test_open, test_close) = locate_test_object(&masked, open + 1..close)
    else {
        panic!("literal test")
    };
    let mut budget = Budget {
        entries_visited: MAX_GLOB_WALK_ENTRIES,
    };
    let set = resolve_projects_array(
        directory.path(),
        &directory.path().join("vitest.config.ts"),
        &masked,
        &original,
        test_open + 1..test_close,
        None,
        &mut budget,
    );
    assert!(set.has_unresolved_entries);
    assert!(set.truncated);
}

/// Verify computed shapes refuse malformed calls and declarations.
#[test]
fn review_computed_shapes_refuse_malformed_calls_and_declarations() {
    for expression in [
        "mergeConfigX(base, {})",
        "mergeConfig",
        "mergeConfig(base)",
        "mergeConfig(base, variable)",
        "mergeConfig(base + other, {})",
        "other(base, {})",
    ] {
        let source =
            format!("import {{ mergeConfig }} from 'vitest/config'; export default {expression};");
        let original: Vec<char> = source.chars().collect();
        let masked = mask_non_code(&source).unwrap();
        let value = find_export_default_value_start(&masked).unwrap();
        assert!(
            merge_config_overrides_literal(&masked, &original, value).is_none(),
            "{expression}"
        );
    }
    for source in [
        "glob.other('./pkg/*')",
        "glob.sync",
        "glob.sync('./pkg/*'",
        "true",
        "false",
        "null",
        "undefined",
        "const entries: string[] = glob.sync('./pkg/*');\nentries",
        "const different = 1;\nentries",
    ] {
        let original: Vec<char> = source.chars().collect();
        let masked = mask_non_code(source).unwrap_or_else(|| original.clone());
        let start = source.rfind('\n').map_or(0, |at| at + 1);
        assert!(
            read_glob_sync_literal(&masked, &original, start..original.len()).is_none(),
            "{source}"
        );
    }
    let source = "import { glob };";
    assert!(!is_imported_from_package(
        &mask_non_code(source).unwrap(),
        &source.chars().collect::<Vec<_>>(),
        "glob",
        &["glob"]
    ));
    let missing = tempfile::tempdir().unwrap();
    assert!(resolve_standalone_config(&missing.path().join("missing.ts")).is_none());
    assert!(resolve_standalone_config(missing.path()).is_none());
}

/// Verify partial glob results keep unknown governance.
#[test]
fn review_partial_glob_results_keep_unknown_governance() {
    let directory = review_fixture(
        "pkg/a/vitest.config.ts",
        "export default { test: { globals: true } }",
    );
    write(
        directory.path(),
        "pkg/b/vitest.config.ts",
        "export default { test: {} }",
    );
    let source =
        "import { glob } from 'glob'\nexport default { test: { projects: glob.sync('./pkg/*') } }";
    let masked = mask_non_code(source).unwrap();
    let original: Vec<char> = source.chars().collect();
    let test_start = original.iter().position(|ch| *ch == ':').unwrap() + 2;
    let test_end = find_matching_delim(&masked, test_start, '{', '}').unwrap();
    let mut budget = Budget {
        entries_visited: MAX_GLOB_WALK_ENTRIES - 2,
    };
    let set = resolve_projects_array(
        directory.path(),
        &directory.path().join("vitest.config.ts"),
        &masked,
        &original,
        test_start + 1..test_end,
        None,
        &mut budget,
    );
    assert_eq!(set.projects.len(), 1, "{set:?}");
    assert!(set.truncated && set.has_unresolved_entries);
    assert!(set
        .diagnostics
        .iter()
        .any(|note| note.contains("budget exhausted")));
    assert!(matches!(
        set.governing_project(&set.projects[0].base_dir.join("a.test.ts")),
        ProjectGovernance::Unknown { .. }
    ));
}

/// Verify merge chain is bounded.
#[test]
fn review_merge_chain_is_bounded() {
    let directory = tempfile::tempdir().unwrap();
    for n in 0..40 {
        write(directory.path(), &format!("chain{n}.ts"), &format!("import {{ mergeConfig }} from 'vitest/config'\nimport base from './chain{}'\nexport default mergeConfig(base, {{ test: {{}} }})", n + 1));
    }
    write(
        directory.path(),
        "chain40.ts",
        "export default { test: { globals: true } }",
    );
    assert!(resolve_standalone_config(&directory.path().join("chain0.ts")).is_none());
}

/// B3: `test.projects: glob.sync('<literal>')` through one const hop -- the exact shape a
/// large React monorepo's root config uses. 575 of 575 files undetermined before it.
#[test]
fn glob_sync_literal_project_list_through_a_const_resolves() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "import { glob } from 'glob'\nconst entries = glob.sync('./packages/*/vitest.config.ts')\nexport default defineConfig({ test: { projects: entries } });",
    );
    write(
        directory.path(),
        "packages/alpha/vitest.config.ts",
        "export default defineConfig({ test: { globals: true } });",
    );
    write(
        directory.path(),
        "packages/beta/vitest.config.ts",
        "export default defineConfig({ test: {} });",
    );
    let set = resolve_projects(directory.path());
    assert!(!set.has_unresolved_entries, "{:?}", set.diagnostics);
    assert_eq!(set.projects.len(), 2);
    let globals: Vec<_> = set
        .projects
        .iter()
        .map(|project| matches!(project.globals, GlobalsDetection::Enabled { .. }))
        .collect();
    assert!(globals.contains(&true), "alpha's globals: {globals:?}");
    assert!(globals.contains(&false), "beta's default: {globals:?}");
}

/// The direct spelling -- no const hop.
#[test]
fn glob_sync_literal_project_list_written_directly_resolves() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "import { glob } from 'glob'\nexport default defineConfig({ test: { projects: glob.sync('./pkgs/*/vitest.config.ts') } });",
    );
    write(
        directory.path(),
        "pkgs/one/vitest.config.ts",
        "export default defineConfig({ test: { globals: true } });",
    );
    let set = resolve_projects(directory.path());
    assert!(!set.has_unresolved_entries, "{:?}", set.diagnostics);
    assert_eq!(set.projects.len(), 1);
    assert!(matches!(
        set.projects[0].globals,
        GlobalsDetection::Enabled { .. }
    ));
}

/// A `glob` that is NOT the package's (no import, or a different specifier) refuses exactly
/// as before -- the verified-import check is load-bearing, not decorative.
#[test]
fn glob_sync_with_an_unverified_glob_identifier_stays_unresolved() {
    for (label, header) in [
        ("no import at all", ""),
        (
            "imported from elsewhere",
            "import { glob } from './my-glob-thing'\n",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            &format!(
                "{header}const entries = glob.sync('./x/*/vitest.config.ts')\nexport default defineConfig({{ test: {{ projects: entries }} }});"
            ),
        );
        let set = resolve_projects(directory.path());
        assert!(
            set.has_unresolved_entries,
            "{label}: an unverified glob must refuse, not resolve"
        );
    }
}

/// A non-literal pattern is a runtime value; refused.
#[test]
fn glob_sync_with_a_variable_pattern_stays_unresolved() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "import { glob } from 'glob'\nimport { pattern } from './patterns'\nexport default defineConfig({ test: { projects: glob.sync(pattern) } });",
    );
    let set = resolve_projects(directory.path());
    assert!(set.has_unresolved_entries);
}

/// Omitted fields inherit from an imported merge base; scalars retain their provenance.
#[test]
fn merge_config_with_an_imported_base_merges_like_extends() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "internal/preset.ts",
        "import { defineConfig } from 'vitest/config'\nexport default defineConfig({ test: { globals: true, include: ['src/**/*.{test,spec}.ts'] } });",
    );
    write(
        directory.path(),
        "packages/state/vitest.config.ts",
        "import { mergeConfig } from 'vitest/config'\nimport base from '../../internal/preset'\nexport default mergeConfig(base, { test: { environment: 'node' } });",
    );
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: ['./packages/state'] } });",
    );
    let set = resolve_projects(directory.path());
    assert!(!set.has_unresolved_entries, "{:?}", set.diagnostics);
    assert_eq!(set.projects.len(), 1);
    // The base's globals: true survives a merge whose overrides do not mention it.
    assert!(
        matches!(set.projects[0].globals, GlobalsDetection::Enabled { .. }),
        "actual: {:?}",
        set.projects[0].globals
    );
}

/// The overrides may arrive wrapped in a verified `defineProject(...)`.
#[test]
fn merge_config_with_define_project_wrapped_overrides_resolves() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "root.ts",
        "export default defineConfig({ test: { globals: true } });",
    );
    write(
        directory.path(),
        "pkg/vitest.config.ts",
        "import { defineProject, mergeConfig } from 'vitest/config'\nimport root from '../root'\nexport default mergeConfig(root, defineProject({ test: { typecheck: { tsconfig: './tsconfig.json' } } }));",
    );
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: ['./pkg'] } });",
    );
    let set = resolve_projects(directory.path());
    assert!(!set.has_unresolved_entries, "{:?}", set.diagnostics);
    assert!(matches!(
        single_project(&set).globals,
        GlobalsDetection::Enabled { .. }
    ));
}

/// A merged nested config contributes both sides to the cross-file comparison scope.
///
/// This is the unit-level contract behind the recall fixture: accepting the observed
/// `as object` suffix must not erase a differing base or turn an unresolved package import
/// into a comparable scope.
#[test]
fn merge_config_cast_scopes_preserve_differences_and_uncertainty() {
    let directory = tempfile::tempdir().unwrap();
    for package in ["a", "b", "same"] {
        write(
            directory.path(),
            &format!("{package}/base.ts"),
            "export default { test: { environment: 'node' } };",
        );
        write(
            directory.path(),
            &format!("{package}/vitest.config.ts"),
            "import { defineConfig, mergeConfig } from 'vitest/config';\n\
             import base from './base.js';\n\
             export default mergeConfig(base, defineConfig({ test: { setupFiles: ['./setup.ts'] } })) as object;",
        );
    }
    write(
        directory.path(),
        "b/base.ts",
        "export default { test: { environment: 'happy-dom' } };",
    );

    let scope = |package: &str| {
        resolved_config_test_scope(&directory.path().join(package).join("vitest.config.ts"))
    };
    let a = scope("a").expect("the observed cast form is static");
    assert_eq!(a, scope("same").unwrap(), "identical chains compare");
    assert_ne!(a, scope("b").unwrap(), "a differing base stays distinct");

    write(
        directory.path(),
        "dynamic/vitest.config.ts",
        "import { mergeConfig } from 'vitest/config';\n\
         import base from '@scope/runtime-config';\n\
         export default mergeConfig(base, { test: { environment: 'node' } }) as object;",
    );
    assert!(
        scope("dynamic").is_err(),
        "an imported package is not a statically resolved base"
    );

    write(
        directory.path(),
        "other-cast/base.ts",
        "export default { test: { environment: 'node' } };",
    );
    write(
        directory.path(),
        "other-cast/vitest.config.ts",
        "import { mergeConfig } from 'vitest/config';\n\
         import base from './base';\n\
         export default mergeConfig(base, { test: {} }) as UserConfig;",
    );
    assert!(
        scope("other-cast").is_err(),
        "the narrow assertion reader must not grow into a partial TypeScript parser"
    );
}

/// A merge whose two inputs contribute no `test` object is not a comparison scope.
///
/// The effective Vitest test configuration is absent in that case, exactly as it is for an
/// ordinary config with no `test` object. One-sided scopes remain present, and their position
/// remains part of the conservative merge identity rather than being flattened speculatively.
#[test]
fn testless_merge_configs_are_absent_while_one_sided_scopes_remain() {
    let directory = tempfile::tempdir().unwrap();
    for (package, base, override_body) in [
        (
            "neither",
            "export default { plugins: [] };",
            "{ resolve: {} }",
        ),
        (
            "base-only",
            "export default { test: { environment: 'node' } };",
            "{ resolve: {} }",
        ),
        (
            "base-only-same",
            "export default { test: { environment: 'node' } };",
            "{ resolve: {} }",
        ),
        (
            "base-only-different",
            "export default { test: { environment: 'happy-dom' } };",
            "{ resolve: {} }",
        ),
        (
            "override-only",
            "export default { plugins: [] };",
            "{ test: { environment: 'node' } }",
        ),
    ] {
        write(directory.path(), &format!("{package}/base.ts"), base);
        write(
            directory.path(),
            &format!("{package}/vitest.config.ts"),
            &format!(
                "import {{ mergeConfig }} from 'vitest/config';\n\
                 import base from './base';\n\
                 export default mergeConfig(base, {override_body});"
            ),
        );
    }

    let scope = |package: &str| {
        resolved_config_test_scope(&directory.path().join(package).join("vitest.config.ts"))
            .expect("the merge shape is statically readable")
    };

    assert_eq!(
        scope("neither"),
        None,
        "two absent test objects must not synthesize a scope"
    );
    let base_only = scope("base-only");
    assert!(base_only.is_some(), "the base's test object must survive");
    assert_eq!(
        base_only,
        scope("base-only-same"),
        "identical one-sided scopes must compare"
    );
    assert_ne!(
        base_only,
        scope("base-only-different"),
        "a conflicting base scope must remain distinct"
    );
    assert!(
        scope("override-only").is_some(),
        "the override's test object must survive"
    );
}

/// Ordinary configs retain the established scope reader's compatibility surface.
#[test]
fn resolved_scope_delegates_non_merge_configs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("vitest.config.ts");
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { environment: 'node' } as UserConfig['test'] });",
    );

    assert_eq!(
        resolved_config_test_scope(&path),
        crate::vitest_config::config_test_scope(&path),
        "the merge-aware entrypoint must not narrow ordinary config parsing"
    );
    assert!(resolved_config_test_scope(&path).unwrap().is_some());

    write(
        directory.path(),
        "vitest.config.ts",
        "import { mergeConfig } from 'vitest/config';\n\
         export default mergeConfig({ test: { environment: 'node' } }, { test: { environment: 'happy-dom' } });",
    );
    assert!(
        resolved_config_test_scope(&path).is_err(),
        "an unsupported merge must not fall through to the generic reader's first argument"
    );
}

/// TypeScript resolves source files behind the runtime extension used by ESM imports.
#[test]
fn merge_config_bases_follow_typescript_source_extensions() {
    let directory = tempfile::tempdir().unwrap();
    for (runtime_extension, source_extension) in [("js", "ts"), ("mjs", "mts"), ("cjs", "cts")] {
        let package = format!("{runtime_extension}-base");
        write(
            directory.path(),
            &format!("{package}/base.{source_extension}"),
            "export default { test: { environment: 'node' } };",
        );
        write(
            directory.path(),
            &format!("{package}/vitest.config.ts"),
            &format!(
                "import {{ mergeConfig }} from 'vitest/config';\n\
                 import base from './base.{runtime_extension}';\n\
                 export default mergeConfig(base, {{ test: {{ globals: true }} }});"
            ),
        );
        assert!(
            resolved_config_test_scope(&directory.path().join(package).join("vitest.config.ts"))
                .is_ok(),
            "a .{runtime_extension} import must find its .{source_extension} source"
        );
    }
}

/// An override that sets globals itself WINS over the base -- mergeConfig's scalar rule.
#[test]
fn merge_config_overrides_win_scalars_over_the_base() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "preset.ts",
        "export default defineConfig({ test: {} });",
    );
    write(
        directory.path(),
        "pkg/vitest.config.ts",
        "import { mergeConfig } from 'vitest/config'\nimport base from '../preset'\nexport default mergeConfig(base, { test: { globals: true } });",
    );
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: ['./pkg'] } });",
    );
    let set = resolve_projects(directory.path());
    assert!(matches!(
        single_project(&set).globals,
        GlobalsDetection::Enabled { .. }
    ));
}

/// Verify root merge config resolves merged fields and projects.
#[test]
fn root_merge_config_resolves_merged_fields_and_projects() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "base.ts",
        "export default { test: { globals: false, include: ['base/**'] } };",
    );
    write(
        directory.path(),
        "vitest.config.ts",
        "import { mergeConfig } from 'vitest/config'\nimport base from './base'\nexport default mergeConfig(base, { test: { globals: true, projects: [{ test: { name: 'child' } }] } });",
    );

    let set = resolve_projects(directory.path());
    let project = single_project(&set);
    assert_eq!(project.name.as_deref(), Some("child"));
    assert!(matches!(project.globals, GlobalsDetection::Enabled { .. }));
    assert_eq!(project.include, owned(&["base/**"]));

    write(
        directory.path(),
        "vitest.config.ts",
        "import { mergeConfig } from 'vitest/config'\nimport base from './base'\nexport default mergeConfig(base, { test: { globals: true } });",
    );
    let set = resolve_projects(directory.path());
    let project = single_project(&set);
    assert!(matches!(project.globals, GlobalsDetection::Enabled { .. }));
    assert_eq!(project.include, owned(&["base/**"]));
}

/// Verify unreadable existing merge base keeps overrides and marks arrays unknown.
#[test]
fn unreadable_existing_merge_base_keeps_overrides_and_marks_arrays_unknown() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "base.ts", "export default makeConfig();");
    write(
        directory.path(),
        "pkg/vitest.config.ts",
        "import { mergeConfig } from 'vitest/config'\nimport base from '../base'\nexport default mergeConfig(base, { test: { globals: true, include: ['own/**'] } });",
    );
    write(
        directory.path(),
        "vitest.config.ts",
        "export default { test: { projects: ['./pkg'] } };",
    );

    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 1, "{set:?}");
    assert!(matches!(
        set.projects[0].globals,
        GlobalsDetection::Enabled { .. }
    ));
    assert!(matches!(
        set.projects[0].include,
        GlobPatterns::Unknown { .. }
    ));
}

/// Unresolved imports keep explicit scalar overrides but leave discovery arrays unknown.
#[test]
fn merge_config_with_an_unresolvable_base_uses_own_fields_only() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "pkg/vitest.config.ts",
        "import { mergeConfig } from 'vitest/config'\nimport base from '@scope/shared-config'\nexport default mergeConfig(base, { test: { globals: true } });",
    );
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: ['./pkg'] } });",
    );
    let set = resolve_projects(directory.path());
    assert!(
        !set.has_unresolved_entries,
        "the project itself is readable: {:?}",
        set.diagnostics
    );
    assert!(matches!(
        single_project(&set).globals,
        GlobalsDetection::Enabled { .. }
    ));
}

/// Verify no config at all is not unresolved.
#[test]
fn no_config_at_all_is_not_unresolved() {
    // Case (1): nothing was missed here -- there is no Vitest config at all, so there is no
    // `test.projects`/workspace entry to have failed to resolve. `has_unresolved_entries`
    // must stay false, or every file in an ordinary config-less repository would resolve to
    // `ProjectGovernance::Unknown` instead of `NotIncluded`, and a caller that fails closed on
    // `Unknown` (as `ProjectSet::governed_globals_for` callers must) would wrongly treat every
    // such file as incomplete.
    let directory = tempfile::tempdir().unwrap();
    let set = resolve_projects(directory.path());
    assert!(set.projects.is_empty());
    assert!(!set.has_unresolved_entries);
    assert!(!set.config_found);
    let file = directory.path().join("thing.test.ts");
    assert_eq!(set.governing_project(&file), ProjectGovernance::NotIncluded);
}

/// Verify single project repo resolves like detect globals.
#[test]
fn single_project_repo_resolves_like_detect_globals() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { globals: true, include: ['src/**/*.test.ts'] } });",
    );
    let set = resolve_projects(directory.path());
    assert!(matches!(
        single_project(&set).globals,
        GlobalsDetection::Enabled { .. }
    ));
    assert_eq!(
        set.projects[0].include,
        GlobPatterns::Known(vec!["src/**/*.test.ts".to_owned()])
    );

    let file = directory.path().join("src/a.test.ts");
    match set.governing_project(&file) {
        ProjectGovernance::Governed(project) => {
            assert!(matches!(project.globals, GlobalsDetection::Enabled { .. }))
        }
        other => panic!("expected Governed, got {other:?}"),
    }
}

/// Verify inline projects extending root inherit its globals.
#[test]
fn inline_projects_extending_root_inherit_its_globals() {
    // The real shape of a public Vue framework: root sets `globals: true`, each inline project sets
    // `extends: true` explicitly and only overrides `include`/`name`.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                globals: true,
                projects: [
                    { extends: true, test: { name: 'unit', include: ['packages/a/**/*.test.ts'] } },
                    { extends: true, test: { name: 'jsdom', include: ['packages/b/**/*.test.ts'], environment: 'jsdom' } },
                ],
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 2);
    for project in &set.projects {
        assert!(
            matches!(project.globals, GlobalsDetection::Enabled { .. }),
            "{:?} should inherit the root's globals: true, got {:?}",
            project.name,
            project.globals
        );
    }
    assert_eq!(set.projects[0].name.as_deref(), Some("unit"));
    assert_eq!(set.projects[1].name.as_deref(), Some("jsdom"));
}

/// Verify extends false diverges from the root.
#[test]
fn extends_false_diverges_from_the_root() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                globals: true,
                projects: [
                    { extends: false, test: { name: 'isolated' } },
                ],
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 1);
    // `extends: false` means Vitest's own default (`globals: false`) applies, not the root's
    // `globals: true` -- the whole point of resolving configuration per project.
    assert!(matches!(
        set.projects[0].globals,
        GlobalsDetection::Disabled { .. }
    ));
}

/// Verify extends by self referencing string path inherits like extends true.
#[test]
fn extends_by_self_referencing_string_path_inherits_like_extends_true() {
    // A real observed shape: `extends: './vitest.config.ts'` pointing back at the very file the
    // entry is declared in.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                globals: true,
                projects: [
                    { extends: './vitest.config.ts', test: { name: 'browser' } },
                ],
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    assert!(matches!(
        single_project(&set).globals,
        GlobalsDetection::Enabled { .. }
    ));
}

/// Verify projects referencing separate config files are resolved independently.
#[test]
fn projects_referencing_separate_config_files_are_resolved_independently() {
    // A real observed shape: a directory glob (`packages/*`) plus an explicit file path.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: { projects: ['packages/*', './vitest.compile.config.ts'] },
        });"#,
    );
    write(
        directory.path(),
        "packages/a/vitest.config.ts",
        "export default { test: { globals: true } };",
    );
    write(
        directory.path(),
        "packages/b/vitest.config.ts",
        "export default { test: { globals: false } };",
    );
    write(
        directory.path(),
        "vitest.compile.config.ts",
        "export default { test: { globals: true, include: ['compile/**/*.test.ts'] } };",
    );

    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 3, "{:#?}", set.projects);
    let by_config: std::collections::BTreeMap<_, _> = set
        .projects
        .iter()
        .map(|project| {
            (
                project
                    .config_path
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
                project.globals.clone(),
            )
        })
        .collect();
    assert!(matches!(
        by_config.get("vitest.compile.config.ts"),
        Some(GlobalsDetection::Enabled { .. })
    ));
    // Independent files never inherit from the root or each other.
    let a = directory.path().join("packages/a/x.test.ts");
    let b = directory.path().join("packages/b/y.test.ts");
    assert!(matches!(
        set.governing_project(&a),
        ProjectGovernance::Governed(p) if matches!(p.globals, GlobalsDetection::Enabled { .. })
    ));
    assert!(matches!(
        set.governing_project(&b),
        ProjectGovernance::Governed(p) if matches!(p.globals, GlobalsDetection::Disabled { .. })
    ));
}

/// Verify workspace file alongside a root config is used instead of it.
#[test]
fn workspace_file_alongside_a_root_config_is_used_instead_of_it() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default { test: { globals: true } };",
    );
    write(
        directory.path(),
        "vitest.workspace.ts",
        r#"export default [
            { test: { name: 'a', globals: true, include: ['a/**/*.test.ts'] } },
            { test: { name: 'b', include: ['b/**/*.test.ts'] } },
        ];"#,
    );

    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 2);
    assert_eq!(set.projects[0].name.as_deref(), Some("a"));
    assert!(matches!(
        set.projects[0].globals,
        GlobalsDetection::Enabled { .. }
    ));
    // The second entry does not set `globals` itself and a workspace entry has no root `test`
    // object to inherit from -- must not silently borrow the sibling root config's `true`.
    assert!(matches!(
        set.projects[1].globals,
        GlobalsDetection::Unknown { .. }
    ));
}

/// Verify a file matching two projects is governed by the last declared one.
#[test]
fn a_file_matching_two_projects_is_governed_by_the_last_declared_one() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                projects: [
                    { test: { name: 'first', globals: true, include: ['shared/**/*.test.ts'] } },
                    { test: { name: 'second', globals: false, include: ['shared/**/*.test.ts'] } },
                ],
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    let file = directory.path().join("shared/a.test.ts");
    match set.governing_project(&file) {
        ProjectGovernance::Governed(project) => {
            assert_eq!(project.name.as_deref(), Some("second"));
            assert!(matches!(project.globals, GlobalsDetection::Disabled { .. }));
        }
        other => panic!("expected Governed(second), got {other:?}"),
    }
}

/// Verify a file matching no project is not included.
#[test]
fn a_file_matching_no_project_is_not_included() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                projects: [
                    { test: { name: 'only', include: ['only/**/*.test.ts'] } },
                ],
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    let file = directory.path().join("elsewhere/a.test.ts");
    assert_eq!(set.governing_project(&file), ProjectGovernance::NotIncluded);
}

/// Verify per project include excludes a cy file even under globals true.
#[test]
fn per_project_include_excludes_a_cy_file_even_under_globals_true() {
    // A `.cy.ts` file outside the resolved `include` must never be treated as governed,
    // which is what lets a caller refuse to analyze it as a Vitest test under `globals: true`.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                globals: true,
                include: ['src/**/*.test.ts'],
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    let cypress_file = directory.path().join("src/component.cy.ts");
    assert_eq!(
        set.governing_project(&cypress_file),
        ProjectGovernance::NotIncluded
    );
    let vitest_file = directory.path().join("src/component.test.ts");
    assert!(matches!(
        set.governing_project(&vitest_file),
        ProjectGovernance::Governed(_)
    ));
}

/// Verify unresolvable projects value forces unknown rather than not included.
#[test]
fn unresolvable_projects_value_forces_unknown_rather_than_not_included() {
    // A real observed shape: `projects: vitestPackages`, a variable, not a literal array.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: vitestPackages } });",
    );
    let set = resolve_projects(directory.path());
    assert!(set.projects.is_empty());
    assert!(set.has_unresolved_entries);
    // Case (3): a config file exists (`config_found`) but its project list could not be
    // read -- unlike the no-config case, something genuinely was missed here.
    assert!(set.config_found);
    let file = directory.path().join("packages/a/src/a.test.ts");
    assert!(matches!(
        set.governing_project(&file),
        ProjectGovernance::Unknown { .. }
    ));
}

/// Verify explicit empty projects array is found but declares no projects.
#[test]
fn explicit_empty_projects_array_is_found_but_declares_no_projects() {
    // Case (2): the user explicitly declared zero projects. Distinct from case (1) (no config
    // at all) via `config_found`, and distinct from case (3) (unreadable project list) via
    // `has_unresolved_entries` staying false here.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: [] } });",
    );
    let set = resolve_projects(directory.path());
    assert!(set.projects.is_empty());
    assert!(!set.has_unresolved_entries);
    assert!(set.config_found);
}

/// `[...defaultExclude, 'extra']` reads as Vitest's own default list plus the extra.
///
/// Both halves matter and both are asserted. Substituting is reading Vitest's API surface --
/// `defaultExclude` IS `DEFAULT_EXCLUDE_GLOBS`, which this module already mirrors from
/// `docs/vitest-surface.md` -- but only when the binding really is Vitest's. A project's own
/// `defaultExclude` is a different value, and a name-only check could not tell them apart,
/// which is the exact failure mode this repository keeps re-deriving.
///
/// Measured cost of getting this wrong: one CSS engine writes `exclude: [...defaultExclude, ...]` in
/// the config that governs its whole repository, so an `Unknown` exclude gave **100 of its
/// 104 files** an undetermined globals mode.
#[test]
fn a_spread_of_vitests_own_default_exclude_is_read_but_only_when_imported_from_vitest() {
    let with_vitest_import = |import: &str| {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            &format!(
                "{import}\nexport default defineConfig({{ test: {{ \
                 exclude: [...defaultExclude, '**/scoped/**'] }} }});"
            ),
        );
        resolve_projects(directory.path())
    };

    let set = with_vitest_import("import { defaultExclude, defineConfig } from 'vitest/config'");
    let project = &set.projects[0];
    match &project.exclude {
        GlobPatterns::Known(patterns) => {
            assert!(
                patterns.iter().any(|p| p == "**/scoped/**"),
                "the written element survives: {patterns:?}"
            );
            assert!(
                patterns.len() > 1,
                "and the spread contributed Vitest's own defaults: {patterns:?}"
            );
            for glob in DEFAULT_EXCLUDE_GLOBS {
                assert!(
                    patterns.iter().any(|p| p == glob),
                    "`{glob}` is part of Vitest's defaultExclude and must be here: {patterns:?}"
                );
            }
        }
        other => panic!("expected a Known exclude, got {other:?}"),
    }

    // Adversarial shapes the scan must refuse, all from review. Each pairs a LOCAL
    // `defaultExclude` with a later `vitest` import, so a check that lets a clause cross a
    // statement boundary would substitute Vitest's defaults for a project's own value --
    // breaking the "VERIFIED" guarantee this function's doc makes.
    for hostile in [
        // `import` inside an identifier: the scan used to match it and start a clause
        // mid-token.
        // A real exploit, not a hypothetical: with BOTH guards removed this input
        // substitutes Vitest's nine default excludes for a binding imported from
        // `./our-globs`. The scan matches `import` inside `reimport`, so the clause starts
        // mid-token, runs past the shorthand `{ defaultExclude }` and on to the NEXT
        // statement's `from 'vitest/config'` -- name from one statement, specifier from
        // another.
        //
        // Either guard alone stops this one, so removing one and seeing green proves nothing
        // about that guard. They are kept as two because they refuse different shapes: the
        // boundary check rejects a clause that began mid-identifier, the `;`/second-`import`
        // check rejects one that crossed a statement boundary however it started.
        "import { defaultExclude } from './our-globs'\nconst reimport = { defaultExclude }\nimport { defineConfig } from 'vitest/config'",
        // A clause that would otherwise swallow a `;` and a second `import`.
        "import { defaultExclude } from './our-globs';import { defineConfig } from 'vitest/config'",
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            &format!(
                "{hostile}\nexport default defineConfig({{ test: {{ \
                 exclude: [...defaultExclude, '**/scoped/**'] }} }});"
            ),
        );
        let hostile_set = resolve_projects(directory.path());
        assert!(
            matches!(hostile_set.projects[0].exclude, GlobPatterns::Unknown { .. }),
            "`defaultExclude` here is the project's own; substituting Vitest's defaults would \
             silently widen what this config excludes. Input:\n{hostile}\nGot: {:?}",
            hostile_set.projects[0].exclude
        );
    }

    // The same spelling bound to something else stays Unknown. Nothing about the array
    // changed -- only where the name came from.
    let local = with_vitest_import(
        "import { defineConfig } from 'vitest/config'\nimport { defaultExclude } from './our-globs'",
    );
    assert!(
        matches!(local.projects[0].exclude, GlobPatterns::Unknown { .. }),
        "a project's own `defaultExclude` is not Vitest's: {:?}",
        local.projects[0].exclude
    );
}

/// Verify self referencing project entry reads the configs own fields without looping.
#[test]
fn self_referencing_project_entry_reads_the_configs_own_fields_without_looping() {
    // A real observed shape: the root config lists its own file among `test.projects`.
    //
    // CHANGED. This used to assert `Unknown`, on the reasoning that re-reading the file finds
    // the same nested `test.projects` and must not loop. The no-loop half is right and is
    // still asserted below; the `Unknown` half was over-broad, and it cost that project 102 of its
    // 104 files an undetermined globals mode on the pinned corpus. Nothing follows a nested
    // `projects` list, so reading the file's OTHER `test` fields cannot loop -- and those
    // fields are exactly what Vitest means by listing your own config as a project.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                name: 'unit',
                projects: ['vitest.config.ts'],
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    // The no-loop assertion: one entry in, one project out.
    assert_eq!(set.projects.len(), 1);
    // The fields came through -- `name` is the proof the object was read at all.
    assert_eq!(set.projects[0].name.as_deref(), Some("unit"));
    // And the answer is the config's own: it sets no `globals`, so Vitest's default applies.
    // `Unknown` here would be refusing to read a value that is written down.
    assert!(
        matches!(set.projects[0].globals, GlobalsDetection::Disabled { .. }),
        "a self-referencing entry's globals is its own config's: {:?}",
        set.projects[0].globals
    );
    assert!(
        !set.has_unresolved_entries,
        "the entry resolved, so nothing about this map is unresolved"
    );
}

/// Verify awaited factory call entry is read as an inline project.
#[test]
fn awaited_factory_call_entry_is_read_as_an_inline_project() {
    // A real observed shape: `await defineVitestProject({ test: { ... } })` as an array element.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                projects: [
                    await defineVitestProject({ test: { name: 'nuxt', globals: true } }),
                ],
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 1);
    assert_eq!(set.projects[0].name.as_deref(), Some("nuxt"));
    assert!(matches!(
        set.projects[0].globals,
        GlobalsDetection::Enabled { .. }
    ));
}

/// Verify truncates rather than resolving an unbounded number of projects.
#[test]
fn truncates_rather_than_resolving_an_unbounded_number_of_projects() {
    let directory = tempfile::tempdir().unwrap();
    let mut entries = String::new();
    for i in 0..(MAX_PROJECTS + 5) {
        entries.push_str(&format!("{{ test: {{ name: 'p{i}' }} }},\n"));
    }
    write(
        directory.path(),
        "vitest.config.ts",
        &format!("export default defineConfig({{ test: {{ projects: [{entries}] }} }});"),
    );
    let set = resolve_projects(directory.path());
    assert!(set.projects.len() <= MAX_PROJECTS);
    assert!(set.truncated);
    assert!(set.has_unresolved_entries);
}

// -- `includeSource` (docs/vitest-surface.md, "In-source testing") ------------------------

/// Verify include source defaults to a known empty list when absent.
#[test]
fn include_source_defaults_to_a_known_empty_list_when_absent() {
    // Vitest's own default is `[]`, not "unknown" -- an ordinary project that never mentions
    // in-source testing must resolve to a confident empty answer, not an uncertain one.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { globals: true } });",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 1);
    assert_eq!(set.projects[0].include_source, GlobPatterns::Known(vec![]));
}

/// Verify include source parses an explicit glob list.
#[test]
fn include_source_parses_an_explicit_glob_list() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: { includeSource: ['src/**/*.ts', 'lib/**/*.js'] },
        });"#,
    );
    let set = resolve_projects(directory.path());
    assert_eq!(
        set.projects[0].include_source,
        GlobPatterns::Known(vec!["src/**/*.ts".to_owned(), "lib/**/*.js".to_owned()])
    );
}

/// Verify include source is inherited through extends true but not extends false.
#[test]
fn include_source_is_inherited_through_extends_true_but_not_extends_false() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                includeSource: ['src/**/*.ts'],
                projects: [
                    { extends: true, test: { name: 'inherits' } },
                    { extends: false, test: { name: 'diverges' } },
                ],
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 2);
    assert_eq!(
        set.projects[0].include_source,
        GlobPatterns::Known(vec!["src/**/*.ts".to_owned()]),
        "extends: true should inherit the root's includeSource"
    );
    // `extends: false` falls back to Vitest's own default (`[]`), not the root's value --
    // the same rule per-project resolution already establishes for
    // `globals`/`include`/`exclude`.
    assert_eq!(
        set.projects[1].include_source,
        GlobPatterns::Known(vec![]),
        "extends: false should not inherit the root's includeSource"
    );
}

/// Verify unresolvable include source value resolves to unknown not a guess.
#[test]
fn unresolvable_include_source_value_resolves_to_unknown_not_a_guess() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { includeSource: someHelper() } });",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 1);
    assert!(matches!(
        set.projects[0].include_source,
        GlobPatterns::Unknown { .. }
    ));
}

// -- `benchmark.*` discovery -------------------------------------------------------------

/// Verify benchmark discovery fields use vitest defaults when absent.
#[test]
fn benchmark_discovery_fields_use_vitest_defaults_when_absent() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { globals: true } });",
    );
    let set = resolve_projects(directory.path());
    let project = &set.projects[0];
    assert_eq!(project.benchmark_include, default_benchmark_include());
    assert_eq!(project.benchmark_exclude, default_benchmark_exclude());
    assert_eq!(
        project.benchmark_include_source,
        GlobPatterns::Known(vec![])
    );
}

/// Verify benchmark discovery fields parse literal arrays and expand extglobs.
#[test]
fn benchmark_discovery_fields_parse_literal_arrays_and_expand_extglobs() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                benchmark: {
                    include: ['perf/**/*.?(c|m)ts'],
                    exclude: ['perf/generated/**'],
                    includeSource: ['src/perf/**/*.ts'],
                },
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    let project = &set.projects[0];
    assert_eq!(
        project.benchmark_include,
        GlobPatterns::Known(vec![
            "perf/**/*.ts".to_owned(),
            "perf/**/*.cts".to_owned(),
            "perf/**/*.mts".to_owned(),
        ])
    );
    assert_eq!(
        project.benchmark_exclude,
        GlobPatterns::Known(vec!["perf/generated/**".to_owned()])
    );
    assert_eq!(
        project.benchmark_include_source,
        GlobPatterns::Known(vec!["src/perf/**/*.ts".to_owned()])
    );
}

/// Verify unsupported benchmark include extglob diagnostic names benchmark field.
#[test]
fn unsupported_benchmark_include_extglob_diagnostic_names_benchmark_field() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: { benchmark: { include: ['packages/+(a|b)/**/*.bench.ts'] } },
        });"#,
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 1);
    match &set.projects[0].benchmark_include {
        GlobPatterns::Unknown { reason } => {
            assert!(
                reason.contains("`benchmark.include`"),
                "diagnostic should name the benchmark include field: {reason}"
            );
            assert!(
                reason.contains("+(a|b)"),
                "diagnostic should name the offending pattern: {reason}"
            );
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

/// Verify benchmark discovery fields follow project inheritance.
#[test]
fn benchmark_discovery_fields_follow_project_inheritance() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                benchmark: {
                    include: ['perf/**/*.perf.ts'],
                    exclude: ['perf/slow/**'],
                    includeSource: ['src/perf/**/*.ts'],
                },
                projects: [
                    { extends: true, test: { name: 'inherits' } },
                    { extends: false, test: { name: 'defaults' } },
                ],
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 2);
    assert_eq!(
        set.projects[0].benchmark_include,
        GlobPatterns::Known(vec!["perf/**/*.perf.ts".to_owned()])
    );
    assert_eq!(
        set.projects[0].benchmark_exclude,
        GlobPatterns::Known(vec!["perf/slow/**".to_owned()])
    );
    assert_eq!(
        set.projects[0].benchmark_include_source,
        GlobPatterns::Known(vec!["src/perf/**/*.ts".to_owned()])
    );
    assert_eq!(
        set.projects[1].benchmark_include,
        default_benchmark_include()
    );
    assert_eq!(
        set.projects[1].benchmark_exclude,
        default_benchmark_exclude()
    );
    assert_eq!(
        set.projects[1].benchmark_include_source,
        GlobPatterns::Known(vec![])
    );
}

/// Verify nonliteral benchmark config is unknown in every discovery channel.
#[test]
fn nonliteral_benchmark_config_is_unknown_in_every_discovery_channel() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { benchmark: makeBenchmarkConfig() } });",
    );
    let set = resolve_projects(directory.path());
    let project = &set.projects[0];
    for patterns in [
        &project.benchmark_include,
        &project.benchmark_exclude,
        &project.benchmark_include_source,
    ] {
        assert!(
            matches!(patterns, GlobPatterns::Unknown { reason } if reason.contains("benchmark")),
            "expected benchmark-specific unknown, got {patterns:?}"
        );
    }
}

// -- Extglob translation (a project's own `include`/`exclude`/`includeSource`) -----------

/// Verify expand extglob translates vitests own default include shape.
#[test]
fn expand_extglob_translates_vitests_own_default_include_shape() {
    let expanded = expand_extglob_optional_groups("**/*.{test,spec}.?(c|m)[jt]s?(x)").unwrap();
    for expected in [
        "**/*.{test,spec}.[jt]s",
        "**/*.{test,spec}.[jt]sx",
        "**/*.{test,spec}.c[jt]s",
        "**/*.{test,spec}.c[jt]sx",
        "**/*.{test,spec}.m[jt]s",
        "**/*.{test,spec}.m[jt]sx",
    ] {
        assert!(
            expanded.iter().any(|pattern| pattern == expected),
            "expected {expected} among {expanded:#?}"
        );
    }
    assert_eq!(expanded.len(), 6);
}

/// Verify expand extglob leaves a plain pattern with no extglob syntax untouched.
#[test]
fn expand_extglob_leaves_a_plain_pattern_with_no_extglob_syntax_untouched() {
    assert_eq!(
        expand_extglob_optional_groups("src/**/*.test.ts").unwrap(),
        vec!["src/**/*.test.ts".to_owned()]
    );
}

/// Verify expand extglob rejects unsupported extglob forms by name.
#[test]
fn expand_extglob_rejects_unsupported_extglob_forms_by_name() {
    for (pattern, operator) in [
        ("packages/+(a|b)/**", '+'),
        ("packages/@(a|b)/**", '@'),
        ("packages/!(a|b)/**", '!'),
        ("packages/*(a|b)/**", '*'),
    ] {
        let error = expand_extglob_optional_groups(pattern)
            .expect_err(&format!("{pattern} should be rejected, not guessed at"));
        assert!(
            error.contains(pattern),
            "diagnostic should name the offending pattern: {error}"
        );
        assert!(
            error.contains(&format!("{operator}(")),
            "diagnostic should name the unsupported operator: {error}"
        );
    }
}

/// Verify a projects entry extglob include governs its test file.
#[test]
fn a_projects_entry_extglob_include_governs_its_test_file() {
    // A real observed shape (found via a 3-test drop on the corpus): the `runtime` project's
    // own `include` spells out Vitest's default include shape literally, extglob and all.
    // `globset::Glob::new` accepts it without error and then matches nothing -- silently, so
    // the file falls through to `Unknown` governance and its ambient tests are never
    // extracted. This must resolve to `Governed`, not silently discover nothing.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                globals: true,
                projects: [
                    {
                        test: {
                            name: 'runtime',
                            include: ['test-dom/**/*.{test,spec}.?(c|m)[jt]s?(x)'],
                        },
                    },
                ],
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    let file = directory.path().join("test-dom/runtime-dom.test.ts");
    match set.governing_project(&file) {
        ProjectGovernance::Governed(project) => {
            assert_eq!(project.name.as_deref(), Some("runtime"));
            assert!(matches!(project.globals, GlobalsDetection::Enabled { .. }));
        }
        other => panic!("expected Governed(runtime), got {other:?}"),
    }
}

/// Verify an unsupported extglob form in a projects include marks it unknown with a
/// diagnostic.
#[test]
fn an_unsupported_extglob_form_in_a_projects_include_marks_it_unknown_with_a_diagnostic() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: { include: ['packages/+(a|b)/**/*.test.ts'] },
        });"#,
    );
    let set = resolve_projects(directory.path());
    let reason = unknown_include_reason(&set);
    assert!(
        reason.contains("+(a|b)"),
        "diagnostic should name the offending pattern: {reason}"
    );
    // Falls back to the built-in defaults rather than matching nothing, mirroring how
    // `discovery.rs` treats any other `Unknown` project `include`.
    let file = directory.path().join("packages/a/x.test.ts");
    assert!(matches!(
        set.governing_project(&file),
        ProjectGovernance::Unknown { .. }
    ));
}

// --- Copilot review #7, item 1: an unresolved entry can still out-rank a known match. -------

/// Verify unresolved entry declared after a matching project forces unknown.
#[test]
fn unresolved_entry_declared_after_a_matching_project_forces_unknown() {
    // `missing-project-dir/*` never resolves to anything, so it is exactly the kind of
    // unresolved `test.projects` entry that -- because it is declared *after* the project that
    // otherwise matches -- could itself have matched and won under "later occurrence wins".
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                projects: [
                    { test: { name: 'known', include: ['shared/**/*.test.ts'] } },
                    'missing-project-dir/*',
                ],
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    assert!(set.has_unresolved_entries);
    let file = directory.path().join("shared/a.test.ts");
    match set.governing_project(&file) {
        ProjectGovernance::Unknown { .. } => {}
        other => panic!(
            "a later unresolved entry could have matched and won; expected Unknown, got \
             {other:?}"
        ),
    }
}

/// Verify unresolved entry declared before a matching project still governs.
#[test]
fn unresolved_entry_declared_before_a_matching_project_still_governs() {
    // The same unresolved entry, but declared *before* the known match this time: "later
    // occurrence wins" already settled in the known project's favor regardless of what that
    // earlier, unresolved entry was, so this must not be over-corrected into `Unknown` too.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: {
                projects: [
                    'missing-project-dir/*',
                    { test: { name: 'known', include: ['shared/**/*.test.ts'] } },
                ],
            },
        });"#,
    );
    let set = resolve_projects(directory.path());
    assert!(set.has_unresolved_entries);
    let file = directory.path().join("shared/a.test.ts");
    match set.governing_project(&file) {
        ProjectGovernance::Governed(project) => {
            assert_eq!(project.name.as_deref(), Some("known"));
        }
        other => panic!(
            "the unresolved entry is declared before the match it cannot out-rank; expected \
             Governed, got {other:?}"
        ),
    }
}

// --- Copilot review #7, item 2: escapes must be decoded correctly or refused, never mangled. -

/// Verify read string literal decodes unicode and hex escapes like javascript.
#[test]
fn read_string_literal_decodes_unicode_and_hex_escapes_like_javascript() {
    // `\u0074` is JavaScript's escape for `t`. The decoded value must equal what a JS engine
    // would actually produce -- `tests/**/*.ts` -- not the mangled `u0074ests/**/*.ts` an
    // earlier version of this scan produced by reducing `\u0074` to a bare `u`.
    let source: Vec<char> = "'\\u0074ests/**/*.ts'".chars().collect();
    let len = source.len();
    assert_eq!(
        read_string_literal(&source, 0..len).as_deref(),
        Some("tests/**/*.ts")
    );

    // `\x41` and the `\u{...}` code-point form both decode to `A` (U+0041), same as JavaScript.
    let source: Vec<char> = "'\\x41-\\u{41}'".chars().collect();
    let len = source.len();
    assert_eq!(read_string_literal(&source, 0..len).as_deref(), Some("A-A"));
}

/// Verify read string literal rejects an escape it does not decode instead of mangling it.
#[test]
fn read_string_literal_rejects_an_escape_it_does_not_decode_instead_of_mangling_it() {
    // `\v` (vertical tab) is a real JavaScript escape this scan does not translate. The earlier
    // behavior silently dropped the backslash, turning `\v` into the literal letter `v` -- a
    // different, still valid-looking string instead of a refusal. It must return `None`.
    let source: Vec<char> = "'foo\\vbar'".chars().collect();
    let len = source.len();
    assert_eq!(read_string_literal(&source, 0..len), None);
}

/// Verify read string literal refuses legacy octal rather than decoding its leading zero.
#[test]
fn read_string_literal_refuses_legacy_octal_rather_than_decoding_its_leading_zero() {
    // `\0` is NUL, but only when no digit follows. `\01` is a legacy *octal* escape that
    // JavaScript reads as U+0001 -- decoding the `0` in isolation would yield NUL followed by
    // a literal `1`, a different string that still looks plausible. Refuse the literal.
    let source: Vec<char> = "'a\\01b'".chars().collect();
    let len = source.len();
    assert_eq!(read_string_literal(&source, 0..len), None);

    // The unambiguous form still decodes: `\0` with a non-digit after it is genuinely NUL.
    let source: Vec<char> = "'a\\0b'".chars().collect();
    let len = source.len();
    assert_eq!(
        read_string_literal(&source, 0..len).as_deref(),
        Some("a\0b")
    );
}

/// Verify an undecodable escape in a projects include is unknown not mangled.
#[test]
fn an_undecodable_escape_in_a_projects_include_is_unknown_not_mangled() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: { include: ['tests/\vfoo/**/*.ts'] },
        });"#,
    );
    let set = resolve_projects(directory.path());
    let reason = unknown_include_reason(&set);
    assert!(
        reason.contains("not a plain quoted string literal"),
        "unexpected reason: {reason}"
    );
}

// --- Copilot review #7, item 3: a projects entry matching nothing must set the flag. --------

/// Verify a literal projects entry matching no config file sets has unresolved entries.
#[test]
fn a_literal_projects_entry_matching_no_config_file_sets_has_unresolved_entries() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        r#"export default defineConfig({
            test: { projects: ['packages/does-not-exist'] },
        });"#,
    );
    let set = resolve_projects(directory.path());
    assert!(set.projects.is_empty());
    assert!(
        set.has_unresolved_entries,
        "a `test.projects` entry matching no config file must not look fully resolved"
    );
    // The caller-visible effect: a file discovery would otherwise have scoped under this
    // (silently skipped) entry must not be reported as confidently `NotIncluded`.
    let file = directory.path().join("elsewhere/a.test.ts");
    assert!(matches!(
        set.governing_project(&file),
        ProjectGovernance::Unknown { .. }
    ));
}

// --- `globals_for` / `governed_globals_for` / the unanimity shortcut ------------------------

/// Two inline projects whose `exclude` is a spread this scan cannot read, so any file they
/// include resolves to `ProjectGovernance::Unknown` and the `globals` answer has to come from
/// the unanimity shortcut (or stay `Unknown`). `a_globals`/`b_globals` are spliced in verbatim
/// so a caller can make the two agree, disagree, or be unreadable.
fn ambiguous_projects_config(a_globals: &str, b_globals: &str) -> String {
    format!(
        "export default defineConfig({{ test: {{ projects: [\n  \
         {{ test: {{ name: 'a', globals: {a_globals}, include: ['a/**/*.test.ts'], \
         exclude: configDefaults.exclude }} }},\n  \
         {{ test: {{ name: 'b', globals: {b_globals}, include: ['b/**/*.test.ts'], \
         exclude: configDefaults.exclude }} }},\n] }} }});"
    )
}

/// Verify globals for answers from the governing project and refuses for an unmatched file.
#[test]
fn globals_for_answers_from_the_governing_project_and_refuses_for_an_unmatched_file() {
    // The two halves of the contract in one place: a governed file gets its project's answer
    // verbatim, and a file no project includes gets `Unknown` -- never `Disabled`, which a
    // caller would read as "a real Vitest test file that happens not to use ambient globals"
    // and analyze anyway.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { globals: true, include: ['src/**/*.test.ts'] } });",
    );
    let set = resolve_projects(directory.path());
    assert!(matches!(
        set.globals_for(&directory.path().join("src/a.test.ts")),
        GlobalsDetection::Enabled { .. }
    ));
    match set.globals_for(&directory.path().join("e2e/a.cy.ts")) {
        GlobalsDetection::Unknown { reason } => {
            assert!(
                reason.contains("not matched by any resolved project"),
                "{reason}"
            );
        }
        other => panic!("expected Unknown for an unmatched file, got {other:?}"),
    }
}

/// Verify governed globals for keeps not included separate from a known answer.
#[test]
fn governed_globals_for_keeps_not_included_separate_from_a_known_answer() {
    // `globals_for` collapses "no project governs this file" into the same `Unknown` as "the
    // answer could not be determined". `governed_globals_for` must not: a caller uses the
    // first to fall back to its repository-wide setting and the second to fail closed.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { globals: true, include: ['src/**/*.test.ts'] } });",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(
        set.governed_globals_for(&directory.path().join("src/a.test.ts")),
        GovernedGlobals::Enabled
    );
    assert_eq!(
        set.governed_globals_for(&directory.path().join("e2e/a.cy.ts")),
        GovernedGlobals::NotIncluded
    );

    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { globals: false, include: ['src/**/*.test.ts'] } });",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(
        set.governed_globals_for(&directory.path().join("src/a.test.ts")),
        GovernedGlobals::Disabled
    );
}

/// Verify an unreadable exclude still answers when every project agrees on globals.
#[test]
fn an_unreadable_exclude_still_answers_when_every_project_agrees_on_globals() {
    // The real shape of a public Vue framework, reduced: governance is genuinely ambiguous (one
    // project's `exclude` spreads `configDefaults.exclude`), but the ambiguity is not load-bearing
    // because every project that could plausibly govern the file says `globals: true`.
    // Without the shortcut this repository analyzes 121 tests instead of 3,479.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        &ambiguous_projects_config("true", "true"),
    );
    let set = resolve_projects(directory.path());
    let file = directory.path().join("a/x.test.ts");
    assert!(
        matches!(
            set.governing_project(&file),
            ProjectGovernance::Unknown { .. }
        ),
        "the fixture must actually be ambiguous, or this asserts nothing"
    );
    assert!(matches!(
        set.globals_for(&file),
        GlobalsDetection::Enabled { .. }
    ));
    assert_eq!(set.governed_globals_for(&file), GovernedGlobals::Enabled);
}

/// Verify the unanimity shortcut also carries a unanimous false.
#[test]
fn the_unanimity_shortcut_also_carries_a_unanimous_false() {
    // The shortcut is about agreement, not about `true`: two projects that both disable
    // globals answer `Disabled`, not `Unknown`.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        &ambiguous_projects_config("false", "false"),
    );
    let set = resolve_projects(directory.path());
    let file = directory.path().join("a/x.test.ts");
    assert!(matches!(
        set.globals_for(&file),
        GlobalsDetection::Disabled { .. }
    ));
    assert_eq!(set.governed_globals_for(&file), GovernedGlobals::Disabled);
}

/// Verify projects that disagree about globals leave an ambiguous file unknown.
#[test]
fn projects_that_disagree_about_globals_leave_an_ambiguous_file_unknown() {
    // This is the half of the shortcut that must NOT fire. When the candidates disagree,
    // which project governs the file decides the answer, so the ambiguity is load-bearing and
    // the fail-closed rule applies again.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        &ambiguous_projects_config("true", "false"),
    );
    let set = resolve_projects(directory.path());
    assert_both_globals_views_unknown(&set, &directory.path().join("a/x.test.ts"));
}

/// Verify one project with an unreadable globals blocks the unanimity shortcut.
#[test]
fn one_project_with_an_unreadable_globals_blocks_the_unanimity_shortcut() {
    // The sibling of the disagreement case: everyone that *can* be read says `true`, but one
    // project's own `globals` is a non-literal expression. "All the ones I could read agree"
    // is not unanimity, and treating it as such would invent ambient tests for whichever
    // project the unreadable one turns out to be.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        &ambiguous_projects_config("true", "process.env.CI"),
    );
    let set = resolve_projects(directory.path());
    assert_both_globals_views_unknown(&set, &directory.path().join("a/x.test.ts"));
}

// --- Matching a file against a project's glob channels --------------------------------------

/// Verify both unreadable discovery channels are reported in one reason.
#[test]
fn both_unreadable_discovery_channels_are_reported_in_one_reason() {
    // `include` and `benchmark.include` are independent channels and a file can be uncertain
    // in both at once. The combined diagnostic has to name both, or whoever reads the warning
    // fixes one unreadable value and is surprised the file is still ungoverned.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { include: someGlobs, benchmark: { include: someGlobs } } });",
    );
    let set = resolve_projects(directory.path());
    match set.governing_project(&directory.path().join("src/a.test.ts")) {
        ProjectGovernance::Unknown { reason } => {
            assert!(reason.contains("`include` is set to"), "{reason}");
            assert!(reason.contains("`benchmark.include` is set to"), "{reason}");
        }
        other => panic!("expected Unknown from two unreadable channels, got {other:?}"),
    }
}

/// Verify an include pattern that cannot compile leaves the file unknown not excluded.
#[test]
fn an_include_pattern_that_cannot_compile_leaves_the_file_unknown_not_excluded() {
    // A glob `globset` refuses outright must not read as "this project does not claim the
    // file": that would silently narrow discovery on a typo. It has to surface as uncertainty.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { include: ['src/[.test.ts'] } });",
    );
    let set = resolve_projects(directory.path());
    match set.governing_project(&directory.path().join("src/a.test.ts")) {
        ProjectGovernance::Unknown { reason } => {
            assert!(
                reason.contains("`include` patterns failed to compile"),
                "{reason}"
            );
        }
        other => panic!("expected Unknown for an uncompilable include, got {other:?}"),
    }
}

/// Verify an exclude pattern that cannot compile leaves the file unknown not included.
#[test]
fn an_exclude_pattern_that_cannot_compile_leaves_the_file_unknown_not_included() {
    // The sibling site of the `include` case above: a fix applied to one channel and not the
    // other is this module's most repeated shape of defect. Here the file *is* included, so
    // an uncompilable `exclude` must not silently resolve to "not excluded" either.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { include: ['src/**/*.test.ts'], exclude: ['dist/['] } });",
    );
    let set = resolve_projects(directory.path());
    match set.governing_project(&directory.path().join("src/a.test.ts")) {
        ProjectGovernance::Unknown { reason } => {
            assert!(
                reason.contains("`exclude` patterns failed to compile"),
                "{reason}"
            );
        }
        other => panic!("expected Unknown for an uncompilable exclude, got {other:?}"),
    }
}

// --- Root configuration files this scan has to refuse ---------------------------------------

/// Verify a root config this scan cannot read resolves no projects but stays config found.
#[test]
fn a_root_config_this_scan_cannot_read_resolves_no_projects_but_stays_config_found() {
    // Every refusal shape at the root shares one contract: no projects, `config_found` true
    // (a config *does* exist -- this is not the plain-library case), `has_unresolved_entries`
    // true so no file resolves to a confident `NotIncluded`, and a diagnostic naming the
    // reason. Swept across all five shapes rather than one, because fixing the arm that was
    // reported and not its four siblings is how these regress.
    for (name, source, needle) in [
        (
            "unterminated string",
            "export default defineConfig({ test: { globals: true, name: 'oops\n' } });",
            "unterminated",
        ),
        (
            "no export default",
            "const config = { test: { globals: true } };\nmodule.exports = config;\n",
            "no `export default`",
        ),
        (
            "function-valued default export",
            "export default () => ({ test: { globals: true } });",
            "not an object literal",
        ),
        (
            "unclosed object literal",
            "export default { test: { globals: true };",
            "never closes",
        ),
        (
            "non-literal test value",
            "export default defineConfig({ test: sharedTest });",
            "sets `test` to something this scan cannot read",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "vitest.config.ts", source);
        let set = resolve_projects(directory.path());
        assert_root_config_refused(&set, name, needle);
        assert!(
            matches!(
                set.governing_project(&directory.path().join("src/a.test.ts")),
                ProjectGovernance::Unknown { .. }
            ),
            "{name}: an unreadable config must not leave files confidently NotIncluded"
        );
    }
}

/// Verify a root config with no test key still resolves one project on vitest defaults.
#[test]
fn a_root_config_with_no_test_key_still_resolves_one_project_on_vitest_defaults() {
    // A Vite config that never mentions `test` is a perfectly ordinary Vitest project: Vitest
    // applies its own defaults. Resolving nothing here would make every test file in such a
    // repository ungoverned.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ plugins: [vue()] });",
    );
    let set = resolve_projects(directory.path());
    assert!(!set.has_unresolved_entries);
    let project = single_project(&set);
    assert_eq!(project.name, None);
    assert_inherited_defaults(project);
    assert!(matches!(
        set.governing_project(&directory.path().join("src/a.test.ts")),
        ProjectGovernance::Governed(_)
    ));
}

/// The `GlobPatterns::Known` a `&[&str]` default constant should resolve to.
fn owned(patterns: &[&str]) -> GlobPatterns {
    GlobPatterns::Known(patterns.iter().map(|p| (*p).to_owned()).collect())
}

/// Verify a projects array that never closes resolves nothing.
#[test]
fn a_projects_array_that_never_closes_resolves_nothing() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: [ } });",
    );
    let set = resolve_projects(directory.path());
    assert_unresolved_with_diagnostic(&set, "never reaches a matching `]`");
}

/// Verify an include array that never closes is unknown rather than empty.
#[test]
fn an_include_array_that_never_closes_is_unknown_rather_than_empty() {
    // An unclosed `[` must not read as "no patterns": an empty `include` list excludes every
    // file in the project, which looks exactly like a project that genuinely has no tests.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { include: [ } });",
    );
    let set = resolve_projects(directory.path());
    let reason = unknown_include_reason(&set);
    assert!(reason.contains("never reaches a matching `]`"), "{reason}");
}

/// Verify a comment between a key and its value does not hide the value.
#[test]
fn a_comment_between_a_key_and_its_value_does_not_hide_the_value() {
    // The value skip runs over the *original* source, so it has to step over comments itself.
    // If it stopped at the `/`, both arrays would read as "set to something other than an
    // array literal" and a correctly-configured project would resolve to `Unknown`.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: {\n  include: /* keep in sync with tsconfig */ \
         ['src/**/*.test.ts'],\n  exclude: // legacy layout\n    ['dist/**'],\n} });",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(
        set.projects[0].include,
        GlobPatterns::Known(vec!["src/**/*.test.ts".to_owned()])
    );
    assert_eq!(
        set.projects[0].exclude,
        GlobPatterns::Known(vec!["dist/**".to_owned()])
    );
}

/// Verify a name key with no value leaves the project unnamed without disturbing globals.
#[test]
fn a_name_key_with_no_value_leaves_the_project_unnamed_without_disturbing_globals() {
    // A truncated `name:` yields an empty value extent. Reading that as a string would either
    // panic on the empty range or invent a name; it must simply be absent, and must not stop
    // the sibling `globals` key from being read.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { name: , globals: true } });",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 1);
    assert_eq!(set.projects[0].name, None);
    assert!(matches!(
        set.projects[0].globals,
        GlobalsDetection::Enabled { .. }
    ));
}

// --- `test.projects` entries: inheritance, references, and refusals -------------------------

/// Verify an unresolvable extends value leaves every unset field unknown.
#[test]
fn an_unresolvable_extends_value_leaves_every_unset_field_unknown() {
    // `extends` that is neither a boolean nor a string literal is the dangerous case: the
    // entry might be inheriting the root's `globals: true`, or might be replacing it. Falling
    // back to the root (or to Vitest's defaults) would be a guess in a direction that silently
    // changes which files get analyzed, so every field the entry does not set itself is
    // `Unknown` -- note the root below says `globals: true` and the entry must NOT report that.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { globals: true, projects: [\n  \
         { extends: makeBase({ shared: true }), test: { name: 'x' } },\n] } });",
    );
    let set = resolve_projects(directory.path());
    let project = single_project(&set);
    assert_eq!(project.name.as_deref(), Some("x"));
    match &project.globals {
        GlobalsDetection::Unknown { reason } => assert!(
            reason.contains("something other than a boolean or a string literal"),
            "{reason}"
        ),
        other => panic!("expected Unknown globals, got {other:?}"),
    }
    for (field, patterns) in [
        ("include", &project.include),
        ("exclude", &project.exclude),
        ("includeSource", &project.include_source),
        ("benchmark.include", &project.benchmark_include),
        ("benchmark.exclude", &project.benchmark_exclude),
        ("benchmark.includeSource", &project.benchmark_include_source),
    ] {
        assert!(
            matches!(patterns, GlobPatterns::Unknown { .. }),
            "{field} should be Unknown, got {patterns:?}"
        );
    }
}

/// Verify extends by string path to another file inherits that files fields.
#[test]
fn extends_by_string_path_to_another_file_inherits_that_files_fields() {
    // A real observed shape, except pointing at a sibling config rather than at itself: the
    // entry sets only its own `name`, and everything else has to come from the file it names.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "base.vitest.config.ts",
        "export default defineConfig({ test: { globals: true, include: ['pkg/**/*.test.ts'] } });",
    );
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: [\n  \
         { extends: './base.vitest.config.ts', test: { name: 'x' } },\n] } });",
    );
    let set = resolve_projects(directory.path());
    let project = assert_single_named_enabled(&set, "x");
    assert_eq!(
        project.include,
        GlobPatterns::Known(vec!["pkg/**/*.test.ts".to_owned()])
    );
}

/// Verify extends by string path to a missing file is unresolvable not ignored.
#[test]
fn extends_by_string_path_to_a_missing_file_is_unresolvable_not_ignored() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { globals: true, projects: [\n  \
         { extends: './missing.vitest.config.ts', test: { name: 'x' } },\n] } });",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 1);
    match &set.projects[0].globals {
        GlobalsDetection::Unknown { reason } => assert!(
            reason.contains("could not load and parse as a base configuration"),
            "{reason}"
        ),
        other => panic!("expected Unknown globals for a missing extends target, got {other:?}"),
    }
}

/// Verify an inline entry with no test key inherits the whole root configuration.
#[test]
fn an_inline_entry_with_no_test_key_inherits_the_whole_root_configuration() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { globals: true, include: ['src/**/*.test.ts'], \
         projects: [{ extends: true }] } });",
    );
    let set = resolve_projects(directory.path());
    let project = single_project(&set);
    assert_eq!(project.name, None);
    assert!(matches!(project.globals, GlobalsDetection::Enabled { .. }));
    assert_eq!(
        project.include,
        GlobPatterns::Known(vec!["src/**/*.test.ts".to_owned()])
    );
}

/// Verify an inline entry whose test value is unreadable resolves to an all unknown
/// project.
#[test]
fn an_inline_entry_whose_test_value_is_unreadable_resolves_to_an_all_unknown_project() {
    // The entry still exists -- it is a project, it just cannot be described -- so it is kept
    // as a `Project` with every field `Unknown` rather than dropped. Dropping it would let a
    // file it scopes resolve to `NotIncluded`.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { globals: true, projects: [{ test: sharedTest }] } });",
    );
    let set = resolve_projects(directory.path());
    let project = single_project(&set);
    assert_eq!(project.name, None);
    match &project.globals {
        GlobalsDetection::Unknown { reason } => assert!(
            reason.contains("sets `test` to something this scan cannot read"),
            "{reason}"
        ),
        other => panic!("expected Unknown globals, got {other:?}"),
    }
    assert!(matches!(project.include, GlobPatterns::Unknown { .. }));
    assert!(matches!(
        set.governing_project(&directory.path().join("src/a.test.ts")),
        ProjectGovernance::Unknown { .. }
    ));
}

/// Verify a bare identifier projects entry is reported rather than skipped.
#[test]
fn a_bare_identifier_projects_entry_is_reported_rather_than_skipped() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: [baseProject] } });",
    );
    let set = resolve_projects(directory.path());
    assert_unresolved_with_diagnostic(&set, "neither a string literal nor an inline object");
}

/// Verify a negated projects glob is recorded as unresolved rather than matched literally.
#[test]
fn a_negated_projects_glob_is_recorded_as_unresolved_rather_than_matched_literally() {
    // `'!packages/private'` needs cross-entry set-difference semantics this scan does not
    // implement. Treating it as an ordinary path would make it match nothing and look
    // harmless; it has to set `has_unresolved_entries` so the first entry's resolved project
    // cannot be trusted as the last word on a file either.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "packages/a/vitest.config.ts",
        "export default defineConfig({ test: { globals: true } });",
    );
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: ['packages/*', '!packages/private'] } });",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 1);
    assert!(set.has_unresolved_entries);
    assert!(
        set.diagnostics
            .iter()
            .any(|note| note.contains("glob negation")),
        "{:?}",
        set.diagnostics
    );
}

// --- Resolving a `test.projects` string entry onto the filesystem ---------------------------

/// Verify a directory entry resolves through that directorys own config file.
#[test]
fn a_directory_entry_resolves_through_that_directorys_own_config_file() {
    // `'packages/a'` names a directory, not a file: the scan has to find that directory's own
    // `vitest.config.*`, resolve globs relative to it, and -- because that file has no `test`
    // key -- fall back to Vitest's documented defaults rather than to `Unknown`.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "packages/a/vitest.config.ts",
        "export default defineConfig({ plugins: [] });",
    );
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: ['packages/a'] } });",
    );
    let set = resolve_projects(directory.path());
    let project = single_project(&set);
    assert_eq!(project.base_dir, directory.path().join("packages/a"));
    assert_eq!(
        project.config_path,
        Some(directory.path().join("packages/a/vitest.config.ts"))
    );
    assert!(matches!(project.globals, GlobalsDetection::Disabled { .. }));
    assert_eq!(project.include, owned(DEFAULT_INCLUDE_GLOBS));
    assert_eq!(project.exclude, owned(DEFAULT_EXCLUDE_GLOBS));
    assert_eq!(project.include_source, GlobPatterns::Known(Vec::new()));
    assert_eq!(
        project.benchmark_include,
        owned(DEFAULT_BENCHMARK_INCLUDE_GLOBS)
    );
    assert_eq!(
        project.benchmark_exclude,
        owned(DEFAULT_BENCHMARK_EXCLUDE_GLOBS)
    );
    assert_eq!(
        project.benchmark_include_source,
        GlobPatterns::Known(Vec::new())
    );
    assert!(matches!(
        set.governing_project(&directory.path().join("packages/a/x.test.ts")),
        ProjectGovernance::Governed(_)
    ));
}

/// Verify a double star glob entry resolves the config files it matches.
#[test]
fn a_double_star_glob_entry_resolves_the_config_files_it_matches() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "packages/a/nested/vitest.config.ts",
        "export default defineConfig({ test: { globals: true, include: ['**/*.test.ts'] } });",
    );
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: ['packages/**/vitest.config.ts'] } });",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 1, "{:?}", set.diagnostics);
    assert_eq!(
        set.projects[0].config_path,
        Some(directory.path().join("packages/a/nested/vitest.config.ts"))
    );
    assert!(matches!(
        set.projects[0].globals,
        GlobalsDetection::Enabled { .. }
    ));
}

/// Verify a glob entry whose leading directory does not exist matches nothing.
#[test]
fn a_glob_entry_whose_leading_directory_does_not_exist_matches_nothing() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: ['missing/*'] } });",
    );
    let set = resolve_projects(directory.path());
    assert_unresolved_with_diagnostic(&set, "matched no project configuration file");
}

/// Verify a symlink matched by a glob entry does not become a second project.
#[cfg(unix)]
#[test]
fn a_symlink_matched_by_a_glob_entry_does_not_become_a_second_project() {
    // Links are not followed, so a symlinked package directory is neither a file nor a
    // directory as far as the walk is concerned. Counting it would resolve the same config
    // file twice and give one project two `declared_at` positions.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "packages/real/vitest.config.ts",
        "export default defineConfig({ test: { globals: true } });",
    );
    std::os::unix::fs::symlink(
        directory.path().join("packages/real"),
        directory.path().join("packages/link"),
    )
    .unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: ['packages/*'] } });",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 1, "{:?}", set.projects);
    assert_eq!(
        set.projects[0].config_path,
        Some(directory.path().join("packages/real/vitest.config.ts"))
    );
}

/// Verify a referenced project file this scan cannot read becomes an all unknown project.
#[test]
fn a_referenced_project_file_this_scan_cannot_read_becomes_an_all_unknown_project() {
    // Two different ways a referenced config can be unreadable, swept together: no recognized
    // `export default` at all, and a `test` value that is not an object literal. Both have to
    // produce a project rather than nothing, so the files it would have scoped stay uncertain.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "packages/b/vitest.config.ts",
        "module.exports = { test: { globals: true } };",
    );
    write(
        directory.path(),
        "packages/c/vitest.config.ts",
        "export default defineConfig({ test: sharedTest });",
    );
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: \
         ['packages/b/vitest.config.ts', 'packages/c/vitest.config.ts'] } });",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 2, "{:?}", set.diagnostics);
    for project in &set.projects {
        match &project.globals {
            GlobalsDetection::Unknown { reason } => assert!(
                reason.contains("could not be read as a standalone project configuration"),
                "{reason}"
            ),
            other => panic!("expected Unknown globals, got {other:?}"),
        }
        assert!(matches!(project.include, GlobPatterns::Unknown { .. }));
        assert!(matches!(
            project.benchmark_include,
            GlobPatterns::Unknown { .. }
        ));
    }
}

/// Verify one glob entry expanding past the project cap stops and says so.
#[test]
fn one_glob_entry_expanding_past_the_project_cap_stops_and_says_so() {
    // The cap has to bite inside a single entry's expansion too, not only across entries --
    // and it has to SAY so. `truncated` used to be written only by the per-entry loop, so a
    // config whose single glob entry overflowed reported `has_unresolved_entries` without
    // `truncated`, contradicting that field's own doc and costing `main`'s warning the
    // "(truncated at the project limit)" clause -- the only part of it a reader can act on.
    let directory = tempfile::tempdir().unwrap();
    write_glob_monorepo(directory.path(), MAX_PROJECTS + 5);
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), MAX_PROJECTS);
    assert!(
        set.has_unresolved_entries,
        "stopping at the cap must not look like a complete answer"
    );
    assert!(
        set.truncated,
        "and the reason must be the project limit, not merely `incomplete`"
    );
}

/// The control: an entry that fits under the cap truncates nothing.
///
/// Without it, the assertion above would hold just as well if `truncated` were set on every
/// glob entry regardless of the cap.
#[test]
fn a_glob_entry_that_fits_under_the_project_cap_is_not_truncated() {
    let directory = tempfile::tempdir().unwrap();
    write_glob_monorepo(directory.path(), 4);
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 4);
    assert!(!set.truncated, "nothing was cut short");
    assert!(!set.has_unresolved_entries, "and nothing went unresolved");
}

// --- Legacy `vitest.workspace.*` files ------------------------------------------------------

/// Verify a workspace file this scan cannot read resolves no projects.
#[test]
fn a_workspace_file_this_scan_cannot_read_resolves_no_projects() {
    for (name, source, needle) in [
        (
            "unterminated string",
            "export default defineWorkspace([\n  'packages/*,\n]);",
            "unterminated",
        ),
        (
            "no export default",
            "module.exports = [{ test: { globals: true } }];",
            "no `export default`",
        ),
        (
            "default export is not an array",
            "export default { projects: ['packages/*'] };",
            "not an array literal",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "vitest.workspace.ts", source);
        let set = resolve_projects(directory.path());
        assert_root_config_refused(&set, name, needle);
    }
}

/// Verify a workspace entry whose object literal never closes is reported.
#[test]
fn a_workspace_entry_whose_object_literal_never_closes_is_reported() {
    // Reachable only from a workspace file: the array's own `[ ... ]` matching is independent
    // of brace nesting, so an entry can open a `{` that nothing ever closes while the array
    // around it still parses.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.workspace.ts",
        "export default [ { name: 'a' ];",
    );
    let set = resolve_projects(directory.path());
    assert_unresolved_with_diagnostic(&set, "starts an object literal that never closes");
    assert!(matches!(
        set.governing_project(&directory.path().join("src/a.test.ts")),
        ProjectGovernance::Unknown { .. }
    ));
}

/// Verify a workspace file declaring more projects than the cap is truncated.
#[test]
fn a_workspace_file_declaring_more_projects_than_the_cap_is_truncated() {
    let directory = tempfile::tempdir().unwrap();
    let mut entries = String::new();
    for index in 0..(MAX_PROJECTS + 5) {
        entries.push_str(&format!("{{ test: {{ name: 'p{index}' }} }},\n"));
    }
    write(
        directory.path(),
        "vitest.workspace.ts",
        &format!("export default defineWorkspace([{entries}]);"),
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), MAX_PROJECTS);
    assert!(set.truncated);
    assert!(set.has_unresolved_entries);
    assert!(
        set.diagnostics
            .iter()
            .any(|note| note.contains("stopped resolving")),
        "{:?}",
        set.diagnostics
    );
}

// --- String-literal reading -----------------------------------------------------------------

/// Verify read string literal decodes the simple escapes javascript defines.
#[test]
fn read_string_literal_decodes_the_simple_escapes_javascript_defines() {
    for (source, expected) in [
        (r"'a\nb'", "a\nb"),
        (r"'a\tb'", "a\tb"),
        (r"'a\rb'", "a\rb"),
        (r"'a\\b'", "a\\b"),
        (r"'it\'s'", "it's"),
        (r#""say \"hi\"""#, "say \"hi\""),
    ] {
        let chars: Vec<char> = source.chars().collect();
        let len = chars.len();
        assert_eq!(
            read_string_literal(&chars, 0..len).as_deref(),
            Some(expected),
            "{source}"
        );
    }
}

/// Verify read string literal refuses everything that is not one plain literal.
#[test]
fn read_string_literal_refuses_everything_that_is_not_one_plain_literal() {
    // Each of these would, if accepted, hand a *different* glob to the matcher than the config
    // actually names -- the wrong-but-plausible failure this function exists to prevent.
    for (source, why) in [
        ("   ", "nothing but trivia"),
        (r"'abc", "unterminated"),
        (r"'a' + 'b'", "concatenation, not one literal"),
        ("`abc`", "template literal"),
        (r"'\x'", "truncated \\x escape"),
        (r"'\xZZ'", "non-hex digits in an \\x escape"),
        (r"'\u00'", "truncated \\u escape"),
        (r"'\u{}'", "\\u{} with no digits"),
        (r"'\u{110000}'", "codepoint past U+10FFFF"),
        (r"'\u{D800}'", "lone surrogate half"),
    ] {
        let chars: Vec<char> = source.chars().collect();
        let len = chars.len();
        assert_eq!(read_string_literal(&chars, 0..len), None, "{why}: {source}");
    }
}

// --- Extglob translation, continued ---------------------------------------------------------

/// Verify expand extglob rejects group shapes it cannot translate soundly.
#[test]
fn expand_extglob_rejects_group_shapes_it_cannot_translate_soundly() {
    // Four different ways a `?(...)` group can be untranslatable. Each has to be an `Err`
    // naming the pattern, never a partial expansion: a half-translated glob still compiles
    // and then matches the wrong set of files, which is indistinguishable from a project that
    // genuinely has no tests there.
    for (pattern, needle) in [
        ("packages/?(a|b/**", "never reaches a"),
        ("packages/?(a|?(b))/**", "nested extglob group"),
        ("packages/?(a|)/**", "empty alternative"),
        (
            "?(a|b)?(c|d)?(e|f)?(g|h)?(i|j)",
            "concrete alternatives, more than",
        ),
    ] {
        let error = expand_extglob_optional_groups(pattern)
            .expect_err(&format!("{pattern} should be rejected, not guessed at"));
        assert!(
            error.contains(pattern),
            "diagnostic should name the offending pattern: {error}"
        );
        assert!(
            error.contains(needle),
            "diagnostic should say why (`{needle}`): {error}"
        );
    }
}

/// Verify expand extglob leaves an introducer that starts no group untouched.
#[test]
fn expand_extglob_leaves_an_introducer_that_starts_no_group_untouched() {
    // `?` here is an ordinary single-character wildcard and the parentheses are literal: there
    // is no `?(`/`+(`/`@(` sequence anywhere, so the pattern must come back byte-for-byte
    // rather than being rewritten or refused.
    assert_eq!(
        expand_extglob_optional_groups("src/(fixtures)/a?.test.ts").unwrap(),
        vec!["src/(fixtures)/a?.test.ts".to_owned()]
    );
}

// --- Arms no configuration file can reach, pinned by direct calls ---------------------------

/// Verify an unclosed benchmark object makes every benchmark channel unknown.
#[test]
fn an_unclosed_benchmark_object_makes_every_benchmark_channel_unknown() {
    // Called directly: `benchmark: {` is only ever looked at inside a `test: { ... }` whose
    // own closing brace has already been matched, and that same brace closes the inner one
    // too -- so no config file can produce this. The arm still has to fail closed on all
    // three channels rather than leave any of them looking like a Vitest default.
    let masked: Vec<char> = "benchmark: {".chars().collect();
    let original = masked.clone();
    let len = masked.len();
    let (include, exclude, include_source) = read_benchmark_fields(&masked, &original, 0..len);
    for channel in [include, exclude, include_source] {
        match channel {
            Some(GlobPatterns::Unknown { reason }) => {
                assert!(reason.contains("never reaches a matching `}`"), "{reason}");
            }
            other => panic!("expected Unknown for an unclosed `benchmark`, got {other:?}"),
        }
    }
}

/// Verify inheriting with no base configuration resolves every field to unknown.
#[test]
fn inheriting_with_no_base_configuration_resolves_every_field_to_unknown() {
    // Also unreachable from `resolve_projects`: the only call site that passes
    // `ExtendsMode::Inherit` always has a base to hand it. The arm is this module's
    // fail-closed floor, and what it must not do is quietly substitute Vitest's defaults --
    // an `include` silently defaulted here would scope a project to files its real config
    // never claimed.
    let config = Path::new("/repo/vitest.config.ts");
    let effective = resolve_effective(&RawTestFields::empty(), &ExtendsMode::Inherit, None, config);
    match &effective.globals {
        GlobalsDetection::Unknown { reason } => {
            assert!(reason.contains("no base configuration"), "{reason}");
        }
        other => panic!("expected Unknown globals, got {other:?}"),
    }
    for (field, patterns) in [
        ("include", &effective.include),
        ("exclude", &effective.exclude),
        ("includeSource", &effective.include_source),
        ("benchmark.include", &effective.benchmark_include),
        ("benchmark.exclude", &effective.benchmark_exclude),
        (
            "benchmark.includeSource",
            &effective.benchmark_include_source,
        ),
    ] {
        match patterns {
            GlobPatterns::Unknown { reason } => assert!(
                reason.contains(&format!("`test.{field}`")),
                "{field}: {reason}"
            ),
            other => panic!("expected Unknown {field}, got {other:?}"),
        }
    }
}

/// Verify a projects glob globset refuses to compile matches nothing and is reported.
#[test]
fn a_projects_glob_globset_refuses_to_compile_matches_nothing_and_is_reported() {
    // `looks_like_glob` accepts the `[`, `globset` then refuses to compile it. Expanding to
    // nothing is the only sound answer -- but it has to be a *reported* nothing, or a typo in
    // a `test.projects` entry silently removes a whole package from discovery while the run
    // still looks clean.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: ['packages/['] } });",
    );
    let set = resolve_projects(directory.path());
    assert_unresolved_with_diagnostic(&set, "matched no project configuration file");
}

// ---------------------------------------------------------------------------------------------
// Rows specified by the 2026-09-27 tests-implementation plan (S1/S2/S3 blocks)
// ---------------------------------------------------------------------------------------------

/// R14 (S1): the one-character repro. A leading `./` in an include glob must match the same
/// files as the same glob without it -- before the fix it matched nothing, costing one
/// scanned repository all 30 of its test files. Pins both directions: `./pkg/**` reaches the
/// file, and the pattern still does not reach files outside it.
#[test]
fn a_leading_dot_slash_in_an_include_glob_matches_the_same_files_as_without() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "pkg/api.spec.ts",
        "import { it } from 'vitest'; it('works', () => {});\n",
    );
    write(
        dir.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { include: ['./pkg/**/*.spec.ts'] } });",
    );
    let set = resolve_projects(dir.path());
    let project = &set.projects[0];
    assert_file_match(
        project,
        &dir.path().join("pkg/api.spec.ts"),
        FileMatch::Yes,
        "the ./-prefixed glob must reach the file the bare glob reaches",
    );
    assert!(
        !matches!(
            match_file(project, &dir.path().join("elsewhere.spec.ts")),
            FileMatch::Yes
        ),
        "and it must not widen beyond what the bare glob matches"
    );
}

/// R15: the boundary probe just past S1's edge. Stripping a LEADING `./` from a pattern must
/// not become normalizing `./` anywhere: `pkg/./x`, `.//pkg`, and `../pkg` must each keep
/// their distinct (here: empty) results rather than silently folding to `pkg`. Passes today
/// because nothing normalizes at all; guards an over-broad fix.
#[test]
fn a_dot_slash_in_a_non_initial_segment_is_not_stripped() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "configs/vitest.config.ts",
        "export default {};\n",
    );
    let root = dir.path();
    let mut budget = Budget::default();

    // The literal directory entry and the glob entry, bare: both must find the project
    // config, or the forms below prove nothing about normalization.
    let literal = expand_project_path(root, "configs", &mut budget).paths;
    assert_eq!(
        literal.len(),
        1,
        "the bare literal entry finds the project config"
    );
    let globbed = expand_project_path(root, "configs/*.ts", &mut budget).paths;
    assert_eq!(
        globbed.len(),
        1,
        "the bare glob entry finds the project config"
    );

    // A LEADING ./ is the one form that must normalize away (the S1 site this guards).
    for pattern in ["./configs", "./configs/*.ts"] {
        let expanded = expand_project_path(root, pattern, &mut budget).paths;
        assert_eq!(expanded.len(), 1, "`{pattern}` must match its bare form");
    }

    // A `.//` or `../` prefix must stay its own pattern: folding either to the bare form
    // would let an over-broad normalization silently widen what a projects entry claims --
    // `../` past the root especially. (`pkg/./x` is deliberately NOT in this list: a
    // mid-path `./` is filesystem-transparent here, which is path resolution, not a
    // normalization this guard has an opinion about.)
    for pattern in [".//configs/*.ts", "../configs/*.ts"] {
        let expanded = expand_project_path(root, pattern, &mut budget).paths;
        assert!(
            expanded.is_empty(),
            "`{pattern}` folds to the bare pattern and stops being its own pattern: {expanded:?}"
        );
    }
}

/// R17 (S2's flag half): a project config this scan cannot read must set
/// `project_config_degraded` in discovery's result, not only print a message to stderr. Ten
/// of the twenty-three `test.projects` repositories in the 2026-09-27 scan had the flag false
/// while stderr carried the degraded-config message; this pins the half that works and the
/// shape the fix must generalize.
#[test]
fn a_degraded_project_config_sets_project_config_degraded() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "pkg/api.spec.ts",
        "import { it } from 'vitest'; it('works', () => {});\n",
    );
    write(dir.path(), "pkg/vitest.config.ts", "export default 1;\n");
    write(
        dir.path(),
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: ['pkg'] } });",
    );
    let set = resolve_projects(dir.path());
    let config = discovery_config(dir.path());
    let discovered = crate::discovery::discover(&config, Some(&set)).unwrap();
    assert!(
        discovered.project_config_degraded,
        "an unreadable project config must degrade the result, not only log"
    );
}

/// A project entry's `test.root` scopes inherited default includes to that directory,
/// retaining its tests without claiming files outside it.
#[test]
fn projects_entry_root_is_prefixed_to_inherited_default_includes() {
    let dir = tempfile::tempdir().unwrap();
    // Canonicalized because that is the form `main.rs` resolves projects from (the config's
    // own root), and the form a declared `root:` is canonicalized to before comparison.
    let root = dir.path().canonicalize().unwrap();
    write(
        &root,
        "pkg/api.spec.ts",
        "import { it } from 'vitest'; it('works', () => {});\n",
    );
    write(
        &root,
        "legacy/old.spec.ts",
        "import { it } from 'vitest'; it('old', () => {});\n",
    );
    write(
        &root,
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: [{ test: { name: 'pkg', root: './pkg' } }] } });",
    );
    let set = resolve_projects(&root);
    let project = &set.projects[0];
    assert_file_match(
        project,
        &root.join("pkg/api.spec.ts"),
        FileMatch::Yes,
        "files under root: belong to the project",
    );
    assert!(
        !matches!(
            match_file(project, &root.join("legacy/old.spec.ts")),
            FileMatch::Yes
        ),
        "files outside root: must not be claimed by inherited default includes"
    );
}

/// A declared `root:`/`dir:` whose text carries `..` components but still resolves inside
/// the analysis root must match that directory's files. `Path` comparisons are lexical, so
/// storing `root/pkg/../pkg` unresolved means `strip_prefix` never matches the canonical
/// paths discovery reports -- the project would silently claim nothing while the run stayed
/// complete. Found by review on the duplication pass; the fix canonicalizes the joined
/// directory before storing it.
#[test]
fn projects_entry_root_with_parent_components_still_matches_its_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(
        &root,
        "pkg/api.spec.ts",
        "import { it } from 'vitest'; it('works', () => {});\n",
    );
    write(
        &root,
        "legacy/old.spec.ts",
        "import { it } from 'vitest'; it('old', () => {});\n",
    );
    write(
        &root,
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: [{ test: { name: 'pkg', root: './pkg/../pkg' } }] } });",
    );
    let set = resolve_projects(&root);
    let project = &set.projects[0];
    assert_file_match(
        project,
        &root.join("pkg/api.spec.ts"),
        FileMatch::Yes,
        "a parent-component root that resolves inside the analysis root still names its \
         directory",
    );
    assert!(
        !matches!(
            match_file(project, &root.join("legacy/old.spec.ts")),
            FileMatch::Yes
        ),
        "resolving the root must not widen it: files outside the resolved directory stay out"
    );
}

/// A project entry's `test.dir` scopes include patterns to that directory, so
/// `src/**/*.spec.ts` reaches `pkg/src/`.
#[test]
fn projects_entry_dir_scopes_relative_includes() {
    let dir = tempfile::tempdir().unwrap();
    // Canonicalized to match the form `main.rs` resolves projects from; see the `root:`
    // test above.
    let root = dir.path().canonicalize().unwrap();
    write(
        &root,
        "pkg/src/api.spec.ts",
        "import { it } from 'vitest'; it('works', () => {});\n",
    );
    write(
        &root,
        "vitest.config.ts",
        "export default defineConfig({ test: { projects: [{ test: { name: 'pkg', dir: './pkg', include: ['src/**/*.spec.ts'] } }] } });",
    );
    let set = resolve_projects(&root);
    let project = &set.projects[0];
    assert_file_match(
        project,
        &root.join("pkg/src/api.spec.ts"),
        FileMatch::Yes,
        "a dir:-relative include must reach files under dir:",
    );
}

/// The **precondition** S2's second half rests on, pinned separately so it cannot rot.
///
/// The fix itself lives at the CLI boundary and is asserted end to end by
/// `main.rs`'s `an_unresolved_projects_entry_degrades_the_json_run_even_when_every_file_attributes`:
/// `projectConfigDegraded` now carries `has_unresolved_entries`/`truncated`, the exact
/// condition the stderr banner names. Nothing in *this* test fails when that fix is
/// reverted, and it does not pretend otherwise -- what it pins is the half of the setup that
/// makes the other test meaningful: with an unreadable entry present but every discovered
/// file attributing to a readable project, discovery's own fell-through-to-the-defaults flag
/// stays `false`. Should discovery ever start raising it here, the CLI test would pass for
/// the wrong reason and this one goes red first, naming why.
///
/// The shape is real: 13 repositories in the 2026-09-27 scan printed the "project map is
/// incomplete" warning with `projectConfigDegraded: false`, so a JSON/CI consumer that never
/// reads stderr saw a clean run.
#[test]
fn an_unreadable_projects_entry_alone_does_not_raise_discoverys_degraded_flag() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "pkg/api.spec.ts",
        "import { it } from 'vitest'; it('works', () => {});\n",
    );
    write(
        dir.path(),
        "vitest.config.ts",
        "import { makeProjects } from './projects';\nexport default defineConfig({ test: { projects: [makeProjects('other'), { test: { name: 'pkg', include: ['pkg/**/*.spec.ts'] } }] } });",
    );
    let set = resolve_projects(dir.path());
    assert!(
        set.has_unresolved_entries,
        "precondition: the call-built entry leaves the map incomplete"
    );
    // The one discovered file attributes to the readable `pkg` project, so discovery's own
    // IncludeDegraded path never fires -- this is exactly the shape that used to pass clean.
    let config = discovery_config(dir.path());
    let discovered = crate::discovery::discover(&config, Some(&set)).unwrap();
    assert!(
        !discovered.project_config_degraded,
        "no file fell through to the defaults, so the discovery-side flag alone stays \
         silent -- which is exactly why the CLI must contribute the project-map condition"
    );
}

/// R20 (S3 sub-mechanism 3): a `test.projects` entry built by a call expression whose
/// argument this scan cannot read must mark the project set degraded, because the files that
/// entry governs cannot be attributed -- silently guessing is how a project's `globals` or
/// `include` gets missed. Measured working today via the CLI flag; pinned here at the unit
/// boundary.
#[test]
fn a_projects_entry_built_by_a_call_expression_marks_the_set_degraded() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "pkg/api.spec.ts",
        "import { it } from 'vitest'; it('works', () => {});\n",
    );
    write(
        dir.path(),
        "vitest.config.ts",
        "import { makeProjects } from './projects';\nexport default defineConfig({ test: { projects: [makeProjects('pkg')] } });",
    );
    let set = resolve_projects(dir.path());
    assert!(
        set.has_unresolved_entries,
        "an entry built by an unreadable call expression must degrade the project set"
    );
}

/// Recursive project patterns keep their single-star segment within one directory.
#[test]
fn project_path_stars_do_not_consume_directory_separators() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for path in [
        "packages/pkg-a/vitest.config.ts",
        "packages/pkg-a/deep/vitest.config.ts",
    ] {
        write(root, path, "export default { test: { globals: true } };");
    }
    let direct = expand_project_path(
        root,
        "packages/**/pkg-*/vitest.config.ts",
        &mut Budget::default(),
    )
    .paths;
    assert_eq!(direct, vec![root.join("packages/pkg-a/vitest.config.ts")]);
    let recursive =
        expand_project_path(root, "packages/**/vitest.config.ts", &mut Budget::default()).paths;
    assert_eq!(recursive.len(), 2);
}

/// Typecheck includes and excludes share the test and benchmark component contract.
#[test]
fn typecheck_globs_keep_single_wildcards_inside_one_component() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "vitest.config.ts",
        "export default { test: { include: [], benchmark: { include: [] }, typecheck: { enabled: true, include: ['src/*.test-d.ts'], exclude: [] } } };",
    );
    let set = resolve_projects(root);
    let project = single_project(&set);
    assert_file_match(
        project,
        &root.join("src/a.test-d.ts"),
        FileMatch::Yes,
        "direct type test retained",
    );
    assert_file_match(
        project,
        &root.join("src/deep/n.test-d.ts"),
        FileMatch::No,
        "single star does not recurse",
    );
    let mut project = project.clone();
    project.typecheck_include = owned(&["src/**/*.test-d.ts"]);
    project.typecheck_exclude = owned(&["src/*.test-d.ts"]);
    assert_file_match(
        &project,
        &root.join("src/a.test-d.ts"),
        FileMatch::No,
        "direct exclusion retained",
    );
    assert_file_match(
        &project,
        &root.join("src/deep/n.test-d.ts"),
        FileMatch::Yes,
        "direct exclusion does not erase nested positive",
    );
}

// ---------------------------------------------------------------------------------------------
// Confinement of configuration-derived paths to the analysis root.
// ---------------------------------------------------------------------------------------------

/// An analysis root with a sibling directory outside it, under one temporary directory.
struct Escape {
    _guard: tempfile::TempDir,
    /// The canonical root, as `Config::load` hands it to `resolve_projects`.
    root: PathBuf,
    /// The canonical sibling outside the root.
    outside: PathBuf,
    /// The root and sibling spelled as the temporary directory reports them (no verbatim prefix
    /// on Windows), for absolute paths written into a config.
    root_spelled: PathBuf,
    outside_spelled: PathBuf,
}

/// Builds the [`Escape`] layout; `outside` holds a project config reading `globals: true`, the
/// observable a leaked read would add to the result.
fn escape_fixture() -> Escape {
    let guard = tempfile::tempdir().unwrap();
    let root_spelled = guard.path().join("root");
    let outside_spelled = guard.path().join("outside");
    fs::create_dir_all(&root_spelled).unwrap();
    write(
        &outside_spelled,
        "vitest.config.ts",
        "export default { test: { globals: true, name: 'outside' } };",
    );
    Escape {
        root: root_spelled.canonicalize().unwrap(),
        outside: outside_spelled.canonicalize().unwrap(),
        root_spelled,
        outside_spelled,
        _guard: guard,
    }
}

/// `path` as a forward-slash string safe to embed in a JavaScript string literal.
fn js_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Asserts the entry was refused for leaving the root: nothing resolved, the set says it is
/// incomplete, and the diagnostic names the reason.
fn assert_refused_outside(set: &ProjectSet, why: &str) {
    assert!(set.projects.is_empty(), "{why}: {:?}", set.projects);
    assert!(set.has_unresolved_entries, "{why}: {set:?}");
    assert!(
        set.diagnostics
            .iter()
            .any(|note| note.contains("outside the analysis root")),
        "{why}: {:?}",
        set.diagnostics
    );
}

/// The reason a project's `globals` is `Unknown`, asserting that it is.
fn unknown_globals_reason(project: &Project) -> &str {
    match &project.globals {
        GlobalsDetection::Unknown { reason } => reason,
        other => panic!("expected Unknown globals, got {other:?}"),
    }
}

/// A `test.projects` string entry that leaves the root is refused whether it is written with `..`,
/// as an absolute path, as a glob, or as an explicit config file, and the outside config is not
/// read (its `globals: true` never reaches the result).
#[test]
fn projects_entry_outside_the_root_is_refused() {
    let fixture = escape_fixture();
    let absolute = js_path(&fixture.outside_spelled);
    let patterns = [
        "../outside".to_owned(),
        "./../outside".to_owned(),
        absolute.clone(),
        "../out*".to_owned(),
        "../outside/vitest.config.ts".to_owned(),
        format!("{absolute}/vitest.config.ts"),
        "../missing".to_owned(),
    ];
    for pattern in patterns {
        write(
            &fixture.root,
            "vitest.config.ts",
            &format!("export default {{ test: {{ projects: ['{pattern}'] }} }};"),
        );
        assert_refused_outside(&resolve_projects(&fixture.root), &pattern);
    }
}

/// Retention control for the refusal: entries that stay inside the root, including a glob and a
/// literal directory, still resolve, and an escaping entry beside them leaves them intact.
#[test]
fn projects_entry_inside_the_root_still_resolves() {
    let fixture = escape_fixture();
    write(
        &fixture.root,
        "pkg/vitest.config.ts",
        "export default { test: { globals: true } };",
    );
    write(
        &fixture.root,
        "more/one/vitest.config.ts",
        "export default { test: { globals: true } };",
    );
    for pattern in ["pkg", "./pkg", "more/*", "pkg/vitest.config.ts"] {
        write(
            &fixture.root,
            "vitest.config.ts",
            &format!("export default {{ test: {{ projects: ['{pattern}'] }} }};"),
        );
        let set = resolve_projects(&fixture.root);
        assert!(
            !set.has_unresolved_entries,
            "{pattern}: {:?}",
            set.diagnostics
        );
        assert_eq!(set.projects.len(), 1, "{pattern}");
        assert!(matches!(
            set.projects[0].globals,
            GlobalsDetection::Enabled { .. }
        ));
    }
    write(
        &fixture.root,
        "vitest.config.ts",
        "export default { test: { projects: ['pkg', '../outside'] } };",
    );
    let set = resolve_projects(&fixture.root);
    assert_eq!(set.projects.len(), 1, "the inside entry survives");
    assert!(set.has_unresolved_entries, "the outside entry is flagged");
    assert!(set
        .diagnostics
        .iter()
        .any(|note| note.contains("`../outside`") && note.contains("outside the analysis root")));
}

/// A symlinked project directory or config file that leaves the root is refused; one that stays
/// inside resolves. Skipped (visibly) where symlinks cannot be created.
#[test]
fn projects_entry_through_a_symlink_is_judged_by_its_target() {
    let fixture = escape_fixture();
    write(
        &fixture.root,
        "real/vitest.config.ts",
        "export default { test: { globals: true } };",
    );
    fs::create_dir_all(fixture.root.join("holder")).unwrap();
    if !crate::confine::try_symlink(&fixture.outside, &fixture.root.join("escape")) {
        return;
    }
    assert!(crate::confine::try_symlink(
        &fixture.outside.join("vitest.config.ts"),
        &fixture.root.join("holder/vitest.config.ts")
    ));
    assert!(crate::confine::try_symlink(
        &fixture.root.join("real"),
        &fixture.root.join("alias")
    ));
    for pattern in ["escape", "holder", "hold*"] {
        write(
            &fixture.root,
            "vitest.config.ts",
            &format!("export default {{ test: {{ projects: ['{pattern}'] }} }};"),
        );
        assert_refused_outside(&resolve_projects(&fixture.root), pattern);
    }
    write(
        &fixture.root,
        "vitest.config.ts",
        "export default { test: { projects: ['alias'] } };",
    );
    let set = resolve_projects(&fixture.root);
    assert!(!set.has_unresolved_entries, "{:?}", set.diagnostics);
    assert_eq!(set.projects.len(), 1);
}

/// A root that is itself reached through a symlink resolves an entry spelled either way.
#[test]
fn projects_entry_resolves_when_the_root_is_reached_through_a_symlink() {
    let guard = tempfile::tempdir().unwrap();
    let real = guard.path().join("real-root");
    let alias = guard.path().join("alias-root");
    write(
        &real,
        "pkg/vitest.config.ts",
        "export default { test: { globals: true } };",
    );
    if !crate::confine::try_symlink(&real, &alias) {
        return;
    }
    for (root, entry) in [
        (&alias, "pkg".to_owned()),
        (&real, "pkg".to_owned()),
        (&alias, js_path(&real.join("pkg"))),
        (&real, js_path(&alias.join("pkg"))),
    ] {
        write(
            root,
            "vitest.config.ts",
            &format!("export default {{ test: {{ projects: ['{entry}'] }} }};"),
        );
        let set = resolve_projects(root);
        assert!(
            !set.has_unresolved_entries,
            "{entry}: {:?}",
            set.diagnostics
        );
        assert_eq!(set.projects.len(), 1, "{entry}");
    }
}

/// The `glob.sync('<literal>')` project list is confined like a string entry.
#[test]
fn glob_sync_project_list_outside_the_root_is_refused() {
    let fixture = escape_fixture();
    write(
        &fixture.root,
        "vitest.config.ts",
        "import { glob } from 'glob'\nexport default { test: { projects: glob.sync('../outside') } };",
    );
    assert_refused_outside(&resolve_projects(&fixture.root), "glob.sync");
    write(
        &fixture.root,
        "pkg/vitest.config.ts",
        "export default { test: { globals: true } };",
    );
    write(
        &fixture.root,
        "vitest.config.ts",
        "import { glob } from 'glob'\nexport default { test: { projects: glob.sync('./*/vitest.config.ts') } };",
    );
    let set = resolve_projects(&fixture.root);
    assert!(!set.has_unresolved_entries, "{:?}", set.diagnostics);
    assert_eq!(set.projects.len(), 1);
}

/// A legacy workspace file's entries are confined like a `test.projects` array.
#[test]
fn workspace_file_entry_outside_the_root_is_refused() {
    let fixture = escape_fixture();
    write(
        &fixture.root,
        "vitest.workspace.ts",
        "export default ['../outside']",
    );
    assert_refused_outside(&resolve_projects(&fixture.root), "workspace file");
}

/// An inline project's string `extends` that leaves the root is not read; the project degrades to
/// an unresolvable base. Spellings that fold back inside, or reach a package under the root,
/// still load.
#[test]
fn extends_outside_the_root_is_refused_and_inside_is_kept() {
    let fixture = escape_fixture();
    write(
        &fixture.outside,
        "base.ts",
        "export default { test: { globals: true } };",
    );
    write(
        &fixture.root,
        "shared/base.ts",
        "export default { test: { globals: true } };",
    );
    write(
        &fixture.root,
        "node_modules/@scope/preset/vitest.config.ts",
        "export default { test: { globals: true } };",
    );
    let absolute = js_path(&fixture.outside_spelled.join("base.ts"));
    for target in [
        "../outside/base.ts".to_owned(),
        "../outside/vitest.config.ts".to_owned(),
        absolute,
    ] {
        write(
            &fixture.root,
            "vitest.config.ts",
            &format!(
                "export default {{ test: {{ projects: [{{ extends: '{target}', test: {{ name: 'x' }} }}] }} }};"
            ),
        );
        let set = resolve_projects(&fixture.root);
        let project = single_project(&set);
        let reason = unknown_globals_reason(project);
        assert!(
            reason.contains("resolves outside the analysis root and was not read"),
            "{target}: {reason}"
        );
    }
    for target in [
        "./shared/base.ts",
        "./shared/../shared/base.ts",
        "./node_modules/@scope/preset/vitest.config.ts",
    ] {
        write(
            &fixture.root,
            "vitest.config.ts",
            &format!(
                "export default {{ test: {{ projects: [{{ extends: '{target}', test: {{ name: 'x' }} }}] }} }};"
            ),
        );
        let set = resolve_projects(&fixture.root);
        assert_single_named_enabled(&set, "x");
    }
}

/// A symlinked `extends` target is judged by where it lands.
#[test]
fn extends_through_a_symlink_is_judged_by_its_target() {
    let fixture = escape_fixture();
    write(
        &fixture.root,
        "shared/base.ts",
        "export default { test: { globals: true } };",
    );
    if !crate::confine::try_symlink(
        &fixture.outside.join("vitest.config.ts"),
        &fixture.root.join("escape.ts"),
    ) {
        return;
    }
    assert!(crate::confine::try_symlink(
        &fixture.root.join("shared/base.ts"),
        &fixture.root.join("inside.ts")
    ));
    write(
        &fixture.root,
        "vitest.config.ts",
        "export default { test: { projects: [{ extends: './escape.ts', test: { name: 'x' } }] } };",
    );
    let set = resolve_projects(&fixture.root);
    assert!(unknown_globals_reason(single_project(&set)).contains("outside the analysis root"));
    write(
        &fixture.root,
        "vitest.config.ts",
        "export default { test: { projects: [{ extends: './inside.ts', test: { name: 'x' } }] } };",
    );
    assert_single_named_enabled(&resolve_projects(&fixture.root), "x");
}

/// A `mergeConfig` base imported from outside the root is not read: the root config's project
/// carries an unresolvable base, never the outside file's fields. The same chain inside the root,
/// written with a `..` that folds back, merges as before.
#[test]
fn merge_config_import_outside_the_root_is_refused_and_inside_is_kept() {
    let fixture = escape_fixture();
    write(
        &fixture.outside,
        "base.ts",
        "export default { test: { globals: true } };",
    );
    write(
        &fixture.root,
        "shared/base.ts",
        "export default { test: { globals: true } };",
    );
    let merged = |specifier: &str| {
        format!(
            "import {{ mergeConfig }} from 'vitest/config';\nimport base from '{specifier}';\nexport default mergeConfig(base, {{ test: {{ name: 'merged' }} }});"
        )
    };
    write(
        &fixture.root,
        "vitest.config.ts",
        &merged("../outside/base"),
    );
    let set = resolve_projects(&fixture.root);
    let reason = unknown_globals_reason(single_project(&set));
    assert!(
        reason.contains("mergeConfig base `../outside/base` resolves outside the analysis root"),
        "{reason}"
    );
    assert!(!reason.contains(&js_path(&fixture.root)), "{reason}");

    for specifier in [
        "./shared/base",
        "./shared/../shared/base",
        "./shared/base.ts",
    ] {
        write(&fixture.root, "vitest.config.ts", &merged(specifier));
        let set = resolve_projects(&fixture.root);
        assert_single_named_enabled(&set, "merged");
    }
}

/// The per-directory scope chain applies the same confinement to its `mergeConfig` bases.
#[test]
fn config_scope_merge_base_outside_the_root_is_an_error() {
    let fixture = escape_fixture();
    write(
        &fixture.outside,
        "base.ts",
        "export default { test: { environment: 'node' } };",
    );
    write(
        &fixture.root,
        "shared/base.ts",
        "export default { test: { environment: 'node' } };",
    );
    let merged = |specifier: &str| {
        format!(
            "import {{ mergeConfig }} from 'vitest/config';\nimport base from '{specifier}';\nexport default mergeConfig(base, {{ test: {{ name: 'merged' }} }});"
        )
    };
    let config = fixture.root.join("pkg/vitest.config.ts");
    write(
        &fixture.root,
        "pkg/vitest.config.ts",
        &merged("../../outside/base"),
    );
    let outcome = resolved_config_test_scope_within(&fixture.root, &config);
    assert!(
        outcome
            .as_ref()
            .is_err_and(|reason| reason.contains("outside the analysis root")),
        "{outcome:?}"
    );
    write(
        &fixture.root,
        "pkg/vitest.config.ts",
        &merged("../shared/../shared/base"),
    );
    let outcome = resolved_config_test_scope_within(&fixture.root, &config);
    assert!(outcome.is_ok_and(|scope| scope.is_some()));
}

/// An inline project's `root`/`dir` that leaves the root is refused as an unreadable project; one
/// that folds back inside, or is absolute but inside, scopes the project as before.
#[test]
fn project_root_or_dir_outside_the_root_is_refused_and_inside_is_kept() {
    let fixture = escape_fixture();
    write(&fixture.root, "pkg/src/a.test.ts", "");
    let absolute_outside = js_path(&fixture.outside_spelled);
    let absolute_inside = js_path(&fixture.root_spelled.join("pkg"));
    for key in ["root", "dir"] {
        for declared in [
            "../outside".to_owned(),
            absolute_outside.clone(),
            "./pkg/../../outside".to_owned(),
            "../missing".to_owned(),
        ] {
            write(
                &fixture.root,
                "vitest.config.ts",
                &format!(
                    "export default {{ test: {{ projects: [{{ test: {{ name: 'p', {key}: '{declared}' }} }}] }} }};"
                ),
            );
            let set = resolve_projects(&fixture.root);
            let project = single_project(&set);
            assert!(
                unknown_globals_reason(project).contains("outside the analysis root"),
                "{key}: {declared}"
            );
            assert!(matches!(project.include, GlobPatterns::Unknown { .. }));
        }
        for declared in ["./pkg", "./pkg/../pkg", absolute_inside.as_str()] {
            write(
                &fixture.root,
                "vitest.config.ts",
                &format!(
                    "export default {{ test: {{ projects: [{{ test: {{ name: 'p', {key}: '{declared}', include: ['src/*.test.ts'] }} }}] }} }};"
                ),
            );
            let set = resolve_projects(&fixture.root);
            let project = single_project(&set);
            assert_file_match(
                project,
                &fixture.root.join("pkg/src/a.test.ts"),
                FileMatch::Yes,
                "an inside directory scopes the project",
            );
        }
    }
}

/// A `root` through a symlink is judged by its target.
#[test]
fn project_root_through_a_symlink_is_judged_by_its_target() {
    let fixture = escape_fixture();
    write(&fixture.root, "pkg/src/a.test.ts", "");
    if !crate::confine::try_symlink(&fixture.outside, &fixture.root.join("escape")) {
        return;
    }
    assert!(crate::confine::try_symlink(
        &fixture.root.join("pkg"),
        &fixture.root.join("alias")
    ));
    write(
        &fixture.root,
        "vitest.config.ts",
        "export default { test: { projects: [{ test: { name: 'p', root: './escape' } }] } };",
    );
    let set = resolve_projects(&fixture.root);
    assert!(unknown_globals_reason(single_project(&set)).contains("outside the analysis root"));
    write(
        &fixture.root,
        "vitest.config.ts",
        "export default { test: { projects: [{ test: { name: 'p', root: './alias', include: ['src/*.test.ts'] } }] } };",
    );
    let set = resolve_projects(&fixture.root);
    assert_file_match(
        single_project(&set),
        &fixture.root.join("pkg/src/a.test.ts"),
        FileMatch::Yes,
        "an inside symlink resolves to its target",
    );
}

/// The alternatives a brace pattern stands for, or `None` when it is not enumerated.
fn listed(pattern: &str) -> Option<Vec<String>> {
    match brace_alternatives(pattern) {
        BraceAlternatives::Listed(all) => Some(all),
        BraceAlternatives::None | BraceAlternatives::TooMany => None,
    }
}

/// Only a comma alternation with a closing brace is enumerated; nested groups multiply out and
/// an excess is reported as too many rather than cut short.
#[test]
fn brace_alternatives_enumerates_only_comma_groups_within_the_limit() {
    assert!(matches!(
        brace_alternatives("pkg/*"),
        BraceAlternatives::None
    ));
    assert!(matches!(
        brace_alternatives("{pkg,other"),
        BraceAlternatives::None
    ));
    assert!(matches!(
        brace_alternatives("pkg/{1..3}"),
        BraceAlternatives::None
    ));
    assert_eq!(
        listed("a/{b,c}/d").unwrap(),
        ["a/b/d", "a/c/d"],
        "one group"
    );
    assert_eq!(
        listed("{a,b}/{c,d}").unwrap(),
        ["a/c", "a/d", "b/c", "b/d"],
        "groups multiply"
    );
    let wide = format!("{{{}}}/{{x,y}}", "p,".repeat(40) + "q");
    assert!(
        matches!(brace_alternatives(&wide), BraceAlternatives::TooMany),
        "41 x 2 alternatives exceed the limit through a nested group"
    );
    let flat = format!("{{{}q}}", "p,".repeat(70));
    assert!(matches!(
        brace_alternatives(&flat),
        BraceAlternatives::TooMany
    ));
}

/// Without an analysis root the unreadable base is named by its path rather than relative to it.
#[test]
fn an_unreadable_merge_base_is_recorded_without_a_root() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "base.ts", "export default makeConfig();");
    write(
        directory.path(),
        "merged.ts",
        "import { mergeConfig } from 'vitest/config'\nimport base from './base'\nexport default mergeConfig(base, { test: { globals: true } });",
    );
    let mut refused = false;
    let fields = resolve_standalone_config_bounded(
        None,
        &directory.path().join("merged.ts"),
        None,
        &mut Vec::new(),
        &mut refused,
    )
    .unwrap();
    let reason = fields.refusal.expect("the unreadable base is recorded");
    assert!(reason.contains("could not be read"), "{reason}");
}

/// A group nested inside one that is within the limit can itself exceed it.
#[test]
fn a_nested_brace_group_over_the_limit_is_too_many() {
    let nested = format!("{{a,{{{}q}}}}", "p,".repeat(70));
    assert!(matches!(
        brace_alternatives(&nested),
        BraceAlternatives::TooMany
    ));
}

/// The comparison scope of a config whose `mergeConfig` base cannot be read is an error, so the
/// file is not compared as if the base were absent.
#[test]
fn a_scope_through_an_unreadable_merge_base_is_an_error() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "base.ts", "export default makeConfig();");
    write(
        directory.path(),
        "merged.ts",
        "import { mergeConfig } from 'vitest/config'\nimport base from './base'\nexport default mergeConfig(base, { test: { globals: true } });",
    );
    assert!(resolved_config_test_scope(&directory.path().join("merged.ts")).is_err());
}

/// A declared `root` that does not exist yet is kept as written rather than refused.
#[test]
fn a_declared_project_root_that_does_not_exist_is_kept_as_written() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default { test: { projects: [{ test: { name: 'p', root: './later' } }] } };",
    );
    let set = resolve_projects(directory.path());
    assert_eq!(set.projects.len(), 1, "{set:?}");
    assert!(
        set.projects[0].base_dir.ends_with("later"),
        "{:?}",
        set.projects[0].base_dir
    );
    assert!(!set.has_unresolved_entries, "{set:?}");
}

/// Two configs that merge each other have no resolvable chain: the scope is an error and the
/// root config's project set is unresolved rather than built from half a chain.
#[test]
fn a_cyclic_merge_chain_is_refused_for_scope_and_for_projects() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "import { mergeConfig } from 'vitest/config'\nimport other from './other'\nexport default mergeConfig(other, { test: { globals: true } });",
    );
    write(
        directory.path(),
        "other.ts",
        "import { mergeConfig } from 'vitest/config'\nimport back from './vitest.config'\nexport default mergeConfig(back, { test: { globals: false } });",
    );
    let error = resolved_config_test_scope(&directory.path().join("vitest.config.ts"))
        .expect_err("a cycle has no scope");
    assert!(error.contains("cyclic"), "{error}");
    let set = resolve_projects(directory.path());
    assert_unresolved_with_diagnostic(&set, "merged default export could not be resolved");
}

/// A named import whose export is not a readable literal makes the scope an error.
#[test]
fn a_named_merge_base_that_is_not_a_literal_is_an_error_for_scope() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "base.ts", "export const other = {};");
    write(
        directory.path(),
        "merged.ts",
        "import { mergeConfig } from 'vitest/config'\nimport { base } from './base'\nexport default mergeConfig(base, { test: { globals: true } });",
    );
    let error = resolved_config_test_scope(&directory.path().join("merged.ts"))
        .expect_err("the named export is missing");
    assert!(error.contains("export cannot be read"), "{error}");
}

/// An entry left unresolved is a set-level diagnostic and an unresolved entry, not only `Unknown`
/// fields on its project: those are ignored once a later project governs the same files.
#[test]
fn an_unresolved_entry_is_recorded_on_the_set() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "pkg/vitest.config.ts",
        "export default makeConfig();",
    );
    for (entry, needle) in [
        (
            "{ test: { name: 'a', root: '../outside' } }",
            "resolves outside",
        ),
        ("{ test: someTests }", "sets `test` to something"),
        (
            "'./pkg/vitest.config.ts'",
            "could not be read as a standalone",
        ),
    ] {
        write(
            directory.path(),
            "vitest.config.ts",
            &format!(
                "export default {{ test: {{ projects: [{entry}, {{ test: {{ name: 'b' }} }}] }} }};"
            ),
        );
        let set = resolve_projects(directory.path());
        assert!(set.has_unresolved_entries, "{entry}: {set:?}");
        assert!(
            set.diagnostics.iter().any(|note| note.contains(needle)),
            "{entry}: {:?}",
            set.diagnostics
        );
    }
}
