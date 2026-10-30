use super::vitest::{self, DIALECT};
use super::{current, Chain, HookTiming, Registers, TableRows};
use std::collections::HashSet;
use std::path::Path;

/// Framework-neutral source files and directories: no string literal in their production code
/// may be a name the dialect declares. Unit tests (`tests.rs`, `tests/`) are exempt: they spell the
/// API in the sources they analyze. New neutral modules join this list as they land.
const NEUTRAL: &[&str] = &[
    "src/framework.rs",
    "src/lang.rs",
    "src/lang",
    "src/inventory.rs",
    "src/inventory",
    "src/normal.rs",
    "src/normal",
    "src/corpus.rs",
    "src/corpus",
];

/// Literals a neutral file may spell although a dialect declares them, because they mean something
/// else there: each is JavaScript vocabulary that collides with an API name.
const COLLISIONS: &[(&str, &str)] = &[
    // TypeScript's `override` modifier on a class member, dropped from the normal form.
    ("src/normal/form.rs", "override"),
    // The `for` keyword, which the parser preflight counts as recursive syntax.
    ("src/lang/limits.rs", "for"),
    // ESTree's field for the condition of an `if`.
    ("src/inventory/module.rs", "test"),
];

/// The string literals in `source`, with `//` comments removed first so prose may name the API.
fn string_literals(source: &str) -> Vec<String> {
    let mut literals = Vec::new();
    for line in source.lines() {
        let mut chars = line.chars().peekable();
        let mut current: Option<String> = None;
        while let Some(c) = chars.next() {
            match (&mut current, c) {
                (None, '/') if chars.peek() == Some(&'/') => break,
                (None, '"') => current = Some(String::new()),
                (Some(text), '\\') => {
                    if let Some(escaped) = chars.next() {
                        text.push(escaped);
                    }
                }
                (Some(_), '"') => literals.extend(current.take()),
                (Some(text), c) => text.push(c),
                (None, _) => {}
            }
        }
    }
    literals
}

/// Every `.rs` file a `NEUTRAL` entry names, recursively for directories.
fn neutral_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut pending: Vec<_> = NEUTRAL.iter().map(|entry| root.join(entry)).collect();
    while let Some(path) = pending.pop() {
        assert!(
            path.exists(),
            "neutral entry {} does not exist",
            path.display()
        );
        if path.is_dir() {
            for entry in std::fs::read_dir(&path).unwrap() {
                pending.push(entry.unwrap().path());
            }
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && !is_unit_test(path.strip_prefix(root).unwrap_or(&path))
        {
            files.push(path);
        }
    }
    files
}

/// Whether `path`, relative to the repository, holds unit tests rather than production code.
fn is_unit_test(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "tests.rs")
        || path
            .components()
            .any(|component| component.as_os_str() == "tests")
}

/// The names in `literals` that `names` declares.
fn violations<'a>(literals: &'a [String], names: &HashSet<&str>) -> Vec<&'a str> {
    literals
        .iter()
        .map(String::as_str)
        .filter(|literal| names.contains(literal))
        .collect()
}

#[test]
fn neutral_modules_spell_no_api_name() {
    let names: HashSet<&str> = DIALECT.names().collect();
    assert!(
        names.len() >= 30,
        "the dialect declares only {} names",
        names.len()
    );
    let files = neutral_files(Path::new(env!("CARGO_MANIFEST_DIR")));
    for (file, literal) in COLLISIONS {
        assert!(
            names.contains(literal),
            "{literal} is no longer an API name; drop its exemption"
        );
        let source =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap();
        assert!(
            string_literals(&source)
                .iter()
                .any(|found| found == literal),
            "{file} no longer spells {literal}; drop its exemption"
        );
    }
    assert!(
        files.len() >= NEUTRAL.len(),
        "only {} neutral files were scanned",
        files.len()
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for file in files {
        let literals = string_literals(&std::fs::read_to_string(&file).unwrap());
        let relative = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let mut found = violations(&literals, &names);
        found.retain(|literal| !COLLISIONS.contains(&(relative.as_str(), *literal)));
        assert!(
            found.is_empty(),
            "{} spells API names {found:?}; read them from the dialect",
            file.display()
        );
    }
}

#[test]
fn the_boundary_check_rejects_a_planted_api_name() {
    let names: HashSet<&str> = DIALECT.names().collect();
    let planted = string_literals("let kind = \"beforeEach\"; // \"skip\" in a comment is prose");
    assert_eq!(violations(&planted, &names), ["beforeEach"]);
    let escaped = string_literals(r#"let text = "say \"hi\""; let other = "describe";"#);
    assert_eq!(escaped, ["say \"hi\"", "describe"]);
    assert_eq!(violations(&escaped, &names), ["describe"]);
    assert!(is_unit_test(Path::new("src/lang/tests.rs")));
    assert!(is_unit_test(Path::new(
        "src/framework/vitest/projects/tests/glob_budget.rs"
    )));
    assert!(!is_unit_test(Path::new("src/lang/tree.rs")));
}

/// Where the repository is checked out does not decide which of its files are unit tests.
#[test]
fn a_checkout_under_a_tests_directory_keeps_its_neutral_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tests").join("checkout");
    for entry in NEUTRAL {
        let path = root.join(entry);
        if entry.ends_with(".rs") {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        } else {
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("code.rs"), "").unwrap();
            std::fs::write(path.join("tests.rs"), "").unwrap();
        }
    }
    let files = neutral_files(&root);
    assert_eq!(files.len(), NEUTRAL.len());
    assert!(files.iter().all(|file| !file.ends_with("tests.rs")));
}

#[test]
fn the_vitest_dialect_declares_the_documented_api() {
    let registrations: Vec<_> = DIALECT
        .registrations
        .iter()
        .map(|registration| registration.name)
        .collect();
    assert_eq!(
        registrations,
        [
            "test",
            "it",
            "describe",
            "suite",
            "bench",
            "beforeAll",
            "beforeEach",
            "afterEach",
            "afterAll",
            "aroundEach",
            "aroundAll",
            "onTestFinished",
            "onTestFailed",
        ]
    );
    let chain: Vec<_> = DIALECT.chain.iter().map(|member| member.name).collect();
    assert_eq!(
        chain,
        [
            "skip",
            "only",
            "todo",
            "fails",
            "concurrent",
            "sequential",
            "shuffle",
            "skipIf",
            "runIf",
            "each",
            "for",
            "extend",
            "override",
            "scoped",
        ]
    );
    assert_eq!(DIALECT.scheduling_options, ["timeout", "retry", "repeats"]);
    assert_eq!(DIALECT.name_observing_matchers, ["Snapshot"]);
    assert_eq!(DIALECT.api_modules, ["vitest"]);
    assert_eq!(DIALECT.in_source_member, Some("vitest"));
}

#[test]
fn every_name_is_declared_once() {
    let mut seen = HashSet::new();
    for name in DIALECT
        .registrations
        .iter()
        .map(|registration| registration.name)
        .chain(DIALECT.namespaces.iter().map(|namespace| namespace.name))
        .chain(DIALECT.chain.iter().map(|member| member.name))
    {
        assert!(seen.insert(name), "{name} is declared twice");
    }
}

#[test]
fn aliases_share_what_they_register() {
    for registration in DIALECT.registrations {
        let canonical = DIALECT
            .registration(registration.canonical)
            .unwrap_or_else(|| panic!("{} has no canonical entry", registration.name));
        assert_eq!(
            canonical.registers, registration.registers,
            "{}",
            registration.name
        );
        assert_eq!(
            canonical.canonical, canonical.name,
            "{} is canonical for itself",
            canonical.name
        );
    }
    assert_eq!(
        DIALECT
            .registration("it")
            .map(|registration| registration.canonical),
        Some("test")
    );
    assert_eq!(
        DIALECT
            .registration("suite")
            .map(|registration| registration.canonical),
        Some("describe")
    );
    assert_eq!(DIALECT.namespace("vitest"), Some("vi"));
}

#[test]
fn lookups_answer_by_meaning() {
    assert_eq!(DIALECT.hook("aroundEach"), Some(HookTiming::AroundEach));
    assert_eq!(DIALECT.hook("test"), None);
    assert_eq!(DIALECT.hook("unknown"), None);
    assert_eq!(
        DIALECT
            .registration("bench")
            .map(|registration| registration.registers),
        Some(Registers::Bench)
    );
    assert_eq!(
        DIALECT.chain_member("each"),
        Some(Chain::Table(TableRows::Spread))
    );
    assert_eq!(
        DIALECT.chain_member("for"),
        Some(Chain::Table(TableRows::Whole))
    );
    assert_eq!(DIALECT.chain_member("only"), Some(Chain::Focus));
    assert_eq!(DIALECT.chain_member("expect"), None);
    assert_eq!(DIALECT.namespace("test"), None);
    assert!(DIALECT.is_api_module("vitest"));
    assert!(!DIALECT.is_api_module("vitest/config"));
}

#[test]
fn the_current_framework_is_vitest_with_its_documented_defaults() {
    assert!(std::ptr::eq(current().dialect(), &DIALECT));
    let defaults = current().file_defaults();
    assert_eq!(defaults.include, vitest::DEFAULT_INCLUDE_GLOBS);
    assert_eq!(
        defaults.typecheck_include,
        vitest::DEFAULT_TYPECHECK_INCLUDE_GLOBS
    );
    assert_eq!(
        defaults.benchmark_include,
        vitest::DEFAULT_BENCHMARK_INCLUDE_GLOBS
    );
    assert_eq!(
        defaults.benchmark_exclude,
        vitest::DEFAULT_BENCHMARK_EXCLUDE_GLOBS
    );
    assert_eq!(defaults.exclude, vitest::DEFAULT_EXCLUDE_GLOBS);
    assert!(defaults.include.contains(&"**/*.test.ts"));
    assert_eq!(
        crate::discovery::DEFAULT_INCLUDE_GLOBS,
        vitest::DEFAULT_INCLUDE_GLOBS
    );
}
