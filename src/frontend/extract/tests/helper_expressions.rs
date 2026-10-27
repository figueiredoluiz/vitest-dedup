use super::*;

const INITIALIZER: &str =
    include_str!("../../../../fixtures/recall/helper-initializer-hook/run.test.ts");

/// Checks extraction eligibility, hook context and the final duplicate-test outcome together.
fn assert_outcome(source: &str, unresolved: usize, hooks: [usize; 2], duplicates: usize) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("run.test.ts");
    std::fs::write(&path, source).unwrap();
    let file = SourceFile::without_project_context(
        path,
        crate::discovery::SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    let extraction = extract(source, &file, Globals::Disabled);
    let tests: Vec<_> = extraction
        .tests
        .iter()
        .filter(|test| test.kind != DeclarationKind::Suite)
        .collect();
    assert_eq!(tests.len(), 2, "{source}");
    assert_eq!(
        tests
            .iter()
            .filter(|test| !test.context.is_comparable())
            .count(),
        unresolved,
        "{source}"
    );
    assert_eq!(
        extraction.incomplete,
        unresolved != 0,
        "{source}: {:?}",
        extraction.diagnostics
    );
    assert_eq!(
        [tests[0].context.hooks.len(), tests[1].context.hooks.len()],
        hooks,
        "{source}"
    );
    let config =
        crate::config::Config::load(directory.path(), crate::config::ConfigOverrides::default())
            .unwrap();
    let result = crate::analysis::analyze(
        extraction.tests,
        &config,
        1,
        crate::model::IncompleteCauses::default(),
    )
    .unwrap();
    assert_eq!(result.tests_analyzed, 2);
    assert_eq!(
        result
            .findings
            .iter()
            .filter(|finding| finding.rule == crate::model::Rule::DuplicateTest)
            .count(),
        duplicates,
        "{source}"
    );
}

/// The runtime oracle and this regression consume the same permanent corpus sources.
#[test]
fn helper_expression_corpus_outcomes() {
    for (source, unresolved, hooks, duplicates) in [
        (INITIALIZER, 0, [2, 1], 0),
        (
            include_str!(
                "../../../../fixtures/recall/helper-initializer-equivalent-hooks/run.test.ts"
            ),
            0,
            [2, 2],
            1,
        ),
        (
            include_str!("../../../../fixtures/recall/helper-initializer-test/run.test.ts"),
            0,
            [0, 0],
            1,
        ),
        (
            include_str!(
                "../../../../fixtures/recall/helper-inert-registration-callback/run.test.ts"
            ),
            0,
            [1, 1],
            1,
        ),
        (
            include_str!("../../../../fixtures/recall/helper-ignored-assignment-hook/run.test.ts"),
            1,
            [1, 1],
            0,
        ),
        (
            include_str!("../../../../fixtures/recall/helper-ignored-sequence-hook/run.test.ts"),
            1,
            [1, 1],
            0,
        ),
        (
            include_str!("../../../../fixtures/recall/helper-ignored-logical-hook/run.test.ts"),
            1,
            [1, 1],
            0,
        ),
        (
            include_str!("../../../../fixtures/recall/helper-ignored-conditional-hook/run.test.ts"),
            1,
            [1, 1],
            0,
        ),
    ] {
        assert_outcome(source, unresolved, hooks, duplicates);
    }
}

/// Factory creation and supported in-source bindings retain exact registration counts.
#[test]
fn helper_framework_values_do_not_register() {
    for source in [
        include_str!("../../../../fixtures/recall/helper-initializer-factories/run.test.ts"),
        include_str!("../../../../fixtures/recall/helper-in-source-bindings/run.test.ts"),
    ] {
        assert_outcome(source, 0, [0, 0], 1);
    }
    assert_outcome(
        include_str!(
            "../../../../fixtures/recall/helper-initializer-factory-argument-hook/run.test.ts"
        ),
        1,
        [1, 1],
        0,
    );
}

/// Callable spelling and an extra local call do not change when initializer hooks register.
#[test]
fn helper_initializer_call_variants() {
    for declaration in ["function setup() {", "const setup = function () {"] {
        assert_outcome(
            &INITIALIZER.replace("const setup = () => {", declaration),
            0,
            [2, 1],
            0,
        );
    }
    assert_outcome(
        &INITIALIZER
            .replace(
                "const setup = () => {",
                "const inner = () => beforeEach(() => { state.value = 1 });\nconst setup = () => {",
            )
            .replace("beforeEach(() => { state.value = 1 })\n}", "inner()\n}"),
        0,
        [2, 1],
        0,
    );
}

/// Wrapped expressions and destructuring can register even when their value is discarded.
#[test]
fn helper_unwalked_expression_variants_are_refused() {
    let original = "const stop = beforeEach(() => { state.value = 1 })";
    for expression in [
        "return void beforeEach(() => { state.value = 1 })",
        "const stop = (beforeEach(() => { state.value = 1 }), 0)",
        "const { value = beforeEach(() => { state.value = 1 }) } = {}",
        "const { value } = { get value() { beforeEach(() => { state.value = 1 }); return 0 } }",
        "const { [String(beforeEach(() => { state.value = 1 }))]: value } = import.meta.vitest",
        "const { missing = beforeEach(() => { state.value = 1 }) } = import.meta.vitest",
    ] {
        assert_outcome(&INITIALIZER.replace(original, expression), 1, [1, 1], 0);
    }
}

/// Ignored literal expressions, unused callbacks and unreachable calls preserve real duplicates.
#[test]
fn helper_inert_expressions_retain_positive_findings() {
    let original = "const stop = beforeEach(() => { state.value = 1 })";
    for expression in [
        "const data = { value: 1 }; void 0; return 1",
        "const { value } = { value: 1 }; return () => beforeEach(() => { state.value = 1 })",
        "return; true && beforeEach(() => { state.value = 1 })",
    ] {
        assert_outcome(&INITIALIZER.replace(original, expression), 0, [1, 1], 1);
    }
}
