use super::*;
use std::path::PathBuf;

/// Extracts `source` as a TypeScript test file that imports its runner explicitly.
fn extract_ts(source: &str) -> FileExtraction {
    let file = SourceFile::without_project_context(
        PathBuf::from("budget.test.ts"),
        crate::discovery::SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    extract(source, &file, Globals::Disabled)
}

/// Runs `body` with both emission limits replaced by `limit`, restoring them afterwards.
fn with_limit<T>(limit: usize, body: impl FnOnce() -> T) -> T {
    struct Restore;
    impl Drop for Restore {
        /// Clears the override even when `body` panics, so later tests see the real limits.
        fn drop(&mut self) {
            imported::EMISSION_LIMIT_OVERRIDE.with(|override_| override_.set(None));
        }
    }
    imported::EMISSION_LIMIT_OVERRIDE.with(|override_| override_.set(Some(limit)));
    let _restore = Restore;
    body()
}

/// Whether any diagnostic contains `needle`.
fn reports(extraction: &FileExtraction, needle: &str) -> bool {
    extraction
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains(needle))
}

/// An `.each` table of exactly the declaration limit is complete; one row more emits the limit
/// and says so. Under a small limit: a table of ten thousand rows exceeds the raw syntax budget
/// and is refused before parsing.
#[test]
fn an_each_table_past_the_declaration_limit_is_truncated_visibly() {
    let table = |rows: usize| {
        let rows: Vec<String> = (0..rows).map(|row| format!("[{row}]")).collect();
        format!(
            "import {{ test, expect }} from 'vitest';\ntest.each([{}])('row %i', (n) => {{ expect(n).toBe(n); }});\n",
            rows.join(",")
        )
    };
    let at = with_limit(5, || extract_ts(&table(5)));
    assert_eq!(at.tests.len(), 5);
    assert!(!at.incomplete, "{:?}", at.diagnostics);

    let past = with_limit(5, || extract_ts(&table(6)));
    assert_eq!(past.tests.len(), 5);
    assert!(past.incomplete);
    assert!(
        reports(&past, "declares more than"),
        "{:?}",
        past.diagnostics
    );
}

/// A truncated extraction is exactly the start of the full one, whatever the limit: emission
/// order is walk order and a suite record precedes its children, so a cut never alters an
/// admitted declaration.
#[test]
fn a_truncated_extraction_is_a_prefix_of_the_full_one() {
    let source = "import { describe, test, expect } from 'vitest';\n\
        test('plain', () => { expect(1).toBe(1); });\n\
        describe('suite', () => {\n\
          test.each([[1], [2], [3]])('row %i', (n) => { expect(n).toBeGreaterThan(0); });\n\
          for (const a of [1, 2]) { test(`loop ${a}`, () => { expect(a).toBe(a); }); }\n\
        });\n\
        describe.each([[1], [2]])('group %i', (g) => { test('inner', () => { expect(g).toBe(g); }); });\n\
        test('tail', () => { expect(2).toBe(2); });\n";
    let full = extract_ts(source);
    assert_eq!(full.tests.len(), 12, "{:?}", full.diagnostics);
    assert!(!full.incomplete, "{:?}", full.diagnostics);
    for limit in 0..=full.tests.len() {
        let cut = with_limit(limit, || extract_ts(source));
        assert_eq!(cut.tests, full.tests[..limit], "limit {limit}");
        assert_eq!(cut.incomplete, limit < full.tests.len(), "limit {limit}");
        if limit < full.tests.len() {
            assert!(reports(&cut, "declares more than"), "{:?}", cut.diagnostics);
        }
    }
}

/// Loop bodies that declare nothing still cost a walk per iteration, so iterations have their own
/// limit: a loop of exactly the limit is complete; one element more is refused visibly, and the
/// declaration after it is not emitted, keeping the truncation a prefix.
#[test]
fn a_loop_past_the_iteration_limit_is_refused_visibly() {
    let source = |elements: &str| {
        format!(
            "import {{ test, expect }} from 'vitest';\nfor (const a of [{elements}]) {{ const b = a; }}\ntest('after', () => {{ expect(1).toBe(1); }});\n"
        )
    };
    let at = with_limit(3, || extract_ts(&source("1, 2, 3")));
    assert_eq!(at.tests.len(), 1, "{:?}", at.diagnostics);
    assert!(!at.incomplete, "{:?}", at.diagnostics);

    let past = with_limit(3, || extract_ts(&source("1, 2, 3, 4")));
    assert!(past.tests.is_empty(), "{:?}", past.tests);
    assert!(past.incomplete);
    assert!(
        reports(&past, "unrolls more than"),
        "{:?}",
        past.diagnostics
    );
}

/// Three nested 40-element loops around one `test` would be 64,000 declarations from a few hundred
/// bytes; at the real limits the walk stops well short and names the cause.
#[test]
fn nested_literal_loops_stop_at_the_real_limits() {
    let elements: Vec<String> = (0..40).map(|element| element.to_string()).collect();
    let elements = elements.join(", ");
    let source = format!(
        "import {{ test, expect }} from 'vitest';\n\
         for (const a of [{elements}]) {{ for (const b of [{elements}]) {{ for (const c of [{elements}]) {{\n\
           test(`case ${{a}} ${{b}} ${{c}}`, () => {{ expect(a + b + c).toBeGreaterThan(-1); }});\n\
         }} }} }}\n"
    );
    let extraction = extract_ts(&source);
    assert!(
        !extraction.tests.is_empty() && extraction.tests.len() <= MAX_DECLARATIONS_PER_FILE,
        "{} declarations",
        extraction.tests.len()
    );
    assert!(extraction.incomplete);
    assert!(
        reports(&extraction, "unrolls more than"),
        "{:?}",
        extraction.diagnostics
    );
}

/// An imported runner that crosses the limit contributes its admitted prefix: the caller's cases
/// are the start of the full run, nothing is marked unresolved, and the cause is the budget rather
/// than a failed runner.
#[test]
fn an_imported_runner_crossing_the_limit_keeps_its_prefix() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("runner.ts"),
        "import { test, expect } from 'vitest'; export function run() { test('a', () => { expect(3).toBe(3) }); test('b', () => { expect(3).toBe(3) }); }",
    )
    .unwrap();
    let path = directory.path().join("caller.test.ts");
    let source = "import { test } from 'vitest'; import { run } from './runner';\nrun();\nrun();\n";
    std::fs::write(&path, source).unwrap();
    let file = SourceFile::without_project_context(
        path,
        crate::discovery::SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );

    let full = extract(source, &file, Globals::Disabled);
    assert_eq!(full.tests.len(), 4, "{:?}", full.diagnostics);
    assert!(!full.incomplete, "{:?}", full.diagnostics);

    let cut = with_limit(3, || extract(source, &file, Globals::Disabled));
    assert_eq!(cut.tests, full.tests[..3]);
    assert!(cut.tests.iter().all(|case| !case.context.unresolved));
    assert!(reports(&cut, "declares more than"), "{:?}", cut.diagnostics);
    assert!(
        !reports(&cut, "imported runner"),
        "the budget, not the runner, is the cause: {:?}",
        cut.diagnostics
    );
}
