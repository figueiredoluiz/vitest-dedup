//! The trace's claims, as equal, conflicting and uncertain pairs of owned bodies.
//!
//! Every row of the contract's control table lives here: an equal pair's events are equal, a
//! conflicting pair's are not, and an uncertain pair's outcome is a stated abstention rather
//! than a claim. Each mechanism's test names the one edit that would break it, so a revert can
//! be proved by that test failing alone.

use super::{trace_text, Trace};
use crate::inventory::SiteKind;
use crate::model::sha256_128;
use crate::normal::CallbackForm;
use std::collections::HashMap;

/// Runtime-different owned pairs never supply equal complete traces; safe extraction still does.
#[test]
fn owned_trace_soundness_preserves_runtime_distinctions() {
    let source = include_str!("../../fixtures/behavior-runtime/trace-soundness.test.ts");
    let events = events(source, &[]);
    assert_eq!(events.len(), 28);
    for title in [
        "branch eager",
        "branch moved",
        "class a",
        "class b",
        "class alias a",
        "class alias b",
        "tag a",
        "tag b",
        "spread a",
        "spread b",
        "if pass",
        "if fail",
        "try pass",
        "try fail",
        "flat one",
        "flat two",
    ] {
        assert!(
            !complete(&events, title),
            "{title} must not certify a partial trace"
        );
    }
    for (left, right) in [
        ("closure eager", "closure moved"),
        ("tdz eager", "tdz moved"),
        ("call a", "call b"),
        ("chai a", "chai b"),
    ] {
        assert_ne!(of(&events, left), of(&events, right), "{left}/{right}");
    }
    assert_eq!(of(&events, "positive a"), of(&events, "positive b"));
}

/// The enclosing statement participates in read safety for both substitution mechanisms.
#[test]
fn trace_substitutions_never_cross_a_control_or_closure_root() {
    for statement in [
        "if (flag) use(x)",
        "for (;flag;) use(x)",
        "for (const item of items) use(x)",
        "for (const item in items) use(x)",
        "while (flag) use(x)",
        "do { use(x) } while (flag)",
        "switch (flag) { case true: use(x) }",
        "try { use(x) } catch {}",
        "function later() { use(x) }",
    ] {
        for declaration in ["const x = make();", "const { x } = outside;"] {
            let source = format!(
                "const outside = {{ x: 1 }}; test('case', () => {{ {declaration} {statement} }});"
            );
            crate::testing::project(
                &format!("import {{ test }} from 'vitest'; {source}"),
                &[],
                false,
                |_, parsed, found, normalizer| {
                    assert_eq!(found.sites.len(), 1);
                    let callback = normalizer.callback(found.sites[0].node).unwrap();
                    let flow = super::dataflow::analyze(parsed, callback.function.unwrap());
                    assert!(flow.inline.is_empty(), "{source}");
                    assert!(flow.member.is_empty(), "{source}");
                },
            );
        }
    }
}

/// A selector's whole value survives ordinary, optional, nested and fluent callee paths.
#[test]
fn computed_callee_keys_keep_identity() {
    for (left, right) in [
        ("handlers['a']()", "handlers['b']()"),
        ("handlers[0]()", "handlers[1]()"),
        ("handlers[a]()", "handlers[b]()"),
        ("handlers['a']?.()", "handlers['b']?.()"),
        ("handlers['a'].run()", "handlers['b'].run()"),
        ("handlers['a']().run()", "handlers['b']().run()"),
    ] {
        let source = format!("const a = 'a'; const b = 'b'; test('a', () => {{ {left} }}); test('b', () => {{ {right} }});");
        let events = events(&source, &[]);
        assert_eq!(events.len(), 2);
        assert_ne!(of(&events, "a"), of(&events, "b"), "{source}");
    }
}

/// Atomic assertions and alias reads cannot hide an unprovable local class.
#[test]
fn unprovable_classes_refuse_all_local_read_routes() {
    for body in [
        "class C { [key]() {} } expect(new C()).toBeDefined()",
        "class C { [key]() {} } use(C)",
        "class C { [key]() {} } const D = C; expect(D).toBeDefined()",
        "const C = class { [key]() {} }; expect(C).toBeDefined()",
        "const C = (class { [key]() {} } as any); expect(C).toBeDefined()",
        "expect(class { [key]() {} }).toBeDefined()",
        "function unused() { class C { [key]() {} } }; expect(1).toBe(1)",
    ] {
        let events = events(&format!("test('case', () => {{ {body} }});"), &[]);
        assert_eq!(events.len(), 1);
        assert!(!complete(&events, "case"), "{body}");
    }
}

/// Omitted runtime expressions and branch partitions cannot certify a complete trace.
#[test]
fn unmodeled_expression_and_branch_walks_are_incomplete() {
    for body in [
        "tag`a`",
        "import('./x')",
        "const a = [...values]",
        "const a = { ...values }",
        "function* gen() { yield 1 }",
        "flag ? a() : b()",
        "flag && a()",
        "flag || a()",
        "flag ?? a()",
        "(tag`a` as unknown)",
        "(flag ? a() : b()) satisfies unknown",
        "switch (flag) { case true: a() }",
    ] {
        let events = events(
            &format!("test('case', () => {{ {body}; expect(1).toBe(1) }});"),
            &[],
        );
        assert_eq!(events.len(), 1);
        assert!(!complete(&events, "case"), "{body}");
    }
}

/// The omitted JSX, iterator, object-copy and import effects use the permanent runtime source.
#[test]
fn owned_expression_walks_refuse_partial_traces() {
    let events = events(
        include_str!("../../fixtures/behavior-runtime/trace-expressions.test.tsx"),
        &[],
    );
    assert_eq!(events.len(), 10);
    for (title, (_, complete, _)) in events {
        assert!(!complete, "{title}");
    }
}

/// The text events of every inline test callback of `main`, by title, in a project holding
/// `files`, with the callback form beside them and whether the trace completed.
fn events(
    main: &str,
    files: &[(&str, &str)],
) -> HashMap<String, (Vec<String>, bool, CallbackForm)> {
    // Vitest collects nothing from a bare `test` when globals are off, so the fixture imports it.
    let main = format!("import {{ test }} from 'vitest';\n{main}");
    crate::testing::project(&main, files, false, |_, parsed, found, normalizer| {
        found
            .sites
            .iter()
            .filter(|site| site.kind == SiteKind::Registers(crate::framework::Registers::Test))
            .filter_map(|site| {
                let title = site.title.clone()?;
                let callback = normalizer.callback(site.node)?;
                if !callback.inline {
                    return None;
                }
                // The trace walks the function itself, not the argument as written: a
                // parenthesized or type-wrapped callback would otherwise read as a body with no
                // statements in it.
                let (events, complete, form) = trace_text(
                    normalizer,
                    parsed,
                    callback.function.unwrap_or(callback.argument),
                    HashMap::new(),
                    false,
                );
                Some((title, (events, complete, form)))
            })
            .collect()
    })
}

/// The events of the test titled `title`, when its trace completed.
fn of<'a>(
    events: &'a HashMap<String, (Vec<String>, bool, CallbackForm)>,
    title: &str,
) -> &'a [String] {
    let (events, complete, _) = &events[title];
    assert!(complete, "{title} did not trace to completion");
    events
}

/// Whether the test titled `title` traced to completion.
fn complete(events: &HashMap<String, (Vec<String>, bool, CallbackForm)>, title: &str) -> bool {
    events[title].1
}

/// Recorded events remain inspectable, but a partial trace must never reach a comparison.
fn partial<'a>(
    events: &'a HashMap<String, (Vec<String>, bool, CallbackForm)>,
    title: &str,
) -> &'a [String] {
    assert!(!complete(events, title), "{title} must be incomplete");
    &events[title].0
}

/// A call's arguments render into its event: an inlined local reads as its initializer, a
/// differing literal stays apart, and a spread never merges with another spread.
#[test]
fn call_arguments_are_the_event_text() {
    let events = events(
        r#"
        test('equal-inlined', () => { const sel = '.a'; find(sel); });
        test('equal-literal', () => { find('.a'); });
        test('conflicting', () => { find('.b'); });
        test('spread-a', () => { find(...a); });
        test('spread-b', () => { find(...b); });
    "#,
        &[],
    );
    assert_eq!(of(&events, "equal-inlined"), of(&events, "equal-literal"));
    assert_ne!(of(&events, "equal-literal"), of(&events, "conflicting"));
    assert_ne!(of(&events, "spread-a"), of(&events, "spread-b"));
}

/// An assertion is atomic and carries its modes: the subject renders as shape, the expected
/// values keep their literals, and opposite polarity is a different event.
#[test]
fn assertions_are_atomic_and_carry_their_modes() {
    let events = events(
        r#"
        test('equal-subject', () => { expect(build("x")).toBe(1); expect(build("y")).toBe(1); });
        test('conflicting-value', () => { expect(build("x")).toBe(2); });
        test('conflicting-polarity', () => { expect(build("x")).not.toBe(1); });
        test('conflicting-matcher', () => { expect(build("x")).toEqual(1); });
    "#,
        &[],
    );
    assert_eq!(
        of(&events, "equal-subject")[0],
        of(&events, "equal-subject")[1]
    );
    assert_ne!(
        of(&events, "equal-subject")[0],
        of(&events, "conflicting-value")[0]
    );
    assert_ne!(
        of(&events, "equal-subject")[0],
        of(&events, "conflicting-polarity")[0]
    );
    assert_ne!(
        of(&events, "equal-subject")[0],
        of(&events, "conflicting-matcher")[0]
    );
}

/// A spread in an assertion's values renders: two assertions over different spreads stay apart.
#[test]
fn assertion_values_render_spreads() {
    let events = events(
        r#"
        test('a', () => { expect(mock).toHaveBeenCalledWith(...a); });
        test('b', () => { expect(mock).toHaveBeenCalledWith(...b); });
    "#,
        &[],
    );
    assert_ne!(of(&events, "a"), of(&events, "b"));
}

/// A chai-style property chain records its links verbatim, additively over the base call's own
/// event: opposite assertions over the same setup differ.
#[test]
fn chai_property_chains_are_verbatim_and_additive() {
    let events = events(
        r#"
        test('called-once', () => { vi.fn()(); expect(spy).to.have.been.calledOnce; });
        test('called-twice', () => { vi.fn()(); expect(spy).to.have.been.calledTwice; });
    "#,
        &[],
    );
    assert_ne!(of(&events, "called-once"), of(&events, "called-twice"));
    let chain: Vec<&String> = of(&events, "called-once")
        .iter()
        .filter(|event| event.starts_with("assert:chain:"))
        .collect();
    assert_eq!(chain.len(), 1, "one chain event, verbatim: {:?}", chain);
    assert!(chain[0].contains("to.have.been.calledOnce"));
}

/// A chain past the link bound is declined whole: no chain event, never a prefix of one.
#[test]
fn a_chain_past_the_bound_emits_no_chain_event() {
    let chain = (0..20)
        .map(|index| format!(".link{index}"))
        .collect::<String>();
    let events = events(
        &format!(
            r#"
        test('long', () => {{ expect(spy){chain}.ok; }});
    "#
        ),
        &[],
    );
    assert!(
        !of(&events, "long")
            .iter()
            .any(|event| event.starts_with("assert:chain:")),
        "a 20-link chain is declined whole: {:?}",
        of(&events, "long")
    );
}

/// A local class renders by its content: a renamed copy links, different content does not, and
/// a dependency the trace cannot prove leaves the trace incomplete.
#[test]
fn local_classes_render_by_content() {
    let events = events(
        r#"
        test('renamed-copy', () => { class C { m() { return 1; } } new C(); });
        test('renamed-twin', () => { class D { m() { return 1; } } new D(); });
        test('different-content', () => { class E { m() { return 2; } } new E(); });
        test('unprovable', () => { class F { [k]() {} } new F(); });
        test('bound-expression', () => { const G = class {}; new G(); });
        test('bound-twin', () => { const H = class {}; new H(); });
        test('bound-different', () => { const I = class { m() {} }; new I(); });
    "#,
        &[],
    );
    assert_eq!(of(&events, "renamed-copy"), of(&events, "renamed-twin"));
    assert_ne!(
        of(&events, "renamed-copy"),
        of(&events, "different-content")
    );
    assert_eq!(of(&events, "bound-expression"), of(&events, "bound-twin"));
    assert_ne!(
        of(&events, "bound-expression"),
        of(&events, "bound-different"),
        "two class expressions with different content never share a trace"
    );
    assert!(!complete(&events, "unprovable"));
}

/// The member rewrite moves a read of the object to each use, so it applies only when every read
/// follows the declaration. A read before it, or inside it, keeps the binding, and traces whole.
#[test]
fn a_read_at_or_before_a_destructuring_declaration_keeps_the_binding() {
    let traced = events(
        r#"
        const shared = { a: 1 };
        test('before', () => { use(a); const { a } = shared; use(a); });
        test('within', () => { const { a, [a]: b } = shared; use(a, b); });
        test('after', () => { const { a } = shared; use(a); });
        test('written-out', () => { use(shared.a); });
    "#,
        &[],
    );
    assert!(complete(&traced, "before"));
    assert!(complete(&traced, "within"));
    assert_eq!(of(&traced, "after"), of(&traced, "written-out"));
    assert!(of(&traced, "before")
        .iter()
        .all(|event| event.contains(":prop:a")));
}

/// A class reading another twice doubles its text per level, so class rendering spends a budget:
/// a short chain renders whole, and a long one finishes promptly with the trace incomplete.
#[test]
fn a_class_chain_spends_the_render_budget() {
    let chain = |levels: usize| {
        let mut classes = "class A0 { v() { return 1; } } ".to_owned();
        for level in 1..=levels {
            let below = level - 1;
            classes.push_str(&format!(
                "class A{level} {{ a() {{ return new A{below}(); }} b() {{ return new A{below}(); }} }} "
            ));
        }
        format!("test('chain', () => {{ {classes} use(new A{levels}()); }});")
    };
    assert!(complete(&events(&chain(3), &[]), "chain"));
    assert!(!complete(&events(&chain(16), &[]), "chain"));
}

/// A constructor renders its class's content too, so it spends the same budget.
#[test]
fn a_constructor_spends_the_render_budget() {
    let methods: String = (0..600)
        .map(|index| format!("m{index}() {{ return {index}; }} "))
        .collect();
    let body = |reads: usize| {
        let reads = "new C(); ".repeat(reads);
        format!("test('reads', () => {{ class C {{ {methods} }} {reads} }});")
    };
    assert!(complete(&events(&body(1), &[]), "reads"));
    let spent = events(&body(60), &[]);
    assert!(!complete(&spent, "reads"));
    let written: usize = spent["reads"].0.iter().map(String::len).sum();
    assert!(
        written <= 2 * crate::normal::MAX_CLASS_RENDER_BYTES,
        "{written}"
    );
}

/// `await f(); g()` and the returned chain are the same events; a rejection handler, an
/// unreturned chain, a parameterized callback and an expression body are never spliced.
#[test]
fn a_returned_chain_is_the_await_it_is() {
    let events = events(
        r#"
        test('awaited', async () => { await f(); g(); });
        test('returned-chain', async () => { return f().then(() => { g(); }); });
        test('rejection-handler', async () => { return f().then(() => { g(); }, () => {}); });
        test('unreturned', async () => { f().then(() => { g(); }); });
        test('parameterized', async () => { return f().then((v) => { g(v); }); });
        test('expression-body', async () => { return f().then(() => g()); });
    "#,
        &[],
    );
    assert_eq!(of(&events, "awaited"), of(&events, "returned-chain"));
    assert_ne!(
        of(&events, "returned-chain"),
        of(&events, "rejection-handler")
    );
    assert_ne!(of(&events, "returned-chain"), of(&events, "unreturned"));
    assert_ne!(of(&events, "returned-chain"), of(&events, "parameterized"));
    assert_ne!(
        of(&events, "returned-chain"),
        of(&events, "expression-body")
    );
}

/// A nested callable's events carry the `deferred-` prefix: they are never the body's own.
#[test]
fn nested_callables_are_deferred() {
    let events = events(
        r#"
        test('nested', () => { () => { g(); }; h(); });
        test('flat', () => { h(); });
    "#,
        &[],
    );
    let nested = of(&events, "nested");
    assert!(
        nested.iter().any(|event| event == "deferred-call:g:g"),
        "the nested callable's event is deferred: {:?}",
        nested
    );
    assert!(nested.iter().any(|event| event == "call:g:h"));
    assert!(!of(&events, "flat")
        .iter()
        .any(|event| event.starts_with("deferred-")));
}

/// Sync loop kinds share one token; async iteration has its own.
#[test]
fn loop_kinds_share_one_token_await_has_its_own() {
    let events = events(
        r#"
        test('for', () => { for (let i = 0; i < 2; i++) { g(); } });
        test('while', () => { let i = 0; while (i < 2) { g(); i++; } });
        test('for-await', async () => { for await (const v of xs) { g(); } });
    "#,
        &[],
    );
    let loops = |events: &[String]| {
        events
            .iter()
            .filter(|event| event.starts_with("loop"))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(loops(of(&events, "for")), ["loop"]);
    assert_eq!(loops(of(&events, "while")), ["loop"]);
    assert_eq!(loops(of(&events, "for-await")), ["loop-await"]);
}

/// Names come from one authority: a local's rename changes nothing, a module-level pair reordered
/// across two tests links, and two different outside objects stay apart.
#[test]
fn naming_is_event_order_and_declaration_tokens() {
    let events = events(
        r#"
        test('order-a', () => { setupA(); setupB(); });
        test('order-b', () => { setupB(); setupA(); });
        test('object-a', () => { o1.first(); o2.second(); });
        test('object-b', () => { o1.second(); o2.first(); });
        test('object-c', () => { o2.first(); o1.second(); });
    "#,
        &[("helpers.ts", "export function setupA() {}\nexport function setupB() {}\nexport const o1 = {}\nexport const o2 = {}\n")],
    );
    let mut sorted_a = of(&events, "order-a").to_vec();
    let mut sorted_b = of(&events, "order-b").to_vec();
    sorted_a.sort();
    sorted_b.sort();
    assert_eq!(
        sorted_a, sorted_b,
        "the reordered pair shares one sorted multiset"
    );
    assert_ne!(of(&events, "object-a"), of(&events, "object-b"));
    let mut sorted_b = of(&events, "object-b").to_vec();
    let mut sorted_c = of(&events, "object-c").to_vec();
    sorted_b.sort();
    sorted_c.sort();
    assert_eq!(
        sorted_b, sorted_c,
        "the swapped pair shares one sorted multiset"
    );
}

/// A local declared earlier but read later shifts nothing: the extra setup keeps the smaller
/// body's events a subsequence of the larger's, because numbering follows the events.
#[test]
fn an_extra_local_declared_first_keeps_the_subsequence() {
    let events = events(
        r#"
        test('small', () => { const a = make(); a.x(); a.y(); });
        test('large', () => { const z = other(); const a = make(); a.x(); a.y(); z.p(); z.q(); });
    "#,
        &[],
    );
    let (small, large) = (of(&events, "small"), of(&events, "large"));
    let mut cursor = 0;
    for event in small {
        while cursor < large.len() && large[cursor] != *event {
            cursor += 1;
        }
        assert!(
            cursor < large.len(),
            "{small:?} is a subsequence of {large:?}"
        );
        cursor += 1;
    }
}

/// A single-use local's initializer moves to its read: the inlined and the written-out bodies
/// carry the same events, in the same order; two reads or an effect between never inline.
#[test]
fn a_single_use_local_inlines_at_its_read() {
    let events = events(
        r#"
        test('inlined', () => { const x = f(); g(x); });
        test('written-out', () => { g(f()); });
        test('two-reads', () => { const y = f(); g(y); h(y); });
        test('effect-between', () => { const z = f(); effect(); g(z); });
        test('read-under-branch', () => { const w = f(); if (c) { g(w); } });
    "#,
        &[],
    );
    assert_eq!(of(&events, "inlined"), of(&events, "written-out"));
    assert_ne!(of(&events, "written-out"), of(&events, "two-reads"));
    assert_ne!(of(&events, "written-out"), of(&events, "effect-between"));
    assert_ne!(
        of(&events, "written-out"),
        partial(&events, "read-under-branch")
    );
}

/// A destructured property of an outside object reads as the member access: equal to the body
/// that wrote it, apart when a mutation stands between, and unsubstituted when the object is a
/// body-local.
#[test]
fn a_destructured_outside_property_reads_as_the_member() {
    let events = events(
        r#"
        import { o } from './helpers';
        test('destructured', () => { const { a } = o; use(a); });
        test('member', () => { use(o.a); });
        test('mutation-between', () => { const { b } = o; o.mutate(); use(b); });
        test('local-object', () => { const local = {}; const { c } = local; use(c); });
    "#,
        &[("helpers.ts", "export const o = { mutate() {} }\n")],
    );
    assert_eq!(of(&events, "destructured"), of(&events, "member"));
    assert_ne!(of(&events, "member"), of(&events, "mutation-between"));
    assert_ne!(of(&events, "member"), of(&events, "local-object"));
}

/// A destructured local's rendering carries the whole property path: two bodies reading
/// different properties of the same call never share an event.
#[test]
fn a_destructured_local_carries_its_property_path() {
    let events = events(
        r#"
        test('fetch-user', () => { const { fetchUser } = useApi(); fetchUser(1); });
        test('delete-user', () => { const { deleteUser } = useApi(); deleteUser(1); });
        test('nested-path', () => { const { user: { id } } = t(); use(id); });
        test('other-nested-path', () => { const { post: { id } } = t(); use(id); });
    "#,
        &[],
    );
    assert_ne!(of(&events, "fetch-user"), of(&events, "delete-user"));
    assert_ne!(of(&events, "nested-path"), of(&events, "other-nested-path"));
}

/// A body the parser admits traces whole: the walk's bound is never the reason a comparable
/// body goes uncompared (the incompleteness control itself is the unprovable class above).
#[test]
fn a_deep_body_traces_whole() {
    let deep = |calls: usize| {
        let mut body = "leaf()".to_owned();
        for index in 0..calls {
            body = format!("w{index}({body})");
        }
        format!("test('deep', () => {{ {body}; }});")
    };
    let events = events(&deep(100), &[]);
    assert!(
        complete(&events, "deep"),
        "a body the parser admits traces whole: {:?}",
        events.get("deep")
    );
}

/// The substance a rule floors on is observable: a bare assertion records one event, a setup
/// plus an assertion records two.
#[test]
fn substance_is_observable() {
    let events = events(
        r#"
        test('one', () => { expect(a).toBe(1); });
        test('two', () => { setup(); expect(a).toBe(1); });
    "#,
        &[],
    );
    assert_eq!(of(&events, "one").len(), 1);
    assert_eq!(of(&events, "two").len(), 2);
}

/// A parenthesized or type-wrapped callback traces as the function it wraps: its events and its
/// form are the plain spelling's.
#[test]
fn a_wrapped_callback_traces_as_the_function_it_wraps() {
    let events = events(
        r#"
        test('plain', async () => { g(); });
        test('paren', (async () => { g(); }));
        test('paren-other', (async () => { h(); }));
    "#,
        &[],
    );
    assert_eq!(of(&events, "plain"), of(&events, "paren"));
    assert_ne!(of(&events, "paren"), of(&events, "paren-other"));
}

/// An inlined local reads as its initializer in a callee position too: the extracted and the
/// written-out calls spell the same path.
#[test]
fn an_inlined_local_expands_in_a_callee_path() {
    let events = events(
        r#"
        test('extracted-call', () => { const f = getFn(); f(); });
        test('written-call', () => { getFn()(); });
        test('extracted-object', () => { const o = getObj(); o.a(); });
        test('written-object', () => { getObj().a(); });
    "#,
        &[],
    );
    assert_eq!(of(&events, "extracted-call"), of(&events, "written-call"));
    assert_eq!(
        of(&events, "extracted-object"),
        of(&events, "written-object")
    );
}

/// A spread subject renders: two assertions over different spreads stay apart.
#[test]
fn an_assertion_subject_renders_its_spread() {
    let events = events(
        r#"
        test('a', () => { expect(...a).toBe(1); });
        test('b', () => { expect(...b).toBe(1); });
    "#,
        &[],
    );
    assert_ne!(of(&events, "a"), of(&events, "b"));
}

/// Substituting an initializer for its read must not move it past a declaration it reads: the
/// crash-order and run-order bodies stay apart, while a pure declaration between still inlines.
#[test]
fn inlining_never_moves_a_read_past_its_declaration() {
    let events = events(
        r#"
        test('crash-order', () => { const x = f(z); let z = 1; g(x); });
        test('run-order', () => { let z = 1; const x = f(z); g(x); });
        test('pure-before', () => { const y = f(); let w = 1; g(y); });
        test('pure-after', () => { let w = 1; const y = f(); g(y); });
    "#,
        &[],
    );
    assert_ne!(of(&events, "crash-order"), of(&events, "run-order"));
    assert_eq!(of(&events, "pure-before"), of(&events, "pure-after"));
}

/// A read that only a condition evaluates is never inlined: moving the initializer there would
/// change how often it runs. The short-circuit twin stays apart.
#[test]
fn a_read_under_a_condition_is_never_inlined() {
    let events = events(
        r#"
        test('extracted', () => { const x = f(); c && g(x); });
        test('written', () => { c && g(f()); });
    "#,
        &[],
    );
    assert_ne!(partial(&events, "extracted"), partial(&events, "written"));
}

/// An unbraced control-flow body is a statement: its calls are behavior, and the braced spelling
/// is the same sequence.
#[test]
fn an_unbraced_body_keeps_its_behavior() {
    let events = events(
        r#"
        test('unbraced-a', () => { if (c) foo(); });
        test('unbraced-b', () => { if (c) bar(); });
        test('braced-a', () => { if (c) { foo(); } });
    "#,
        &[],
    );
    assert_ne!(
        partial(&events, "unbraced-a"),
        partial(&events, "unbraced-b")
    );
    assert_eq!(partial(&events, "unbraced-a"), partial(&events, "braced-a"));
}

/// A constructor event keeps its ordered arguments: two constructions over different literals
/// never share an event.
#[test]
fn a_constructor_event_keeps_its_arguments() {
    let events = events(
        r#"
        test('a', () => { new URL('/a', base); });
        test('b', () => { new URL('/b', base); });
        test('twin', () => { new URL('/a', base); });
    "#,
        &[],
    );
    assert_ne!(of(&events, "a"), of(&events, "b"));
    assert_eq!(of(&events, "a"), of(&events, "twin"));
}

/// A static block's statements are definition-time behavior, read from the list they are.
#[test]
fn a_static_block_keeps_its_statements() {
    let events = events(
        r#"
        test('a', () => { class C { static { g(); } } new C(); });
        test('b', () => { class C { static { h(); } } new C(); });
    "#,
        &[],
    );
    assert_ne!(of(&events, "a"), of(&events, "b"));
}

/// A spread runs the iterator protocol even on an inert operand: an initializer does not move
/// across one, so the two orders stay apart.
#[test]
fn a_spread_blocks_inlining_across_it() {
    let events = events(
        r#"
        test('before', () => { const x = f(); const a = [...items]; g(x); });
        test('after', () => { const a = [...items]; const x = f(); g(x); });
    "#,
        &[],
    );
    assert_ne!(partial(&events, "before"), partial(&events, "after"));
}

/// A computed terminal link reads a value, not an assertion name: it is no chain event, so the
/// dynamic member and the literal member stay apart.
#[test]
fn a_computed_terminal_link_is_no_chain_event() {
    let events = events(
        r#"
        test('computed', () => { expect(false)[ok]; });
        test('literal', () => { expect(false).ok; });
    "#,
        &[],
    );
    assert_ne!(of(&events, "computed"), of(&events, "literal"));
    assert!(
        !of(&events, "computed")
            .iter()
            .any(|event| event.starts_with("assert:chain:")),
        "the computed member records no chain event: {:?}",
        of(&events, "computed")
    );
}

/// A bigint, a regexp and null are three different assertion subjects.
#[test]
fn structural_subjects_keep_bigint_and_regexp_kinds() {
    let events = events(
        r#"
        test('bigint', () => { expect(1n).toBeTruthy(); });
        test('regexp', () => { expect(/a/).toBeTruthy(); });
        test('null', () => { expect(null).toBeTruthy(); });
    "#,
        &[],
    );
    assert_ne!(of(&events, "bigint"), of(&events, "null"));
    assert_ne!(of(&events, "regexp"), of(&events, "null"));
}

/// An effectful initializer is not absorbed through another inlined declaration: the composed
/// chain keeps its own events instead of folding away. Pinned whole, because the property is the
/// trace, not one event in it.
#[test]
fn an_effectful_initializer_is_not_absorbed_through_another_inlining() {
    let events = events(
        r#"
        test('composed', () => { effect(); const x = g(); const y = x(); h(y); });
    "#,
        &[],
    );
    // The absorbed candidate is pruned: its initializer stays an event of its own, and the read
    // site names the local instead of folding the initializer into itself.
    assert_eq!(
        of(&events, "composed"),
        [
            "call:g:effect",
            "call:g:g",
            "call:g:h args=[\"(CallExpression arguments:[] callee:$0 optional:false \
             typeArguments:null)\"]",
            "call:$0",
        ]
    );
}

/// A generator continuation's body never runs when called: it is not the awaited spelling's
/// twin, and no splice claims it is.
#[test]
fn a_generator_continuation_is_never_spliced() {
    let events = events(
        r#"
        test('spliced?', () => { return f().then(function* () { g(); }); });
        test('deferred', () => { return f().then(function* () { g(); }); h(); });
    "#,
        &[],
    );
    // The generator body's events stay deferred, and no await marker is spliced in.
    let spliced = of(&events, "spliced?");
    assert!(
        !spliced.iter().any(|event| event == "await"),
        "no await marker without execution: {spliced:?}"
    );
    assert!(
        !spliced.iter().any(|event| event == "call:g:g"),
        "the generator body never ran: {spliced:?}"
    );
    assert!(
        spliced
            .iter()
            .any(|event| event.starts_with("deferred-call:g:g")),
        "the body stays deferred: {spliced:?}"
    );
}

/// An effect that evaluates before the read, inside the read's own statement, blocks inlining:
/// moving the initializer there reverses two observable evaluations.
#[test]
fn an_effect_before_the_read_within_its_statement_blocks_inlining() {
    let events = events(
        r#"
        test('extracted', () => { const x = mark(1); consume(mark(2), x); });
        test('written', () => { consume(mark(2), mark(1)); });
    "#,
        &[],
    );
    assert_ne!(of(&events, "extracted"), of(&events, "written"));
}

/// A computed object key evaluates when the literal does: its events are in the sequence, so the
/// two orders stay apart and an initializer does not move across one.
#[test]
fn a_computed_key_is_behavior_and_blocks_inlining() {
    let events = events(
        r#"
        test('before', () => { const x = mark(1); const o = { [mark(2)]: 1 }; g(x); });
        test('after', () => { const o = { [mark(2)]: 1 }; const x = mark(1); g(x); });
        test('twin', () => { const x = mark(1); const o = { [mark(2)]: 1 }; g(x); });
    "#,
        &[],
    );
    assert_eq!(of(&events, "before"), of(&events, "twin"));
    assert_ne!(of(&events, "before"), of(&events, "after"));
}

/// A class a shape names renders by its content: two constructions over different classes never
/// share an assertion's shape, while a renamed copy still links.
#[test]
fn a_class_inside_a_shape_renders_by_content() {
    let events = events(
        r#"
        test('a', () => { class C { m() { return success(); } } expect(new C().m()).toBe(1); });
        test('b', () => { class C { m() { return failure(); } } expect(new C().m()).toBe(1); });
        test('twin', () => { class D { m() { return success(); } } expect(new D().m()).toBe(1); });
    "#,
        &[],
    );
    assert_ne!(of(&events, "a"), of(&events, "b"));
    assert_eq!(of(&events, "a"), of(&events, "twin"));
}

/// A member chain evaluates from its object inward, and a computed key after the object it reads
/// from: `object()[key()]` runs `object()` first.
#[test]
fn a_member_chain_keeps_its_evaluation_order() {
    let events = events(
        r#"
        test('order', () => { object()[key()](); });
        test('swapped', () => { key(); object(); call(); });
    "#,
        &[],
    );
    assert_ne!(of(&events, "order"), of(&events, "swapped"));
    let order = of(&events, "order");
    let object_at = order
        .iter()
        .position(|event| event == "call:g:object")
        .unwrap();
    let key_at = order
        .iter()
        .position(|event| event == "call:g:key")
        .unwrap();
    assert!(object_at < key_at, "object() runs first: {order:?}");
}

/// Debug probe: the dataflow maps for the declining member fixture. Deleted before delivery.
#[test]
fn debug_probe_dataflow() {
    use crate::lang::parse;
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let main = "import { o } from './helpers';\ntest('t', () => { const { b } = o; use(o.reset(), b); });\n";
    let path = dir.path().join("a.test.tsx");
    fs::write(&path, main).unwrap();
    let parsed = parse(&path, main).unwrap();
    let d = crate::behavior::dataflow::analyze(&parsed, {
        // the test callback's function node: the last argument of the registration
        let tree = &parsed.tree;
        let root = tree.root();
        tree.descendants(root)
            .find(|&n| tree.kind(n) == "ArrowFunctionExpression")
            .unwrap()
    });
    println!("inline: {:?}", d.inline.keys().collect::<Vec<_>>());
    println!("member: {:?}", d.member);
    println!("names: {:?}", d.names);
}

/// A `function` continuation that reads its own `this` or `arguments` is never spliced: the
/// re-homed reads would name the test body's, not the continuation's.
#[test]
fn a_function_continuation_reading_this_is_never_spliced() {
    let events = events(
        r#"
        test('reads-arguments', () => { return f().then(function () { g(arguments[0]); }); });
        test('plain', () => { return f().then(function (v) { g(v); }); });
    "#,
        &[],
    );
    let reads = of(&events, "reads-arguments");
    assert!(
        !reads.iter().any(|event| event == "await"),
        "no splice when the continuation reads arguments: {reads:?}"
    );
    assert!(
        reads
            .iter()
            .any(|event| event.starts_with("deferred-call:g:g")),
        "the body stays deferred: {reads:?}"
    );
    // The parameterized spelling is rejected by its own precondition; both stay unspliced.
    assert!(
        !of(&events, "plain").iter().any(|event| event == "await"),
        "a parameterized continuation is never spliced either"
    );
}

/// The member rewrite moves the object's read to every use, so nothing effectful may stand
/// before a use, inside its own statement either: the rewrite declines to a name tag, and the
/// rewritten spelling stays apart.
#[test]
fn an_effect_before_the_use_blocks_the_member_rewrite() {
    let events = events(
        r#"
        import { o } from './helpers';
        test('extracted', () => { const { b } = o; use(o.reset(), b); });
        test('written', () => { use(o.reset(), o.b); });
    "#,
        &[("helpers.ts", "export const o = { reset() {} }\n")],
    );
    assert_ne!(of(&events, "extracted"), of(&events, "written"));
}

/// Statement families the earlier controls did not exercise: every loop kind, `try` with its
/// handler and finalizer, `switch`, `throw`, and a non-chain `return` with its argument.
#[test]
fn statement_families_all_record() {
    let events = events(
        r#"
        test('loops', () => {
          for (f(); i < 2; i++) { g(); }
          for (const k in o) { g(); }
          let n = 0;
          while (n < 1) { g(); n++; }
          do { g(); } while (n < 1);
        });
        test('try', () => { try { g(); } catch (e) { h(); } finally { i(); } });
        test('switch', () => { switch (v) { case 1: g(); break; default: h(); } });
        test('throw-return', () => { if (v) { throw e; } return v; });
    "#,
        &[],
    );
    assert!(of(&events, "loops").iter().any(|event| event == "loop"));
    assert!(partial(&events, "try").iter().any(|event| event == "try"));
    assert!(partial(&events, "try")
        .iter()
        .any(|event| event == "call:g:h"));
    assert!(partial(&events, "try")
        .iter()
        .any(|event| event == "call:g:i"));
    assert!(partial(&events, "switch")
        .iter()
        .any(|event| event == "call:g:g"));
    assert!(partial(&events, "switch")
        .iter()
        .any(|event| event == "call:g:h"));
    let throw_return = partial(&events, "throw-return");
    assert!(throw_return.iter().any(|event| event == "throw"));
    assert!(throw_return.iter().any(|event| event == "return"));
}

/// A do-while records its marker even when it is the only loop in the body.
#[test]
fn a_do_while_records_its_marker() {
    let events = events(
        r#"
        test('dow', () => { do { g(); } while (c); });
    "#,
        &[],
    );
    assert!(of(&events, "dow").iter().any(|event| event == "loop"));
}

/// A function declaration in a guarded statement position walks as deferred behavior.
#[test]
fn a_guarded_function_declaration_walks_deferred() {
    let events = events(
        r#"
        test('guarded', () => { if (c) function f() { g(); } f(); });
    "#,
        &[],
    );
    assert!(
        partial(&events, "guarded")
            .iter()
            .any(|event| event == "deferred-call:g:g"),
        "the declaration's body is deferred: {:?}",
        partial(&events, "guarded")
    );
}

/// A `for` head that is not a declaration still walks its initializer expression.
#[test]
fn a_for_head_expression_is_walked() {
    let events = events(
        r#"
        test('expr-init', () => { for (f(); i < 2; i++) { g(); } });
    "#,
        &[],
    );
    let evts = of(&events, "expr-init");
    assert!(evts.iter().any(|event| event == "call:g:f"));
    assert!(evts.iter().any(|event| event == "loop"));
}

/// Deferred state reaches every statement family inside a nested declaration, not only calls.
#[test]
fn deferred_reaches_the_statement_families() {
    let events = events(
        r#"
        test('nested', () => {
          function later() {
            while (n) { g(); }
            try { h(); } finally { i(); }
            throw e;
            return v;
          }
        });
    "#,
        &[],
    );
    let evts = partial(&events, "nested");
    assert!(evts.iter().any(|event| event == "deferred-loop"));
    assert!(evts.iter().any(|event| event == "deferred-try"));
    assert!(evts.iter().any(|event| event == "deferred-call:g:h"));
    assert!(evts.iter().any(|event| event == "deferred-throw"));
    assert!(evts.iter().any(|event| event == "deferred-return"));
}

/// The `.then` splice declines each remaining precondition, and the chain then reads as an
/// ordinary call over a deferred callback.
#[test]
fn then_splice_declines_its_remaining_preconditions() {
    let events = events(
        r#"
        test('computed-then', () => { return f()['then'](() => { g(); }); });
        test('not-then', () => { return f().catch(() => { g(); }); });
        test('inner-not-call', () => { return p.then(() => { g(); }); });
        test('chained', () => { return f().then(() => {}).then(() => { g(); }); });
        test('zero-args', () => { return f().then(); });
        test('non-function', () => { return f().then(1); });
        test('reads-this', () => { return f().then(function () { g(this); }); });
    "#,
        &[],
    );
    for title in [
        "computed-then",
        "not-then",
        "inner-not-call",
        "chained",
        "zero-args",
        "non-function",
        "reads-this",
    ] {
        let evts = of(&events, title);
        assert!(
            !evts.iter().any(|event| event == "await"),
            "{title} is never spliced: {evts:?}"
        );
    }
    // Each declined shape still records the callback's body as deferred behavior.
    for title in ["not-then", "inner-not-call", "chained", "reads-this"] {
        let evts = of(&events, title);
        assert!(
            evts.iter()
                .any(|event| event.starts_with("deferred-call:g:g")),
            "{title} keeps its deferred body: {evts:?}"
        );
    }
}

/// Expression families in statement position: optional chains, unary, conditional, sequences,
/// templates, wrapped expressions, and a computed object key with a call in it.
#[test]
fn expression_families_all_record() {
    let events = events(
        r#"
        test('chains', async () => { await g?.(x); a?.b; });
        test('operators', () => { !x; c ? h() : i(); (1, 2, g()); });
        test('template', () => { const s = `t${x}`; g(s); });
        test('wrapped', () => { (g()); const v = x as unknown as number; w!.p; });
        test('object', () => { const o = { [k()]: v(), plain: 1 }; g(o); });
        test('nested-fn', () => { const h = () => { g(); }; return h; });
    "#,
        &[],
    );
    let chains = of(&events, "chains");
    assert!(chains.iter().any(|event| event == "await"));
    assert!(
        chains.iter().any(|event| event.starts_with("call:g:g")),
        "the optional call spells its dotted path: {chains:?}"
    );
    let operators = partial(&events, "operators");
    assert!(operators.iter().any(|event| event == "call:g:h"));
    assert!(operators.iter().any(|event| event == "call:g:i"));
    assert!(operators.iter().any(|event| event == "call:g:g"));
    assert!(of(&events, "object")
        .iter()
        .any(|event| event == "call:g:k"));
    assert!(of(&events, "object")
        .iter()
        .any(|event| event == "call:g:v"));
    assert!(of(&events, "nested-fn")
        .iter()
        .any(|event| event == "deferred-call:g:g"));
}

/// Callee-path arms: a computed member callee, a private method, `this`, a fluent chain with
/// type arguments, and an optional-call chain.
#[test]
fn callee_paths_all_spell_their_path() {
    let events = events(
        r#"
        test('computed-callee', () => { obj[k](); });
        test('private-callee', () => { class C { #m() {} run() { this.#m(); } } new C().run(); });
        test('fluent-types', () => { expectTypeOf<A>().toEqualTypeOf<B>(); });
        test('optional-callee', () => { obj?.run(); });
    "#,
        &[],
    );
    let computed = of(&events, "computed-callee");
    assert!(
        computed.iter().any(|event| event == "call:g:obj[g:k]"),
        "a computed callee retains its key: {computed:?}"
    );
    let fluent = of(&events, "fluent-types");
    assert!(
        fluent
            .iter()
            .any(|event| event.starts_with("call:g:expectTypeOf")),
        "the fluent path carries the outer event: {fluent:?}"
    );
    let optional = of(&events, "optional-callee");
    assert!(
        optional
            .iter()
            .any(|event| event.starts_with("call:g:obj.run")),
        "an optional call spells the dotted path: {optional:?}"
    );
}

/// Assertion-recognizer arms: a computed matcher, an unknown link, type arguments on both
/// halves, a missing subject, and a spread subject.
#[test]
fn assertion_recognizer_declines_and_spells() {
    let events = events(
        r#"
        test('computed-matcher', () => { expect(x)[mode](1); });
        test('unknown-link', () => { expect.other(x).toBe(1); });
        test('typed', () => { expect<string>(x).toBe(1); });
        test('no-subject', () => { expect(); });
        test('spread-subject', () => { expect(...xs).toBe(1); });
    "#,
        &[],
    );
    let computed = of(&events, "computed-matcher");
    assert!(
        !computed.iter().any(|event| event.starts_with("assert:")),
        "a computed matcher is an ordinary call: {computed:?}"
    );
    let unknown = of(&events, "unknown-link");
    assert!(
        !unknown.iter().any(|event| event.starts_with("assert:")),
        "an unknown link is not an assertion: {unknown:?}"
    );
    let typed = of(&events, "typed");
    assert!(
        typed.iter().any(|event| event.starts_with("assert:expect")),
        "the typed spelling still recognizes: {typed:?}"
    );
    assert!(
        typed.iter().any(|event| event.contains("subject=")),
        "the subject rides the event: {typed:?}"
    );
}

/// A soft or poll qualifier reaches the chain form too, with its links verbatim.
#[test]
fn qualified_chains_record_verbatim() {
    let events = events(
        r#"
        test('soft-chain', () => { expect.soft(x).to.be.ok; });
        test('poll-chain', () => { expect.poll(x).to.be.ok; });
        test('plain-chain', () => { expect(x).to.be.ok; });
    "#,
        &[],
    );
    assert!(of(&events, "soft-chain")
        .iter()
        .any(|event| event == "assert:chain:expect.soft.to.be.ok"));
    assert!(of(&events, "poll-chain")
        .iter()
        .any(|event| event == "assert:chain:expect.poll.to.be.ok"));
    assert!(of(&events, "plain-chain")
        .iter()
        .any(|event| event == "assert:chain:expect.to.be.ok"));
}

/// The Trace accessors answer over a recorded trace; the corpus rules read them later.
#[test]
fn trace_accessors_answer() {
    let evts = events(
        r#"
        test('two', () => { g(); h(); });
    "#,
        &[],
    );
    let (events, complete, _) = &evts["two"];
    let trace = Trace {
        events: events.iter().map(|event| sha256_128(event)).collect(),
        complete: *complete,
    };
    assert_eq!(trace.len(), 2, "the trace counts every event");
    assert!(!trace.is_empty());
    assert!(Trace::default().is_empty(), "the default trace is empty");
}

/// Destructuring forms that carry no provable rewrite keep their name tags or plain numbers:
/// array patterns contribute position, defaults and computed keys are unsupported shapes, and a
/// rest names no property.
#[test]
fn unrewritable_destructuring_forms_stay_named() {
    let events = events(
        r#"
        import { o, pair } from './helpers';
        test('array-pattern', () => { const [x] = pair; use(x); });
        test('default', () => { const { a = 1 } = o; use(a); });
        test('rest', () => { const { ...r } = o; use(r); });
        test('computed-key', () => { const { [k]: v } = o; use(v); });
        test('for-of-head', () => { for (const x of pair) { use(x); } });
        test('catch-param', () => { try { g(); } catch (e) { use(e); } });
    "#,
        &[(
            "helpers.ts",
            "export const o = {}\nexport const pair = [1, 2]\n",
        )],
    );
    let array_pattern = of(&events, "array-pattern");
    assert!(
        array_pattern
            .iter()
            .any(|event| event.starts_with("call:g:use args=")),
        "the array pattern's read stays an ordinary event: {array_pattern:?}"
    );
    // Unsupported substitutions keep their names; the catch partition is incomplete.
    for title in [
        "array-pattern",
        "default",
        "rest",
        "computed-key",
        "for-of-head",
        "catch-param",
    ] {
        assert_eq!(complete(&events, title), title != "catch-param", "{title}");
    }
}

/// The effect scan reaches every expression family through the statements it screens, so an
/// initializer does not move across a hole, a spread property, a computed read, or a wrapped
/// call.
#[test]
fn effect_forms_block_inlining() {
    let events = events(
        r#"
        test('hole', () => { const x = f(); const a = [,]; g(x); });
        test('spread-prop', () => { const x = f(); const a = { ...r }; g(x); });
        test('computed-read', () => { const x = f(); const a = o[k()]; g(x); });
        test('unary', () => { const x = f(); const a = !eff(); g(x); });
        test('conditional', () => { const x = f(); const a = c ? eff1() : eff2(); g(x); });
        test('wrapped', () => { const x = f(); const a = eff() as unknown; g(x); });
        test('import-call', () => { const x = f(); const a = import('./x'); g(x); });
    "#,
        &[],
    );
    for title in [
        "hole",
        "spread-prop",
        "computed-read",
        "unary",
        "conditional",
        "wrapped",
        "import-call",
    ] {
        let evts = if ["spread-prop", "conditional", "import-call"].contains(&title) {
            partial(&events, title)
        } else {
            of(&events, title)
        };
        assert!(
            evts.iter().any(|event| event == "call:g:f"),
            "{title} keeps its initializer's event in place: {evts:?}"
        );
    }
}

/// Class and callee paths the earlier controls missed: a static accessor, `super` in a method,
/// a shadowed assertion entry, an `import()` and a tagged template.
#[test]
fn remaining_class_and_callee_paths_record() {
    let events = events(
        r#"
        test('accessor', () => { class C { static accessor x = 1; } new C(); });
        test('super', () => { class D extends B { run() { super.run(); } } new D().run(); });
        test('shadowed-expect', () => { const expect = (v) => ({ toBe: () => {} }); expect(1).toBe(2); });
        test('dynamic-import', () => { import('./x'); });
        test('tagged', () => { tag`t${x}`; });
    "#,
        &[],
    );
    for title in [
        "accessor",
        "super",
        "shadowed-expect",
        "dynamic-import",
        "tagged",
    ] {
        assert_eq!(
            complete(&events, title),
            !["dynamic-import", "tagged"].contains(&title),
            "{title}"
        );
    }
    let accessor = of(&events, "accessor");
    assert!(
        accessor.iter().any(|event| event.contains("static-field")),
        "the static accessor is a definition-time field: {accessor:?}"
    );
    // A method body is inert: `super.run()` shows up only inside the class content the shapes
    // carry, while the definition-time heritage is its own event.
    let super_call = of(&events, "super");
    assert!(
        super_call
            .iter()
            .any(|event| event.starts_with("class-eval:") && event.contains("heritage:g:B")),
        "the heritage is a definition-time event: {super_call:?}"
    );
    let shadowed = of(&events, "shadowed-expect");
    assert!(
        !shadowed.iter().any(|event| event.starts_with("assert:")),
        "a body-local shadowing the entry is not the API: {shadowed:?}"
    );
    let tagged = partial(&events, "tagged");
    assert!(
        tagged.is_empty(),
        "a tagged template records no event and must refuse comparison: {tagged:?}"
    );
}

/// The call form with a qualifier records its modifiers and values like the plain entry.
#[test]
fn qualified_call_assertions_record() {
    let events = events(
        r#"
        test('soft-call', () => { expect.soft(x).toBe(1); });
        test('poll-not', () => { expect.poll(x).not.toBe(1); });
    "#,
        &[],
    );
    assert!(of(&events, "soft-call")
        .iter()
        .any(|event| event.starts_with("assert:expect.soft.toBe(")));
    assert!(of(&events, "poll-not")
        .iter()
        .any(|event| event.starts_with("assert:expect.poll.not.toBe(")));
}

/// Concise arrow callbacks splice and walk like their block spellings: the body is what the
/// test returns.
#[test]
fn concise_callbacks_splice_and_walk() {
    let events = events(
        r#"
        test('concise-splice', async () => f().then(() => { g(); }));
        test('awaited', async () => { await f(); g(); });
        test('concise-plain', async () => g());
    "#,
        &[],
    );
    assert_eq!(of(&events, "concise-splice"), of(&events, "awaited"));
    let plain = of(&events, "concise-plain");
    assert!(plain.iter().any(|event| event == "call:g:g"), "{plain:?}");
}

/// Statement shapes the earlier fixtures missed: an empty `for`, a `try` without `catch`, a
/// bare `return`, a labeled loop, a declaration `for` head, and an empty statement or bare
/// block in the scanned region.
#[test]
fn remaining_statement_shapes_record() {
    let events = events(
        r#"
        test('empty-for', () => { for (;;) { break; } g(); });
        test('finally-only', () => { try { g(); } finally { h(); } });
        test('bare-return', () => { if (c) { return; } g(); });
        test('labeled', () => { outer: while (c) { break outer; } g(); });
        test('decl-for', () => { for (let i = 0, j = f(); i < 1; i++) { g(j); } });
        test('empties', () => { ; { g(); } });
    "#,
        &[],
    );
    for title in [
        "empty-for",
        "finally-only",
        "bare-return",
        "labeled",
        "decl-for",
        "empties",
    ] {
        assert_eq!(
            complete(&events, title),
            !["finally-only", "bare-return"].contains(&title),
            "{title}"
        );
    }
    assert!(
        of(&events, "labeled").iter().any(|event| event == "loop"),
        "the labeled body's loop is walked"
    );
    let decl_for = of(&events, "decl-for");
    assert!(
        decl_for.iter().any(|event| event == "call:g:f"),
        "the declaration head's initializer is walked: {decl_for:?}"
    );
    assert!(partial(&events, "finally-only")
        .iter()
        .any(|event| event == "call:g:h"));
}

/// Type arguments on a constructor, a class bound to nothing usable, an imported class, and a
/// member-claimed callee all spell their paths.
#[test]
fn constructor_and_callee_paths_record() {
    let events = events(
        r#"
        import { o, Factory } from './helpers';
        test('typed-new', () => { new C<string>(); });
        test('uninitialized', () => { var A; new A(); });
        test('imported-class', () => { new Factory(); });
        test('member-callee', () => { const { run } = o; run(); });
        test('this-callee', () => { function bound() { this.run(); } });
    "#,
        &[(
            "helpers.ts",
            "export const o = { run() {} }\nexport class Factory {}\n",
        )],
    );
    for title in [
        "typed-new",
        "uninitialized",
        "imported-class",
        "member-callee",
        "this-callee",
    ] {
        assert!(complete(&events, title), "{title} traces whole");
    }
    let typed = of(&events, "typed-new");
    assert!(
        typed.iter().any(|event| event.starts_with("new:")),
        "the typed construction records: {typed:?}"
    );
    let member = of(&events, "member-callee");
    assert!(
        member
            .iter()
            .any(|event| event.starts_with("call:var:") && event.contains(".run")),
        "the member-claimed callee spells object.property: {member:?}"
    );
}

/// Assertion recognizers decline their last arms: an unknown modifier link, a subjectless
/// assertion, matcher type arguments, a chain hung off a call, and a chain off nothing
/// recognized.
#[test]
fn assertion_declines_reach_every_arm() {
    let events = events(
        r#"
        test('unknown-modifier', () => { expect(x).foo.bar(1); });
        test('subjectless', () => { expect().toBe(1); });
        test('matcher-typed', () => { expect(x).toBe<number>(1); });
        test('call-base', () => { expect(1)(2).toBe(3); });
        test('not-expect', () => { wrapper.to.be.ok; });
    "#,
        &[],
    );
    for title in [
        "unknown-modifier",
        "subjectless",
        "matcher-typed",
        "call-base",
        "not-expect",
    ] {
        assert!(complete(&events, title), "{title} traces whole");
    }
    for title in ["unknown-modifier", "call-base", "not-expect"] {
        let evts = of(&events, title);
        assert!(
            !evts.iter().any(|event| event.starts_with("assert:")),
            "{title} is no assertion: {evts:?}"
        );
    }
    assert!(of(&events, "subjectless")
        .iter()
        .any(|event| event.starts_with("assert:expect.toBe(subject=()")));
    assert!(of(&events, "matcher-typed")
        .iter()
        .any(|event| event.starts_with("assert:expect.toBe(")));
}

/// Destructuring shapes that reach the remaining pattern arms: quoted and nested keys, an
/// object inside an array, a defaulted element, a key walking to an array, and a never-read
/// binding.
#[test]
fn exotic_destructuring_patterns_record() {
    let events = events(
        r#"
        import { o, xs } from './helpers';
        test('quoted-key', () => { const { 'x-y': v } = o; use(v); });
        test('nested-under-key', () => { const { a: { b } } = o; use(b); });
        test('object-in-array', () => { const [{ a }] = xs; use(a); });
        test('defaulted-element', () => { const [x = 1] = xs; use(x); });
        test('array-under-key', () => { const { a: [b] } = o; use(b); });
        test('never-read', () => { const { unused } = o; g(); });
        test('unsafe-read', () => { const { a } = o; if (c) { use(a); } });
        test('non-const', () => { let x = f(); use(x); });
        test('multi-declarator', () => { const y = f(), z = g(); use(y); });
    "#,
        &[(
            "helpers.ts",
            "export const o = { a: { b: 1 } }\nexport const xs = [{ a: 1 }]\n",
        )],
    );
    for title in [
        "quoted-key",
        "nested-under-key",
        "object-in-array",
        "defaulted-element",
        "array-under-key",
        "never-read",
        "unsafe-read",
        "non-const",
        "multi-declarator",
    ] {
        assert_eq!(complete(&events, title), title != "unsafe-read", "{title}");
    }
}

/// A class whose method runs a `with` statement cannot be proven: the trace abstains.
#[test]
fn a_class_with_with_statement_abstains() {
    let events = events(
        r#"
        test('with-body', () => { class C { m() { with (o) { g(); } } } new C(); });
    "#,
        &[("helpers.ts", "export const o = {}\n")],
    );
    assert!(
        !complete(&events, "with-body"),
        "a `with` in a class is unprovable"
    );
}

/// A plain zero-parameter `function` continuation splices like an arrow: it reads neither the
/// resolved value nor its own frame.
#[test]
fn a_plain_function_continuation_splices() {
    let events = events(
        r#"
        test('function-continuation', async () => { return f().then(function () { g(); }); });
        test('awaited', async () => { await f(); g(); });
    "#,
        &[],
    );
    assert_eq!(of(&events, "function-continuation"), of(&events, "awaited"));
}

/// Callee and chain shapes at the recognizer's edges: a function declaration as a guarded
/// statement, a chain call over an optional chain, and a non-null assertion inside a chain.
#[test]
fn edge_callee_and_chain_shapes_trace() {
    let events = events(
        r#"
        test('guarded-function', () => { if (c) { function f() { g(); } } f(); });
        test('chain-in-callee', () => { x?.y().z(); });
        test('nonnull-in-chain', () => { v?.w!.p; });
    "#,
        &[],
    );
    for title in ["guarded-function", "chain-in-callee", "nonnull-in-chain"] {
        assert_eq!(
            complete(&events, title),
            title != "guarded-function",
            "{title}"
        );
    }
    let guarded = partial(&events, "guarded-function");
    assert!(
        guarded.iter().any(|event| event == "deferred-call:g:g"),
        "the inner function's body is deferred behavior: {guarded:?}"
    );
}
// Prepared mutation kills for the survivors of run 4 (see /tmp/nc76-missed.txt).
// Landed after the clean re-run records the floor; they can only raise the score.

/// A binary or logical expression's operands are walked: `a === b && g()` records g.
#[test]
fn binary_and_logical_operands_are_walked() {
    let events = events(
        r#"
        test('binary', () => { if (a === b && g()) { h(); } });
        test('logical', () => { const s = a || g(); h(s); });
    "#,
        &[],
    );
    assert!(partial(&events, "binary")
        .iter()
        .any(|event| event.starts_with("call:g:g")));
    assert!(partial(&events, "logical")
        .iter()
        .any(|event| event.starts_with("call:g:g")));
}

/// A unary expression's operand is walked: `!g()` records g.
#[test]
fn a_unary_operand_is_walked() {
    let events = events(
        r#"
        test('unary', () => { const v = !g(); h(v); });
    "#,
        &[],
    );
    assert!(of(&events, "unary")
        .iter()
        .any(|event| event.starts_with("call:g:g")));
}

/// An array literal's elements are walked: `[g()]` records g.
#[test]
fn an_array_element_is_walked() {
    let events = events(
        r#"
        test('array', () => { const s = [g()]; h(s); });
    "#,
        &[],
    );
    assert!(of(&events, "array")
        .iter()
        .any(|event| event.starts_with("call:g:g")));
}

/// A `this`-receiver call inside a deferred body spells `this.run`.
#[test]
fn a_this_callee_spells_this() {
    let events = events(
        r#"
        test('this-callee', () => { function bound() { this.run(); } });
    "#,
        &[],
    );
    let evts = of(&events, "this-callee");
    assert!(
        evts.iter()
            .any(|event| event.starts_with("deferred-call:this.run")),
        "`this` spells its path: {evts:?}"
    );
}

/// A fluent chain's outer event carries the inner call: `expectTypeOf<A>().toEqualTypeOf<B>()`
/// spells the whole path, not a prefix.
#[test]
fn a_fluent_outer_event_carries_the_inner_call() {
    let events = events(
        r#"
        test('fluent', () => { expectTypeOf<A>().toEqualTypeOf<B>(); });
    "#,
        &[],
    );
    let fluent = of(&events, "fluent");
    assert!(
        fluent.iter().any(
            |event| event.starts_with("call:g:expectTypeOf") && event.contains("toEqualTypeOf")
        ),
        "the outer event carries the inner call: {fluent:?}"
    );
}

/// A static block records its effects even when its class is never constructed: the
/// definition-time event is the only record.
#[test]
fn a_static_block_records_without_construction() {
    let events = events(
        r#"
        test('never-constructed', () => { class C { static { g(); } } h(); });
    "#,
        &[],
    );
    let evts = of(&events, "never-constructed");
    assert!(
        evts.iter()
            .any(|event| event.starts_with("class-eval:") && event.contains("static-block")),
        "the static block is a definition-time event: {evts:?}"
    );
    assert!(
        evts.iter().any(|event| event == "call:g:h"),
        "the unrelated call survives: {evts:?}"
    );
}

/// An array walked to under a key tags the whole route: `const { a: [b] } = o` and
/// `const { c: [b] } = o` read different properties of the same object.
#[test]
fn an_array_under_a_key_carries_the_route() {
    let events = events(
        r#"
        import { o } from './helpers';
        test('a-route', () => { const { a: [b] } = o; use(b); });
        test('c-route', () => { const { c: [b] } = o; use(b); });
    "#,
        &[("helpers.ts", "export const o = { a: [1], c: [1] }\n")],
    );
    assert_ne!(of(&events, "a-route"), of(&events, "c-route"));
}

/// Inert statements in the scanned gap stay inert: an initializer moves across an empty
/// statement, a bare block, an inert expression statement, and every inert container form.
#[test]
fn inert_gap_forms_do_not_block_inlining() {
    let events = events(
        r#"
        test('empties', () => { const x = f(); ;; {} g(x); });
        test('empties-written', () => { ;; {} g(f()); });
        test('inert-expr', () => { const x = f(); 1; g(x); });
        test('inert-expr-written', () => { 1; g(f()); });
        test('inert-array', () => { const x = f(); const a = [1, 2]; g(x); });
        test('inert-array-written', () => { const a = [1, 2]; g(f()); });
        test('inert-object', () => { const x = f(); const a = { k: 1 }; g(x); });
        test('inert-object-written', () => { const a = { k: 1 }; g(f()); });
        test('inert-template', () => { const x = f(); const a = `t`; g(x); });
        test('inert-template-written', () => { const a = `t`; g(f()); });
        test('inert-binary', () => { const x = f(); const a = 1 + 2; g(x); });
        test('inert-binary-written', () => { const a = 1 + 2; g(f()); });
        test('inert-unary', () => { const x = f(); const a = -1; g(x); });
        test('inert-unary-written', () => { const a = -1; g(f()); });
        test('inert-conditional', () => { const x = f(); const a = c ? 1 : 2; g(x); });
        test('inert-conditional-written', () => { const a = c ? 1 : 2; g(f()); });
        test('inert-wrapped', () => { const x = f(); const a = 1 as unknown; g(x); });
        test('inert-wrapped-written', () => { const a = 1 as unknown; g(f()); });
        test('inert-member', () => { const x = f(); const a = o.p; g(x); });
        test('inert-member-written', () => { const a = o.p; g(f()); });
    "#,
        &[("helpers.ts", "export const o = { p: 1 }\n")],
    );
    for title in [
        "empties",
        "inert-expr",
        "inert-array",
        "inert-object",
        "inert-template",
        "inert-binary",
        "inert-unary",
        "inert-conditional",
        "inert-wrapped",
        "inert-member",
    ] {
        let written = format!("{title}-written");
        if title == "inert-conditional" {
            assert_eq!(partial(&events, title), partial(&events, &written));
            continue;
        }
        assert_eq!(
            of(&events, title),
            of(&events, &written),
            "{title} moves across the inert gap"
        );
        assert!(complete(&events, title), "{title} traces whole");
    }
}

/// Whether two bodies trace alike.
fn traces_alike(left: &str, right: &str) -> bool {
    let events = events(
        &format!("test('left', () => {{ {left} }}); test('right', () => {{ {right} }});"),
        &[],
    );
    of(&events, "left") == of(&events, "right")
}

/// A retained binding's reads carry what bound it, whatever binds it: they name only the local,
/// and a bare identifier or member initializer emits no event of its own.
#[test]
fn a_retained_binding_keeps_its_source() {
    for body in [
        "use(a); var { a } = SOURCE; use(a);",
        "use(a); const { a } = SOURCE; use(a);",
        "let { a } = SOURCE; use(a);",
        "const { a } = SOURCE; use(a); use(a);",
        "const [a] = SOURCE; use(a); use(a);",
        "const { p: { a } } = SOURCE; use(a); use(a);",
        "const { ...a } = SOURCE; use(a); use(a);",
        "const { a = 1 } = SOURCE; use(a); use(a);",
        "const { a } = SOURCE.p; use(a); use(a);",
        "const a = SOURCE; use(a); use(a);",
        "for (const { a } of SOURCE) { use(a); }",
        "for (const a in SOURCE) { use(a); }",
        "for (let a = SOURCE; a; a--) { use(a); }",
    ] {
        let at = |source: &str| body.replace("SOURCE", source);
        assert!(!traces_alike(&at("first"), &at("second")), "{body}");
        assert!(traces_alike(&at("first"), &at("first")), "{body}");
    }
}

/// The source stays with its own reads, so swapping which is read first is another program.
#[test]
fn a_source_stays_with_its_reads() {
    assert!(!traces_alike(
        "const a = first; const b = second; use(a); use(a); use(b); use(b);",
        "const a = first; const b = second; use(b); use(b); use(a); use(a);"
    ));
    assert!(!traces_alike(
        "const [a, b] = first; use(a); use(a); use(b); use(b);",
        "const [a, b] = first; use(b); use(b); use(a); use(a);"
    ));
}

/// Which local a source, a default or a computed key reads is part of the binding.
#[test]
fn a_binding_of_a_local_names_which_local() {
    let body = |binding: &str| {
        format!("const c = one; const d = two; {binding} use(a); use(a); c.x(); d.y();")
    };
    for (left, right) in [
        ("const a = c;", "const a = d;"),
        ("const { a = c } = first;", "const { a = d } = first;"),
        ("const { [c]: a } = first;", "const { [d]: a } = first;"),
    ] {
        assert!(!traces_alike(&body(left), &body(right)), "{left}");
    }
}

/// What a pattern binds is part of the binding, and how its names are spelled is not.
#[test]
fn a_binding_keeps_its_target_and_not_its_spelling() {
    assert!(!traces_alike(
        "const [a] = first; use(a); use(a);",
        "const [, a] = first; use(a); use(a);"
    ));
    assert!(traces_alike(
        "const { p: a } = first; use(a); use(a);",
        "const { p: b } = first; use(b); use(b);"
    ));
    assert!(traces_alike(
        "for (const a of first) { use(a); }",
        "for (let a of first) { use(a); }"
    ));
    assert!(traces_alike(
        "const a: string = first; use(a); use(a);",
        "const b: string = first; use(b); use(b);"
    ));
}

/// A tag takes no number and records no event, so the locals after a retained one keep theirs and
/// a body of bindings alone stays empty.
#[test]
fn a_tag_leaves_numbering_and_events_alone() {
    let events = events(
        r#"
        test('small', () => { const a = make(); a.x(); a.y(); });
        test('large', () => {
          const z = first; const a = make(); a.x(); a.y(); use(z); use(z);
        });
        test('reading', () => {
          const y = first; const z = y; const a = make(); a.x(); a.y(); use(y); use(z); use(z);
        });
        test('bare', () => { const [a] = first; const { b } = second; });
        test('nested', () => { function unused() { const [a] = first; } });
    "#,
        &[],
    );
    assert_eq!(of(&events, "small"), &of(&events, "large")[..3]);
    assert_eq!(of(&events, "small"), &of(&events, "reading")[..3]);
    assert!(of(&events, "bare").is_empty());
    assert!(of(&events, "nested").is_empty());
}

/// A tag tells reads apart and never joins them: two equal sources keep their own numbers.
#[test]
fn a_tag_never_joins_two_locals() {
    assert!(!traces_alike(
        "const a = {}; const b = {}; use(a); use(b); use(a);",
        "const a = {}; const b = {}; use(a); use(a); use(b);"
    ));
}

/// A substitution already renders the source at the read, and an unread sibling adds nothing, so
/// the body stays its written-out form.
#[test]
fn a_substituted_binding_carries_no_tag() {
    let events = events(
        r#"
        const first = {};
        test('member', () => { const { a } = first; expect(a).toBe(1); });
        test('written member', () => { expect(first.a).toBe(1); });
        test('partly', () => { const { a, b } = first; expect(a).toBe(1); });
        test('alias', () => { const a = first; use(a); });
        test('written alias', () => { use(first); });
    "#,
        &[],
    );
    assert_eq!(of(&events, "member"), of(&events, "written member"));
    assert_eq!(of(&events, "partly"), of(&events, "written member"));
    assert_eq!(of(&events, "alias"), of(&events, "written alias"));
}

/// A literal difference is parameterization's claim, and a source with its own event names itself
/// there, so neither difference reaches every read.
#[test]
fn a_literal_or_a_call_difference_stays_out_of_the_tag() {
    assert!(traces_alike(
        "const a = 1; use(a); use(a);",
        "const a = 2; use(a); use(a);"
    ));
    let events = events(
        r#"
        test('call', () => { const a = make(); use(a); use(a); });
        test('awaited', async () => { const a = await make(); use(a); use(a); });
        test('built', () => { const a = new Make(); use(a); use(a); });
        test('function', () => { const a = () => first(); use(a); use(a); });
        test('left', () => { const [a] = make(first); use(a); use(a); });
        test('right', () => { const [a] = make(second); use(a); use(a); });
    "#,
        &[],
    );
    for title in ["call", "awaited", "built", "function"] {
        let events = of(&events, title);
        assert!(events.iter().all(|event| !event.contains('<')), "{title}");
    }
    assert_eq!(of(&events, "left")[1..], of(&events, "right")[1..]);
    assert_ne!(of(&events, "left")[0], of(&events, "right")[0]);
}

/// A source with its own event still leaves the pattern to the tag: position, order of reads, and
/// what a default or a computed key reads.
#[test]
fn a_pattern_over_a_call_keeps_its_target() {
    for (left, right) in [
        (
            "const [a] = make(); use(a); use(a);",
            "const [, a] = make(); use(a); use(a);",
        ),
        (
            "const [a, b] = make(); use(a); use(a); use(b); use(b);",
            "const [a, b] = make(); use(b); use(b); use(a); use(a);",
        ),
        (
            "for (const [a] of make()) { use(a); }",
            "for (const [, a] of make()) { use(a); }",
        ),
        (
            "const { [first]: a } = make(); use(a); use(a);",
            "const { [second]: a } = make(); use(a); use(a);",
        ),
        (
            "const { a = first } = make(); use(a); use(a);",
            "const { a = second } = make(); use(a); use(a);",
        ),
    ] {
        assert!(!traces_alike(left, right), "{left}");
    }
}

/// A source only partly a call keeps what the call's event does not name.
#[test]
fn a_source_beside_a_call_is_tagged() {
    for body in [
        "const a = await SOURCE; use(a); use(a);",
        "const { a } = await SOURCE; use(a); use(a);",
        "const a = [SOURCE, make()]; use(a); use(a);",
        "const a = { x: SOURCE, y: make() }; use(a); use(a);",
        "const a = make().SOURCE; use(a); use(a);",
    ] {
        let at = |source: &str| body.replace("SOURCE", source);
        assert!(!traces_alike(&at("first"), &at("second")), "{body}");
    }
}

/// A default only reads its local, so that local's own reads carry no tag of the binding.
#[test]
fn a_default_read_is_not_tagged() {
    let events = events(
        "test('t', () => { const c = make(); const { a = c } = first; use(c); use(a); });",
        &[],
    );
    let use_of_c = &of(&events, "t")[1];
    assert!(!use_of_c.contains('<'), "{use_of_c}");
    assert!(of(&events, "t")[2].contains('<'));
}

/// A source reading a local class spends none of the budget the events share, so an alias cannot
/// leave a trace incomplete.
#[test]
fn an_alias_of_a_class_spends_no_budget() {
    let mut body = "class A1 { a() { return 1; } b() { return 2; } }".to_owned();
    for number in 2..=10 {
        let previous = number - 1;
        body.push_str(&format!(
            "class A{number} {{ a() {{ return A{previous}; }} b() {{ return A{previous}; }} }}"
        ));
    }
    let events = events(
        &format!("test('t', () => {{ {body} const alias = A10; expect(1).toBe(1); }});"),
        &[],
    );
    assert!(complete(&events, "t"));
}

/// A wrapper a type or a parenthesis adds changes nothing about the call inside it.
#[test]
fn a_wrapped_call_names_itself_like_a_bare_one() {
    let bare = "const a = make(); use(a); use(a);";
    for wrapped in ["(make())", "make() as unknown", "make()!", "await (make())"] {
        let body = format!("const a = {wrapped}; use(a); use(a);");
        if wrapped.starts_with("await") {
            let async_form = "const a = await make(); use(a); use(a);";
            let events = events(
                &format!(
                    "test('left', async () => {{ {async_form} }}); \
                     test('right', async () => {{ {body} }});"
                ),
                &[],
            );
            assert_eq!(of(&events, "left"), of(&events, "right"));
        } else {
            assert!(traces_alike(bare, &body), "{wrapped}");
        }
    }
}

/// A private name's spelling is not part of a binding read from it.
#[test]
fn a_private_source_keeps_its_spelling_out_of_the_tag() {
    let body = |name: &str| {
        format!(
            "class C {{ #{name} = 1; m() {{ const x = this.#{name}; use(x); use(x); }} }} new C();"
        )
    };
    assert!(traces_alike(&body("a"), &body("b")));
}
