//! Whether importing an installed package can register Vitest tests.
//!
//! A helper module that imports a package runs that package's code, and a package's body is a
//! build artifact the analyzer never reads. What can be read is its manifest: code that
//! registers Vitest tests has to reach Vitest, so a package whose whole dependency closure names
//! no test runner cannot do it. Reading the closure is bounded and conservative -- a manifest
//! that cannot be read, a dependency that is not installed, or a closure past the cap all mean
//! "cannot tell", which is the refusal this check replaces.
//!
//! Only registration is ruled out. Import-time side effects of a package (patching globals,
//! polyfills) are not modelled, exactly as they are not for a package the test file imports.

use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

/// Most packages one closure may visit before it is refused.
const MAX_PACKAGES: usize = 256;

thread_local! {
    /// Verdict per package root, so a barrel's many imports of one package read its closure once.
    static VERDICTS: RefCell<HashMap<PathBuf, bool>> = RefCell::new(HashMap::new());
    /// Whether an entry file names Vitest, per file, so a shared entry is read once.
    static ENTRIES: RefCell<HashMap<(PathBuf, bool), bool>> = RefCell::new(HashMap::new());
}

/// Whether `name` is a test runner, or a package that exists to extend one.
fn is_runner(name: &str) -> bool {
    matches!(
        name,
        "vitest" | "jest" | "mocha" | "jasmine" | "playwright" | "playwright-core"
    ) || ["@vitest/", "@jest/", "@playwright/", "vitest-"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// The directory of the installed package that owns `file`: the component after the last
/// `node_modules`, plus a second one for a scope (`@scope/name`).
fn package_root(file: &Path) -> Option<PathBuf> {
    let components: Vec<Component> = file.components().collect();
    let index = components
        .iter()
        .rposition(|component| component.as_os_str() == "node_modules")?;
    let name = components.get(index + 1)?;
    let length = if name.as_os_str().to_string_lossy().starts_with('@') {
        index + 3
    } else {
        index + 2
    };
    (components.len() >= length).then(|| components[..length].iter().collect())
}

/// The root of installed package `name`, searched from `from` the way Node does: a sibling in the
/// enclosing `node_modules`, then `node_modules` of every ancestor directory.
fn find_package(from: &Path, name: &str) -> Option<PathBuf> {
    // A manifest key is repository-controlled text: only `name` or `@scope/name` may reach a path.
    let segment =
        |text: &str| !text.is_empty() && text != "." && text != ".." && !text.contains(['\\', ':']);
    let shaped = match name.split('/').collect::<Vec<_>>().as_slice() {
        [package] => segment(package),
        [scope, package] => scope.starts_with('@') && segment(scope) && segment(package),
        _ => false,
    };
    if !shaped {
        return None;
    }
    from.ancestors().find_map(|directory| {
        let candidate = if directory.file_name().is_some_and(|n| n == "node_modules") {
            directory.join(name)
        } else {
            directory.join("node_modules").join(name)
        };
        candidate
            .join("package.json")
            .is_file()
            .then_some(candidate)
    })
}

/// Declared dependency names of one manifest: `(name, required)`. Optional dependencies and
/// peers marked optional are not required to be installed.
fn dependencies(manifest: &Value) -> Vec<(String, bool)> {
    let table = |field: &str| {
        manifest
            .get(field)
            .and_then(Value::as_object)
            .map(|object| object.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let optional_peer = |name: &str| {
        manifest
            .get("peerDependenciesMeta")
            .and_then(|meta| meta.get(name))
            .and_then(|entry| entry.get("optional"))
            .and_then(Value::as_bool)
            == Some(true)
    };
    let mut names: Vec<(String, bool)> = table("dependencies")
        .into_iter()
        .map(|name| (name, true))
        .collect();
    names.extend(
        table("peerDependencies")
            .into_iter()
            .map(|name| (name.clone(), !optional_peer(&name))),
    );
    names.extend(
        table("optionalDependencies")
            .into_iter()
            .map(|name| (name, false)),
    );
    names
}

/// Reads the dependency closure of the package at `root`.
fn closure_has_no_runner(root: &Path) -> bool {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(package) = pending.pop() {
        if !visited.insert(package.clone()) {
            continue;
        }
        if visited.len() > MAX_PACKAGES {
            return false;
        }
        let Some(manifest) = std::fs::read_to_string(package.join("package.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            return false;
        };
        for (name, required) in dependencies(&manifest) {
            if is_runner(&name) {
                return false;
            }
            match find_package(&package, &name) {
                Some(found) => pending.push(found),
                None if required => return false,
                None => {}
            }
        }
    }
    true
}

/// Largest entry file the text check reads; a larger one is "cannot tell".
const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;

/// Names a test file calls without importing them when the project enables Vitest globals.
const GLOBAL_REGISTRATION: &[&str] = &[
    "test",
    "it",
    "describe",
    "suite",
    "bench",
    "beforeAll",
    "beforeEach",
    "afterAll",
    "afterEach",
    "onTestFinished",
    "onTestFailed",
    "aroundEach",
    "aroundAll",
];

/// `text` with comments, string literals and regular-expression literals blanked, so prose ("don't
/// change it."), messages and quotes inside a pattern are not mistaken for code. A `/` starts a
/// regular expression where an operand is expected (after an operator, a bracket, `,`, `;`, or
/// `return`/`typeof`); elsewhere it divides. A pattern read as a division after a `)` (as in
/// `if (x) /re/.test(y)`) is the one shape this misses.
fn without_comments_and_strings(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    // The last character of code seen, and the word it ended, to tell a pattern from a division.
    let mut previous = ' ';
    let mut word = String::new();
    while let Some(c) = chars.next() {
        let operand_expected = matches!(
            previous,
            ' ' | '('
                | ','
                | '='
                | ':'
                | '['
                | '!'
                | '&'
                | '|'
                | '?'
                | '{'
                | '}'
                | ';'
                | '+'
                | '-'
                | '*'
                | '%'
                | '<'
                | '>'
                | '~'
                | '^'
        ) || matches!(word.as_str(), "return" | "typeof");
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = ' ';
                for next in chars.by_ref() {
                    if last == '*' && next == '/' {
                        break;
                    }
                    last = next;
                }
                out.push(' ');
            }
            '/' if operand_expected => {
                let mut in_class = false;
                while let Some(next) = chars.next() {
                    match next {
                        '\\' => {
                            chars.next();
                        }
                        '[' => in_class = true,
                        ']' => in_class = false,
                        '/' if !in_class => break,
                        '\n' => break,
                        _ => {}
                    }
                }
                out.push(' ');
                previous = ')';
                word.clear();
            }
            '`' => {
                // Template text is blanked, but an interpolation is code.
                let mut interpolation = 0usize;
                while let Some(next) = chars.next() {
                    if interpolation == 0 {
                        if next == '\\' {
                            chars.next();
                        } else if next == '`' {
                            break;
                        } else if next == '$' && chars.peek() == Some(&'{') {
                            chars.next();
                            interpolation = 1;
                            out.push(' ');
                        }
                    } else {
                        match next {
                            '{' => interpolation += 1,
                            '}' => {
                                interpolation -= 1;
                                if interpolation == 0 {
                                    out.push(' ');
                                    continue;
                                }
                            }
                            _ => {}
                        }
                        out.push(next);
                    }
                }
                out.push(' ');
            }
            '\'' | '"' => {
                while let Some(next) = chars.next() {
                    if next == '\\' {
                        chars.next();
                    } else if next == c {
                        break;
                    }
                }
                out.push(' ');
            }
            other => {
                out.push(other);
                if other.is_alphanumeric() || other == '_' || other == '$' {
                    word.push(other);
                } else if !other.is_whitespace() {
                    word.clear();
                }
                if !other.is_whitespace() {
                    previous = other;
                }
            }
        }
    }
    out
}

/// Whether the `(` at `open` starts the parameters of a function or method being defined rather
/// than a call: its matching `)` is followed by a block (`describe(path) { ... }`).
fn defines_function(code: &str, open: usize) -> bool {
    let mut depth = 0usize;
    for (offset, byte) in code.as_bytes()[open..].iter().take(4096).enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    // A block on the next line after a call is a statement, not a body.
                    let rest = &code[open + offset + 1..];
                    let gap = rest.len() - rest.trim_start_matches([' ', '\t']).len();
                    return rest[gap..].starts_with('{');
                }
            }
            _ => {}
        }
    }
    false
}

/// Whether `text` calls (or chains from) a registration global as a free name: the name stands
/// alone, not part of a longer identifier and not a property (`pattern.test(...)`), and is
/// followed by `(`, or by `.` and a Vitest modifier (`describe.each(...)`). Textual and deliberately
/// generous: a local function that happens to be called `test` is a false alarm, which only costs
/// the proof.
fn calls_registration_global(text: &str) -> bool {
    const MODIFIERS: &[&str] = &[
        "each",
        "skip",
        "only",
        "todo",
        "concurrent",
        "sequential",
        "runIf",
        "skipIf",
        "extend",
        "fails",
        "for",
        "shuffle",
    ];
    let code = without_comments_and_strings(text);
    let bytes = code.as_bytes();
    let is_name = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$';
    GLOBAL_REGISTRATION.iter().any(|name| {
        code.match_indices(name).any(|(start, _)| {
            let end = start + name.len();
            let before = start.checked_sub(1).map(|index| bytes[index]);
            if !before.is_none_or(|byte| !is_name(byte) && byte != b'.') {
                return false;
            }
            let after = code[end..].trim_start_matches([' ', '\t']);
            let end = code.len() - after.len();
            match bytes.get(end) {
                Some(b'(') => !defines_function(&code, end),
                Some(b'.') => MODIFIERS.iter().any(|modifier| {
                    code[end + 1..].starts_with(modifier)
                        && !code
                            .as_bytes()
                            .get(end + 1 + modifier.len())
                            .is_some_and(|b| is_name(*b))
                }),
                _ => false,
            }
        })
    })
}

/// Whether the entry file names Vitest anywhere in its text, or -- when the project enables
/// globals, which a package can lean on without importing anything -- calls a registration
/// global. A package that registers tests without declaring Vitest as a dependency is caught here
/// when it does so from the file imported; code split across other files of an undeclared package
/// is the residual case the manifest closure cannot see.
fn entry_registers(file: &Path, globals: bool) -> bool {
    let key = (file.to_path_buf(), globals);
    if let Some(known) = ENTRIES.with(|entries| entries.borrow().get(&key).copied()) {
        return known;
    }
    let registers = std::fs::metadata(file)
        .ok()
        .filter(|metadata| metadata.len() <= MAX_ENTRY_BYTES)
        .and_then(|_| std::fs::read(file).ok())
        .is_none_or(|bytes| {
            let text = String::from_utf8_lossy(&bytes);
            text.contains("vitest") || (globals && calls_registration_global(&text))
        });
    ENTRIES.with(|entries| entries.borrow_mut().insert(key, registers));
    registers
}

/// Whether the installed package that owns `file` provably cannot register Vitest tests: no
/// runner anywhere in its manifest closure, and none named in the file imported. `globals` is
/// whether the project enables Vitest globals.
pub(crate) fn cannot_register_tests(file: &Path, globals: bool) -> bool {
    let Some(root) = package_root(file) else {
        return false;
    };
    let closure = VERDICTS.with(|verdicts| verdicts.borrow().get(&root).copied());
    let closure = closure.unwrap_or_else(|| {
        let verdict = closure_has_no_runner(&root);
        VERDICTS.with(|verdicts| verdicts.borrow_mut().insert(root, verdict));
        verdict
    });
    closure && !entry_registers(file, globals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Writes `node_modules/<name>/package.json` under `root` and returns the package directory.
    fn install(root: &Path, name: &str, manifest: &str) -> PathBuf {
        let directory = root.join("node_modules").join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("package.json"), manifest).unwrap();
        fs::write(directory.join("index.js"), "").unwrap();
        directory
    }

    fn verdict(package: &Path) -> bool {
        VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
        ENTRIES.with(|entries| entries.borrow_mut().clear());
        cannot_register_tests(&package.join("index.js"), false)
    }

    /// A package with no runner anywhere in its closure is admitted; one that names a runner
    /// directly, as a peer, or through a dependency is not.
    #[test]
    fn a_runner_anywhere_in_the_closure_refuses_the_package() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let clean = install(root, "clean", r#"{"dependencies":{"leaf":"1"}}"#);
        install(root, "leaf", r#"{"name":"leaf"}"#);
        assert!(verdict(&clean));

        let direct = install(root, "direct", r#"{"dependencies":{"vitest":"1"}}"#);
        assert!(!verdict(&direct));
        let peer = install(root, "peer", r#"{"peerDependencies":{"vitest":"*"}}"#);
        assert!(!verdict(&peer));
        let scoped = install(root, "scoped", r#"{"dependencies":{"@vitest/spy":"1"}}"#);
        assert!(!verdict(&scoped));
        let indirect = install(root, "indirect", r#"{"dependencies":{"wrapper":"1"}}"#);
        install(root, "wrapper", r#"{"peerDependencies":{"vitest":"*"}}"#);
        assert!(!verdict(&indirect));
    }

    /// Anything unreadable is "cannot tell": a missing required dependency, a broken manifest,
    /// a file outside any package. A missing optional dependency is not.
    #[test]
    fn what_cannot_be_read_is_refused_and_optional_gaps_are_not() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let missing = install(root, "missing", r#"{"dependencies":{"absent":"1"}}"#);
        assert!(!verdict(&missing));
        let broken = install(root, "broken", "{ not json");
        assert!(!verdict(&broken));
        let optional = install(
            root,
            "optional",
            r#"{"optionalDependencies":{"absent":"1"},"peerDependencies":{"gone":"1"},
                "peerDependenciesMeta":{"gone":{"optional":true}}}"#,
        );
        assert!(verdict(&optional));
        assert!(!cannot_register_tests(&root.join("src/app.ts"), false));
    }

    /// Scoped packages are rooted at `@scope/name`, and a package that is its own dependency
    /// cycle terminates.
    #[test]
    fn scopes_and_cycles_are_handled() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let scoped = install(root, "@org/tool", r#"{"dependencies":{"@org/tool":"1"}}"#);
        assert!(verdict(&scoped));
        assert_eq!(package_root(&scoped.join("dist/index.js")).unwrap(), scoped);
    }

    /// A package that names Vitest in the file imported is refused whatever its manifest says.
    #[test]
    fn an_entry_that_names_vitest_is_refused_despite_a_clean_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "undeclared", r#"{"name":"undeclared"}"#);
        assert!(verdict(&package));
        fs::write(
            package.join("index.js"),
            "import { test } from 'vitest'\nexport const run = () => test('a', () => {})",
        )
        .unwrap();
        assert!(!verdict(&package));
        assert!(!cannot_register_tests(&package.join("missing.js"), false));
    }

    /// Under globals a package can register without importing anything; a registration global called
    /// as a free name in the entry refuses it, a property of that name does not.
    #[test]
    fn an_entry_that_calls_a_registration_global_is_refused_under_globals() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "lean", r#"{"name":"lean"}"#);
        for (source, registers) in [
            ("export const run = () => test('a', () => {})", true),
            (
                "export const run = () => describe.each([1])('a', () => {})",
                true,
            ),
            ("export const run = () => { beforeEach(() => {}) }", true),
            ("export const run = (re) => re.test('a')", false),
            (
                "export const attest = () => 1; export const latest = (x) => x",
                false,
            ),
            ("export const run = () => detest(1)", false),
            (
                "// do not change it.\n/* test( */ export const run = () => 'describe(x)'",
                false,
            ),
            ("export const run = () => it.skip('a', () => {})", true),
            ("export const items = (it) => it.map((x) => x)", false),
            ("export default { describe(path) { return path } }", false),
            (
                "function test(value) { return value }\nexport const run = () => 1",
                false,
            ),
            ("test('a', () => {})\n{ const x = 1 }", true),
            ("exports.run = () => test ('hidden', () => {})", true),
            ("exports.run = () => `${test('hidden', () => {})}`", true),
            ("exports.run = () => `plain test('x') text`", false),
            (
                "const q = /'/; exports.run = () => test('x', () => {})",
                true,
            ),
            (
                "const q = /[\"'`]/; exports.run = () => describe('x', () => {})",
                true,
            ),
            ("exports.run = (a, b) => a / 2 + b / 3; /* it( */", false),
            ("exports.run = () => { return /it\\(/.test('x') }", false),
        ] {
            fs::write(package.join("index.js"), source).unwrap();
            ENTRIES.with(|entries| entries.borrow_mut().clear());
            VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
            assert_eq!(
                cannot_register_tests(&package.join("index.js"), true),
                !registers,
                "{source}"
            );
            assert!(
                cannot_register_tests(&package.join("index.js"), false),
                "{source}"
            );
        }
    }

    /// A dependency key is text from the repository: one that is not a package name never reaches
    /// the file system, and counts as a dependency that is not installed.
    #[test]
    fn dependency_names_that_are_paths_are_not_followed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        install(root, "elsewhere", r#"{"name":"elsewhere"}"#);
        // A clean manifest outside `node_modules`, which only a traversing key would reach.
        fs::create_dir_all(root.join("outside")).unwrap();
        fs::write(root.join("outside/package.json"), r#"{"name":"outside"}"#).unwrap();
        for key in [
            "../outside",
            "../elsewhere",
            "/etc/passwd",
            "a/b/c",
            "@scope",
            "..",
            "C:\\x",
            "a\\b",
        ] {
            let package = install(
                root,
                "holder",
                &format!(r#"{{"dependencies":{{"{key}":"1"}}}}"#),
            );
            assert!(!verdict(&package), "{key}");
        }
        let package = install(root, "good", r#"{"dependencies":{"elsewhere":"1"}}"#);
        assert!(verdict(&package));
    }

    /// A closure past the cap is refused instead of read without bound.
    #[test]
    fn a_closure_past_the_cap_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let names: Vec<String> = (0..=MAX_PACKAGES).map(|n| format!("p{n}")).collect();
        let table: Vec<String> = names.iter().map(|n| format!("\"{n}\":\"1\"")).collect();
        let hub = install(
            root,
            "hub",
            &format!("{{\"dependencies\":{{{}}}}}", table.join(",")),
        );
        for name in &names {
            install(root, name, r#"{"name":"leaf"}"#);
        }
        assert!(!verdict(&hub));
    }
}
