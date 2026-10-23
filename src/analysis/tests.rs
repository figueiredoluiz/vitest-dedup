//! Tests for [`super::analyze`], built entirely from hand-constructed [`TestCase`] values.
//!
//! Two groups of tests:
//!
//! - One `#[test]` per file in `fixtures/duplicates/`, named after the fixture, reconstructing
//!   the IR that fixture's source would produce and asserting the verdict stated in its header
//!   comment. Every fixture is fully self-contained (real cross-file pairs where the
//!   discrimination is cross-file, real differing code where the discrimination depends on it),
//!   so these tests build the IR straight from what each file actually contains -- no fabricated
//!   "missing half". `not-duplicates-different-mocks-{a,b}` previously exposed a real IR gap
//!   (`Context.mocked_modules` carried only specifiers, so two different `vi.mock` factories for
//!   the same module were indistinguishable); `model.rs` now carries a `MockedModule` fingerprint
//!   per entry, so that fixture is asserted for real below, with no remaining workaround.
//! - Adversarial and structural cases: empty input, singleton groups, the spanning-set/cluster
//!   size boundary, trivial/empty bodies, threshold edges, a rule turned `Off`, and determinism
//!   under shuffled input order.
//!
//! Three rows of the 2026-09-27 tests-implementation plan are NOT here because they cannot
//! compile against today's model, by design rather than omission, and are named so the model
//! change is not discovered mid-fix: the `redundant-hook` idempotence signal needs a field on
//! `model::Hook` (e.g. `body_effect`) before `redundant_hook_ignores_a_pair_whose_body_is_not_
//! known_idempotent` and the fingerprint-stream row beside it can be written, and
//! `cross_file_duplicate_requires_equal_module_scope_setup` needs module-scope setup in
//! `Context` first. They land with those fixes.

use super::*;
use crate::model::{
    DeclarationKind, EachRows, EachTable, FixtureBinding, FixtureValue, Hook, Location,
    MockFactory, MockedModule, Modifier, Position, Span, Title,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

// ---------------------------------------------------------------------------------------------
// IR builders
// ---------------------------------------------------------------------------------------------

/// Build a one-column source-span fixture on the supplied line.
fn line_span(line: u32) -> Span {
    Span {
        start: Position { line, column: 1 },
        end: Position { line, column: 2 },
        start_byte: 0,
        end_byte: 1,
    }
}

/// A hook the rule may report: non-trivial body, and idempotent.
///
/// `idempotent: true` here is the *rule-layer* fixture's business. Whether a real body earns it
/// is decided in the frontend and tested there (`extract::tests`); these tests are about what the
/// rule does with the answer, and building every hook non-idempotent would silence the rule and
/// make each one vacuous. `non_idempotent_hook` below is the contrasting builder.
fn hook(kind: HookKind, alpha: &str, line: u32) -> Hook {
    Hook {
        idempotent: true,
        ..non_idempotent_hook(kind, alpha, line)
    }
}

/// A hook whose body the frontend could not prove idempotent -- the default for anything it
/// cannot argue about, and the reason `redundant-hook` scored 0 of 10 before S5.
fn non_idempotent_hook(kind: HookKind, alpha: &str, line: u32) -> Hook {
    Hook {
        kind,
        location: Location {
            path: PathBuf::from("hooks.ts"),
            span: line_span(line),
        },
        body: fingerprints(alpha, false),
        idempotent: false,
        alpha_source: std::sync::Arc::from(alpha),
    }
}

/// Build fingerprint fixtures with the requested alpha digest and trivial flag, leaving other
/// evidence empty.
fn fingerprints(alpha: &str, trivial: bool) -> crate::model::Fingerprints {
    crate::model::Fingerprints {
        alpha: crate::model::BodyDigest::of(alpha),
        structural: crate::model::BodyDigest::of(""),
        behavior: crate::model::BehaviorTrace::default(),
        trivial,
        callback_form: crate::model::CallbackForm::Interchangeable,
    }
}

/// A `vi.mock(specifier, factory)` registration. `factory: None` models automocking
/// (`vi.mock('./x')` with no second argument, `MockFactory::Automock`), which `MockedModule`
/// documents as a distinct, meaningful state from an explicit factory -- not an absent value to be
/// treated as a wildcard. This helper's `Option<&str>` signature never needs to build
/// `MockFactory::Unresolved`: producing that variant must also set `Context::unresolved`, which is
/// a separate, explicit `.unresolved()` builder step no caller here conflates with "no factory".
fn mocked(specifier: &str, factory: Option<&str>) -> MockedModule {
    MockedModule {
        specifier: std::sync::Arc::from(specifier),
        factory: match factory {
            Some(alpha) => MockFactory::Factory(std::sync::Arc::from(alpha)),
            None => MockFactory::Automock,
        },
    }
}

/// A `test.extend` fixture binding. `body: None` models a plain literal value (`FixtureValue::
/// Literal`), `Some` a setup function (`FixtureValue::Setup`) -- per `FixtureBinding`'s doc
/// comment, "unknown is not the same as matching". As with `mocked` above, `FixtureValue::
/// Unresolved` is never built through this helper; it always implies `Context::unresolved`.
fn fixture(name: &str, body: Option<&str>) -> FixtureBinding {
    FixtureBinding {
        name: name.to_owned(),
        body: match body {
            Some(alpha) => FixtureValue::Setup(alpha.to_owned()),
            None => FixtureValue::Literal(String::new()),
        },
    }
}

/// A fluent builder for a single `TestCase`, defaulting to a plain top-level, non-trivial,
/// has-a-body test with an empty `Context`.
struct T {
    /// Overrides the built location's byte span; see [`Self::build_with_span`].
    span_override: Option<(u32, u32)>,
    /// Builds a `Title::Parameterized` instead of `Title::Static` -- an `.each` row whose title
    /// pattern did not fully render; see [`Self::parameterized_title`].
    parameterized_title: bool,
    /// Marks the case as a row of a statically enumerated table; see
    /// [`crate::model::TestCase::table_row_class`].
    table_row_class: Option<u32>,
    /// [`crate::model::TestCase::row_cells_fully_observable`]. Defaults to `true` -- the
    /// conservative value -- so a builder that says nothing gets the stricter grouping.
    row_cells_fully_observable: bool,

    path: &'static str,
    line: u32,
    suite: Vec<&'static str>,
    title: &'static str,
    modifiers: Vec<Modifier>,
    context: Context,
    alpha: &'static str,
    /// Defaults to `""`, matching every pre-`parameterization-candidate` test in this file: those
    /// tests only ever compare `alpha`, so an empty `structural` was never load-bearing for them.
    /// `parameterization-candidate` groups on `structural` instead, so any test exercising it must
    /// set this explicitly via [`T::structural`].
    structural: &'static str,
    /// Defaults to empty, which -- combined with [`super::MIN_BEHAVIOR_EVENTS_FOR_PARAMETERIZATION`]
    /// -- deliberately keeps every test built without calling [`T::behavior`] out of
    /// `parameterization-candidate` grouping entirely. Without this default, dozens of pre-existing
    /// same-file `duplicate-test`/`duplicate-test-name` fixtures above (built with a placeholder
    /// empty `structural`, since they predate this rule) would collide on `structural == ""` and
    /// start firing a rule they were never written to exercise.
    behavior: Vec<String>,
    /// Defaults to `Interchangeable`, which is what almost every body is.
    callback_form: CallbackForm,
    trivial: bool,
    has_body: bool,
    /// Defaults to [`DeclarationKind::Test`]. Set via [`T::kind`] to build a `describe`/`suite`
    /// (or `bench`) entry instead, for tests that need a container declaration alongside real
    /// tests -- see `tests_analyzed_excludes_suites`.
    kind: DeclarationKind,
}

impl T {
    /// Initialize a test-case builder with source identity, title and alpha evidence, using
    /// default context and modifiers.
    fn new(path: &'static str, line: u32, title: &'static str, alpha: &'static str) -> Self {
        Self {
            span_override: None,
            parameterized_title: false,
            table_row_class: None,
            row_cells_fully_observable: true,
            path,
            line,
            suite: Vec::new(),
            title,
            modifiers: Vec::new(),
            context: Context::default(),
            alpha,
            structural: "",
            behavior: Vec::new(),
            callback_form: CallbackForm::Interchangeable,
            trivial: false,
            has_body: true,
            kind: DeclarationKind::Test,
        }
    }

    /// Marks the case as one row of a statically enumerated `test.each`/`test.for` table.
    fn table_row(mut self) -> Self {
        self.table_row_class = Some(0);
        self
    }

    /// Marks the case as a row whose callback provably DROPS the cells past its fixed
    /// parameters -- a uniform array table bound by a rest-free parameter list. Two such rows
    /// binding the same arguments are one case however their raw data differs.
    fn row_cells_dropped(mut self) -> Self {
        self.row_cells_fully_observable = false;
        self
    }

    /// Marks the case as a table row with a specific canonical value identity.
    fn table_row_as(mut self, class: u32) -> Self {
        self.table_row_class = Some(class);
        self
    }

    /// Overrides the declaration kind. Used to build a `DeclarationKind::Suite` entry -- a
    /// `describe`/`suite` block is a container, not an executable test, and must not be counted
    /// toward `tests_analyzed` or grouped by either duplication rule.
    fn kind(mut self, kind: DeclarationKind) -> Self {
        self.kind = kind;
        self
    }

    /// Sets `body.structural`, the shape [`crate::model::Rule::ParameterizationCandidate`] groups
    /// on. Two tests built with the same `structural` (and differing `alpha`) are exactly the
    /// "same shape, different literals" pair that rule reports.
    fn structural(mut self, structural: &'static str) -> Self {
        self.structural = structural;
        self
    }

    /// Sets `body.behavior`, so a test can clear
    /// [`super::MIN_BEHAVIOR_EVENTS_FOR_PARAMETERIZATION`] and actually participate in
    /// `parameterization-candidate` grouping. `events` mirrors the shape
    /// [`crate::frontend::fingerprint`]'s `BehaviorCollector` would really emit (see its module
    /// doc) closely enough for these tests' purposes; the exact event strings are never compared
    /// by this rule, only `events.len()`.
    fn behavior(mut self, events: &[&str]) -> Self {
        self.behavior = events.iter().map(|event| (*event).to_owned()).collect();
        self
    }

    /// Whether the callback form changes this body's meaning. See [`CallbackForm`].
    fn callback_form(mut self, form: CallbackForm) -> Self {
        self.callback_form = form;
        self
    }

    /// Set the fixture suite path and return the updated builder.
    fn suite(mut self, parts: &[&'static str]) -> Self {
        self.suite = parts.to_vec();
        self
    }

    /// Replace fixture context hooks and return the updated builder.
    fn hooks(mut self, hooks: Vec<Hook>) -> Self {
        self.context.hooks = hooks;
        self
    }

    /// Set the context mock identities on this test builder.
    fn mocked_modules(mut self, modules: Vec<MockedModule>) -> Self {
        self.context.mocked_modules = modules;
        self
    }

    /// Set the context fixture bindings on this test builder.
    fn fixtures(mut self, fixtures: Vec<FixtureBinding>) -> Self {
        self.context.fixtures = fixtures;
        self
    }

    /// Store the supplied module identities as shared context strings.
    fn imports(mut self, imports: &[&str]) -> Self {
        self.context.imports = imports.iter().map(|m| Arc::from(*m)).collect();
        self
    }

    /// Append the skip modifier to this declaration fixture.
    fn skip(mut self) -> Self {
        self.modifiers.push(Modifier::Skip);
        self
    }

    /// Sets the full modifier list directly, for tests exercising `Modifier::Each`, `SkipIf`, or
    /// `RunIf` rather than the plain `.skip()`/`.todo()` shortcuts above.
    fn modifiers(mut self, modifiers: Vec<Modifier>) -> Self {
        self.modifiers = modifiers;
        self
    }

    /// Mark the fixture todo and remove its executable-body flag.
    fn todo(mut self) -> Self {
        self.modifiers.push(Modifier::Todo);
        self.has_body = false;
        self
    }

    /// Mark this fixture body as trivial for rule controls.
    fn trivial(mut self) -> Self {
        self.trivial = true;
        self
    }

    /// Marks the title as an `.each` pattern that did not fully render (`Title::Parameterized`),
    /// the participant shape `duplicate-each-row`'s fully-rendered-title requirement (S7) turns
    /// on. Everything else about the built case is unchanged.
    fn parameterized_title(mut self) -> Self {
        self.parameterized_title = true;
        self
    }

    /// Mark the fixture context unresolved for fail-closed rule tests.
    fn unresolved(mut self) -> Self {
        self.context.unresolved = true;
        self
    }

    /// [`Self::unresolved`], conditional -- for tests that build the same pair with and without
    /// the unresolved context and compare the outcomes.
    fn unresolved_of(mut self, unresolved: bool) -> Self {
        if unresolved {
            self.context.unresolved = true;
        }
        self
    }

    /// [`Self::build`], with an explicit byte span. Suite-containment tests need spans that
    /// actually enclose other cases' spans, which line-derived spans never do.
    fn build_with_span(mut self, start_byte: u32, end_byte: u32) -> TestCase {
        self.span_override = Some((start_byte, end_byte));
        self.build()
    }

    /// Materialize the owned test case, honoring any source-span override and configured
    /// fingerprint/context fields.
    fn build(self) -> TestCase {
        TestCase {
            registration_file: None,
            location: Location {
                path: PathBuf::from(self.path),
                span: self.span_override.map_or_else(
                    || line_span(self.line),
                    |(start_byte, end_byte)| Span {
                        start: Position {
                            line: self.line,
                            column: 1,
                        },
                        end: Position {
                            line: self.line,
                            column: 2,
                        },
                        start_byte,
                        end_byte,
                    },
                ),
            },
            kind: self.kind,
            suite_path: self
                .suite
                .into_iter()
                .map(|part| Title::Static(part.to_owned()))
                .collect(),
            title: if self.parameterized_title {
                Title::Parameterized(self.title.to_owned())
            } else {
                Title::Static(self.title.to_owned())
            },
            modifiers: self.modifiers,
            context: self.context,
            body: crate::model::Fingerprints {
                alpha: crate::model::BodyDigest::of(self.alpha),
                structural: crate::model::BodyDigest::of(self.structural),
                behavior: crate::model::BehaviorTrace::new(&self.behavior),
                trivial: self.trivial,
                callback_form: self.callback_form,
            },
            has_body: self.has_body,
            table_row_class: self.table_row_class,
            row_cells_fully_observable: self.row_cells_fully_observable,
        }
    }
}

/// Build an analysis fixture config with the supplied severities and quality-gate threshold
/// percentage.
fn config_with(rules: &[(Rule, Severity)], threshold: u8) -> Config {
    let mut map = BTreeMap::new();
    for (rule, severity) in rules {
        map.insert(*rule, *severity);
    }
    Config {
        root: PathBuf::from("."),
        include: Vec::new(),
        include_is_explicit: false,
        exclude: Vec::new(),
        rules: map,
        threshold,
        source: "test".to_owned(),
    }
}

/// The `analyze` call most of this file makes: the default rule set, one worker, and a clean
/// incomplete state.
fn analyze_default(tests: Vec<TestCase>) -> AnalysisResult {
    analyze(tests, &default_config(), 1, IncompleteCauses::default()).unwrap()
}

/// [`analyze_default`]'s shape for the tests that need their own config or file count.
fn analyze_with(tests: Vec<TestCase>, config: &Config, files_analyzed: usize) -> AnalysisResult {
    analyze(tests, config, files_analyzed, IncompleteCauses::default()).unwrap()
}

/// Build the baseline analysis fixture config with no severity overrides and a zero threshold.
fn default_config() -> Config {
    config_with(&[], 0)
}

/// Count duplicate-test and duplicate-test-name findings in that order.
fn rule_counts(findings: &[Finding]) -> (usize, usize) {
    let duplicate_test = findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateTest)
        .count();
    let duplicate_name = findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateTestName)
        .count();
    (duplicate_test, duplicate_name)
}

/// `count` tests that differ only by their line number, all sharing one body -- the group
/// every spanning/cluster-size test needs, in the generator shape those tests used to repeat.
/// Titles are `"{label} {index}"`.
fn numbered_group(
    file: &'static str,
    count: u32,
    body: &'static str,
    label: &str,
) -> Vec<TestCase> {
    (0..count)
        .map(|index| {
            T::new(
                file,
                index + 1,
                Box::leak(format!("{label} {index}").into_boxed_str()),
                body,
            )
            .build()
        })
        .collect()
}

/// The most common assertion in this file, as one call: a rule fired exactly `expected` times.
/// `why` stays at the call site because it is the test's actual claim.
fn assert_rule_count(findings: &[Finding], rule: Rule, expected: usize, why: &str) {
    assert_eq!(rule_count(findings, rule), expected, "{why}");
}

/// The overlap pair: same structural shape and behavior, differing alpha -- the participant set
/// `parameterization-candidate` and `near-duplicate-test` both want, which three suppression
/// tests share verbatim.
fn overlap_pair() -> Vec<TestCase> {
    let events = ["call:setup", "assert:toBe"];
    vec![
        T::new(
            "overlap.test.ts",
            1,
            "accepts one",
            "setup(); expect(value()).toBe(1)",
        )
        .structural("setup(); expect(value()).toBe(<number>)")
        .behavior(&events)
        .build(),
        T::new(
            "overlap.test.ts",
            5,
            "accepts two",
            "setup(); expect(value()).toBe(2)",
        )
        .structural("setup(); expect(value()).toBe(<number>)")
        .behavior(&events)
        .build(),
    ]
}

/// The determinism claim both input-ordering tests make: build the fixture twice, shuffle one
/// copy harder than a sort could accidentally undo (reverse, then swap two members), and require
/// identical AND byte-identical findings between the two runs. `what` names the findings the
/// caller's fixture produces, for the byte-identical assertion's message.
fn findings_stable_under_reordering(
    build: impl Fn() -> Vec<TestCase>,
    files_analyzed: usize,
    what: &str,
) -> (AnalysisResult, AnalysisResult) {
    let original = build();
    let mut shuffled = build();
    shuffled.reverse();
    shuffled.swap(0, 3);

    let result_a = analyze_with(original, &default_config(), files_analyzed);
    let result_b = analyze_with(shuffled, &default_config(), files_analyzed);

    assert_eq!(result_a.findings, result_b.findings);
    assert_eq!(
        serde_json::to_string(&result_a.findings).unwrap(),
        serde_json::to_string(&result_b.findings).unwrap(),
        "byte-identical {what} required under input reordering"
    );
    (result_a, result_b)
}

/// The `--changed-since` analysis shape: one `analyze_with_modes` run over `tests` scoped to
/// `changed`, under the given config and file count.
fn analyze_changed(
    tests: Vec<TestCase>,
    config: &Config,
    files_analyzed: usize,
    changed: &BTreeSet<PathBuf>,
) -> AnalysisResult {
    analyze_with_modes(
        tests,
        config,
        files_analyzed,
        IncompleteCauses::default(),
        RunModes {
            changed_files: Some(changed),
            baseline: None,
            suppressions: &[],
        },
    )
    .unwrap()
}

/// The config every `cross-file-duplicate` test uses: the rule at Warning, threshold 0.
/// The alpha-equal pair split across two files: `cross-file-duplicate`'s whole subject, and the
/// prologue its tests share verbatim.
fn cross_file_pair() -> Vec<TestCase> {
    let body = "expect(add(1, 1)).toBe(2)";
    vec![
        T::new("a.test.ts", 1, "adds in file a", body).build(),
        T::new("b.test.ts", 1, "adds in file b", body).build(),
    ]
}

/// Enable cross-file duplicate warnings in the analysis fixture configuration.
fn cross_file_config() -> Config {
    config_with(&[(Rule::CrossFileDuplicate, Severity::Warning)], 0)
}

/// Analyzes one test carrying `hooks`, under `config` -- the setup every `redundant-hook` test
/// reduces to, differing only in the hook chain and the expectation.
fn analyze_hooked(hooks: Vec<Hook>, config: &Config) -> AnalysisResult {
    let tests = vec![T::new("h.test.ts", 30, "monthly", "alpha-one")
        .hooks(hooks)
        .build()];
    analyze_with(tests, config, 1)
}

/// Count findings produced by the requested rule.
fn rule_count(findings: &[Finding], rule: Rule) -> usize {
    findings.iter().filter(|f| f.rule == rule).count()
}

// ---------------------------------------------------------------------------------------------
// One test per fixtures/duplicates/*.test.ts
// ---------------------------------------------------------------------------------------------

/// fixtures/duplicates/duplicates-same-context-a.test.ts
/// Two tests, distinct titles ("adds two numbers" / "sums one and one"), identical bodies
/// (`expect(add(1, 1)).toBe(2)`), sharing one file-level `beforeEach` (`reset()`) and the same
/// file-level import (`reset, add` from `./calculator`). Expected: duplicate-test only -- the
/// baseline positive case.
#[test]
fn fixture_duplicates_same_context_a() {
    let shared_hook = hook(HookKind::BeforeEach, "reset()", 3);
    let tests = vec![
        T::new(
            "a.test.ts",
            6,
            "adds two numbers",
            "expect(add(1, 1)).toBe(2)",
        )
        .hooks(vec![shared_hook.clone()])
        .imports(&["./calculator"])
        .build(),
        T::new(
            "a.test.ts",
            10,
            "sums one and one",
            "expect(add(1, 1)).toBe(2)",
        )
        .hooks(vec![shared_hook])
        .imports(&["./calculator"])
        .build(),
    ];
    let result = analyze_default(tests);
    let (dup_test, dup_name) = rule_counts(&result.findings);
    assert_eq!(
        dup_test, 1,
        "identical bodies under equivalent context must duplicate"
    );
    assert_eq!(
        dup_name, 0,
        "titles differ, so duplicate-test-name must not fire"
    );
    assert!(!result.incomplete());
}

/// fixtures/duplicates/not-duplicates-different-hooks.test.ts
/// Two tests with distinct titles ("computation one" / "computation two") under different
/// `describe`s whose `beforeEach` bodies differ in real code (`total = 1` vs `total = 99`), not
/// merely in a comment -- comments are stripped during fingerprinting, so a comment-only
/// difference would make the hooks compare equal and this fixture would assert nothing.
/// Expected: not a duplicate under either rule (hook context differs; full titles also differ
/// both from the differing suite prefix and from the differing titles themselves).
#[test]
fn fixture_not_duplicates_different_hooks() {
    let tests = vec![
        T::new(
            "b.test.ts",
            8,
            "computation one",
            "expect(total + 1).toBe(2)",
        )
        .suite(&["group 1"])
        .hooks(vec![hook(HookKind::BeforeEach, "total = 1", 7)])
        .build(),
        T::new(
            "b.test.ts",
            14,
            "computation two",
            "expect(total + 1).toBe(2)",
        )
        .suite(&["group 2"])
        .hooks(vec![hook(HookKind::BeforeEach, "total = 99", 13)])
        .build(),
    ];
    let result = analyze_default(tests);
    assert!(result.findings.is_empty());
}

/// fixtures/duplicates/not-duplicates-different-mocks-a.test.ts and -b.test.ts.
///
/// Both real files import `value` from `./module-a` and mock that same specifier
/// (`vi.mock('./module-a', ...)`), with bodies that are textually identical
/// (`expect(value).toBeGreaterThan(0)`). The only difference between the two files is the mock
/// factory's return value (`{ value: 1 }` vs `{ value: 2 }`). `Context.mocked_modules` now carries
/// a `MockedModule` per entry -- specifier plus an alpha-normalized fingerprint of the factory --
/// so the two files' factory fingerprints differ even though the specifier matches, and
/// `Context::is_equivalent`'s plain `==` on `mocked_modules` sees that difference with no change
/// to that method. Expected: not a duplicate.
#[test]
fn fixture_not_duplicates_different_mocks() {
    let body = "expect(value).toBeGreaterThan(0)";
    let tests = vec![
        T::new("mocks-a.test.ts", 8, "reads the mocked value", body)
            .mocked_modules(vec![mocked("./module-a", Some("({value: 1})"))])
            .imports(&["./module-a"])
            .build(),
        T::new("mocks-b.test.ts", 8, "reads the mocked value", body)
            .mocked_modules(vec![mocked("./module-a", Some("({value: 2})"))])
            .imports(&["./module-a"])
            .build(),
    ];
    let result = analyze_with(tests, &default_config(), 2);
    let (dup_test, dup_name) = rule_counts(&result.findings);
    assert_eq!(
        dup_test, 0,
        "different factory fingerprints for the same specifier must not merge"
    );
    assert_eq!(
        dup_name, 0,
        "different files: duplicate-test-name is scoped per file"
    );
}

/// Not a corpus fixture: exercises the `Option<String>` half of `MockedModule::factory` directly.
/// `vi.mock('./x')` with no second argument (automocking, `factory: None`) and
/// `vi.mock('./x', () => ({...}))` (an explicit replacement, `factory: Some(..)`) are documented
/// in `model.rs` as distinct, meaningful states -- automocking is not "no information," it is a
/// specific different behavior. An implementation that treated `None` as a wildcard (for example
/// by comparing only `Option::is_some()`, or by defaulting a missing factory to the same sentinel
/// used elsewhere) would wrongly merge these. `MockedModule`'s derived `Eq` already distinguishes
/// `None` from any `Some`, so this is really a regression guard on that behavior staying wired
/// through `ContextKey` and `Context::is_equivalent` unchanged.
#[test]
fn automocking_and_an_explicit_factory_are_not_equivalent_contexts() {
    let body = "expect(value).toBeDefined()";
    let tests = vec![
        T::new("automock.test.ts", 1, "uses the module", body)
            .mocked_modules(vec![mocked("./x", None)])
            .build(),
        T::new("explicit-factory.test.ts", 2, "uses the module too", body)
            .mocked_modules(vec![mocked("./x", Some("({value: 1})"))])
            .build(),
    ];
    let result = analyze_with(tests, &default_config(), 2);
    assert!(
        result.findings.is_empty(),
        "automocking (None) must not be treated as equivalent to an explicit factory (Some)"
    );
}

/// Not a corpus fixture: mirrors the mock-factory fix, this time for `test.extend` fixtures.
/// Two files each extend with a fixture literally named `db`, but the fixtures' setup bodies
/// differ (`FixtureBinding.body`, the alpha fingerprint of the setup function). Before
/// `FixtureBinding` existed, `Context.fixtures` was just fixture *names*, so this pair would have
/// been the same class of bug as the mock-factory gap: same name, different behavior, wrongly
/// merged. Expected: not a duplicate.
#[test]
fn same_fixture_name_with_different_setup_bodies_is_not_an_equivalent_context() {
    let body = "expect(db.isReady).toBe(true)";
    let tests = vec![
        T::new("fixture-a.test.ts", 1, "uses the db fixture", body)
            .fixtures(vec![fixture("db", Some("createTestDb()"))])
            .build(),
        T::new("fixture-b.test.ts", 2, "uses the db fixture too", body)
            .fixtures(vec![fixture("db", Some("createProdLikeDb()"))])
            .build(),
    ];
    let result = analyze_with(tests, &default_config(), 2);
    assert!(
        result.findings.is_empty(),
        "same fixture name with different setup bodies must not be treated as equivalent"
    );
}

/// Pins the load-bearing assumption behind leaving `Context.imports` as bare specifiers (no
/// imported-name list): the design holds that `body.alpha` already gives a free/imported
/// reference an identity derived from its *binding origin* (which name, from which module), so
/// `import { a } from './x'` and `import { b } from './x'` produce different `alpha` values one
/// level down, before `Context.imports` (`["./x"]` in both cases) is ever consulted.
///
/// This test can only pin the analysis-layer half of that claim: GIVEN two structurally identical
/// bodies whose `alpha` already differs (as the assumption predicts for two different named
/// imports), `duplicate-test` correctly refuses to merge them even though `Context.imports` alone
/// would call the two contexts equivalent. It does NOT verify that the real frontend actually
/// produces different `alpha` values for `import { a } from './x'` vs `import { b } from './x'`
/// at an otherwise-identical call site -- that requires `src/frontend`'s fingerprint module,
/// which does not build yet and is out of this module's reach by the IR boundary's own design (a
/// hand-built `TestCase` chooses its own `alpha`; it cannot observe what a real extractor would
/// choose). Reporting this explicitly rather than letting the gap pass silently: if a future
/// integration test through the real frontend shows two such imports collapsing to the *same*
/// `alpha`, that is the mock-factory bug a third time, and `Context.imports` needs the imported
/// names, matching the shape of `MockedModule`/`FixtureBinding`.
#[test]
fn named_imports_from_the_same_module_are_assumed_to_get_different_alpha() {
    let tests = vec![
        T::new("names-a.test.ts", 1, "calls a", "a()")
            .imports(&["./x"])
            .build(),
        T::new("names-b.test.ts", 2, "calls b", "b()")
            .imports(&["./x"])
            .build(),
    ];
    let result = analyze_with(tests, &default_config(), 2);
    assert!(
        result.findings.is_empty(),
        "different named imports from the same module must not be reported as duplicates, \
         given the assumption that their alpha fingerprints differ"
    );
}

/// fixtures/duplicates/not-duplicates-literal-variation.test.ts -- see that file's header comment
/// for why its stale `not-duplicates-` name is kept as-is now that `parameterization-candidate`
/// reports it (a `src/frontend` test reads it by hardcoded path).
/// Two tests, different titles, bodies differing only in literal values (so `body.alpha` differs
/// -- `alpha` preserves literals per `Fingerprints`' doc comment -- while `body.structural`
/// collapses those literals to their kind and so compares equal).
/// Expected: not `duplicate-test` (bodies are not alpha-equal), but a `parameterization-candidate`
/// -- exactly the pair that rule exists to report.
#[test]
fn fixture_not_duplicates_literal_variation_is_a_parameterization_candidate() {
    let structural = "expect(area(<number>, <number>)).toBe(<number>)";
    let tests = vec![
        T::new("d.test.ts", 3, "add 1 + 1", "expect(area(2, 3)).toBe(6)")
            .structural(structural)
            .behavior(&["assert:toBe"])
            .build(),
        T::new("d.test.ts", 7, "add 2 + 3", "expect(area(4, 5)).toBe(20)")
            .structural(structural)
            .behavior(&["assert:toBe"])
            .build(),
    ];
    let result = analyze_default(tests);
    let (dup_test, dup_name) = rule_counts(&result.findings);
    assert_eq!(dup_test, 0, "differing alpha must not fire duplicate-test");
    assert_eq!(dup_name, 0, "titles differ");
    assert_eq!(
        rule_count(&result.findings, Rule::ParameterizationCandidate),
        1,
        "equal structural, differing alpha, same file and context: exactly a parameterization \
         candidate"
    );
}

/// fixtures/duplicates/duplicate-test-name.test.ts
/// Two tests, same title, different bodies. Expected: duplicate-test-name only.
#[test]
fn fixture_duplicate_test_name() {
    let tests = vec![
        T::new("e.test.ts", 4, "computation", "expect(1+1).toBe(2)").build(),
        T::new("e.test.ts", 8, "computation", "expect(2+3).toBe(5)").build(),
    ];
    let result = analyze_default(tests);
    let (dup_test, dup_name) = rule_counts(&result.findings);
    assert_eq!(dup_test, 0);
    assert_eq!(dup_name, 1);
    assert_eq!(result.findings[0].severity, Severity::Warning);
}

/// fixtures/duplicates/duplicates-one-skipped.test.ts
/// Two tests with distinct titles ("validates input" / "validates input again"), identical
/// non-trivial bodies (`const outcome = validate('abc'); expect(outcome.ok).toBe(true)`), same
/// file-level import (`validate` from `./validate`), one `.skip`. Bodies must be non-trivial or
/// the triviality guard would exclude them from comparison and this fixture would assert nothing.
/// Expected: duplicate-test, with the finding calling out the inert participant. Titles now
/// differ, so duplicate-test-name must NOT also fire (unlike the previous fixture revision).
#[test]
fn fixture_duplicates_one_skipped() {
    let body = "const outcome = validate('abc'); expect(outcome.ok).toBe(true)";
    let tests = vec![
        T::new("f.test.ts", 6, "validates input", body)
            .imports(&["./validate"])
            .build(),
        T::new("f.test.ts", 11, "validates input again", body)
            .imports(&["./validate"])
            .skip()
            .build(),
    ];
    let result = analyze_default(tests);
    let (dup_test, dup_name) = rule_counts(&result.findings);
    assert_eq!(dup_test, 1);
    assert_eq!(
        dup_name, 0,
        "titles differ, so duplicate-test-name must not fire"
    );
    let duplicate = result
        .findings
        .iter()
        .find(|f| f.rule == Rule::DuplicateTest)
        .unwrap();
    assert!(
        duplicate.message.contains("inert"),
        "message must call out the skipped participant: {}",
        duplicate.message
    );
    // The message is for humans; `inert` is the machine answer, and the two must agree. Asserting
    // only the prose is how a reporter came to recover this by substring-matching the message.
    assert_eq!(
        duplicate.inert,
        vec![false, true],
        "the second participant is the `.skip` one"
    );
    assert!(duplicate.has_inert_participant());
    assert_eq!(duplicate.inert_titles(), vec!["validates input again"]);
}

/// fixtures/duplicates/duplicates-different-suites-no-hooks.test.ts
/// Two tests, identical non-trivial bodies (`const element = render('button');
/// expect(element.tagName).toBe('BUTTON')`), distinct titles ("renders button in A" / "... in
/// B"), under different `describe`s, no hooks anywhere, same file-level import (`render` from
/// `./render`). Bodies must be non-trivial or the triviality guard would exclude them from
/// comparison. Expected: duplicate-test (suite path differs but behavior doesn't);
/// duplicate-test-name must NOT fire (titles and suite prefixes both differ).
#[test]
fn fixture_duplicates_different_suites_no_hooks() {
    let body = "const element = render('button'); expect(element.tagName).toBe('BUTTON')";
    let tests = vec![
        T::new("g.test.ts", 7, "renders button in A", body)
            .suite(&["Suite A"])
            .imports(&["./render"])
            .build(),
        T::new("g.test.ts", 13, "renders button in B", body)
            .suite(&["Suite B"])
            .imports(&["./render"])
            .build(),
    ];
    let result = analyze_default(tests);
    let (dup_test, dup_name) = rule_counts(&result.findings);
    assert_eq!(dup_test, 1);
    assert_eq!(dup_name, 0);
}

/// fixtures/duplicates/duplicates-alpha-equivalent.test.ts
/// Two tests with distinct titles ("processes result via value" / "... via num") whose bodies are
/// alpha-equivalent (`const value = compute(42); expect(value).toBe(42)` vs the same with `num`
/// in place of `value` -- alpha-normalization renames locals by resolved symbol, so both collapse
/// to the same `alpha` string), same file-level import (`compute` from `./compute`). Titles are
/// deliberately distinct so this fixture isolates duplicate-test from duplicate-test-name.
/// Expected: duplicate-test only.
#[test]
fn fixture_duplicates_alpha_equivalent() {
    let alpha = "const $1 = compute(42); expect($1).toBe(42)";
    let tests = vec![
        T::new("h.test.ts", 7, "processes result via value", alpha)
            .imports(&["./compute"])
            .build(),
        T::new("h.test.ts", 12, "processes result via num", alpha)
            .imports(&["./compute"])
            .build(),
    ];
    let result = analyze_default(tests);
    let (dup_test, dup_name) = rule_counts(&result.findings);
    assert_eq!(dup_test, 1);
    assert_eq!(
        dup_name, 0,
        "titles are deliberately distinct in the corrected fixture"
    );
}

/// fixtures/duplicates/not-duplicates-local-vs-imported-name.test.ts
///
/// No longer ambiguous: the file now has a real `import { page } from './page'`. The first test's
/// body reads that imported `page` directly (a free reference resolving to the import, so
/// `Context.imports` includes `./page` and `alpha` keeps the identifier `page` verbatim, since
/// alpha-normalization only renames locals). The second test shadows it with its own
/// `const page = {...}` (a local declaration, so it does *not* resolve to the import --
/// `Context.imports` is empty there -- and `alpha` gains an extra local-declaration statement plus
/// a renamed reference). Both fields the rule inspects differ for a principled reason, not a
/// guess: `Context.imports` (`["./page"]` vs `[]`) and `body.alpha` (no declaration vs a `const`
/// declaration). Titles are also now distinct, so this fixture isolates duplicate-test from
/// duplicate-test-name. Expected: not a duplicate-test.
#[test]
fn fixture_not_duplicates_local_vs_imported_name() {
    let tests = vec![
        T::new(
            "i.test.ts",
            6,
            "reads url from the imported page",
            "expect(page.url).toContain('example')",
        )
        .imports(&["./page"])
        .build(),
        T::new(
            "i.test.ts",
            10,
            "reads url from a locally built page",
            "const $1 = {url: 'https://example.com'}; expect($1.url).toContain('example')",
        )
        .build(),
    ];
    let result = analyze_default(tests);
    let (dup_test, dup_name) = rule_counts(&result.findings);
    assert_eq!(
        dup_test, 0,
        "semantic parser must not merge an import with a shadowing local"
    );
    assert_eq!(
        dup_name, 0,
        "titles are deliberately distinct in the corrected fixture"
    );
}

/// fixtures/duplicates/not-duplicates-different-helper-modules-a.test.ts and -b.test.ts.
///
/// Now a real two-file pair, no fabrication needed: each file has one test titled "formats a
/// date" with an identical body (`expect(format('2024-01-01')).toBe('01/01/2024')`), but file `a`
/// imports `format` from `./utils-a` and file `b` imports it from `./utils-b`.
/// Expected: not a duplicate-test (imports differ, so `Context::is_equivalent` is false).
/// duplicate-test-name cannot fire either since the two tests are in different files.
#[test]
fn fixture_not_duplicates_different_helper_modules() {
    let body = "expect(format('2024-01-01')).toBe('01/01/2024')";
    let tests = vec![
        T::new("helper-modules-a.test.ts", 6, "formats a date", body)
            .imports(&["./utils-a"])
            .build(),
        T::new("helper-modules-b.test.ts", 6, "formats a date", body)
            .imports(&["./utils-b"])
            .build(),
    ];
    let result = analyze_with(tests, &default_config(), 2);
    let (dup_test, dup_name) = rule_counts(&result.findings);
    assert_eq!(
        dup_test, 0,
        "helpers resolved from different modules must not merge"
    );
    assert_eq!(
        dup_name, 0,
        "different files: duplicate-test-name is scoped per file"
    );
}

// ---------------------------------------------------------------------------------------------
// Adversarial / structural cases
// ---------------------------------------------------------------------------------------------

/// Regression fixture for analysis rules: empty test list yields empty result.
#[test]
fn empty_test_list_yields_empty_result() {
    let result = analyze(
        Vec::new(),
        &default_config(),
        0,
        IncompleteCauses::default(),
    )
    .unwrap();
    assert!(result.findings.is_empty());
    assert_eq!(result.tests_analyzed, 0);
    assert_eq!(result.files_analyzed, 0);
    assert!(!result.incomplete());
    assert!(threshold_passed(&result, &config_with(&[], 0)));
}

/// Regression fixture for analysis rules: single test yields no findings.
#[test]
fn single_test_yields_no_findings() {
    let tests = vec![T::new("s.test.ts", 1, "only test", "expect(1).toBe(1)").build()];
    let result = analyze_default(tests);
    assert!(result.findings.is_empty());
    assert_eq!(result.tests_analyzed, 1);
}

/// Regression fixture for analysis rules: group of exactly four produces a linear spanning set.
#[test]
fn group_of_exactly_four_produces_a_linear_spanning_set() {
    let tests = numbered_group("spanning.test.ts", 4, "expect(1).toBe(1)", "case");
    let result = analyze_default(tests);
    let dup_test: Vec<_> = result
        .findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateTest)
        .collect();
    // windows(2) over 4 members: exactly 3 pair findings, none of them a cluster.
    assert_eq!(dup_test.len(), 3);
    for finding in &dup_test {
        assert_eq!(finding.locations.len(), 2);
    }
    assert!(!result.incomplete());
}

/// Regression fixture for analysis rules: group of exactly five collapses into one cluster
/// finding.
#[test]
fn group_of_exactly_five_collapses_into_one_cluster_finding() {
    let tests = numbered_group("cluster.test.ts", 5, "expect(1).toBe(1)", "case");
    let result = analyze_default(tests);
    let dup_test: Vec<_> = result
        .findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateTest)
        .collect();
    assert_eq!(
        dup_test.len(),
        1,
        "5+ members must collapse into a single finding"
    );
    assert_eq!(dup_test[0].locations.len(), 5);
    assert!(dup_test[0].message.contains('5'));
    assert!(
        dup_test[0].message.to_lowercase().contains("cluster"),
        "cluster message must explicitly signal truncation: {}",
        dup_test[0].message
    );
}

/// Regression fixture for analysis rules: empty alpha bodies still compare equal when not
/// trivial.
#[test]
fn empty_alpha_bodies_still_compare_equal_when_not_trivial() {
    // Same file (post-A7, `duplicate-test` no longer groups across files): the point of this test
    // is that an empty-but-non-trivial `alpha` still compares equal, not cross-file behavior.
    let tests = vec![
        T::new("empty.test.ts", 1, "a", "").build(),
        T::new("empty.test.ts", 2, "b", "").build(),
    ];
    let result = analyze_default(tests);
    assert_eq!(rule_counts(&result.findings).0, 1);
}

/// Regression fixture for analysis rules: trivial bodies never participate even with equal
/// alpha.
#[test]
fn trivial_bodies_never_participate_even_with_equal_alpha() {
    let tests = vec![
        T::new("t1.test.ts", 1, "a", "").trivial().build(),
        T::new("t2.test.ts", 2, "b", "").trivial().build(),
    ];
    let result = analyze_with(tests, &default_config(), 2);
    assert!(result.findings.is_empty());
}

/// Regression fixture for analysis rules: todo tests have no body and never participate in
/// duplicate test.
#[test]
fn todo_tests_have_no_body_and_never_participate_in_duplicate_test() {
    let tests = vec![
        T::new("todo1.test.ts", 1, "a", "expect(1).toBe(1)")
            .todo()
            .build(),
        T::new("todo2.test.ts", 2, "b", "expect(1).toBe(1)")
            .todo()
            .build(),
    ];
    let result = analyze_with(tests, &default_config(), 2);
    assert!(result.findings.is_empty());
}

#[test]
fn unresolved_context_never_participates_in_duplicate_test() {
    // One test's context could not be fully resolved (`Context::unresolved`); the other's body
    // and context match it exactly. `context.is_comparable()` must exclude the unresolved test
    // from grouping up front, so no duplicate-test finding is asserted on top of an ambient fact
    // the extractor could not actually read -- even though, textually, everything else lines up.
    let tests = vec![
        T::new("resolved.test.ts", 1, "a", "expect(1).toBe(1)").build(),
        T::new("unresolved.test.ts", 2, "b", "expect(1).toBe(1)")
            .unresolved()
            .build(),
    ];
    let result = analyze_with(tests, &default_config(), 2);
    assert!(
        result.findings.is_empty(),
        "a test with an unresolved context must never anchor a duplicate-test finding"
    );
    assert!(
        result.incomplete(),
        "excluding an unresolved-context test must mark the result incomplete: absence of a \
         finding for it proves nothing"
    );
}

#[test]
fn two_unresolved_contexts_that_are_otherwise_identical_still_produce_nothing() {
    // Both participants have unresolved contexts, and are otherwise identical in every field
    // (same file, same title, same body, same empty Context) -- the kind of input that would
    // produce both a duplicate-test and a duplicate-test-name finding if unresolved contexts were
    // not filtered before grouping. Neither rule may fire.
    let tests = vec![
        T::new("u.test.ts", 1, "same title", "expect(1).toBe(1)")
            .unresolved()
            .build(),
        T::new("u.test.ts", 5, "same title", "expect(1).toBe(1)")
            .unresolved()
            .build(),
    ];
    let result = analyze_with(tests, &default_config(), 2);
    assert!(
        result.findings.is_empty(),
        "two unresolved-context tests must not duplicate each other under either rule, even when \
         everything else about them matches"
    );
    assert!(result.incomplete());
}

#[test]
fn a_file_with_every_test_excluded_is_named_rather_than_only_counted() {
    // The failure this guards is not a wrong number, it is an invisible file. A file whose tests
    // are all excluded produces no findings, so it appears nowhere in any report -- which reads
    // exactly like a file that was compared and came back clean. `unresolved_tests` cannot
    // distinguish the two, because it is a corpus-wide count.
    let tests = vec![
        T::new("blind.test.ts", 1, "a", "expect(1).toBe(1)")
            .unresolved()
            .build(),
        T::new("blind.test.ts", 2, "b", "expect(2).toBe(2)")
            .unresolved()
            .build(),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        result.blind_files.len(),
        1,
        "a file with every test excluded must be reported as blind"
    );
    assert_eq!(result.blind_files[0].path, PathBuf::from("blind.test.ts"));
    assert_eq!(
        result.blind_files[0].tests, 2,
        "the count must be the file's executable tests, all of them excluded"
    );
}

#[test]
fn a_partially_excluded_file_is_not_blind() {
    // The boundary that matters: one resolved test is enough to put the file in the report, where
    // its findings (or absence of them) are qualified by the corpus-wide count. Only a WHOLLY
    // excluded file is invisible, so only it earns the louder signal. A guard that counted any
    // exclusion would fire on 123 files here instead of 35 and be ignored within a week.
    let tests = vec![
        T::new("partial.test.ts", 1, "a", "expect(1).toBe(1)").build(),
        T::new("partial.test.ts", 2, "b", "expect(2).toBe(2)")
            .unresolved()
            .build(),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        result.unresolved_tests, 1,
        "the excluded test must still be counted corpus-wide"
    );
    assert!(
        result.blind_files.is_empty(),
        "a file with one comparable test still appears in the report and is not blind"
    );
}

#[test]
fn blind_files_are_ordered_worst_first_so_a_bounded_list_stays_honest() {
    // The list is truncated for display. Ordered by path, an alphabetically late file with a
    // hundred tests would fall off the end behind single-test files, and the truncation would
    // hide exactly the file that cost the most coverage.
    let mut tests = vec![T::new("aaa-small.test.ts", 1, "only", "expect(1).toBe(1)")
        .unresolved()
        .build()];
    for (line, title) in [(1, "t1"), (2, "t2"), (3, "t3")] {
        tests.push(
            T::new("zzz-large.test.ts", line, title, "expect(1).toBe(1)")
                .unresolved()
                .build(),
        );
    }
    let result = analyze_with(tests, &default_config(), 2);
    let order: Vec<_> = result
        .blind_files
        .iter()
        .map(|file| (file.path.to_string_lossy().to_string(), file.tests))
        .collect();
    assert_eq!(
        order,
        vec![
            ("zzz-large.test.ts".to_owned(), 3),
            ("aaa-small.test.ts".to_owned(), 1),
        ],
        "the file costing the most tests must sort first, ahead of path order"
    );
}

#[test]
fn threshold_exactly_at_the_boundary_passes() {
    // "a" and "b" duplicate each other (2 flagged locations); "c" and "d" are unique. 2 flagged
    // out of 4 discovered tests = 50%. Threshold 50 must pass (<=), threshold 49 must fail.
    let tests = vec![
        T::new("th.test.ts", 1, "a", "expect(1).toBe(1)").build(),
        T::new("th.test.ts", 2, "b", "expect(1).toBe(1)").build(),
        T::new("th.test.ts", 3, "c", "expect(2).toBe(2)").build(),
        T::new("th.test.ts", 4, "d", "expect(3).toBe(3)").build(),
    ];
    let result = analyze(
        tests.clone(),
        &config_with(&[], 50),
        1,
        IncompleteCauses::default(),
    )
    .unwrap();
    assert_eq!(rule_counts(&result.findings).0, 1);
    assert!(threshold_passed(&result, &config_with(&[], 50)));

    let result = analyze(tests, &config_with(&[], 49), 1, IncompleteCauses::default()).unwrap();
    assert!(!threshold_passed(&result, &config_with(&[], 49)));
}

/// `Severity::Off` means a rule does no work, not that its findings are filtered out afterwards.
/// Two identical bodies under different titles in two files are visible to exactly two rules --
/// `duplicate-test` is file-scoped and sees nothing here, so the pair belongs to
/// `cross-file-duplicate` -- and both are named `Off` below, which is what makes "no findings at
/// all" the whole claim rather than "no findings from the one rule we remembered".
///
/// `cross-file-duplicate` joined that list when it stopped shipping `Off`: before the flip this
/// case passed while naming only `duplicate-test`, because the second rule was silent for a
/// reason the test never stated.
#[test]
fn rule_set_to_off_produces_no_findings_at_all() {
    let tests = vec![
        T::new("off1.test.ts", 1, "one title", "expect(1).toBe(1)").build(),
        T::new("off2.test.ts", 2, "another title", "expect(1).toBe(1)").build(),
    ];
    let result = analyze(
        tests,
        &config_with(
            &[
                (Rule::DuplicateTest, Severity::Off),
                (Rule::CrossFileDuplicate, Severity::Off),
            ],
            0,
        ),
        2,
        IncompleteCauses::default(),
    )
    .unwrap();
    assert!(result.findings.is_empty(), "{:?}", result.findings);
}

#[test]
fn determinism_under_shuffled_input_order() {
    let shared_hook = hook(HookKind::BeforeEach, "// setup", 1);
    let build = || {
        vec![
            T::new("z.test.ts", 20, "case c", "expect(1).toBe(1)")
                .hooks(vec![shared_hook.clone()])
                .build(),
            T::new("z.test.ts", 5, "case a", "expect(1).toBe(1)")
                .hooks(vec![shared_hook.clone()])
                .build(),
            T::new("z.test.ts", 12, "case b", "expect(1).toBe(1)")
                .hooks(vec![shared_hook.clone()])
                .build(),
            T::new("y.test.ts", 3, "same title", "expect(2).toBe(2)").build(),
            T::new("y.test.ts", 8, "same title", "expect(3).toBe(3)").build(),
            T::new("solo.test.ts", 1, "unique", "expect(9).toBe(9)").build(),
        ]
    };

    let (result_a, _result_b) = findings_stable_under_reordering(build, 3, "findings output");
    assert!(
        !result_a.findings.is_empty(),
        "sanity: the scenario must actually produce findings"
    );
}

#[test]
fn exit_code_succeeds_with_no_error_findings_and_threshold_within_range() {
    let tests = vec![T::new("ok.test.ts", 1, "only test", "expect(1).toBe(1)").build()];
    let config = default_config();
    let result = analyze_with(tests, &config, 1);
    assert_eq!(exit_code(&result, &config), ExitCode::Success);
}

#[test]
fn exit_code_fails_the_quality_gate_on_an_error_severity_finding() {
    // Same file (post-A7, `duplicate-test` no longer groups across files): what this test pins is
    // the exit-code contract, which needs the finding to actually fire, not cross-file behavior.
    let tests = vec![
        T::new("dup.test.ts", 1, "a", "expect(1).toBe(1)").build(),
        T::new("dup.test.ts", 2, "b", "expect(1).toBe(1)").build(),
    ];
    let config = default_config(); // duplicate-test defaults to Error.
    let result = analyze_with(tests, &config, 1);
    assert_eq!(exit_code(&result, &config), ExitCode::QualityGateFailed);
}

#[test]
fn exit_code_at_the_default_threshold_fails_on_an_error_finding_even_when_the_threshold_itself_would_pass(
) {
    // At the *default* threshold (0), `had_error_finding` still gates the outcome on its own,
    // independent of the (already-passing) percentage check -- this is the truncation-safety net
    // `AnalysisResult::had_error_finding`'s doc comment describes: a single retained Error finding
    // must fail a `--threshold 0` run even in the degenerate case where `threshold_passed`'s own
    // percentage math would have said yes.
    //
    // Same file (post-A7, `duplicate-test` no longer groups across files): the finding must still
    // fire so the isolation this test performs (threshold vs. error-finding) stays meaningful.
    let tests = vec![
        T::new("dup.test.ts", 1, "a", "expect(1).toBe(1)").build(),
        T::new("dup.test.ts", 2, "b", "expect(1).toBe(1)").build(),
    ];
    let config = config_with(&[], 0);
    let result = analyze_with(tests, &config, 1);
    assert_eq!(exit_code(&result, &config), ExitCode::QualityGateFailed);
}

#[test]
fn exit_code_honors_a_configured_nonzero_threshold_even_with_an_error_finding() {
    // Regression: a configured non-zero threshold must actually mean something for
    // `duplicate-test`/`duplicate-test-name`, which default to Error severity. Before this fix,
    // `exit_code` failed on any retained Error finding unconditionally, which made every
    // non-zero `--threshold` value ineffective against the two rules a threshold most needs to
    // govern -- `--threshold 100` still failed the same as `--threshold 0`.
    //
    // Same file (post-A7, `duplicate-test` no longer groups across files): the finding must still
    // fire so this test actually exercises the interaction it claims to.
    let tests = vec![
        T::new("dup.test.ts", 1, "a", "expect(1).toBe(1)").build(),
        T::new("dup.test.ts", 2, "b", "expect(1).toBe(1)").build(),
    ];
    let config = config_with(&[], 100);
    let result = analyze_with(tests, &config, 1);
    assert!(result.had_error_finding, "sanity: the finding must fire");
    assert!(
        threshold_passed(&result, &config),
        "100% threshold must always pass"
    );
    assert_eq!(
        exit_code(&result, &config),
        ExitCode::Success,
        "a threshold of 100 must permit error-severity findings, per its documented semantics"
    );
}

#[test]
fn exit_code_still_fails_a_nonzero_threshold_once_the_error_rate_exceeds_it() {
    // The other half of that fix: a non-zero threshold must still fail once the configured rate is
    // actually crossed -- this is not "non-zero threshold disables the gate", only "non-zero
    // threshold changes what rate is acceptable".
    let tests = vec![
        T::new("dup.test.ts", 1, "a", "expect(1).toBe(1)").build(),
        T::new("dup.test.ts", 2, "b", "expect(1).toBe(1)").build(),
    ];
    let config = config_with(&[], 10);
    let result = analyze_with(tests, &config, 1);
    assert!(
        !threshold_passed(&result, &config),
        "both tests flagged out of 2 analyzed is 100%, above a 10% threshold"
    );
    assert_eq!(exit_code(&result, &config), ExitCode::QualityGateFailed);
}

/// `main.rs` cannot reach `reporters::shared::terminal_safe` directly (it is `pub(crate)`
/// inside this library, and `main.rs` compiles as a separate binary crate that only sees this
/// crate's fully `pub` surface), so it goes through this re-export instead. This pins that the
/// re-export is not a second, drifted implementation: a filename-shaped string carrying both a
/// newline (which would forge an extra report line) and a raw ANSI escape (which could rewrite
/// how a terminal displays the rest of the line) comes out exactly as
/// `reporters::shared::terminal_safe` itself would render it -- newline gone, escape byte gone,
/// legitimate text untouched.
#[test]
fn terminal_safe_reexport_sanitizes_newlines_and_ansi_escapes_like_the_reporters_do() {
    let hostile_filename = "evil\n\x1b[31mtest.ts";
    let sanitized = crate::reporters::terminal_safe(hostile_filename);
    assert!(
        !sanitized.contains('\n'),
        "a newline must not survive: could forge a report line"
    );
    assert!(
        !sanitized.contains('\u{1b}'),
        "a raw ESC byte must not survive: could trigger a terminal escape sequence"
    );
    assert_eq!(sanitized, "evil[31mtest.ts");
}

#[test]
fn exit_code_never_reports_operational_error() {
    // exit_code's return type only has Success and QualityGateFailed reachable from its logic;
    // this test exists as a guard so a future edit that tries to thread an operational failure
    // through this function is caught immediately.
    let empty = analyze(
        Vec::new(),
        &default_config(),
        0,
        IncompleteCauses::default(),
    )
    .unwrap();
    match exit_code(&empty, &default_config()) {
        ExitCode::Success | ExitCode::QualityGateFailed => {}
        ExitCode::OperationalError => panic!("exit_code must never return OperationalError"),
    }
}

/// D1: renamed from `pair_budget_exhaustion_marks_the_result_incomplete`. That name promised
/// end-to-end propagation into `AnalysisResult::incomplete`, but the test never called `analyze`
/// at all and never asserted `incomplete` -- it only checked `PairBudget::charge` and an unrelated
/// group-size fact. What it actually verified (still useful, kept below under an honest name) is
/// that a budget of 0 refuses any nonzero charge, and that a two-test duplicate group costs
/// exactly 1 unit. The propagation claim the old name made is now actually exercised by
/// `pair_budget_exhaustion_propagates_to_analysis_result_incomplete`.
#[test]
fn pair_budget_of_zero_refuses_a_two_member_groups_cost() {
    let mut budget = PairBudget::new(0);
    assert!(
        !budget.charge(1),
        "a budget of 0 must refuse any nonzero charge"
    );

    let tests = vec![
        T::new("budget1.test.ts", 1, "a", "expect(1).toBe(1)").build(),
        T::new("budget2.test.ts", 2, "b", "expect(1).toBe(1)").build(),
    ];
    let groups = group_and_order(&tests, |t| Some(t.body.alpha));
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0].len() - 1,
        1,
        "a 2-member group costs exactly 1 unit to report"
    );
}

/// D1's honest replacement: drives the real `analyze` code path (through the test-only
/// `analyze_with_budget` seam, since `MAX_PAIR_COMPARISONS` is 2 million and unreachable from any
/// practical test fixture) with a budget too small to afford even the first eligible group, and
/// asserts the propagation the old test's name claimed but never checked.
#[test]
fn pair_budget_exhaustion_propagates_to_analysis_result_incomplete() {
    // Same file: post-A7, `duplicate-test` only groups within one file, so a cross-file pair
    // (as the original test used) would never even form a group here, let alone charge the
    // budget -- that would make this test pass for the wrong reason.
    let tests = vec![
        T::new("budget.test.ts", 1, "a", "expect(1).toBe(1)").build(),
        T::new("budget.test.ts", 2, "b", "expect(1).toBe(1)").build(),
    ];
    let result = analyze_with_budget(
        tests,
        &default_config(),
        1,
        IncompleteCauses::default(),
        RunModes::default(),
        0,
    )
    .unwrap();
    assert!(
        result.findings.is_empty(),
        "the one eligible group costs more than the 0-unit budget, so it must be skipped entirely"
    );
    assert!(
        result.incomplete(),
        "exhausting the pair budget before every group is processed must mark the result \
         incomplete -- absence of a finding proves nothing here"
    );
}

// ---------------------------------------------------------------------------------------------
// A1: `.each` rows sharing one declaration site must never duplicate each other
// ---------------------------------------------------------------------------------------------

/// A1 regression, driven by `fixtures/each/each-array-of-arrays.test.ts`'s real shape: one
/// `test.each` table with 3 rows. The real extractor gives every row the declaration's one
/// call-site `Location`, and here all 3 rows also share a body (row-specific literals are exactly
/// what `body.alpha` would normalize away in the real fingerprint) and the same empty `Context` --
/// the worst case, since it is precisely the shape that used to collapse into one
/// `DuplicateTestKey` group and get reported as a duplicate of itself. Confirmed on the real
/// corpus before the fix: `./target/debug/vitest-dedup fixtures/each` paired
/// `each-array-of-arrays.test.ts:4:1` with itself. Expected now: zero `duplicate-test` findings.
#[test]
fn each_table_rows_never_duplicate_each_other() {
    let body = "expect(a + b).toBe(expected)";
    let tests = (0..3)
        .map(|index| {
            T::new(
                "each-array-of-arrays.test.ts",
                4,
                Box::leak(format!("add(row {index})").into_boxed_str()),
                body,
            )
            .build()
        })
        .collect::<Vec<_>>();
    let result = analyze_default(tests);
    let (dup_test, dup_name) = rule_counts(&result.findings);
    assert_eq!(
        dup_test, 0,
        "rows of one .each table share a declaration site, not a duplicate pair"
    );
    assert_eq!(
        dup_name, 0,
        "titles differ per row here, so duplicate-test-name should not fire either"
    );
}

/// A1's follow-on for `duplicate-test-name`: a `test.each` table whose title pattern references
/// none of the row data (or a table the extractor can't statically read at all, per
/// `each-nonliteral-table.test.ts`/`each-printf-directives.test.ts`) renders the identical
/// `Title::Parameterized` for every row. Bodies are deliberately made to differ per row here, so
/// this isolates `duplicate-test-name` from `duplicate-test`: without the same declaration-site
/// exclusion applied to this rule too, this is A1 wearing `duplicate-test-name`'s clothes.
#[test]
fn each_table_rows_with_identical_titles_never_duplicate_each_others_name() {
    let tests = (0..3)
        .map(|index| {
            T::new(
                "each-static-title.test.ts",
                4,
                "always the same title",
                Box::leak(format!("expect(row{index}).toBeDefined()").into_boxed_str()),
            )
            .build()
        })
        .collect::<Vec<_>>();
    let result = analyze_default(tests);
    let (_, dup_name) = rule_counts(&result.findings);
    assert_eq!(
        dup_name, 0,
        "rows of one .each table share a declaration site, not a duplicate-test-name pair"
    );
}

// A2: semantically-significant modifiers must be visible to the duplicate-test key
// ---------------------------------------------------------------------------------------------

/// A2, `.each` case: `test.each(EMAILS)('validates email %s', ...)` and
/// `test.each(PHONES)('validates phone %s', ...)` call the same validation logic (alpha-equal
/// bodies, empty `Context`) over different data. Different tables are different behavior and must
/// not be merged just because the callback happens to normalize the same way.
#[test]
fn different_each_tables_are_not_an_equivalent_modifier_key() {
    let body = "expect(validate(i).ok).toBe(true)";
    let each_of = |rows: &[&str]| {
        Modifier::Each(EachTable {
            is_for: false,
            rows: EachRows::Literal(rows.iter().map(|row| (*row).to_owned()).collect()),
        })
    };
    let tests = vec![
        T::new("validate.test.ts", 3, "validates email %s", body)
            .modifiers(vec![each_of(&["'a@x.com'", "'b@x.com'"])])
            .build(),
        T::new("validate.test.ts", 7, "validates phone %s", body)
            .modifiers(vec![each_of(&["'111'", "'222'"])])
            .build(),
    ];
    let result = analyze_default(tests);
    assert!(
        result.findings.is_empty(),
        "different .each tables driving the same body must not be reported as duplicates"
    );
}

/// A2, `skipIf` case: `test.skipIf(isWindows)('computes on posix', ...)` and
/// `test.skipIf(isLinux)('computes on non-linux', ...)` run under opposite conditions even though
/// the computation body is identical. Different guard conditions are different behavior.
#[test]
fn opposite_skip_if_conditions_are_not_an_equivalent_modifier_key() {
    let body = "expect(compute()).toBe(1)";
    let tests = vec![
        T::new("platform.test.ts", 1, "computes on posix", body)
            .modifiers(vec![Modifier::SkipIf("isWindows".to_owned())])
            .build(),
        T::new("platform.test.ts", 5, "computes on non-linux", body)
            .modifiers(vec![Modifier::SkipIf("isLinux".to_owned())])
            .build(),
    ];
    let result = analyze_default(tests);
    assert!(
        result.findings.is_empty(),
        "opposite skipIf conditions guard different run scenarios and must not duplicate"
    );
}

/// Positive control for the two tests above: the *same* `skipIf` condition on two otherwise
/// identical tests must still duplicate. Without this, a bug that made `ModifierKey` overly
/// strict (for example, comparing by identity instead of by value) could pass both negative tests
/// above for the wrong reason.
#[test]
fn same_skip_if_condition_is_still_an_equivalent_modifier_key() {
    let body = "expect(compute()).toBe(1)";
    let tests = vec![
        T::new("platform.test.ts", 1, "computes on posix a", body)
            .modifiers(vec![Modifier::SkipIf("isWindows".to_owned())])
            .build(),
        T::new("platform.test.ts", 5, "computes on posix b", body)
            .modifiers(vec![Modifier::SkipIf("isWindows".to_owned())])
            .build(),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        rule_counts(&result.findings).0,
        1,
        "identical skipIf conditions guard the same scenario and must still duplicate"
    );
}

// ---------------------------------------------------------------------------------------------
// A7: `duplicate-test` is scoped to a single file
// ---------------------------------------------------------------------------------------------

/// A7: `duplicate-test` now groups only within a single file, and so does `duplicate-test-name`.
/// Cross-file duplication belongs to `cross-file-duplicate`, a separate rule with its own
/// severity -- `Warning`, not `duplicate-test`'s `Error`, because a copy spanning two files may be
/// deliberate. Two files with byte-identical tests (same body, same title, same empty `Context`)
/// must therefore produce exactly one finding, and it must be that rule's: the file-scoped pair
/// escaping into `duplicate-test` at Error severity is the regression this guards.
#[test]
fn identical_tests_in_different_files_are_not_a_duplicate_test_under_file_scoping() {
    let body = "expect(add(1, 1)).toBe(2)";
    let tests = vec![
        T::new("calc-a.test.ts", 4, "adds two numbers", body).build(),
        T::new("calc-b.test.ts", 4, "adds two numbers", body).build(),
    ];
    let result = analyze_with(tests, &default_config(), 2);
    assert_eq!(
        rule_counts(&result.findings),
        (0, 0),
        "identical bodies across different files must not be reported by duplicate-test or \
         duplicate-test-name post-A7: {:?}",
        result.findings
    );
    assert_eq!(
        rule_count(&result.findings, Rule::CrossFileDuplicate),
        1,
        "the pair is cross-file-duplicate's, and that rule ships Warning: {:?}",
        result.findings
    );
    assert_eq!(result.findings.len(), 1, "{:?}", result.findings);
}

// ---------------------------------------------------------------------------------------------
// `parameterization-candidate` -- equal `structural`, differing `alpha`
// ---------------------------------------------------------------------------------------------

/// A shared `structural` shape used by several tests below: `expect(f(<number>)).toBe(<boolean>)`,
/// the exact shape `expect(f(0)).toBe(false)` / `expect(f(1)).toBe(true)` collapses to. The literal
/// text does not matter to the rule (only `group_and_order`'s key equality does), but keeping it
/// recognizable documents what each test is standing in for.
const PARAM_STRUCTURAL: &str = "expect(f(<number>)).toBe(<boolean>)";

/// fixtures/duplicates/duplicates-parameterization-boundary-values.test.ts
/// Two tests differing only in literal values, at a boundary (`0`/`false` vs `1`/`true`). Equal
/// `structural`, differing `alpha`, equal (empty) `Context`, same file. Expected: exactly one
/// `parameterization-candidate` finding, and neither `duplicate-test` nor `duplicate-test-name`.
#[test]
fn fixture_duplicates_parameterization_boundary_values() {
    let tests = vec![
        T::new(
            "boundary.test.ts",
            5,
            "zero is not positive",
            "expect(isPositive(0)).toBe(false)",
        )
        .structural(PARAM_STRUCTURAL)
        .behavior(&["assert:toBe"])
        .build(),
        T::new(
            "boundary.test.ts",
            9,
            "one is positive",
            "expect(isPositive(1)).toBe(true)",
        )
        .structural(PARAM_STRUCTURAL)
        .behavior(&["assert:toBe"])
        .build(),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        rule_count(&result.findings, Rule::ParameterizationCandidate),
        1,
        "equal structural + differing alpha under an equivalent context is exactly a \
         parameterization candidate"
    );
    let (dup_test, dup_name) = rule_counts(&result.findings);
    assert_eq!(
        dup_test, 0,
        "alpha differs, so duplicate-test must not also fire"
    );
    assert_eq!(dup_name, 0, "titles differ");
    assert_eq!(result.findings.len(), 1);

    let finding = &result.findings[0];
    assert_eq!(
        finding.severity,
        Severity::Warning,
        "collapsing into test.each is a readability judgement, not a defect -- Warning, not \
         Error"
    );
    // "Message wording matters": the remedy is consolidation that keeps every case, never
    // deletion, so the message must not read as license to delete one of a deliberately-covered
    // boundary pair.
    let lower = finding.message.to_lowercase();
    assert!(
        !lower.contains("duplicate") && !lower.contains("remove") && !lower.contains("delete"),
        "message must not use duplicate/remove/delete wording: {}",
        finding.message
    );
    assert!(
        lower.contains("test.each") || lower.contains("consolidat"),
        "message must name the consolidation remedy: {}",
        finding.message
    );
}

/// fixtures/duplicates/not-duplicates-structural-differs.test.ts
/// Two tests whose bodies are genuinely different shapes (a bare call+assert vs. an `if` with two
/// branches), so `body.structural` differs even though both are "assert something about a
/// function's output". Expected: no finding under any rule.
#[test]
fn fixture_not_duplicates_structural_differs() {
    let tests = vec![
        T::new(
            "shape.test.ts",
            5,
            "doubles a positive number",
            "expect(double(2)).toBe(4)",
        )
        .structural("expect(double(<number>)).toBe(<number>)")
        .behavior(&["assert:toBe"])
        .build(),
        T::new(
            "shape.test.ts",
            9,
            "rejects negative input instead of doubling it",
            "const input = -1; if (input < 0) { expect(() => double(input)).toThrow() } else { \
             expect(double(input)).toBeGreaterThan(0) }",
        )
        .structural(
            "(if (bin < id:input <number>) (expect (fn () (double id:input)).toThrow()) (expect \
             (double id:input).toBeGreaterThan(<number>)))",
        )
        .behavior(&["if", "assert:toThrow", "assert:toBeGreaterThan"])
        .build(),
    ];
    let result = analyze_default(tests);
    assert!(
        result.findings.is_empty(),
        "different structural shapes must never be reported as a parameterization candidate"
    );
}

/// Equal `structural`, equal `alpha` too: this is `duplicate-test`'s claim (byte-for-byte
/// equivalent behavior, Redundant), not `parameterization-candidate`'s (Consolidatable). The two
/// verdicts must never both fire for the same pair -- see `model.rs`'s `Verdict` doc comment on why
/// conflating them matters.
#[test]
fn equal_alpha_is_duplicate_test_not_parameterization_candidate() {
    let body = "expect(f(0)).toBe(false)";
    let tests = vec![
        T::new("equal-alpha.test.ts", 1, "case a", body)
            .structural(PARAM_STRUCTURAL)
            .behavior(&["assert:toBe"])
            .build(),
        T::new("equal-alpha.test.ts", 5, "case b", body)
            .structural(PARAM_STRUCTURAL)
            .behavior(&["assert:toBe"])
            .build(),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        rule_counts(&result.findings).0,
        1,
        "equal alpha under equal structural is duplicate-test's claim"
    );
    assert_eq!(
        rule_count(&result.findings, Rule::ParameterizationCandidate),
        0,
        "equal alpha must never also be reported as a parameterization candidate"
    );
}

/// Differing `structural`: no finding, even with differing `alpha` and an otherwise-matching
/// context. `structural` is the grouping key, and it must actually gate the rule.
#[test]
fn parameterization_candidate_requires_equal_structural() {
    let tests = vec![
        T::new(
            "diff-structural.test.ts",
            1,
            "case a",
            "expect(f(0)).toBe(false)",
        )
        .structural("expect(f(<number>)).toBe(<boolean>)")
        .behavior(&["assert:toBe"])
        .build(),
        T::new(
            "diff-structural.test.ts",
            5,
            "case b",
            "expect(g(0)).toBe(false)",
        )
        .structural("expect(g(<number>)).toBe(<boolean>)")
        .behavior(&["assert:toBe"])
        .build(),
    ];
    let result = analyze_default(tests);
    assert!(
        result.findings.is_empty(),
        "different structural shapes (different callee) must not be a parameterization candidate"
    );
}

/// A1's guard, inherited: every row of one `test.each`/`test.for` table shares the declaration's
/// single call-site `Location`. Even when rows are built here with differing `alpha` and equal
/// `structural` (the shape that would otherwise key-match), sharing one declaration site must
/// still collapse them to a single representative with nothing left to pair -- reporting the rows
/// of one `.each` as a parameterization candidate of itself would be absurd (per the task brief).
#[test]
fn each_table_rows_never_produce_a_parameterization_candidate() {
    let tests = (0..3)
        .map(|index| {
            T::new(
                "each-param.test.ts",
                4,
                Box::leak(format!("boundary row {index}").into_boxed_str()),
                Box::leak(format!("expect(f({index})).toBe(true)").into_boxed_str()),
            )
            .structural(PARAM_STRUCTURAL)
            .behavior(&["assert:toBe"])
            .build()
        })
        .collect::<Vec<_>>();
    let result = analyze_default(tests);
    assert_eq!(
        rule_count(&result.findings, Rule::ParameterizationCandidate),
        0,
        "rows of one .each table share a declaration site, not a parameterization-candidate pair"
    );
}

/// Differing `Context` (different imports) blocks `parameterization-candidate` the same way it
/// blocks `duplicate-test`: an ambient-fact mismatch means the two bodies are not comparable,
/// regardless of how their fingerprints relate.
#[test]
fn parameterization_candidate_requires_equivalent_context() {
    let tests = vec![
        T::new(
            "diff-context.test.ts",
            1,
            "case a",
            "expect(f(0)).toBe(false)",
        )
        .structural(PARAM_STRUCTURAL)
        .behavior(&["assert:toBe"])
        .imports(&["./a"])
        .build(),
        T::new(
            "diff-context.test.ts",
            5,
            "case b",
            "expect(f(1)).toBe(true)",
        )
        .structural(PARAM_STRUCTURAL)
        .behavior(&["assert:toBe"])
        .imports(&["./b"])
        .build(),
    ];
    let result = analyze_default(tests);
    assert!(
        result.findings.is_empty(),
        "differing context must block parameterization-candidate, same as duplicate-test"
    );
}

/// Trivial bodies never participate, for the same reason `duplicate-test` excludes them: an
/// empty/stub body is not worth comparing under any rule.
#[test]
fn trivial_bodies_never_participate_in_parameterization_candidate() {
    let tests = vec![
        T::new("trivial.test.ts", 1, "case a", "expect(f(0)).toBe(false)")
            .structural(PARAM_STRUCTURAL)
            .behavior(&["assert:toBe"])
            .trivial()
            .build(),
        T::new("trivial.test.ts", 5, "case b", "expect(f(1)).toBe(true)")
            .structural(PARAM_STRUCTURAL)
            .behavior(&["assert:toBe"])
            .trivial()
            .build(),
    ];
    let result = analyze_default(tests);
    assert!(
        result.findings.is_empty(),
        "trivial bodies must never seed a parameterization-candidate group"
    );
}

/// A body with zero recorded `behavior` events (the `MIN_BEHAVIOR_EVENTS_FOR_PARAMETERIZATION`
/// floor) never seeds a group, even when `structural`/`alpha` would otherwise qualify. This is the
/// substance guard, distinct from `trivial`: these bodies are not stubs, they simply have nothing
/// this rule recognizes as comparable behavior.
#[test]
fn zero_behavior_events_never_participate_in_parameterization_candidate() {
    let tests = vec![
        T::new("no-behavior.test.ts", 1, "case a", "5")
            .structural(PARAM_STRUCTURAL)
            .build(),
        T::new("no-behavior.test.ts", 5, "case b", "6")
            .structural(PARAM_STRUCTURAL)
            .build(),
    ];
    let result = analyze_default(tests);
    assert!(
        result.findings.is_empty(),
        "a body with no recorded behavior events has no substance for this rule to compare"
    );
}

/// Turning the rule `Off` does no work and reports nothing, mirroring
/// `rule_set_to_off_produces_no_findings_at_all` for `duplicate-test`.
#[test]
fn parameterization_candidate_rule_set_to_off_produces_no_findings() {
    let tests = vec![
        T::new("off.test.ts", 1, "case a", "expect(f(0)).toBe(false)")
            .structural(PARAM_STRUCTURAL)
            .behavior(&["assert:toBe"])
            .build(),
        T::new("off.test.ts", 5, "case b", "expect(f(1)).toBe(true)")
            .structural(PARAM_STRUCTURAL)
            .behavior(&["assert:toBe"])
            .build(),
    ];
    let result = analyze(
        tests,
        &config_with(&[(Rule::ParameterizationCandidate, Severity::Off)], 0),
        1,
        IncompleteCauses::default(),
    )
    .unwrap();
    assert!(result.findings.is_empty());
}

/// Determinism under shuffled input, mirroring `determinism_under_shuffled_input_order`. Four
/// distinct `alpha` values sharing one `structural`/`Context`/file exercise both
/// `representative_per_alpha` (which must not depend on scan order to pick the same
/// representatives) and `group_outcome`'s spanning-set pairing, alongside an unrelated pair in a
/// different file that must not interact with it.
#[test]
fn determinism_under_shuffled_input_order_for_parameterization_candidate() {
    let build = || {
        vec![
            T::new(
                "shuffle-param.test.ts",
                20,
                "case d",
                "expect(f(3)).toBe(true)",
            )
            .structural(PARAM_STRUCTURAL)
            .behavior(&["assert:toBe"])
            .build(),
            T::new(
                "shuffle-param.test.ts",
                5,
                "case a",
                "expect(f(0)).toBe(false)",
            )
            .structural(PARAM_STRUCTURAL)
            .behavior(&["assert:toBe"])
            .build(),
            T::new(
                "shuffle-param.test.ts",
                12,
                "case c",
                "expect(f(2)).toBe(false)",
            )
            .structural(PARAM_STRUCTURAL)
            .behavior(&["assert:toBe"])
            .build(),
            T::new(
                "shuffle-param.test.ts",
                8,
                "case b",
                "expect(f(1)).toBe(true)",
            )
            .structural(PARAM_STRUCTURAL)
            .behavior(&["assert:toBe"])
            .build(),
            T::new("solo-param.test.ts", 1, "unique", "expect(f(9)).toBe(true)")
                .structural(PARAM_STRUCTURAL)
                .behavior(&["assert:toBe"])
                .build(),
        ]
    };

    let (result_a, _result_b) =
        findings_stable_under_reordering(build, 5, "parameterization-candidate findings");
    assert!(
        !result_a
            .findings
            .iter()
            .filter(|f| f.rule == Rule::ParameterizationCandidate)
            .collect::<Vec<_>>()
            .is_empty(),
        "sanity: the scenario must actually produce parameterization-candidate findings"
    );
}

// ---------------------------------------------------------------------------------------------
// `tests_analyzed` counts executable declarations only, not `describe`/`suite` containers
// ---------------------------------------------------------------------------------------------

/// A `describe` block wrapping two real tests must report `tests_analyzed == 2`, not 3. A suite is
/// a container -- it is never itself `redundant` or `consolidatable` -- so it must not be counted
/// the same way [`is_executable_declaration`] already keeps it out of every rule's own grouping.
/// Regression guard for the bug where `AnalysisResult::tests_analyzed` was `tests.len()`
/// unfiltered, which both misreported the summary count and (see
/// `threshold_crosses_only_once_suites_are_excluded_from_the_denominator` below) silently made
/// [`threshold_passed`] more permissive than configured.
#[test]
fn tests_analyzed_excludes_suites() {
    let tests = vec![
        T::new("suite.test.ts", 1, "a describe block", "")
            .kind(DeclarationKind::Suite)
            .build(),
        T::new("suite.test.ts", 2, "case a", "expect(1).toBe(1)")
            .suite(&["a describe block"])
            .build(),
        T::new("suite.test.ts", 3, "case b", "expect(2).toBe(2)")
            .suite(&["a describe block"])
            .build(),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        result.tests_analyzed, 2,
        "a describe block is a container, not an executable test, and must not inflate \
         tests_analyzed"
    );
}

/// A corpus whose error-severity duplication rate crosses a configured threshold only once suites
/// are excluded from `tests_analyzed`'s denominator: one `describe` (a container contributing
/// nothing to either numerator or denominator), two tests that duplicate each other (both flagged,
/// error severity), and two unrelated unique tests.
///
/// True rate (4 real tests, 2 flagged): 2/4 = 50%. Rate a suite-inflated denominator would have
/// reported instead (5 "tests", 2 flagged): 2/5 = 40%. A threshold of 45 sits strictly between the
/// two: the correct denominator must fail this gate, while the bugged one would have wrongly
/// passed it -- exactly the "threshold is more permissive than the user asked for" defect this
/// guards against.
#[test]
fn truncation_cannot_make_a_configured_threshold_pass() {
    // The defect: findings are sorted and then truncated to a retention cap, and `threshold_passed`
    // used to recompute its numerator from whatever survived. A run with more error-severity
    // findings than the cap could therefore discard enough participants that the retained
    // percentage passed while the true rate exceeded the configured threshold -- a clean gate on a
    // corpus that failed it.
    //
    // Constructed directly rather than by producing MAX_FINDINGS real findings, which would make
    // the test enormous for no extra signal. The property under test is precisely that the gate
    // reads the recorded pre-truncation count instead of the retained list, so a result whose two
    // disagree is exactly the right input.
    let result = AnalysisResult {
        tests_analyzed: 100,
        // 40 of 100 locations were flagged before truncation: a 40% rate.
        error_finding_locations: 40,
        // ...but presentation retained nothing, so re-deriving the numerator would read 0%.
        findings: Vec::new(),
        had_error_finding: true,
        incomplete_causes: IncompleteCauses {
            pair_budget_exhausted: true,
            ..IncompleteCauses::default()
        },
        ..AnalysisResult::default()
    };

    assert!(
        !threshold_passed(&result, &config_with(&[], 10)),
        "a 40% pre-truncation rate must fail a 10% threshold regardless of how many findings \
         survived presentation truncation"
    );
    assert!(
        threshold_passed(&result, &config_with(&[], 40)),
        "and it must still pass a threshold that genuinely permits that rate"
    );
}

#[test]
fn threshold_crosses_only_once_suites_are_excluded_from_the_denominator() {
    let tests = vec![
        T::new("rate.test.ts", 1, "a describe block", "")
            .kind(DeclarationKind::Suite)
            .build(),
        T::new("rate.test.ts", 2, "duplicate a", "expect(1).toBe(1)").build(),
        T::new("rate.test.ts", 3, "duplicate b", "expect(1).toBe(1)").build(),
        T::new("rate.test.ts", 4, "unique c", "expect(2).toBe(2)").build(),
        T::new("rate.test.ts", 5, "unique d", "expect(3).toBe(3)").build(),
    ];
    let config = config_with(&[], 45);
    let result = analyze_with(tests, &config, 1);
    assert_eq!(
        result.tests_analyzed, 4,
        "the describe block must not be counted"
    );
    assert_eq!(
        rule_counts(&result.findings).0,
        1,
        "sanity: the two duplicate tests must actually produce one duplicate-test finding"
    );
    assert!(
        !threshold_passed(&result, &config),
        "true rate is 2/4 = 50%, above the 45% threshold; a suite-inflated denominator (2/5 = \
         40%) would have wrongly passed this same threshold"
    );
}

// ---------------------------------------------------------------------------------------------
// `near-duplicate-test`: reordered-independent-setup, the only bridged transformation so far
// ---------------------------------------------------------------------------------------------

/// fixtures/near/near-reordered-setup-positive.test.ts (pending move into fixtures/duplicates/,
/// see that file's header). `configure()`/`seed()` are independent, so swapping their order is a
/// pure reordering: same multiset of behavior events, different sequence, differing `alpha`.
#[test]
fn fixture_near_reordered_setup_positive() {
    let assertion = "assert:expect.toBe(subject=call:isReady(); value=true)";
    let tests = vec![
        T::new(
            "reordered.test.ts",
            29,
            "configures then seeds",
            "configure({ mode: 'test' }); seed({ users: 3 }); expect(isReady()).toBe(true)",
        )
        // Order affects `structural` exactly the way it affects `alpha` (both are serialized in
        // source order, only literals differ in how they're rendered), so this test sets distinct
        // `structural` per side -- matching what the real frontend would produce for a reordered
        // pair -- rather than relying on `T`'s shared empty default, which would otherwise let a
        // spurious `parameterization-candidate` collision (same default `structural`, differing
        // `alpha`) mask what this test is actually checking.
        .structural("call configure; call seed; assert")
        .behavior(&["call:configure", "call:seed", assertion])
        .build(),
        T::new(
            "reordered.test.ts",
            35,
            "seeds then configures",
            "seed({ users: 3 }); configure({ mode: 'test' }); expect(isReady()).toBe(true)",
        )
        .structural("call seed; call configure; assert")
        .behavior(&["call:seed", "call:configure", assertion])
        .build(),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        rule_count(&result.findings, Rule::NearDuplicateTest),
        1,
        "same behavior events, reordered, under equivalent context must be near-duplicate-test"
    );
    assert_eq!(
        rule_counts(&result.findings).0,
        0,
        "differing alpha (statement order differs) must never also fire duplicate-test"
    );
    assert_eq!(
        rule_count(&result.findings, Rule::ParameterizationCandidate),
        0,
        "differing structural (order differs) must never also fire parameterization-candidate"
    );
    assert_eq!(result.findings[0].severity, Severity::Warning);
    assert!(
        result.findings[0].message.contains("different ordering"),
        "a genuinely reordered trace must say what differed: {}",
        result.findings[0].message
    );
}

#[test]
fn parameterization_candidate_suppresses_the_same_near_duplicate_participants() {
    let tests = overlap_pair();
    let result = analyze_default(tests);
    assert_eq!(
        rule_count(&result.findings, Rule::ParameterizationCandidate),
        1,
        "the stronger consolidation finding must remain"
    );
    assert_eq!(
        rule_count(&result.findings, Rule::NearDuplicateTest),
        0,
        "the same participants must not also receive a weaker similarity prompt"
    );
}

#[test]
fn suppressed_near_duplicate_does_not_consume_pair_budget() {
    let tests = overlap_pair();
    let result = analyze_with_budget(
        tests,
        &default_config(),
        1,
        IncompleteCauses::default(),
        RunModes::default(),
        1,
    )
    .unwrap();
    assert_eq!(
        rule_count(&result.findings, Rule::ParameterizationCandidate),
        1
    );
    assert_eq!(rule_count(&result.findings, Rule::NearDuplicateTest), 0);
    assert!(
        !result.incomplete_causes.pair_budget_exhausted,
        "the one-unit parameterization finding exhausts the available budget exactly; its \
         suppressed near-duplicate restatement must cost nothing"
    );
}

/// One `TestCase` for the C4 suppression tests: a declaration at `line` whose normalized shape is
/// `structural` and whose behavior trace is `events`.
///
/// These three tests vary exactly two things -- which declarations share `structural`, and which
/// share a behavior trace -- so spelling the builder chain out per case made the varying part the
/// hardest thing to see, and the duplication ratchet charged for it.
fn c4_case(
    line: u32,
    title: &'static str,
    body: &'static str,
    structural: &'static str,
    events: &[&'static str],
) -> TestCase {
    T::new("c4.test.ts", line, title, body)
        .structural(structural)
        .behavior(events)
        .build()
}

/// C4, defect 1: the two rules picked different rows of one `.each` table.
///
/// Reproduced against the binary on a 24,147-test corpus before it was reproduced here -- one
/// 3-row `it.each` reported by both `parameterization-candidate` and `near-duplicate-test` with
/// identical `Location`s in both findings and no suppression.
///
/// The mechanism, which is what this fixture encodes: both rules collapse through
/// `dedup_by_declaration` first, so one row of the table survives in each. Which row depends on the
/// group, and the groups are keyed differently -- `structural` for one rule, the behavior multiset
/// for the other -- so a table whose rows do not all share a behavior trace puts a different row in
/// each group. The old key was the surviving row's `tests` INDEX, so it compared the arbitrary half
/// and missed.
///
/// `row_b` and `solo` are the near-duplicate pair; `row_a` and `solo` are the parameterization pair.
/// All three share `structural`, which is what makes the two findings the same claim.
#[test]
fn a_parameterization_finding_suppresses_a_near_duplicate_that_kept_a_different_row() {
    let shape = "expect(render(<string>)).toBe(<string>)";
    let tests = vec![
        // Two rows of one table: one declaration `Location`, one `structural`, differing `alpha`,
        // and differing behavior traces -- which is what lets the two groups diverge.
        c4_case(
            10,
            "renders a",
            "expect(render('a')).toBe('A')",
            shape,
            &["call:render", "assert:toBe"],
        ),
        c4_case(
            10,
            "renders b",
            "expect(render('b')).toBe('B')",
            shape,
            &["call:transform", "assert:toBe"],
        ),
        // A separate declaration sharing `structural` with both rows, and the behavior of row b.
        c4_case(
            20,
            "renders c",
            "expect(render('c')).toBe('C')",
            shape,
            &["call:transform", "assert:toBe"],
        ),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        rule_count(&result.findings, Rule::ParameterizationCandidate),
        1,
        "the stronger claim must still be made"
    );
    assert_eq!(
        rule_count(&result.findings, Rule::NearDuplicateTest),
        0,
        "the near-duplicate names the same declarations and the same normalized shape as the \
         parameterization finding; which row each rule kept is arbitrary and must not decide this"
    );
}

/// C4, defect 2: four of the five findings were strict SUBSETS of a larger parameterization group,
/// and suppression was exact-match only.
///
/// A strict subset needs the parameterization finding to name MORE declarations than the
/// near-duplicate one, which needs a CLUSTER rather than a spanning set: `group_outcome` reports a
/// group of up to `SPANNING_GROUP_MAX` as pairs, and only a larger one as a single finding over all
/// its members. Five declarations share `structural`, so parameterization reports one cluster over
/// all five; two of them also share a behavior trace, so `near-duplicate-test` reports that pair,
/// which names nothing the cluster did not.
#[test]
fn a_near_duplicate_that_is_a_subset_of_a_parameterization_cluster_is_suppressed() {
    let shape = "expect(total(<number>)).toBe(<number>)";
    let shared = ["call:total", "assert:toBe"];
    let mut tests = vec![
        // The two that also share a behavior trace: the near-duplicate pair.
        c4_case(1, "totals one", "expect(total(1)).toBe(1)", shape, &shared),
        c4_case(5, "totals two", "expect(total(2)).toBe(2)", shape, &shared),
    ];
    // Three more sharing `structural` only. Each gets its own behavior trace so none of them pairs
    // with another under `near-duplicate-test`, leaving exactly one near-duplicate group to
    // suppress. Spelled out rather than looped: `T::new` borrows its title and body, so a loop
    // building them with `format!` cannot outlive the iteration.
    for test in [
        c4_case(
            9,
            "totals three",
            "expect(total(3)).toBe(3)",
            shape,
            &["call:sum", "assert:toBe"],
        ),
        c4_case(
            13,
            "totals four",
            "expect(total(4)).toBe(4)",
            shape,
            &["call:tally", "assert:toBe"],
        ),
        c4_case(
            17,
            "totals five",
            "expect(total(5)).toBe(5)",
            shape,
            &["call:count", "assert:toBe"],
        ),
    ] {
        tests.push(test);
    }
    let result = analyze_default(tests);
    assert_eq!(
        rule_count(&result.findings, Rule::ParameterizationCandidate),
        1,
        "five declarations past SPANNING_GROUP_MAX are one cluster finding, not a set of pairs"
    );
    assert_eq!(
        rule_count(&result.findings, Rule::NearDuplicateTest),
        0,
        "a strict subset of an already-reported cluster restates it; only a group reaching a \
         declaration the parameterization finding did not is a separate claim"
    );
}

/// The other half of the subset rule, so suppression cannot quietly become "any overlap".
///
/// The near-duplicate pair shares ONE declaration with the parameterization group and reaches one
/// the group never named. That is a different pair, so it is a different claim, and it must survive.
#[test]
fn a_near_duplicate_reaching_a_declaration_outside_the_group_still_fires() {
    let tests = vec![
        c4_case(
            1,
            "totals one",
            "expect(total(1)).toBe(1)",
            "expect(total(<number>)).toBe(<number>)",
            &["call:total", "assert:toBe"],
        ),
        c4_case(
            5,
            "totals two",
            "expect(total(2)).toBe(2)",
            "expect(total(<number>)).toBe(<number>)",
            &["call:total", "assert:toBe"],
        ),
        // Shares the behavior of the pair above, so it joins their near-duplicate group, but its
        // `structural` differs, so it was never in the parameterization group.
        c4_case(
            9,
            "totals via helper",
            "expect(sum(3)).toBe(3)",
            "expect(helper(<number>)).toBe(<number>)",
            &["call:total", "assert:toBe"],
        ),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        rule_count(&result.findings, Rule::ParameterizationCandidate),
        1
    );
    assert_eq!(
        rule_count(&result.findings, Rule::NearDuplicateTest),
        1,
        "a group reaching a declaration the parameterization finding never named is a separate \
         claim and must not be suppressed"
    );
}

#[test]
fn overlap_suppression_distinguishes_tests_that_share_declaration_locations() {
    let tests = vec![
        T::new("rows.test.ts", 1, "parameter a", "parameter(1)")
            .structural("parameter(<number>)")
            .behavior(&["call:parameter-one", "assert:toBe"])
            .build(),
        T::new("rows.test.ts", 5, "parameter b", "parameter(2)")
            .structural("parameter(<number>)")
            .behavior(&["call:parameter-two", "assert:toBe"])
            .build(),
        T::new("rows.test.ts", 1, "near a", "prepare(); verify()")
            .structural("near shape a")
            .behavior(&["call:prepare", "call:verify"])
            .build(),
        T::new("rows.test.ts", 5, "near b", "verify(); prepare()")
            .structural("near shape b")
            .behavior(&["call:verify", "call:prepare"])
            .build(),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        rule_count(&result.findings, Rule::ParameterizationCandidate),
        1
    );
    assert_eq!(
        rule_count(&result.findings, Rule::NearDuplicateTest),
        1,
        "different expanded cases may share declaration locations; only the exact TestCase \
         participants reported by parameterization may suppress a near-duplicate finding"
    );
}

#[test]
fn disabling_parameterization_keeps_the_near_duplicate_finding() {
    let tests = overlap_pair();
    let config = config_with(&[(Rule::ParameterizationCandidate, Severity::Off)], 0);
    let result = analyze_with(tests, &config, 1);
    assert_eq!(
        rule_count(&result.findings, Rule::ParameterizationCandidate),
        0
    );
    assert_eq!(
        rule_count(&result.findings, Rule::NearDuplicateTest),
        1,
        "suppression must depend on an emitted stronger finding"
    );
}

#[test]
fn same_order_near_duplicate_message_does_not_claim_reordering() {
    let events = ["call:setup", "assert:toBe"];
    let tests = vec![
        T::new(
            "same-order.test.ts",
            1,
            "via local",
            "const x = get(); check(x)",
        )
        .structural("local then check")
        .behavior(&events)
        .build(),
        T::new("same-order.test.ts", 5, "inline", "check(get())")
            .structural("inline check")
            .behavior(&events)
            .build(),
    ];
    let result = analyze_default(tests);
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::NearDuplicateTest)
        .expect("same ordered behavior with differing bodies remains a near-duplicate prompt");
    assert!(
        finding.message.contains("same order"),
        "{}",
        finding.message
    );
    assert!(
        !finding.message.contains("different order"),
        "the message must not invent a reordering: {}",
        finding.message
    );
}

/// fixtures/duplicates/near-reordered-setup-dependent-twin.test.ts. The precondition-violating
/// twin: the two reordered statements share state (`total`), so swapping them changes the final
/// value the assertion observes. Fewer than [`super::MIN_BEHAVIOR_EVENTS_FOR_NEAR_DUPLICATE`]
/// events either way (plain assignments record no behavior event, only the assertion does), so
/// this is excluded by the substance floor before the multiset is even compared -- see the
/// dedicated `..._even_with_enough_events` test below for the case where the floor is cleared but
/// the multisets still differ.
#[test]
fn fixture_near_reordered_setup_dependent_twin_stays_distinct() {
    let tests = vec![
        T::new(
            "twin.test.ts",
            15,
            "resets before multiplying",
            "total = 1; total = total * 5; expect(total).toBe(5)",
        )
        // Distinct `structural` per side, matching how the real frontend renders these two
        // differently-ordered bodies -- see the comment on the positive fixture above for why
        // this is needed to keep an unrelated `parameterization-candidate` collision (same
        // default empty `structural`, differing `alpha`) from masking this test's own assertion.
        .structural("assign total 1; assign total mul; assert value=5")
        .behavior(&["assert:expect.toBe(subject=free:total; value=5)"])
        .build(),
        T::new(
            "twin.test.ts",
            21,
            "multiplies before resetting",
            "total = total * 5; total = 1; expect(total).toBe(1)",
        )
        .structural("assign total mul; assign total 1; assert value=1")
        .behavior(&["assert:expect.toBe(subject=free:total; value=1)"])
        .build(),
    ];
    let result = analyze_default(tests);
    assert!(
        result.findings.is_empty(),
        "a dependent reordering that changes the observed value must stay distinct"
    );
}

/// Same shape as the dependent twin above, but with enough behavior events to clear
/// [`super::MIN_BEHAVIOR_EVENTS_FOR_NEAR_DUPLICATE`], so this exercises the multiset-inequality
/// guard itself rather than the substance floor: one shared setup call plus a differing assertion
/// value must still refuse to match, proving the guard holds even once there is "enough" behavior.
#[test]
fn near_duplicate_test_requires_equal_multiset_not_just_enough_events() {
    let tests = vec![
        T::new(
            "twin-substantial.test.ts",
            1,
            "resets before multiplying",
            "seed(); total = 1; total = total * 5; expect(total).toBe(5)",
        )
        .structural("call seed; assign total 1; assign total mul; assert value=5")
        .behavior(&[
            "call:seed",
            "assert:expect.toBe(subject=free:total; value=5)",
        ])
        .build(),
        T::new(
            "twin-substantial.test.ts",
            5,
            "multiplies before resetting",
            "seed(); total = total * 5; total = 1; expect(total).toBe(1)",
        )
        .structural("call seed; assign total mul; assign total 1; assert value=1")
        .behavior(&[
            "call:seed",
            "assert:expect.toBe(subject=free:total; value=1)",
        ])
        .build(),
    ];
    let result = analyze_default(tests);
    assert!(
        result.findings.is_empty(),
        "differing assertion value means the behavior multisets differ, even with a shared call \
         event and enough events to clear the substance floor"
    );
}

/// fixtures/duplicates/near-boilerplate-gradient.test.ts, spot-checked at the 0 and 50 shared-call
/// levels (the two extremes). A naive "fraction of shared events" measure would score the 50-call
/// pair near 1.0; multiset equality must refuse both, at every level, because `submitPayment` /
/// `cancelPayment` and their differing assertion values are never removed from the multiset no
/// matter how much identical `seedOrder` boilerplate surrounds them.
#[test]
fn fixture_near_boilerplate_gradient_stays_distinct_at_every_level() {
    let submit = |events: &[&str]| {
        let mut events: Vec<String> = events.iter().map(|e| (*e).to_owned()).collect();
        events.push("call:submitPayment".to_owned());
        events.push("assert:expect.toBe(subject=free:order.status; value='submitted')".to_owned());
        events
    };
    let cancel = |events: &[&str]| {
        let mut events: Vec<String> = events.iter().map(|e| (*e).to_owned()).collect();
        events.push("call:cancelPayment".to_owned());
        events.push("assert:expect.toBe(subject=free:order.status; value='cancelled')".to_owned());
        events
    };
    let seed_calls: Vec<&str> = std::iter::repeat_n("call:seedOrder", 50).collect();

    for &n in &[0usize, 5, 20, 50] {
        let shared = &seed_calls[..n];
        let submit_events = submit(shared);
        let cancel_events = cancel(shared);
        let tests = vec![
            T::new(
                "gradient.test.ts",
                1,
                Box::leak(format!("submits order with {n} shared setup call(s)").into_boxed_str()),
                Box::leak(format!("submitPayment(order); n={n}").into_boxed_str()),
            )
            // Distinct `structural` per side (and per `n`), matching reality: `submitPayment`
            // and `cancelPayment` are different callees, so their serialized shape differs too --
            // see the comment on the reordered-setup positive test above for why this guards
            // against an unrelated `parameterization-candidate` collision.
            .structural(Box::leak(
                format!("call submitPayment n={n}").into_boxed_str(),
            ))
            .behavior(&submit_events.iter().map(String::as_str).collect::<Vec<_>>())
            .build(),
            T::new(
                "gradient.test.ts",
                2,
                Box::leak(format!("cancels order with {n} shared setup call(s)").into_boxed_str()),
                Box::leak(format!("cancelPayment(order); n={n}").into_boxed_str()),
            )
            .structural(Box::leak(
                format!("call cancelPayment n={n}").into_boxed_str(),
            ))
            .behavior(&cancel_events.iter().map(String::as_str).collect::<Vec<_>>())
            .build(),
        ];
        let result = analyze_default(tests);
        assert!(
            result.findings.is_empty(),
            "boilerplate level {n}: submitting and cancelling a payment must stay distinct \
             regardless of how much identical setup surrounds them"
        );
    }
}

/// `ContextKey` and `Context::is_equivalent` must agree on every pair, not merely by comment.
///
/// `ContextKey`'s own doc says it "captures exactly what `is_equivalent` checks" -- and until this
/// test, nothing enforced that. The two exist for different mechanisms and neither can delegate to
/// the other: grouping needs a hashable KEY (it is a `BTreeMap` key, and its canonical JSON is
/// published as `TestFingerprint::context_key`), while `is_equivalent` is a pairwise PREDICATE.
/// That is why the duplication is legitimate; what was not legitimate is the two being kept in
/// step by prose.
///
/// So `is_equivalent` earns its keep as the executable specification the key is checked against,
/// rather than being dead code with no production caller. A field added to one and forgotten in
/// the other fails here -- but only if the matrix below VARIES that field. It did not for
/// `module_setup` or `config_scope`: both were added to the key, left out of the predicate, and
/// this test passed anyway, because every row held them at their defaults. Review caught what the
/// test was written to catch and could not.
///
/// So the matrix is the contract, not the assertion loop. A new field on [`Context`] needs a row
/// here; without one, this test keeps passing and keeps proving nothing about it.
#[test]
fn the_grouping_key_agrees_with_context_equivalence_on_every_pair() {
    let base = || Context {
        hooks: vec![hook(HookKind::BeforeEach, "seed()", 1)],
        mocked_modules: vec![mocked("./db", Some("factory"))],
        fixtures: vec![fixture("user", Some("value"))],
        imports: vec![Arc::from("./db")],
        module_setup: Vec::new(),
        config_scope: None,
        unresolved: false,
    };

    // Each variant differs from `base` in exactly one thing the key claims to capture, plus two
    // that it claims NOT to capture -- so agreement is asserted in both directions.
    let mut differing_hook_body = base();
    differing_hook_body.hooks = vec![hook(HookKind::BeforeEach, "reset()", 1)];
    let mut differing_hook_kind = base();
    differing_hook_kind.hooks = vec![hook(HookKind::AfterEach, "seed()", 1)];
    let mut differing_mock = base();
    differing_mock.mocked_modules = vec![mocked("./db", Some("other"))];
    let mut differing_fixture = base();
    differing_fixture.fixtures = vec![fixture("user", Some("other"))];
    let mut differing_import = base();
    differing_import.imports = vec![Arc::from("./other")];
    // Same hook body at a different source line: equivalent, and the key must agree, because
    // `Hook::location` is exactly what derived equality would wrongly compare.
    let mut moved_hook = base();
    moved_hook.hooks = vec![hook(HookKind::BeforeEach, "seed()", 99)];
    // Added after review observed that the key and the predicate had diverged: both of these
    // reached `ContextKey` while `is_equivalent` ignored them, and this matrix -- the test written
    // to catch exactly that -- could not see it, because every row left them at their defaults.
    let mut differing_scope = base();
    differing_scope.config_scope = Some("{ environment: 'node' }".to_owned());
    let mut differing_setup = base();
    differing_setup.module_setup = vec!["seed()".to_owned()];

    let contexts = [
        ("base", base()),
        ("hook body", differing_hook_body),
        ("hook kind", differing_hook_kind),
        ("mock factory", differing_mock),
        ("fixture value", differing_fixture),
        ("import", differing_import),
        ("hook moved", moved_hook),
        ("config scope", differing_scope),
        ("module setup", differing_setup),
    ];

    for (left_name, left) in &contexts {
        for (right_name, right) in &contexts {
            let equivalent = left.is_equivalent(right);
            let keyed = ContextKey::new(left) == ContextKey::new(right);
            assert_eq!(
                equivalent, keyed,
                "{left_name} vs {right_name}: `is_equivalent` says {equivalent} but the grouping \
                 key says {keyed}. One of them has a field the other does not."
            );
        }
    }

    // The pair the whole design exists for: same work, different source position.
    assert!(
        contexts[0].1.is_equivalent(&contexts[6].1),
        "a hook that moved is still the same ambient fact"
    );
}

/// A body whose callback form changes its meaning is not near-duplicate to the other form.
///
/// An arrow does not bind `this`/`arguments` and a `function` does, so `this.value = 5` writes to
/// a different object in each. The two perform the *same events* -- one assignment, one assertion
/// -- so `behavior` is equal and always was; what differs is which object those events act on.
/// `alpha` records it, which is why `duplicate-test` was never exposed and only the
/// behaviour-keyed rules were.
///
/// The blindness is older than the commit that revealed it: before assignments emitted an event,
/// this pair sat on ONE event and `MIN_BEHAVIOR_EVENTS_FOR_NEAR_DUPLICATE` filtered it out. The
/// substance floor was hiding it rather than the rule telling the forms apart -- a pass that ends
/// the moment anything raises the event count, which is exactly what happened.
#[test]
fn a_this_dependent_body_is_not_near_duplicate_to_the_other_callback_form() {
    let events = [
        "assign:= target=(member this .value) value=<number:5>",
        "assert:toBe",
    ];
    // Distinct `alpha` AND `structural`, because that is what the real pair has -- `ser_callable`
    // renders the form into both once the body reads `this`. Sharing either makes this test
    // vacuous for a different reason: equal `alpha` lets `duplicate-test` claim the pair so
    // `near-duplicate-test` never sees it, and equal `structural` lets
    // `parameterization-candidate` claim it so the restatement suppression removes the finding.
    // Both were hit while writing this, and neither showed up as a failure -- only the revert
    // check did.
    //
    // On `alpha`: `ser_callable` renders the two
    // forms as `arrow-fn` and `function-fn` once the body reads `this`. Giving them the same
    // `alpha` would make this test vacuous -- `duplicate-test` would claim the pair and
    // `representative_per_alpha` would keep `near-duplicate-test` from ever seeing it, so the
    // silence would prove nothing about the callback form.
    let tests = vec![
        T::new(
            "forms.test.ts",
            1,
            "arrow form",
            "(arrow-fn () this.value = 5)",
        )
        .structural("(arrow-fn () this.value = <number>)")
        .behavior(&events)
        .callback_form(CallbackForm::Arrow)
        .build(),
        T::new(
            "forms.test.ts",
            9,
            "function form",
            "(function-fn () this.value = 5)",
        )
        .structural("(function-fn () this.value = <number>)")
        .behavior(&events)
        .callback_form(CallbackForm::Function)
        .build(),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        rule_count(&result.findings, Rule::NearDuplicateTest),
        0,
        "the two forms are not interchangeable here: {:#?}",
        result.findings
    );

    // The control. With the SAME form, a pair carrying these events IS reported -- so the
    // silence above comes from the callback form and not from the events failing to match.
    // Distinct `structural` on each, so `parameterization-candidate` cannot claim the pair and
    // suppress the near-duplicate finding as a restatement of itself.
    let same_form = vec![
        T::new(
            "forms.test.ts",
            1,
            "one",
            "this.value = 5; expect(this.value).toBe(5)",
        )
        .structural("S1")
        .behavior(&events)
        .callback_form(CallbackForm::Arrow)
        .build(),
        T::new("forms.test.ts", 9, "two", "this.other(); this.value = 5")
            .structural("S2")
            .behavior(&[events[1], events[0]])
            .callback_form(CallbackForm::Arrow)
            .build(),
    ];
    let result = analyze_default(same_form);
    assert!(
        rule_count(&result.findings, Rule::NearDuplicateTest) > 0,
        "same form, same events: this pair must still be reported: {:#?}",
        result.findings
    );
}

/// A `near-duplicate-test` finding naming exactly a `parameterization-candidate` finding's
/// participants is restating it, and only the actionable one survives.
///
/// Both rules can reach one pair from different directions. Both now propose the same remedy --
/// one `test.each` table keeping every case -- so the second finding adds nothing, and
/// `parameterization-candidate` is the one that survives because it states that remedy from a
/// proof (identical structure, differing literals) rather than from a suspicion that adjudicated
/// at 3 of 30 correct. Measured on a real 24,147-test corpus: 304 of 1,141 `near-duplicate-test`
/// findings were exact restatements -- 12% of the entire report.
#[test]
fn near_duplicate_restating_a_parameterization_finding_is_suppressed() {
    // Same structure, differing only in a literal -> `parameterization-candidate`. Same behaviour
    // events in a different order -> `near-duplicate-test`. One pair, both rules.
    let tests = vec![
        T::new(
            "restate.test.ts",
            1,
            "case a",
            "check(1); verify(); expect(x).toBe(1)",
        )
        .structural("S")
        .behavior(&["call:check", "call:verify", "assert:toBe"])
        .build(),
        T::new(
            "restate.test.ts",
            9,
            "case b",
            "verify(); check(2); expect(x).toBe(2)",
        )
        .structural("S")
        .behavior(&["call:verify", "call:check", "assert:toBe"])
        .build(),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        rule_count(&result.findings, Rule::ParameterizationCandidate),
        1,
        "the actionable finding survives: {:#?}",
        result.findings
    );
    assert_eq!(
        rule_count(&result.findings, Rule::NearDuplicateTest),
        0,
        "and the restatement of it does not: {:#?}",
        result.findings
    );
}

/// Suppression is by *exact* participants, never by overlap: a near-duplicate finding about a
/// different pair is a separate claim and must survive, even when one of its tests also appears
/// in a parameterization finding.
#[test]
fn near_duplicate_over_a_different_pair_survives_the_suppression() {
    let tests = vec![
        T::new(
            "distinct.test.ts",
            1,
            "case a",
            "check(1); verify(); expect(x).toBe(1)",
        )
        .structural("S")
        .behavior(&["call:check", "call:verify", "assert:toBe"])
        .build(),
        T::new(
            "distinct.test.ts",
            9,
            "case b",
            "verify(); check(2); expect(x).toBe(2)",
        )
        .structural("S")
        .behavior(&["call:verify", "call:check", "assert:toBe"])
        .build(),
        // Shares the behaviour multiset, so it pairs with both above -- but its own structure
        // differs, so no parameterization finding names it.
        T::new(
            "distinct.test.ts",
            17,
            "case c",
            "verify(); check(3); expect(y).toEqual(3)",
        )
        .structural("OTHER")
        .behavior(&["assert:toBe", "call:check", "call:verify"])
        .build(),
    ];
    let result = analyze_default(tests);
    assert!(
        rule_count(&result.findings, Rule::NearDuplicateTest) > 0,
        "a near-duplicate pair no parameterization finding names must survive: {:#?}",
        result.findings
    );
}

/// Equal `alpha` (bodies are byte-for-byte the same, so trivially bag-equal too) is
/// `duplicate-test`'s claim, never `near-duplicate-test`'s -- mirrors
/// `equal_alpha_is_duplicate_test_not_parameterization_candidate` for this rule.
#[test]
fn near_duplicate_test_excludes_equal_alpha_members() {
    let body = "configure(); seed(); expect(isReady()).toBe(true)";
    let events = ["call:configure", "call:seed", "assert:toBe"];
    let tests = vec![
        T::new("equal-alpha.test.ts", 1, "case a", body)
            .behavior(&events)
            .build(),
        T::new("equal-alpha.test.ts", 5, "case b", body)
            .behavior(&events)
            .build(),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        rule_counts(&result.findings).0,
        1,
        "equal alpha is duplicate-test's claim"
    );
    assert_eq!(
        rule_count(&result.findings, Rule::NearDuplicateTest),
        0,
        "equal alpha must never also be reported as near-duplicate-test"
    );
}

/// Differing `Context` (different imports) blocks `near-duplicate-test` the same way it blocks
/// `duplicate-test` and `parameterization-candidate`: an ambient-fact mismatch means the bodies
/// are not comparable, regardless of how their behavior multisets relate.
#[test]
fn near_duplicate_test_requires_equivalent_context() {
    let events = ["call:configure", "call:seed", "assert:toBe"];
    let tests = vec![
        T::new(
            "diff-context.test.ts",
            1,
            "case a",
            "seed(); configure(); expect(x).toBe(true)",
        )
        .behavior(&events)
        .imports(&["./a"])
        .build(),
        T::new(
            "diff-context.test.ts",
            5,
            "case b",
            "configure(); seed(); expect(x).toBe(true)",
        )
        .behavior(&events)
        .imports(&["./b"])
        .build(),
    ];
    let result = analyze_default(tests);
    assert!(
        result.findings.is_empty(),
        "differing context must block near-duplicate-test, same as the other rules"
    );
}

/// The [`super::MIN_BEHAVIOR_EVENTS_FOR_NEAR_DUPLICATE`] floor: a single-event body clearing bag
/// equality is excluded outright, since "reordered setup" is meaningless with nothing to reorder.
#[test]
fn near_duplicate_test_requires_the_minimum_behavior_event_floor() {
    let tests = vec![
        T::new("one-event.test.ts", 1, "case a", "expect(x).toBe(true)")
            .structural("assert x")
            .behavior(&["assert:toBe"])
            .build(),
        T::new("one-event.test.ts", 5, "case b", "expect(y).toBe(true)")
            .structural("assert y")
            .behavior(&["assert:toBe"])
            .build(),
    ];
    let result = analyze_default(tests);
    assert_eq!(
        rule_count(&result.findings, Rule::NearDuplicateTest),
        0,
        "a single-event body has nothing to reorder and must not seed a near-duplicate-test group"
    );
}

/// Turning the rule `Off` does no work and reports nothing, mirroring the same check for
/// `parameterization-candidate`.
#[test]
fn near_duplicate_test_rule_set_to_off_produces_no_findings() {
    let events = ["call:configure", "call:seed", "assert:toBe"];
    let tests = vec![
        T::new(
            "off.test.ts",
            1,
            "case a",
            "seed(); configure(); expect(x).toBe(true)",
        )
        .structural("call seed; call configure; assert")
        .behavior(&events)
        .build(),
        T::new(
            "off.test.ts",
            5,
            "case b",
            "configure(); seed(); expect(x).toBe(true)",
        )
        .structural("call configure; call seed; assert")
        .behavior(&events)
        .build(),
    ];
    let result = analyze(
        tests,
        &config_with(&[(Rule::NearDuplicateTest, Severity::Off)], 0),
        1,
        IncompleteCauses::default(),
    )
    .unwrap();
    assert_eq!(
        rule_count(&result.findings, Rule::NearDuplicateTest),
        0,
        "turning the rule off must produce none of its own findings"
    );
}

/// fixtures/duplicates/near-non-transitive-cluster.test.ts. Similarity in general is not
/// transitive, so a rule that clusters by "shares an edge with a similar member" risks
/// asserting an unverified A/C equivalence. This rule cannot: it groups by *exact* equality of
/// [`super::NearDuplicateKey`] (see that struct's doc and the module-level rule doc), so a shared
/// group already means every pairwise key is identical, not merely chained through a pivot. A
/// (`createOrder`, reads `status` before any mutation), B (`loadOrder`, extracted-local read) and
/// C (`cancelOrder` then `loadOrder`, opposite assertion polarity) are "near" each other only in
/// the loose narrative sense of "A resembles B, B resembles C" -- every pair differs by at least one
/// callee, and B/C additionally differ in assertion polarity -- so no pair shares a behavior
/// multiset and the rule links none of them. The non-transitive triple this fixture stresses
/// cannot arise under exact-key grouping.
#[test]
fn fixture_near_non_transitive_cluster_links_no_pair() {
    let tests = vec![
        T::new(
            "non-transitive.test.ts",
            55,
            "a new order is pending",
            "const order = createOrder(); expect(order.status).toBe('pending')",
        )
        .structural("call createOrder; assert value='pending'")
        .behavior(&[
            "call:createOrder",
            "assert:expect.toBe(subject=free:order.status; value='pending')",
        ])
        .build(),
        T::new(
            "non-transitive.test.ts",
            60,
            "a loaded order is pending",
            "const order = loadOrder(1); const status = order.status; \
             expect(status).toBe('pending')",
        )
        .structural("call loadOrder; assert value='pending'")
        .behavior(&[
            "call:loadOrder",
            "assert:expect.toBe(subject=free:status; value='pending')",
        ])
        .build(),
        T::new(
            "non-transitive.test.ts",
            66,
            "a cancelled order is not pending",
            "cancelOrder(1); const order = loadOrder(1); const status = order.status; \
             expect(status).not.toBe('pending')",
        )
        .structural("call cancelOrder; call loadOrder; assert not value='pending'")
        .behavior(&[
            "call:cancelOrder",
            "call:loadOrder",
            "assert:expect.not.toBe(subject=free:status; value='pending')",
        ])
        .build(),
    ];
    let result = analyze_default(tests);
    assert!(
        result.findings.is_empty(),
        "A, B, and C differ in callee (and B/C additionally in assertion polarity), so every \
         pairwise behavior multiset is unequal -- none of the three pairs may be linked, let \
         alone clustered together"
    );
}

// ---------------------------------------------------------------------------------------------
// `cross-file-duplicate`
// ---------------------------------------------------------------------------------------------

/// `Warning` by default since the 2026-09-17 adjudication (9/9 `redundant` over 15,980 tests):
/// the same alpha-equal pair split across two files is reported by a run that passes no `--rule`
/// at all, at exactly `Warning` -- `duplicate-test` is file-scoped and contributes nothing here,
/// so the single finding is this rule's alone. The severity is asserted, not merely the count: a
/// flip to `Error` would change what the finding claims about a copy that may be deliberate.
#[test]
fn cross_file_duplicate_is_warning_by_default() {
    let result = analyze_with(cross_file_pair(), &default_config(), 2);
    assert_eq!(
        rule_count(&result.findings, Rule::CrossFileDuplicate),
        1,
        "cross-file-duplicate ships Warning, so a default run must report this pair: {:?}",
        result.findings
    );
    assert_eq!(
        result.findings.len(),
        1,
        "duplicate-test is file-scoped, so nothing but cross-file-duplicate may fire here: {:?}",
        result.findings
    );
    assert_eq!(result.findings[0].severity, Severity::Warning);
}

/// The same pair under an explicit `--rule cross-file-duplicate=warning`: the shape the rule
/// reports is the same-alpha, equivalent-context pair across two files that `duplicate-test`'s
/// file scoping alone would miss. Kept separate from the default-severity test above so that a
/// default flip in either direction leaves the rule's *behavior* pinned by a case that does not
/// depend on the default at all.
#[test]
fn cross_file_duplicate_reports_identical_bodies_across_files_when_enabled() {
    let result = analyze_with(cross_file_pair(), &cross_file_config(), 2);
    assert_eq!(
        rule_count(&result.findings, Rule::CrossFileDuplicate),
        1,
        "identical bodies under equivalent context across two files must be reported once enabled"
    );
    assert_eq!(result.findings[0].locations.len(), 2);
    assert_ne!(
        result.findings[0].locations[0].path, result.findings[0].locations[1].path,
        "a cross-file-duplicate finding must always span two different files"
    );
}

/// A pair must never be reported under both `duplicate-test` and `cross-file-duplicate`. Three
/// identical-body tests: two share a file (`duplicate-test`'s pair) and a third lives in a second
/// file (only reachable via `cross-file-duplicate`). With both rules enabled, the two rules'
/// finding location-sets must not overlap -- `cross-file-duplicate` collapses to one
/// representative per file specifically so it can never re-report the same-file pair.
#[test]
fn cross_file_duplicate_never_reports_the_same_pair_as_duplicate_test() {
    let body = "expect(add(1, 1)).toBe(2)";
    let tests = vec![
        T::new("a.test.ts", 1, "case a1", body).build(),
        T::new("a.test.ts", 5, "case a2", body).build(),
        T::new("b.test.ts", 1, "case b1", body).build(),
    ];
    let config = config_with(
        &[
            (Rule::DuplicateTest, Severity::Error),
            (Rule::CrossFileDuplicate, Severity::Warning),
        ],
        0,
    );
    let result = analyze_with(tests, &config, 3);
    assert_eq!(
        rule_counts(&result.findings).0,
        1,
        "the same-file pair (a1, a2)"
    );
    assert_eq!(
        rule_count(&result.findings, Rule::CrossFileDuplicate),
        1,
        "one cross-file edge linking file a's representative to file b"
    );

    let duplicate_test_locations: std::collections::BTreeSet<_> = result
        .findings
        .iter()
        .find(|f| f.rule == Rule::DuplicateTest)
        .unwrap()
        .locations
        .iter()
        .cloned()
        .collect();
    let cross_file_locations: std::collections::BTreeSet<_> = result
        .findings
        .iter()
        .find(|f| f.rule == Rule::CrossFileDuplicate)
        .unwrap()
        .locations
        .iter()
        .cloned()
        .collect();
    assert!(
        duplicate_test_locations
            .intersection(&cross_file_locations)
            .count()
            < 2,
        "the two rules must never name the exact same pair of locations"
    );
}

/// Copilot review #4, defect 2: a `skipIf`/`runIf` condition resolving to a local binding used to
/// fingerprint as `<local:start-end:name>`, unique only within one file. Two files that each
/// declare a same-named local at the same byte offsets (identical boilerplate up to that point)
/// but bind it to different values produced the *same* modifier fingerprint, and `CrossFileKey`
/// deliberately drops the path -- so `cross-file-duplicate` reported a false match between two
/// tests gated on opposite conditions. `src/frontend/registry.rs`'s `condition_fingerprint` now
/// folds the file into a local condition's identity (mirroring `fingerprint.rs`'s own
/// `free:module`/`free:outer` treatment), so the two `SkipIf` strings below stand in for what that
/// fix produces for two files' same-span, different-valued local bindings and must not key-match.
#[test]
fn cross_file_duplicate_does_not_match_same_span_local_conditions_from_different_files() {
    let body = "expect(add(1, 1)).toBe(2)";
    let tests = vec![
        T::new("a.test.ts", 1, "case a", body)
            .modifiers(vec![Modifier::SkipIf(
                "<local:a.test.ts:10-17:enabled>".to_owned(),
            )])
            .build(),
        T::new("b.test.ts", 1, "case b", body)
            .modifiers(vec![Modifier::SkipIf(
                "<local:b.test.ts:10-17:enabled>".to_owned(),
            )])
            .build(),
    ];
    let config = cross_file_config();
    let result = analyze_with(tests, &config, 2);
    assert!(
        result.findings.is_empty(),
        "same-span local conditions from two different files must not be treated as the same \
         modifier"
    );
}

/// Positive control for the fix above: two files whose `SkipIf` condition is genuinely the same
/// identity (a true global, or the same resolved import/literal) must still be linked once
/// `cross-file-duplicate` is enabled -- the fix must not overreach into breaking a legitimate
/// cross-file match.
#[test]
fn cross_file_duplicate_still_matches_genuinely_identical_conditions_across_files() {
    let body = "expect(add(1, 1)).toBe(2)";
    let tests = vec![
        T::new("a.test.ts", 1, "case a", body)
            .modifiers(vec![Modifier::SkipIf("<global:isCI>".to_owned())])
            .build(),
        T::new("b.test.ts", 1, "case b", body)
            .modifiers(vec![Modifier::SkipIf("<global:isCI>".to_owned())])
            .build(),
    ];
    let config = cross_file_config();
    let result = analyze_with(tests, &config, 2);
    assert_eq!(
        rule_count(&result.findings, Rule::CrossFileDuplicate),
        1,
        "a genuinely identical (global) condition across two files must still be linked"
    );
}

/// Differing `Context` blocks `cross-file-duplicate` the same way it blocks every other rule here.
#[test]
fn cross_file_duplicate_requires_equivalent_context() {
    let body = "expect(add(1, 1)).toBe(2)";
    let tests = vec![
        T::new("a.test.ts", 1, "case a", body)
            .imports(&["./a"])
            .build(),
        T::new("b.test.ts", 1, "case b", body)
            .imports(&["./b"])
            .build(),
    ];
    let config = cross_file_config();
    let result = analyze_with(tests, &config, 2);
    assert!(
        result.findings.is_empty(),
        "differing context must block cross-file-duplicate too"
    );
}

// ---------------------------------------------------------------------------------------------
// Baseline and changed-file modes
//
// Exercised through `analyze_with_modes` rather than only through `crate::modes` because the
// property that matters is not "the marker was set" -- it is that setting it moves the quality
// gate, which is decided in this module, before truncation. `modes.rs` owns matching; this owns
// what matching does to the exit code.
// ---------------------------------------------------------------------------------------------

/// Builds a loaded [`Baseline`] by round-tripping `findings` through a real file, so these tests
/// exercise the same write/parse/validate path the CLI does rather than a hand-built map.
fn baseline_from(findings: &[Finding]) -> (tempfile::TempDir, crate::modes::Baseline) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("baseline.json");
    crate::modes::write_baseline(&path, findings).unwrap();
    let baseline = crate::modes::load_baseline(&path).unwrap();
    (directory, baseline)
}

/// Build two same-file declaration fixtures with identical bodies and different titles.
fn duplicate_pair() -> Vec<TestCase> {
    let body = "expect(add(1, 1)).toBe(2)";
    vec![
        T::new("math.test.ts", 1, "adds", body).build(),
        T::new("math.test.ts", 9, "adds again", body).build(),
    ]
}

#[test]
/// A baselined Error-severity finding stays in the result but is excluded from the gate:
/// acceptance changes what counts, never what is reported.
fn a_baselined_error_finding_is_still_reported_but_no_longer_fails_the_gate() {
    let config = default_config();
    let unbaselined = analyze(duplicate_pair(), &config, 1, IncompleteCauses::default()).unwrap();
    assert!(
        unbaselined.had_error_finding,
        "sanity: this pair must fail the gate before a baseline exists"
    );
    assert_eq!(
        exit_code(&unbaselined, &config),
        ExitCode::QualityGateFailed
    );

    let (_directory, baseline) = baseline_from(&unbaselined.findings);
    let baselined = analyze_with_modes(
        duplicate_pair(),
        &config,
        1,
        IncompleteCauses::default(),
        RunModes {
            changed_files: None,
            baseline: Some(&baseline),
            suppressions: &[],
        },
    )
    .unwrap();

    assert_eq!(
        baselined.findings.len(),
        unbaselined.findings.len(),
        "a baseline must not remove findings from the report, only from the gate"
    );
    assert!(baselined.findings.iter().all(|finding| finding.baselined));
    assert!(!baselined.had_error_finding);
    assert_eq!(baselined.error_finding_locations, 0);
    assert_eq!(exit_code(&baselined, &config), ExitCode::Success);
    assert_eq!(baselined.modes.baselined, unbaselined.findings.len());
    assert_eq!(baselined.modes.new_findings, 0);
    assert!(baselined.modes.baseline_applied);
}

#[test]
/// A baseline that does not list a finding leaves it fully live: allowance is consumed per
/// occurrence, so a fingerprint accepted once does not cover a second occurrence.
fn a_finding_absent_from_the_baseline_still_fails_the_gate() {
    let config = default_config();
    // A baseline recorded from a *different* file: nothing in this run matches it.
    let elsewhere = analyze(
        vec![
            T::new("other.test.ts", 1, "x", "expect(other()).toBe(1)").build(),
            T::new("other.test.ts", 9, "y", "expect(other()).toBe(1)").build(),
        ],
        &config,
        1,
        IncompleteCauses::default(),
    )
    .unwrap();
    let (_directory, baseline) = baseline_from(&elsewhere.findings);

    let result = analyze_with_modes(
        duplicate_pair(),
        &config,
        1,
        IncompleteCauses::default(),
        RunModes {
            changed_files: None,
            baseline: Some(&baseline),
            suppressions: &[],
        },
    )
    .unwrap();
    assert!(result.findings.iter().all(|finding| !finding.baselined));
    assert!(result.had_error_finding);
    assert_eq!(exit_code(&result, &config), ExitCode::QualityGateFailed);
    assert_eq!(result.modes.baselined, 0);
    assert_eq!(result.modes.new_findings, result.findings.len());
}

#[test]
fn changed_scope_that_withholds_a_finding_marks_the_run_incomplete() {
    let config = default_config();
    let tests = vec![
        T::new("touched.test.ts", 1, "a", "expect(one()).toBe(1)").build(),
        T::new("touched.test.ts", 9, "b", "expect(one()).toBe(1)").build(),
        T::new("untouched.test.ts", 1, "c", "expect(two()).toBe(2)").build(),
        T::new("untouched.test.ts", 9, "d", "expect(two()).toBe(2)").build(),
    ];
    let changed = BTreeSet::from([PathBuf::from("touched.test.ts")]);
    let result = analyze_changed(tests, &config, 2, &changed);

    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.modes.withheld_unchanged, 1);
    assert!(
        result.incomplete(),
        "withholding a real finding narrows what this report can claim, and must say so"
    );
    assert!(result.modes.changed_scope);
}

#[test]
fn changed_scope_that_withholds_nothing_stays_complete() {
    // The other half: a changed-files run whose report is identical to a full run's must not be
    // marked incomplete on the strength of the flag alone. Crying wolf on every clean pull request
    // is how a signal stops being read.
    let config = default_config();
    let changed = BTreeSet::from([PathBuf::from("math.test.ts")]);
    let result = analyze_changed(duplicate_pair(), &config, 1, &changed);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.modes.withheld_unchanged, 0);
    assert!(!result.incomplete());
}

#[test]
/// A finding with any participant in a changed file survives changed-file scoping -- keying
/// on the first location alone would drop half the cross-file duplicates.
fn a_cross_file_duplicate_with_an_unchanged_twin_survives_changed_scoping() {
    // The trap this design exists to avoid: `cross-file-duplicate` pairs a changed file with an
    // unchanged one. Because analysis covers the whole corpus and only findings are filtered, the
    // pair is still found *and* still reported -- it has a participant in a changed file. Had the
    // corpus been restricted to changed files instead, the twin would never have been extracted
    // and this run would have reported the changed file as clean.
    let config = config_with(&[(Rule::CrossFileDuplicate, Severity::Error)], 0);
    let body = "expect(shared()).toBe(1)";
    let tests = vec![
        T::new("changed.test.ts", 1, "here", body).build(),
        T::new("unchanged.test.ts", 1, "there", body).build(),
    ];
    let changed = BTreeSet::from([PathBuf::from("changed.test.ts")]);
    let result = analyze_with_modes(
        tests,
        &config,
        2,
        IncompleteCauses::default(),
        RunModes {
            changed_files: Some(&changed),
            baseline: None,
            suppressions: &[],
        },
    )
    .unwrap();

    assert_eq!(
        rule_count(&result.findings, Rule::CrossFileDuplicate),
        1,
        "a duplicate whose twin lives in an unchanged file must still be reported"
    );
    assert_eq!(result.modes.withheld_unchanged, 0);
}

// ---------------------------------------------------------------------------------------------
// subsumed-test, duplicate-suite, duplicate-each-row, redundant-hook
//
// The fixture corpus pins each rule's real-binary behavior (participants, severity, suppression);
// these unit tests pin the boundaries the corpus cannot reach with hand-written IR: span-based
// suite containment, the empty/subsequence guards, and the rule that two unresolvable tables are
// never rows of one declaration.
// ---------------------------------------------------------------------------------------------

/// `subsumed-test` is Off by default (measured evidence, see `model.rs`), so tests that pin its
/// behavior enable it explicitly -- the same contract the corpus gate honors with
/// `--rule subsumed-test=warning`. A zero-assertion against an off rule would be vacuous.
fn subsumed_enabled_config() -> Config {
    config_with(&[(Rule::SubsumedTest, Severity::Warning)], 0)
}

#[test]
fn subsumed_test_reports_a_strict_behavior_subset_once_per_direction() {
    let config = subsumed_enabled_config();
    let small = T::new("s.test.ts", 1, "submits", "alpha-one")
        .behavior(&["call:order.submit", "assert:status"])
        .build();
    let large = T::new("s.test.ts", 9, "submits and confirms", "alpha-two")
        .behavior(&[
            "call:order.submit",
            "assert:status",
            "call:order.confirm",
            "assert:confirmed",
        ])
        .build();
    let result = analyze(vec![small, large], &config, 1, IncompleteCauses::default()).unwrap();
    assert_eq!(rule_count(&result.findings, Rule::SubsumedTest), 1);
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::SubsumedTest)
        .unwrap();
    assert_eq!(finding.severity, Severity::Warning);
    // The smaller test is the subsumed participant, listed first.
    assert_eq!(finding.titles[0], "submits");
    assert_eq!(finding.titles[1], "submits and confirms");
}

#[test]
fn subsumed_test_skips_pairs_the_other_rules_own() {
    let config = subsumed_enabled_config();
    // Equal behavior lists (identical order) are not a strict subset in either direction.
    let equal_behavior = vec![
        T::new("s.test.ts", 1, "a", "alpha-one")
            .behavior(&["call:x", "assert:y"])
            .build(),
        T::new("s.test.ts", 9, "b", "alpha-two")
            .behavior(&["call:x", "assert:y"])
            .build(),
    ];
    let result = analyze_with(equal_behavior, &config, 1);
    assert_eq!(
        rule_count(&result.findings, Rule::SubsumedTest),
        0,
        "equal behavior lists belong to near-duplicate-test, not subsumed-test"
    );

    // Equal alpha is duplicate-test's claim regardless of behavior shape.
    let equal_alpha = vec![
        T::new("s.test.ts", 1, "a", "same")
            .behavior(&["call:x"])
            .build(),
        T::new("s.test.ts", 9, "b", "same")
            .behavior(&["call:x", "call:y"])
            .build(),
    ];
    let result = analyze_with(equal_alpha, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::SubsumedTest), 0);
}

#[test]
fn subsumed_test_ignores_unresolved_contexts_and_empty_behavior() {
    let config = subsumed_enabled_config();
    let tests = vec![
        T::new("s.test.ts", 1, "a", "alpha-one")
            .unresolved()
            .behavior(&["call:x"])
            .build(),
        T::new("s.test.ts", 9, "b", "alpha-two")
            .behavior(&["call:x", "call:y"])
            .build(),
        // Empty behavior can never be a strict subsequence (an empty subset is vacuous).
        T::new("s.test.ts", 17, "c", "alpha-three")
            .behavior(&[])
            .build(),
        T::new("s.test.ts", 25, "d", "alpha-four")
            .behavior(&["call:z"])
            .build(),
    ];
    let result = analyze_with(tests, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::SubsumedTest), 0);
}

#[test]
fn duplicate_each_row_reports_identical_rows_of_one_declaration() {
    let config = default_config();
    // Same location (one declaration), same rendered title, same substituted body: the same
    // case registered twice.
    let rows = vec![
        T::new("e.test.ts", 5, "doubles 2", "row-body")
            .table_row()
            .build(),
        T::new("e.test.ts", 5, "doubles 2", "row-body")
            .table_row()
            .build(),
    ];
    let result = analyze_with(rows, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::DuplicateEachRow), 1);
    let finding = &result.findings[0];
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(finding.locations.len(), 2);
}

#[test]
fn duplicate_each_row_requires_row_values_to_match() {
    let config = default_config();
    // Different row values stay distinct even when only the rendered title exposes the
    // difference.
    let titled = vec![
        T::new("e.test.ts", 5, "doubles 2", "row-body")
            .table_row_as(0)
            .build(),
        T::new("e.test.ts", 5, "doubles 5", "row-body")
            .table_row_as(1)
            .build(),
    ];
    let result = analyze_with(titled, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::DuplicateEachRow), 0);

    // The same is true when only row substitution exposes the difference in the body.
    let bodied = vec![
        T::new("e.test.ts", 5, "doubles 2", "body-a")
            .table_row_as(0)
            .build(),
        T::new("e.test.ts", 5, "doubles 2", "body-b")
            .table_row_as(1)
            .build(),
    ];
    let result = analyze_with(bodied, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::DuplicateEachRow), 0);
}

/// Pins `TestCase`'s derived `Ord` for the one case its own doc comment calls out: rows of a
/// `.each`/`.for` table that share both a declaration `Location` (per-row) and a rendered `Title`
/// (a template that does not reference row data, or an unreadable table -- see
/// `duplicate_each_row_requires_title_and_body_to_match` and the loop-unroll false-positive test
/// below). `location`, `kind`, `suite_path`, `title`, `modifiers`, and `context` are all equal
/// between these two rows, so `Ord` can only settle the comparison inside `body`
/// (`Fingerprints`) -- and `alpha` is `Fingerprints`' first field, so it is the first thing that
/// can break the tie.
///
/// # What "sorted by `alpha`" means now
///
/// `Fingerprints::alpha` is a [`BodyDigest`], not the serialized text, so the tie-break is the
/// digest's order rather than the text's lexicographic order. It is still *decided by `alpha`*,
/// still total, and still a pure function of the body's content -- truncated SHA-256 is
/// deterministic, so the same two bodies order the same way on every platform, every run, and
/// every Rust release. What it is not is predictable from reading the two strings, so this test
/// asserts against the digests' own comparison instead of hardcoding a direction: that is the
/// property the code actually has, and asserting a direction taken from the text would be
/// asserting something no longer true.
///
/// # What this does and does not catch, checked by reverting rather than assumed
///
/// It fails whenever `alpha` stops deciding: drop `body` from `TestCase`'s `Ord`, or drop `alpha`
/// from `Fingerprints`' `Ord`, and `row_a.cmp(&row_b)` returns `Equal` where this expects a
/// strict order (verified by making exactly that edit and watching this test fail).
///
/// It does **not** catch `alpha` merely moving out of first position within `Fingerprints`. An
/// earlier draft of this comment claimed it did; reverting proved otherwise, because `structural`
/// and `behavior` are *equal* between these two rows, so a tie-break that reaches them first
/// falls through to `alpha` and returns the same answer. That is a real gap, and it is a small
/// one: field order inside `Fingerprints` cannot change the outcome for any pair whose earlier
/// fields are equal, which is every pair this tie-break actually decides. The assertions above
/// pin `structural` and `behavior` as equal precisely so a reader can see why.
///
/// This used to be reached by `body.exact` first, a field removed because the frontend computed
/// it and no rule ever read it. `exact` was always equal
/// between two rows of one declaration, because it renders the callback's own un-substituted
/// source text, which is identical for every row of one table (row-value substitution feeds
/// `alpha`/`structural`/`behavior`, deliberately never `exact`) -- so removing it is a no-op
/// for this comparison, not a behavior change. This test pins the *result* (sorted by `alpha`)
/// so that claim stays true if anything upstream of it -- `Fingerprints`' field order,
/// `TestCase`'s field order, or a future field addition -- ever moves without a deliberate
/// update here.
#[test]
fn each_rows_sharing_location_and_title_sort_by_body_alpha() {
    let row_b = T::new("each.test.ts", 5, "same title", "b-row-alpha")
        .table_row()
        .build();
    let row_a = T::new("each.test.ts", 5, "same title", "a-row-alpha")
        .table_row()
        .build();
    assert_eq!(
        row_a.location, row_b.location,
        "rows of one .each declaration share a Location"
    );
    assert_eq!(
        row_a.title, row_b.title,
        "an unrendered/unreadable title is shared too"
    );
    assert_eq!(
        row_a.body.structural, row_b.body.structural,
        "structural is equal here, so it cannot be what decides the tie"
    );
    assert_eq!(
        row_a.body.behavior, row_b.body.behavior,
        "behavior is equal here, so it cannot be what decides the tie"
    );

    let by_alpha = BodyDigest::of("a-row-alpha").cmp(&BodyDigest::of("b-row-alpha"));
    assert_ne!(
        by_alpha,
        std::cmp::Ordering::Equal,
        "two different bodies must not digest to the same value"
    );
    assert_eq!(
        row_a.cmp(&row_b),
        by_alpha,
        "with every earlier field tied, body.alpha's digest must decide the ordering"
    );

    let mut expected = vec![row_a.clone(), row_b.clone()];
    expected.sort();
    let mut shuffled = vec![row_b, row_a];
    shuffled.sort();
    assert_eq!(
        shuffled, expected,
        "derived Ord must sort same-location, same-title rows by body.alpha"
    );
}

/// The loop-unroll false positive, pinned: a `for...of` over literals unrolls per iteration, and the
/// iterations share one declaration location with (potentially) identical title and body because
/// the loop variable is not substituted. Shared location is not row-ness -- only
/// [`crate::model::TestCase::table_row_class`] carries that claim, and a body branching on the loop
/// variable guards different coverage per iteration.
#[test]
fn duplicate_each_row_ignores_loop_iterations_that_share_a_location() {
    let config = default_config();
    let iterations = vec![
        T::new("loop.test.ts", 5, "selects shapes", "branch-on-key").build(),
        T::new("loop.test.ts", 5, "selects shapes", "branch-on-key").build(),
    ];
    let result = analyze_with(iterations, &config, 1);
    assert_eq!(
        rule_count(&result.findings, Rule::DuplicateEachRow),
        0,
        "loop iterations at one location must not read as table rows"
    );

    // The same shape, marked as rows (what the frontend does for a real table), still fires --
    // the marker is the discriminator, not something that silently disabled the rule.
    let rows = vec![
        T::new("loop.test.ts", 5, "selects shapes", "branch-on-key")
            .table_row()
            .build(),
        T::new("loop.test.ts", 5, "selects shapes", "branch-on-key")
            .table_row()
            .build(),
    ];
    let result = analyze_with(rows, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::DuplicateEachRow), 1);
}

#[test]
fn duplicate_each_row_supports_suite_rows_and_compares_row_values() {
    let config = default_config();
    let repeated = vec![
        T::new("suite.test.ts", 5, "admin permissions", "suite-body")
            .kind(DeclarationKind::Suite)
            .table_row_as(0)
            .build(),
        T::new("suite.test.ts", 5, "editor permissions", "suite-body")
            .kind(DeclarationKind::Suite)
            .table_row_as(1)
            .build(),
        T::new("suite.test.ts", 5, "admin permissions", "suite-body")
            .kind(DeclarationKind::Suite)
            .table_row_as(0)
            .build(),
    ];
    let result = analyze_with(repeated, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::DuplicateEachRow), 1);

    let distinct = vec![
        T::new("suite.test.ts", 5, "permissions", "suite-body")
            .kind(DeclarationKind::Suite)
            .table_row_as(0)
            .build(),
        T::new("suite.test.ts", 5, "permissions", "suite-body")
            .kind(DeclarationKind::Suite)
            .table_row_as(1)
            .build(),
    ];
    let result = analyze_with(distinct, &config, 1);
    assert_eq!(
        rule_count(&result.findings, Rule::DuplicateEachRow),
        0,
        "same title and suite body do not make different table rows equal"
    );

    let nested_under_distinct_outer_rows = vec![
        T::new("suite.test.ts", 12, "inner x", "suite-body")
            .suite(&["outer a"])
            .kind(DeclarationKind::Suite)
            .table_row_as(0)
            .build(),
        T::new("suite.test.ts", 12, "inner x", "suite-body")
            .suite(&["outer b"])
            .kind(DeclarationKind::Suite)
            .table_row_as(0)
            .build(),
    ];
    let result = analyze(
        nested_under_distinct_outer_rows,
        &config,
        1,
        IncompleteCauses::default(),
    )
    .unwrap();
    assert_eq!(
        rule_count(&result.findings, Rule::DuplicateEachRow),
        0,
        "an inner table's declaration-local row class must not collide across outer suite rows"
    );
}

#[test]
fn duplicate_each_row_messages_name_the_evidence_for_tests_and_suites() {
    let config = default_config();
    let suite_rows = vec![
        T::new("suite.test.ts", 5, "admin", "suite-body")
            .kind(DeclarationKind::Suite)
            .table_row_as(0)
            .build(),
        T::new("suite.test.ts", 5, "admin", "suite-body")
            .kind(DeclarationKind::Suite)
            .table_row_as(0)
            .build(),
    ];
    let result = analyze_with(suite_rows, &config, 1);
    let suite_message = &result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateEachRow)
        .expect("suite row finding")
        .message;
    assert!(suite_message.contains("describe.each/describe.for"));
    assert!(suite_message.contains("resolved row values"));

    let test_rows = vec![
        T::new("test.test.ts", 5, "case", "body")
            .table_row()
            .build(),
        T::new("test.test.ts", 5, "case", "body")
            .table_row()
            .build(),
    ];
    let result = analyze_with(test_rows, &config, 1);
    let test_message = &result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateEachRow)
        .expect("test row finding")
        .message;
    assert!(test_message.contains("parameterized test or benchmark"));
    assert!(test_message.contains("substituted body"));
}

/// Pinned: a table this extractor could not enumerate produces one representative case with
/// an *unresolved* context. Two such representatives -- identical title, identical body -- must
/// never read as rows of one declaration through this rule. The guard is structural: rows are
/// grouped by shared declaration location (which representatives never share), and unresolved
/// contexts are excluded outright by `Context::is_comparable`, so neither an unshared location
/// nor an unread table can launder a match.
#[test]
fn duplicate_each_row_never_pairs_unresolvable_table_representatives() {
    let config = default_config();
    let tests = vec![
        T::new("e.test.ts", 5, "generated case", "shared-body")
            .unresolved()
            .build(),
        T::new("e.test.ts", 40, "generated case", "shared-body")
            .unresolved()
            .build(),
    ];
    let result = analyze_with(tests, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::DuplicateEachRow), 0);
    assert!(
        result.incomplete(),
        "unresolved contexts must keep the run visibly incomplete"
    );
}

/// A suite case with a span that actually encloses its children, the way the frontend builds them.
fn suite_at(
    path: &'static str,
    line: u32,
    start_byte: u32,
    end_byte: u32,
    title: &'static str,
) -> TestCase {
    T::new(path, line, title, "suite-body")
        .kind(DeclarationKind::Suite)
        .build_with_span(start_byte, end_byte)
}

/// The suite rule reports the duplicated `describe` blocks themselves -- titles prove which
/// declarations grouped, which counts alone cannot (two identical child tests grouped by mistake
/// would also collapse to one finding).
#[test]
fn duplicate_suite_reports_the_copy_once_and_suppresses_cross_copy_child_findings() {
    let config = default_config();
    let mut suites = vec![
        suite_at("d.test.ts", 10, 0, 500, "checkout"),
        suite_at("d.test.ts", 60, 600, 1100, "checkout"),
    ];
    // Identical children, one inside each copy's span.
    let child_in_first =
        T::new("d.test.ts", 12, "checkout > totals", "child-body").build_with_span(100, 200);
    let child_in_second =
        T::new("d.test.ts", 62, "checkout > totals", "child-body").build_with_span(700, 800);
    // An unrelated third test, identical to both children, OUTSIDE every duplicated suite: the
    // suppression must not swallow real findings that are not cross-copy restatements.
    let outsider = T::new("d.test.ts", 90, "standalone", "child-body").build_with_span(1500, 1600);
    suites.push(child_in_first);
    suites.push(child_in_second);
    suites.push(outsider);

    let result = analyze_with(suites, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::DuplicateSuite), 1);
    let suite_finding = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateSuite)
        .expect("exactly one duplicate-suite finding was asserted");
    assert_eq!(
        suite_finding.titles,
        ["checkout", "checkout"],
        "the suite rule must group the describe blocks, not the identical tests inside them: \
         {:#?}",
        suite_finding.titles
    );
    // duplicate-test between the two copies' children: suppressed. Between a copy's child and
    // the outsider: kept.
    let duplicate_tests: Vec<&Finding> = result
        .findings
        .iter()
        .filter(|finding| finding.rule == Rule::DuplicateTest)
        .collect();
    assert_eq!(
        duplicate_tests.len(),
        1,
        "exactly the child-outsider pair survives; the cross-copy pair is the suite finding's \
         to report: {duplicate_tests:#?}"
    );
    assert!(duplicate_tests[0]
        .titles
        .iter()
        .any(|title| title == "standalone"));
}

#[test]
fn duplicate_suite_ignores_inert_copies_and_differing_blocks() {
    let config = default_config();
    // A skipped copy is dead code, not executing redundancy.
    let inert = vec![suite_at("d.test.ts", 10, 0, 500, "checkout"), {
        let mut suite = suite_at("d.test.ts", 60, 600, 1100, "checkout");
        suite.modifiers.push(Modifier::Skip);
        suite
    }];
    let result = analyze_with(inert, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::DuplicateSuite), 0);

    // Different content under the same title is not a copy.
    let differing = vec![
        suite_at("d.test.ts", 10, 0, 500, "checkout"),
        T::new("d.test.ts", 60, "checkout", "different-body")
            .kind(DeclarationKind::Suite)
            .build_with_span(600, 1100),
    ];
    let result = analyze_with(differing, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::DuplicateSuite), 0);
}

#[test]
fn redundant_hook_reports_each_repeating_pair_once() {
    let config = default_config();
    let shared_hooks = vec![
        hook(HookKind::BeforeEach, "seed-primary", 4),
        hook(HookKind::BeforeEach, "seed-primary", 22),
    ];
    let tests = vec![
        T::new("h.test.ts", 10, "monthly", "alpha-one")
            .hooks(shared_hooks.clone())
            .build(),
        // A second test under the same chain must not produce a second finding for the same
        // hook pair.
        T::new("h.test.ts", 30, "regional", "alpha-two")
            .hooks(shared_hooks)
            .build(),
    ];
    let result = analyze_with(tests, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::RedundantHook), 1);
    let finding = &result.findings[0];
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(finding.titles, vec!["beforeEach", "beforeEach"]);
    assert_eq!(finding.locations[0].span.start.line, 4);
    assert_eq!(finding.locations[1].span.start.line, 22);
}

/// The hook scan charges every candidate before comparison. The first pair here does not match;
/// consuming the one-candidate budget on it must prevent the later matching pair from being
/// inspected or reported.
#[test]
fn redundant_hook_scan_budget_bounds_work_before_comparison() {
    let config = default_config();
    let shared_hooks = vec![
        hook(HookKind::BeforeEach, "seed-primary", 4),
        hook(HookKind::BeforeEach, "seed-replica", 22),
        hook(HookKind::BeforeEach, "seed-replica", 40),
    ];
    let tests = vec![T::new("h.test.ts", 50, "monthly", "alpha-one")
        .hooks(shared_hooks)
        .build()];

    let result = analyze_with_budgets(
        tests,
        &config,
        1,
        IncompleteCauses::default(),
        RunModes::default(),
        MAX_PAIR_COMPARISONS,
        1,
    )
    .unwrap();

    assert_eq!(rule_count(&result.findings, Rule::RedundantHook), 0);
    assert!(
        result.incomplete(),
        "exhausting the hook scan budget must make incompleteness visible"
    );
    assert!(
        result.incomplete_causes.hook_scan_exhausted,
        "an exhausted hook scan is its own cause"
    );
    // The point of splitting them. `MAX_PAIR_COMPARISONS` was passed in full and never reached,
    // so claiming it ran out would send a reader to the wrong limit -- the misattribution
    // `IncompleteCauses` exists to remove.
    assert!(
        !result.incomplete_causes.pair_budget_exhausted,
        "the pair budget was untouched here and must not be blamed"
    );
    let reasons = result.incomplete_causes.reasons();
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("redundant-hook")),
        "the banner must name the scan that actually ran out: {reasons:?}"
    );
    assert!(
        !reasons
            .iter()
            .any(|reason| reason.contains("pair-comparison")),
        "and must not name the one that did not: {reasons:?}"
    );
}

#[test]
fn redundant_hook_requires_same_kind_and_alpha_equal_bodies() {
    let config = default_config();
    // Same body text, different kinds: a beforeAll ancestor does not make a nested beforeEach a
    // repeat of *it*.
    let mixed_kinds = vec![
        hook(HookKind::BeforeAll, "seed-primary", 4),
        hook(HookKind::BeforeEach, "seed-primary", 22),
    ];
    let tests = vec![T::new("h.test.ts", 10, "monthly", "alpha-one")
        .hooks(mixed_kinds)
        .build()];
    let result = analyze_with(tests, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::RedundantHook), 0);

    let differing_bodies = vec![
        hook(HookKind::BeforeEach, "seed-primary", 4),
        hook(HookKind::BeforeEach, "seed-replica", 22),
    ];
    let tests = vec![T::new("h.test.ts", 10, "monthly", "alpha-one")
        .hooks(differing_bodies)
        .build()];
    let result = analyze_with(tests, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::RedundantHook), 0);
}

// ---------------------------------------------------------------------------------------------
// Rows specified by the 2026-09-27 tests-implementation plan (S5/S7/S10 blocks)
// ---------------------------------------------------------------------------------------------

/// R1: the `!trivial` guard in `emit_redundant_hook` was unconstrained before this test -- the
/// `hook()` builder hardcodes `trivial: false`, so no existing test could reach the branch and a
/// mutant deleting it survived. A hook with an empty or stub body is not evidence of repeated
/// WORK; reporting "redundant setup" about one would be a claim about nothing.
#[test]
fn redundant_hook_skips_a_pair_whose_hook_bodies_are_trivial() {
    let config = default_config();
    let trivial = |kind: HookKind, alpha: &str, line: u32| Hook {
        kind,
        location: Location {
            path: PathBuf::from("hooks.ts"),
            span: line_span(line),
        },
        body: fingerprints(alpha, true),
        idempotent: false,
        alpha_source: std::sync::Arc::from(alpha),
    };
    let result = analyze_hooked(
        vec![
            trivial(HookKind::BeforeEach, "setup", 4),
            trivial(HookKind::BeforeEach, "setup", 22),
        ],
        &config,
    );
    assert_eq!(
        rule_count(&result.findings, Rule::RedundantHook),
        0,
        "a pair of trivial hook bodies is never evidence of repeated setup"
    );

    // The control that makes the guard observable: the same pair, non-trivial, must fire. A test
    // with only the case above passes even if the guard is deleted from the wrong side.
    let result = analyze_hooked(
        vec![
            hook(HookKind::BeforeEach, "setup", 4),
            hook(HookKind::BeforeEach, "setup", 22),
        ],
        &config,
    );
    assert_eq!(rule_count(&result.findings, Rule::RedundantHook), 1);
}

/// An intervening same-kind hook that merely OBSERVES blocks the pair, in every hook kind.
///
/// The tempting rule is "only `before*` needs this, because for `after*` the outer hook runs last
/// and wins". That is wrong, and the counter-case is concrete:
///
/// ```text
/// afterEach(() => { cleanup() })                                    // outer
/// describe('teardown verification', () => {
///   afterEach(() => { expect(screen.queryByText('mounted')).toBeNull() })   // middle
///   describe('component', () => {
///     afterEach(() => { cleanup() })                                // inner
/// ```
///
/// Teardown runs inner, then middle, then outer. Delete the inner `cleanup()` and the middle
/// assertion sees a still-mounted component and fails -- so the inner hook was not redundant.
/// The intervening hook need only observe, never write, which is why the gate rejects on any
/// non-equal same-kind hook rather than trying to decide what it does.
#[test]
fn redundant_hook_declines_a_pair_with_an_intervening_same_kind_hook() {
    let config = default_config();
    for kind in [
        HookKind::BeforeEach,
        HookKind::AfterEach,
        HookKind::BeforeAll,
        HookKind::AfterAll,
    ] {
        let result = analyze_hooked(
            vec![
                hook(kind, "cleanup", 4),
                hook(kind, "assert-unmounted", 22),
                hook(kind, "cleanup", 40),
            ],
            &config,
        );
        assert_rule_count(
            &result.findings,
            Rule::RedundantHook,
            0,
            "{kind:?}: something runs between the pair that could notice the deletion",
        );
    }

    // The contrast, and the reason this is a gate rather than a blanket refusal of three-hook
    // chains: an intervening hook that IS a copy of the pair cannot observe anything the pair
    // does not, so the chain still reports -- which is what the three-hook-chain test above pins.
    let result = analyze_hooked(
        vec![
            hook(HookKind::AfterEach, "cleanup", 4),
            hook(HookKind::AfterEach, "cleanup", 22),
            hook(HookKind::AfterEach, "cleanup", 40),
        ],
        &config,
    );
    assert!(
        rule_count(&result.findings, Rule::RedundantHook) > 0,
        "an intervening COPY observes nothing the pair does not"
    );
}

/// R2: pins `for inner in &hooks[offset + 1..]` at arity three -- every LATER pair in the chain,
/// not just adjacent neighbours. `fixtures/duplicates/redundant-hook-three-level-chain.test.ts`
/// is the end-to-end twin; this one survives an extractor change.
#[test]
fn redundant_hook_reports_every_later_pair_in_a_three_hook_chain() {
    let config = default_config();
    let hooks = vec![
        hook(HookKind::BeforeEach, "reset-and-migrate", 4),
        hook(HookKind::BeforeEach, "reset-and-migrate", 22),
        hook(HookKind::BeforeEach, "reset-and-migrate", 40),
    ];
    let tests = vec![T::new("h.test.ts", 50, "monthly", "alpha-one")
        .hooks(hooks)
        .build()];
    let result = analyze_with(tests, &config, 1);
    assert_eq!(rule_count(&result.findings, Rule::RedundantHook), 3);
    let mut pairs: Vec<(u32, u32)> = result
        .findings
        .iter()
        .filter(|finding| finding.rule == Rule::RedundantHook)
        .map(|finding| {
            (
                finding.locations[0].span.start.line,
                finding.locations[1].span.start.line,
            )
        })
        .collect();
    pairs.sort_unstable();
    assert_eq!(pairs, vec![(4, 22), (4, 40), (22, 40)]);
}

/// R5, the analyzer half of S7: a row the extractor marked unresolved never participates in
/// `duplicate-each-row` grouping, even with an identical rendered title and an identical
/// substituted body. The S7 fix's extractor half marks rows whose cells were not all readable;
/// without this exclusion the marked rows would collapse exactly as the fixtures
/// `not-duplicate-each-row-*` pin from the outside. The predicate lives here, so it is pinned
/// here rather than through an extractor that must produce a particular unreadable cell.
#[test]
fn duplicate_each_row_requires_every_cell_of_a_row_to_be_readable() {
    let config = default_config();
    let rows = |second_unresolved: bool| {
        vec![
            T::new("t.test.ts", 10, "adds 1 to 2", "expect(add(1, 2))")
                .table_row()
                .build(),
            T::new("t.test.ts", 10, "adds 1 to 2", "expect(add(1, 2))")
                .table_row()
                .unresolved_of(second_unresolved)
                .build(),
        ]
    };
    let result = analyze(rows(true), &config, 1, IncompleteCauses::default()).unwrap();
    assert_eq!(
        rule_count(&result.findings, Rule::DuplicateEachRow),
        0,
        "a row carrying an unresolved context is not comparable evidence of a repeated row"
    );

    // The control: both rows resolved and genuinely identical must fire, so this test cannot
    // pass by the rule being deleted or suppressed outright.
    let result = analyze(rows(false), &config, 1, IncompleteCauses::default()).unwrap();
    assert_eq!(rule_count(&result.findings, Rule::DuplicateEachRow), 1);
}

/// The body half of S7, both directions, and the reason it is not simply "rows must match".
///
/// A first attempt required row equality unconditionally. That removed the 17 false positives and
/// also broke `fixtures/observed/observed-uniform-each-table.test.ts`, whose header had predicted
/// exactly that failure: "it fails if a future fix ... is implemented by giving up on row
/// expansion". A uniform ARRAY table whose callback has no rest parameter provably drops the
/// cells past its fixed parameters, so `[1, 2]` and `[1, 3]` under `(value) => ...` both bind
/// `value = 1` and are the same case twice -- a true positive verified against Vitest 5.
///
/// So the requirement is conditional on whether the callback can see the difference, and both
/// halves are pinned here. Dropping either arm of `row_cells_fully_observable` fails one of them.
#[test]
fn duplicate_each_row_requires_row_equality_only_when_the_callback_can_observe_it() {
    let config = default_config();
    let rows = |observable: bool| {
        (0..2)
            .map(|index| {
                let case = T::new("t.test.ts", 10, "accepts 1", "expect(typeof value)")
                    .table_row_as(index)
                    .behavior(&["assert:toBe"]);
                let case = if observable {
                    case
                } else {
                    case.row_cells_dropped()
                };
                case.build()
            })
            .collect::<Vec<_>>()
    };

    // Callback drops the differing cell: two rows binding the same arguments ARE one case.
    let dropped = analyze(rows(false), &config, 1, IncompleteCauses::default()).unwrap();
    assert_eq!(
        rule_count(&dropped.findings, Rule::DuplicateEachRow),
        1,
        "rows whose difference the callback provably cannot read are the same case twice"
    );

    // Callback can read every cell (rest parameter, or an object row bound whole): the rows
    // differ where it looks, so the rule may not claim them as one.
    let observed = analyze(rows(true), &config, 1, IncompleteCauses::default()).unwrap();
    assert_eq!(
        rule_count(&observed.findings, Rule::DuplicateEachRow),
        0,
        "rows differing anywhere the callback can observe are not one case"
    );
}

/// R6, the title half of S7: two rows of one table whose title pattern did NOT fully render
/// (`Title::Parameterized`) must not group, however identical their substituted bodies are --
/// the rendered title is half the rule's claim, and an unrendered pattern is shared by every row
/// of the declaration by construction. Today `RowKey::Executable` takes `full_title()`
/// unconditionally, so these rows DO collapse; the test is ignored until the fix lands and then
/// guards it.
#[test]
fn duplicate_each_row_requires_a_fully_rendered_title() {
    let config = default_config();
    let tests = vec![
        T::new(
            "t.test.ts",
            10,
            "overwrites default ${key} with %p",
            "expect(merge(key))",
        )
        .table_row()
        .parameterized_title()
        .build(),
        T::new(
            "t.test.ts",
            10,
            "overwrites default ${key} with %p",
            "expect(merge(key))",
        )
        .table_row()
        .parameterized_title()
        .build(),
    ];
    let result = analyze_with(tests, &config, 1);
    assert_eq!(
        rule_count(&result.findings, Rule::DuplicateEachRow),
        0,
        "rows sharing an unrendered title pattern were never shown to the reader under the \
         same title; the rule may not claim them as one"
    );
}

/// R13 (S10): `near_duplicate_test_message` must name the consolidation remedy, on **both** of
/// its branches. All 11 of the rule's actionable findings on the 2026-09-27 public scan want a
/// `test.each` table and none want deletion, so the old wording -- "review before deleting any" --
/// offered the reader exactly one decision and it was the wrong one. This is the first
/// message-text pin in this suite (mutation-scan row (f): no test asserted any user-facing
/// message text).
///
/// Both branches are exercised because the function has exactly two (reordered events, and equal
/// event order with differing bodies) and a fix applied to one site but not its sibling is this
/// repository's most repeated defect. The third assertion is the honesty half: the verdict is
/// still `similar-uncertain` and the rule's false-positive rate on that same scan was 26.7%, so
/// naming consolidation as the *likely* remedy is the goal while asserting equivalence is not --
/// a wording that quietly upgraded the claim would pass the first two assertions and fail this
/// one.
#[test]
fn near_duplicate_test_message_names_consolidation_not_deletion() {
    let same_order_events = ["call:setup", "assert:toBe"];
    let same_order = vec![
        T::new(
            "same-order.test.ts",
            1,
            "via local",
            "const x = get(); check(x)",
        )
        .structural("local then check")
        .behavior(&same_order_events)
        .build(),
        T::new("same-order.test.ts", 5, "inline", "check(get())")
            .structural("inline check")
            .behavior(&same_order_events)
            .build(),
    ];
    // The reordered branch: one multiset, two orders, and deliberately distinct `structural` per
    // side so a `parameterization-candidate` collision cannot stand in for the finding under test.
    let reordered = vec![
        T::new(
            "reordered.test.ts",
            1,
            "configures then seeds",
            "configure(); seed(); expect(isReady()).toBe(true)",
        )
        .structural("call configure; call seed; assert")
        .behavior(&["call:configure", "call:seed", "assert:toBe"])
        .build(),
        T::new(
            "reordered.test.ts",
            5,
            "seeds then configures",
            "seed(); configure(); expect(isReady()).toBe(true)",
        )
        .structural("call seed; call configure; assert")
        .behavior(&["call:seed", "call:configure", "assert:toBe"])
        .build(),
    ];

    for (branch, tests) in [("same order", same_order), ("reordered", reordered)] {
        let result = analyze_default(tests);
        let message = result
            .findings
            .iter()
            .find(|finding| finding.rule == Rule::NearDuplicateTest)
            .map(|finding| finding.message.clone())
            .unwrap_or_else(|| {
                panic!("the {branch} branch must still produce a near-duplicate finding")
            });
        assert!(
            message.contains("consolidate"),
            "the remedy the {branch} message proposes must be consolidation: {message}"
        );
        assert!(
            !message.contains("review before deleting"),
            "the {branch} message must not frame deletion as the decision to make: {message}"
        );
        assert!(
            message.contains("equivalence is not proven"),
            "the {branch} message must keep the verdict's uncertainty, not claim equivalence: \
             {message}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Context-key rows of the 2026-09-27 plan (S6/S8): R9, R10, R11
//
// `ContextKey` itself is private to this module, but its published digest
// (`test_fingerprints`' `context_key`) is exactly what the rules and the fingerprint stream see,
// so the claims are stated by extracting two one-test sources and comparing digests.
// ---------------------------------------------------------------------------------------------

/// Extracts one source's single test and returns its published `context_key` digest.
fn context_key_of(source: &str, globals: crate::frontend::Globals) -> String {
    let file = crate::discovery::SourceFile {
        path: PathBuf::from("case.test.ts"),
        language: crate::discovery::SourceLanguage::TypeScript,
        module_flavor: crate::discovery::ModuleFlavor::Ambiguous,
        config_scope: None,
        project_globals: None,
    };
    let extraction = crate::frontend::extract(source, &file, globals);
    let tests = extraction.tests;
    // A `describe` contributes a Suite record beside its child; the context under test is the
    // executable test's.
    let test = tests
        .iter()
        .find(|test| test.kind == crate::model::DeclarationKind::Test)
        .unwrap_or_else(|| panic!("source declares exactly one executable test, got {tests:?}"));
    test_fingerprints(std::slice::from_ref(test))[0]
        .context_key
        .clone()
}

/// R9 (S6 mechanism 3): a module-scope `await setup({ rootDir })` booting a fixture application is
/// ambient context every test in the file inherits, so two files booting DIFFERENT applications
/// must not compare equal -- which is what let `cross-file-duplicate` link the nuxt-i18n pair.
/// Today the awaited setup call does not reach `Context` at all, so the digests are equal and this
/// test is ignored; it is the model change the `not-cross-file-duplicate-module-scope-setup`
/// fixtures wait on, stated where the key is built rather than through a finding.
#[test]
fn context_key_folds_a_module_scope_awaited_setup_call() {
    let alpha = context_key_of(
        "await setup({ rootDir: './fixtures/alpha' });\ntest('renders', () => { expect(render()).toBeDefined(); });\n",
        crate::frontend::Globals::Enabled,
    );
    let beta = context_key_of(
        "await setup({ rootDir: './fixtures/beta' });\ntest('renders', () => { expect(render()).toBeDefined(); });\n",
        crate::frontend::Globals::Enabled,
    );
    assert_ne!(
        alpha, beta,
        "booting a different fixture application is context the rules must see"
    );
}

/// R10 (S8): an ambient (globals-mode, un-imported) `beforeAll` that stubs a global changes what
/// every test under it observes, so it must differentiate the context key even when the zero-config
/// path has not resolved the project's `globals` -- that is the observed text-splitting shape,
/// where the invisible hook let `duplicate-test` fire at error severity. Today the ambient hook
/// never registers and the keys are equal; ignored until `vi.stubGlobal` reaches `Context`.
#[test]
fn vi_stub_global_in_an_ancestor_hook_differentiates_the_context_key() {
    // The observed shape exactly: `describe`/`it`/`expect`/`vi` are imported, the hook
    // registrations are ambient -- legal only under `globals: true`, which is why the unresolved
    // zero-config mode (Globals::Disabled here) is where the defect fires.
    let import = "import { describe, it, expect, vi } from 'vitest';\n";
    let plain = context_key_of(
        &(import.to_owned()
            + "describe('available', () => { it('splits', () => { expect(split('ab')).toHaveLength(2); }); });\n"),
        crate::frontend::Globals::Disabled,
    );
    let stubbed = context_key_of(
        &(import.to_owned()
            + "describe('unavailable', () => { beforeAll(() => { vi.stubGlobal('Intl', { Segmenter: undefined }); }); it('splits', () => { expect(split('ab')).toHaveLength(2); }); });\n"),
        crate::frontend::Globals::Disabled,
    );
    assert_ne!(
        plain, stubbed,
        "same inputs under opposite global state is the file's entire purpose"
    );
}

/// `contextKey` is a PUBLISHED record: a context carrying none of the new facts must hash as before.
///
/// `ContextKey`'s canonical JSON is published as `TestFingerprint::context_key`, and a field that
/// always serializes moves every digest a consumer has already recorded -- including for tests that
/// have none of what the field describes. That happened twice, silently, with
/// `JSONL_SCHEMA_VERSION` still at "0.1": `module_setup: []` moved a single-test corpus's key from
/// `24e7184815de5034` to `48330a74e83d00fc`, and `config_scope: null` moved it again to
/// `5a74ce87a73231da`. Caught in review of the second, which made the first visible.
///
/// Absent and empty are the same fact for both fields, so both are skipped when empty and a
/// default context hashes exactly as it did before either existed. Dropping either
/// `skip_serializing_if` fails this test.
#[test]
fn an_empty_context_key_omits_the_new_fields_from_its_published_json() {
    // The contract is the serialized SHAPE, not a hash constant: a hash pinned here would be a
    // number nobody could re-derive, and the digest of a real run depends on its imports and
    // hooks too. What must hold is that a context carrying none of the new facts serializes
    // exactly as it did before they existed.
    let empty = canonical_json(&ContextKey::new(&Context::default()));
    assert!(
        !empty.contains("module_setup"),
        "an empty `module_setup` must not appear -- it would move every published digest: {empty}"
    );
    assert!(
        !empty.contains("config_scope"),
        "an absent `config_scope` must not appear, for the same reason: {empty}"
    );

    // And both must appear when they carry something, or the fields would be inert.
    let with_scope = canonical_json(&ContextKey::new(&Context {
        config_scope: Some("{ environment: 'node' }".to_owned()),
        ..Context::default()
    }));
    assert!(with_scope.contains("config_scope"), "{with_scope}");
    let with_setup = canonical_json(&ContextKey::new(&Context {
        module_setup: vec!["setup({ rootDir: './a' })".to_owned()],
        ..Context::default()
    }));
    assert!(with_setup.contains("module_setup"), "{with_setup}");

    // The digests must differ from the empty one, which is what makes the fields load-bearing.
    let empty_digest = ContextKey::new(&Context::default()).digest();
    assert_ne!(
        empty_digest,
        ContextKey::new(&Context {
            config_scope: Some("{ environment: 'node' }".to_owned()),
            ..Context::default()
        })
        .digest()
    );
    assert_ne!(
        empty_digest,
        ContextKey::new(&Context {
            module_setup: vec!["setup({ rootDir: './a' })".to_owned()],
            ..Context::default()
        })
        .digest()
    );
}

/// S6 mechanism 2: a per-directory config separates contexts by CONTENT, not by location.
///
/// Two byte-identical tests under `environment: 'node'` and `environment: 'happy-dom'` are not
/// duplicates, and the scan read no config below the analysis root, so `cross-file-duplicate`
/// linked them -- 5 of that rule's 14 adjudicated false positives are this shape: a docs
/// framework shipping one config per scenario directory, an SSR component library pinning one
/// environment, a bundler keeping five specs under a playground tree. Whose repositories those
/// are is recorded in the local provenance ledger.
///
/// Keying on content rather than path is the half worth pinning: two directories whose configs
/// are IDENTICAL must still compare, or the fix trades a precision gain for a silent recall loss.
#[test]
fn a_per_directory_config_separates_contexts_only_when_its_contents_differ() {
    let key = |scope: Option<&str>| {
        ContextKey::new(&Context {
            config_scope: scope.map(str::to_owned),
            ..Context::default()
        })
        .digest()
    };

    let node = key(Some("{ environment: 'node' }"));
    let happy = key(Some("{ environment: 'happy-dom' }"));
    let none = key(None);

    assert_ne!(node, happy, "different environments are different contexts");
    assert_eq!(
        node,
        key(Some("{ environment: 'node' }")),
        "identical configs must still compare equal -- keying on the config's PATH would separate \
         two directories that run their tests exactly the same way"
    );
    assert_ne!(
        none, node,
        "a file governed by a nested config is not the same context as one governed by the root"
    );
}

/// The discriminator R10 rests on: a file's OWN `beforeAll` is not Vitest's.
///
/// R10 registers an un-imported `beforeAll(...)` as an ambient hook, which is safe only because
/// the identifier binds to NOTHING -- no import, no declaration, no parameter. A file that
/// declares its own function of that name is a different thing entirely, and calling it must not
/// silently become a context-differentiating hook registration.
///
/// This was unpinned when first written, and the revert check is the only reason that is known:
/// dropping `is_free_reference` and registering any `beforeAll`-named callee changed no test.
#[test]
fn a_locally_declared_beforeall_is_not_registered_as_an_ambient_hook() {
    let import = "import { describe, it, expect, vi } from 'vitest';\n";
    let body = "describe('s', () => { beforeAll(() => { vi.stubGlobal('Intl', { Segmenter: undefined }); }); it('t', () => { expect(split('ab')).toHaveLength(2); }); });\n";

    // Ambient: binds to nothing, so it IS Vitest's hook under a failed globals detection.
    let ambient = context_key_of(
        &(import.to_owned() + body),
        crate::frontend::Globals::Disabled,
    );

    // The same call, with the file declaring its own `beforeAll`. Now it binds to that function,
    // and registering it would be claiming a Vitest lifecycle hook where the file has a helper.
    let own = context_key_of(
        &(import.to_owned() + "function beforeAll(f) { f(); }\n" + body),
        crate::frontend::Globals::Disabled,
    );
    assert_ne!(
        ambient, own,
        "an identifier that resolves to a local declaration is not an ambient Vitest hook"
    );

    // And the locally-declared one leaves the context exactly as a file with no such call at all.
    let none = context_key_of(
        &(import.to_owned()
            + "function beforeAll(f) { f(); }\ndescribe('s', () => { it('t', () => { expect(split('ab')).toHaveLength(2); }); });\n"),
        crate::frontend::Globals::Disabled,
    );
    assert_eq!(
        own, none,
        "calling the file's own helper registers no hook, so the context is unchanged"
    );
}

/// R11: the sibling sweep beside R10, REWRITTEN -- its original premise was wrong.
///
/// It asserted that `afterAll(() => vi.unstubAllGlobals())` restores equivalence with a file that
/// never stubbed, by analogy to `vi.unmock` cancelling `vi.mock`. That analogy does not hold, and
/// `Context::mocked_modules`' own doc draws the distinction it elides: the mock fold is over
/// **hoisted, module-top-level** registrations, whose net effect is decidable statically --
/// non-hoisted `vi.doMock`/`vi.doUnmock` explicitly "taint the file instead".
///
/// `beforeAll`/`afterAll` are neither hoisted nor module-scope. The stub is ACTIVE FOR EVERY TEST
/// in the suite; the unstub only restores state afterwards, for other files. So the tests really
/// do run under different global state, and asserting equivalence would have re-opened S8 for
/// every file that politely cleans up after itself -- which is most well-written files, including
/// the text-splitting shape R10 exists for.
///
/// What the original was actually guarding is kept, because it is a real risk: a fix must not
/// make stubbed context *irrevocably* different, i.e. unmatchable even against an identical file.
/// That is asserted below as B == C, not as B == A.
#[test]
fn vi_unstub_all_globals_restores_context_equivalence() {
    let import = "import { describe, it, expect, vi } from 'vitest';\n";
    let plain = context_key_of(
        &(import.to_owned()
            + "describe('available', () => { it('splits', () => { expect(split('ab')).toHaveLength(2); }); });\n"),
        crate::frontend::Globals::Disabled,
    );
    let stubbed_then_restored = |suite: &str| {
        context_key_of(
            &(import.to_owned()
                + &format!("describe('{suite}', () => {{ beforeAll(() => {{ vi.stubGlobal('Intl', {{ Segmenter: undefined }}); }}); afterAll(() => {{ vi.unstubAllGlobals(); }}); it('splits', () => {{ expect(split('ab')).toHaveLength(2); }}); }});\n")),
            crate::frontend::Globals::Disabled,
        )
    };

    // The tests run WITH the stub active -- `afterAll` restores state for other files, not for
    // the tests above it. So this must not compare equal to the never-stubbed file.
    assert_ne!(
        plain,
        stubbed_then_restored("unavailable"),
        "the stub is active for every test in the suite; restoring it afterwards does not make \
         these tests equivalent to ones that never ran under it"
    );

    // But two files that stub and restore the same way ARE equivalent to each other. This is the
    // half the original test was right to protect: an ambient hook must differentiate context,
    // not become a unique fingerprint nothing can ever match.
    assert_eq!(
        stubbed_then_restored("unavailable"),
        stubbed_then_restored("unavailable"),
        "identical stub-and-restore contexts must still match each other"
    );
}

mod registration_identity;
