//! Generated config discovery, user precedence and per-context ownership controls.
use super::*;
use oxc_resolver::FileSystemOs;

const ALIASES: &str = r#"{"compilerOptions":{"paths":{"~/*":["../src/*"]}},"include":["../**/*"]}"#;

/// Build a fresh source view before any resolver can cache its configuration.
fn fixture(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let root = dir.path().canonicalize().unwrap();
    (dir, root)
}

/// Exercise the production working resolver, including its project-file confinement policy.
fn resolved(root: &Path, from: &str, specifier: &str) -> Option<PathBuf> {
    super::super::resolve::resolve_file(&root.join(from), specifier, root)
        .map(|path| path.canonicalize().unwrap())
}

/// Every supported Nuxt marker routes aliases through one shared discovery policy.
#[test]
fn markers_discover_legacy_aliases_without_a_root_config() {
    let manifests = [
        r#"{"dependencies":{"nuxt":"3"}}"#,
        r#"{"devDependencies":{"nuxt":"3"}}"#,
        r#"{"optionalDependencies":{"nuxt":"3"}}"#,
    ];
    let markers: Vec<_> = CONFIG_NAMES
        .into_iter()
        .map(|name| (name, "export default {}"))
        .chain(manifests.into_iter().map(|text| ("package.json", text)))
        .collect();
    for marker in markers {
        let (_dir, root) = fixture(&[
            marker,
            (".nuxt/tsconfig.json", ALIASES),
            ("tests/unit/run.test.ts", ""),
            ("src/helper.ts", "export const value = 1"),
        ]);
        for _ in 0..2 {
            assert_eq!(
                resolved(&root, "tests/unit/run.test.ts", "~/helper"),
                Some(root.join("src/helper.ts")),
                "{marker:?}"
            );
            assert_eq!(
                resolved(&root, "tests/unit/run.test.ts", "../../src/helper"),
                Some(root.join("src/helper.ts"))
            );
        }
        assert!(
            !root.join("tsconfig.json").exists(),
            "discovery must not write a config"
        );
    }
}

/// Nuxt 4 references choose aliases by recorded file ownership, including with a real root.
#[test]
fn context_references_keep_app_server_shared_and_node_aliases_separate() {
    for root_config in [None, Some(CONTEXTS)] {
        let (_dir, root) = fixture(&[("nuxt.config.ts", "export default {}")]);
        for context in ["app", "server", "shared", "node"] {
            std::fs::create_dir_all(root.join(context)).unwrap();
            std::fs::write(root.join(context).join("run.test.ts"), "").unwrap();
            std::fs::write(root.join(context).join("helper.ts"), "").unwrap();
            std::fs::create_dir_all(root.join(".nuxt")).unwrap();
            let config = serde_json::json!({
                "compilerOptions": { "paths": { "~/*": [format!("../{context}/*")] } },
                "include": [format!("../{context}/**/*")]
            });
            std::fs::write(
                root.join(format!(".nuxt/tsconfig.{context}.json")),
                config.to_string(),
            )
            .unwrap();
        }
        // Nuxt 4 also emits a legacy config; its broader aliases must not override contexts.
        std::fs::write(root.join(".nuxt/tsconfig.json"), ALIASES).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/helper.ts"), "").unwrap();
        if let Some(config) = root_config {
            std::fs::write(root.join("tsconfig.json"), config).unwrap();
        }
        std::fs::write(root.join("unowned.test.ts"), "").unwrap();
        for context in ["app", "server", "shared", "node"] {
            assert_eq!(
                resolved(&root, &format!("{context}/run.test.ts"), "~/helper"),
                Some(root.join(context).join("helper.ts"))
            );
        }
        assert_eq!(resolved(&root, "unowned.test.ts", "~/helper"), None);
    }
}

/// Explicit user configuration never loses an alias, an exclusion, or a load failure to fallback.
#[test]
fn user_configs_remain_authoritative_including_exclusions_and_errors() {
    for (config, expected) in [
        (
            r#"{"compilerOptions":{"paths":{"~/*":["custom/*"]}}}"#,
            Some("custom/helper.ts"),
        ),
        (r#"{"include":["custom/**/*"]}"#, None),
        (
            r#"{"compilerOptions":{"paths":{"other/*":["custom/*"]}}}"#,
            None,
        ),
        (r#"{"extends":"./missing.json"}"#, None),
        ("not json", None),
    ] {
        let (_dir, root) = fixture(&[
            ("nuxt.config.ts", "export default {}"),
            ("tsconfig.json", config),
            (".nuxt/tsconfig.json", ALIASES),
            ("run.test.ts", ""),
            ("src/helper.ts", ""),
            ("custom/helper.ts", ""),
        ]);
        assert_eq!(
            resolved(&root, "run.test.ts", "~/helper"),
            expected.map(|path| root.join(path)),
            "{config}"
        );
    }
}

/// Nuxt evidence and its aliases cannot leak into a separate package or across the analysis root.
#[test]
fn discovery_stays_inside_the_nearest_project_and_analysis_root() {
    let (_dir, root) = fixture(&[
        ("nuxt.config.ts", "export default {}"),
        (".nuxt/tsconfig.json", ALIASES),
        ("src/helper.ts", ""),
        ("packages/plain/package.json", "{}"),
        ("packages/plain/run.test.ts", ""),
        ("packages/web/nuxt.config.mjs", "export default {}"),
        ("packages/web/.nuxt/tsconfig.json", ALIASES),
        ("packages/web/src/helper.ts", ""),
        ("packages/web/run.test.ts", ""),
        ("scope/run.test.ts", ""),
    ]);
    assert_eq!(
        resolved(&root, "packages/plain/run.test.ts", "~/helper"),
        None
    );
    assert_eq!(
        resolved(&root, "packages/web/run.test.ts", "~/helper"),
        Some(root.join("packages/web/src/helper.ts"))
    );
    let scope = root.join("scope");
    assert_eq!(resolved(&scope, "run.test.ts", "~/helper"), None);
    for manifest in [
        None,
        Some("{}"),
        Some(r#"{"peerDependencies":{"nuxt":"4"}}"#),
    ] {
        let (_dir, root) = fixture(&[
            (".nuxt/tsconfig.json", ALIASES),
            ("run.test.ts", ""),
            ("src/helper.ts", ""),
        ]);
        if let Some(manifest) = manifest {
            std::fs::write(root.join("package.json"), manifest).unwrap();
        }
        assert_eq!(resolved(&root, "run.test.ts", "~/helper"), None);
    }
}

/// Missing, partial and invalid generated configurations retain typed failure diagnostics.
#[test]
fn generated_config_failures_are_not_silently_retried_without_aliases() {
    for (generated, extra, absent) in [
        (None, None, true),
        (Some("not json"), None, false),
        (None, Some((".nuxt/tsconfig.app.json", "{}")), true),
        (None, Some((".nuxt/tsconfig.shared.json", "{}")), true),
    ] {
        let (_dir, root) = fixture(&[
            ("nuxt.config.ts", "export default {}"),
            ("run.test.ts", ""),
            ("src/helper.ts", ""),
        ]);
        if generated.is_some() || extra.is_some() {
            std::fs::create_dir_all(root.join(".nuxt")).unwrap();
        }
        if let Some(text) = generated {
            std::fs::write(root.join(".nuxt/tsconfig.json"), text).unwrap();
        }
        if let Some((path, text)) = extra {
            std::fs::write(root.join(path), text).unwrap();
        }
        let fs = FileSystemOs::new();
        let resolver =
            ResolverGeneric::new_with_file_system(fs.clone(), super::super::resolve::options());
        let error = resolve_file(
            &resolver,
            &fs,
            &root.join("run.test.ts"),
            "./src/helper",
            &root,
        )
        .unwrap_err();
        let gap = super::super::resolve::classify_tsconfig_gap(&error).unwrap();
        assert_eq!(
            matches!(gap, super::super::resolve::TsconfigGap::Absent(_)),
            absent,
            "{error}"
        );
        assert!(resolved(&root, "run.test.ts", "./src/helper").is_none());
        let missing = crate::prerequisites::assess(
            &root,
            &super::super::resolve::ResolutionGaps {
                tsconfig: std::collections::BTreeMap::from([(gap, 1)]),
                ..Default::default()
            },
        );
        assert!(
            missing
                .iter()
                .any(|item| item.path.starts_with(".nuxt/")
                    && (!absent || item.fix == "nuxt prepare")),
            "{missing:?}"
        );
    }
}

/// Generated aliases remain confined while ordinary imports inside the project still work.
#[test]
fn generated_aliases_cannot_escape_the_analysis_root() {
    let (_dir, root) = fixture(&[
        ("project/nuxt.config.ts", "export default {}"),
        (
            "project/.nuxt/tsconfig.json",
            r#"{"compilerOptions":{"paths":{"~/*":["../../outside/*"]}},"include":["../**/*"]}"#,
        ),
        ("project/run.test.ts", ""),
        ("project/local.ts", ""),
        ("outside/helper.ts", ""),
    ]);
    let project = root.join("project");
    let _ = super::super::resolve::take_confinement_refusal();
    assert_eq!(resolved(&project, "run.test.ts", "~/helper"), None);
    assert!(super::super::resolve::take_confinement_refusal());
    assert_eq!(
        resolved(&project, "run.test.ts", "./local"),
        Some(project.join("local.ts"))
    );
    assert!(!super::super::resolve::take_confinement_refusal());
}

/// The virtual config adapter never invents files when constructed without a project.
#[test]
fn virtual_config_factory_and_file_interfaces_preserve_real_sources() {
    let (_dir, root) = fixture(&[("nuxt.config.ts", "real content")]);
    let empty = GeneratedConfig::<FileSystemOs>::new();
    let virtual_path = root.join("tsconfig.json");
    assert!(empty.metadata(&virtual_path).is_err());
    let fs = GeneratedConfig {
        inner: FileSystemOs::new(),
        config: Some((virtual_path.clone(), LEGACY)),
    };
    assert_eq!(fs.read(&virtual_path).unwrap(), LEGACY.as_bytes());
    assert_eq!(fs.read_to_string(&virtual_path).unwrap(), LEGACY);
    assert!(fs.metadata(&virtual_path).unwrap().is_file());
    assert!(fs.symlink_metadata(&virtual_path).unwrap().is_file());
    assert_eq!(fs.canonicalize(&virtual_path).unwrap(), virtual_path);
    let real = root.join("nuxt.config.ts");
    assert_eq!(fs.read(&real).unwrap(), b"real content");
    assert_eq!(fs.read_to_string(&real).unwrap(), "real content");
    assert!(fs.metadata(&real).unwrap().is_file());
    assert!(fs.symlink_metadata(&real).unwrap().is_file());
    assert_eq!(fs.canonicalize(&real).unwrap(), real);
    assert!(fs.read_link(&real).is_err());
}

/// A supplied caller needs no physical directory to resolve an absolute project import.
#[test]
fn unavailable_project_metadata_preserves_ordinary_resolution() {
    let (_dir, root) = fixture(&[("helper.ts", "export const value = 1")]);
    let helper = root.join("helper.ts");
    let specifier = resolver_path(&helper).to_string_lossy().into_owned();
    let ordinary = ResolverGeneric::<FileSystemOs>::new(super::super::resolve::options())
        .resolve_file(root.join("supplied/run.test.ts"), &specifier)
        .unwrap()
        .into_path_buf();
    assert_eq!(ordinary.canonicalize().unwrap(), helper);
    assert_eq!(
        resolved(&root, "supplied/run.test.ts", &specifier),
        Some(helper)
    );
}

/// Canonical and symlink spellings discover the same generated config and target identity.
#[cfg(unix)]
#[test]
fn a_symlinked_root_uses_the_same_generated_configuration() {
    let (_dir, root) = fixture(&[
        ("project/nuxt.config.ts", "export default {}"),
        ("project/.nuxt/tsconfig.json", ALIASES),
        ("project/run.test.ts", ""),
        ("project/src/helper.ts", ""),
    ]);
    let linked = root.join("linked");
    std::os::unix::fs::symlink(root.join("project"), &linked).unwrap();
    assert_eq!(
        resolved(&linked, "run.test.ts", "~/helper"),
        Some(root.join("project/src/helper.ts"))
    );
}

/// Windows canonical importer prefixes cannot turn a file path into a resolver query.
#[cfg(windows)]
#[test]
fn canonical_windows_importers_preserve_generated_and_user_config_identity() {
    for (verbatim, ordinary) in [
        (r"\\?\C:\project\run.test.ts", r"C:\project\run.test.ts"),
        (
            r"\\?\UNC\server\share\run.test.ts",
            r"\\server\share\run.test.ts",
        ),
    ] {
        assert_eq!(
            resolver_path(Path::new(verbatim)).as_ref(),
            Path::new(ordinary)
        );
    }
    for config in [None, Some(r#"{"extends":"./.nuxt/tsconfig.json"}"#)] {
        let (dir, root) = fixture(&[
            ("nuxt.config.ts", "export default {}"),
            (".nuxt/tsconfig.json", ALIASES),
            ("run.test.ts", ""),
            ("src/helper.ts", ""),
        ]);
        if let Some(config) = config {
            std::fs::write(root.join("tsconfig.json"), config).unwrap();
        }
        for spelling in [dir.path().to_owned(), root.clone()] {
            assert_eq!(
                resolved(&spelling, "run.test.ts", "~/helper"),
                Some(root.join("src/helper.ts"))
            );
        }
    }
}
