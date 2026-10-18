use super::*;

// Row substitution
// -----------------------------------------------------------------------------------------

/// Fingerprints the first arrow callback in `source` as if it were a `.each` row body with
/// `substitution` supplied -- the state `extract.rs` puts this module in for a row whose
/// table it proved fully readable, and the only way to reach `resolve_row_substitution`.
struct RowBodyCollector<'a, 'b> {
    semantic: &'b Semantic<'a>,
    source: &'b str,
    substitution: &'b RowSubstitution,
    result: Option<BodyFingerprint>,
}

impl<'a> Visit<'a> for RowBodyCollector<'a, '_> {
    /// Fingerprint the first readable arrow callback with the fixture's row substitutions.
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if self.result.is_none() {
            if let Some(Argument::ArrowFunctionExpression(arrow)) = it.arguments.get(1) {
                self.result = Some(fingerprint(
                    FunctionLike::Arrow(arrow),
                    self.semantic,
                    self.source,
                    Path::new("case.test.ts"),
                    Some(self.substitution),
                ));
            }
        }
        walk::walk_call_expression(self, it);
    }
}

/// Fingerprint the fixture's first readable arrow callback with the supplied row
/// substitutions.
fn fingerprint_row(source: &str, substitution: &RowSubstitution) -> BodyFingerprint {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(std::path::Path::new("case.test.ts")).unwrap();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    assert!(!parsed.fatal_error, "fixture failed to parse: {source}");
    let program: &Program = &parsed.program;
    let semantic = SemanticBuilder::new().build(program).semantic;
    let mut collector = RowBodyCollector {
        semantic: &semantic,
        source,
        substitution,
        result: None,
    };
    collector.visit_program(program);
    collector.result.expect("an arrow callback")
}

/// Wrap a row-cell kind/raw-text pair in an owned optional substitution value.
fn cell(kind: &'static str, raw: &str) -> Option<(&'static str, String)> {
    Some((kind, raw.to_owned()))
}

/// A fully-readable row's cells are substituted into the body *and* erased from the printed
/// signature, so a substituted row matches the hand-written duplicate it is equivalent to --
/// which declares no parameters at all.
#[test]
fn a_readable_row_substitutes_its_cells_and_drops_the_parameters_they_bound() {
    let substituted = fingerprint_row(
        "test('t', (a, b) => { expect(area(a, b)).toBe(6); });",
        &RowSubstitution::Positional(vec![cell("number", "2"), cell("number", "3")]),
    );
    let handwritten = fingerprint_all("test('t', () => { expect(area(2, 3)).toBe(6); });");
    assert_eq!(
        substituted.alpha, handwritten[0].alpha,
        "a substituted row must be byte-identical to the hand-written form it duplicates"
    );
    assert_eq!(
        substituted.behavior, handwritten[0].behavior,
        "...including its behavior events"
    );

    // Named rows bind by field name into a single destructured parameter, and the whole
    // pattern disappears when every property it declares was substituted.
    let substituted = fingerprint_row(
        "test('t', ({ a, b }) => { expect(area(a, b)).toBe(6); });",
        &RowSubstitution::Named(vec![
            ("a".to_owned(), ("number", "2".to_owned())),
            ("b".to_owned(), ("number", "3".to_owned())),
        ]),
    );
    assert_eq!(
        substituted.alpha, handwritten[0].alpha,
        "a named row must substitute by field name"
    );

    // A property the row supplies no field for stays an ordinary local, and the pattern
    // therefore stays in the signature.
    let partial = fingerprint_row(
        "test('t', ({ a, c }) => { expect(area(a, c)).toBe(6); });",
        &RowSubstitution::Named(vec![("a".to_owned(), ("number", "2".to_owned()))]),
    );
    assert!(
        partial.alpha.contains("(object-pattern (prop prop:c "),
        "an unsupplied property is left declared: {}",
        partial.alpha
    );
    assert!(
        partial.alpha.contains("<number:2>"),
        "...while the supplied one is still substituted: {}",
        partial.alpha
    );

    // A positional gap (a trailing `TestContext` parameter, or an elided cell) is the same
    // case: that parameter is left exactly as it would be with no substitution at all.
    let gap = fingerprint_row(
        "test('t', (a, ctx) => { expect(area(a, ctx)).toBe(6); });",
        &RowSubstitution::Positional(vec![cell("number", "2"), None]),
    );
    assert!(
        gap.alpha.contains("<number:2>") && gap.alpha.contains("(fn (L0) "),
        "the substituted parameter vanishes and the gap parameter stays: {}",
        gap.alpha
    );
}

/// `resolve_row_substitution` is deliberately all-or-nothing: any shape whose cell-to-
/// parameter mapping it cannot vouch for substitutes *nothing*, because a body with some
/// parameters replaced by literals and others left as locals has lost exactly the
/// differentiation an entirely unsubstituted body still has.
#[test]
fn a_row_whose_parameter_shape_cannot_be_vouched_for_substitutes_nothing() {
    let positional = RowSubstitution::Positional(vec![cell("number", "2")]);
    let named = RowSubstitution::Named(vec![("a".to_owned(), ("number", "2".to_owned()))]);

    #[rustfmt::skip]
    let declined: &[(&str, &str, &RowSubstitution)] = &[
        ("rest-parameter", "test('t', (...a) => { g(a); });", &positional),
        ("parameter-default", "test('t', (a = 1) => { g(a); });", &positional),
        ("positional-into-a-pattern", "test('t', ([a]) => { g(a); });", &positional),
        ("named-into-a-plain-parameter", "test('t', (a) => { g(a); });", &named),
        ("named-with-two-parameters", "test('t', ({ a }, b) => { g(a); });", &named),
        ("named-into-a-rest-pattern", "test('t', ({ a, ...r }) => { g(a); });", &named),
        ("named-into-a-nested-property", "test('t', ({ a: { b } }) => { g(b); });", &named),
        ("named-into-a-computed-key", "test('t', ({ [k]: v, a }) => { g(v); });", &named),
    ];
    for (name, source, substitution) in declined {
        let with_row = fingerprint_row(source, substitution);
        let without_row = fingerprint_all(source);
        assert_eq!(
            with_row.alpha, without_row[0].alpha,
            "{name}: `{source}` must fingerprint exactly as it does with no substitution \
                 at all"
        );
    }
}

/// A written parameter no longer holds the cell it bound, so substituting its later reads
/// would render the cell where the body actually observes the write -- the false-merge class
/// helper inlining declines (`helpers.rs`'s written-parameter refusal). The refusal mirrors
/// the shape guard: all-or-nothing, and covering exactly the parameters that would have
/// entered the map.
#[test]
fn a_row_whose_parameter_is_written_substitutes_nothing() {
    let positional = RowSubstitution::Positional(vec![cell("number", "2")]);

    #[rustfmt::skip]
    let written: &[(&str, &str)] = &[
        ("plain-assignment", "test('t', (a) => { a = 5; g(a); });"),
        ("compound-assignment", "test('t', (a) => { a += 5; g(a); });"),
        ("update", "test('t', (a) => { a++; g(a); });"),
        ("destructuring-write", "test('t', (a) => { [a] = arr; g(a); });"),
        ("for-of-target", "test('t', (a) => { for (a of arr) { g(a); } });"),
    ];
    for (name, source) in written {
        let with_row = fingerprint_row(source, &positional);
        let without_row = fingerprint_all(source);
        assert_eq!(
            with_row.alpha, without_row[0].alpha,
            "{name}: a written parameter must leave the body unsubstituted: `{source}`"
        );
    }

    let named = RowSubstitution::Named(vec![("a".to_owned(), ("number", "2".to_owned()))]);
    let source = "test('t', ({ a }) => { a = 5; g(a); });";
    assert_eq!(
        fingerprint_row(source, &named).alpha,
        fingerprint_all(source)[0].alpha,
        "a written destructured property must leave the body unsubstituted"
    );

    // All-or-nothing includes the sibling: one written parameter declines the whole row.
    let source = "test('t', (a, b) => { a = 5; g(a, b); });";
    let both = RowSubstitution::Positional(vec![cell("number", "2"), cell("number", "3")]);
    assert_eq!(
        fingerprint_row(source, &both).alpha,
        fingerprint_all(source)[0].alpha,
        "one written parameter must substitute nothing for the row"
    );

    // The guard covers exactly the cell-bound parameters: a written parameter with no cell
    // stays the ordinary local it always was, and does not decline its sibling.
    let source = "test('t', (a, ctx) => { ctx = 5; g(a, ctx); });";
    let gap = RowSubstitution::Positional(vec![cell("number", "2"), None]);
    let substituted = fingerprint_row(source, &gap);
    assert!(
        substituted.alpha.contains("<number:2>"),
        "the cell-bound sibling still substitutes: {}",
        substituted.alpha
    );
}

/// An optional-chain expression used as a *callee* is the one place `call_path`,
/// `ser_chain` and `walk_expr` each reach their own `ChainElement` match -- `o?.b()` puts the
/// call inside the chain, so the callee it flattens is a plain member; `(o?.b)()` puts the
/// chain inside the callee. Every element kind is one row, and each is asserted to produce
/// the same path as its non-optional twin, which is the equivalence the arm exists for.
#[test]
fn an_optional_chain_in_callee_position_flattens_like_its_plain_twin() {
    for (name, optional, plain) in [
        ("static-member", "(o?.b)();", "o.b();"),
        ("call", "(o?.b())();", "o.b()();"),
        ("computed-member", "(o?.[0])();", "o[0]();"),
        ("non-null", "(o?.b!)();", "o.b();"),
    ] {
        assert_eq!(
            behavior_of(optional),
            behavior_of(plain),
            "{name}: `{optional}` must flatten to the same call path as `{plain}`"
        );
    }

    // The same for the class-only element kind.
    let source = "class C { #m() {} run() { test('t', () => { (this?.#m)(); }); } }";
    let events = fingerprint_all(source).swap_remove(0).behavior;
    assert!(
        events.contains(&"call:this.#m".to_owned()),
        "a private method behind an optional chain must still flatten: {events:?}"
    );

    // `ser_chain` and `walk_expr` reach their own `TSNonNullExpression` element only when the
    // assertion is the chain's outermost link.
    assert_eq!(
        alpha_of("const v = o?.b!;"),
        alpha_of("const v = o?.b;"),
        "a non-null assertion inside an optional chain carries no identity of its own"
    );
    assert_eq!(
        behavior_of("g(o?.b!);"),
        behavior_of("g(o?.b);"),
        "...and the behavior walk descends through it identically"
    );
}

/// The remaining deliberate raw-text fallbacks, each asserted to *be* the fallback rather
/// than a guess: a destructuring update target, and a side-effect-only import that declares no
/// local binding for `import_origins` to record.
#[test]
fn unmodeled_shapes_keep_their_verbatim_source() {
    // CHANGED: this used to assert `(o.b as any)++` falls back to `<opaque:o.b as any>`. It
    // was pinning an asymmetry, not a decision -- `ser_expr` unwraps the same four TS
    // wrappers in READ position, so one construct was transparent when read and opaque when
    // written, and a type assertion on a left-hand side silently stopped two otherwise
    // identical bodies from matching. Type syntax carries no runtime identity; the write path
    // now unwraps it too, and the pair below is what says so.
    assert_eq!(
        alpha_of("(o.b as any)++;"),
        alpha_of("o.b++;"),
        "a type assertion on an update target carries no runtime identity"
    );
    assert_eq!(
        alpha_of("(o.b as any) = 1;"),
        alpha_of("o.b = 1;"),
        "...and the same holds for a plain assignment target"
    );
    // Still a genuine fallback: a destructuring target is not a shape this models.
    assert!(
        alpha_of("[a.b] = pair;").contains("<opaque:"),
        "a destructuring assignment target this module does not model falls back to raw text"
    );

    // A bare `import './m';` has no specifiers at all. It must not disturb the identities of
    // the imports around it.
    let with_side_effect = fingerprint_all(
        "import './side';
import { r } from './m';
test('t', () => { g(r); });",
    );
    let without = fingerprint_all(
        "import { r } from './m';
test('t', () => { g(r); });",
    );
    assert_eq!(
        with_side_effect[0].alpha, without[0].alpha,
        "a side-effect-only import contributes no binding and changes no identity"
    );
}

/// The destructuring rewrite's read-safety guard, which is `try_register_members`' own copy
/// of the scan `a_read_inside_control_flow_or_a_closure_is_never_inlined` covers for the
/// single-use-local path.
#[test]
fn a_destructured_read_inside_control_flow_is_never_rewritten() {
    let source = "import { o } from './m';\n\
                      test('t', () => { const { a } = o; if (1) { expect(a).toBe(3); } });";
    let events = fingerprint_all(source).swap_remove(0).behavior;
    let event = events
        .iter()
        .find(|event| event.starts_with("assert:"))
        .unwrap_or_else(|| panic!("no assertion event: {events:?}"));
    assert!(
        event.contains("subject=id:L0:prop:a"),
        "a read inside a branch is not safe to rewrite as member access, but its property \
             identity must survive: {event}"
    );
}

/// A selected literal participates in owned class capture without erasing its property key.
#[test]
fn destructured_class_capture_final_outcomes() {
    use crate::model::Rule;
    let source = include_str!("../../../../fixtures/duplicates/destructured-class-capture.test.ts");
    let findings = class_outcome_findings(source, 4, 0);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].rule, Rule::DuplicateTest);
    assert_eq!(findings[0].titles, ["capture first", "capture renamed"]);
    let values =
        include_str!("../../../../fixtures/duplicates/destructured-class-capture-value.test.ts");
    let findings = complete_pair_findings(values);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].rule, Rule::ParameterizationCandidate);
    assert_eq!(findings[0].titles, ["value one", "value two"]);
}

/// Unmodeled selection still excludes its consumer, while a direct capture remains comparable.
#[test]
fn destructured_class_capture_uncertain_sources() {
    for declaration in [
        "const { value } = source();",
        "const { value = 1 } = { value: undefined };",
        "const { value, ...rest } = { value: 1 };",
        "const { value } = { ...source(), value: 1 };",
        "const { __proto__: value } = { __proto__: null };",
    ] {
        let source = format!(
            "import {{test,expect}} from 'vitest'; test('uncertain',()=>{{{declaration} class C{{read(){{return value}}}} expect(new C().read()).toBe(1)}}); test('safe first',()=>{{const x=1;class C{{read(){{return x}}}}expect(new C().read()).toBe(1)}}); test('safe renamed',()=>{{const renamed=1;class Renamed{{read(){{return renamed}}}}expect(new Renamed().read()).toBe(1)}});"
        );
        let findings = class_outcome_findings(&source, 3, 1);
        assert_eq!(findings.len(), 1, "{declaration}: {findings:?}");
        assert_eq!(findings[0].titles, ["safe first", "safe renamed"]);
    }
}

/// Constructor execution and argument identity participate without disabling known matches.
#[test]
fn constructor_final_outcomes() {
    use crate::model::Rule;
    let observed = include_str!("../../../../fixtures/duplicates/constructor-effects.test.ts");
    assert!(complete_pair_findings(observed).is_empty());
    for (first, second) in [
        ("new Store(loadA())", "new Cart(loadA())"),
        ("new Store(loadA())", "new Store(loadB())"),
        ("new constructors['a'](1)", "new constructors['b'](1)"),
        ("new Store<string>(1)", "new Store<number>(1)"),
        ("new Store(...firstArgs)", "new Store(...secondArgs)"),
        ("new Store(1)", "Store(1)"),
    ] {
        for prefix in ["", "const subject = "] {
            let source = format!(
                "import {{ test, expect }} from 'vitest'; test('a', () => {{ {prefix}{first}; tick(); expect(trace).toEqual([1]); }}); test('b', () => {{ {prefix}{second}; tick(); expect(trace).toEqual([1]); }});"
            );
            assert!(
                !complete_pair_findings(&source)
                    .iter()
                    .any(|f| f.rule == Rule::NearDuplicateTest),
                "{source}"
            );
        }
    }
    let duplicate = "import { test, expect } from 'vitest'; test('a', () => { const first = new Store(1); first.load(); expect(trace).toEqual([1]); }); test('b', () => { const renamed = new Store(1); renamed.load(); expect(trace).toEqual([1]); });";
    assert!(complete_pair_findings(duplicate)
        .iter()
        .any(|f| f.rule == Rule::DuplicateTest));
    let near = "import { test, expect } from 'vitest'; test('a', () => { new Store(1); tick(); expect(trace).toEqual([1]); }); test('b', () => { void 0; new Store(1); tick(); expect(trace).toEqual([1]); });";
    assert!(complete_pair_findings(near)
        .iter()
        .any(|f| f.rule == Rule::NearDuplicateTest));

    let literal = "import { test, expect } from 'vitest'; test('a', () => { new Store(1); tick(); expect(trace).toEqual([1]); }); test('b', () => { new Store(2); tick(); expect(trace).toEqual([1]); });";
    let findings = complete_pair_findings(literal);
    assert!(!findings.iter().any(|f| f.rule == Rule::NearDuplicateTest));
    assert_eq!(
        findings
            .iter()
            .filter(|f| f.rule == Rule::ParameterizationCandidate)
            .count(),
        1
    );
    for (first, second) in [
        (
            "new Store(first(), second());",
            "new Store(second(), first());",
        ),
        (
            "function build() { new Store(loadA()); } build();",
            "function build() { new Store(loadB()); } build();",
        ),
        (
            "const s = new Store(loadA()); s.load(); s.load();",
            "const s = new Store(loadB()); s.load(); s.load();",
        ),
    ] {
        let source = format!(
            "import {{ test, expect }} from 'vitest'; test('a', () => {{ {first} tick(); expect(trace).toEqual([1]); }}); test('b', () => {{ {second} tick(); expect(trace).toEqual([1]); }});"
        );
        assert!(
            !complete_pair_findings(&source)
                .iter()
                .any(|f| f.rule == Rule::NearDuplicateTest),
            "{source}"
        );
    }
}

/// Equivalent owned class bodies retain comparison across names, aliases and captures.
#[test]
fn owned_classes_retain_known_positive_findings() {
    use crate::model::Rule;
    for (first, second) in [
        (
            "class C { value = 1; } const v = new C(); expect(v.value).toBe(1);",
            "class Renamed { value = 1; } const v = new Renamed(); expect(v.value).toBe(1);",
        ),
        (
            "class C { value = 1; } const K = C; new K(); expect(1).toBe(1);",
            "class D { value = 1; } const Alias = D; new Alias(); expect(1).toBe(1);",
        ),
        (
            "const trace = []; class C { constructor() { trace.push(1); } } new C(); expect(trace).toEqual([1]);",
            "const trace = []; class C { constructor() { trace.push(1); } } new C(); expect(trace).toEqual([1]);",
        ),
        (
            "let x = 1; class C { value = x; } x = 2; new C(); expect(x).toBe(2);",
            "let x = 1; class C { value = x; } x = 2; new C(); expect(x).toBe(2);",
        ),
        (
            "class A { value = 1; } class B { value = 2; } let K = A; K = B; new K(); expect(1).toBe(1);",
            "class A { value = 1; } class B { value = 2; } let K = A; K = B; new K(); expect(1).toBe(1);",
        ),
        (
            "const C = class { #value = 1; get value() { return this.#value; } }; const v = new C(); expect(v.value).toBe(1);",
            "const C = class { #renamed = 1; get value() { return this.#renamed; } }; const v = new C(); expect(v.value).toBe(1);",
        ),
        (
            "function C() { this.value = 1; } const v = new C(); expect(v.value).toBe(1);",
            "function C() { this.value = 1; } const v = new C(); expect(v.value).toBe(1);",
        ),
    ] {
        let source = format!(
            "import {{test, expect}} from 'vitest'; test('first', () => {{ {first} }}); test('second', () => {{ {second} }});"
        );
        let findings = complete_pair_findings(&source);
        assert_eq!(
            findings
                .iter()
                .filter(|finding| finding.rule == Rule::DuplicateTest)
                .count(),
            1,
            "{source}: {findings:?}"
        );
    }
    let near = "import {test,expect} from 'vitest'; test('a',()=>{class C{value=1} new C(); tick(); expect(1).toBe(1)}); test('b',()=>{void 0; class C{value=1} new C(); tick(); expect(1).toBe(1)});";
    assert_eq!(
        complete_pair_findings(near)
            .iter()
            .filter(|finding| finding.rule == Rule::NearDuplicateTest)
            .count(),
        1
    );
}

/// Implementation, nominal identity, captured state and definition effects cannot merge.
#[test]
fn owned_classes_distinguish_conflicting_final_outcomes() {
    use crate::model::Rule;
    let observed = include_str!("../../../../fixtures/duplicates/local-class-constructor.test.ts");
    let findings = complete_pair_findings(observed);
    assert!(!findings.iter().any(|finding| matches!(
        finding.rule,
        Rule::DuplicateTest | Rule::NearDuplicateTest | Rule::SubsumedTest
    )));
    assert_eq!(
        findings
            .iter()
            .filter(|finding| finding.rule == Rule::ParameterizationCandidate)
            .count(),
        1
    );
    for (first, second) in [
        (
            "class C { constructor() { trace.push(first()); } } new C();",
            "class C { constructor() { trace.push(second()); } } new C();",
        ),
        (
            "const C = class { value = first(); }; new C();",
            "const C = class { value = second(); }; new C();",
        ),
        (
            "class C { get value() { return first(); } } new C();",
            "class C { value() { return first(); } } new C();",
        ),
        (
            "class C extends First { } new C();",
            "class C extends Second { } new C();",
        ),
        (
            "class C { static { first(); } } new C();",
            "class C { static { second(); } } new C();",
        ),
        (
            "class C { static value = first(); } new C();",
            "class C { value = first(); } new C();",
        ),
        (
            "class A {} class B {} new A();",
            "class A {} class B {} new B();",
        ),
        (
            "const x = 1; class C { value = x; } new C();",
            "const x = 2; class C { value = x; } new C();",
        ),
        (
            "let x = 1; class C { value = x; } x = 2; new C();",
            "let x = 1; class C { value = x; } x = 3; new C();",
        ),
        (
            "const trace = []; trace.push(1); class C { constructor() { trace.push(2); } } new C();",
            "const trace = [99]; trace.push(1); class C { constructor() { trace.push(2); } } new C();",
        ),
    ] {
        let source = format!(
            "import {{test, expect}} from 'vitest'; const trace=[]; test('first', () => {{ {first} expect(trace).toEqual([1]); }}); test('second', () => {{ {second} expect(trace).toEqual([1]); }});"
        );
        let findings = complete_pair_findings(&source);
        assert_class_distinct(&findings, &source);
    }
}

/// Pin equivalent, parameterizable, distinct and unresolved owned-class outcomes.
#[test]
fn owned_class_fragility_outcomes() {
    use crate::model::Rule;
    let equivalent =
        include_str!("../../../../fixtures/duplicates/capture-binding-renamed.test.ts");
    let retained = complete_pair_findings(equivalent);
    assert_eq!(retained.len(), 1, "{retained:?}");
    assert_eq!(retained[0].rule, Rule::DuplicateTest);

    let escaped = include_str!("../../../../fixtures/duplicates/escaped-instance-name.test.ts");
    let escaped_findings = complete_pair_findings(escaped);
    assert_eq!(escaped_findings.len(), 1, "{escaped_findings:?}");
    assert_eq!(escaped_findings[0].rule, Rule::ParameterizationCandidate);

    let escaped_mismatch =
        include_str!("../../../../fixtures/duplicates/escaped-instance-mismatch.test.ts");
    let mismatch_findings = complete_pair_findings(escaped_mismatch);
    assert_eq!(mismatch_findings.len(), 1, "{mismatch_findings:?}");
    assert_eq!(mismatch_findings[0].rule, Rule::ParameterizationCandidate);

    let whole_instance =
        include_str!("../../../../fixtures/duplicates/instance-name-assertion.test.ts");
    let whole_findings = complete_pair_findings(whole_instance);
    assert_eq!(whole_findings.len(), 1, "{whole_findings:?}");
    assert_eq!(whole_findings[0].rule, Rule::ParameterizationCandidate);

    let nonreflective =
        include_str!("../../../../fixtures/duplicates/instance-nonreflective-assertion.test.ts");
    let nonreflective_findings = complete_pair_findings(nonreflective);
    assert_eq!(
        nonreflective_findings.len(),
        1,
        "{nonreflective_findings:?}"
    );
    assert_eq!(nonreflective_findings[0].rule, Rule::DuplicateTest);

    let erased_type = include_str!("../../../../fixtures/duplicates/erased-field-type.test.ts");
    let typed_findings = complete_pair_findings(erased_type);
    assert_eq!(typed_findings.len(), 1, "{typed_findings:?}");
    assert_eq!(typed_findings[0].rule, Rule::DuplicateTest);

    let changed_effect =
        include_str!("../../../../fixtures/duplicates/local-class-constructor.test.ts");
    let changed_findings = complete_pair_findings(changed_effect);
    assert_eq!(changed_findings.len(), 1, "{changed_findings:?}");
    assert_eq!(changed_findings[0].rule, Rule::ParameterizationCandidate);

    let uncertain =
        include_str!("../../../../fixtures/duplicates/external-instance-owned-control.test.ts");
    let scoped = class_outcome_findings(uncertain, 4, 2);
    assert_eq!(scoped.len(), 1, "{scoped:?}");
    assert_eq!(scoped[0].rule, Rule::DuplicateTest);
}

/// Uncertain instance and class-object writes exclude only their dependent tests.
#[test]
fn owned_class_review_dynamic_write_scope() {
    use crate::model::Rule;
    for source in [
        include_str!("../../../../fixtures/duplicates/loop-instance-write.test.ts"),
        include_str!("../../../../fixtures/duplicates/class-mutation-after-instance.test.ts"),
        include_str!("../../../../fixtures/duplicates/assigned-class-alias-mutation.test.ts"),
    ] {
        let findings = class_outcome_findings(source, 4, 2);
        assert_eq!(findings.len(), 1, "{source}: {findings:?}");
        assert_eq!(findings[0].rule, Rule::DuplicateTest);
    }
    let hoisted = include_str!("../../../../fixtures/duplicates/hoisted-instance-write.test.ts");
    let findings = class_outcome_findings(hoisted, 4, 2);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].rule, Rule::DuplicateTest);
    assert_eq!(
        findings[0].titles,
        ["safe hoisted first", "safe hoisted second"]
    );
}

/// Equal initializers cannot erase which enclosing binding a class or closure captures.
#[test]
fn owned_capture_bindings_preserve_identity_and_renaming() {
    for source in [
        include_str!("../../../../fixtures/duplicates/capture-binding-constructor.test.ts"),
        include_str!("../../../../fixtures/duplicates/capture-binding-object.test.ts"),
        include_str!("../../../../fixtures/duplicates/capture-binding-nested-static.test.ts"),
        include_str!("../../../../fixtures/duplicates/capture-binding-callable-field.test.ts"),
        include_str!("../../../../fixtures/duplicates/capture-binding-static.test.ts"),
    ] {
        let findings = complete_pair_findings(source);
        assert!(findings.is_empty(), "{source}: {findings:?}");
    }
    let nested = include_str!("../../../../fixtures/duplicates/capture-binding-nested.test.ts");
    let retained = class_outcome_findings(nested, 4, 2);
    assert_eq!(retained.len(), 1, "{retained:?}");
    assert_eq!(retained[0].rule, crate::model::Rule::DuplicateTest);
    assert_eq!(retained[0].titles, ["control first", "control second"]);
    for source in [
        include_str!("../../../../fixtures/duplicates/capture-binding-renamed.test.ts"),
        include_str!("../../../../fixtures/duplicates/capture-binding-closure-renamed.test.ts"),
    ] {
        let findings = complete_pair_findings(source);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule, crate::model::Rule::DuplicateTest);
    }
}

/// Distinct constructions retain declaration identity even before their first observation.
#[test]
fn owned_instances_preserve_binding_identity() {
    use crate::model::Rule;
    let settings = r#"{"rules":{"subsumed-test":"warning"}}"#;
    for source in [
        include_str!("../../../../fixtures/duplicates/instance-binding-references.test.ts"),
        include_str!("../../../../fixtures/duplicates/instance-binding-order.test.ts"),
        include_str!("../../../../fixtures/duplicates/instance-binding-mutation.test.ts"),
    ] {
        let findings =
            class_outcome_findings_with_files(source, 2, 0, &[("settings.json", settings)]);
        assert!(findings.is_empty(), "{source}: {findings:?}");
    }
    for (source, rule) in [
        (
            include_str!("../../../../fixtures/duplicates/instance-binding-renamed.test.ts"),
            Rule::DuplicateTest,
        ),
        (
            include_str!("../../../../fixtures/duplicates/instance-binding-near.test.ts"),
            Rule::NearDuplicateTest,
        ),
        (
            include_str!("../../../../fixtures/duplicates/instance-binding-parameterized.test.ts"),
            Rule::ParameterizationCandidate,
        ),
    ] {
        let findings = complete_pair_findings(source);
        assert_eq!(findings.len(), 1, "{source}: {findings:?}");
        assert_eq!(findings[0].rule, rule, "{source}: {findings:?}");
    }
}

/// Native and imported instances do not enter the owned-class write uncertainty path.
#[test]
fn owned_instance_tracking_excludes_external_constructors() {
    use crate::model::Rule;
    for source in [
        include_str!("../../../../fixtures/duplicates/external-instance-conditional.test.ts"),
        include_str!("../../../../fixtures/duplicates/external-instance-self.test.ts"),
        include_str!("../../../../fixtures/duplicates/external-instance-reflection.test.ts"),
        include_str!("../../../../fixtures/duplicates/external-instance-imported.test.ts"),
    ] {
        let findings = complete_pair_findings(source);
        assert_eq!(findings.len(), 1, "{source}: {findings:?}");
        assert_eq!(findings[0].rule, Rule::DuplicateTest);
    }
    let source =
        include_str!("../../../../fixtures/duplicates/external-instance-owned-control.test.ts");
    let findings = class_outcome_findings(source, 4, 2);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].rule, Rule::DuplicateTest);
    assert_eq!(findings[0].titles, ["control first", "control second"]);
    let aliases =
        include_str!("../../../../fixtures/duplicates/external-instance-alias-control.test.ts");
    let findings = class_outcome_findings(aliases, 6, 4);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].rule, Rule::DuplicateTest);
    assert_eq!(findings[0].titles, ["control first", "control second"]);
}

/// Readable each-row cells participate in the owned capture environment.
#[test]
fn owned_classes_keep_each_row_capture_values() {
    use crate::model::Rule;
    let source = "import {test,expect} from 'vitest'; test.each([1])('a %s',(x)=>{class C{value=x} new C(); expect(1).toBe(1)}); test.each([2])('b %s',(x)=>{class C{value=x} new C(); expect(1).toBe(1)});";
    let findings = complete_pair_findings(source);
    assert!(!findings
        .iter()
        .any(|finding| matches!(finding.rule, Rule::DuplicateTest | Rule::NearDuplicateTest)));
    assert_eq!(
        findings
            .iter()
            .filter(|finding| finding.rule == Rule::ParameterizationCandidate)
            .count(),
        1
    );
}
