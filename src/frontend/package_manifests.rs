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
use std::sync::{Mutex, OnceLock};

/// Most packages one closure may visit before it is refused.
const MAX_PACKAGES: usize = 256;

thread_local! {
    /// Verdict per package root, so a barrel's many imports of one package read its closure once.
    static VERDICTS: RefCell<HashMap<PathBuf, bool>> = RefCell::new(HashMap::new());
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
                previous = ')';
                word.clear();
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
                previous = ')';
                word.clear();
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

/// Most local files one package's entry may reach before the text rule gives up.
const MAX_LOCAL_FILES: usize = 1024;
/// Most bytes those files may hold in all.
const MAX_LOCAL_BYTES: u64 = 32 * 1024 * 1024;

/// Verdicts of the text rule over what an entry reaches, per entry and globals mode, shared by
/// every worker: a package is read once per run, not once per test file that imports it.
fn reached_verdicts() -> &'static Mutex<HashMap<(PathBuf, bool), bool>> {
    static VERDICTS: OnceLock<Mutex<HashMap<(PathBuf, bool), bool>>> = OnceLock::new();
    VERDICTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Forgets every verdict of the text rule; for tests that rewrite a file and ask again.
#[cfg(test)]
pub(crate) fn forget_reached() {
    reached_verdicts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// The relative specifiers a script loads: `from './x'`, `require('./x')`, `import('./x')` and
/// `import './x'`. Read from the raw text, so a specifier inside a comment is a false lead, which
/// at worst names a file that does not exist.
fn local_specifiers(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let is_name = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$';
    let mut found = Vec::new();
    for keyword in ["from", "require", "import"] {
        for (start, _) in text.match_indices(keyword) {
            if start > 0 && (is_name(bytes[start - 1]) || bytes[start - 1] == b'.') {
                continue;
            }
            let mut at = start + keyword.len();
            while bytes
                .get(at)
                .is_some_and(|b| b.is_ascii_whitespace() || *b == b'(')
            {
                at += 1;
            }
            let Some(&quote) = bytes.get(at).filter(|b| matches!(**b, b'\'' | b'"')) else {
                continue;
            };
            let rest = &text[at + 1..];
            if let Some(end) = rest.find(quote as char) {
                let specifier = &rest[..end];
                if matches!(specifier, "." | "..")
                    || specifier.starts_with("./")
                    || specifier.starts_with("../")
                {
                    found.push(specifier.to_owned());
                }
            }
        }
    }
    found
}

/// `path` with `.` and `..` folded away, without touching the file system.
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// The script file a relative `specifier` written in `from` names, when it is inside `root`:
/// as written, with a script extension, or as a directory index.
fn resolve_local(from: &Path, root: &Path, specifier: &str) -> Option<PathBuf> {
    let base = normalized(&from.parent()?.join(specifier));
    if !base.starts_with(root) {
        return None;
    }
    let mut candidates = vec![base.clone()];
    for extension in ["js", "mjs", "cjs"] {
        let mut named = base.clone().into_os_string();
        named.push(format!(".{extension}"));
        candidates.push(PathBuf::from(named));
        candidates.push(base.join(format!("index.{extension}")));
    }
    // A link inside the package can point anywhere; only a script whose real path is inside the
    // package's real root is part of it.
    let real_root = root.canonicalize().ok()?;
    candidates.into_iter().find(|candidate| {
        candidate.is_file()
            && candidate
                .extension()
                .is_some_and(|extension| matches!(extension.to_str(), Some("js" | "mjs" | "cjs")))
            && candidate
                .canonicalize()
                .is_ok_and(|real| real.starts_with(&real_root))
    })
}

/// Whether the entry file, or a script of the same package it loads by a relative path, names
/// Vitest anywhere in its text, or -- when the project enables globals, which a package can lean
/// on without importing anything -- calls a registration global. A package that registers tests
/// without declaring Vitest as a dependency is caught here. Past the file and size caps, or at a
/// file too large to read, the answer is "cannot tell", which is the refusal.
fn entry_registers(file: &Path, globals: bool) -> bool {
    let key = (file.to_path_buf(), globals);
    if let Some(known) = reached_verdicts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .copied()
    {
        return known;
    }
    let registers = reached_files_register(file, globals);
    reached_verdicts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, registers);
    registers
}

fn reached_files_register(entry: &Path, globals: bool) -> bool {
    let Some(root) = package_root(entry) else {
        return true;
    };
    let mut pending = vec![entry.to_path_buf()];
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut bytes = 0u64;
    while let Some(file) = pending.pop() {
        if !seen.insert(file.clone()) {
            continue;
        }
        let Some(length) = std::fs::metadata(&file)
            .ok()
            .map(|metadata| metadata.len())
            .filter(|length| *length <= MAX_ENTRY_BYTES)
        else {
            return true;
        };
        bytes += length;
        if seen.len() > MAX_LOCAL_FILES || bytes > MAX_LOCAL_BYTES {
            return true;
        }
        let Ok(raw) = std::fs::read(&file) else {
            return true;
        };
        let text = String::from_utf8_lossy(&raw);
        if text.contains("vitest") || (globals && calls_registration_global(&text)) {
            return true;
        }
        pending.extend(
            local_specifiers(&text)
                .iter()
                .filter_map(|specifier| resolve_local(&file, &root, specifier)),
        );
    }
    false
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
        forget_reached();
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
            (
                "const half = \"a\" / 2; exports.run = () => test('x', () => {}); const t = 4 / 2",
                true,
            ),
            (
                "const half = `${n}px` / 2; exports.run = () => test('x', () => {}); const t = 4 / 2",
                true,
            ),
            ("exports.run = (a, b) => a / 2 + b / 3; /* it( */", false),
            ("exports.run = () => { return /it\\(/.test('x') }", false),
        ] {
            fs::write(package.join("index.js"), source).unwrap();
            forget_reached();
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

    /// A package that registers from a file other than the one imported is refused, however that
    /// file is reached; files outside the package and files that do not exist are not followed.
    #[test]
    fn local_files_an_entry_reaches_are_judged_too() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "split", r#"{"name":"split"}"#);
        let write = |name: &str, text: &str| {
            let path = package.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            "registrar.js",
            "const { test } = require('vitest'); exports.go = () => test('a', () => {})",
        );
        write("globals.js", "exports.go = () => describe('a', () => {})");
        write("lib/index.js", "exports.go = () => test('a', () => {})");
        write("clean.js", "exports.go = () => 1");

        fs::write(root.join("node_modules/outside.js"), "test('a', () => {})").unwrap();
        for (entry, globals, registers) in [
            ("exports.go = require('./registrar').go", false, true),
            ("module.exports = require('./registrar.js')", false, true),
            ("export * from './registrar'", false, true),
            ("export { go } from './registrar.js'", false, true),
            ("import('./registrar')", false, true),
            ("import './registrar'", false, true),
            ("exports.go = require('./globals').go", false, false),
            ("exports.go = require('./globals').go", true, true),
            ("exports.go = require('./lib').go", true, true),
            ("exports.go = require('./clean').go", true, false),
            ("exports.go = require('../outside').go", true, false),
            ("exports.go = require('./absent').go", true, false),
            ("exports.go = require('lodash').go", true, false),
            ("// require('./registrar')\nexports.go = 1", false, true),
        ] {
            fs::write(package.join("index.js"), entry).unwrap();
            forget_reached();
            VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
            assert_eq!(
                cannot_register_tests(&package.join("index.js"), globals),
                !registers,
                "{entry} (globals {globals})"
            );
        }
    }

    /// A link out of the package is not a file of the package.
    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_package_is_not_followed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "linked", r#"{"name":"linked"}"#);
        fs::write(
            root.join("outside.js"),
            "exports.go = () => test('a', () => {})",
        )
        .unwrap();
        std::os::unix::fs::symlink(root.join("outside.js"), package.join("registrar.js")).unwrap();
        fs::write(
            package.join("index.js"),
            "exports.go = require('./registrar').go",
        )
        .unwrap();
        forget_reached();
        VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
        assert!(cannot_register_tests(&package.join("index.js"), true));
    }

    /// `require('.')` and `require('..')` name a directory index.
    #[test]
    fn dot_specifiers_name_a_directory_index() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "dots", r#"{"name":"dots"}"#);
        fs::create_dir_all(package.join("sub")).unwrap();
        fs::write(
            package.join("index.js"),
            "exports.go = () => test('a', () => {})",
        )
        .unwrap();
        fs::write(
            package.join("sub/index.js"),
            "module.exports = require('..')",
        )
        .unwrap();
        fs::write(package.join("sub/self.js"), "module.exports = require('.')").unwrap();
        for entry in ["sub/index.js", "sub/self.js"] {
            forget_reached();
            VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
            assert!(
                !cannot_register_tests(&package.join(entry), true),
                "{entry}"
            );
        }
    }

    /// Reading follows cycles once, and gives up past the file cap rather than reading without bound.
    #[test]
    fn local_files_are_read_once_and_within_a_cap() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "ring", r#"{"name":"ring"}"#);
        fs::write(package.join("index.js"), "require('./a')").unwrap();
        fs::write(package.join("a.js"), "require('./b')").unwrap();
        fs::write(package.join("b.js"), "require('./a')").unwrap();
        forget_reached();
        VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
        assert!(cannot_register_tests(&package.join("index.js"), false));
        let wide = install(root, "wide", r#"{"name":"wide"}"#);
        let imports: Vec<String> = (0..=MAX_LOCAL_FILES)
            .map(|n| format!("require('./f{n}')"))
            .collect();
        fs::write(wide.join("index.js"), imports.join(";")).unwrap();
        for n in 0..=MAX_LOCAL_FILES {
            fs::write(wide.join(format!("f{n}.js")), "exports.x = 1").unwrap();
        }
        forget_reached();
        VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
        assert!(!cannot_register_tests(&wide.join("index.js"), false));
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
