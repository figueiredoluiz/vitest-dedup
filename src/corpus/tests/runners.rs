//! Final corpus and finding outcomes on the same permanent source used by the runtime oracle.
use super::*;
use crate::model::Rule;
use std::path::PathBuf;

/// Erased declarations cannot stand in for initialized factory parameters.
#[test]
fn returned_factory_erased_reads_stay_opaque() {
    let results = corpora(
        &[
            (
                "factory.ts",
                include_str!("../../../fixtures/oracle/factory-erased-read/factory.ts"),
            ),
            (
                "enum.test.ts",
                include_str!("../../../fixtures/oracle/factory-erased-read/enum.test.ts"),
            ),
            (
                "interface.test.ts",
                include_str!("../../../fixtures/oracle/factory-erased-read/interface.test.ts"),
            ),
            (
                "type.test.ts",
                include_str!("../../../fixtures/oracle/factory-erased-read/type.test.ts"),
            ),
            (
                "local.test.ts",
                include_str!("../../../fixtures/oracle/factory-erased-read/local.test.ts"),
            ),
            (
                "control.test.ts",
                include_str!("../../../fixtures/oracle/factory-erased-read/control.test.ts"),
            ),
        ],
        &[
            "enum.test.ts",
            "interface.test.ts",
            "type.test.ts",
            "local.test.ts",
            "control.test.ts",
        ],
    );
    assert_eq!(
        results
            .iter()
            .map(|result| result.tests.len())
            .collect::<Vec<_>>(),
        [3, 3, 3, 4, 2]
    );
    assert_all_wrappers_refused(&results[..3]);
    assert!(results[3].tests.iter().all(|case| case.exclusion.is_some()));
    for (path, result) in [
        "enum.test.ts",
        "interface.test.ts",
        "type.test.ts",
        "local.test.ts",
    ]
    .into_iter()
    .zip(&results[..4])
    {
        let outcome = owned_findings(path, result.clone());
        assert_rule_absent(outcome, Rule::DuplicateTest);
    }
    assert!(results[4].tests.iter().all(|case| case.exclusion.is_none()));
    let retained = owned_findings("control.test.ts", results[4].clone());
    assert_eq!(
        retained
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateTest)
            .count(),
        1
    );
}

/// Copy costs refuse a repeated large modifier before preparation, retaining the small control.
#[test]
fn returned_modifier_copies_obey_the_work_bound() {
    assert_factory_copy_bound(
        include_str!("../../../fixtures/oracle/factory-modifier-work/factory.ts"),
        include_str!("../../../fixtures/oracle/factory-modifier-work/large.test.ts"),
        include_str!("../../../fixtures/oracle/factory-modifier-work/control.test.ts"),
        81,
    );
}

/// Full inventories are reserved before repeated frame and normalization copies.
#[test]
fn returned_inventory_copies_obey_the_work_bound() {
    assert_factory_copy_bound(
        include_str!("../../../fixtures/oracle/factory-inventory-work/factory.ts"),
        include_str!("../../../fixtures/oracle/factory-inventory-work/large.test.ts"),
        include_str!("../../../fixtures/oracle/factory-inventory-work/control.test.ts"),
        81,
    );
}

/// Parameter path storage is reserved before a large flattened selection is allocated.
#[test]
fn returned_parameter_paths_obey_copy_work_bound() {
    assert_factory_copy_bound(
        include_str!("../../../fixtures/oracle/factory-parameter-work/factory.ts"),
        include_str!("../../../fixtures/oracle/factory-parameter-work/large.test.ts"),
        include_str!("../../../fixtures/oracle/factory-parameter-work/control.test.ts"),
        2,
    );
}

/// Multiple parameters consume one allowance rather than restarting copy work for each pattern.
#[test]
fn returned_parameter_paths_share_copy_work_bound() {
    assert_factory_copy_bound(
        include_str!("../../../fixtures/oracle/factory-parameter-work/factory.ts"),
        include_str!("../../../fixtures/oracle/factory-parameter-work/split.test.ts"),
        include_str!("../../../fixtures/oracle/factory-parameter-work/control.test.ts"),
        2,
    );
}

/// Refuse a costly caller without losing the small caller's duplicate positive.
fn assert_factory_copy_bound(factory: &str, large: &str, control: &str, refused: usize) {
    let results = corpora(
        &[
            ("factory.ts", factory),
            ("large.test.ts", large),
            ("control.test.ts", control),
        ],
        &["large.test.ts", "control.test.ts"],
    );
    assert_eq!(results[0].tests.len(), refused);
    assert_all_wrappers_refused(&results[..1]);
    assert_eq!(results[1].tests.len(), 2);
    assert!(results[1].tests.iter().all(|case| case.exclusion.is_none()));
    let outcome = owned_findings("control.test.ts", results[1].clone());
    assert_eq!(
        outcome
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateTest)
            .count(),
        1
    );
}

/// Suite-local captures obey invocation-time initialization while ready captures stay eligible.
#[test]
fn returned_factory_enclosing_capture_requires_initialization() {
    let late = include_str!("../../../fixtures/oracle/factory-suite-capture/late.test.ts");
    let ready = include_str!("../../../fixtures/oracle/factory-suite-capture/ready.test.ts");
    let results = corpora(
        &[("late.test.ts", late), ("ready.test.ts", ready)],
        &["late.test.ts", "ready.test.ts"],
    );
    assert_eq!(results[0].tests.len(), 3);
    assert_eq!(
        results[0].tests[0].exclusion,
        Some(Exclusion::RegisteredInsideFunction)
    );
    assert!(results[0].tests[1..]
        .iter()
        .all(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)));
    assert_eq!(results[1].tests.len(), 2);
    assert!(results[1].tests.iter().all(|case| case.exclusion.is_none()));
    assert_rule_absent(
        owned_outcome(vec![
            ("late.test.ts", results[0].clone()),
            ("ready.test.ts", results[1].clone()),
        ]),
        Rule::CrossFileDuplicate,
    );
}

/// A real parsed caller and its dependency files before batch planning.
struct ProofProject {
    root: tempfile::TempDir,
    path: PathBuf,
    source: String,
    parsed: crate::lang::Parsed,
    found: crate::inventory::Inventory,
    run: Run,
}

impl ProofProject {
    /// Parse the permanent oracle input with the same inventory the planner receives.
    fn new(caller: &str, source: &str, dependencies: &[(&str, &str)]) -> Self {
        let root = tempfile::tempdir().unwrap();
        for (name, content) in dependencies.iter().copied().chain([(caller, source)]) {
            fs::write(root.path().join(name), content).unwrap();
        }
        let path = root.path().join(caller);
        let parsed = crate::lang::parse(&path, source).unwrap();
        let mut run = Run::default();
        let found = crate::inventory::inventory(
            &path,
            source,
            &parsed,
            crate::inventory::Setting {
                dialect: &DIALECT,
                globals: false,
                root: root.path(),
            },
            &mut run.modules,
        );
        Self {
            root,
            path,
            source: source.to_owned(),
            parsed,
            found,
            run,
        }
    }

    /// Run the production planner over the current caller inventory.
    fn batch(&mut self) -> crate::corpus::runners::Batch {
        crate::corpus::runners::expand(
            &self.path,
            &self.source,
            &self.parsed,
            &self.found,
            Setting {
                dialect: &DIALECT,
                globals: false,
                root: self.root.path(),
                benchmarks: false,
            },
            &mut self.run,
        )
    }
}

/// Scheduling metadata follows selected derived callbacks.
#[test]
fn derived_handoff_scheduling_uses_selected_callbacks() {
    let factory = include_str!("../../../fixtures/oracle/factory-proof-handoff/factory.ts");
    for (caller, safe) in [
        (
            include_str!("../../../fixtures/oracle/factory-proof-handoff/async.test.ts"),
            false,
        ),
        (
            include_str!("../../../fixtures/oracle/factory-proof-handoff/sync.test.ts"),
            true,
        ),
    ] {
        let mut project = ProofProject::new("case.test.ts", caller, &[("factory.ts", factory)]);
        let batch = project.batch();
        assert!(batch.refused.is_empty());
        assert_eq!(batch.calls.len(), 2);
        let invoked: Vec<_> = batch
            .calls
            .iter()
            .filter(|call| !call.tests.is_empty())
            .collect();
        assert_eq!(invoked.len(), 1);
        assert_eq!(invoked[0].tests.len(), 1);
        assert_eq!(invoked[0].scheduling_safe, safe);
    }
}

/// A completed supplier batch protects caller readers while retaining independent duplicates.
#[test]
fn derived_handoff_reaches_caller_mutation_analysis() {
    let source = include_str!("../../../fixtures/oracle/factory-proof-handoff/local.test.ts");
    let guarded = corpora(&[("local.test.ts", source)], &["local.test.ts"]).remove(0);
    assert_eq!(guarded.tests.len(), 8);
    assert!(guarded.tests.iter().all(|case| case.exclusion.is_some()));
    assert_rule_absent(
        owned_findings("local.test.ts", guarded),
        Rule::DuplicateTest,
    );
    let mut project = ProofProject::new("local.test.ts", source, &[]);
    assert_eq!(project.found.escapes.len(), 1);
    // Exercise the consumer boundary after its supplier has discharged the escape precondition.
    // This modifies only the test inventory; production admission keeps the existing guard.
    project.found.escapes.clear();
    let batch = project.batch();
    assert!(batch.refused.is_empty());
    assert_eq!(batch.calls.len(), 2);
    assert_eq!(batch.mutation_sites.len(), 1);
    let sites = project
        .found
        .sites
        .iter()
        .filter(|site| {
            matches!(
                site.kind,
                crate::inventory::SiteKind::Registers(crate::framework::Registers::Test)
            )
        })
        .map(|site| site.node)
        .collect();
    let registrations = batch.calls.iter().map(|call| call.call).collect();
    batch.promote_inventory(&project.parsed, &mut project.found);
    let frame = crate::corpus::runners::Frame {
        mutation_sites: Default::default(),
        inventory: Some(Box::new(project.found.clone())),
        function: project.parsed.tree.root(),
        bindings: Default::default(),
        values: Default::default(),
        sites,
        registrations,
        derived: Default::default(),
    };
    let corpus = crate::corpus::read_parsed(
        &project.path,
        &project.source,
        Setting {
            dialect: &DIALECT,
            globals: false,
            root: project.root.path(),
            benchmarks: false,
        },
        &mut project.run,
        &project.parsed,
        Some(frame),
    )
    .unwrap();
    assert_eq!(corpus.tests.len(), 4, "{:?}", corpus.tests);
    let readers: Vec<_> = corpus
        .tests
        .iter()
        .filter(|case| {
            case.title
                .as_deref()
                .is_some_and(|title| title.starts_with("reader"))
        })
        .collect();
    assert_eq!(readers.len(), 2);
    assert!(
        readers
            .iter()
            .all(|case| case.exclusion == Some(Exclusion::OrderDependentState)),
        "{readers:?}"
    );
    assert_eq!(
        corpus
            .tests
            .iter()
            .filter(|case| case.exclusion.is_none())
            .count(),
        2
    );
    let outcome = owned_findings("local.test.ts", corpus);
    assert_eq!(
        outcome
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateTest)
            .count(),
        1,
        "{:?}",
        outcome.findings
    );
}

/// Never-invoked async registrations do not invalidate a synchronous runner's hook proof.
#[test]
fn unused_runner_callbacks_preserve_the_registered_hook_positive() {
    let helper = include_str!("../../../fixtures/oracle/hook-scheduling/retained.ts");
    let caller = include_str!("../../../fixtures/oracle/hook-scheduling/retained.test.ts");
    let corpus = corpora(
        &[("retained.ts", helper), ("retained.test.ts", caller)],
        &["retained.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 1);
    assert!(corpus.tests.iter().all(|test| test.exclusion.is_none()));
    assert_eq!(corpus.hooks.declarations.len(), 2);
    let findings = owned_findings("retained.test.ts", corpus);
    assert_eq!(
        findings
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::RedundantHook)
            .count(),
        1
    );
}

/// Scheduling uncertainty withholds hook deletion without dropping imported tests or setup context.
#[test]
fn asynchronous_runner_keeps_tests_and_distinct_setup_contexts() {
    let helper = include_str!("../../../fixtures/oracle/hook-scheduling/runner.ts");
    let caller = include_str!("../../../fixtures/oracle/hook-scheduling/imported.test.ts");
    let other = include_str!("../../../fixtures/oracle/hook-scheduling/other.test.ts");
    let mut results = corpora(
        &[
            ("runner.ts", helper),
            ("imported.test.ts", caller),
            ("other.test.ts", other),
        ],
        &["imported.test.ts", "other.test.ts"],
    );
    let mut corpus = results.remove(0);
    let other = results.remove(0);
    assert_eq!(corpus.tests.len(), 2);
    assert_eq!(other.tests.len(), 2);
    assert_ne!(corpus.tests[0].key, other.tests[0].key);
    corpus.tests.extend(other.tests);
    assert_eq!(corpus.tests.len(), 4, "{:?}", corpus.tests);
    assert!(corpus.tests.iter().all(|case| case.exclusion.is_none()));
    assert_eq!(corpus.hooks.declarations.len(), 2);
    assert!(corpus
        .hooks
        .declarations
        .iter()
        .all(|hook| hook.key.is_none()));
    let duplicates: Vec<_> = findings_in(corpus, "imported.test.ts")
        .into_iter()
        .filter(|finding| finding.rule == Rule::DuplicateTest)
        .collect();
    assert_eq!(duplicates.len(), 2, "{duplicates:?}");
    assert!(duplicates.iter().all(|finding| finding.titles.len() == 2));
}

/// Imported runtime callbacks constrain hooks reached directly and through a nested helper.
#[test]
fn runner_scheduling_evidence_reaches_the_callers_hook_chains() {
    let hooks = "import { beforeEach } from 'vitest'; let ready = 0; export function setup() { beforeEach(() => { ready = 1; }); beforeEach(() => { ready = 1; }); }";
    for asynchronous in [false, true] {
        for nested in [false, true] {
            let helper = format!("import {{ beforeEach, test, expect }} from 'vitest'; import {{ setup }} from './hooks'; let ready = 0; export function install() {{ {} test('case', {} () => {{ expect(true).toBe(true); }}); }}", if nested { "setup();" } else { "beforeEach(() => { ready = 1; }); beforeEach(() => { ready = 1; });" }, if asynchronous { "async" } else { "" });
            let caller = "import { install } from './runner'; install();";
            let corpus = corpora(
                &[
                    ("hooks.ts", hooks),
                    ("runner.ts", &helper),
                    ("case.test.ts", caller),
                ],
                &["case.test.ts"],
            )
            .remove(0);
            assert_eq!(corpus.tests.len(), 1);
            assert!(
                corpus.tests.iter().all(|case| case.exclusion.is_none()),
                "nested={nested} async={asynchronous}: {:?}",
                corpus.tests
            );
            assert_eq!(corpus.hooks.declarations.len(), 2);
            assert_eq!(
                corpus
                    .hooks
                    .declarations
                    .iter()
                    .filter(|hook| hook.key.is_some())
                    .count(),
                if asynchronous { 0 } else { 2 },
                "nested={nested}"
            );
        }
    }
}

/// Imported setup participates in chains with direct or expanded tests and preserves diagnostics.
#[test]
fn imported_setup_chains_cover_registration_shapes() {
    for own_test in [false, true] {
        for separate in [false, true] {
            let helper = format!("import {{ beforeEach, test, expect }} from 'vitest'; let ready = 0; export function install() {{ beforeEach(() => {{ ready = 1; }}); beforeEach(() => {{ ready = 1; }}); {} }} export function cases() {{ test('case', () => {{ expect(ready).toBe(1); }}); }}", if own_test { "test('inside', () => { expect(ready).toBe(1); });" } else { "" });
            let caller = format!("import {{ install, cases }} from './runner'; import {{ test, expect }} from 'vitest'; install(); {}", if separate { "cases();" } else { "test('direct', () => { expect(2).toBe(2); });" });
            let dir = tempfile::tempdir().unwrap();
            let caller = if separate {
                caller
            } else {
                caller.replace("install, cases", "install")
            };
            std::fs::write(dir.path().join("runner.ts"), helper).unwrap();
            let mut engine = Engine::default();
            for name in ["a.test.ts", "b.test.ts"] {
                let path = dir.path().join(name);
                std::fs::write(&path, &caller).unwrap();
                let read = engine.read_with_sources(
                    &path,
                    std::path::Path::new(name),
                    &caller,
                    Setting {
                        dialect: &DIALECT,
                        globals: false,
                        root: dir.path(),
                        benchmarks: false,
                    },
                    true,
                    Vec::new(),
                );
                assert!(!read.partial);
            }
            assert_eq!(engine.tests, if own_test { 4 } else { 2 });
            let diagnostics = engine.take_diagnostic_hooks();
            assert_eq!(diagnostics.len(), 2);
            assert!(diagnostics
                .iter()
                .all(|(_, hook)| hook.effect_key.is_some()));
            let outcome = crate::rules::run_with_hooks(
                &engine.files,
                &engine.suites,
                &engine.hooks,
                &crate::config::Config::load(dir.path(), Default::default()).unwrap(),
                &[],
            );
            assert_eq!(
                outcome
                    .findings
                    .iter()
                    .filter(|finding| finding.rule == Rule::RedundantHook)
                    .count(),
                1
            );
        }
    }
}

/// Dynamic registration remains visibly partial even when there is no static test node to count.
#[test]
fn direct_eval_without_a_static_test_site_is_not_a_complete_empty_read() {
    let source =
        include_str!("../../../fixtures/recall/runner-eval-binding-guard/eval-only.test.ts");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("eval-only.test.ts");
    std::fs::write(&path, source).unwrap();
    let mut engine = Engine::default();
    let read = engine.read_with_sources(
        &path,
        std::path::Path::new("eval-only.test.ts"),
        source,
        Setting {
            dialect: &DIALECT,
            globals: false,
            root: dir.path(),
            benchmarks: false,
        },
        false,
        Vec::new(),
    );
    assert_eq!(engine.tests, 0);
    assert!(read.partial);
    let excluded = read.excluded_file.unwrap();
    assert_eq!(
        excluded.reason,
        crate::model::ExclusionReason::UnresolvedContext
    );
    assert!(excluded.detail.contains("eval"));
}

/// Direct eval invalidates captured-input substitution; an ordinary member named eval does not.
#[test]
fn owned_eval_binding_guard_retains_the_safe_imported_runner() {
    let unsafe_helper =
        include_str!("../../../fixtures/recall/runner-eval-binding-guard/unsafe.ts");
    let safe_helper = include_str!("../../../fixtures/recall/runner-eval-binding-guard/safe.ts");
    let unsafe_caller =
        include_str!("../../../fixtures/recall/runner-eval-binding-guard/unsafe.test.ts");
    let safe_caller =
        include_str!("../../../fixtures/recall/runner-eval-binding-guard/safe.test.ts");
    let mut results = corpora(
        &[
            ("unsafe.ts", unsafe_helper),
            ("safe.ts", safe_helper),
            ("unsafe.test.ts", unsafe_caller),
            ("safe.test.ts", safe_caller),
        ],
        &["unsafe.test.ts", "safe.test.ts"],
    );
    let safe = results.pop().unwrap();
    let refused = results.pop().unwrap();
    assert_eq!(refused.tests.len(), 1);
    assert!(refused.tests.iter().all(|case| case.exclusion.is_some()));
    assert!(refused.helper_sources.is_empty());
    assert!(findings_in(refused, "unsafe.test.ts").is_empty());
    assert_eq!(safe.tests.len(), 2);
    assert!(safe.tests.iter().all(|case| case.exclusion.is_none()));
    let duplicates: Vec<_> = findings_in(safe, "safe.test.ts")
        .into_iter()
        .filter(|finding| finding.rule == Rule::DuplicateTest)
        .collect();
    assert_eq!(duplicates.len(), 1, "{duplicates:?}");
    assert_eq!(duplicates[0].titles, ["safe first", "safe second"]);
}

/// Table callbacks keep unbound inputs when direct eval defeats the readonly binding proof.
#[test]
fn owned_eval_row_guard_retains_unbound_dimensions_and_safe_row_substitution() {
    let unsafe_rows =
        include_str!("../../../fixtures/recall/runner-eval-binding-guard/rows-unsafe.test.ts");
    let safe_rows =
        include_str!("../../../fixtures/recall/runner-eval-binding-guard/rows-safe.test.ts");
    let mut results = corpora(
        &[
            ("rows-unsafe.test.ts", unsafe_rows),
            ("rows-safe.test.ts", safe_rows),
        ],
        &["rows-unsafe.test.ts", "rows-safe.test.ts"],
    );
    let safe = results.pop().unwrap();
    let unsafe_rows = results.pop().unwrap();
    assert_eq!(unsafe_rows.tests.len(), 4);
    let excluded = unsafe_rows
        .tests
        .iter()
        .filter(|case| case.exclusion.is_some())
        .count();
    assert_eq!(
        findings_in(unsafe_rows, "rows-unsafe.test.ts")
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateTest)
            .count(),
        0
    );
    assert_eq!(excluded, 4);
    assert_eq!(safe.tests.len(), 2);
    assert!(safe.tests.iter().all(|case| case.exclusion.is_none()));
    let duplicates: Vec<_> = findings_in(safe, "rows-safe.test.ts")
        .into_iter()
        .filter(|finding| finding.rule == Rule::DuplicateTest)
        .collect();
    assert_eq!(duplicates.len(), 1, "{duplicates:?}");
    assert_eq!(duplicates[0].titles, ["safe row first", "safe row second"]);
}

/// Equal numeric symbol IDs in different modules never inherit another module's actual values.
#[test]
fn nested_imports_use_their_own_lexical_binding_environment() {
    let leaf = include_str!("../../../fixtures/recall/runner-nested-binding-isolation/leaf.ts");
    let runner = include_str!("../../../fixtures/recall/runner-nested-binding-isolation/runner.ts");
    let caller =
        include_str!("../../../fixtures/recall/runner-nested-binding-isolation/run.test.ts");
    let parsed_leaf = crate::lang::parse(std::path::Path::new("leaf.ts"), leaf).unwrap();
    let parsed_runner = crate::lang::parse(std::path::Path::new("runner.ts"), runner).unwrap();
    let crate::lang::Declaration::Function(function) = parsed_runner
        .scopes
        .symbol(parsed_runner.scopes.top_level("run").unwrap())
        .declaration
    else {
        panic!("runner function")
    };
    let parameter = parsed_runner.tree.list(function, "params")[0].unwrap();
    let symbol = parsed_runner
        .scopes
        .symbol_at(parsed_runner.tree.span(parameter).start)
        .unwrap();
    assert_eq!(
        parsed_leaf.scopes.top_level("hidden"),
        Some(symbol),
        "The control must stress numeric symbol reuse across module boundaries"
    );
    let corpus = corpora(
        &[
            ("leaf.ts", leaf),
            ("runner.ts", runner),
            ("run.test.ts", caller),
        ],
        &["run.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 3);
    assert!(corpus.tests.iter().all(|case| case.exclusion.is_none()));
    let duplicates: Vec<_> = findings_in(corpus, "run.test.ts")
        .into_iter()
        .filter(|finding| finding.rule == Rule::DuplicateTest)
        .collect();
    assert_eq!(duplicates.len(), 1, "{duplicates:?}");
    assert_eq!(duplicates[0].titles, ["page literal", "page bound"]);
}

/// Nested frames preserve captured and shadowed values, return boundaries and source identities.
#[test]
fn nested_calls_preserve_the_permanent_capture_and_module_initialization_controls() {
    for (files, expected_tests, titles) in [
        (
            vec![
                (
                    "leaf.ts",
                    include_str!(
                        "../../../fixtures/recall/runner-nested-forwarded-capture/leaf.ts"
                    ),
                ),
                (
                    "runner.ts",
                    include_str!(
                        "../../../fixtures/recall/runner-nested-forwarded-capture/runner.ts"
                    ),
                ),
                (
                    "run.test.ts",
                    include_str!(
                        "../../../fixtures/recall/runner-nested-forwarded-capture/run.test.ts"
                    ),
                ),
            ],
            3,
            ["page leaf", "page local"],
        ),
        (
            vec![
                (
                    "runner.ts",
                    include_str!("../../../fixtures/recall/runner-nested-module-const/runner.ts"),
                ),
                (
                    "run.test.ts",
                    include_str!("../../../fixtures/recall/runner-nested-module-const/run.test.ts"),
                ),
            ],
            2,
            ["page first", "page second"],
        ),
    ] {
        let corpus = corpora(&files, &["run.test.ts"]).remove(0);
        assert_eq!(corpus.tests.len(), expected_tests);
        assert!(
            corpus.tests.iter().all(|case| case.exclusion.is_none()),
            "{corpus:?}"
        );
        assert!(!corpus
            .tests
            .iter()
            .any(|case| case.title.as_deref() == Some("dead tail")));
        let duplicates: Vec<_> = findings_in(corpus, "run.test.ts")
            .into_iter()
            .filter(|finding| finding.rule == Rule::DuplicateTest)
            .collect();
        assert_eq!(duplicates.len(), 1, "{duplicates:?}");
        assert_eq!(duplicates[0].titles, titles);
    }
}

/// An unproved child discards its parent's speculative registrations and preserves other files.
#[test]
fn nested_refusals_are_atomic_and_do_not_taint_independent_positives() {
    let header = "import {test,expect} from 'vitest';";
    for (runner, leaf) in [
        ("export function run(){test('hidden',()=>{});return run()}", "export const unused=1"),
        ("export function run(){return child()} function child(){test('hidden',()=>{});return run()}", "export const unused=1"),
        ("export function run(){test('hidden',()=>{});return child();const child=()=>test('dead',()=>{})}", "export const unused=1"),
        ("import child from './leaf';export function run(){test('hidden',()=>{});return child(null)}", "import {test} from 'vitest';export default function child({value}){test('child',()=>{})}"),
        ("import {missing} from './leaf';export function run(){test('hidden',()=>{})}", "export function exists(){}"),
        ("import child from './leaf';export function run(){test('hidden',()=>{});return child(unknown)}", "import {test} from 'vitest';export default function child(value){test('child',()=>{})}"),
    ] {
        let runner = format!("{header}{runner}");
        let unsafe_source = "import {test,expect} from 'vitest';import {run} from './runner';test('before',()=>expect(' A '.trim()).toBe('A'));run();test('after',()=>expect(' A '.trim()).toBe('A'));";
        let safe = include_str!("../../../fixtures/recall/runner-unproved-scope-retention/safe.test.ts");
        let mut results = corpora(&[("runner.ts", &runner), ("leaf.ts", leaf), ("unsafe.test.ts", unsafe_source), ("safe.test.ts", safe)], &["unsafe.test.ts", "safe.test.ts"]);
        let safe = results.pop().unwrap();
        let refused = results.pop().unwrap();
        assert_eq!(refused.tests.len(), 3, "{runner}");
        assert!(refused.tests.iter().all(|case| case.exclusion.is_some() && case.origin.is_none()), "{runner}: {refused:?}");
        assert!(refused.helper_sources.is_empty());
        assert!(findings_in(refused, "unsafe.test.ts").is_empty());
        assert_eq!(safe.tests.len(), 2);
        assert!(safe.tests.iter().all(|case|case.exclusion.is_none()));
        assert_eq!(findings_in(safe, "safe.test.ts").iter().filter(|finding|finding.rule==Rule::DuplicateTest).count(),1);
    }
}

/// Nested calls and re-export resolution consume the same depth allowance.
#[test]
fn nested_export_hops_share_one_depth_budget() {
    for (leaf_hops, expected) in [(4, 3), (5, 1)] {
        let mut files = vec![(
            "run.test.ts".to_owned(),
            "import {run} from './outer0';run();".to_owned(),
        )];
        for i in 0..3 {
            files.push((
                format!("outer{i}.ts"),
                format!("export {{run}} from './outer{}'", i + 1),
            ));
        }
        files.push((
            "outer3.ts".to_owned(),
            "import {test,expect} from 'vitest';import {leaf} from './leaf0';export function run(){test('outer',()=>expect('outer'.includes('out')).toBe(true));return leaf()}".to_owned(),
        ));
        for i in 0..leaf_hops - 1 {
            files.push((
                format!("leaf{i}.ts"),
                format!("export {{leaf}} from './leaf{}'", i + 1),
            ));
        }
        files.push((format!("leaf{}.ts",leaf_hops-1),"import {test,expect} from 'vitest';export function leaf(){test('first',()=>expect(' A '.trim()).toBe('A'));test('second',()=>expect(' A '.trim()).toBe('A'));}".to_owned()));
        let refs: Vec<_> = files
            .iter()
            .map(|(name, source)| (name.as_str(), source.as_str()))
            .collect();
        let corpus = corpora(&refs, &["run.test.ts"]).remove(0);
        assert_eq!(corpus.tests.len(), expected, "{leaf_hops}");
        if leaf_hops == 4 {
            assert!(corpus.tests.iter().all(|case| case.exclusion.is_none()));
            assert_eq!(
                findings_in(corpus, "run.test.ts")
                    .iter()
                    .filter(|finding| finding.rule == Rule::DuplicateTest)
                    .count(),
                1
            );
        } else {
            assert!(corpus.tests.iter().all(|case| case.exclusion.is_some()));
            assert!(corpus.helper_sources.is_empty());
            assert!(findings_in(corpus, "run.test.ts").is_empty());
        }
    }
}

/// Findings retain source locations while grouping executions by their caller file.
fn findings(corpus: FileCorpus) -> Vec<crate::model::Finding> {
    findings_in(corpus, "slugs.test.ts")
}

/// Judge a corpus under its actual registration file.
fn findings_in(corpus: FileCorpus, path: &str) -> Vec<crate::model::Finding> {
    let config = crate::config::Config {
        root: PathBuf::from("."),
        include: Vec::new(),
        include_is_explicit: false,
        exclude: Vec::new(),
        rules: Default::default(),
        threshold: 0,
        source: "test".to_owned(),
    };
    crate::rules::run(&[(PathBuf::from(path), corpus.tests)], &config).findings
}

/// Named and default callables become comparable; changing the actual behavior retains silence.
#[test]
fn permanent_runner_controls_preserve_execution_counts_and_findings() {
    for (runner, caller, expected) in [
        (
            include_str!("../../../fixtures/recall/framework-runner-named-control/runner.ts"),
            include_str!("../../../fixtures/recall/framework-runner-named-control/slugs.test.ts"),
            1,
        ),
        (
            include_str!("../../../fixtures/recall/framework-runner-default-const/runner.ts"),
            include_str!("../../../fixtures/recall/framework-runner-default-const/slugs.test.ts"),
            1,
        ),
        (
            include_str!(
                "../../../fixtures/recall/framework-runner-default-const-negative/runner.ts"
            ),
            include_str!(
                "../../../fixtures/recall/framework-runner-default-const-negative/slugs.test.ts"
            ),
            0,
        ),
    ] {
        let corpus = corpora(
            &[("runner.ts", runner), ("slugs.test.ts", caller)],
            &["slugs.test.ts"],
        )
        .remove(0);
        assert_eq!(corpus.tests.len(), 2);
        assert_eq!(
            corpus
                .tests
                .iter()
                .filter(|case| case.exclusion.is_none())
                .count(),
            2,
            "{corpus:?}"
        );
        assert_eq!(corpus.helper_sources.len(), 1);
        let claims: Vec<_> = findings(corpus)
            .into_iter()
            .filter(|finding| finding.rule == Rule::DuplicateTest)
            .collect();
        assert_eq!(claims.len(), expected, "{claims:?}");
        if let Some(finding) = claims.first() {
            assert!(finding
                .locations
                .iter()
                .any(|location| location.path == std::path::Path::new("runner.ts")));
            assert!(finding
                .locations
                .iter()
                .any(|location| location.path == std::path::Path::new("slugs.test.ts")));
            let helper = finding
                .locations
                .iter()
                .position(|location| location.path == std::path::Path::new("runner.ts"))
                .unwrap();
            assert_eq!(
                finding.registration_files[helper],
                [PathBuf::from("slugs.test.ts")]
            );
        }
    }
}

/// Returning a template is not an invocation: a failed batch emits no speculative helper records.
#[test]
fn a_failed_sibling_call_cannot_leave_partial_expansion_records() {
    let source = include_str!("../../../fixtures/recall/framework-runner-named-control/runner.ts");
    let caller="import {defineSlugChecks} from './runner'; defineSlugChecks('page'); defineSlugChecks(unknown);";
    let corpus = corpora(
        &[("runner.ts", source), ("slugs.test.ts", caller)],
        &["slugs.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 2);
    assert!(corpus.tests.iter().all(
        |case| case.origin.is_none() && case.exclusion == Some(Exclusion::RegisteredElsewhere)
    ));
    assert!(corpus.helper_sources.is_empty());
    assert!(findings(corpus).is_empty());
}

/// Repetition counts executions: two calls of one template are four runtime tests whose written
/// pair still pairs, and each invocation pairs with its own twin across calls (distinct
/// declarations, one per invocation).
#[test]
fn repeated_imported_calls_pair_per_invocation() {
    let runner =
        include_str!("../../../fixtures/recall/runner-repeated-registration-ownership/runner.ts");
    let caller =
        include_str!("../../../fixtures/recall/runner-repeated-registration-ownership/run.test.ts");
    let corpus = corpora(
        &[("runner.ts", runner), ("run.test.ts", caller)],
        &["run.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 4);
    assert!(corpus.tests.iter().all(|case| case.exclusion.is_none()));
    assert_eq!(corpus.helper_sources.len(), 1);
    let findings = findings_in(corpus, "run.test.ts");
    // The written pair, and one duplicate per template test between the two invocations.
    assert_eq!(findings.len(), 5, "{findings:?}");
    let duplicates: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == Rule::DuplicateTest)
        .collect();
    assert_eq!(duplicates.len(), 3);
    assert!(duplicates.iter().all(|finding| {
        finding.locations.len() == 2
            && finding
                .locations
                .iter()
                .all(|location| location.path == std::path::Path::new("runner.ts"))
    }));
    // Each invocation twin pairs with itself across calls: the titles agree within the pair.
    assert!(duplicates
        .iter()
        .any(|finding| finding.titles[0] == finding.titles[1]
            && finding.titles[0] == "runner normalizes first label"));
    assert!(duplicates
        .iter()
        .any(|finding| finding.titles[0] == finding.titles[1]
            && finding.titles[0] == "runner normalizes second label"));
    assert!(duplicates
        .iter()
        .any(|finding| finding.titles[0] != finding.titles[1]));
    assert_eq!(
        duplicates[0].registration_files,
        vec![vec![PathBuf::from("run.test.ts")]; 2]
    );
}

/// Source identity distinguishes same-offset declarations in different helper modules.
#[test]
fn equal_offsets_in_distinct_helpers_preserve_the_owned_duplicate() {
    let corpus = corpora(
        &[
            (
                "left.ts",
                include_str!(
                    "../../../fixtures/recall/runner-declaration-origin-collision/left.ts"
                ),
            ),
            (
                "right.ts",
                include_str!(
                    "../../../fixtures/recall/runner-declaration-origin-collision/right.ts"
                ),
            ),
            (
                "run.test.ts",
                include_str!(
                    "../../../fixtures/recall/runner-declaration-origin-collision/run.test.ts"
                ),
            ),
        ],
        &["run.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 2);
    assert!(corpus.tests.iter().all(|case| case.exclusion.is_none()));
    assert_eq!(corpus.tests[0].span, corpus.tests[1].span);
    let findings = findings_in(corpus, "run.test.ts");
    assert_eq!(findings.len(), 2, "{findings:?}");
    assert!(findings
        .iter()
        .any(|finding| finding.rule == Rule::DuplicateTest));
    assert!(findings
        .iter()
        .any(|finding| finding.rule == Rule::DuplicateTestName));
    for finding in findings {
        assert_eq!(finding.locations.len(), 2);
        assert_ne!(finding.locations[0].path, finding.locations[1].path);
    }
}

/// A refused root runner cannot yield a false duplicate while an independent file still can.
#[test]
fn permanent_refusal_control_withholds_the_scope_and_retains_independent_work() {
    let mut corpora = corpora(
        &[
            (
                "helper.ts",
                include_str!("../../../fixtures/recall/runner-unproved-scope-retention/helper.ts"),
            ),
            (
                "unsafe.test.ts",
                include_str!(
                    "../../../fixtures/recall/runner-unproved-scope-retention/unsafe.test.ts"
                ),
            ),
            (
                "safe.test.ts",
                include_str!(
                    "../../../fixtures/recall/runner-unproved-scope-retention/safe.test.ts"
                ),
            ),
        ],
        &["unsafe.test.ts", "safe.test.ts"],
    );
    let safe = corpora.pop().unwrap();
    let refused = corpora.pop().unwrap();
    assert_eq!(refused.tests.len(), 3);
    assert_eq!(
        refused
            .tests
            .iter()
            .filter(|case| case.exclusion == Some(Exclusion::UnprovedRunner))
            .count(),
        2
    );
    assert!(refused.tests.iter().all(|case| case.exclusion.is_some()));
    assert!(findings_in(refused, "unsafe.test.ts").is_empty());
    assert_eq!(safe.tests.len(), 2);
    assert!(safe.tests.iter().all(|case| case.exclusion.is_none()));
    let findings = findings_in(safe, "safe.test.ts");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].titles, ["safe first", "safe second"]);
}

/// A hook result that never runs cannot turn a hooks-only helper into an unknown test runner.
#[test]
fn unused_hook_results_preserve_the_permanent_duplicate_control() {
    let source =
        include_str!("../../../fixtures/recall/helper-initializer-equivalent-hooks/run.test.ts");
    for kind in ["const", "let", "var"] {
        let source = source.replace("const stop", &format!("{kind} stop"));
        let corpus = corpora(&[("run.test.ts", &source)], &["run.test.ts"]).remove(0);
        assert_eq!(corpus.tests.len(), 2, "{kind}");
        assert!(
            corpus.tests.iter().all(|case| case.exclusion.is_none()),
            "{kind}"
        );
        let findings = findings_in(corpus, "run.test.ts");
        assert_eq!(findings.len(), 1, "{kind}: {findings:?}");
        assert_eq!(findings[0].rule, Rule::DuplicateTest);
        assert_eq!(findings[0].titles, ["hooked > reads", "plain > reads"]);
    }
}

/// Refusal reaches earlier children in its actual suite, preserving independent root positives.
#[test]
fn an_unproved_suite_call_does_not_clear_its_children_or_taint_the_root() {
    let runner = include_str!("../../../fixtures/recall/framework-runner-named-control/runner.ts");
    let caller="import {test,describe,expect} from 'vitest'; import {defineSlugChecks} from './runner';
      describe('uncertain',()=>{test('early',()=>expect(1).toBe(1));defineSlugChecks(unknown);test('late',()=>expect(1).toBe(1));});
      test('root first',()=>expect(' A '.trim()).toBe('A'));test('root second',()=>expect(' A '.trim()).toBe('A'));";
    let corpus = corpora(
        &[("runner.ts", runner), ("slugs.test.ts", caller)],
        &["slugs.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 5);
    assert_eq!(
        corpus
            .tests
            .iter()
            .filter(|case| case.exclusion == Some(Exclusion::UnprovedRunner))
            .count(),
        2
    );
    assert_eq!(
        corpus
            .tests
            .iter()
            .filter(|case| case.exclusion.is_none())
            .count(),
        2
    );
    let findings = findings(corpus);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].titles, ["root first", "root second"]);
}

/// Module metadata does not depend on the invocation, so a runner callback reading
/// `import.meta` is expanded and compared; a callback reading the invocation's own
/// `arguments` stays refused.
#[test]
fn an_import_meta_callback_is_compared_while_arguments_stay_refused() {
    let runner = include_str!("../../../fixtures/recall/runner-import-meta/runner.ts");
    let caller = include_str!("../../../fixtures/recall/runner-import-meta/meta.test.ts");
    let corpus = corpora(
        &[("runner.ts", runner), ("meta.test.ts", caller)],
        &["meta.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 2);
    assert!(corpus.tests.iter().all(|case| case.exclusion.is_none()));
    let findings = findings(corpus);
    assert_eq!(findings.len(), 2, "{findings:?}");
    assert_eq!(findings[0].rule, Rule::DuplicateTest);
    assert_eq!(
        findings[0].titles,
        ["page reads module metadata", "page reads module metadata"]
    );
    assert_eq!(findings[1].rule, Rule::DuplicateTestName);

    let receiver_runner =
        include_str!("../../../fixtures/recall/runner-arguments-receiver/runner.ts");
    let receiver_caller =
        include_str!("../../../fixtures/recall/runner-arguments-receiver/caller.test.ts");
    let corpus = corpora(
        &[
            ("runner.ts", receiver_runner),
            ("caller.test.ts", receiver_caller),
        ],
        &["caller.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 1);
    // The refusal surfaces as the wrapper reach: what the callback sees depends on the call.
    assert_eq!(
        corpus.tests[0].exclusion,
        Some(Exclusion::RegisteredElsewhere)
    );
    assert!(findings_in(corpus, "caller.test.ts").is_empty());
}
