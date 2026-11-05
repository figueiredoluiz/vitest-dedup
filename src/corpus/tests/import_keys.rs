//! A static import a key cannot name by its exports is named by the root-relative file it resolves
//! to, with the query Vite loads it by.

use super::*;

/// The test's same-file key and module key in `source`, read beside `recipes.ts` and `setup.css`
/// in a fresh root.
fn keys_in_fresh_root(source: &str) -> (u64, u64) {
    let files = [
        ("a.test.ts", source),
        ("recipes.ts", "export const servings = 4\n"),
        ("setup.css", "body {}\n"),
    ];
    let corpus = corpora(&files, &["a.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 1, "{source}");
    let test = &corpus.tests[0];
    assert_eq!(test.exclusion, None, "{source}");
    (test.key, test.module_key)
}

/// One tree keys alike wherever it lies, and a `?query` keeps two loads of one file apart.
#[test]
fn import_keys_are_root_relative_and_keep_their_query() {
    let test = "test('t', () => { expect(m).toBeDefined() })\n";
    let namespace = |specifier: &str| {
        format!("{HEADER}import * as m from '{specifier}'\nimport './setup.css'\n{test}")
    };
    let plain = namespace("./recipes");
    assert_eq!(keys_in_fresh_root(&plain), keys_in_fresh_root(&plain));
    let raw = namespace("./recipes?raw");
    assert_eq!(keys_in_fresh_root(&raw), keys_in_fresh_root(&raw));
    assert_ne!(keys_in_fresh_root(&raw).0, keys_in_fresh_root(&plain).0);
    let bare = |specifier: &str| {
        format!("{HEADER}import '{specifier}'\ntest('t', () => {{ expect(1).toBe(1) }})\n")
    };
    assert_ne!(
        keys_in_fresh_root(&bare("./setup.css?inline")).1,
        keys_in_fresh_root(&bare("./setup.css")).1
    );
    let named = |specifier: &str| {
        format!("{HEADER}import {{ servings }} from '{specifier}'\ntest('t', () => {{ expect(servings).toBe(4) }})\n")
    };
    assert_ne!(
        keys_in_fresh_root(&named("./recipes?raw")).0,
        keys_in_fresh_root(&named("./recipes")).0
    );
    assert_eq!(
        keys_in_fresh_root(&named("./recipes")).0,
        keys_in_fresh_root(&named("./recipes.ts")).0
    );
}

/// A leading `#` is a package import, so only a later `?` or `#` is a query.
#[test]
fn only_a_later_query_or_fragment_is_kept() {
    assert_eq!(crate::lang::query("./a?raw"), "?raw");
    assert_eq!(crate::lang::query("./a#frag"), "#frag");
    assert_eq!(crate::lang::query("#internal"), "");
    assert_eq!(crate::lang::query("#internal?url"), "?url");
    assert_eq!(crate::lang::query("./a"), "");
}

/// A project file and a package file of one relative spelling are two modules: the project path
/// is tagged as a file, the package as the spelling it was not resolved from.
#[test]
fn a_project_path_never_keys_as_a_package_spelling() {
    let test = |specifier: &str| {
        format!("{HEADER}import * as m from '{specifier}'\ntest('t', () => {{ expect(m.side).toBe('project') }})\n")
    };
    let project = test("./foo/index.js");
    let package = test("foo/index.js");
    let files = [
        ("a.test.ts", project.as_str()),
        ("b.test.ts", package.as_str()),
        ("foo/index.js", "export const side = 'project'\n"),
        (
            "node_modules/foo/index.js",
            "export const side = 'package'\n",
        ),
        ("node_modules/foo/package.json", "{\"name\":\"foo\"}\n"),
    ];
    let corpora = corpora(&files, &["a.test.ts", "b.test.ts"]);
    assert!(corpora.iter().all(|corpus| corpus.tests.len() == 1));
    assert_ne!(corpora[0].tests[0].key, corpora[1].tests[0].key);
}
