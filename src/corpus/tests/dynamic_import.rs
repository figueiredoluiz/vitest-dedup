//! A relative `import()` is keyed by the root-relative path it names, not by its spelling alone.
use super::module_meta::{compared_pair, distinct_modules, duplicate};
use super::*;
use crate::model::Rule;

const FOUR: &str = "export const servings = 4\n";
const TWELVE: &str = "export const servings = 12\n";

/// A test titled `title` that reads `servings` from what `specifier` (source text) loads.
fn reads(title: &str, specifier: &str) -> String {
    format!("it('{title}', async () => {{ const {{ servings }} = await import({specifier}); expect(servings).toBe(4) }})\n")
}

/// A test file holding one [`reads`] test per specifier.
fn loading(specifiers: &[&str]) -> String {
    let mut source = "import { it, expect } from 'vitest'\n".to_owned();
    for (index, specifier) in specifiers.iter().enumerate() {
        source.push_str(&reads(&format!("reads {index}"), specifier));
    }
    source
}

/// Each named file's one comparable test, read in one run, with its cross-file outcome.
fn cross_file(files: &[(&str, &str)], tests: &[&str]) -> (Vec<FileCorpus>, crate::rules::Outcome) {
    let results = corpora(files, tests);
    for (path, corpus) in tests.iter().zip(&results) {
        assert_eq!(corpus.tests.len(), 1, "{path}");
        assert!(corpus.tests[0].exclusion.is_none(), "{path}");
    }
    let outcome = owned_outcome(tests.iter().copied().zip(results.clone()).collect());
    (results, outcome)
}

/// Whether the outcome links the tests of exactly the files at `paths`.
fn links(outcome: &crate::rules::Outcome, paths: &[&str]) -> bool {
    outcome.findings.iter().any(|finding| {
        let mut found: Vec<_> = finding
            .locations
            .iter()
            .map(|location| location.path.clone())
            .collect();
        found.sort();
        finding.rule == Rule::CrossFileDuplicate
            && found
                == paths
                    .iter()
                    .map(std::path::PathBuf::from)
                    .collect::<Vec<_>>()
    })
}

/// A constant template and a string naming one path are one test; two equal-content modules
/// are two; a spelling that names the file another way stays apart, since only Vitest's own
/// resolution could prove it is the same file.
#[test]
fn dynamic_import_keys_name_the_path_not_its_spelling() {
    let directory = loading(&["'./one/recipes/.'", "'./one/recipes/'"]);
    let corpus = compared_pair(
        &[("one/recipes/index.ts", FOUR), ("case.test.ts", &directory)],
        "case.test.ts",
    );
    assert_eq!(corpus.tests[0].key, corpus.tests[1].key);
    duplicate("case.test.ts", corpus);

    let template = loading(&["`./one/recipes`", "'./one/recipes'"]);
    let corpus = compared_pair(
        &[("one/recipes.ts", FOUR), ("case.test.ts", &template)],
        "case.test.ts",
    );
    assert_eq!(corpus.tests[0].key, corpus.tests[1].key);
    duplicate("case.test.ts", corpus);

    // Equal content in two files: identity is the file, not what it holds.
    let apart = loading(&["'./one/recipes'", "'./two/recipes'"]);
    distinct_modules(
        &[
            ("one/recipes.ts", FOUR),
            ("two/recipes.ts", FOUR),
            ("case.test.ts", &apart),
        ],
        "case.test.ts",
    );
    for other in [
        "'./one/recipes.ts'",
        "'./one/recipes.js'",
        "'./one/recipes#copy'",
        "'./one/recipes/'",
    ] {
        let spelled = loading(&["'./one/recipes'", other]);
        distinct_modules(
            &[
                ("one/recipes.ts", FOUR),
                ("one/recipes.js", FOUR),
                ("case.test.ts", &spelled),
            ],
            "case.test.ts",
        );
    }
}

/// One spelling in two folders names two modules; two spellings in two folders name one.
#[test]
fn dynamic_import_keys_follow_the_module_across_files() {
    let local = loading(&["'./recipes'"]);
    let reach = loading(&["'../one/recipes'"]);
    let files = [
        ("one/recipes.ts", FOUR),
        ("two/recipes.ts", TWELVE),
        ("one/a.test.ts", local.as_str()),
        ("two/a.test.ts", local.as_str()),
        ("three/a.test.ts", reach.as_str()),
    ];
    let (results, outcome) = cross_file(
        &files,
        &["one/a.test.ts", "two/a.test.ts", "three/a.test.ts"],
    );
    let key = |index: usize| results[index].tests[0].module_key;
    assert_ne!(key(0), key(1), "the same spelling names another file");
    assert_eq!(key(0), key(2), "another spelling names the same file");
    assert!(links(&outcome, &["one/a.test.ts", "three/a.test.ts"]));
    assert!(!links(&outcome, &["one/a.test.ts", "two/a.test.ts"]));
    assert!(!links(&outcome, &["three/a.test.ts", "two/a.test.ts"]));
}

/// Checkout location is not module identity: keys repeat across two roots and still tell the
/// modules apart within each.
#[test]
fn dynamic_import_keys_are_root_relative() {
    let local = loading(&["'./recipes'"]);
    let files = [
        ("one/recipes.ts", FOUR),
        ("two/recipes.ts", TWELVE),
        ("one/a.test.ts", local.as_str()),
        ("two/a.test.ts", local.as_str()),
    ];
    let tests = ["one/a.test.ts", "two/a.test.ts"];
    let keys = || -> Vec<(u64, u64)> {
        cross_file(&files, &tests)
            .0
            .iter()
            .map(|corpus| (corpus.tests[0].key, corpus.tests[0].module_key))
            .collect()
    };
    let (first, repeated) = (keys(), keys());
    assert_ne!(first[0], first[1]);
    assert_eq!(first, repeated);
}

/// A query, a missing file, a builtin and the API: equal spellings stay one test and different
/// ones stay apart. A query names another module than the plain path.
#[test]
fn dynamic_import_keys_keep_other_spellings_apart() {
    for (equal, different) in [
        ("'./recipes?raw'", "'./recipes'"),
        ("'./missing'", "'./absent'"),
        ("'node:os'", "'node:path'"),
        ("'vitest'", "'vitest/config'"),
    ] {
        let same = loading(&[equal, equal]);
        let corpus = compared_pair(
            &[("recipes.ts", FOUR), ("case.test.ts", &same)],
            "case.test.ts",
        );
        assert_eq!(corpus.tests[0].key, corpus.tests[1].key, "{equal}");
        duplicate("case.test.ts", corpus);
        let apart = loading(&[equal, different]);
        distinct_modules(
            &[("recipes.ts", FOUR), ("case.test.ts", &apart)],
            "case.test.ts",
        );
    }
}
