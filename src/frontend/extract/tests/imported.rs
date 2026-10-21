use super::*;
use std::path::Path;
use std::path::PathBuf;

/// Asserts two path spellings resolve to the same file.
///
/// Windows' `std::fs::canonicalize` adds a verbatim `\\?\` prefix while oxc-resolver returns an
/// ordinary absolute path. Canonicalizing both sides keeps the assertion about exact source
/// identity without requiring those equivalent platform spellings to be byte-identical.
fn assert_same_file(actual: &Path, expected: &Path) {
    assert_eq!(
        actual.canonicalize().unwrap(),
        expected.canonicalize().unwrap()
    );
}

fn contains_same_file<'a>(paths: impl IntoIterator<Item = &'a PathBuf>, expected: &Path) -> bool {
    let expected = expected.canonicalize().unwrap();
    paths
        .into_iter()
        .any(|path| path.canonicalize().is_ok_and(|path| path == expected))
}

#[test]
/// Pins the shared runtime fixture's registration counts, caller ownership and source identity.
fn owned_imported_runner_matches_runtime_registration_and_source_identity() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/vitest-harness/imported");
    let path = root.join("imported.test.ts");
    let extraction = extract_harness("imported/imported.test.ts");
    assert_eq!(extraction.tests.len(), 11);
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
    let tests = declared_tests(&extraction);
    assert_eq!(tests.len(), 7);
    assert_eq!(
        full_titles(&tests),
        vec![
            "shared runner > first",
            "shared runner > second",
            "shared runner > first",
            "shared runner > second",
            "caller suite > shared runner > first",
            "caller suite > shared runner > second",
            "direct",
        ]
    );
    for case in &tests[..6] {
        assert_same_file(&case.location.path, &root.join("runner.ts"));
        assert_eq!(case.registration_file(), path);
        assert!(!case.context.unresolved);
    }
    assert_eq!(tests[0].location, tests[2].location);
    assert_eq!(tests[0].body.alpha, tests[2].body.alpha);
    assert_eq!(tests[0].body.alpha, tests[1].body.alpha);
    assert_eq!(tests[6].registration_file, None);
}

/// Extracts a shared `scripts/vitest-harness` input exactly as its runtime oracle ran it.
fn extract_harness(relative: &str) -> FileExtraction {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts/vitest-harness")
        .join(relative);
    let source = std::fs::read_to_string(&path).unwrap();
    let file = SourceFile::without_project_context(
        path,
        crate::discovery::SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    extract(&source, &file, Globals::Disabled)
}

/// The extracted `test` declarations (suites excluded), in registration order.
fn declared_tests(extraction: &FileExtraction) -> Vec<&TestCase> {
    extraction
        .tests
        .iter()
        .filter(|case| case.kind == DeclarationKind::Test)
        .collect()
}

/// Extracts a harness input, asserting its declaration count and that nothing was refused.
fn extract_complete(relative: &str, declarations: usize) -> FileExtraction {
    let extraction = extract_harness(relative);
    assert_eq!(
        extraction.tests.len(),
        declarations,
        "{:?}",
        extraction.diagnostics
    );
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
    extraction
}

/// Full titles of `tests`, in order.
fn full_titles(tests: &[&TestCase]) -> Vec<String> {
    tests.iter().map(|case| case.full_title()).collect()
}

struct RunnerCase {
    directory: tempfile::TempDir,
    file: SourceFile,
    source: String,
}

impl RunnerCase {
    /// Writes one named helper and caller into an isolated first-party root.
    fn new(helper: &str, caller: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("runner.ts"), helper).unwrap();
        let path = directory.path().join("caller.test.ts");
        let source = format!("import {{ test, describe, beforeEach, vi }} from 'vitest'; import {{ run }} from './runner';\n{caller}");
        std::fs::write(&path, &source).unwrap();
        let file = SourceFile::without_project_context(
            path,
            crate::discovery::SourceLanguage::TypeScript,
            ModuleFlavor::Ambiguous,
        );
        Self {
            directory,
            file,
            source,
        }
    }

    /// Extracts the owned caller under an explicit ambient-globals policy.
    fn extract(&self, globals: Globals) -> FileExtraction {
        extract(&self.source, &self.file, globals)
    }
}

const SIMPLE_RUNNER: &str = "import { test, expect } from 'vitest'; export function run() { test('a', () => { expect(3).toBe(3) }); test('b', () => { expect(3).toBe(3) }); }";

#[test]
/// Retains caller hook/setup/configuration context while using the helper's lexical frame.
fn caller_hooks_setup_configuration_and_modifiers_survive_the_helper_frame() {
    let mut case = RunnerCase::new(SIMPLE_RUNNER, "vi.useFakeTimers(); describe.skipIf(flag)('caller', () => { run(); beforeEach(() => { vi.clearAllTimers(); }); });");
    case.file.config_scope = Some(Ok("caller project".into()));
    let extraction = case.extract(Globals::Disabled);
    assert_eq!(extraction.tests.len(), 3);
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
    for test in &extraction.tests[1..] {
        assert_eq!(test.context.hooks.len(), 1);
        assert!(test
            .modifiers
            .iter()
            .any(|modifier| matches!(modifier, Modifier::SkipIf(_))));
        assert_eq!(test.context.hooks[0].location.path, case.file.path);
        assert_eq!(test.context.config_scope.as_deref(), Some("caller project"));
        assert_eq!(test.context.module_setup.len(), 1);
        assert!(!test.context.unresolved);
    }
    assert_eq!(extraction.helper_sources.len(), 1);
    assert_eq!(
        extraction.helper_sources.values().next().unwrap(),
        SIMPLE_RUNNER
    );
    case.file.config_scope = Some(Err("unknown config".into()));
    let unknown = case.extract(Globals::Disabled);
    assert_eq!(unknown.tests.len(), 3);
    assert!(unknown.tests.iter().all(|case| case.context.unresolved));
}

#[test]
/// Varies governing globals without interpreting shadowed functions as Vitest.
fn caller_globals_enable_ambient_helpers_without_overriding_shadowed_bindings() {
    let case = RunnerCase::new(
        "export function run() { test('a', () => { expect(3).toBe(3) }); }",
        "run(); test('direct', () => {});",
    );
    let disabled = case.extract(Globals::Disabled);
    assert_eq!(disabled.tests.len(), 1);
    assert!(disabled.incomplete && disabled.tests[0].context.unresolved);
    let enabled = case.extract(Globals::Enabled);
    assert_eq!(enabled.tests.len(), 2);
    assert!(!enabled.incomplete);
    assert!(enabled.tests.iter().all(|case| !case.context.unresolved));
}

#[test]
/// Refuses unsupported runner effects and taints tests on both sides of the call.
fn unsupported_imported_effects_refuse_the_body_and_taint_earlier_and_later_siblings() {
    let helpers = [
        "export function run() { beforeEach(setup); test('a', () => {}); }",
        "export function run() { beforeEach(() => {}, setup()); test('a', () => {}); }",
        "export function run() { const t = test.extend({ db: makeDb() }); t('a', () => {}); }",
        "export function run() { const t = test.extend({ [key]: 1 }); t('a', () => {}); }",
        "export function run() { const t = test.extend({ ...shared }); t('a', () => {}); }",
        "export function run() { const t = test.extend(shared); t('a', () => {}); }",
        "export function run(name = 'x') { test(name, () => {}); }",
        "export function run(...names) { test('a', () => {}); }",
        "export function run({ name }) { test('a', () => {}); }",
        "export function run(name) { name = 'x'; test(name, () => {}); }",
        "export function run(name) { test(`${name}`, () => {}); }",
        "export function run(): void; export function run() { test('a', () => {}); }",
        "export function run(): void;",
        "export async function run() { test('a', () => {}); }",
        "export function* run() { test('a', () => {}); }",
        "export let run = () => { test('a', () => {}); };",
        "export var run = function() { test('a', () => {}); };",
        "export const run = async () => { test('a', () => {}); };",
        "export const run = function* () { test('a', () => {}); };",
        "export const run = (cb) => test('a', cb);",
        "export const run = () => { if (flag) test('a', () => {}); };",
        "export const run = makeRunner();",
        "export default makeRunner();",
        "export default async function () { test('a', () => {}); }",
        "export function run() { test('a', () => {}); } run = () => {};",
        "export function run() { test('a', () => {}); } function other() { run = () => {}; }",
        "export function run() { test(title, () => {}); }",
        "export function run() { test('a', callback); }",
        "export function run() { const t = test.skip.extend({ foo: 1 }); t('a', () => {}); }",
        "export function run() { if (flag) test('a', () => {}); }",
        "export function run() { setup(); test('a', () => {}); }",
        "export function run() { test('a', () => {}); } run = () => {};",
        "export function run() { test.skip('a', () => {}); }",
        "export function run() { describe('suite', callback); }",
        "export function run() { describe('suite', (name = test('default', () => {})) => {}); }",
        "export function run() { describe('suite', async () => {}); }",
        "export function run() { test('a', { retry: setup() }, () => {}); }",
        "export function run() { test('a', () => {}); } beforeEach(() => {});",
        "export function run() { return test('a', () => {}); }",
    ];
    for helper in helpers {
        let helper = format!("import {{ test, describe, beforeEach }} from 'vitest'; {helper}");
        let case = RunnerCase::new(
            &helper,
            "test('before', () => {}); run(); test('after', () => {});",
        );
        let extraction = case.extract(Globals::Disabled);
        assert_eq!(extraction.tests.len(), 2, "{helper}");
        assert!(extraction.incomplete, "{helper}");
        assert_eq!(extraction.diagnostics.len(), 1, "{helper}");
        assert!(
            extraction.tests.iter().all(|case| case.context.unresolved),
            "{helper}"
        );
        assert!(extraction.helper_sources.is_empty());
    }
}

#[test]
/// Limits a scoped refusal to its suite while retaining a comparable sibling.
fn refusal_stays_in_its_suite_and_extracted_sibling_scope_remains_comparable() {
    let case = RunnerCase::new(SIMPLE_RUNNER, "describe('safe', () => { run(); }); describe('refused', () => { test('direct', () => {}); run(setup()); }); test('outside', () => {});");
    let extraction = case.extract(Globals::Disabled);
    assert_eq!(extraction.tests.len(), 6);
    assert!(extraction.incomplete);
    assert!(extraction.tests[..3]
        .iter()
        .all(|case| !case.context.unresolved));
    assert!(extraction.tests[3..5]
        .iter()
        .all(|case| case.context.unresolved));
    assert!(!extraction.tests[5].context.unresolved);
}

#[test]
/// Withholds real helper bodies when the runner or any re-export hop is mocked.
fn mocked_direct_and_barrel_runners_never_contribute_real_body_tests() {
    for target in ["./runner", "./barrel"] {
        let case = RunnerCase::new(SIMPLE_RUNNER, &format!("vi.mock('{target}', () => ({{ run: () => {{}} }})); run(); test('direct', () => {{}});"));
        std::fs::write(
            case.directory.path().join("barrel.ts"),
            "export { run } from './runner';",
        )
        .unwrap();
        let source = case.source.replace("from './runner'", "from './barrel'");
        let extraction = extract(&source, &case.file, Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1, "{target}");
        assert!(
            extraction.incomplete && extraction.tests[0].context.unresolved,
            "{target}"
        );
    }
    let case = RunnerCase::new(
        SIMPLE_RUNNER,
        "vi.mock(specifier); run(); test('direct', () => {});",
    );
    let extraction = case.extract(Globals::Disabled);
    assert_eq!(extraction.tests.len(), 1);
    assert!(extraction.incomplete && extraction.tests[0].context.unresolved);
}

#[test]
/// Exercises unresolved, nested, installed-package and cyclic export boundaries.
fn missing_nested_package_and_cyclic_exports_are_bounded_refusals() {
    for helper in [
        "export { run } from './runner';",
        "export { run } from './missing';",
        "export function other() {}",
        "import { nested } from './nested'; export function run() { nested(); }",
        "export function broken( {",
    ] {
        let case = RunnerCase::new(helper, "run(); run(); test('direct', () => {});");
        let extraction = case.extract(Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1, "{helper}");
        assert_eq!(extraction.diagnostics.len(), 1);
        assert!(extraction.incomplete && extraction.tests[0].context.unresolved);
    }
    let case = RunnerCase::new(SIMPLE_RUNNER, "run(); test('direct', () => {});");
    std::fs::remove_file(case.directory.path().join("runner.ts")).unwrap();
    let missing = case.extract(Globals::Disabled);
    assert_eq!(missing.tests.len(), 1);
    assert!(missing.incomplete && missing.tests[0].context.unresolved);
    let package = case.directory.path().join("node_modules/runner");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("package.json"),
        r#"{"name":"runner","type":"module","main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(package.join("index.js"), SIMPLE_RUNNER).unwrap();
    let source = case.source.replace("from './runner'", "from 'runner'");
    let installed = extract(&source, &case.file, Globals::Disabled);
    assert_eq!(installed.tests.len(), 1);
    assert!(installed.incomplete && installed.tests[0].context.unresolved);
}

#[test]
/// Excludes uncalled exports, ordinary utilities, shadows and calls inside test bodies.
fn uncalled_utilities_shadowed_imports_and_test_body_calls_do_not_register_tests() {
    let case = RunnerCase::new(
        SIMPLE_RUNNER,
        "test('direct', () => { run(); }); { const run = () => {}; run(); }",
    );
    let extraction = case.extract(Globals::Disabled);
    assert_eq!(extraction.tests.len(), 1);
    assert!(!extraction.incomplete);
    assert!(extraction.helper_sources.is_empty());
}

#[test]
/// Places imported expansion on both sides of the shared local/imported depth limit.
fn shared_local_and_imported_depth_has_an_exact_boundary() {
    for depth in [7, 8] {
        let mut caller = "entry0(); test('direct', () => {});".to_owned();
        for level in 0..depth {
            caller.push_str(&format!(
                "function entry{level}() {{ {} }}",
                if level + 1 == depth {
                    "run();".into()
                } else {
                    format!("entry{}();", level + 1)
                }
            ));
        }
        let case = RunnerCase::new(SIMPLE_RUNNER, &caller);
        let extraction = case.extract(Globals::Disabled);
        assert_eq!(extraction.tests.len(), if depth == 7 { 3 } else { 1 });
        assert_eq!(extraction.incomplete, depth == 8);
        if depth == 8 {
            assert!(extraction.tests[0].context.unresolved);
        }
    }
}

#[test]
/// Rejects oversized/deeply nested helper sources while retaining the direct caller test.
fn helper_source_size_and_nesting_are_refused_before_parsing() {
    let sources = [
        format!(
            "{}{}",
            SIMPLE_RUNNER,
            " ".repeat(crate::discovery::MAX_SOURCE_FILE_BYTES as usize)
        ),
        format!(
            "export function run() {{ {}test('deep', () => {{}});{} }}",
            "{".repeat(MAX_NESTING_DEPTH),
            "}".repeat(MAX_NESTING_DEPTH)
        ),
    ];
    for helper in sources {
        let case = RunnerCase::new(&helper, "run(); test('direct', () => {});");
        let extraction = case.extract(Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert!(extraction.incomplete && extraction.tests[0].context.unresolved);
    }
}

#[test]
/// Prevents historical declaration extraction from substituting live helper sources.
fn historical_callers_do_not_read_present_day_imported_bodies() {
    let case = RunnerCase::new(SIMPLE_RUNNER, "run(); test('direct', () => {});");
    let extraction = extract_with_imports(&case.source, &case.file, Globals::Disabled, false);
    assert_eq!(extraction.tests.len(), 1);
    assert!(extraction.incomplete && extraction.tests[0].context.unresolved);
    assert!(extraction.helper_sources.is_empty());
    assert!(extraction.diagnostics[0].contains("historical helper sources"));
    assert_eq!(
        extraction.reasons,
        vec![ExclusionReason::HelperSourcesUnavailable]
    );
}

#[test]
/// Classifies an explicit competing runner without consulting unavailable historical sources.
fn historical_competing_runner_calls_are_complete_non_vitest() {
    let source = "import { test } from 'node:test';\ntest('native case', () => {});";
    let file = SourceFile::without_project_context(
        PathBuf::from("node.test.ts"),
        crate::discovery::SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    let extraction = extract_with_imports(source, &file, Globals::Disabled, false);

    assert!(extraction.tests.is_empty());
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
    assert!(extraction.diagnostics.is_empty());
}

#[test]
/// Retains real duplicate positives while separating caller contexts and shared source sites.
fn shared_imported_sources_preserve_duplicate_positives_without_pairing_themselves() {
    let case = RunnerCase::new(SIMPLE_RUNNER, "run(); run();");
    let first = case.extract(Globals::Disabled);
    assert_eq!(first.tests.len(), 4);
    let second_path = case.directory.path().join("second.test.ts");
    let second_source = case
        .source
        .replace("run(); run();", "vi.useRealTimers(); run();");
    std::fs::write(&second_path, &second_source).unwrap();
    let second_file = SourceFile::without_project_context(
        second_path,
        crate::discovery::SourceLanguage::TypeScript,
        ModuleFlavor::Ambiguous,
    );
    let second = extract(&second_source, &second_file, Globals::Disabled);
    assert_eq!(second.tests.len(), 2);
    let config = crate::config::Config::load(
        case.directory.path(),
        crate::config::ConfigOverrides::default(),
    )
    .unwrap();
    let result = crate::analysis::analyze(
        first.tests.into_iter().chain(second.tests).collect(),
        &config,
        2,
        crate::model::IncompleteCauses::default(),
    )
    .unwrap();
    assert_eq!(result.tests_analyzed, 6);
    assert_eq!(
        result
            .findings
            .iter()
            .filter(|finding| finding.rule == crate::model::Rule::CrossFileDuplicate)
            .count(),
        0
    );
    let duplicates: Vec<_> = result
        .findings
        .iter()
        .filter(|finding| finding.rule == crate::model::Rule::DuplicateTest)
        .collect();
    assert_eq!(duplicates.len(), 2);
    for finding in duplicates {
        assert_eq!(finding.locations.len(), 2);
        assert_ne!(finding.locations[0], finding.locations[1]);
        assert_eq!(finding.registration_files_for(0).count(), 1);
    }
}

#[test]
/// Pins the expansion budget across prepare/walk and bounds refusal diagnostics.
fn imported_calls_share_the_two_pass_budget_and_report_once() {
    for calls in [MAX_HELPER_EXPANSIONS / 2, MAX_HELPER_EXPANSIONS / 2 + 1] {
        let case = RunnerCase::new(
            SIMPLE_RUNNER,
            &format!("{}test('direct', () => {{}});", "run();".repeat(calls)),
        );
        let extraction = case.extract(Globals::Disabled);
        assert_eq!(
            extraction.tests.len(),
            if calls == MAX_HELPER_EXPANSIONS / 2 {
                2 * calls + 1
            } else {
                2 * (calls - 2) + 1
            }
        );
        assert_eq!(extraction.incomplete, calls > MAX_HELPER_EXPANSIONS / 2);
        if extraction.incomplete {
            assert_eq!(extraction.diagnostics.len(), 1);
            assert!(extraction.tests.iter().all(|case| case.context.unresolved));
        }
    }
}

#[test]
/// Rejects barrel effects and retains static titles from concise suite callbacks.
fn inert_barrels_are_required_and_concise_imported_suites_keep_static_titles() {
    let case = RunnerCase::new("import { describe, test } from 'vitest'; export function run() { describe('suite', () => test('nested', () => {})); }", "run();");
    let extraction = case.extract(Globals::Disabled);
    assert_eq!(extraction.tests.len(), 2);
    assert!(!extraction.incomplete);
    assert_eq!(extraction.tests[1].full_title(), "suite > nested");
    std::fs::write(
        case.directory.path().join("barrel.ts"),
        "export { run } from './runner'; setup();",
    )
    .unwrap();
    let source = case.source.replace("from './runner'", "from './barrel'");
    let refused = extract(&source, &case.file, Globals::Disabled);
    assert_eq!(refused.tests.len(), 0);
    assert!(refused.incomplete);
}

#[test]
/// Exercises unsupported callable, namespace, invocation and nested-call forms.
fn default_namespace_spread_and_opaque_nested_callables_are_explicitly_refused() {
    let helpers = [
        "export default makeRunner;",
        "export const run = () => setup();",
        "export function run() { const local = () => test('a', () => {}); local(); }",
        "export function run() { const local = setup(); }",
        "export function run(value = test('default', () => {})) { return 2; }",
        "export function run({ value }) { return 2; }",
        "export function run() { return; test('a', () => {}); }",
    ];
    for helper in helpers {
        let case = RunnerCase::new(
            &format!("import {{ test }} from 'vitest'; {helper}"),
            "run();",
        );
        let source = if helper.starts_with("export default") {
            case.source.replace("import { run }", "import run")
        } else {
            case.source.clone()
        };
        let extraction = extract(&source, &case.file, Globals::Disabled);
        assert_eq!(extraction.tests.len(), 0);
        assert!(extraction.incomplete, "{helper}");
    }
    for method in ["call", "apply"] {
        let ordinary = RunnerCase::new(
            "export function run() { return 2; }",
            &format!("run.{method}(); test('direct', () => {{}});"),
        );
        let refused = ordinary.extract(Globals::Disabled);
        assert_eq!(refused.tests.len(), 1);
        assert!(refused.incomplete && refused.tests[0].context.unresolved);
    }
    let case = RunnerCase::new(SIMPLE_RUNNER, "run();");
    for source in [
        case.source
            .replace("import { run }", "import * as ns")
            .replace("run();", "ns.run();"),
        case.source.replace("run();", "run(...args);"),
    ] {
        let extraction = extract(&source, &case.file, Globals::Disabled);
        assert_eq!(extraction.tests.len(), 0);
        assert!(extraction.incomplete);
    }
}

#[test]
/// Taints every caller scope when helper-chain module effects cannot be proved inert.
fn unknown_module_dependencies_taint_the_whole_calling_file() {
    for module_effect in [
        "beforeEach(() => {});",
        "import './hooks';",
        "export { extra } from './hooks';",
        "export default setup();",
        "export default class Registry {}",
    ] {
        let case = RunnerCase::new(&format!("{SIMPLE_RUNNER} {module_effect}"), "test('before', () => {}); describe('caller', () => { run(); }); test('after', () => {});");
        let extraction = case.extract(Globals::Disabled);
        assert_eq!(extraction.tests.len(), 3);
        assert!(extraction.incomplete);
        assert!(extraction.tests.iter().all(|case| case.context.unresolved));
    }
}

#[test]
/// Keeps a proved inert imported arrow utility complete without adding declarations.
fn named_arrow_utilities_are_not_mistaken_for_declaring_runners() {
    let case = RunnerCase::new(
        "export const run = () => 2;",
        "run(); test('direct', () => {});",
    );
    let extraction = case.extract(Globals::Disabled);
    assert_eq!(extraction.tests.len(), 1);
    assert!(!extraction.incomplete);
}

#[test]
/// Bounds unique retained source bytes while repeat calls reuse the same source.
fn unique_helper_sources_share_a_retention_budget_while_repeat_calls_share_one_source() {
    let helper = format!(
        "{}{}",
        SIMPLE_RUNNER,
        " ".repeat(crate::discovery::MAX_SOURCE_FILE_BYTES as usize / 2)
    );
    let case = RunnerCase::new(&helper, "run(); run(); other(); test('direct', () => {});");
    std::fs::write(case.directory.path().join("other.ts"), &helper).unwrap();
    let source = format!("import {{ run as other }} from './other'; {}", case.source);
    let extraction = extract(&source, &case.file, Globals::Disabled);
    assert_eq!(extraction.tests.len(), 5);
    assert!(extraction.incomplete);
    assert_eq!(extraction.helper_sources.len(), 1);
    assert!(extraction.tests.iter().all(|case| case.context.unresolved));
    assert!(extraction.diagnostics[0].contains("retained helper sources"));
}

/// Requires one unresolved direct test when an erased binding cannot provide a runner.
fn assert_refused_type_binding(extraction: FileExtraction) {
    assert_eq!(extraction.tests.len(), 1);
    assert_eq!(extraction.tests[0].full_title(), "direct");
    assert!(extraction.incomplete);
    assert!(extraction.tests[0].context.unresolved);
}

#[test]
/// Rejects runtime runner calls through whole/inline type-only imports and re-exports.
fn type_only_imports_and_reexports_never_supply_runtime_runner_bindings() {
    let case = RunnerCase::new(SIMPLE_RUNNER, "run(); test('direct', () => {});");
    for import in ["import type { run }", "import { type run }"] {
        let source = case.source.replace("import { run }", import);
        let extraction = extract(&source, &case.file, Globals::Disabled);
        assert_refused_type_binding(extraction);
    }
    for export in [
        "export type { run } from './runner';",
        "export { type run } from './runner';",
        "function run() {} export type { run };",
    ] {
        std::fs::write(case.directory.path().join("barrel.ts"), export).unwrap();
        let source = case.source.replace("from './runner'", "from './barrel'");
        let extraction = extract(&source, &case.file, Globals::Disabled);
        assert_refused_type_binding(extraction);
    }
}

#[test]
/// Prevents erased Vitest imports from becoming runtime framework bindings.
fn type_only_framework_names_cannot_declare_imported_tests() {
    for helper in ["import type { test } from 'vitest'; export function run() { test('not a runtime binding', () => {}); }", "import { type test } from 'vitest'; export function run() { test('not a runtime binding', () => {}); }"] {
        let case = RunnerCase::new(helper, "run(); test('direct', () => {});");
        let extraction = case.extract(Globals::Disabled);
        assert_refused_type_binding(extraction);
    }
}

const MOCKING_RUNNER: &str = r#"import { test, expect, vi } from 'vitest';
export function run() {
  test('first mock', async () => {
    vi.resetModules();
    vi.doMock('./dep', () => ({ value: 1 }));
    expect((await import('./dep')).value).toBe(1);
  });
  test('second mock', async () => {
    vi.resetModules();
    vi.doMock('./dep', () => ({ value: 2 }));
    expect((await import('./dep')).value).toBe(2);
  });
}"#;

#[test]
/// Refuses unresolved helper callback mocking and taints direct callers before/after it.
fn helper_callback_mocking_refuses_real_bodies_and_taints_the_calling_file() {
    let case = RunnerCase::new(
        MOCKING_RUNNER,
        "test('before', () => {}); run(); test('after', () => {});",
    );
    std::fs::write(
        case.directory.path().join("dep.ts"),
        "export const value = 0;",
    )
    .unwrap();
    let extraction = case.extract(Globals::Disabled);
    assert_eq!(extraction.tests.len(), 2);
    assert_eq!(extraction.tests[0].full_title(), "before");
    assert_eq!(extraction.tests[1].full_title(), "after");
    assert!(extraction.incomplete);
    assert!(extraction.tests.iter().all(|case| case.context.unresolved));
    assert!(extraction.diagnostics[0].contains("mocking effects"));
}

const OBJECT_UTILITY: &str = "const db = { reset() {} }; export { db as run }; export default db;";

#[test]
/// Keeps named/default object methods outside the imported-runner candidate set.
fn named_and_default_object_methods_are_not_runner_candidates() {
    let case = RunnerCase::new(
        OBJECT_UTILITY,
        "run.reset(); db.reset(); test('direct', () => {});",
    );
    let source = format!("import db from './runner'; {}", case.source);
    let extraction = extract(&source, &case.file, Globals::Disabled);
    assert_eq!(extraction.tests.len(), 1);
    assert_eq!(extraction.tests[0].full_title(), "direct");
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
    assert!(!extraction.tests[0].context.unresolved);
}

/// Bounds repeated refusal work to one visit per existing declaration in a calling scope.
#[test]
fn repeated_refusals_scan_existing_declarations_once_and_keep_the_diagnostic() {
    let source = format!(
        "import {{ test }} from 'vitest'; {}",
        (0..300)
            .map(|i| format!("test('t{i}', () => {{}}); "))
            .collect::<String>()
    );
    let case = RunnerCase::new("", "");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &source, SourceType::ts()).parse();
    assert!(parsed.diagnostics.is_empty());
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&parsed.program);
    assert!(semantic.diagnostics.is_empty());
    let mut registry = Registry::new(&parsed.program, Globals::Disabled);
    let mut walker = Walker::new(
        &source,
        &case.file,
        &parsed.program,
        &semantic.semantic,
        Globals::Disabled,
    );
    walker.walk_statements(&parsed.program.body, &mut registry);
    assert_eq!(walker.tests.len(), 300);
    assert!(walker.tests.iter().all(|case| !case.context.unresolved));
    for _ in 0..MAX_HELPER_EXPANSIONS {
        walker.refuse_helper_expansion(refusal(
            ExclusionReason::HelperLimit,
            "this file's helper-expansion budget is spent",
        ));
    }
    assert!(
        walker.helper_refusal_taint_visits <= 300,
        "{} declaration visits for one scope",
        walker.helper_refusal_taint_visits
    );
    assert_eq!(walker.diagnostics.len(), 1);
    assert!(walker.tests.iter().all(|case| case.context.unresolved));
    walker.walk_statement(&parsed.program.body[1], &mut registry);
    assert_eq!(walker.tests.len(), 301);
    assert!(walker.tests[300].context.unresolved);
}

/// The nested refusal's code and the "further refusals" bookkeeping come from the helper
/// refusals alone: another reason the walker holds (here an unsupported statement recorded first)
/// is neither mistaken for the helper's cause nor allowed to hide a later helper refusal that
/// happens to share its code.
#[test]
fn helper_refusal_reasons_are_tracked_apart_from_other_walker_reasons() {
    let source = "import { test } from 'vitest'; test('t', () => {});";
    let case = RunnerCase::new("", "");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&parsed.program);
    let mut walker = Walker::new(
        source,
        &case.file,
        &parsed.program,
        &semantic.semantic,
        Globals::Disabled,
    );
    walker.reasons.push(ExclusionReason::UnsupportedStatement);
    walker.refuse_helper_expansion(refusal(ExclusionReason::HelperLimit, "budget"));
    assert_eq!(
        walker.helper_refusal_reasons,
        vec![ExclusionReason::HelperLimit]
    );
    walker.refuse_helper_expansion(refusal(ExclusionReason::UnsupportedStatement, "same code"));
    walker.refuse_helper_expansion(refusal(ExclusionReason::UnsupportedStatement, "again"));
    assert_eq!(
        walker
            .further_refusals
            .iter()
            .map(|(reason, _)| *reason)
            .collect::<Vec<_>>(),
        vec![ExclusionReason::UnsupportedStatement],
        "a code already held for another reason still records its first helper refusal, once"
    );
}

const GLOBAL_HOOK_RUNNER: &str = r#"import { beforeEach, test, expect } from 'vitest';
let count = 0;
beforeEach(() => { count += 1; });
export function value() { return count; }
export function run() { test('helper', () => { expect(value()).toBe(1); }); }"#;

/// Keeps sibling suites unresolved when an imported module installs a file-wide hook.
#[test]
fn imported_module_hooks_cannot_be_tainted_only_at_the_calling_suite() {
    let case = RunnerCase::new(GLOBAL_HOOK_RUNNER, "describe('a', () => { run(); }); describe('b', () => { test('sibling', () => { expect(value()).toBe(2); }); });");
    let source = format!(
        "import {{ expect }} from 'vitest'; import {{ value }} from './runner'; {}",
        case.source
    );
    let extraction = extract(&source, &case.file, Globals::Disabled);
    assert_eq!(extraction.tests.len(), 3);
    assert_eq!(extraction.tests[2].full_title(), "b > sibling");
    assert!(extraction.incomplete);
    assert!(extraction.tests.iter().all(|case| case.context.unresolved));
}

#[test]
/// Pins rendered titles, counts and shared declaration identity against Vitest 5.0.3 (Node 24.21.0),
/// which registers exactly these 8 tests from the same input.
fn caller_arguments_render_imported_titles_as_vitest_does() {
    let extraction = extract_complete("imported-params/params.test.ts", 15);
    let tests = declared_tests(&extraction);
    assert_eq!(
        full_titles(&tests),
        vec![
            "suite a > case a",
            "suite bc > case bc",
            "suite a > case a",
            "suite x > case x",
            "suite y > case y",
            "caller > suite nested > case nested",
            "t2",
            "forwarded",
        ]
    );
    assert!(tests.iter().all(|case| case.context.is_comparable()));
    // One declaration site: every call to `declareCase` shares its location and its body.
    assert_eq!(tests[0].location, tests[1].location);
    assert_eq!(tests[0].body.alpha, tests[3].body.alpha);
    assert_ne!(tests[0].body.alpha, tests[6].body.alpha);
    assert_eq!(tests[6].body.alpha, tests[7].body.alpha);
}

/// Extracts `caller` against a one-export helper in an isolated first-party root.
fn extract_call(helper: &str, caller: &str) -> FileExtraction {
    let helper = format!("import {{ test, describe, beforeEach, afterEach, beforeAll, afterAll }} from 'vitest'; {helper}");
    RunnerCase::new(&helper, caller).extract(Globals::Disabled)
}

/// Titles of the extracted `test` declarations, in registration order.
fn test_titles(extraction: &FileExtraction) -> Vec<String> {
    extraction
        .tests
        .iter()
        .filter(|case| case.kind == DeclarationKind::Test)
        .map(|case| case.full_title())
        .collect()
}

#[test]
/// Each call renders its own arguments; extra inert arguments and unused parameters are ignored.
fn each_call_site_renders_its_own_argument_and_ignores_unused_ones() {
    let helper = "export function run(a, b) { test(`${a}`, () => { expect(b); }); }";
    let extraction = extract_call(helper, "run('x', 1); run('y', 2, 'extra'); run(3 + 4, 5);");
    assert_eq!(extraction.tests.len(), 3, "{:?}", extraction.diagnostics);
    assert!(!extraction.incomplete);
    assert_eq!(test_titles(&extraction), ["x", "y", "7"]);
    assert!(extraction
        .tests
        .iter()
        .all(|case| case.context.is_comparable()));
    // `b` is read by the body, so the parameter and a literal are different bodies.
    let literal = extract_call(
        "export function run(a) { test(`${a}`, () => { expect(1); }); }",
        "run('x');",
    );
    assert_ne!(extraction.tests[0].body.alpha, literal.tests[0].body.alpha);
}

#[test]
/// A static title is unaffected by the arguments a runner ignores.
fn arguments_a_static_title_does_not_read_do_not_change_the_extraction() {
    let extraction = extract_call(
        "export function run(a) { test('fixed', () => {}); }",
        "run(); run('x');",
    );
    assert_eq!(test_titles(&extraction), ["fixed", "fixed"]);
    assert!(!extraction.incomplete);
}

#[test]
/// A parameter the caller cannot prove refuses the body instead of rendering a guessed title.
fn unproved_arguments_for_a_title_parameter_refuse_the_runner_and_taint_siblings() {
    let helper = "export function run(a) { test(`${a}`, () => {}); }";
    for call in [
        "run();",
        "run(...names);",
        "run(computed());",
        "run(value);",
        "run('x', computed());",
        "run(`${value}`);",
    ] {
        let extraction = extract_call(
            helper,
            &format!("test('before', () => {{}}); {call} test('after', () => {{}});"),
        );
        assert_eq!(extraction.tests.len(), 2, "{call}");
        assert!(extraction.incomplete, "{call}");
        assert_eq!(extraction.diagnostics.len(), 1, "{call}");
        assert!(
            extraction.tests.iter().all(|case| case.context.unresolved),
            "{call}"
        );
    }
}

#[test]
/// Defaults, rest, destructuring and writes are refused rather than substituted.
fn defaulted_rest_destructured_and_written_parameters_are_not_substituted() {
    for helper in [
        "export function run(a = 'd') { test(`${a}`, () => {}); }",
        "export function run(...a) { test(`${a}`, () => {}); }",
        "export function run({ a }) { test(`${a}`, () => {}); }",
        "export function run([a]) { test(`${a}`, () => {}); }",
        "export function run(a) { a += 'z'; test(`${a}`, () => {}); }",
        "export function run(a) { a++; test('fixed', () => {}); }",
        "export function run(a) { function later() { a = 'z'; } test(`${a}`, () => {}); }",
    ] {
        let extraction = extract_call(helper, "run('x');");
        assert_eq!(extraction.tests.len(), 0, "{helper}");
        assert!(extraction.incomplete, "{helper}");
    }
}

#[test]
/// Suite titles, nested callbacks and loop-unrolled or forwarded arguments all use the caller's values.
fn arguments_reach_suite_titles_loops_and_forwarding_callers() {
    let helper = "export function run(s, t) { describe(s, () => { test(t, () => {}); }); }";
    let extraction = extract_call(
        helper,
        "for (const n of ['a', 'b']) run(n, n); function wrap(v) { run(v, v); } wrap('w');",
    );
    assert_eq!(test_titles(&extraction), ["a > a", "b > b", "w > w"]);
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
    // An unfoldable loop iterable cannot bind its variable; the walk refuses rather than guessing.
    let dynamic = extract_call(helper, "for (const n of names) run(n, n);");
    assert_eq!(dynamic.tests.len(), 0);
    assert!(dynamic.incomplete);
    assert_eq!(dynamic.diagnostics.len(), 1);
}

#[test]
/// A body-local binding with the parameter's name shadows it; the title still reads the parameter.
fn body_locals_shadowing_a_parameter_do_not_change_the_rendered_title() {
    let helper =
        "export function run(a) { test(`${a}`, () => { const a = 'inner'; expect(a); }); }";
    let extraction = extract_call(helper, "run('outer');");
    assert_eq!(test_titles(&extraction), ["outer"]);
}

#[test]
/// Vitest 5.0.3 (Node 24.21.0) registers these 7 tests and passes all of them, including the
/// sibling declared before the helper call that installs the scope's hook.
fn imported_hooks_join_the_calling_scope_and_nested_hooks_stay_isolated() {
    let extraction = extract_complete("imported-hooks/hooks.test.ts", 9);
    let tests = declared_tests(&extraction);
    assert_eq!(
        tests
            .iter()
            .map(|case| (case.full_title(), case.context.hooks.len()))
            .collect::<Vec<_>>(),
        vec![
            ("before scope".to_owned(), 0),
            ("scoped > earlier sibling".to_owned(), 1),
            ("scoped > inside".to_owned(), 1),
            ("scoped > later sibling".to_owned(), 1),
            ("helper suite > sees inner".to_owned(), 1),
            ("fixture test".to_owned(), 0),
            ("after".to_owned(), 0),
        ]
    );
    let runner = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts/vitest-harness/imported-hooks/runner.ts")
        .canonicalize()
        .unwrap();
    for index in [1, 2, 3, 4] {
        let hook = &tests[index].context.hooks[0];
        assert_same_file(&hook.location.path, &runner);
        assert_eq!(hook.kind, HookKind::BeforeEach);
    }
    // The three scoped tests share the one hook the helper installed; the nested hook is distinct.
    assert_eq!(tests[1].context.hooks, tests[3].context.hooks);
    assert_ne!(
        tests[1].context.hooks[0].body,
        tests[4].context.hooks[0].body
    );
    assert_eq!(
        tests[5]
            .context
            .fixtures
            .iter()
            .map(|fixture| fixture.name.as_str())
            .collect::<Vec<_>>(),
        ["value"]
    );
    assert!(tests.iter().all(|case| !case.context.unresolved));
    assert!(contains_same_file(
        extraction.helper_sources.keys(),
        &runner
    ));
}

/// Hook kinds recorded for the first test, in effective order.
fn hook_kinds(extraction: &FileExtraction) -> Vec<HookKind> {
    extraction.tests[0]
        .context
        .hooks
        .iter()
        .map(|hook| hook.kind)
        .collect()
}

#[test]
/// Every hook kind, a timeout argument and a repeated call register in the calling scope in order;
/// a hook-only runner still contributes its hooks and retains its source.
fn imported_hook_kinds_timeouts_order_and_repeat_calls_register_in_the_calling_scope() {
    let helper = "export function install() { beforeAll(() => { a(); }); beforeEach(() => { b(); }, 500); afterEach(() => { c(); }); afterAll(() => { d(); }); }";
    let case = RunnerCase::new(
        &format!("import {{ beforeEach, afterEach, beforeAll, afterAll }} from 'vitest'; {helper}"),
        "beforeEach(() => { first(); }); run(); test('t', () => {});",
    );
    let source = case
        .source
        .replace("import { run }", "import { install as run }");
    let extraction = extract(&source, &case.file, Globals::Disabled);
    assert_eq!(extraction.tests.len(), 1, "{:?}", extraction.diagnostics);
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
    assert_eq!(
        hook_kinds(&extraction),
        [
            HookKind::BeforeAll,
            HookKind::BeforeEach,
            HookKind::BeforeEach,
            HookKind::AfterEach,
            HookKind::AfterAll
        ],
        "caller hook first, then the runner's, afters innermost-first"
    );
    assert_eq!(extraction.helper_sources.len(), 1);
    let twice = source.replace("run();", "run(); run();");
    let repeated = extract(&twice, &case.file, Globals::Disabled);
    assert_eq!(repeated.tests[0].context.hooks.len(), 9);
    let hooks = &repeated.tests[0].context.hooks;
    assert_eq!(hooks[3].location, hooks[4].location);
    assert_ne!(hooks[2].location, hooks[3].location);
}

#[test]
/// Hooks and fixtures keep the calling scope's boundaries: a runner called in a suite does not
/// reach the suite's siblings, and a refusal still excludes only the affected scope.
fn imported_hooks_stay_in_the_suite_that_called_the_runner() {
    let extraction = extract_call(
        "export function run() { beforeEach(() => { a(); }); }",
        "describe('in', () => { run(); test('t', () => {}); }); test('out', () => {});",
    );
    assert_eq!(extraction.tests.len(), 3, "{:?}", extraction.diagnostics);
    let hooks: Vec<_> = extraction
        .tests
        .iter()
        .map(|case| case.context.hooks.len())
        .collect();
    assert_eq!(hooks, [0, 1, 0], "suite, its test, the outside test");
}

#[test]
/// Scoped hooks off a fixture-extended test join the scope; fixture values separate contexts.
fn fixture_bindings_and_scoped_hooks_separate_contexts_by_value() {
    let with = |value: &str| {
        extract_call(
            &format!("export function run() {{ const t = test.extend({{ value: {value} }}); t.beforeEach(() => {{ a(); }}); t('x', ({{ value }}) => {{ use(value); }}); }}"),
            "run();",
        )
    };
    let three = with("3");
    let four = with("4");
    assert_eq!(three.tests.len(), 1, "{:?}", three.diagnostics);
    assert!(!three.incomplete, "{:?}", three.diagnostics);
    assert_eq!(three.tests[0].context.hooks.len(), 1);
    assert_eq!(three.tests[0].context.fixtures.len(), 1);
    assert_ne!(
        three.tests[0].context.fixtures,
        four.tests[0].context.fixtures
    );
    assert_ne!(three.tests[0].context, four.tests[0].context);
    let setup = extract_call(
        "export function run() { const t = test.extend({ db: async ({}, use) => { await use(1); } }); t('x', ({ db }) => { use(db); }); }",
        "run();",
    );
    assert!(!setup.incomplete && setup.tests[0].context.is_comparable());
}

#[test]
/// The pre-scan cannot fold a loop variable, but the runner's hooks still apply to the scope;
/// a call the walk then refuses leaves the scope tainted rather than hookless and comparable.
fn hooks_of_a_runner_called_with_a_loop_variable_still_reach_the_scope() {
    let helper = "export function run(n) { beforeEach(() => { a(); }); test(n, () => {}); }";
    let looped = extract_call(helper, "for (const n of ['a', 'b']) run(n);");
    assert_eq!(test_titles(&looped), ["a", "b"], "{:?}", looped.diagnostics);
    assert!(!looped.incomplete, "{:?}", looped.diagnostics);
    assert!(looped
        .tests
        .iter()
        .all(|case| case.context.hooks.len() == 1));
    let unprovable = extract_call(
        helper,
        "for (const n of names) run(n); test('t', () => {});",
    );
    assert!(unprovable.incomplete);
    assert!(unprovable.tests.iter().all(|case| case.context.unresolved));
    // A body the shape check refuses may register its hook in the pre-scan, but the walk refuses
    // it and taints the scope, so no test is compared under that hook.
    let unsafe_body = extract_call(
        "export function run(n) { beforeEach(() => { a(); }); if (n) test(n, () => {}); }",
        "test('t', () => {}); for (const n of ['a']) run(n);",
    );
    assert!(unsafe_body.incomplete);
    assert_eq!(unsafe_body.tests.len(), 1);
    assert!(unsafe_body.tests.iter().all(|case| case.context.unresolved));
}

/// Extracts the shared callable-forms harness input exactly as its runtime oracle ran it.
#[test]
/// Vitest 5.0.3 (Node 24.21.0) registers these 7 tests from the same input: block and concise
/// arrows, a function expression, default declaration and arrow, and an aliased export.
fn exported_arrows_expressions_defaults_and_aliases_match_the_runtime() {
    let extraction = extract_complete("imported-forms/forms.test.ts", 8);
    let tests = declared_tests(&extraction);
    assert_eq!(
        full_titles(&tests),
        vec![
            "arrow block",
            "arrow concise > inner",
            "expression x",
            "default declaration",
            "default arrow",
            "aliased",
            "arrow block",
        ]
    );
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/vitest-harness/imported-forms");
    let sources = [
        "runner.ts",
        "runner.ts",
        "runner.ts",
        "runner.ts",
        "default-arrow.ts",
        "runner.ts",
        "runner.ts",
    ];
    for (case, source) in tests.iter().zip(sources) {
        assert_same_file(&case.location.path, &root.join(source));
        assert!(case.context.is_comparable());
    }
    // Repeat calls share one declaration site and body; distinct callables do not.
    assert_eq!(tests[0].location, tests[6].location);
    assert_eq!(tests[0].body.alpha, tests[6].body.alpha);
    assert_ne!(tests[0].body.alpha, tests[3].body.alpha);
}

#[test]
/// Hooks and fixtures from a concise arrow or default export join the calling scope.
fn hooks_in_arrow_and_default_runners_reach_the_calling_scope() {
    for (helper, import) in [
        (
            "export const run = () => { beforeEach(() => { a(); }); test('t', () => {}); };",
            "import { run }",
        ),
        (
            "export const run = () => beforeEach(() => { a(); });",
            "import { run }",
        ),
        (
            "export default () => { beforeEach(() => { a(); }); };",
            "import run",
        ),
        (
            "export default function () { beforeEach(() => { a(); }); }",
            "import run",
        ),
    ] {
        let case = RunnerCase::new(
            &format!("import {{ test, beforeEach }} from 'vitest'; {helper}"),
            "run(); test('own', () => {});",
        );
        let source = case.source.replace("import { run }", import);
        let extraction = extract(&source, &case.file, Globals::Disabled);
        assert!(
            !extraction.incomplete,
            "{helper}: {:?}",
            extraction.diagnostics
        );
        let own = extraction.tests.last().unwrap();
        assert_eq!(own.title, Title::Static("own".to_owned()), "{helper}");
        assert_eq!(own.context.hooks.len(), 1, "{helper}");
    }
}

#[test]
/// Parameters and written-parameter refusals apply to arrows exactly as to declarations.
fn arrow_runner_parameters_follow_the_declaration_rules() {
    let render = extract_call(
        "export const run = (label) => describe(label, () => { test(label, () => {}); });",
        "run('a'); run('b');",
    );
    assert_eq!(
        test_titles(&render),
        ["a > a", "b > b"],
        "{:?}",
        render.diagnostics
    );
    for helper in [
        "export const run = (label = 'x') => { test(label, () => {}); };",
        "export const run = ([label]) => { test(label, () => {}); };",
        "export const run = (label) => { function later() { label = 'z'; } test(label, () => {}); };",
    ] {
        let refused = extract_call(helper, "run('a');");
        assert_eq!(refused.tests.len(), 0, "{helper}");
        assert!(refused.incomplete, "{helper}");
    }
}

#[test]
/// Vitest 5.0.3 (Node 24.21.0) registers and passes these 6 tests: a runner forwarding its
/// parameter to an imported runner, repeated calls, a local helper, and a re-export alias.
fn nested_imported_calls_share_identity_titles_hooks_and_provenance() {
    let extraction = extract_complete("imported-nested/nested.test.ts", 9);
    let tests = declared_tests(&extraction);
    assert_eq!(
        full_titles(&tests),
        vec![
            "a > own",
            "leaf a",
            "b > own",
            "leaf b",
            "scoped > leaf local",
            "scoped > leaf alias",
        ]
    );
    // Root hooks apply to every test; the scoped calls add their own pair.
    assert_eq!(
        tests
            .iter()
            .map(|case| case.context.hooks.len())
            .collect::<Vec<_>>(),
        [2, 2, 2, 2, 4, 4]
    );
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/vitest-harness/imported-nested");
    let leaf = root.join("leaf.ts").canonicalize().unwrap();
    for index in [1, 3, 4, 5] {
        assert_same_file(&tests[index].location.path, &leaf);
    }
    assert_eq!(tests[1].location, tests[3].location);
    assert_eq!(tests[1].body.alpha, tests[4].body.alpha);
    assert_same_file(&tests[0].location.path, &root.join("runner.ts"));
    assert!(tests.iter().all(|case| case.context.is_comparable()));
    assert_eq!(
        extraction.helper_sources.len(),
        2,
        "runner and leaf sources are retained"
    );
}

/// Adds sibling modules to a runner case's first-party root.
fn write_modules(case: &RunnerCase, modules: &[(&str, &str)]) {
    for (name, source) in modules {
        std::fs::write(case.directory.path().join(name), source).unwrap();
    }
}

const NESTED_PRELUDE: &str = "import { test, describe, beforeEach } from 'vitest'; ";

#[test]
/// A chain of imported runners shares the 8-frame limit with the caller's local helpers.
fn nested_imported_depth_has_an_exact_boundary() {
    for levels in [8, 9] {
        let case = RunnerCase::new(
            &format!("{NESTED_PRELUDE}export function run() {{ test('leaf', () => {{}}); }}"),
            "run(); test('direct', () => {});",
        );
        for level in 1..levels {
            let body = if level + 1 == levels {
                format!("{NESTED_PRELUDE}export function run() {{ test('leaf', () => {{}}); }}")
            } else {
                format!(
                    "import {{ run as next }} from './l{}'; export function run() {{ next(); }}",
                    level + 1
                )
            };
            write_modules(&case, &[(&format!("l{level}.ts"), &body)]);
        }
        write_modules(
            &case,
            &[(
                "runner.ts",
                "import { run as next } from './l1'; export function run() { next(); }",
            )],
        );
        let extraction = case.extract(Globals::Disabled);
        assert_eq!(
            extraction.tests.len(),
            if levels == 8 { 2 } else { 1 },
            "{levels}: {:?}",
            extraction.diagnostics
        );
        assert_eq!(extraction.incomplete, levels == 9, "{levels}");
        assert_eq!(extraction.diagnostics.len(), usize::from(levels == 9));
    }
}

#[test]
/// Recursion across modules and through a local helper is refused once, with the scope tainted.
fn nested_cycles_are_refused_across_modules_and_through_local_helpers() {
    for (runner, other) in [
        (
            "import { pong } from './other'; export function ping() { test('p', () => {}); pong(); }",
            "import { ping } from './runner'; export function pong() { ping(); }",
        ),
        (
            "export function ping() { test('p', () => {}); again(); } function again() { ping(); }",
            "",
        ),
    ] {
        let case = RunnerCase::new(&format!("{NESTED_PRELUDE}{runner}"), "test('before', () => {}); ping(); test('after', () => {});");
        write_modules(&case, &[("other.ts", &format!("{NESTED_PRELUDE}{other}"))]);
        let source = case.source.replace("import { run }", "import { ping }");
        let extraction = extract(&source, &case.file, Globals::Disabled);
        assert_eq!(extraction.tests.len(), 2, "{runner}: {:?}", extraction.diagnostics);
        assert!(extraction.incomplete, "{runner}");
        assert_eq!(extraction.diagnostics.len(), 1, "{runner}");
        assert!(extraction.tests.iter().all(|case| case.context.unresolved), "{runner}");
    }
}

#[test]
/// A runner that calls the same leaf twice, or an inert utility, declares exactly what runs.
fn repeated_and_inert_nested_calls_declare_exactly_what_runs() {
    let case = RunnerCase::new(
        &format!("{NESTED_PRELUDE}import {{ leaf, util }} from './leaf'; export function run() {{ leaf(); util(); leaf(); }}"),
        "run();",
    );
    write_modules(
        &case,
        &[(
            "leaf.ts",
            &format!("{NESTED_PRELUDE}export function leaf() {{ test('leaf', () => {{}}); }} export function util() {{ return 2; }}"),
        )],
    );
    let extraction = case.extract(Globals::Disabled);
    assert_eq!(extraction.tests.len(), 2, "{:?}", extraction.diagnostics);
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
    assert_eq!(extraction.tests[0].location, extraction.tests[1].location);
}

#[test]
/// A refused nested body, a nested module with setup effects and an installed-package terminal
/// all refuse the outer runner; none leaves a partial declaration or a comparable sibling.
fn unsupported_nested_runners_refuse_the_outer_runner_and_taint_the_file() {
    for leaf in [
        "export function leaf() { if (flag) test('leaf', () => {}); }",
        "export function leaf() { test('leaf', () => {}); } beforeEach(() => {});",
        "export function leaf() { test('leaf', () => {}); } export default setup();",
        "export { leaf } from 'leaf-package';",
    ] {
        let case = RunnerCase::new(
            &format!("{NESTED_PRELUDE}import {{ leaf }} from './leaf'; export function run() {{ test('own', () => {{}}); leaf(); }}"),
            "test('before', () => {}); run(); test('after', () => {});",
        );
        write_modules(&case, &[("leaf.ts", &format!("{NESTED_PRELUDE}{leaf}"))]);
        let package = case.directory.path().join("node_modules/leaf-package");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"leaf-package","type":"module","main":"index.js"}"#,
        )
        .unwrap();
        std::fs::write(package.join("index.js"), "export function leaf() {}").unwrap();
        let extraction = case.extract(Globals::Disabled);
        assert_eq!(
            extraction.tests.len(),
            2,
            "{leaf}: {:?}",
            extraction.diagnostics
        );
        assert!(extraction.incomplete, "{leaf}");
        assert!(
            extraction.tests.iter().all(|case| case.context.unresolved),
            "{leaf}"
        );
    }
}

#[test]
/// A module the runner imports runs at import time even when never called, so its setup effects
/// taint the file; an inert imported module does not.
fn imported_but_uncalled_modules_with_setup_effects_taint_the_file() {
    for (effects, tainted) in [
        ("beforeEach(() => {});", true),
        ("import './deeper'; export const x = 1;", true),
        ("export function helper() {}", false),
    ] {
        let case = RunnerCase::new(
            &format!("{NESTED_PRELUDE}import './effects'; export function run() {{ test('own', () => {{}}); }}"),
            "test('before', () => {}); run();",
        );
        write_modules(
            &case,
            &[
                ("effects.ts", &format!("{NESTED_PRELUDE}{effects}")),
                ("deeper.ts", &format!("{NESTED_PRELUDE}setup();")),
            ],
        );
        let extraction = case.extract(Globals::Disabled);
        assert_eq!(extraction.incomplete, tainted, "{effects}");
        assert_eq!(
            extraction.tests.len(),
            if tainted { 1 } else { 2 },
            "{effects}"
        );
        assert_eq!(
            extraction.tests.iter().all(|case| case.context.unresolved),
            tainted,
            "{effects}"
        );
    }
}

#[test]
/// A recovered worker failure carries its own code, and a nested runner refusal keeps its own.
fn recovered_and_nested_refusals_carry_their_own_codes() {
    let recovered = recovered_extraction("worker failed".to_owned());
    assert_eq!(recovered.reasons, vec![ExclusionReason::ExtractionFailed]);
    assert_eq!(recovered.diagnostics, vec!["worker failed".to_owned()]);
    let case = RunnerCase::new(
        "import { test, expect } from 'vitest'; export function run() { run(); test('x', () => { expect(1).toBe(1); }); }",
        "run();",
    );
    let extraction = case.extract(Globals::Disabled);
    assert_eq!(extraction.reasons, vec![ExclusionReason::HelperLimit]);
}
