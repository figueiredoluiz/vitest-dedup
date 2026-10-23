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
        "import { nested } from './nested'; export function run() { nested(); }",
        "export function broken( {",
    ] {
        let case = RunnerCase::new(helper, "run(); run(); test('direct', () => {});");
        let extraction = case.extract(Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1, "{helper}");
        assert_eq!(extraction.diagnostics.len(), 1, "{helper}");
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

/// A default export of a name is read like `export { name as default }`: same-file function
/// declarations (before or after the export statement) and `const` callables declared before it,
/// exported or not.
#[test]
fn a_default_export_of_a_name_is_read_like_an_alias() {
    let helpers = [
        "const run = () => { test('inside', () => {}); }; export default run;",
        "export default run; function run() { test('inside', () => {}); }",
        "export const run = () => { test('inside', () => {}); }; export default run;",
        "export function run() { test('inside', () => {}); } export default run;",
        "const other = function () { test('inside', () => {}); }; export default other;",
    ];
    for helper in helpers {
        let case = RunnerCase::new(
            &format!("import {{ test }} from 'vitest'; {helper}"),
            "run();",
        );
        let source = case.source.replace("import { run }", "import run");
        let extraction = extract(&source, &case.file, Globals::Disabled);
        assert_eq!(
            extraction.tests.len(),
            1,
            "{helper}: {:?}",
            extraction.diagnostics
        );
        assert_eq!(extraction.tests[0].full_title(), "inside", "{helper}");
        assert!(
            !extraction.incomplete,
            "{helper}: {:?}",
            extraction.diagnostics
        );
    }
}

/// A default-exported name that is not a readable runner stays refused, with the file set aside.
#[test]
fn a_default_export_of_an_unreadable_name_is_refused() {
    let helpers = [
        "let run = () => { test('inside', () => {}); }; export default run;",
        "var run = () => { test('inside', () => {}); }; export default run;",
        "function run() { test('inside', () => {}); } run = () => {}; export default run;",
        "function run() { test('inside', () => {}); } function swap() { run = () => {}; } export default run;",
        "export default run; const run = () => { test('inside', () => {}); };",
        "class run { static go() { test('inside', () => {}); } } export default run;",
        "import inner from './inner'; export default inner;",
        "const run = { go() { test('inside', () => {}); } }; export default run;",
        "const run = makeRunner(); export default run;",
        "const run = async () => { test('inside', () => {}); }; export default run;",
        "export default undeclared;",
    ];
    for helper in helpers {
        let case = RunnerCase::new(
            &format!("import {{ test }} from 'vitest'; {helper}"),
            "run();",
        );
        std::fs::write(
            case.directory.path().join("inner.ts"),
            "import { test } from 'vitest'; export default function inner() { test('inside', () => {}); }",
        )
        .unwrap();
        let source = case.source.replace("import { run }", "import run");
        let extraction = extract(&source, &case.file, Globals::Disabled);
        assert_eq!(extraction.tests.len(), 0, "{helper}");
        assert!(extraction.incomplete, "{helper}");
    }
}

/// A default export that evaluates a name which is no readable callable can throw or run code at
/// module load, so a named runner in the same module is refused with it.
#[test]
fn a_named_runner_beside_an_unreadable_default_export_is_refused() {
    let tail = [
        "export default undeclared;",
        "export default later; const later = () => {};",
        "export default Math;",
        "declare function absent(): void; export default absent;",
    ];
    for tail in tail {
        let case = RunnerCase::new(
            &format!("import {{ test }} from 'vitest'; export function run() {{ test('inside', () => {{}}); }} {tail}"),
            "run();",
        );
        let extraction = extract(&case.source, &case.file, Globals::Disabled);
        assert_eq!(extraction.tests.len(), 0, "{tail}");
        assert!(extraction.incomplete, "{tail}");
    }
    let readable = RunnerCase::new(
        "import { test } from 'vitest'; export function run() { test('inside', () => {}); } function other() {} export default other;",
        "run();",
    );
    let extraction = extract(&readable.source, &readable.file, Globals::Disabled);
    assert_eq!(extraction.tests.len(), 1, "{:?}", extraction.diagnostics);
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
        // A helper that registers nothing is setup however it is invoked.
        let setup = ordinary.extract(Globals::Disabled);
        assert_eq!(setup.tests.len(), 1);
        assert!(!setup.incomplete && !setup.tests[0].context.unresolved);
        assert_eq!(setup.tests[0].context.module_setup.len(), 1);
        // A runner that declares tests is not read through `.call`/`.apply`.
        let runner = RunnerCase::new(
            SIMPLE_RUNNER,
            &format!("run.{method}(); test('direct', () => {{}});"),
        );
        let refused = runner.extract(Globals::Disabled);
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
    let diagnostics = extraction.diagnostics.clone();
    assert_eq!(extraction.tests.len(), 1);
    assert_eq!(extraction.tests[0].full_title(), "direct");
    assert!(extraction.incomplete, "{diagnostics:?} {extraction:?}");
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
    // `export { type run }` still loads the runner module, which registers tests.
    std::fs::write(
        case.directory.path().join("barrel.ts"),
        "export { type run } from './runner';",
    )
    .unwrap();
    let source = case.source.replace("from './runner'", "from './barrel'");
    assert_refused_type_binding(extract(&source, &case.file, Globals::Disabled));
    // A whole type-only re-export loads nothing at runtime: the runner's tests are never read
    // through it, and nothing it exports can register.
    std::fs::write(
        case.directory.path().join("barrel.ts"),
        "export type { run } from './runner';",
    )
    .unwrap();
    let extraction = extract(&source, &case.file, Globals::Disabled);
    assert_eq!(extraction.tests.len(), 1);
    assert_eq!(extraction.tests[0].full_title(), "direct");
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
            r#"{"name":"leaf-package","type":"module","main":"index.js","peerDependencies":{"vitest":"*"}}"#,
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

// ---- Setup helpers: a call that provably registers nothing ----------------------------------

/// A directory of helper modules a caller can be written beside and extracted from repeatedly, so
/// two extractions can name the same helper files.
struct Site {
    directory: tempfile::TempDir,
}

impl Site {
    fn new(files: &[(&str, &str)]) -> Self {
        let site = Self {
            directory: tempfile::tempdir().unwrap(),
        };
        for (name, text) in files {
            site.write(name, text);
        }
        site
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.directory.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn extract(&self, caller: &str) -> FileExtraction {
        self.extract_with(caller, Globals::Disabled)
    }

    fn extract_with(&self, caller: &str, globals: Globals) -> FileExtraction {
        self.extract_in("", caller, globals)
    }

    /// Extracts `caller` written to `relative/caller.test.ts`.
    fn extract_in(&self, relative: &str, caller: &str, globals: Globals) -> FileExtraction {
        let directory = self.directory.path().join(relative);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("caller.test.ts");
        std::fs::write(&path, caller).unwrap();
        let file = SourceFile::without_project_context(
            path,
            crate::discovery::SourceLanguage::TypeScript,
            ModuleFlavor::Ambiguous,
        );
        extract(caller, &file, globals)
    }
}

/// Builds a caller with the given helper modules beside it, extracts it and returns the result.
fn with_helpers(files: &[(&str, &str)], caller: &str) -> FileExtraction {
    Site::new(files).extract(caller)
}

const TWO_TESTS: &str =
    "test('a', () => { expect(1).toBe(1) }); test('b', () => { expect(1).toBe(1) });";

/// A helper that registers nothing is setup: nothing is refused, both tests stay comparable and
/// each carries one setup fact.
#[test]
fn a_call_to_a_helper_that_registers_nothing_is_setup() {
    let caller = format!(
        "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; setup(); {TWO_TESTS}"
    );
    let extraction = with_helpers(
        &[("setup.ts", "export function setup() { return 1 }")],
        &caller,
    );
    assert_eq!(extraction.tests.len(), 2, "{:?}", extraction.diagnostics);
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
    for case in &extraction.tests {
        assert!(!case.context.unresolved);
        assert_eq!(case.context.module_setup.len(), 1);
        assert!(case.context.module_setup[0].starts_with("helper:"));
    }
    // The call is setup wherever it sits, including after the tests and inside a suite.
    let later = format!(
        "import {{ test, expect, describe }} from 'vitest'; import {{ setup }} from './setup'; {TWO_TESTS} describe('s', () => {{ setup(); }});"
    );
    let extraction = with_helpers(
        &[("setup.ts", "export function setup() { return 1 }")],
        &later,
    );
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
    assert!(extraction
        .tests
        .iter()
        .all(|case| case.context.module_setup.len() == 1));
}

/// What the setup stands on is part of its identity: the helper, its arguments and what they read.
#[test]
fn setup_identity_separates_helpers_arguments_and_what_arguments_read() {
    let site = Site::new(&[(
        "setup.ts",
        "export function setup(value?: unknown) { return value }",
    )]);
    let setup_of = |arguments: &str, prelude: &str| {
        let caller = format!(
            "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; {prelude} setup({arguments}); test('a', () => {{ expect(1).toBe(1) }});"
        );
        let extraction = site.extract(&caller);
        assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
        extraction.tests[0].context.module_setup.clone()
    };
    let base = setup_of("1", "");
    assert_eq!(base, setup_of("1", ""));
    assert_ne!(base, setup_of("2", ""));
    // A name the argument reads is part of it: the text of the call is identical.
    let reads = |value: &str| setup_of("config", &format!("const config = {{ mode: '{value}' }};"));
    assert_ne!(reads("a"), reads("b"));
    assert_eq!(reads("a"), reads("a"));
    // The same call over a different helper module is a different setup.
    let other = Site::new(&[(
        "setup.ts",
        "export function setup(value?: unknown) { return [value] }",
    )]);
    let caller = "import { test, expect } from 'vitest'; import { setup } from './setup'; setup(1); test('a', () => { expect(1).toBe(1) });";
    assert_ne!(base, other.extract(caller).tests[0].context.module_setup);
}

/// A name an argument reads from another module is part of the identity by module and export: the
/// call text is the same either way.
#[test]
fn setup_identity_separates_what_arguments_import() {
    let site = Site::new(&[
        (
            "setup.ts",
            "export function setup(value?: unknown) { return value }",
        ),
        ("config-a.ts", "export const config = { mode: 'a' }"),
        ("config-b.ts", "export const config = { mode: 'b' }"),
    ]);
    let setup_of = |module: &str| {
        let caller = format!(
            "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; import {{ config }} from './{module}'; setup(config); test('a', () => {{ expect(1).toBe(1) }});"
        );
        let extraction = site.extract(&caller);
        assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
        extraction.tests[0].context.module_setup.clone()
    };
    assert_eq!(setup_of("config-a"), setup_of("config-a"));
    assert_ne!(setup_of("config-a"), setup_of("config-b"));
}

/// Locals the identity can place are declarations (variables, functions, classes); one it cannot
/// place refuses the setup instead of guessing at it.
#[test]
fn setup_identity_places_classes_and_refuses_what_it_cannot_place() {
    let site = Site::new(&[(
        "setup.ts",
        "export function setup(value?: unknown) { return value }",
    )]);
    let run = |declaration: &str| {
        let caller = format!(
            "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; {declaration} setup(Subject); test('a', () => {{ expect(1).toBe(1) }});"
        );
        site.extract(&caller)
    };
    let class = run("class Subject { value = 1 }");
    assert!(!class.incomplete, "{:?}", class.diagnostics);
    assert_ne!(
        class.tests[0].context.module_setup,
        run("class Subject { value = 2 }").tests[0]
            .context
            .module_setup
    );
    let enumeration = run("enum Subject { A }");
    assert!(enumeration.incomplete, "{:?}", enumeration.diagnostics);
}

/// A helper is handed arguments it could call: one that reaches a registration API makes the call
/// a runner question again, not setup.
#[test]
fn arguments_that_could_register_something_keep_the_call_refused() {
    let helper = (
        "setup.ts",
        "export function setup(callback?: () => void) { callback?.() }",
    );
    let refused = |arguments: &str, prelude: &str| {
        let caller = format!(
            "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; {prelude} setup({arguments}); {TWO_TESTS}"
        );
        let extraction = with_helpers(&[helper], &caller);
        assert!(
            extraction.incomplete,
            "{arguments}: {:?}",
            extraction.diagnostics
        );
        assert!(extraction.tests.iter().all(|case| case.context.unresolved));
    };
    refused("() => { test('inside', () => {}) }", "");
    refused(
        "register",
        "function register() { test('inside', () => {}) }",
    );
    refused(
        "register",
        "const register = () => test('inside', () => {});",
    );
    refused("...rest", "const rest = [];");
    // An argument that only builds data is fine.
    let caller = format!(
        "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; setup(() => 1); {TWO_TESTS}"
    );
    let extraction = with_helpers(&[helper], &caller);
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
}

/// A helper that registers anything is still read as a runner or refused, never setup.
#[test]
fn a_helper_that_registers_is_not_setup() {
    let caller = format!(
        "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; setup(); {TWO_TESTS}"
    );
    let extraction = with_helpers(
        &[(
            "setup.ts",
            "import { beforeEach } from 'vitest'; export function setup() { if (process.env.CI) { beforeEach(() => {}); } }",
        )],
        &caller,
    );
    assert!(extraction.incomplete, "{:?}", extraction.diagnostics);
    assert!(extraction.tests.iter().all(|case| case.context.unresolved));
}

/// A mocked helper contributes its mock, not its body: it cannot be proved from the real file.
#[test]
fn a_mocked_helper_is_not_setup() {
    let caller = format!(
        "import {{ test, expect, vi }} from 'vitest'; import {{ setup }} from './setup'; vi.mock('./setup', () => ({{ setup: () => {{}} }})); setup(); {TWO_TESTS}"
    );
    let extraction = with_helpers(
        &[("setup.ts", "export function setup() { return 1 }")],
        &caller,
    );
    assert!(extraction.incomplete, "{:?}", extraction.diagnostics);
    assert!(extraction.tests.iter().all(|case| case.context.unresolved));
}

/// `@nuxt/test-utils/runtime` installed the way a project has it: a manifest that lists Vitest as a
/// peer (other entry points register hooks) and a runtime entry that never mentions it.
fn install_test_utils(files: &mut Vec<(&'static str, &'static str)>) {
    files.push((
        "node_modules/@nuxt/test-utils/package.json",
        r#"{"name":"@nuxt/test-utils","type":"module","exports":{"./runtime":"./dist/runtime-utils/index.mjs"},"peerDependencies":{"vitest":"*"}}"#,
    ));
    files.push((
        "node_modules/@nuxt/test-utils/dist/runtime-utils/index.mjs",
        "export const mountSuspended = () => 1\nexport const registerEndpoint = () => 1\nexport const mockNuxtImport = () => 1\nexport const other = () => 1",
    ));
}

/// The framework's own runtime exports are setup by name, though the package lists Vitest as a peer;
/// an export that is not known stays refused.
#[test]
fn known_test_utils_runtime_exports_are_setup() {
    let mut files = Vec::new();
    install_test_utils(&mut files);
    let known = format!(
        "import {{ test, expect }} from 'vitest'; import {{ mountSuspended }} from '@nuxt/test-utils/runtime'; mountSuspended(1); {TWO_TESTS}"
    );
    let extraction = with_helpers(&files, &known);
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
    assert!(extraction
        .tests
        .iter()
        .all(|case| case.context.module_setup.len() == 1));
    let unknown = known.replace("mountSuspended", "other");
    let extraction = with_helpers(&files, &unknown);
    assert!(extraction.incomplete, "{:?}", extraction.diagnostics);
}

/// `registerEndpoint` installs a mock endpoint when it runs and calls nothing it is handed. What it
/// stands on is part of the setup, so files whose handlers differ never compare equal.
#[test]
fn register_endpoint_is_setup_that_stands_on_what_it_reads() {
    fn setup_of(handler: &str) -> Vec<String> {
        let caller = format!(
            "import {{ test, expect }} from 'vitest'; import {{ registerEndpoint }} from '@nuxt/test-utils/runtime'; const handler = {handler}; registerEndpoint('/users', {{ handler }}); {TWO_TESTS}"
        );
        let extraction = with_helpers(&[], &caller);
        assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
        assert_eq!(extraction.tests.len(), 2);
        extraction.tests[0].context.module_setup.clone()
    }
    let first = setup_of("() => ({ users: [] })");
    assert_eq!(first.len(), 1);
    assert!(first[0].starts_with("api:"));
    assert_eq!(first, setup_of("() => ({ users: [] })"));
    assert_ne!(first, setup_of("() => ({ users: [1] })"));
}

/// `mockNuxtImport` is a hoisted mock of one auto-imported binding, keyed by its name; it is
/// refused like a nested `vi.mock` anywhere but the top level, and with a factory it cannot read.
#[test]
fn mock_nuxt_import_is_a_hoisted_mock_keyed_by_name() {
    let import = "import { test, expect, describe } from 'vitest'; import { mockNuxtImport } from '@nuxt/test-utils/runtime';";
    let extract_with =
        |statement: &str| with_helpers(&[], &format!("{import} {statement} {TWO_TESTS}"));
    let mocked = |factory: &str| {
        let extraction = extract_with(&format!("mockNuxtImport('useRoute', {factory});"));
        assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
        extraction.tests[0].context.mocked_modules.clone()
    };
    let first = mocked("() => () => ({ path: '/a' })");
    assert_eq!(first.len(), 1);
    assert_eq!(&*first[0].specifier, "nuxt-import:useRoute");
    assert!(matches!(
        first[0].factory,
        crate::model::MockFactory::Factory(_)
    ));
    assert_eq!(first, mocked("() => () => ({ path: '/a' })"));
    assert_ne!(first, mocked("() => () => ({ path: '/b' })"));
    // Two names are two mocks; the same name twice is one, the last winning.
    let extraction = extract_with(
        "mockNuxtImport('useRoute', () => () => 1); mockNuxtImport('useState', () => () => 2);",
    );
    assert_eq!(extraction.tests[0].context.mocked_modules.len(), 2);
    let twice = extract_with(
        "mockNuxtImport('useRoute', () => () => 1); mockNuxtImport('useRoute', () => () => 2);",
    );
    let last_only = extract_with("mockNuxtImport('useRoute', () => () => 2);");
    assert_eq!(twice.tests[0].context.mocked_modules.len(), 1);
    assert_eq!(
        twice.tests[0].context.mocked_modules,
        last_only.tests[0].context.mocked_modules
    );
    for unreadable in [
        "mockNuxtImport('useRoute', factory);",
        "mockNuxtImport(name, () => () => 1);",
        "mockNuxtImport('useRoute');",
        "describe('s', () => { mockNuxtImport('useRoute', () => () => 1); });",
    ] {
        let extraction = extract_with(unreadable);
        assert!(
            extraction.tests.iter().all(|case| case.context.unresolved),
            "{unreadable}"
        );
    }
}

/// A name written after its declaration is not what the declaration says: a callback reassigned
/// before the call, or a config changed through a member, keeps the call from being setup.
#[test]
fn bindings_written_after_their_declaration_are_not_placed() {
    let site = Site::new(&[(
        "setup.ts",
        "export function setup(value?: unknown) { return value }",
    )]);
    let run = |prelude: &str| {
        let caller = format!(
            "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; {prelude} setup(subject); {TWO_TESTS}"
        );
        site.extract(&caller)
    };
    for written in [
        "let subject = () => {}; subject = () => test('hidden', () => {});",
        "const subject = { mode: 1 }; subject.mode = 2;",
        "const subject = { mode: 1 }; subject['mode']++;",
        "const subject = { mode: 1 }; delete subject.mode;",
        "const subject = [1]; (subject as number[])[0] = 2;",
        "const subject = { mode: 1 }; ({ mode: subject.mode } = { mode: 2 });",
        "const subject = { mode: 1 }; [subject.mode] = [2];",
        "const subject = { mode: 1 }; ({ ...subject.rest } = { a: 1 });",
        "const subject = { mode: 1 }; for (subject.mode of [2]) {}",
    ] {
        let extraction = run(written);
        assert!(
            extraction.incomplete,
            "{written}: {:?}",
            extraction.diagnostics
        );
    }
    let extraction = run("const subject = { mode: 1 };");
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
    // Read through a member, a written binding is placed when every value written in is visible to
    // the comparison: in a hook, not at module level, where no context records it.
    let placed = |write: &str| {
        let caller = format!(
            "import {{ test, expect, beforeEach }} from 'vitest'; import {{ setup }} from './setup'; const subject = {{ mode: 1 }}; {write} setup(subject.mode); {TWO_TESTS}"
        );
        !site.extract(&caller).incomplete
    };
    assert!(placed("beforeEach(() => { subject.mode = 2 });"));
    assert!(!placed("subject.mode = 2;"));
}

/// Setup runs in order and as many times as it is called: both are part of the context.
#[test]
fn setup_calls_keep_their_order_and_their_count() {
    let site = Site::new(&[(
        "setup.ts",
        "export function setup(value?: unknown) { return value }",
    )]);
    let setup_of = |calls: &str| {
        let caller = format!(
            "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; {calls} test('a', () => {{ expect(1).toBe(1) }});"
        );
        let extraction = site.extract(&caller);
        assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
        extraction.tests[0].context.module_setup.clone()
    };
    let forward = setup_of("setup(1); setup(2);");
    assert_eq!(forward.len(), 2);
    assert_ne!(forward, setup_of("setup(2); setup(1);"));
    assert_ne!(setup_of("setup(1);"), setup_of("setup(1); setup(1);"));
    assert_eq!(setup_of("setup(1); setup(1);").len(), 2);
}

/// The local name an argument reads is tied to the export it stands for.
#[test]
fn setup_identity_ties_each_name_to_its_export() {
    let site = Site::new(&[
        (
            "setup.ts",
            "export function setup(left?: unknown, right?: unknown) { return [left, right] }",
        ),
        ("values.ts", "export const a = 1; export const b = 2"),
    ]);
    let setup_of = |first: &str, second: &str| {
        let caller = format!(
            "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; import {{ {first} as left, {second} as right }} from './values'; setup(left, right); test('a', () => {{ expect(1).toBe(1) }});"
        );
        let extraction = site.extract(&caller);
        assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
        extraction.tests[0].context.module_setup.clone()
    };
    assert_eq!(setup_of("a", "b"), setup_of("a", "b"));
    assert_ne!(setup_of("a", "b"), setup_of("b", "a"));
}

/// The arguments of `registerEndpoint` are evaluated first, so a hook registered there is refused;
/// a package that only resembles the framework's is not the framework.
#[test]
fn register_endpoint_arguments_are_judged_and_the_specifier_is_exact() {
    let hooked = format!(
        "import {{ test, expect, beforeEach }} from 'vitest'; import {{ registerEndpoint }} from '@nuxt/test-utils/runtime'; registerEndpoint('/x', (beforeEach(() => {{}}), () => ({{}}))); {TWO_TESTS}"
    );
    assert!(with_helpers(&[], &hooked).incomplete);
    // With globals the hook is a free name: the identity alone would place it.
    let global_hook = format!(
        "import {{ registerEndpoint }} from '@nuxt/test-utils/runtime'; registerEndpoint('/x', (beforeEach(() => {{}}), () => ({{}}))); {TWO_TESTS}"
    );
    assert!(
        Site::new(&[])
            .extract_with(&global_hook, Globals::Enabled)
            .incomplete
    );
    let lookalike = format!(
        "import {{ test, expect }} from 'vitest'; import {{ registerEndpoint }} from '@nuxt/test-utils-custom/runtime'; registerEndpoint('/x', () => ({{}})); {TWO_TESTS}"
    );
    assert!(with_helpers(&[], &lookalike).incomplete);
    let mock_lookalike = format!(
        "import {{ test, expect }} from 'vitest'; import {{ mockNuxtImport }} from '@nuxt/test-utils-custom/runtime'; mockNuxtImport('useX', () => () => 1); {TWO_TESTS}"
    );
    let extraction = with_helpers(&[], &mock_lookalike);
    assert!(extraction.tests[0].context.mocked_modules.is_empty());
}

/// A `mockNuxtImport` factory runs wherever the auto-import is called, so one that registers
/// something leaves the file unresolved.
#[test]
fn a_nuxt_mock_factory_that_registers_leaves_the_file_unresolved() {
    let caller = format!(
        "import {{ test, expect, beforeEach }} from 'vitest'; import {{ mockNuxtImport }} from '@nuxt/test-utils/runtime'; mockNuxtImport('useSetup', () => () => {{ beforeEach(() => {{}}) }}); {TWO_TESTS}"
    );
    let extraction = with_helpers(&[], &caller);
    assert!(extraction.tests.iter().all(|case| case.context.unresolved));
}

/// A factory may read a binding the file writes when every value written into it is visible and
/// registers nothing; a bare reference, a hidden write or a registering value still refuses.
#[test]
fn a_nuxt_mock_factory_may_read_a_binding_the_file_writes() {
    let run = |body: &str| {
        let caller = format!(
            "import {{ test, expect, beforeEach, describe }} from 'vitest'; import {{ mockNuxtImport }} from '@nuxt/test-utils/runtime'; {body} {TWO_TESTS}"
        );
        let extraction = with_helpers(&[], &caller);
        assert!(extraction.tests.len() >= 2, "{body}");
        extraction.tests.iter().all(|case| !case.context.unresolved)
    };
    let route = "const routeMock = { value: { path: '/' } }; mockNuxtImport('useRoute', () => () => routeMock.value);";
    for accepted in [
        format!("{route} beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }});"),
        format!("{route} beforeEach(() => {{ (routeMock as any).value.path = '/x' }});"),
        format!("{route} type Route = typeof routeMock; beforeEach(() => {{ routeMock.value = {{ path: '/x' }} as Route['value'] }});"),
        format!("{route} beforeEach(() => {{ Object.assign(routeMock.value, {{ path: '/y' }}) }});"),
        format!("{route} beforeEach(() => {{ delete routeMock.value; expect(routeMock.value).toBe(1) }});"),
        "const state = { count: 0 }; mockNuxtImport('useCount', () => () => state.count); beforeEach(() => { state.count++ });".to_owned(),
        "let route = { path: '/' }; mockNuxtImport('useRoute', () => () => route.path); beforeEach(() => { route = { path: '/x' } });".to_owned(),
        "const holder = { value: vi.fn() }; mockNuxtImport('useHolder', () => () => holder.value); beforeEach(() => { holder.value = vi.fn(); holder.value.mockImplementation(() => 1) });".to_owned(),
        "const kinds = { Kind: class {} }; mockNuxtImport('useKind', () => () => kinds.Kind); beforeEach(() => { kinds.Kind = class {}; new kinds.Kind(() => 1) });".to_owned(),
        format!("{route} beforeEach(async () => {{ await (routeMock.value = {{ path: '/x' }}); try {{ routeMock.value = {{ path: '/y' }} }} catch {{ routeMock.value = {{ path: '/z' }} }} }});"),
        "const state = { run: () => {} }; beforeEach(() => { state.run = () => {} }); mockNuxtImport('useRun', () => () => state.run);".to_owned(),
        "import { afterAll as finally_ } from 'vitest'; const state = { run: () => {} }; finally_(() => { state.run = () => {} }); mockNuxtImport('useRun', () => () => state.run);".to_owned(),
        format!("{route} test('writes', () => {{ routeMock.value = {{ path: '/x' }} }});"),
        format!("{route} beforeEach(() => {{ routeMock.value = {{ path: '/x' }}; globalThis.eval('1') }});"),
        "const state = { run: () => {} }; mockNuxtImport('useRun', () => () => state.run); beforeEach(() => { try { void 0 } catch { var picked = () => {} } state.run = picked });".to_owned(),
        format!("{route} describe('suite', () => {{ {{ beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}) }}; test('inner', () => {{}}) }});"),
        format!("{route} describe('suite', () => {{ const pick = () => {{ return 1 }}; pick(); beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}); test('inner', () => {{}}) }});"),
        format!("{route} describe('suite', () => {{ function pick() {{ return 1 }} class Kind {{ make() {{ throw new Error('x') }} }} pick(); new Kind(); beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}); test('inner', () => {{}}) }});"),
        format!("{route} beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}); test('loops', () => {{ for (const key in routeMock.value) {{ expect(key).toBe('path') }} let path; ({{ path = routeMock.value.path }} = {{}} as {{ path?: string }}) }});"),
        "const state = { marker: 0, make: () => <T,>(value: T) => value }; mockNuxtImport('useMarker', () => () => state.marker); beforeEach(() => { state.marker = 1; (state.make())<number>; });".to_owned(),
    ] {
        assert!(run(&accepted), "{accepted}");
    }
    for refused in [
        format!("{route} beforeEach(() => {{ routeMock.value = () => test('hidden', () => {{}}) }});"),
        format!("{route} function reset(target: unknown) {{ return target }} beforeEach(() => {{ routeMock.value = {{ path: '/x' }}; reset(routeMock) }});"),
        format!("{route} beforeEach(() => {{ ({{ a: routeMock.value }} = {{ a: {{ path: '/x' }} }}) }});"),
        format!("{route} beforeEach(() => {{ for (routeMock.value of [{{ path: '/x' }}]) {{}} }});"),
        format!("{route} beforeEach(() => {{ routeMock.value = {{ path: '/x' }}; routeMock.value.tag`${{() => test('hidden', () => {{}})}}` }});"),
        "const state = { count: 0 }; mockNuxtImport('useState', () => () => state); beforeEach(() => { state.count++ });".to_owned(),
        "let n = 0; mockNuxtImport('useCount', () => () => n); beforeEach(() => { n++ });".to_owned(),
        "let route = { path: '/' }; mockNuxtImport('useRoute', () => () => route.path); beforeEach(() => { route = { path: () => test('hidden', () => {}) } });".to_owned(),
        "let route; mockNuxtImport('useRoute', () => () => route.path); beforeEach(() => { route = { path: '/x' } });".to_owned(),
        "const holder = { value: vi.fn() }; mockNuxtImport('useHolder', () => () => holder.value); beforeEach(() => { holder.value = vi.fn(); holder.value.mockImplementation(() => beforeEach(() => {})) });".to_owned(),
        "const holder = { value: vi.fn() }; mockNuxtImport('useHolder', () => () => holder.value); beforeEach(() => { holder.value = vi.fn(); holder.value.mockImplementation(...[() => 1]) });".to_owned(),
        "const kinds = { Kind: class {} }; mockNuxtImport('useKind', () => () => kinds.Kind); beforeEach(() => { kinds.Kind = class {}; new kinds.Kind(() => test('hidden', () => {})) });".to_owned(),
        "const kinds = { Kind: class {} }; mockNuxtImport('useKind', () => () => kinds.Kind); beforeEach(() => { kinds.Kind = class {}; new kinds.Kind(...[() => 1]) });".to_owned(),
        "const { route } = { route: { path: '/' } }; mockNuxtImport('useRoute', () => () => route.path); beforeEach(() => { route.path = '/x' });".to_owned(),
        "mockNuxtImport('useRoute', () => (route = { path: '/' }) => { route.path = '/x'; return route.path });".to_owned(),
        "const holder = { value: vi.fn() }; mockNuxtImport('useHolder', () => () => holder.value); beforeEach(() => { holder.value = vi.fn() }); beforeEach(() => holder.value.mockImplementation(() => 1));".to_owned(),
        "const state = { run: () => {} }; mockNuxtImport('useRun', () => () => state.run); beforeEach(() => { const set = (fn: () => void) => { state.run = fn }; set(() => test('hidden', () => {})) });".to_owned(),
        "const state = { run: () => {} }; mockNuxtImport('useRun', () => () => state.run); beforeEach(() => { function plant(this: { fn: () => void }) { state.run = this.fn } plant.call({ fn: () => test('hidden', () => {}) }) });".to_owned(),
        "const state = { run: () => {} }; mockNuxtImport('useRun', () => () => state.run); beforeEach(() => { try { throw () => test('hidden', () => {}) } catch (error) { state.run = error as () => void } });".to_owned(),
        "const state = { run: () => {} }; mockNuxtImport('useRun', () => () => state.run); beforeEach(() => { class Planter { static { state.run = () => {} } } new Planter() });".to_owned(),
        "const state = { run: () => {}, self() { return this } }; beforeEach(() => { state.run = () => {}; const alias = state.self(); alias.run = () => test('hidden', () => {}) }); mockNuxtImport('useRun', () => () => state.run);".to_owned(),
        // Writes the compared context cannot see, each one edit from an accepted case.
        format!("{route} delete routeMock.value;"),
        "const state = { count: 0 }; mockNuxtImport('useCount', () => () => state.count); state.count++;".to_owned(),
        "let route = { path: '/' }; mockNuxtImport('useRoute', () => () => route.path); beforeEach(() => { route = { path: '/x' } }); (route as any)++;".to_owned(),
        format!("{route} onReady(() => {{ routeMock.value = {{ path: '/x' }} }});"),
        // A write the compared context cannot see: module level, a plain function, a chained test
        // callee, a local function that is only named like a hook.
        format!("{route} test.only('writes', () => {{ routeMock.value = {{ path: '/x' }} }});"),
        // Codex rounds 3-4: hook calls the walker never registers, or records as if they always ran;
        // a free hook outside globals mode.
        format!("{route} describe('suite', () => {{ if (routeMock.value) {{ beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}) }} }});"),
        format!("{route} for (const _ of [1]) {{ beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}) }}"),
        format!("{route} try {{ beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}) }} finally {{}}"),
        format!("{route} afterAll(() => {{ routeMock.value = {{ path: '/x' }} }});"),
        // Codex round 7: a callback the walker does not read as the body, and a hook no test is
        // governed by (accepted from syntax, refused because the walk recorded no host).
        format!("{route} beforeEach(...[], () => {{ routeMock.value = {{ path: '/x' }} }});"),
        format!("{route} beforeEach(() => {{}}, () => {{ routeMock.value = {{ path: '/x' }} }});"),
        format!("{route} test('t', () => {{}}, () => {{ routeMock.value = {{ path: '/x' }} }});"),
        format!("{route} describe('suite', () => {{ beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}) }});"),
        // A redeclared binding is read from its first declaration only, written or not.
        "var run = () => {}; var run = () => test('hidden', () => {}); mockNuxtImport('useRun', () => () => run);".to_owned(),
        // Codex round 8: a `var` hoisted out of a `catch` carries the thrown value.
        "const state = { run: () => {} }; mockNuxtImport('useRun', () => () => state.run); beforeEach(() => { try { throw () => test('hidden', () => {}) } catch (error) { var carried = error } state.run = carried as () => void });".to_owned(),
        "const state = { run: () => {} }; mockNuxtImport('useRun', () => () => state.run); beforeEach(() => { try { throw () => test('hidden', () => {}) } catch (error) { try { throw 0 } catch { var carried = error } } state.run = carried as () => void });".to_owned(),
        // Codex round 6: an exported binding escapes without a reference.
        "export const routeMock = { value: { path: '/' } }; mockNuxtImport('useRoute', () => () => routeMock.value); beforeEach(() => { routeMock.value = { path: '/x' } });".to_owned(),
        format!("{route} beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}); export {{ routeMock }};"),
        format!("{route} beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}); export default routeMock;"),
        format!("{route} beforeEach(() => {{ routeMock.value = {{ path: '/x' }}; eval(\"routeMock.value = {{ run: () => test('hidden', () => {{}}) }}\") }});"),
        // Codex round 5: a wrapped callee, and a hook after an early exit in its suite.
        format!("{route} (beforeEach as typeof beforeEach)(() => {{ routeMock.value = {{ path: '/x' }} }});"),
        format!("{route} (beforeEach)(() => {{ routeMock.value = {{ path: '/x' }} }});"),
        format!("{route} describe('suite', () => {{ test('first', () => {{}}); return; beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}) }});"),
        format!("{route} describe('suite', () => {{ if (routeMock.value) {{ throw new Error('x') }} beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}) }});"),
        format!("{route} describe('suite', () => {{ class Kind {{ static {{ throw new Error('x') }} }} beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}) }});"),
        format!("{route} void beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }});"),
        format!("{route} if (beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }})) {{}}"),
        format!("{route} if (describe('suite', () => {{ beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}) }})) {{}}"),
        format!("{route} await beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }});"),
        format!("{route} const hooks = () => {{ beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}) }}; hooks();"),
        format!("{route} describe.skip('suite', () => {{ beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}) }});"),
        format!("{route} const run = () => describe('suite', () => {{ beforeEach(() => {{ routeMock.value = {{ path: '/x' }} }}) }}); run();"),
        format!("{route} routeMock.value = {{ path: '/x' }};"),
        format!("{route} const reset = () => {{ routeMock.value = {{ path: '/x' }} }}; beforeEach(reset);"),
        "const routeMock = { value: { path: '/' } }; mockNuxtImport('useRoute', () => () => routeMock.value); const afterAll = (run: () => void) => run(); afterAll(() => { routeMock.value = { path: '/x' } });".to_owned(),
        // Codex round 1, reproduced before fixing.
        "var state = { run: () => {} }; beforeEach(() => { state.marker = 1 }); var state = { run: () => test('hidden', () => {}) }; mockNuxtImport('useRun', () => () => state.run);".to_owned(),
        "let state = { run: () => {} }; beforeEach(() => { const alias = (state = { run: () => {} }); alias.run = () => test('hidden', () => {}) }); mockNuxtImport('useRun', () => () => state.run);".to_owned(),
        "const state = { run: () => {}, set<T>(fn: () => void) { this.run = fn } }; beforeEach(() => { state.run = () => {}; (state.set<unknown>)(() => test('hidden', () => {})) }); mockNuxtImport('useRun', () => () => state.run);".to_owned(),
    ] {
        assert!(!run(&refused), "{refused}");
    }
}

/// An accepted write sits where the walker records it: the suite hook is in the context of the test
/// it governs, and a free hook counts only when the project enables globals.
#[test]
fn an_accepted_written_binding_write_is_in_the_compared_context() {
    // Each accepted placement: the hook must be recorded in the governed test's context, or the
    // proof is accepting a write the comparison cannot see.
    let mock = "import { mockNuxtImport } from '@nuxt/test-utils/runtime'; const routeMock = { value: { path: '/' } }; mockNuxtImport('useRoute', () => () => routeMock.value);";
    let check = |imports: &str, body: &str, globals: Globals| {
        let caller = format!("{imports} {mock} {body}");
        let extraction = Site::new(&[]).extract_with(&caller, globals);
        let tests: Vec<_> = extraction
            .tests
            .iter()
            .filter(|case| case.kind == crate::model::DeclarationKind::Test)
            .collect();
        assert_eq!(tests.len(), 1, "{body}");
        assert!(!tests[0].context.unresolved, "{body}");
        assert_eq!(
            tests[0].context.hooks.len(),
            1,
            "{body}: {:?}",
            tests[0].context.hooks
        );
    };
    let imports = "import { test, expect, beforeEach, describe, suite } from 'vitest';";
    let write = "beforeEach(() => { routeMock.value = { path: '/x' } });";
    let reads = "test('reads', () => { expect(routeMock.value.path).toBe('/x') });";
    check(imports, &format!("{write} {reads}"), Globals::Disabled);
    check(
        imports,
        &format!("describe('s', () => {{ {write} {reads} }});"),
        Globals::Disabled,
    );
    check(
        imports,
        &format!("describe('s', () => {{ describe('t', () => {{ {{ {write} }} {reads} }}) }});"),
        Globals::Disabled,
    );
    check(
        imports,
        &format!("suite('s', () => {{ {write} {reads} }});"),
        Globals::Disabled,
    );
    check(
        "import { test, expect, beforeEach as before } from 'vitest';",
        &format!("before(() => {{ routeMock.value = {{ path: '/x' }} }}); {reads}"),
        Globals::Disabled,
    );
    check(
        "",
        &format!("describe('s', () => {{ {write} {reads} }});"),
        Globals::Enabled,
    );
}

/// An imported object changed through a member is not the export the proof read.
#[test]
fn an_imported_binding_written_by_the_caller_is_not_placed() {
    let site = Site::new(&[
        (
            "setup.ts",
            "export function setup(value?: any) { value?.callback?.() }",
        ),
        ("config.ts", "export const config = { callback() {} }"),
    ]);
    let run = |write: &str| {
        let caller = format!(
            "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; import {{ config }} from './config'; {write} setup(config); {TWO_TESTS}"
        );
        site.extract(&caller)
    };
    assert!(run("config.callback = () => test('hidden', () => {});").incomplete);
    assert!(!run("").incomplete);
}

/// Framework setup and helper setup are recorded apart, so where a helper call stands among the
/// framework statements is part of what it stands on.
#[test]
fn a_helper_call_keeps_its_place_among_framework_setup() {
    let site = Site::new(&[("setup.ts", "export function setup() { return 1 }")]);
    let setup_of = |body: &str| {
        let caller = format!(
            "import {{ test, expect, vi }} from 'vitest'; import {{ setup }} from './setup'; {body} test('a', () => {{ expect(1).toBe(1) }});"
        );
        let extraction = site.extract(&caller);
        assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
        extraction.tests[0].context.module_setup.clone()
    };
    let before = setup_of("vi.stubGlobal('mode', 'changed'); setup();");
    let after = setup_of("setup(); vi.stubGlobal('mode', 'changed');");
    assert_eq!(before.len(), 2);
    assert_ne!(before, after);
}

/// A mocked framework runtime contributes its mock, not its real body.
#[test]
fn a_mocked_register_endpoint_is_not_the_real_one() {
    let caller = format!(
        "import {{ test, expect, vi, beforeEach }} from 'vitest'; import {{ registerEndpoint }} from '@nuxt/test-utils/runtime'; vi.mock('@nuxt/test-utils/runtime', () => ({{ registerEndpoint: () => beforeEach(() => {{}}) }})); registerEndpoint('/x', () => ({{}})); {TWO_TESTS}"
    );
    assert!(with_helpers(&[], &caller).incomplete);
}

/// `import.meta` stands for the file it is written in.
#[test]
fn setup_identity_includes_the_file_import_meta_stands_for() {
    let site = Site::new(&[(
        "setup.ts",
        "export function setup(value?: unknown) { return value }",
    )]);
    let setup_of = |directory: &str, import: &str| {
        let caller = format!(
            "import {{ test, expect }} from 'vitest'; import {{ setup }} from '{import}'; setup(new URL('./fixture.json', import.meta.url)); test('a', () => {{ expect(1).toBe(1) }});"
        );
        let extraction = site.extract_in(directory, &caller, Globals::Disabled);
        assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
        extraction.tests[0].context.module_setup.clone()
    };
    assert_ne!(setup_of("a", "../setup"), setup_of("b", "../setup"));
}

/// A handler's own parameters and locals are part of its text, not names to place.
#[test]
fn setup_identity_does_not_refuse_names_declared_inside_what_it_already_holds() {
    let site = Site::new(&[(
        "setup.ts",
        "export function setup(value?: unknown) { return value }",
    )]);
    let run = |body: &str| {
        let caller = format!(
            "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; {body}; setup(handler); {TWO_TESTS}"
        );
        site.extract(&caller)
    };
    for handler in [
        "function handler(event) { return event.body }",
        "const handler = (event) => { const body = event.body; return body }",
        "class handler { run(event) { try { return event } catch (error) { return error } } }",
    ] {
        let extraction = run(handler);
        assert!(
            !extraction.incomplete,
            "{handler}: {:?}",
            extraction.diagnostics
        );
    }
}

/// `mockNuxtImport` is modelled only as a direct call; other spellings leave the file unresolved.
#[test]
fn mock_nuxt_import_through_a_namespace_or_call_is_not_modelled() {
    let namespace = format!(
        "import {{ test, expect }} from 'vitest'; import * as utils from '@nuxt/test-utils/runtime'; utils.mockNuxtImport('useRoute', () => () => 1); {TWO_TESTS}"
    );
    assert!(with_helpers(&[], &namespace).incomplete);
    let applied = format!(
        "import {{ test, expect }} from 'vitest'; import {{ mockNuxtImport }} from '@nuxt/test-utils/runtime'; mockNuxtImport.call(null, 'useRoute', () => () => 1); {TWO_TESTS}"
    );
    assert!(with_helpers(&[], &applied).incomplete);
}

/// A type used in an argument is erased and does not need placing.
#[test]
fn setup_identity_ignores_names_used_only_as_types() {
    let site = Site::new(&[(
        "setup.ts",
        "export function setup(value?: unknown) { return value }",
    )]);
    let caller = format!(
        "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; interface Options {{ mode: string }}; type Handler = (event: Options) => void; setup({{ mode: 'a' }} as Options); setup((event: Options): Handler => (() => {{}}) as Handler); {TWO_TESTS}"
    );
    let extraction = site.extract(&caller);
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
}

/// A loop variable takes its value from the iterable, which the declaration does not show.
#[test]
fn a_loop_variable_read_by_a_setup_call_is_not_placed() {
    let site = Site::new(&[(
        "setup.ts",
        "export function setup(callback?: any) { callback?.() }",
    )]);
    let run = |body: &str| {
        site.extract(&format!(
            "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; {body} {TWO_TESTS}"
        ))
    };
    assert!(run("for (const cb of [() => test('hidden', () => {})]) setup(cb);").incomplete);
    assert!(run("for (const mode in { fast: 1 }) setup(mode);").incomplete);
    assert!(!run("const mode = 'fast'; setup(mode);").incomplete);
}

/// A call the walk may run any number of times is not one fixed piece of setup.
#[test]
fn setup_under_control_flow_is_not_recorded() {
    let site = Site::new(&[(
        "setup.ts",
        "export function setup(value?: unknown) { return value }",
    )]);
    let run = |body: &str| {
        site.extract(&format!(
            "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; {body} {TWO_TESTS}"
        ))
    };
    for conditional in [
        "if (process.env.CI) setup(1);",
        "if (process.env.CI) { setup(1) } else { setup(2) }",
        "for (let i = 0; i < 3; i++) setup(i);",
        "while (process.env.CI) { setup(1) }",
        "switch (process.env.CI) { case '1': setup(1) }",
        "try { setup(1) } catch { setup(2) }",
        "for (const mode of process.env.MODES) setup(mode);",
        "for (const mode of []) setup(1);",
        "function init() { setup(1) } if (process.env.CI) init();",
        "function init() { setup(1) } function outer() { init() } while (process.env.CI) outer();",
    ] {
        assert!(run(conditional).incomplete, "{conditional}");
    }
    assert!(!run("setup(1);").incomplete);
    assert!(!run("for (const mode of ['a', 'b']) setup(1);").incomplete);
    assert!(!run("function init() { setup(1) } init();").incomplete);
}

/// Framework values handed to a helper (`vi.fn()`, `expect`) are placed by name.
#[test]
fn setup_identity_places_names_taken_from_vitest() {
    let site = Site::new(&[(
        "setup.ts",
        "export function setup(value?: unknown) { return value }",
    )]);
    let extraction = site.extract(&format!(
        "import {{ test, expect, vi }} from 'vitest'; import {{ setup }} from './setup'; setup({{ onError: vi.fn(), check: expect }}); {TWO_TESTS}"
    ));
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
}

/// A setup call inside an imported runner inherits the control flow the runner is called under.
#[test]
fn setup_inside_a_conditionally_called_runner_is_not_recorded() {
    let site = Site::new(&[
        ("setup.ts", "export function setup(value?: unknown) { return value }"),
        (
            "runner.ts",
            "import { test, expect } from 'vitest'; import { setup } from './setup'; export function run() { setup(1); test('inside', () => { expect(1).toBe(1) }); }",
        ),
    ]);
    let run = |call: &str| {
        site.extract(&format!(
            "import {{ test, expect }} from 'vitest'; import {{ run }} from './runner'; {call} {TWO_TESTS}"
        ))
    };
    let unconditional = run("run();");
    assert!(!unconditional.incomplete, "{:?}", unconditional.diagnostics);
    assert!(run("if (process.env.CI) run();").incomplete);
}

/// Framework setup counted in the caller reaches setup calls inside an imported runner.
#[test]
fn a_runner_setup_call_keeps_the_callers_place_among_framework_setup() {
    let site = Site::new(&[
        ("setup.ts", "export function setup() { return 1 }"),
        (
            "runner.ts",
            "import { test, expect } from 'vitest'; import { setup } from './setup'; export function run() { setup(); test('inside', () => { expect(1).toBe(1) }); }",
        ),
    ]);
    let setup_of = |body: &str| {
        let extraction = site.extract(&format!(
            "import {{ test, expect, vi }} from 'vitest'; import {{ run }} from './runner'; {body} {TWO_TESTS}"
        ));
        assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
        extraction.tests[0].context.module_setup.clone()
    };
    assert_ne!(
        setup_of("vi.stubGlobal('mode', 'changed'); run();"),
        setup_of("run(); vi.stubGlobal('mode', 'changed');")
    );
}

/// A suite instance walked once for a table the analyzer cannot read runs once per row.
#[test]
fn setup_inside_an_unreadable_each_table_is_not_recorded() {
    let site = Site::new(&[(
        "setup.ts",
        "export function setup(value?: unknown) { return value }",
    )]);
    let run = |suite: &str| {
        site.extract(&format!(
            "import {{ test, expect, describe }} from 'vitest'; import {{ setup }} from './setup'; {suite} {TWO_TESTS}"
        ))
    };
    assert!(
        run("describe.each(getRows())('row', () => { setup(1); test('x', () => {}); });")
            .incomplete
    );
    assert!(!run("describe('plain', () => { setup(1); test('x', () => {}); });").incomplete);
}

/// A setup call inside a local helper stands where the helper is called, not where it is declared;
/// a loop the walk does not unroll is not recorded once per element.
#[test]
fn setup_in_local_helpers_and_loops_is_placed_where_it_runs() {
    let site = Site::new(&[(
        "setup.ts",
        "export function setup(value?: unknown) { return value }",
    )]);
    let run = |body: &str| {
        site.extract(&format!(
            "import {{ test, expect, vi }} from 'vitest'; import {{ setup }} from './setup'; {body} {TWO_TESTS}"
        ))
    };
    let before = run("function init() { setup(1) } vi.stubGlobal('m', 1); init();");
    let after = run("function init() { setup(1) } init(); vi.stubGlobal('m', 1);");
    assert!(!before.incomplete && !after.incomplete);
    assert_ne!(
        before.tests[0].context.module_setup,
        after.tests[0].context.module_setup
    );
    for unrolled_not in [
        "for (const m of [a(), b()]) setup(1);",
        "for (const m of [...xs]) setup(1);",
        "for (const [m] of [[1], [2]]) setup(1);",
    ] {
        assert!(run(unrolled_not).incomplete, "{unrolled_not}");
    }
    assert!(!run("for (const m of ['a', 'b']) setup(1);").incomplete);
}

/// Names read by a destructuring pattern (a default, a computed key) are part of the identity.
#[test]
fn setup_identity_includes_what_a_destructuring_pattern_reads() {
    let site = Site::new(&[(
        "setup.ts",
        "export function setup(value?: unknown) { return value }",
    )]);
    let setup_of = |fallback: &str, pattern: &str| {
        let extraction = site.extract(&format!(
            "import {{ test, expect }} from 'vitest'; import {{ setup }} from './setup'; const fallback = '{fallback}'; {pattern} setup(value); {TWO_TESTS}"
        ));
        assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
        extraction.tests[0].context.module_setup.clone()
    };
    for pattern in [
        "const { value = fallback } = {};",
        "const [value = fallback] = [];",
        "const { [fallback]: value } = { fast: 1, slow: 2 };",
    ] {
        assert_ne!(
            setup_of("fast", pattern),
            setup_of("slow", pattern),
            "{pattern}"
        );
        assert_eq!(
            setup_of("fast", pattern),
            setup_of("fast", pattern),
            "{pattern}"
        );
    }
}

/// A refused endpoint registration says why: mocked, under control flow, or unplaceable.
#[test]
fn register_endpoint_refusals_name_their_cause() {
    let call = |before: &str, statement: &str| {
        let caller = format!(
            "import {{ test, expect, vi }} from 'vitest'; import {{ registerEndpoint }} from '@nuxt/test-utils/runtime'; {before} {statement} {TWO_TESTS}"
        );
        with_helpers(&[], &caller)
    };
    let mocked = call(
        "vi.mock('@nuxt/test-utils/runtime', () => ({ registerEndpoint: () => 1 }));",
        "registerEndpoint('/x', () => ({}));",
    );
    assert!(
        mocked.reasons.contains(&ExclusionReason::RunnerMocked),
        "{:?}",
        mocked.reasons
    );
    let conditional = call(
        "",
        "if (process.env.CI) { registerEndpoint('/x', () => ({})); }",
    );
    assert!(conditional.incomplete);
    assert!(
        conditional
            .diagnostics
            .iter()
            .any(|text| text.contains("control flow")),
        "{:?}",
        conditional.diagnostics
    );
    let unplaceable = call("enum Mode { A }", "registerEndpoint('/x', () => Mode);");
    assert!(
        unplaceable
            .diagnostics
            .iter()
            .any(|text| text.contains("cannot place")),
        "{:?}",
        unplaceable.diagnostics
    );
}
