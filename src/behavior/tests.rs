//! The trace's claims, as equal, conflicting and uncertain pairs of owned bodies.
//!
//! Every row of the contract's control table lives here: an equal pair's events are equal, a
//! conflicting pair's are not, and an uncertain pair's outcome is a stated abstention rather
//! than a claim. Each mechanism's test names the one edit that would break it, so a revert can
//! be proved by that test failing alone.

use super::trace_text;
use crate::inventory::SiteKind;
use crate::normal::CallbackForm;
use std::collections::HashMap;

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
    "#,
        &[],
    );
    assert_eq!(of(&events, "renamed-copy"), of(&events, "renamed-twin"));
    assert_ne!(
        of(&events, "renamed-copy"),
        of(&events, "different-content")
    );
    assert_eq!(of(&events, "bound-expression"), of(&events, "bound-twin"));
    assert!(!complete(&events, "unprovable"));
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
    assert_ne!(of(&events, "written-out"), of(&events, "read-under-branch"));
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
    assert_ne!(of(&events, "extracted"), of(&events, "written"));
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
    assert_ne!(of(&events, "unbraced-a"), of(&events, "unbraced-b"));
    assert_eq!(of(&events, "unbraced-a"), of(&events, "braced-a"));
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
    assert_ne!(of(&events, "before"), of(&events, "after"));
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
