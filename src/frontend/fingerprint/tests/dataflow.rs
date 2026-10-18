use super::*;

// Dataflow canonicalization: the three `near-duplicate-test` transformations added on top of
// reordered-setup multiset equality. Each has a positive-convergence test, a
// precondition-violating case that must NOT converge, and (once, covering all three) a
// determinism check. See `fixtures/duplicates/near-*-positive.test.ts` and their twins for the
// fixture-level versions of the same claims.
// -----------------------------------------------------------------------------------------

#[test]
fn dataflow_inlines_single_use_local_to_match_inlined_call() {
    let prints = fingerprint_all(
        "import { computeTotal } from './cart';\n\
             test('via local', () => { const cart = [10, 20]; const total = computeTotal(cart); \
                 expect(total).toBe(30); });\n\
             test('inline', () => { const cart = [10, 20]; expect(computeTotal(cart)).toBe(30); });",
    );
    let (a, b) = sorted_behavior_pair(&prints);
    assert_eq!(
        a, b,
        "a local read exactly once must inline to match the equivalent inlined call: {a:?} \
             vs {b:?}"
    );
}

/// The read-site half of single-use inlining, pinned in all three directions -- the boundary
/// `fixtures/observed/observed-single-use-local-erases-behavior.test.ts` distills. Before
/// `walk_inlined_read` existed, only the assertion-subject direction worked and the other two
/// silently dropped the initializer's `call:` events.
#[test]
fn dataflow_inlined_read_outside_an_assertion_keeps_initializer_events() {
    let prints = fingerprint_all(
        "import { buildEmpty, buildFrom } from './builders';\n\
             test('from', () => { const subject = buildFrom(1, 2, 3); const result = \
                 subject.run(); expect(result).toBe(1); expect(result).toEqual(2); });\n\
             test('empty', () => { const subject = buildEmpty(); const result = subject.run(); \
                 expect(result).toBe(1); expect(result).toEqual(2); });",
    );
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    for (index, expected) in [(0usize, "buildFrom"), (1usize, "buildEmpty")] {
        assert!(
            prints[index]
                .behavior
                .iter()
                .any(|event| event.starts_with("call:") && event.contains(expected)),
            "a single-use local read outside an assertion must still account for its \
                 initializer's call event ({expected}): {:?}",
            prints[index].behavior
        );
    }
    let mut a = prints[0].behavior.clone();
    let mut b = prints[1].behavior.clone();
    a.sort();
    b.sort();
    assert_ne!(
        a, b,
        "two tests building genuinely different subjects into a single-use local must not \
             produce equal behavior multisets -- that collision is the `near-duplicate-test` \
             false positive this re-emission exists to close"
    );
}

/// The same shape with the local read TWICE: the read-once gate declines to inline, so the
/// declaration's own event survives and the two tests still differ. This is the "inlining is
/// the trigger, not the bodies" half of the boundary.
#[test]
fn dataflow_local_read_twice_outside_an_assertion_stays_distinct() {
    let prints = fingerprint_all(
        "import { buildEmpty, buildFrom } from './builders';\n\
             test('from', () => { const subject = buildFrom(1, 2, 3); const first = \
                 subject.run(); const second = subject.run(); expect(first).toBe(second); });\n\
             test('empty', () => { const subject = buildEmpty(); const first = subject.run(); \
                 const second = subject.run(); expect(first).toBe(second); });",
    );
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    let mut a = prints[0].behavior.clone();
    let mut b = prints[1].behavior.clone();
    a.sort();
    b.sort();
    assert_ne!(a, b, "{a:?} vs {b:?}");
}

/// The assertion-SUBJECT direction, which `shape` already covered: it must keep working, and
/// it must not acquire a duplicate `call:` event now that the read site also emits. The
/// inlined twin emits no event for `buildFrom` at all (an assertion is one atomic event), so
/// an extra event here would be the over-correction that splits
/// `near-extracted-local-positive` apart.
#[test]
fn dataflow_inlined_read_as_assertion_subject_is_not_double_counted() {
    let prints = fingerprint_all(
        "import { buildFrom } from './builders';\n\
             test('extracted', () => { const subject = buildFrom(1, 2, 3); \
                 expect(subject).toBe(1); });\n\
             test('inlined', () => { expect(buildFrom(1, 2, 3)).toBe(1); });",
    );
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    assert_eq!(
        prints[0].behavior, prints[1].behavior,
        "an extracted single-use local read as an assertion subject must still match the \
             inlined form exactly, with no extra call event"
    );
}

/// Order, not just multiset: the re-emitted events must land in the same *sequence* the
/// inlined body produces, because `subsumed-test` keys on an ordered subsequence while
/// `near-duplicate-test` keys on a sorted multiset. Walking the initializer back at its
/// declaration would satisfy the multiset and fail this.
#[test]
fn dataflow_inlined_read_preserves_the_inlined_body_event_order() {
    let prints = fingerprint_all(
        "import { build, run } from './builders';\n\
             test('extracted', () => { const subject = build(1); run(subject); });\n\
             test('inlined', () => { run(build(1)); });",
    );
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    assert_eq!(
        prints[0].behavior, prints[1].behavior,
        "the extracted form must emit the initializer's events in the same order the \
             inlined form does (enclosing call first, argument second)"
    );
    let run_position = prints[1]
        .behavior
        .iter()
        .position(|event| event.starts_with("call:") && event.contains(":run args="))
        .expect("run call event");
    let build_position = prints[1]
        .behavior
        .iter()
        .position(|event| event.starts_with("call:") && event.contains(":build args="))
        .expect("build call event");
    assert!(
        run_position < build_position,
        "guard on the premise of this test -- the inlined form emits the enclosing call \
             before its argument: {:?}",
        prints[1].behavior
    );
}

#[test]
fn single_use_inlining_composes_with_reordered_setup() {
    let prints = fingerprint_all(
        "import { grantAccess, loadProfile } from './accounts';\n\
             test('direct', () => { const user = loadProfile(1); grantAccess(1); \
                 expect(user.email).toContain('@'); });\n\
             test('reordered', () => { grantAccess(1); const record = loadProfile(1); \
                 const email = record.email; expect(email).toContain('@'); });",
    );
    assert_eq!(prints.len(), 2);
    let mut direct = prints[0].behavior.clone();
    let mut reordered = prints[1].behavior.clone();
    direct.sort();
    reordered.sort();
    assert_eq!(
        direct, reordered,
        "single-use extraction must not defeat the behavior multiset's setup reordering: \
             {direct:?} vs {reordered:?}"
    );
}

#[test]
fn effectful_two_step_extraction_still_matches_the_inline_form() {
    let prints = fingerprint_all(
        "import { fetchRow } from './rows';\n\
             test('extracted', () => { const raw = fetchRow(1); const name = raw.name; \
                 expect(name).toBe('x'); });\n\
             test('inline', () => { expect(fetchRow(1).name).toBe('x'); });",
    );
    assert_eq!(prints.len(), 2);
    let mut extracted = prints[0].behavior.clone();
    let mut inline = prints[1].behavior.clone();
    extracted.sort();
    inline.sort();
    assert_eq!(
        extracted, inline,
        "the composed-reordering guard must not disable ordinary effectful extraction"
    );
}

#[test]
fn dataflow_does_not_inline_local_read_twice() {
    // Mirrors `fixtures/duplicates/near-extracted-local-evaluation-twin.test.ts`: a local read
    // twice must not be inlined, because a side-effecting initializer (here `nextId`) would
    // then appear evaluated once instead of twice.
    let prints = fingerprint_all(
        "import { nextId } from './ids';\n\
             test('x', () => { const id = nextId(); expect(id).toBe(1); expect(id).toBe(1); });",
    );
    let behavior = &prints[0].behavior;
    assert!(
        behavior
            .iter()
            .any(|event| event.starts_with("call:") && event.contains("nextId")),
        "a local read twice must not be inlined -- the declaration's own call event must \
             survive standalone: {behavior:?}"
    );
}

#[test]
fn dataflow_inline_blocked_by_intervening_effect() {
    let prints = fingerprint_all(
        "import { compute, track } from './m';\n\
             test('x', () => { const total = compute(); track(); expect(total).toBe(1); });",
    );
    let behavior = &prints[0].behavior;
    assert!(
        behavior
            .iter()
            .any(|event| event.starts_with("call:") && event.contains("compute")),
        "a side-effecting statement between declaration and read must block single-use \
             inlining, so the declaration's own call event must survive standalone: {behavior:?}"
    );
}

#[test]
fn dataflow_destructure_renders_as_member_access() {
    let prints = fingerprint_all(
        "import { config } from './config';\n\
             test('destructure', () => { const { retries } = config; expect(retries).toBe(3); });\n\
             test('member', () => { expect(config.retries).toBe(3); });",
    );
    let (a, b) = sorted_behavior_pair(&prints);
    assert_eq!(
        a, b,
        "a destructured property read must render as the equivalent member access: {a:?} \
             vs {b:?}"
    );
}

/// The bug [`DataflowSubst::DestructuredProperty`] exists for, in its measured shape: the destructured
/// object is a *call*, so "destructuring to member access" declines and the property name had
/// nowhere left to go. `alpha` and `structural` both keep it (they serialize the binding
/// pattern), so only `behavior` -- `near-duplicate-test` and `subsumed-test` -- was exposed.
#[test]
fn a_destructured_binding_name_survives_into_the_behavior_event() {
    let prints = fingerprint_all(
        "import { useThing } from './thing';\n\
             test('a', () => { const { isEveryTag } = useThing(); expect(isEveryTag.value).toBe(true); });\n\
             test('b', () => { const { isSameAsInitial } = useThing(); expect(isSameAsInitial.value).toBe(true); });",
    );
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    assert_ne!(
        prints[0].alpha, prints[1].alpha,
        "precondition: the two bodies were never alpha-equal, so `duplicate-test` was never \
             the rule at risk here"
    );
    let mut a = prints[0].behavior.clone();
    let mut b = prints[1].behavior.clone();
    a.sort();
    b.sort();
    assert_ne!(
        a, b,
        "two tests reading different destructured properties of the same factory call must \
             not share a behavior multiset: {a:?} vs {b:?}"
    );
    assert!(
        a.iter().any(|event| event.contains(":prop:isEveryTag")),
        "the property key is what distinguishes them, so it must appear in the event: {a:?}"
    );
}

/// The callee-position half of the same erasure: the destructured binding is *called*, not
/// read into an assertion, so the name has to reach `call_path`, not just `shape`.
#[test]
fn a_destructured_callee_name_survives_into_the_behavior_event() {
    let prints = fingerprint_all(
        "import { useApi } from './api';\n\
             test('a', async () => { const { fetchUser } = useApi(); await fetchUser(1); });\n\
             test('b', async () => { const { deleteUser } = useApi(); await deleteUser(1); });",
    );
    let (a, b) = sorted_behavior_pair(&prints);
    assert_ne!(
        a, b,
        "two tests calling different methods destructured off the same factory must not \
             share a behavior multiset: {a:?} vs {b:?}"
    );
}

/// The decoration must stay inside `behavior`. `alpha` and `structural` are handed a
/// permanently empty dataflow map (see `fingerprint`), which is what keeps `duplicate-test`
/// at Error severity and `parameterization-candidate` unmoved by this entry kind; this pins
/// that wiring rather than trusting it.
#[test]
fn a_destructured_binding_name_never_reaches_alpha_or_structural() {
    let prints = fingerprint_all(
        "import { useThing } from './thing';\n\
             test('a', () => { const { isEveryTag } = useThing(); expect(isEveryTag.value).toBe(true); });",
    );
    assert!(
        !prints[0].alpha.contains("L0:prop:") && !prints[0].structural.contains("L0:prop:"),
        "the behavior-only decoration leaked into a claimed identity: alpha={} structural={}",
        prints[0].alpha,
        prints[0].structural
    );
}

/// The same erasure one nesting level down, found by review after the flat case was fixed:
/// both bodies bind one local whose own key is `id`, so retaining only the innermost key left
/// them with the identical behavior event. Probes the nested shape rather than restating the
/// flat one.
#[test]
fn a_nested_destructured_binding_keeps_the_whole_property_path() {
    let prints = fingerprint_all(
        "import { useThing } from './thing';\n\
             test('a', () => { const { user: { id } } = useThing(); expect(id).toBe(1); });\n\
             test('b', () => { const { post: { id } } = useThing(); expect(id).toBe(1); });",
    );
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    assert_ne!(
        prints[0].behavior, prints[1].behavior,
        "a nested destructured binding must carry the key it was nested under, not only its \
             own: {:?} vs {:?}",
        prints[0].behavior, prints[1].behavior
    );
}

/// The path is `:`-joined, not `.`-joined, because it is spliced into a call path where `.`
/// is the member separator. With a dotted join these two bodies -- one calling a doubly
/// destructured `get`, the other calling `.get` on a singly destructured `api` -- both render
/// `call:L0:prop:api.get`. Probes the collision, not a comfortable variant of it.
#[test]
fn a_nested_path_cannot_imitate_a_member_call_path() {
    let prints = fingerprint_all(
        "import { useThing } from './thing';\n\
             test('a', () => { const { api: { get } } = useThing(); get(); });\n\
             test('b', () => { const { api } = useThing(); api.get(); });",
    );
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    assert_ne!(
        prints[0].behavior, prints[1].behavior,
        "a nested property path must not render the same as the member access it resembles: \
             {:?} vs {:?}",
        prints[0].behavior, prints[1].behavior
    );
}

/// A retained key is appended to an event raw, so a key able to spell the event grammar's own
/// punctuation lets one body's event imitate another's. Both bodies here are real, parseable
/// TypeScript, and before `collect_pattern` restricted itself to keys written as identifiers
/// they emitted the identical `["call:...o", "call:L0:prop:x(<number:1>)"]` -- a
/// self-inflicted `near-duplicate-test` false positive of exactly the kind
/// `escape_literal_delimiters` exists to prevent for literals. Probes the shortest forgery
/// found, not a comfortable variant of it.
#[test]
fn a_top_level_quoted_key_cannot_register_a_destructured_property_event() {
    let prints = fingerprint_all(
        "import { o } from './o';\n\
             test('identifier', () => { const { x } = o; x(1); });\n\
             test('quoted', () => { const { 'x(<number:1>)': f } = o; f(); });",
    );
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    assert_ne!(
        prints[0].behavior, prints[1].behavior,
        "the top-level destructuring-to-member scan must apply the same quoted-key guard as \
             collect_pattern; otherwise its inserted DestructuredProperty entry bypasses the guard"
    );
    assert!(
        !prints[1]
            .behavior
            .iter()
            .any(|event| event.contains(":prop:")),
        "quoted keys must not be spliced into behavior grammar: {:?}",
        prints[1].behavior
    );
}

#[test]
fn a_quoted_key_cannot_forge_another_body_s_event() {
    let prints = fingerprint_all(
        "import { o } from './o';\n\
             test('a', () => { const { x } = o(); x(1); });\n\
             test('b', () => { const { 'x(<number:1>)': f } = o(); f(); });",
    );
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    assert_ne!(
        prints[0].behavior, prints[1].behavior,
        "a quoted destructuring key spelled to imitate a call's argument fragment must not \
             produce the same behavior events as the call it imitates: {:?} vs {:?}",
        prints[0].behavior, prints[1].behavior
    );
}

/// The hazard the `or_insert` precedence in `analyze_dataflow` guards: a symbol
/// "destructuring to member access" already claimed must keep rendering as `object.property`,
/// or `const { a } = o; use(a)` stops matching `use(o.a)`. `dataflow_destructure_renders_as_
/// member_access` pins the equivalence itself; this pins the *reason* it survives, so an
/// overwrite-instead-of-`or_insert` regression is named by the failure rather than guessed at.
#[test]
fn a_member_access_substitution_outranks_the_binding_name() {
    let prints = fingerprint_all(
        "import { config } from './config';\n\
             test('destructure', () => { const { retries } = config; expect(retries).toBe(3); });",
    );
    assert!(
        !prints[0]
            .behavior
            .iter()
            .any(|event| event.contains(":prop:")),
        "a substituted destructured property must render as the member access alone, with no \
             binding-name decoration to break the equivalence: {:?}",
        prints[0].behavior
    );
}

#[test]
fn dataflow_destructure_blocked_by_intervening_mutation() {
    // Mirrors `fixtures/duplicates/near-destructure-mutation-twin.test.ts`.
    let prints = fingerprint_all(
        "test('x', () => { \
                 const state = { count: 1 }; \
                 const { count } = state; \
                 state.count = 2; \
                 expect(count).toBe(1); \
             });",
    );
    let behavior = &prints[0].behavior;
    assert!(
        behavior.iter().any(|event| event.contains(":prop:count")),
        "a mutation between the destructuring declaration and its read must block \
             member-access canonicalization while retaining property identity: {behavior:?}"
    );
}

#[test]
fn dataflow_await_then_matches_bare_await() {
    let prints = fingerprint_all(
        "import { setup, state } from './pipeline';\n\
             test('await', async () => { await setup(); expect(state()).toBe('ready'); });\n\
             test('then', () => { return setup().then(() => { expect(state()).toBe('ready'); }); });",
    );
    let (a, b) = sorted_behavior_pair(&prints);
    assert_eq!(
        a, b,
        "a bare `await` and an equivalent single-argument `.then` chain must produce the \
             same behavior multiset: {a:?} vs {b:?}"
    );
}

#[test]
fn dataflow_await_then_blocked_by_rejection_handler() {
    // Not covered by any `fixtures/` twin: the module doc and this transformation's own
    // precondition (a single awaited call, no interleaving and no rejection handler) both call
    // out a `.then(onFulfilled, onRejected)` as NOT equivalent to a bare `await`.
    let prints = fingerprint_all(
        "import { setup, state } from './pipeline';\n\
             test('await', async () => { await setup(); expect(state()).toBe('ready'); });\n\
             test('then', () => { return setup().then(() => { expect(state()).toBe('ready'); }, \
                 () => {}); });",
    );
    let (a, b) = sorted_behavior_pair(&prints);
    assert_ne!(
        a, b,
        "a `.then` with a rejection handler must NOT canonicalize to the same behavior \
             multiset as a bare await: {a:?} vs {b:?}"
    );
    assert!(
        prints[1].behavior.iter().any(|event| event == "return"),
        "a `.then` with a rejection handler must fall back to ordinary `return` event \
             collection, not the await canonicalization: {:?}",
        prints[1].behavior
    );
}

#[test]
fn dataflow_canonicalization_is_deterministic() {
    let source = "import { computeTotal } from './cart';\n\
             test('via local', () => { const cart = [10, 20]; const total = computeTotal(cart); \
                 expect(total).toBe(30); });";
    let first = fingerprint_all(source);
    let second = fingerprint_all(source);
    assert_eq!(
        first[0].behavior, second[0].behavior,
        "dataflow canonicalization must be deterministic across separate fingerprinting runs \
             of the same source"
    );
}

// ---------------------------------------------------------------------------------------

// The purity precondition behind dataflow canonicalization
// -----------------------------------------------------------------------------------------
//
// `expression_has_effect` and `statement_has_effect_at` are the allowlist that decides
// whether a single-use `const` may be inlined into its one read (and whether a destructured
// property may be rewritten to a member access). Both functions, and 13 of
// `expression_has_effect`'s arms, survived mutation -- including replacing the whole function
// with `true`, which disables the canonicalization wholesale. Nothing failed, because no test
// asserted what an inlined body's `behavior` actually looks like.
//
// These do, through the assertion subject: inlined, it is the initializer's own shape;
// blocked, it is the bare local.

/// [`assertion_subject`] for a whole source file rather than a bare body: the two
/// destructuring-canonicalization tests need imports above the test, which `behavior_of`'s
/// own wrapping cannot carry.
fn assertion_subject_of_source(source: &str) -> String {
    let events = fingerprint_all(source).swap_remove(0).behavior;
    let event = events
        .iter()
        .find(|event| event.starts_with("assert:"))
        .unwrap_or_else(|| panic!("no assertion event: {events:?}"))
        .clone();
    let start = event.find("subject=").expect("subject") + "subject=".len();
    let end = event.find("; value=").expect("value");
    event[start..end].to_owned()
}

/// Every shape `expression_has_effect`/`statement_has_effect_at` clears as pure, all in one
/// body, so that any one of them wrongly reporting an effect blocks the inlining this asserts.
///
/// What makes this fail: replace either function with `true`; delete any of the arms below
/// (the `_ =>` fallback is "has an effect"); or break the `Inline` substitution itself.
#[test]
fn a_pure_intervening_statement_does_not_block_single_use_local_inlining() {
    let pure = concat!(
        ";",                               // EmptyStatement
        " { 1; }",                         // BlockStatement, recursing
        " var w;",                         // a declarator with no initializer
        " 1;",                             // ExpressionStatement
        " const p1 = 1 + 2;",              // BinaryExpression, both operands
        " const p2 = 1 || 2;",             // LogicalExpression
        " const p3 = !1;",                 // UnaryExpression
        " const p4 = 1 ? 2 : 3;",          // ConditionalExpression
        " const p5 = (1);",                // ParenthesizedExpression
        " const p6 = 1 as any;",           // TSAsExpression
        " const p7 = 1 satisfies number;", // TSSatisfiesExpression
        " const p8 = o!;",                 // TSNonNullExpression
        " const p9 = `x${1}`;",            // TemplateLiteral
        " const pa = [1];",                // ArrayExpression
        " const pb = {a:1};",              // ObjectExpression
        " const pc = o.b;",                // StaticMemberExpression
        " const pd = o[0];",               // ComputedMemberExpression
        " const pe = null;",               // the literal/identifier/this/super arm
    );
    assert_eq!(
        assertion_subject(&inlining_probe(pure)),
        "(call id:global:build <number>)",
        "with only allowlisted-pure statements in between, the single-use local must inline \
             into its one read"
    );
}

/// The other direction, one statement shape per row. Each row's intervening statement is
/// *identical* to the pure control above but for one sub-expression, and every row emits no
/// behavior event of its own, so the only thing that moves is whether inlining was declined.
///
/// An assignment is the vehicle throughout because it is effectful (`_ =>` in
/// `expression_has_effect`) while contributing nothing to `behavior` -- so a row that wrongly
/// inlines is visible in the subject and nowhere else.
#[test]
fn an_effectful_intervening_statement_blocks_single_use_local_inlining() {
    #[rustfmt::skip]
    let rows: &[(&str, &str)] = &[
        ("binary-right", "const b = 1 + (y = 1);"),
        ("binary-left", "const b = (y = 1) + 1;"),
        ("logical-right", "const b = 1 || (y = 1);"),
        ("logical-left", "const b = (y = 1) || 1;"),
        ("conditional-test", "const b = (y = 1) ? 1 : 2;"),
        ("conditional-consequent", "const b = 1 ? (y = 1) : 2;"),
        ("conditional-alternate", "const b = 1 ? 2 : (y = 1);"),
        ("computed-member-property", "const b = o[(y = 1)];"),
        ("computed-member-object", "const b = (y = 1)[0];"),
        ("static-member-object", "const b = (y = 1).c;"),
        ("unary", "const b = !(y = 1);"),
        ("array-element", "const b = [(y = 1)];"),
        ("object-property", "const b = {a:(y = 1)};"),
        ("object-spread", "const b = {...o};"),
        ("template-interpolation", "const b = `x${y = 1}`;"),
        ("parenthesized", "const b = ((y = 1));"),
        ("ts-as", "const b = (y = 1) as any;"),
        ("ts-satisfies", "const b = (y = 1) satisfies unknown;"),
        ("ts-non-null", "const b = (y = 1)!;"),
        ("inside-a-block", "{ (y = 1); }"),
        ("bare-expression-statement", "(y = 1);"),
        ("control-flow-is-never-cleared", "if (1) { }"),
    ];
    for (name, intervening) in rows {
        let subject = assertion_subject(&inlining_probe(intervening));
        assert!(
            subject.starts_with("id:L"),
            "{name}: `{intervening}` has an effect, so `subject` must stay the un-inlined \
                 local, got {subject:?}"
        );
    }
}

/// The same precondition guards the destructuring-to-member-access rewrite, which reads the
/// widest declaration-to-last-read range rather than a single read.
#[test]
fn the_purity_precondition_also_guards_destructuring_to_member_access() {
    // The object has to be a *resolved* free symbol -- an unresolved true global carries no
    // identity safe to key the rewrite on -- so it is imported rather than assumed.
    let subject_for = |intervening: &str| {
        assertion_subject_of_source(&format!(
            "import {{ o }} from './o';\n\
                 test('t', () => {{ let y; const {{ a }} = o; {intervening} \
                 expect(a).toBe(3); }});"
        ))
    };
    let subject = subject_for("const p = 1 + 2;");
    assert!(
        subject.starts_with("(member ") && subject.ends_with(" .a)"),
        "a destructured property of a resolved free object must canonicalize to the member \
             access it stands for, got {subject:?}"
    );
    let subject = subject_for("const p = 1 + (y = 1);");
    assert!(
        subject.starts_with("id:L") && subject.ends_with(":prop:a"),
        "an intervening effect must block member-access equivalence while retaining the \
             property's identity, got {subject:?}"
    );
}

/// `SymbolReadScan` refuses to reason about a read that sits inside a branch, loop, switch,
/// try, class body, or nested closure: execution past the declaration is then conditional,
/// repeated, or deferred, so "declared once, read exactly there" is unsound.
///
/// The observable is event *order*. With the read ruled unsafe, the declaration keeps its own
/// `call:global:f` event where it stands; with the read wrongly accepted, `walk_stmt` skips
/// the declaration and the event re-appears later (at the read) or, inside a class body,
/// vanishes entirely. Every `unsafe_depth` bracket in that visitor is one row here, plus a
/// positive control so the test cannot pass by declaring everything unsafe.
#[test]
fn a_read_inside_control_flow_or_a_closure_is_never_inlined() {
    #[rustfmt::skip]
    let unsafe_reads: &[(&str, &str)] = &[
        ("if", "if (1) { g(x); }"),
        ("for", "for (;;) { g(x); }"),
        ("for-in", "for (const k in o) { g(x); }"),
        ("for-of", "for (const k of o) { g(x); }"),
        ("while", "while (1) { g(x); }"),
        ("do-while", "do { g(x); } while (0);"),
        ("switch", "switch (1) { case 1: g(x); }"),
        ("try", "try { g(x); } catch (e) { }"),
        ("class-body", "class C { p = x; }"),
        ("function-declaration", "function h() { g(x); }"),
        ("arrow", "const h = () => { g(x); };"),
    ];
    for (name, reader) in unsafe_reads {
        let events = behavior_of(&format!("const x = f(); {reader}"));
        assert_eq!(
            events.first().map(String::as_str),
            Some("call:global:f"),
            "{name}: the read in `{reader}` is not safe to inline, so the declaration must \
                 keep its own event in place: {events:?}"
        );
    }

    // The control: a read in the same flat statement list IS inlined, so the declaration's
    // event moves to the read site. Without this the test above would pass just as well if
    // every read were ruled unsafe.
    let events = behavior_of("const x = f(); g(x);");
    assert!(events[0].starts_with("call:global:g args="), "{events:?}");
    assert_eq!(events[1], "call:global:f");
}

// -----------------------------------------------------------------------------------------

// Dataflow candidate selection
// -----------------------------------------------------------------------------------------

/// `analyze_dataflow` only ever considers a single-declarator `const` whose one read is in
/// the same flat statement list. Every other shape is skipped, and each skip is one row.
///
/// Un-inlined is the safe answer, so these rows assert the *absence* of a canonicalization
/// this module must not claim -- with the positive control in
/// `a_read_inside_control_flow_or_a_closure_is_never_inlined` proving the mechanism works at
/// all, so "skip everything" cannot pass both.
#[test]
fn only_a_single_declarator_const_read_once_is_ever_inlined() {
    #[rustfmt::skip]
    let never_inlined: &[(&str, &str)] = &[
        ("let-not-const", "let x = build(1); expect(x).toBe(3);"),
        ("var-not-const", "var x = build(1); expect(x).toBe(3);"),
        ("two-declarators", "const x = build(1), y = 2; expect(x).toBe(3);"),
        ("read-twice", "const x = build(1); g(x); expect(x).toBe(3);"),
        ("never-read", "const x = build(1); expect(build(2)).toBe(3);"),
    ];
    for (name, body) in never_inlined {
        let events = behavior_of(body);
        assert!(
            events
                .iter()
                .any(|event| event.starts_with("call:global:build")),
            "{name}: `{body}` must keep the initializer's own event where it stands: \
                 {events:?}"
        );
    }
    // A declarator with no initializer at all is skipped before any of that.
    let events = behavior_of("let x; x = build(1); expect(2).toBe(3);");
    assert!(
        events
            .iter()
            .any(|event| event.starts_with("assign:=") && event.contains("call id:global:build")),
        "a declaration with no initializer is not an inlining candidate: {events:?}"
    );
    // An array-pattern declaration is neither an `Inline` nor a `Member` candidate.
    assert!(
        behavior_of("const [x] = build(1); expect(x).toBe(3);")
            .first()
            .is_some_and(|event| event.starts_with("call:global:build")),
        "an array-pattern declaration is not a dataflow candidate"
    );
}

/// `try_register_members` only produces `object.property` equivalence for shapes it can
/// vouch for. Declined shapes still retain the destructured property's identity.
#[test]
fn destructuring_to_member_access_declines_every_shape_it_cannot_vouch_for() {
    let subject_for = |source: String| assertion_subject_of_source(&source);
    let imported =
        |body: &str| format!("import {{ o, k }} from './m';\ntest('t', () => {{ {body} }});");

    // The control: a plain destructured property of a resolved free object IS rewritten.
    let subject = subject_for(imported("const { a } = o; expect(a).toBe(3);"));
    assert!(
        subject.starts_with("(member ") && subject.ends_with(" .a)"),
        "the control must be rewritten, got {subject:?}"
    );

    #[rustfmt::skip]
    let unnamed: &[(&str, &str)] = &[
        ("computed-key", "const { [k]: a } = o; expect(a).toBe(3);"),
        ("nested-pattern", "const { a: { b } } = o; expect(b).toBe(3);"),
        ("property-default", "const { a = 1 } = o; expect(a).toBe(3);"),
        ("object-pattern-rest", "const { ...a } = o; expect(a).toBe(3);"),
    ];
    for (name, body) in unnamed {
        let subject = subject_for(imported(body));
        assert!(
            subject.starts_with("id:L"),
            "{name}: `{body}` has no plain property identity to retain, got {subject:?}"
        );
    }

    #[rustfmt::skip]
    let identity_only: &[(&str, &str)] = &[
        ("object-is-not-an-identifier", "const { a } = mk(); expect(a).toBe(3);"),
        ("object-is-a-body-local", "const o2 = mk(); const { a } = o2; expect(a).toBe(3);"),
    ];
    for (name, body) in identity_only {
        let subject = subject_for(imported(body));
        assert!(
            subject.starts_with("id:L") && subject.ends_with(":prop:a"),
            "{name}: `{body}` is not a shape this rewrite can vouch for, so the read must \
                 retain only its property identity, got {subject:?}"
        );
    }

    // An unresolved true global has no identity safe to key the rewrite on, even though the
    // pattern itself is the supported shape.
    let subject = subject_for(
        "test('t', () => { const { a } = someGlobal; expect(a).toBe(3); });".to_owned(),
    );
    assert!(
        subject.starts_with("id:L") && subject.ends_with(":prop:a"),
        "an unresolved global object carries no safe object identity, but the property name \
             must survive, got {subject:?}"
    );

    // A destructured property that is never read has nothing to substitute.
    let events = fingerprint_all(&imported("const { a } = o; expect(1).toBe(3);")).swap_remove(0);
    assert_eq!(
        events.behavior.len(),
        1,
        "an unread destructured property contributes no event: {:?}",
        events.behavior
    );
}

// -----------------------------------------------------------------------------------------
