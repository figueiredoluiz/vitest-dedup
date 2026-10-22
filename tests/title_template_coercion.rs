//! A template-literal title interpolating a composite coerces the way JavaScript's `ToString`
//! coerces it, so the runtime title -- not the internal canonical fold -- is what the analyzer
//! records and compares.

use vitest_dedup::analysis::analyze;
use vitest_dedup::config::{Config, ConfigOverrides};
use vitest_dedup::discovery::{ModuleFlavor, SourceFile, SourceLanguage};
use vitest_dedup::frontend::{extract, Globals};
use vitest_dedup::model::{IncompleteCauses, Rule, Severity};

/// The fixture extracted and analyzed, with title texts asserted first: the coerced template
/// title and its literal twin must be byte-identical before any rule reads them.
#[test]
fn composite_title_substitutions_coerce_as_javascript() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let input = SourceFile::without_project_context(
        directory.path().join("case.test.ts"),
        SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    let extracted = extract(
        include_str!("../fixtures/duplicates/title-template-composite-coercion.test.ts"),
        &input,
        Globals::Disabled,
    );
    assert_eq!(extracted.tests.len(), 4);
    assert!(!extracted.incomplete, "{:?}", extracted.diagnostics);
    assert!(extracted
        .tests
        .iter()
        .all(|test| test.context.is_comparable()));
    let titles: Vec<&str> = extracted
        .tests
        .iter()
        .map(|test| test.title.display())
        .collect();
    assert_eq!(
        titles,
        vec![
            "obj [object Object]",
            "obj [object Object]",
            "arr 1,2",
            "arr 1,2",
        ],
        "a template title is the string JavaScript builds, never the canonical fold"
    );

    let result = analyze(
        extracted.tests.clone(),
        &config,
        1,
        IncompleteCauses::default(),
    )
    .unwrap();
    assert_eq!(result.unresolved_tests, 0);
    assert_eq!(
        result.findings.len(),
        2,
        "each colliding-title pair is one duplicate-test-name finding: {:?}",
        result.findings
    );
    assert!(result
        .findings
        .iter()
        .all(|finding| finding.rule == Rule::DuplicateTestName
            && finding.severity == Severity::Warning));
}

/// The refusal half: a composite whose coercion is interceptable has no provable title text, so
/// the title degrades to `Title::Dynamic` and the test leaves comparison rather than guessing.
#[test]
fn interceptable_coercion_leaves_the_title_dynamic() {
    let directory = tempfile::tempdir().unwrap();
    let input = SourceFile::without_project_context(
        directory.path().join("case.test.ts"),
        SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    let source = "import { test, expect } from 'vitest';\n\
                  test(`x ${{ toString: () => 'y' }}`, () => { expect(1).toBe(1); });\n\
                  test('x y', () => { expect(1).toBe(1); });\n";
    let extracted = extract(source, &input, Globals::Disabled);
    assert_eq!(extracted.tests.len(), 2);
    let dynamic = extracted
        .tests
        .iter()
        .find(|test| test.title.display() == "x y")
        .expect("the literal title extracts statically");
    assert!(
        !extracted
            .tests
            .iter()
            .any(|test| test.title.display() == "x y y"
                || test.title.display().contains("[object Object]")),
        "the interceptable coercion must not be guessed"
    );
    assert!(
        extracted
            .tests
            .iter()
            .any(|test| !test.context.is_comparable()),
        "a dynamic title marks its test unresolved"
    );
    assert!(dynamic.context.is_comparable());
}

/// A composite whose coercion is interceptable is still a KNOWN value: a loop over such
/// elements keeps unrolling one declaration per element, and only a title that interpolates
/// the element goes dynamic. Requiring the coercion to fold would collapse the loop to a
/// single walk and silently drop half the runtime declarations. The two iterations share one
/// declaration site, so no rule reports the pair -- loop iterations are deliberately not
/// compared, as `observed-loop-unrolled-declarations` pins elsewhere -- which makes the
/// extraction count the whole regression.
#[test]
fn interceptable_coercion_still_enumerates_its_loop() {
    let directory = tempfile::tempdir().unwrap();
    let config =
        vitest_dedup::config::Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let input = SourceFile::without_project_context(
        directory.path().join("case.test.ts"),
        SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    let source = "import { test, expect } from 'vitest';\n\
                  for (const row of [{ toString: 1 }, { toString: 2 }]) {\n\
                  test('literal', () => { expect(1).toBe(1); });\n\
                  }\n";
    let extracted = extract(source, &input, Globals::Disabled);
    assert_eq!(
        extracted.tests.len(),
        2,
        "the loop unrolls per element however the element stringifies: {:?}",
        extracted.tests
    );
    assert_eq!(extracted.tests[0].title.display(), "literal");
    let result = analyze(
        extracted.tests.clone(),
        &config,
        1,
        IncompleteCauses::default(),
    )
    .unwrap();
    assert!(
        result.findings.is_empty(),
        "loop iterations share a declaration site and are never reported as a pair: {:?}",
        result.findings
    );
}

/// A helper parameter bound to a plain composite interpolates its coerced text into the titles
/// the helper declares, exactly as a loop binding does: `[object Object]`, never the canonical
/// fold.
#[test]
fn helper_parameter_titles_coerce_composites() {
    let directory = tempfile::tempdir().unwrap();
    let input = SourceFile::without_project_context(
        directory.path().join("case.test.ts"),
        SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    let source = "import { test, expect } from 'vitest';\n\
                  function declare(label: unknown) {\n\
                  test(`ctx ${label}`, () => { expect(1).toBe(1); });\n\
                  }\n\
                  declare({ a: 1 });\n";
    let extracted = extract(source, &input, Globals::Disabled);
    assert_eq!(extracted.tests.len(), 1);
    assert_eq!(
        extracted.tests[0].title.display(),
        "ctx [object Object]",
        "a bound composite title coerces, never renders the canonical fold"
    );
}
