//! Final corpus and finding outcomes on the same permanent source used by the runtime oracle.
use super::*;
use crate::model::Rule;
use std::path::PathBuf;

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

/// Repetition counts executions while the remedy still identifies one source declaration pair.
#[test]
fn repeated_imported_calls_share_declaration_identity() {
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
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].rule, Rule::DuplicateTest);
    assert_eq!(findings[0].locations.len(), 2);
    assert!(findings[0]
        .locations
        .iter()
        .all(|location| location.path == std::path::Path::new("runner.ts")));
    assert_eq!(
        findings[0].registration_files,
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
