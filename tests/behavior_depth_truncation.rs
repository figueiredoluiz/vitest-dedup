//! A body whose behavior trace the depth cap cut short must leave comparison entirely --
//! its trace compares equal to a different body's equally-truncated trace, and that equality is
//! an artifact of the cap, not a similarity claim.

use vitest_dedup::analysis::analyze;
use vitest_dedup::config::{Config, ConfigOverrides};
use vitest_dedup::discovery::{ModuleFlavor, SourceFile, SourceLanguage};
use vitest_dedup::frontend::{extract, Globals};
use vitest_dedup::model::{IncompleteCauses, Rule, Severity};

/// The deep pair is unresolved and silent; the shallow retention pair keeps its finding.
///
/// The fixture holds four tests: two whose constructors (`trace.push(1)` versus
/// `trace.unshift(2)`) sit past the behavior collector's depth cap under 260 unbraced `if`
/// statements, and the same construction at two levels of nesting where the effect is
/// collected. Under Node v26.0.0 / Vitest 5.0.5 the file runs 3 passed / 1 failed, which is
/// exactly the difference the truncated trace cannot see.
#[test]
fn behavior_depth_truncation_leaves_comparison_and_keeps_the_shallow_finding() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let input = SourceFile::without_project_context(
        directory.path().join("case.test.ts"),
        SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    let extracted = extract(
        include_str!("../fixtures/duplicates/behavior-depth-truncation.test.ts"),
        &input,
        Globals::Disabled,
    );
    assert_eq!(extracted.tests.len(), 4);
    assert!(!extracted.incomplete, "{:?}", extracted.diagnostics);

    let by_title = |needle: &str| {
        extracted
            .tests
            .iter()
            .find(|test| test.title.display() == needle)
            .unwrap_or_else(|| panic!("no test titled {needle:?}"))
    };
    // The truncated bodies carry the unresolved flag, and only they do.
    for needle in ["deep push", "deep unshift"] {
        let test = by_title(needle);
        assert!(
            !test.context.is_comparable(),
            "{needle}: a depth-truncated trace must never be compared"
        );
        assert!(
            test.body.behavior.len() >= 2,
            "{needle}: the truncated trace still holds its recorded prefix"
        );
    }
    for needle in ["shallow sink", "shallow sink inert marker"] {
        let test = by_title(needle);
        assert!(
            test.context.is_comparable(),
            "{needle}: a body inside the cap stays comparable"
        );
    }

    // The defect itself, pinned: the two deep bodies collected *equal* traces despite
    // doing different work. Equality here is why they must be excluded rather than compared.
    let deep_push = by_title("deep push");
    let deep_unshift = by_title("deep unshift");
    assert_eq!(
        deep_push.body.behavior, deep_unshift.body.behavior,
        "the depth cap leaves both deep bodies the same truncated trace"
    );
    assert_ne!(
        deep_push.body.alpha, deep_unshift.body.alpha,
        "alpha keeps the raw-text fallback past the cap, so the bodies still differ"
    );
    // The shallow pair converges the way the rule intends: equal behavior, differing alpha.
    let shallow_a = by_title("shallow sink");
    let shallow_b = by_title("shallow sink inert marker");
    assert_eq!(shallow_a.body.behavior, shallow_b.body.behavior);
    assert_ne!(shallow_a.body.alpha, shallow_b.body.alpha);

    let result = analyze(
        extracted.tests.clone(),
        &config,
        1,
        IncompleteCauses::default(),
    )
    .unwrap();
    assert_eq!(result.tests_analyzed, 4);
    assert_eq!(result.unresolved_tests, 2);
    assert_eq!(
        result.findings.len(),
        1,
        "only the shallow retention pair may be reported: {:?}",
        result.findings
    );
    assert_eq!(result.findings[0].rule, Rule::NearDuplicateTest);
    assert_eq!(result.findings[0].severity, Severity::Warning);
}

/// A hook body past the depth cap taints its scope the same way: two contexts carrying
/// equally-truncated hook digests would compare as equivalent.
#[test]
fn behavior_depth_truncation_in_a_hook_leaves_its_scope_unresolved() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let input = SourceFile::without_project_context(
        directory.path().join("case.test.ts"),
        SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    // Unbraced `if`s, as in the fixture: bracketed nesting past 256 is refused by the parser
    // budget, and the refusal is not the mechanism under test.
    let deep = format!("{}expect(1).toBe(1);", "if (true) ".repeat(260));
    let source = format!(
        "import {{ test, expect, beforeEach }} from 'vitest';\n\
         beforeEach(() => {{ {deep} }});\n\
         test('under deep hook', () => {{ expect(1).toBe(1); }});\n\
         test('under deep hook twin', () => {{ expect(1).toBe(1); }});\n",
    );
    let extracted = extract(&source, &input, Globals::Disabled);
    assert_eq!(extracted.tests.len(), 2);
    assert!(
        extracted
            .tests
            .iter()
            .all(|test| !test.context.is_comparable()),
        "a hook whose body truncated must taint every test in its scope"
    );
    let result = analyze(
        extracted.tests.clone(),
        &config,
        1,
        IncompleteCauses::default(),
    )
    .unwrap();
    assert_eq!(result.unresolved_tests, 2);
    assert!(
        result.findings.is_empty(),
        "the identical twin bodies must not be called duplicates on a truncated hook: {:?}",
        result.findings
    );
}
