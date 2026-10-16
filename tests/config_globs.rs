//! Glob component boundaries through discovery, extraction, and duplicate findings.

use std::fs;
use vitest_dedup::analysis::analyze;
use vitest_dedup::config::{Config, ConfigOverrides};
use vitest_dedup::discovery::discover;
use vitest_dedup::frontend::{extract, Globals};
use vitest_dedup::model::{IncompleteCauses, Rule};
use vitest_dedup::vitest_projects::resolve_projects;

const SOURCE: &str = include_str!("fixtures/config-globs/duplicate-source.txt");

/// Assert the exact admitted paths and retained findings for one configuration.
fn check_scope(tool_config: Option<&str>, vitest_config: Option<&str>, expected: &[&str]) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for name in ["src/a.test.ts", "src/deep/er/n.test.ts"] {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, SOURCE).unwrap();
    }
    if let Some(config) = tool_config {
        fs::write(root.join(".vitest-dedup.json"), config).unwrap();
    }
    if let Some(config) = vitest_config {
        fs::write(root.join("vitest.config.ts"), config).unwrap();
    }
    let config = Config::load(root, ConfigOverrides::default()).unwrap();
    let projects = resolve_projects(&config.root);
    let discovered = discover(&config, vitest_config.map(|_| &projects)).unwrap();
    assert!(!discovered.incomplete, "{:?}", discovered.errors);
    let mut actual: Vec<_> = discovered
        .files
        .iter()
        .map(|file| {
            file.path
                .strip_prefix(&config.root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    actual.sort();
    assert_eq!(actual, expected);
    let mut tests = Vec::new();
    for file in &discovered.files {
        let extracted = extract(SOURCE, file, Globals::Disabled);
        assert!(!extracted.incomplete, "{:?}", extracted.diagnostics);
        assert_eq!(extracted.tests.len(), 2);
        tests.extend(extracted.tests);
    }
    let result = analyze(
        tests,
        &config,
        discovered.files.len(),
        IncompleteCauses::default(),
    )
    .unwrap();
    assert_eq!(result.files_analyzed, expected.len());
    assert_eq!(result.tests_analyzed, expected.len() * 2);
    assert_eq!(
        result
            .findings
            .iter()
            .filter(|f| f.rule == Rule::DuplicateTest)
            .count(),
        expected.len()
    );
}

/// The exact owned source and paths used by the Vitest runtime discovery oracle.
#[test]
fn tool_globs_preserve_component_boundaries_and_recursive_positive_findings() {
    for pattern in ["src/*.test.ts", "src/?.test.ts", "./src/*.test.ts"] {
        check_scope(
            Some(&format!(r#"{{"include":["{pattern}"]}}"#)),
            None,
            &["src/a.test.ts"],
        );
    }
    check_scope(
        Some(r#"{"include":["src/**/*.test.ts"]}"#),
        None,
        &["src/a.test.ts", "src/deep/er/n.test.ts"],
    );
    check_scope(
        Some(r#"{"include":["src/**/*.test.ts"],"exclude":["src/*.test.ts"]}"#),
        None,
        &["src/deep/er/n.test.ts"],
    );
}

/// Project channels use the same component semantics as explicit analyzer globs.
#[test]
fn project_globs_preserve_component_boundaries_and_recursive_positive_findings() {
    for pattern in ["src/*.test.ts", "src/?.test.ts", "./src/*.test.ts"] {
        check_scope(
            None,
            Some(&format!(
                "export default {{ test: {{ include: ['{pattern}'], exclude: [] }} }};"
            )),
            &["src/a.test.ts"],
        );
    }
    check_scope(
        None,
        Some("export default { test: { include: ['src/**/*.test.ts'], exclude: [] } };"),
        &["src/a.test.ts", "src/deep/er/n.test.ts"],
    );
    check_scope(
        None,
        Some(
            "export default { test: { include: ['src/**/*.test.ts'], exclude: ['src/*.test.ts'] } };",
        ),
        &["src/deep/er/n.test.ts"],
    );
}

/// Additive in-source channels and benchmark channels retain the same boundary.
#[test]
fn additive_channels_do_not_let_single_stars_cross_directories() {
    for fields in [
        "include: [], exclude: [], includeSource: ['src/*.test.ts']",
        "include: [], exclude: [], benchmark: { include: ['src/*.test.ts'], exclude: [] }",
        "include: [], exclude: [], benchmark: { include: [], exclude: [], includeSource: ['src/*.test.ts'] }",
    ] {
        check_scope(
            None,
            Some(&format!("export default {{ test: {{ {fields} }} }};")),
            &["src/a.test.ts"],
        );
    }
}

/// A single question mark cannot stand for exactly one directory separator.
#[test]
fn question_mark_cannot_replace_a_directory_separator() {
    check_scope(
        Some(r#"{"include":["src?deep/er/n.test.ts","src/a.test.ts"]}"#),
        None,
        &["src/a.test.ts"],
    );
    check_scope(
        None,
        Some("export default { test: { include: ['src?deep/er/n.test.ts', 'src/a.test.ts'], exclude: [] } };"),
        &["src/a.test.ts"],
    );
    check_scope(
        Some(r#"{"include":["src/**/*.test.ts"],"exclude":["src?deep/er/n.test.ts"]}"#),
        None,
        &["src/a.test.ts", "src/deep/er/n.test.ts"],
    );
    check_scope(
        None,
        Some("export default { test: { include: ['src/**/*.test.ts'], exclude: ['src?deep/er/n.test.ts'] } };"),
        &["src/a.test.ts", "src/deep/er/n.test.ts"],
    );
}
