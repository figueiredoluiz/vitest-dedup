//! An `import()` that may read a relative path is keyed by its spelling and the directory of the
//! file that imports it.
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

/// In one file, a constant template, a constant binding and a table row holding the literal are
/// the literal; any other spelling is another module, since only Vitest's resolution could prove
/// two spellings load one file.
#[test]
fn dynamic_import_keys_keep_the_spelling_within_a_file() {
    let files = [
        ("one/recipes.ts", FOUR),
        ("one/recipes.js", FOUR),
        ("one/recipes/index.ts", FOUR),
        ("two/recipes.ts", FOUR),
    ];
    let header = "import { it, expect } from 'vitest'\n";
    let literal = reads("reads 1", "'./one/recipes'");
    for equal in [
        loading(&["`./one/recipes`", "'./one/recipes'"]),
        format!("{header}const path = './one/recipes'\n{}{literal}", reads("reads 0", "path")),
        format!(
            "{header}it.each(['./one/recipes'])('reads %s', async (path) => {{ const {{ servings }} = await import(path); expect(servings).toBe(4) }})\n{literal}"
        ),
    ] {
        let corpus = compared_pair(
            &[files.as_slice(), &[("case.test.ts", equal.as_str())]].concat(),
            "case.test.ts",
        );
        assert_eq!(corpus.tests[0].key, corpus.tests[1].key, "{equal}");
        duplicate("case.test.ts", corpus);
    }
    for other in [
        "'./two/recipes'",
        "'./one/recipes.ts'",
        "'./one/recipes.js'",
        "'./one/recipes#copy'",
        "'./one/recipes?raw'",
        "'./one/recipes/'",
        "'./one/recipes/.'",
        "'./one/../one/recipes'",
    ] {
        let spelled = loading(&["'./one/recipes'", other]);
        distinct_modules(
            &[files.as_slice(), &[("case.test.ts", spelled.as_str())]].concat(),
            "case.test.ts",
        );
    }
}

/// One spelling in two folders names two modules, a literal or a constant; two files in one
/// folder name one.
#[test]
fn dynamic_import_keys_follow_the_importing_folder() {
    let local = loading(&["'./recipes'"]);
    let constant = format!(
        "import {{ it, expect }} from 'vitest'\nconst path = './recipes'\n{}",
        reads("reads 0", "path")
    );
    let files = [
        ("one/recipes.ts", FOUR),
        ("two/recipes.ts", TWELVE),
        ("one/a.test.ts", local.as_str()),
        ("one/b.test.ts", local.as_str()),
        ("two/a.test.ts", local.as_str()),
        ("one/c.test.ts", constant.as_str()),
        ("two/c.test.ts", constant.as_str()),
    ];
    let tests = [
        "one/a.test.ts",
        "one/b.test.ts",
        "two/a.test.ts",
        "one/c.test.ts",
        "two/c.test.ts",
    ];
    let (results, outcome) = cross_file(&files, &tests);
    let key = |index: usize| results[index].tests[0].module_key;
    assert_eq!(key(0), key(1), "one folder, one module");
    assert_ne!(key(0), key(2), "the same spelling names another file");
    assert!(links(&outcome, &["one/a.test.ts", "one/b.test.ts"]));
    assert!(!links(&outcome, &["one/a.test.ts", "two/a.test.ts"]));
    assert!(!links(&outcome, &["one/c.test.ts", "two/c.test.ts"]));
}

/// Checkout location is not module identity: keys repeat across two roots and still tell the
/// folders apart within each.
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

/// A package, a builtin or the API held by a constant is the literal it holds; one spelling in
/// two folders is two, since whether a source is relative is not decided from its syntax.
#[test]
fn dynamic_import_keys_read_packages_from_the_importing_folder() {
    let header = "import { it, expect } from 'vitest'\n";
    for (spelling, other) in [
        ("'node:os'", "'node:path'"),
        ("'vitest'", "'vitest/config'"),
    ] {
        let constant = format!(
            "{header}const path = {spelling}\n{}{}",
            reads("reads 0", "path"),
            reads("reads 1", spelling)
        );
        let corpus = compared_pair(&[("case.test.ts", constant.as_str())], "case.test.ts");
        assert_eq!(corpus.tests[0].key, corpus.tests[1].key, "{spelling}");
        duplicate("case.test.ts", corpus);
        let apart = loading(&[spelling, other]);
        distinct_modules(&[("case.test.ts", apart.as_str())], "case.test.ts");
        let same = loading(&[spelling]);
        let files = [
            ("one/a.test.ts", same.as_str()),
            ("two/a.test.ts", same.as_str()),
        ];
        let (_, outcome) = cross_file(&files, &["one/a.test.ts", "two/a.test.ts"]);
        assert!(
            !links(&outcome, &["one/a.test.ts", "two/a.test.ts"]),
            "{spelling}"
        );
    }
}
