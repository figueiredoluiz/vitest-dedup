//! The trace's claims, as equal, conflicting and uncertain pairs of owned bodies.
//!
//! Every row of the contract's control table lives here: an equal pair's events are equal, a
//! conflicting pair's are not, and an uncertain pair's outcome is a stated abstention rather
//! than a claim. Each mechanism's test names the one edit that would break it, so a revert can
//! be proved by that test failing alone.

use super::trace_text;
use crate::framework::vitest::DIALECT;
use crate::inventory::{inventory, Modules, SiteKind};
use crate::lang::parse;
use crate::normal::{CallbackForm, Exports, Normalizer, Setting, Structure};
use std::collections::HashMap;
use std::fs;

/// The text events of every inline test callback of `main`, by title, in a project holding
/// `files`, with the callback form beside them and whether the trace completed.
fn events(
    main: &str,
    files: &[(&str, &str)],
) -> HashMap<String, (Vec<String>, bool, CallbackForm)> {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    let main = format!("import {{ test }} from 'vitest';\n{main}");
    let path = dir.path().join("a.test.ts");
    fs::write(&path, &main).unwrap();
    let parsed = parse(&path, &main).unwrap();
    let found = inventory(
        &path,
        &main,
        &parsed,
        crate::inventory::Setting {
            dialect: &DIALECT,
            globals: false,
            root: dir.path(),
        },
        &mut Modules::default(),
    );
    let structure = Structure::of(&parsed, &found);
    let exports = Exports::default();
    let setting = Setting {
        dialect: &DIALECT,
        root: dir.path(),
        exports: &exports,
    };
    let mut normalizer = Normalizer::new(&path, &parsed, setting, &structure);
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
            // The trace walks the function itself, not the argument as written: a parenthesized
            // or type-wrapped callback would otherwise read as a body with no statements in it.
            let (events, complete, form) = trace_text(
                &mut normalizer,
                &parsed,
                callback.function.unwrap_or(callback.argument),
                HashMap::new(),
                false,
            );
            Some((title, (events, complete, form)))
        })
        .collect()
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
