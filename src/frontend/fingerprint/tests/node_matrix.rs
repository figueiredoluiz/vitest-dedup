use super::*;

// Node-kind matrices: what each modeled `match` arm is *for*
// -----------------------------------------------------------------------------------------
//
// 135 of this module's surviving mutants are `match`-arm deletions in `ser_expr`/`ser_stmt`/
// `walk_expr`/`walk_stmt` and their helpers. A deleted arm does not crash: it routes the node
// kind through the `_ =>` fallback, which is `opaque`/`degraded` raw source text for the
// serializers and silence for the behavior walk. That is precisely how two tests differing
// *only* in that node kind stop being distinguishable -- or, in the other direction, how two
// alpha-equivalent bodies stop matching because their raw text differs.
//
// The tests below pin, per node kind, the exact fragment the arm emits. An assertion that
// some fingerprint merely *differs* from another would not do it: `opaque` embeds verbatim
// source, so two different nodes routed through the fallback still differ from each other.
// Only the emitted content says the arm ran.

/// A TypeScript wrapper around an ASSIGNMENT TARGET is transparent, the same as one around a
/// value.
///
/// `ser_simple_assignment_target`'s four TS arms unwrap to the inner expression, and none had
/// coverage: every assignment in these tests writes to a bare identifier. They are the
/// assignment-side twin of a fix review already made on the value side -- the arms exist
/// precisely so `count! = 1` and `count = 1` are not two different behaviours -- and a wrapper
/// that stopped unwrapping would make type annotations look like behavioural differences, which
/// is a false negative on every duplicate pair where one half is annotated.
#[test]
fn a_typescript_wrapper_on_an_assignment_target_is_transparent() {
    let bare = alpha_of("let count = 0; count = 5; expect(count).toBe(5);");
    for wrapped in [
        "let count = 0; count! = 5; expect(count).toBe(5);",
        "let count = 0; (count as number) = 5; expect(count).toBe(5);",
        "let count = 0; (count satisfies number) = 5; expect(count).toBe(5);",
        "let count = 0; (<number>count) = 5; expect(count).toBe(5);",
    ] {
        assert_eq!(
            alpha_of(wrapped),
            bare,
            "a type wrapper is not a behavioural difference: {wrapped}"
        );
    }
}

/// `behavior` for one `async` test callback body.
fn async_behavior_of(body: &str) -> Vec<String> {
    let prints = fingerprint_all(&format!("test('t', async () => {{ {body} }});"));
    assert_eq!(prints.len(), 1, "expected one test body for {body:?}");
    prints.into_iter().next().expect("one body").behavior
}

/// Every fingerprint view for one `async` test callback body, from a single parse.
fn async_prints_of(body: &str) -> BodyFingerprint {
    let prints = fingerprint_all(&format!("test('t', async () => {{ {body} }});"));
    assert_eq!(prints.len(), 1, "expected one test body for {body:?}");
    prints.into_iter().next().expect("one body")
}

/// Every modeled node kind, with the exact fragment its `Serializer` arm emits.
///
/// Each row fails if its arm is deleted (the node falls through to `opaque`, whose text is
/// the raw source and never this fragment), and most fail on any edit to the fragment itself.
#[rustfmt::skip]
const SERIALIZED_FRAGMENTS: &[(&str, &str, &str)] = &[
    // (case name, test-callback body, fragment that must appear in `alpha`)
    // --- literals: the kind tag is what keeps `true` apart from `'true'` ---
    ("boolean", "const v = true;", "<boolean:true>"),
    ("null", "const v = null;", "<null:null>"),
    ("number", "const v = 1;", "<number:1>"),
    ("bigint", "const v = 1n;", "<bigint:1>"),
    ("regex", "const v = /ab/g;", "<regex:/ab/g>"),
    ("string", "const v = 's';", "<string:s>"),
    ("template", "const v = `q${1}`;", "(template `q` <number:1> ``)"),
    // --- `this` is both serialized and recorded as a form-changing observation ---
    ("this-value", "f(this);", " this)"),
    ("this-changes-the-callable-form", "f(this);", "(arrow-fn () "),
    ("arguments-changes-the-callable-form", "g(arguments);", "(arrow-fn () "),
    // --- composites ---
    ("array", "const v = [1];", "(array <number:1>)"),
    ("array-hole", "const v = [1,,2];", "<hole>"),
    ("array-spread", "const v = [...a];", "(array (spread "),
    ("argument-spread", "f(...a);", "(call id:global:f (spread "),
    ("object", "const v = {a:1};", "(object (prop prop:a <number:1>))"),
    ("object-computed-key", "const v = {[k]:1};", "(prop [id:global:k] <number:1>)"),
    ("object-spread", "const v = {...a};", "(object (spread "),
    ("arrow", "const v = () => 1;", "(fn () <number:1>)"),
    ("function-expression", "const v = function(){};", "(fn () (block ))"),
    ("generator-expression", "const v = async function*(){};", "(async *fn () (block ))"),
    ("call", "const v = f(1);", "(call id:global:f <number:1>)"),
    ("optional-call", "const v = f?.(1);", "(call?. id:global:f <number:1>)"),
    ("new", "const v = new F(1);", "(new id:global:F <number:1>)"),
    ("assign", "let x; x = 1;", "(assign = id:"),
    ("assign-to-member", "o.b = 1;", "(assign = (member id:global:o .b) <number:1>)"),
    ("assign-to-index", "o[0] = 1;", "(assign = (index id:global:o <number:0>) <number:1>)"),
    ("assign-destructuring-falls-back", "[a] = o;", "<opaque:[a]>"),
    ("binary", "const v = 1+2;", "(bin + <number:1> <number:2>)"),
    ("logical", "const v = 1||2;", "(logic || <number:1> <number:2>)"),
    ("unary", "const v = !1;", "(unary ! <number:1>)"),
    ("update", "let x; x++;", "(update ++ false id:"),
    ("update-member", "o.b++;", "(update ++ false (member id:global:o .b))"),
    ("update-index", "o[0]++;", "(update ++ false (index id:global:o <number:0>))"),
    ("conditional", "const v = 1?2:3;", "(cond <number:1> <number:2> <number:3>)"),
    ("sequence", "const v = (1,2);", "(seq <number:1> <number:2>)"),
    ("yield", "const v = function*(){ yield 1; };", "(yield <number:1>)"),
    ("yield-delegate", "const v = function*(){ yield* g(); };", "(yield* "),
    ("yield-bare", "const v = function*(){ yield; };", "(yield)"),
    ("static-member", "const v = o.b;", "(member id:global:o .b)"),
    ("optional-static-member", "const v = o?.b;", "(member id:global:o ?.b)"),
    ("computed-member", "const v = o[0];", "(index id:global:o <number:0>)"),
    ("optional-computed-member", "const v = o?.[0];", "(index? id:global:o <number:0>)"),
    ("tagged-template", "const v = t`q`;", "(tagged id:global:t (template `q`))"),
    ("instantiation", "const v = f<number>;", "(instantiate id:global:f<targs:"),
    // --- statements ---
    ("block", "{ 1; }", "(block (stmt <number:1>))"),
    ("empty", ";", "(empty)"),
    ("debugger", "debugger;", "(debugger)"),
    ("var", "var a = 1;", "(var (decl "),
    ("let", "let a = 1;", "(let (decl "),
    ("const", "const a = 1;", "(const (decl "),
    ("declarator-without-init", "let a;", "(let (decl L0))"),
    ("function-declaration", "function g(){}", "(block (function-decl L0 (fn () (block ))))"),
    ("if", "if(1)2;", "(if <number:1> (stmt <number:2>))"),
    ("if-else", "if(1)2;else 3;", "(if <number:1> (stmt <number:2>) (stmt <number:3>))"),
    ("for", "for(;;)1;", "(for () () () (stmt <number:1>))"),
    ("for-with-parts", "for(let i=0;i<1;i++)2;", "(for (let (decl "),
    ("for-with-expression-init", "for(f();;)1;", "(for (call id:global:f ) () () "),
    ("for-in-declaration", "for(const k in o)1;", "(for-in (decl "),
    ("for-in-identifier", "let k; for(k in o)1;", "(for-in id:"),
    ("for-in-member", "for(o.b in o)1;", "(for-in (member id:global:o .b) "),
    ("for-in-index", "for(o[0] in o)1;", "(for-in (index id:global:o <number:0>) "),
    ("for-of", "for(const k of o)1;", "(for-of (decl "),
    ("while", "while(1)2;", "(while <number:1> (stmt <number:2>))"),
    ("do-while", "do 1; while(2);", "(do-while (stmt <number:1>) <number:2>)"),
    ("return-bare", "return;", "(return)"),
    ("return-value", "return 1;", "(return <number:1>)"),
    ("throw", "throw 1;", "(throw <number:1>)"),
    ("try-catch-finally", "try{1;}catch(e){2;}finally{3;}", "(try (block (stmt <number:1>)) (catch "),
    ("try-without-handler", "try{1;}finally{2;}", "(try (block (stmt <number:1>)) () (block"),
    ("catch-without-param", "try{1;}catch{2;}", "(catch () (block (stmt <number:2>)))"),
    ("switch", "switch(1){case 2: 3;}", "(switch <number:1> (case <number:2> (stmt <number:3>)))"),
    ("switch-default", "switch(1){default: 3;}", "(case default (stmt <number:3>))"),
    ("break", "while(1){break;}", "(break)"),
    ("break-label", "l: while(1){break l;}", "(break l)"),
    ("continue", "while(1){continue;}", "(continue)"),
    ("continue-label", "l: while(1){continue l;}", "(continue l)"),
    ("label", "l: 1;", "(label l (stmt <number:1>))"),
    ("class-owned-summary", "class C {}", "(class C0 body="),
    // --- parameters and binding patterns ---
    ("param-identifier", "const v = (a) => a;", "(fn (L1) id:L1)"),
    ("param-object-pattern", "const v = ({a}) => a;", "(object-pattern (prop prop:a "),
    ("param-nested-object-pattern", "const v = ({a:{b}}) => b;", "(prop prop:a (object-pattern "),
    ("param-array-pattern", "const v = ([a]) => a;", "(array-pattern L1)"),
    ("param-array-pattern-hole", "const v = ([a,,b]) => a;", "(array-pattern L1 <hole> L2)"),
    ("param-default", "const v = (a=1) => a;", "(default L1 <number:1>)"),
    ("param-rest", "const v = (...a) => a;", "(rest L1)"),
    ("param-object-rest", "const v = ({...r}) => r;", "(object-pattern (rest "),
    ("param-array-rest", "const v = ([...r]) => r;", "(array-pattern (rest "),
];

/// Rows whose whole point is that the wrapper *is* unwrapped: a deleted arm replaces the
/// inner expression with the wrapper's verbatim source, which shows up as `<opaque:`.
#[rustfmt::skip]
const UNWRAPPED_EXPRESSIONS: &[(&str, &str, &str)] = &[
    ("parenthesized", "const v = (1);", "<number:1>"),
    ("ts-as", "const v = 1 as any;", "<number:1>"),
    ("ts-satisfies", "const v = 1 satisfies number;", "<number:1>"),
    ("ts-non-null", "const v = o!;", "global:o"),
    ("ts-type-assertion", "const v = <number>o;", "global:o"),
];

/// A nullish guard on a computed member is part of the body's identity.
///
/// `o[0]` throws when `o` is nullish; `o?.[0]` yields `undefined`. Two tests differing only in
/// that guard are different tests -- and they serialized identically until 2026-09-25, so
/// `duplicate-test` reported them at ERROR severity on other people's CI.
///
/// Both siblings already recorded optionality (`.b` against `?.b`, `(call` against
/// `(call?.`), so the gap was the one member form that did not: the "fixed at one site, not
/// its siblings" shape rather than a decision. The whole family is asserted here together so
/// the next form added is compared against all of them.
#[test]
fn optionality_is_part_of_every_member_and_call_form() {
    let pairs = [
        ("computed member", "const v = o[0];", "const v = o?.[0];"),
        ("static member", "const v = o.b;", "const v = o?.b;"),
        ("call", "f(1);", "f?.(1);"),
        // It propagates: a collapsed link contaminates everything built on top of it.
        (
            "computed member in a chain",
            "const v = o[0].k;",
            "const v = o?.[0].k;",
        ),
        ("computed member as an argument", "f(o[0]);", "f(o?.[0]);"),
        (
            "private field",
            "class A { #v = 1; static r(o) { return o.#v; } }",
            "class A { #v = 1; static r(o) { return o?.#v; } }",
        ),
        (
            "private field in a chain",
            "class A { #v = {}; static r(o) { return o.#v.k; } }",
            "class A { #v = {}; static r(o) { return o?.#v.k; } }",
        ),
    ];
    for (name, plain, optional) in pairs {
        assert_ne!(
            alpha_of(plain),
            alpha_of(optional),
            "{name}: a nullish guard must change `alpha`, or two different tests collapse \
                 into one `duplicate-test` finding at error severity"
        );
    }

    // The control. Optionality is the ONLY difference being claimed -- without this, the
    // assertions above would hold just as well if every body serialized uniquely.
    assert_eq!(
        alpha_of("const v = o[0];"),
        alpha_of("const v = o[0];"),
        "the same body must still serialize identically"
    );
}

/// Parentheses that end an optional chain are part of the body.
///
/// `o?.b.c` short-circuits to `undefined` when `o` is nullish; `(o?.b).c` evaluates the chain
/// first and then reads `.c` of its `undefined`, which throws. The serializer unwrapped the
/// parentheses and the chain alike, so the pair collapsed into one `duplicate-test` finding at
/// error severity. The marker is placed only where a parenthesized chain is a receiver, so every
/// other spelling keeps its text.
#[test]
fn parentheses_that_end_an_optional_chain_are_part_of_the_body() {
    let pairs = [
        ("member", "const v = o?.b.c;", "const v = (o?.b).c;"),
        ("call", "const v = o?.b();", "const v = (o?.b)();"),
        ("index", "const v = o?.b[0];", "const v = (o?.b)[0];"),
        (
            "private field",
            "class A { #c = 1; static r(o) { return o?.b.#c; } }",
            "class A { #c = 1; static r(o) { return (o?.b).#c; } }",
        ),
        (
            "past a type wrapper",
            "const v = o?.b.c;",
            "const v = (o?.b)!.c;",
        ),
        (
            "type wrapper inside the parentheses",
            "const v = o?.b.c;",
            "const v = (o?.b as any).c;",
        ),
    ];
    for (name, chained, ended) in pairs {
        assert_ne!(
            alpha_of(chained),
            alpha_of(ended),
            "{name}: ending the chain must change `alpha`"
        );
    }

    // Controls: only the chain boundary is claimed to matter.
    assert_eq!(
        alpha_of("const v = (o?.b).c;"),
        alpha_of("const v = ((o?.b)).c;"),
        "nested parentheses around the same chain are one boundary"
    );
    assert_eq!(
        alpha_of("const v = (o?.b);"),
        alpha_of("const v = o?.b;"),
        "parentheses that are not a receiver stay transparent"
    );
    // The outer `?.` short-circuits on the same nullish value, so the parentheses change nothing.
    for (name, chained, ended) in [
        ("member", "const v = o?.b?.c;", "const v = (o?.b)?.c;"),
        ("index", "const v = o?.b?.[0];", "const v = (o?.b)?.[0];"),
        ("call", "const v = o?.b?.();", "const v = (o?.b)?.();"),
    ] {
        assert_eq!(
            alpha_of(chained),
            alpha_of(ended),
            "{name}: an optional access after the parentheses makes them irrelevant"
        );
    }
    assert_eq!(
        alpha_of("const v = (o).b;"),
        alpha_of("const v = o.b;"),
        "parentheses around a non-chain receiver stay transparent"
    );
    assert!(
        !alpha_of("const v = o?.b.c;").contains("chain-end"),
        "an unparenthesized chain keeps its previous text"
    );
    // A chain cannot be a tag without parentheses, so there is no unparenthesized spelling to
    // compare; the receiver path is still the one marking it.
    assert!(alpha_of("const v = (o?.b)`x`;").contains("chain-end"));
}

/// `for await` selects a different iteration protocol, so it is part of the loop's identity.
///
/// `for await (const v of xs)` prefers `Symbol.asyncIterator`, falling back to a wrapped
/// synchronous iterator that awaits Promise-valued elements. Ordinary `for` selects
/// `Symbol.iterator` and yields those Promises unchanged. Both
/// callbacks must be `async` for the pair to parse, so the callable's own asyncness cannot
/// carry the difference: until 2026-10-18 the serializer and the behavior walk both dropped
/// `ForOfStatement.r#await`, and `duplicate-test` reported the pair at error severity on
/// bodies where one test passes and the other fails.
#[test]
fn for_await_selects_a_different_iteration_protocol() {
    let sync = async_prints_of("for(const k of o)f();");
    let r#await = async_prints_of("for await(const k of o)f();");

    for (view, left, right) in [
        ("alpha", sync.alpha.clone(), r#await.alpha.clone()),
        (
            "structural",
            sync.structural.clone(),
            r#await.structural.clone(),
        ),
        (
            "behavior",
            format!("{:?}", sync.behavior),
            format!("{:?}", r#await.behavior),
        ),
    ] {
        assert_ne!(
            left, right,
            "{view}: a body that awaits its elements is a different program"
        );
    }

    // The exact fragments, so a deleted or respelled arm cannot hide behind a bare
    // inequality: `opaque` embeds verbatim source, which also differs.
    assert!(
        r#await.alpha.contains("(for-await-of "),
        "the serializer must name the protocol: {}",
        r#await.alpha
    );
    assert_eq!(
        sync.behavior,
        vec!["loop".to_owned(), "call:global:f".to_owned()],
        "a sync for-of keeps the shared loop token"
    );
    assert_eq!(
        r#await.behavior,
        vec!["loop-await".to_owned(), "call:global:f".to_owned()],
        "an awaited for-of records the protocol in its loop event"
    );

    // The control. The protocol is the ONLY difference being claimed.
    assert_eq!(
        async_prints_of("for(const k of o)f();").alpha,
        sync.alpha,
        "the same body must still serialize identically"
    );
    assert_eq!(
        async_prints_of("for await(const k of o)f();").alpha,
        r#await.alpha,
        "the same awaited body must still serialize identically"
    );
}

#[test]
fn every_modeled_node_kind_emits_its_own_serialized_fragment() {
    let mut problems: Vec<String> = Vec::new();
    for (name, body, fragment) in SERIALIZED_FRAGMENTS {
        let alpha = alpha_of(body);
        if !alpha.contains(fragment) {
            problems.push(format!(
                "{name}: expected {fragment:?} in the `alpha` of `{body}`, got {alpha}"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "{} of {} rows did not emit their fragment:\n{}",
        problems.len(),
        SERIALIZED_FRAGMENTS.len(),
        problems.join("\n")
    );
    for (name, body, fragment) in UNWRAPPED_EXPRESSIONS {
        let alpha = alpha_of(body);
        assert!(
            alpha.contains(fragment),
            "{name}: expected {fragment:?} in the `alpha` of `{body}`, got {alpha}"
        );
        assert!(
            !alpha.contains("<opaque:"),
            "{name}: `{body}` must be unwrapped to its inner expression, not rendered as \
                 raw text: {alpha}"
        );
    }
    // `await` needs its own async wrapper.
    let alpha = async_alpha_of("const v = await 1;");
    assert!(
        alpha.contains("(await <number:1>)"),
        "await must be modeled, got {alpha}"
    );
}

/// Pairs of bodies that differ in nothing but one operator (or one operator-shaped choice:
/// a position, an optional link, a declaration keyword) and must never share a fingerprint.
///
/// [`SERIALIZED_FRAGMENTS`] proves each arm exists and spells its own shape, but every row
/// there carries exactly *one* operator per kind. An arm that hard-coded its operator, or
/// dropped `operator.as_str()` from its format string altogether, satisfies every one of
/// those rows while collapsing `a + b` into `a - b`. This is the other half of that guard,
/// and each row here is a `duplicate-test` at error severity on correct code if it stops
/// holding -- the two bodies are different programs.
///
/// `o[0]` against `o?.[0]` was once absent from this table on purpose: it was the one member
/// form whose optional link did not reach `alpha`, so the row would have failed and pinning
/// the collision would have cemented it. [`Serializer::ser_computed_member`] now reads
/// `member.optional`, matching [`Serializer::ser_static_member`] (`.b` against `?.b`) and
/// [`Serializer::ser_call`] (`(call` against `(call?.`), so the pair is asserted below with
/// the rest.
#[rustfmt::skip]
const OPERATOR_PAIRS: &[(&str, &str, &str)] = &[
    // (case name, one body, the body it must not fingerprint as)
    ("arithmetic",            "const v = a + b;",  "const v = a - b;"),
    ("arithmetic non-additive", "const v = a * b;", "const v = a / b;"),
    ("remainder vs division", "const v = a % b;",  "const v = a / b;"),
    ("exponent vs product",   "const v = a ** b;", "const v = a * b;"),
    ("comparison direction",  "const v = a < b;",  "const v = a > b;"),
    ("comparison strictness", "const v = a < b;",  "const v = a <= b;"),
    ("loose vs strict equality",   "const v = a == b;", "const v = a === b;"),
    ("loose vs strict inequality", "const v = a != b;", "const v = a !== b;"),
    ("shift direction",       "const v = a << b;", "const v = a >> b;"),
    ("shift fill",            "const v = a >> b;", "const v = a >>> b;"),
    ("bitwise",               "const v = a & b;",  "const v = a | b;"),
    ("membership",            "const v = a in b;", "const v = a instanceof b;"),
    ("logical or vs and",     "const v = a || b;", "const v = a && b;"),
    ("nullish vs or",         "const v = a ?? b;", "const v = a || b;"),
    ("unary not vs negate",   "const v = !a;",     "const v = -a;"),
    ("unary sign",            "const v = +a;",     "const v = -a;"),
    ("unary typeof vs void",  "const v = typeof a;", "const v = void a;"),
    ("unary complement",      "const v = ~a;",     "const v = !a;"),
    // Order matters as much as the operator: `++x` and `x++` evaluate to different values.
    ("update direction",      "let x; x++;",       "let x; x--;"),
    ("update position",       "let x; x++;",       "let x; ++x;"),
    ("assignment compound",   "let x; x += 1;",    "let x; x -= 1;"),
    ("assignment plain vs compound", "let x; x = 1;", "let x; x += 1;"),
    ("assignment logical",    "let x; x ||= 1;",   "let x; x &&= 1;"),
    // An optional link is a guard against a nullish receiver, so it is part of what the
    // body does -- exactly as the `.b`/`?.b` and `f(...)`/`f?.(...)` spellings record.
    ("optional member",       "const v = o.b;",    "const v = o?.b;"),
    ("optional call",         "f(1);",             "f?.(1);"),
    ("optional computed member", "const v = o[0];", "const v = o?.[0];"),
    ("static vs computed member", "const v = o.b;", "const v = o[b];"),
    ("call vs construction",  "const v = f(1);",   "const v = new f(1);"),
    // Statement-level choices, on the same terms.
    ("for-in vs for-of",      "for(const k in o)f();", "for(const k of o)f();"),
    ("while vs do-while",     "while(a)f();",      "do f(); while(a);"),
    ("else branch present",   "if(a)f();",         "if(a)f();else f();"),
    ("break vs continue",     "while(a){break;}",  "while(a){continue;}"),
    ("labelled break",        "l: while(a){break;}", "l: while(a){break l;}"),
    ("throw vs return",       "throw a;",          "return a;"),
    ("declaration kind var/let",   "var a = 1;",   "let a = 1;"),
    ("declaration kind let/const", "let a = 1;",   "const a = 1;"),
    ("yield delegation", "const v = function*(){ yield g(); };",
                         "const v = function*(){ yield* g(); };"),
    ("switch case vs default", "switch(a){case 1: f();}", "switch(a){default: f();}"),
    ("array hole",            "const v = [1,,2];", "const v = [1,2];"),
    ("argument spread",       "f(a);",             "f(...a);"),
];

/// An operator is part of a body's identity wherever one appears.
///
/// If a row here fails, two tests that compute genuinely different things have become
/// `alpha`-equal, which is the input `duplicate-test` reports at error severity.
#[test]
fn an_operator_is_part_of_the_identity_of_every_construct_that_carries_one() {
    for (name, left, right) in OPERATOR_PAIRS {
        assert_ne!(
            alpha_of(left),
            alpha_of(right),
            "{name}: `{left}` / `{right}`"
        );
    }
}

/// `structural` erases a literal's *value*, and nothing else. An operator survives it, so
/// the same pairs stay distinct there too -- otherwise `parameterization-candidate` would
/// propose a `test.each` table over two bodies that compute different things, which is the
/// same defect one severity down.
#[test]
fn an_operator_survives_the_structural_collapse_of_literal_values() {
    for (name, left, right) in OPERATOR_PAIRS {
        assert_ne!(
            structural_of(left),
            structural_of(right),
            "{name}: `{left}` / `{right}`"
        );
    }
}

/// `structural` for one test callback body.
fn structural_of(body: &str) -> String {
    structural_in(Path::new("case.test.ts"), body)
}

/// The update *position* reaches the behavior event, not only `alpha`.
///
/// `updates_are_part_of_behavior_identity` pins the postfix spelling; a renderer that
/// printed one word for both positions would still satisfy it. `++i` and `i++` differ in
/// the value the surrounding expression observes, so the two must never share an event.
#[test]
fn an_update_records_which_side_of_its_target_it_sits_on() {
    let prefix = behavior_of("let x; ++x; expect(x).toBe(1);");
    let postfix = behavior_of("let x; x++; expect(x).toBe(1);");
    assert!(
        prefix.iter().any(|event| event.contains("update:prefix++")),
        "a prefix update must name its position: {prefix:?}"
    );
    assert!(
        postfix
            .iter()
            .any(|event| event.contains("update:postfix++")),
        "...and so must a postfix one: {postfix:?}"
    );
    assert_ne!(prefix, postfix, "the two positions are different behavior");
}

/// `structural` collapses a literal's *value* to its kind, which is what lets
/// `parameterization-candidate` propose a `test.each` table. That collapse lives in the same
/// `ser_expr` arms as the `alpha` rendering, so it needs its own assertion: two bodies
/// differing only in a literal's value must be structurally equal, and two bodies differing
/// in the literal's *kind* must not.
#[test]
fn structural_collapses_literal_values_but_not_literal_kinds() {
    #[rustfmt::skip]
    let same_kind: &[(&str, &str, &str)] = &[
        ("boolean", "const v = true;", "const v = false;"),
        ("number", "const v = 1;", "const v = 22;"),
        ("bigint", "const v = 1n;", "const v = 22n;"),
        ("regex", "const v = /a/;", "const v = /bb/;"),
        ("string", "const v = 'a';", "const v = 'bb';"),
    ];
    for (name, left, right) in same_kind {
        let source = format!("test('a', () => {{ {left} }});\ntest('b', () => {{ {right} }});");
        let prints = fingerprint_all(&source);
        assert_eq!(prints.len(), 2);
        assert_eq!(
            prints[0].structural, prints[1].structural,
            "{name}: two values of one literal kind must share a `structural` shape"
        );
        assert_ne!(
            prints[0].alpha, prints[1].alpha,
            "{name}: ...while `alpha` keeps the values apart"
        );
    }

    // Cross-kind: the kind tag survives the value collapse. `1000` the number and `'1000'`
    // the string were a real adjudicated false duplicate before the tag existed.
    let prints = fingerprint_all(
        "test('a', () => { const v = 1000; });\ntest('b', () => { const v = '1000'; });",
    );
    assert_ne!(
        prints[0].structural, prints[1].structural,
        "a number and a string must never collapse to the same structural shape"
    );
}

/// Unselected literal siblings are dead data: a const object destructure serializes by
/// what its pattern selects, the declaration-side half of the invariance the class-capture
/// path's `(selected key value)` rendering carries.
///
/// The refusal controls are load-bearing: anything not provably inert -- a call, a
/// computed key, a spread, a getter -- keeps the full literal, so a body whose dropped
/// property evaluates something stays distinct from one whose does not.
#[test]
fn unselected_literal_siblings_do_not_change_a_const_destructure() {
    for (name, bare, sibling) in [
        (
            "scalar",
            "const { v } = { v: 1 };",
            "const { v } = { v: 1, w: 5 };",
        ),
        (
            "string",
            "const { v } = { v: 'a' };",
            "const { v } = { v: 'a', w: 'b' };",
        ),
        (
            "nested data",
            "const { v } = { v: 1 };",
            "const { v } = { v: 1, w: { deep: 2 } };",
        ),
        (
            "scalar template",
            "const { v } = { v: 1 };",
            "const { v } = { v: 1, w: `a${1}b${'c'}` };",
        ),
    ] {
        assert_eq!(
            alpha_of(bare),
            alpha_of(sibling),
            "{name}: dead data the pattern cannot read is not identity"
        );
        assert_eq!(
            structural_of(bare),
            structural_of(sibling),
            "{name}: ...at either serialization level"
        );
    }

    // The selected value stays load-bearing: one edit (v: 1 -> v: 2) must separate them.
    assert_ne!(
        alpha_of("const { v } = { v: 1 };"),
        alpha_of("const { v } = { v: 2 };")
    );

    // Everything the erasure refuses keeps today's full-literal rendering.
    for (name, left, right) in [
        (
            "effectful value",
            "const { v } = { v: 1 };",
            "const { v } = { v: 1, w: f() };",
        ),
        (
            "computed key",
            "const { v } = { v: 1 };",
            "const { v } = { v: 1, [k]: 2 };",
        ),
        (
            "spread",
            "const { v } = { v: 1 };",
            "const { v } = { v: 1, ...rest };",
        ),
        (
            "accessor",
            "const { v } = { v: 1 };",
            "const { v } = { v: 1, get w() { return 2 } };",
        ),
        (
            "non-const declaration",
            "let { v } = { v: 1 };",
            "let { v } = { v: 1, w: 5 };",
        ),
        (
            "pattern rest",
            "const { v, ...rest } = { v: 1, w: 5 };",
            "const { v, ...rest } = { v: 1, w: 6 };",
        ),
        (
            "template coercion that can throw",
            "const { v } = { v: 1 };",
            "const { v } = { v: 1, w: `${{ toString: 0 }}` };",
        ),
        (
            "regexp template coercion",
            "const { v } = { v: 1 };",
            "const { v } = { v: 1, w: `${/x/}` };",
        ),
        (
            "selected key absent",
            "const { v } = {};",
            "const { v } = { w: 5 };",
        ),
    ] {
        assert_ne!(
            alpha_of(left),
            alpha_of(right),
            "{name}: the refusal keeps the dropped part visible"
        );
    }
}

/// The `walk_stmt`/`walk_expr` half of the same problem: an arm's job there is to *descend*,
/// so a deleted arm silently drops every event underneath it. Each row states the exact
/// event list, because a subset assertion would still pass with a missing event.
#[rustfmt::skip]
const BEHAVIOR_EVENTS: &[(&str, &str, &[&str])] = &[
    // (case name, body, the exact `behavior` event list)
    ("expression-statement", "f();", &["call:global:f"]),
    ("block", "{ f(); }", &["call:global:f"]),
    ("variable-declaration", "const v = f();", &["call:global:f"]),
    ("if", "if(f())g();", &["if", "call:global:f", "call:global:g"]),
    ("if-else", "if(1)f();else g();", &["if", "call:global:f", "call:global:g"]),
    ("for", "for(;f();g())h();", &["loop", "call:global:f", "call:global:g", "call:global:h"]),
    ("for-in", "for(const k in f())g();", &["loop", "call:global:f", "call:global:g"]),
    ("for-of", "for(const k of f())g();", &["loop", "call:global:f", "call:global:g"]),
    ("while", "while(f())g();", &["loop", "call:global:f", "call:global:g"]),
    ("do-while", "do g(); while(f());", &["loop", "call:global:g", "call:global:f"]),
    ("try", "try{f();}catch(e){g();}finally{h();}", &["try", "call:global:f", "call:global:g", "call:global:h"]),
    ("switch", "switch(f()){case g(): h();}", &["call:global:f", "call:global:g", "call:global:h"]),
    ("return", "return f();", &["return", "call:global:f"]),
    ("throw", "throw f();", &["throw", "call:global:f"]),
    ("labeled", "l: f();", &["call:global:f"]),
    ("function-declaration-is-deferred", "function g(){ f(); }", &["deferred-call:global:f"]),
    ("arrow-value-is-deferred", "const v = () => { f(); };", &["deferred-call:global:f"]),
    ("arrow-expression-value-is-deferred", "const v = () => f();", &["deferred-call:global:f"]),
    ("function-value-is-deferred", "const v = function(){ f(); };", &["deferred-call:global:f"]),
    ("static-member", "f().b;", &["call:global:f"]),
    ("computed-member", "f()[g()];", &["call:global:f", "call:global:g"]),
    ("binary", "f()+g();", &["call:global:f", "call:global:g"]),
    ("logical", "f()||g();", &["call:global:f", "call:global:g"]),
    ("unary", "!f();", &["call:global:f"]),
    ("conditional", "f()?g():h();", &["call:global:f", "call:global:g", "call:global:h"]),
    ("assignment", "o.b = f();", &["assign:= target=(member id:global:o .b) value=(call id:global:f )"]),
    ("sequence", "(f(),g());", &["call:global:f", "call:global:g"]),
    ("array", "[f()];", &["call:global:f"]),
    ("object", "const v = {a:f()};", &["call:global:f"]),
    ("parenthesized", "(f());", &["call:global:f"]),
    ("ts-as", "f() as unknown;", &["call:global:f"]),
    ("ts-satisfies", "f() satisfies unknown;", &["call:global:f"]),
    ("ts-non-null", "f()!;", &["call:global:f"]),
    ("chain-call", "o?.b(f());", &[r#"call:global:o.b args=["(call id:global:f )"]"#, "call:global:f"]),
    ("chain-static-member", "o?.b.c;", &[]),
    ("chain-computed-member", "o?.[f()];", &["call:global:f"]),
    ("call-arguments-and-callee", "f(g())(h());", &[r#"call:global:f args=["(call id:global:h )"]"#, r#"call:global:f args=["(call id:global:g )"]"#, "call:global:g", "call:global:h"]),
    ("spread-argument", "f(...g());", &[r#"call:global:f args=["...(call id:global:g )"]"#, "call:global:g"]),
];

#[test]
fn every_modeled_node_kind_is_descended_into_by_the_behavior_walk() {
    for (name, body, expected) in BEHAVIOR_EVENTS {
        let events = behavior_of(body);
        let expected: Vec<String> = expected.iter().map(|e| (*e).to_owned()).collect();
        assert_eq!(
            events, expected,
            "{name}: unexpected `behavior` for `{body}`"
        );
    }
    assert_eq!(
        async_behavior_of("await f();"),
        vec!["await".to_owned(), "call:global:f".to_owned()],
        "an `await` must be recorded and then descended into"
    );
}

/// `call_path` flattens a callee into a dotted path. Every arm it models exists so that one
/// spelling of a callee produces the *same* event as another; a deleted arm falls through to
/// `<raw source>`, which breaks exactly that equivalence.
#[test]
fn call_path_flattens_every_modeled_callee_shape() {
    #[rustfmt::skip]
    let exact: &[(&str, &str, &str)] = &[
        ("identifier", "f();", "call:global:f"),
        ("this", "this.m();", "call:this.m"),
        ("static-member", "o.b.c();", "call:global:o.b.c"),
        ("computed-member", "o[0]();", "call:global:o[]"),
        ("inner-call", "o.b().c();", "call:global:o.b.c"),
        ("chain-static-member", "o?.b();", "call:global:o.b"),
        ("chain-computed-member", "o?.[0]();", "call:global:o[]"),
    ];
    for (name, body, expected) in exact {
        let events = behavior_of(body);
        assert!(
            events.iter().any(|event| event == expected),
            "{name}: expected {expected:?} among {events:?} for `{body}`"
        );
    }

    // The unwrapping arms (`(f)()`, `(f as any)()`, `f!()`) exist so a decorated callee
    // produces the identical event to the bare one. Comparing against the bare spelling is
    // what makes a deleted arm fail: `<(f)>` is still a perfectly distinct path.
    let bare = behavior_of("f();");
    for (name, body) in [
        ("parenthesized", "(f)();"),
        ("ts-as", "(f as any)();"),
        ("ts-non-null", "f!();"),
    ] {
        assert_eq!(
            behavior_of(body),
            bare,
            "{name}: `{body}` must flatten to the same call path as `f()`"
        );
    }

    // And a callee shape with no arm at all is deliberately kept as raw text rather than
    // guessed at.
    assert!(
        behavior_of("(o ? f : g)();")
            .iter()
            .any(|event| event.starts_with("call:<")),
        "an unmodeled callee must be recorded as verbatim source"
    );
}

/// `assertion_event` recognizes `expect(...)[.modifier].matcher(...)`. Each modifier arm
/// deleted turns the whole thing back into an ordinary `call:` event, which is the difference
/// between `near-duplicate-test` seeing an assertion and seeing a method call.
#[test]
fn assertion_event_recognizes_every_modifier_spelling() {
    #[rustfmt::skip]
    let cases: &[(&str, &str, &str)] = &[
        ("plain", "expect(1).toBe(2);", "assert:expect.toBe(subject=<number>; value=<number:2>)"),
        ("not", "expect(1).not.toBe(2);", "assert:expect.not.toBe("),
        ("resolves", "expect(p).resolves.toBe(2);", "assert:expect.resolves.toBe("),
        ("rejects", "expect(p).rejects.toBe(2);", "assert:expect.rejects.toBe("),
        ("chained-modifiers", "expect(p).resolves.not.toBe(2);", "assert:expect.resolves.not.toBe("),
        ("soft", "expect.soft(1).toBe(2);", "assert:expect.soft.toBe("),
        ("poll", "expect.poll(f).toBe(2);", "assert:expect.poll.toBe("),
        ("parenthesized-base", "(expect(1)).toBe(2);", "assert:expect.toBe("),
        ("no-subject", "expect().toBe(2);", "assert:expect.toBe(subject=(); "),
    ];
    for (name, body, fragment) in cases {
        let events = behavior_of(body);
        assert!(
            events.iter().any(|event| event.starts_with(fragment)),
            "{name}: expected an event starting {fragment:?} among {events:?}"
        );
    }

    // The negative side of the same match: an unrecognized modifier is NOT an assertion, so
    // it must stay an ordinary call event rather than being silently accepted.
    for (name, body) in [
        ("unknown-modifier", "expect(1).maybe.toBe(2);"),
        ("unknown-expect-namespace", "expect.eventually(1).toBe(2);"),
        ("not-expect-at-all", "assertThat(1).toBe(2);"),
    ] {
        let events = behavior_of(body);
        assert!(
            !events.iter().any(|event| event.starts_with("assert:")),
            "{name}: `{body}` is not an `expect` assertion: {events:?}"
        );
    }

    // A local named `expect` is somebody's own binding and carries none of the meaning.
    let events = behavior_of("const expect = mk(); expect(1).toBe(2);");
    assert!(
        !events.iter().any(|event| event.starts_with("assert:")),
        "a body-local `expect` must not be treated as Vitest's: {events:?}"
    );
}

// -----------------------------------------------------------------------------------------
