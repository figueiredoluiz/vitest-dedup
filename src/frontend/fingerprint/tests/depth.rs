use super::*;

// The recursion depth guards, asserted on emitted fingerprint content
// -----------------------------------------------------------------------------------------
//
// Every `ser_*`/`walk_*` function opens with `if depth > MAX_DEPTH { ...degrade... }`. A
// mutation run over this module left 466 survivors in that one pattern -- both the guard's
// comparison (`>` -> `>=`/`==`/`<`) and every `depth + 1` recursive step (`+` -> `*`/`-`).
// They survived because nothing in the suite nested anything deeply enough for the guard to
// fire outside one synthetic `ser_expr` case, and because degradation is *symmetric*: when
// two compared bodies both degrade, no rule's verdict moves, so an end-to-end assertion
// cannot see it.
//
// The only thing that can see it is the emitted fingerprint itself, so these tests assert
// **how many levels were serialized before the fallback took over** -- a number fixed by
// `MAX_DEPTH`, the entry depth of the construct, and the depth step between levels. A guard
// that fires one level early or late, or a `depth` counter that stops advancing, changes that
// number. Asserting merely that `<max-depth:` appears somewhere would not: that is the
// "assert the guard exists" shape `scripts/check/check-coverage.mjs`'s header refuses to
// reward, and it is what the surviving mutants prove is worthless here.

/// Nesting depth generated for the matrix below. Must exceed the largest `expected_levels`
/// any case can produce (255, for a `step`-1 statement chain) so every case actually reaches
/// its guard, with margin to spare for the degraded tail to be non-empty.
const DEPTH_MATRIX_NESTING: usize = 270;

/// How many nesting levels a recursion reaches before its `depth > MAX_DEPTH` guard fires.
///
/// `entry` is the `depth` the counted function is called with for nesting level 0, and `step`
/// is how much `depth` advances between one level's counted call and the next's (2 when the
/// cycle passes through a second guarded function, 3 when it passes through two). The guard is
/// `>`, not `>=`, so a call at `depth == MAX_DEPTH` still serializes; the last level serialized
/// is therefore the largest `k` with `entry + k * step <= MAX_DEPTH`.
fn expected_levels(entry: u32, step: u32) -> usize {
    ((MAX_DEPTH - entry) / step) as usize + 1
}

/// One row of [`depth_guard_serializes_exactly_the_levels_inside_max_depth`].
struct DepthCase {
    /// Case name, quoted in every assertion message.
    name: &'static str,
    /// The test callback's body, with `@` standing for the generated chain.
    template: &'static str,
    /// Emitted once per nesting level, immediately before the level's own chain source.
    pre: &'static str,
    /// Emitted once per nesting level, immediately after it.
    post: &'static str,
    /// The innermost, non-recursive node.
    leaf: &'static str,
    /// A fragment the serializer emits exactly once per level it serialized. Must be
    /// impossible in the raw source, so a `<max-depth:...>` fallback's verbatim text can
    /// never contribute a phantom occurrence -- the test asserts that.
    token: &'static str,
    /// `depth` at which the function that emits `token` is called for nesting level 0.
    entry: u32,
    /// `depth` advance between consecutive levels' `token`-emitting calls.
    step: u32,
    /// Whether the callback has to be `async` for the chain to parse.
    asynchronous: bool,
}

/// The matrix. Each row's `entry`/`step` is the call path from `Serializer::run`, which enters
/// `ser_callable` at depth 0: `ser_stmts` at 1, the body's own `ser_stmt` at 2, a
/// `const` declarator at 3, and its initializer's `ser_expr` at 4.
const DEPTH_CASES: &[DepthCase] = &[
    // --- expressions, reached through `const v = @;` (so `ser_expr` starts at depth 4) ---
    DepthCase {
        name: "unary",
        template: "const v = @;",
        pre: "!",
        post: "",
        leaf: "1",
        token: "(unary ! ",
        entry: 4,
        step: 1,
        asynchronous: false,
    },
    DepthCase {
        // `+` is left-associative, so `1+1+1` nests without any bracket: this drives
        // `BinaryExpression`'s *left* `depth + 1`.
        name: "binary-left",
        template: "const v = @;",
        pre: "",
        post: "+1",
        leaf: "1",
        token: "(bin + ",
        entry: 4,
        step: 1,
        asynchronous: false,
    },
    DepthCase {
        // `**` is right-associative, which is how the *right* `depth + 1` of the same arm
        // gets driven deep without a parenthesis changing the step.
        name: "binary-right",
        template: "const v = @;",
        pre: "1**",
        post: "",
        leaf: "1",
        token: "(bin ** ",
        entry: 4,
        step: 1,
        asynchronous: false,
    },
    DepthCase {
        name: "logical",
        template: "const v = @;",
        pre: "",
        post: "&&1",
        leaf: "1",
        token: "(logic && ",
        entry: 4,
        step: 1,
        asynchronous: false,
    },
    DepthCase {
        // A conditional's alternate is right-associative too.
        name: "conditional",
        template: "const v = @;",
        pre: "1?1:",
        post: "",
        leaf: "1",
        token: "(cond ",
        entry: 4,
        step: 1,
        asynchronous: false,
    },
    DepthCase {
        name: "await",
        template: "const v = @;",
        pre: "await ",
        post: "",
        leaf: "1",
        token: "(await ",
        entry: 4,
        step: 1,
        asynchronous: true,
    },
    DepthCase {
        name: "assignment",
        template: "let x; const v = @;",
        pre: "x=",
        post: "",
        leaf: "1",
        token: "(assign = ",
        entry: 4,
        step: 1,
        asynchronous: false,
    },
    DepthCase {
        // `ser_expr` -> `ser_static_member` -> `ser_expr`: two guards per level.
        name: "static-member",
        template: "const v = @;",
        pre: "",
        post: ".b",
        leaf: "'a'",
        token: "(member ",
        entry: 5,
        step: 2,
        asynchronous: false,
    },
    DepthCase {
        name: "computed-member",
        template: "const v = @;",
        pre: "",
        post: "[0]",
        leaf: "'a'",
        token: "(index ",
        entry: 5,
        step: 2,
        asynchronous: false,
    },
    DepthCase {
        name: "call-callee",
        template: "const v = @;",
        pre: "",
        post: "()",
        leaf: "f",
        token: "(call ",
        entry: 5,
        step: 2,
        asynchronous: false,
    },
    DepthCase {
        // `new new f()()` is `new (new f())()`: the callee position nests.
        name: "new-callee",
        template: "const v = @;",
        pre: "new ",
        post: "()",
        leaf: "f",
        token: "(new ",
        entry: 5,
        step: 2,
        asynchronous: false,
    },
    DepthCase {
        name: "array-element",
        template: "const v = @;",
        pre: "[",
        post: "]",
        leaf: "1",
        token: "(array ",
        entry: 4,
        step: 2,
        asynchronous: false,
    },
    DepthCase {
        name: "object-property",
        template: "const v = @;",
        pre: "{a:",
        post: "}",
        leaf: "1",
        token: "(object ",
        entry: 4,
        step: 2,
        asynchronous: false,
    },
    DepthCase {
        name: "template-interpolation",
        template: "const v = @;",
        pre: "`${",
        post: "}`",
        leaf: "1",
        token: "(template ",
        entry: 5,
        step: 2,
        asynchronous: false,
    },
    DepthCase {
        // `ser_expr` -> `ser_tagged_template` -> `ser_template` -> `ser_expr`: three guards.
        name: "tagged-template",
        template: "const v = @;",
        pre: "t`${",
        post: "}`",
        leaf: "1",
        token: "(tagged ",
        entry: 5,
        step: 3,
        asynchronous: false,
    },
    DepthCase {
        // Parentheses are unwrapped, so each level costs one `ser_expr` frame that emits
        // nothing; the sequence inside it is what shows up.
        name: "sequence",
        template: "const v = @;",
        pre: "(1,",
        post: ")",
        leaf: "1",
        token: "(seq ",
        entry: 5,
        step: 2,
        asynchronous: false,
    },
    DepthCase {
        // `ser_expr`(paren) -> `ser_expr`(chain) -> `ser_chain` -> `ser_static_member`:
        // four guards per level, and the only shape that drives `ser_chain`'s own.
        name: "optional-chain",
        template: "const v = @;",
        pre: "(",
        post: "?.b)",
        leaf: "'a'",
        token: "(member ",
        entry: 7,
        step: 4,
        asynchronous: false,
    },
    DepthCase {
        name: "nested-callable",
        template: "const v = @;",
        pre: "async ()=>",
        post: "",
        leaf: "1",
        token: "(async fn () ",
        entry: 5,
        step: 2,
        asynchronous: false,
    },
    // --- binding patterns, reached through the declarator's own `ser_binding_pattern` ---
    DepthCase {
        name: "object-pattern",
        template: "const @ = o;",
        pre: "{a:",
        post: "}",
        leaf: "v",
        token: "(object-pattern ",
        entry: 4,
        step: 1,
        asynchronous: false,
    },
    DepthCase {
        name: "array-pattern",
        template: "const @ = o;",
        pre: "[",
        post: "]",
        leaf: "v",
        token: "(array-pattern ",
        entry: 4,
        step: 1,
        asynchronous: false,
    },
    // --- statements, reached directly from the body's own statement list ---
    DepthCase {
        // `ser_stmts` -> `ser_stmt` -> `ser_stmts`. The counted call at depth 1 is the
        // callback body's own statement list, so there is no occurrence outside the chain.
        name: "block",
        template: "@",
        pre: "{",
        post: "}",
        leaf: "f();",
        token: "(block ",
        entry: 1,
        step: 2,
        asynchronous: false,
    },
    DepthCase {
        name: "if",
        template: "@",
        pre: "if(1)",
        post: "",
        leaf: "f();",
        token: "(if ",
        entry: 2,
        step: 1,
        asynchronous: false,
    },
    DepthCase {
        name: "while",
        template: "@",
        pre: "while(1)",
        post: "",
        leaf: "f();",
        token: "(while ",
        entry: 2,
        step: 1,
        asynchronous: false,
    },
    DepthCase {
        name: "do-while",
        template: "@",
        pre: "do ",
        post: " while(1);",
        leaf: "f();",
        token: "(do-while ",
        entry: 2,
        step: 1,
        asynchronous: false,
    },
    DepthCase {
        name: "for",
        template: "@",
        pre: "for(;;)",
        post: "",
        leaf: "f();",
        token: "(for ",
        entry: 2,
        step: 1,
        asynchronous: false,
    },
    DepthCase {
        name: "for-in",
        template: "@",
        pre: "for(const k in o)",
        post: "",
        leaf: "f();",
        token: "(for-in ",
        entry: 2,
        step: 1,
        asynchronous: false,
    },
    DepthCase {
        name: "for-of",
        template: "@",
        pre: "for(const k of o)",
        post: "",
        leaf: "f();",
        token: "(for-of ",
        entry: 2,
        step: 1,
        asynchronous: false,
    },
    DepthCase {
        name: "for-await-of",
        template: "@",
        pre: "for await(const k of o)",
        post: "",
        leaf: "f();",
        token: "(for-await-of ",
        entry: 2,
        step: 1,
        asynchronous: true,
    },
    DepthCase {
        // `ser_stmt` -> `ser_stmts`(try block) -> `ser_stmt`.
        name: "try",
        template: "@",
        pre: "try{",
        post: "}catch(e){}",
        leaf: "f();",
        token: "(try ",
        entry: 2,
        step: 2,
        asynchronous: false,
    },
    DepthCase {
        // `ser_stmt` -> `ser_switch_case` -> `ser_stmt`.
        name: "switch-case",
        template: "@",
        pre: "switch(1){case 1:",
        post: "}",
        leaf: "f();",
        token: "(case ",
        entry: 3,
        step: 2,
        asynchronous: false,
    },
];

/// Builds one matrix case's source at `nesting` levels of recursion.
fn depth_case_source(case: &DepthCase, nesting: usize) -> String {
    let chain = format!(
        "{}{}{}",
        case.pre.repeat(nesting),
        case.leaf,
        case.post.repeat(nesting)
    );
    let body = case.template.replace('@', &chain);
    let asyncness = if case.asynchronous { "async " } else { "" };
    format!("test('t', {asyncness}() => {{ {body} }});")
}

/// The survivor-killing half of the depth work: for ~30 distinct recursion shapes, the
/// emitted `alpha` must contain **exactly** as many serialized levels as fit inside
/// `MAX_DEPTH`, and the rest must have degraded.
///
/// What makes each of these fail: change `depth > MAX_DEPTH` to `>=` or `==` and the count
/// moves by one; change it to `<` and nothing is serialized at all; replace any of the
/// arm's `depth + 1` with `depth * 1` and the counter stops advancing, so the guard never
/// fires and no level degrades; replace it with `depth - 1` and the counter runs backwards
/// into a `u32` underflow. Verified by hand on the `unary` row (see this module's
/// `depth_guard_fires_only_strictly_past_max_depth`, which pins the same boundary from both
/// sides).
#[test]
fn depth_guard_serializes_exactly_the_levels_inside_max_depth() {
    for case in DEPTH_CASES {
        let source = depth_case_source(case, DEPTH_MATRIX_NESTING);
        // Anti-vacuity: the count below is only a measurement of serialized levels if the
        // raw source -- which `degraded` copies verbatim into `<max-depth:...>` -- cannot
        // contribute an occurrence of its own.
        assert!(
            !source.contains(case.token),
            "{}: the token {:?} occurs in the source itself, so counting it measures nothing",
            case.name,
            case.token
        );
        let prints = fingerprint_all(&source);
        assert_eq!(prints.len(), 1, "{}: expected one test body", case.name);
        let alpha = &prints[0].alpha;
        let expected = expected_levels(case.entry, case.step);
        assert_eq!(
            alpha.matches(case.token).count(),
            expected,
            "{}: expected exactly {expected} serialized levels before the depth guard \
                 (entry depth {}, {} guard(s) per level, MAX_DEPTH={MAX_DEPTH}); the guard fired \
                 at the wrong level or the depth counter did not advance",
            case.name,
            case.entry,
            case.step
        );
        assert!(
            alpha.contains("<max-depth:"),
            "{}: {DEPTH_MATRIX_NESTING} levels is past MAX_DEPTH, so the tail must have \
                 degraded to raw text",
            case.name
        );
    }
}

/// The boundary, from both sides, for the deepest and widest recursion in the module.
///
/// A `const v = !!!...1` chain of `n` unary operators puts the innermost `1` at
/// `ser_expr` depth `4 + n`. The guard is `depth > MAX_DEPTH`, so `n == MAX_DEPTH - 4`
/// serializes everything and `n == MAX_DEPTH - 3` degrades exactly the leaf and nothing else.
/// Probing *just* past the edge rather than comfortably inside it is the point: this is the
/// one assertion in the module that distinguishes `>` from `>=` and from `==` directly, and
/// it also pins the exact byte content on both sides rather than the presence of a marker.
#[test]
fn depth_guard_fires_only_strictly_past_max_depth() {
    let inside = (MAX_DEPTH - 4) as usize;
    let source = format!("test('t', () => {{ const v = {}1; }});", "!".repeat(inside));
    let prints = fingerprint_all(&source);
    let alpha = &prints[0].alpha;
    assert!(
        !alpha.contains("<max-depth:"),
        "at {inside} nested unary operators the innermost literal sits at depth \
             MAX_DEPTH exactly, which the guard must still serialize"
    );
    assert_eq!(
        alpha.matches("(unary ! ").count(),
        inside,
        "every level must have been serialized"
    );
    assert!(
        // One `)` per unary level, then the four frames the body is wrapped in:
        // `(decl`, `(const`, `(block`, `(fn`.
        alpha.ends_with(&format!("<number:1>{}", ")".repeat(inside + 4))),
        "unexpected tail: {}",
        &alpha[alpha.len().saturating_sub(40)..]
    );

    let past = inside + 1;
    let source = format!("test('t', () => {{ const v = {}1; }});", "!".repeat(past));
    let prints = fingerprint_all(&source);
    let alpha = &prints[0].alpha;
    assert_eq!(
        alpha.matches("<max-depth:").count(),
        1,
        "one level past the cap exactly one node -- the innermost literal -- may degrade"
    );
    assert!(
        alpha.contains("(unary ! <max-depth:1>)"),
        "the degraded node must be the leaf, carrying its own raw source text: {}",
        &alpha[alpha.len().saturating_sub(60)..]
    );
    assert_eq!(
        alpha.matches("(unary ! ").count(),
        past,
        "all {past} unary levels are still inside the cap; only their leaf is past it"
    );
}

/// The whole emitted `alpha`, byte for byte, for a chain that runs well past the cap.
///
/// The matrix above counts levels; this pins content. It is the assertion that constrains the
/// *second* `depth + 1` in `BinaryExpression`'s arm: the right operand of the innermost
/// serialized level is a literal `1` that sits one level past the cap, so it must render as
/// `<max-depth:1>` while every shallower level's right operand renders as `<number:1>`.
/// Pinning `depth + 1` only on the left would leave the right one free.
#[test]
fn alpha_past_max_depth_emits_exact_degraded_content() {
    let nesting = DEPTH_MATRIX_NESTING;
    let levels = expected_levels(4, 1);
    let source = format!(
        "test('t', () => {{ const v = 1{}; }});",
        "+1".repeat(nesting)
    );
    let prints = fingerprint_all(&source);

    // The innermost serialized level's left child is the sub-chain starting at nesting level
    // `levels`, copied verbatim by `degraded`; its right child is the literal `1`.
    let degraded_left = format!("1{}", "+1".repeat(nesting - levels));
    let chain = format!(
        "{}<max-depth:{degraded_left}> <max-depth:1>){}",
        "(bin + ".repeat(levels),
        " <number:1>)".repeat(levels - 1)
    );
    assert!(
        prints[0].alpha.starts_with("(fn () (block (const (decl "),
        "unexpected preamble: {}",
        &prints[0].alpha[..60]
    );
    assert!(
        prints[0].alpha.ends_with(&format!("{chain}))))")),
        "the serialized chain must match byte for byte at the depth boundary"
    );

    // And the same for the two-guards-per-level member chain, whose degraded node is the
    // member expression itself rather than a leaf.
    let levels = expected_levels(5, 2);
    let source = format!(
        "test('t', () => {{ const v = 'a'{}; }});",
        ".b".repeat(nesting)
    );
    let prints = fingerprint_all(&source);
    let degraded = format!("'a'{}", ".b".repeat(nesting - levels));
    let chain = format!(
        "{}<max-depth:{degraded}>{}",
        "(member ".repeat(levels),
        " .b)".repeat(levels)
    );
    assert!(
        prints[0].alpha.ends_with(&format!("{chain}))))")),
        "the member chain must match byte for byte at the depth boundary"
    );
}

/// The `behavior` half: `walk_stmt`/`walk_expr`/`walk_call` degrade by emitting *nothing*
/// past the cap, so the observable is whether an event survives the nesting at all.
///
/// `run` enters `walk_stmt` at depth 0, so `n` nested blocks put the innermost statement at
/// `walk_stmt` depth `n`, its expression at `walk_expr` depth `n + 1`, and a call inside that
/// at `walk_call` depth `n + 2`. Each of the three guards therefore has its own boundary, and
/// each is pinned from both sides below with an event that the guarded function itself is
/// what pushes -- `if` for `walk_stmt`, `await` for `walk_expr`, `call:` for `walk_call`.
/// Nesting to only one of the three boundaries would leave the other two guards free to move
/// by a level without any assertion noticing.
#[test]
fn behavior_walk_depth_guard_records_up_to_max_depth_and_no_further() {
    let nest = |levels: usize, inner: &str, asynchronous: bool| {
        let asyncness = if asynchronous { "async " } else { "" };
        format!(
            "test('t', {asyncness}() => {{ {}{inner}{} }});",
            "{".repeat(levels),
            "}".repeat(levels)
        )
    };
    let events = |source: String| fingerprint_all(&source).swap_remove(0).behavior;

    // `walk_stmt` itself pushes `if`, at depth == the number of enclosing blocks.
    let inside = MAX_DEPTH as usize;
    assert_eq!(
        events(nest(inside, "if(1);", false)),
        vec!["if".to_owned()],
        "`walk_stmt` at depth MAX_DEPTH exactly must still record"
    );
    assert!(
        events(nest(inside + 1, "if(1);", false)).is_empty(),
        "one block deeper is past the cap for `walk_stmt`"
    );

    // `walk_expr` pushes `await`, one level below the statement that holds it.
    let inside = (MAX_DEPTH - 1) as usize;
    assert_eq!(
        events(nest(inside, "await f();", true)),
        vec!["await".to_owned()],
        "`walk_expr` at depth MAX_DEPTH exactly must still record"
    );
    assert!(
        events(nest(inside + 1, "await f();", true)).is_empty(),
        "one block deeper is past the cap for `walk_expr`"
    );

    // `walk_call` pushes `call:`, two levels below the statement.
    let inside = (MAX_DEPTH - 2) as usize;
    let recorded = events(nest(inside, "f();", false));
    assert_eq!(
        recorded.len(),
        1,
        "`walk_call` at depth MAX_DEPTH exactly must still record: {recorded:?}"
    );
    assert!(
        recorded[0].starts_with("call:"),
        "expected the call event, got {recorded:?}"
    );
    assert!(
        events(nest(inside + 1, "f();", false)).is_empty(),
        "one block deeper is past the cap for `walk_call`"
    );
}

/// The flag half of the behavior depth guards: dropping events past the cap is only safe if the
/// drop is *reported*, because a truncated trace compares equal to a different body's
/// equally-truncated trace (two 260-deep constructors calling `trace.push(1)` versus
/// `trace.unshift(2)` collided into one false `near-duplicate-test` exactly this way). Every
/// event-dropping guard must therefore set `behavior_truncated`, and no body that stays inside
/// the cap may carry the flag.
///
/// The two guards that degrade *safely* -- `call_path`'s raw-source fallback and
/// `property_chain_event`'s outright decline -- are deliberately not on this list: their
/// under-matching keeps body-specific text (or declines a canonicalization) and cannot
/// manufacture equality between two different bodies.
///
/// Nesting arithmetic (see the test above): `n` blocks put the statement at `walk_stmt` depth
/// `n`, its expression at `n + 1`, a call at `walk_call` `n + 2`, a `new` at `walk_new` `n + 2`
/// with the callee descent (`walk_under_members`) starting at `n + 3` and advancing one level
/// per member link.
#[test]
fn behavior_truncation_flag_marks_every_event_dropping_guard_and_nothing_else() {
    let nest = |levels: usize, inner: &str, asynchronous: bool| {
        let asyncness = if asynchronous { "async " } else { "" };
        format!(
            "test('t', {asyncness}() => {{ {}{inner}{} }});",
            "{".repeat(levels),
            "}".repeat(levels)
        )
    };
    let run = |source: String| {
        let mut prints = fingerprint_all(&source);
        assert_eq!(prints.len(), 1, "expected one test body");
        let print = prints.swap_remove(0);
        (print.behavior, print.behavior_truncated)
    };
    let cap = MAX_DEPTH as usize;

    // `walk_stmt` drops `if` past the cap. Its clear side cannot sit at the cap exactly: the
    // `if`'s own test expression is walked one level deeper (`walk_expr` at `n + 1`), so the
    // deepest statement whose whole subtree stays inside is two levels short of the cap.
    let (events, truncated) = run(nest(cap - 2, "if(1);", false));
    assert_eq!(events, vec!["if".to_owned()]);
    assert!(!truncated, "inside the cap, nothing was dropped");
    let (events, truncated) = run(nest(cap + 1, "if(1);", false));
    assert!(events.is_empty());
    assert!(truncated, "`walk_stmt`'s drop must set the flag");

    // `walk_expr` drops `await` one level below the statement. A literal argument keeps the
    // subtree two levels deep, so the clear side is two levels short; a call argument would cost
    // two more and conflate this guard with `walk_call`'s.
    let (events, truncated) = run(nest(cap - 2, "await 1;", true));
    assert_eq!(events.len(), 1);
    assert!(!truncated);
    let source = format!(
        "test('t', async () => {{ {}await 1;{} }});",
        "{".repeat(cap),
        "}".repeat(cap)
    );
    let (_, truncated) = run(source);
    assert!(truncated, "`walk_expr`'s drop must set the flag");

    // `walk_call` drops the whole call event two levels below the statement; its callee descent
    // starts one deeper still, so the clear side is three levels short.
    let (events, truncated) = run(nest(cap - 3, "f();", false));
    assert_eq!(events.len(), 1);
    assert!(!truncated);
    let (events, truncated) = run(nest(cap - 1, "f();", false));
    assert!(events.is_empty());
    assert!(truncated, "`walk_call`'s drop must set the flag");

    // `walk_new` drops the `new:` event two levels below the statement. Its own boundary cannot
    // be reached with the flag clear -- the callee descent it always performs starts one level
    // deeper -- so the clear side sits three levels in, where `new:` is still recorded.
    let (events, truncated) = run(nest(cap - 3, "new F();", false));
    assert_eq!(events.len(), 1, "expected the new: event: {events:?}");
    assert!(!truncated);
    let (events, truncated) = run(nest(cap - 1, "new F();", false));
    assert!(events.is_empty());
    assert!(truncated, "`walk_new`'s drop must set the flag");

    // `walk_under_members` can drop with the call event already pushed: `o.b.f()` records its
    // `call:` from `walk_call` and then descends the two member links, so a nesting that leaves
    // the call inside the cap but the descent outside sets the flag while the event survives.
    // The descent re-checks depth once per loop iteration -- two links plus the base identifier
    // make three checks -- so the clear side needs the whole chain five levels inside.
    let (events, truncated) = run(nest(cap - 5, "o.b.f();", false));
    assert_eq!(events.len(), 1, "expected the call: event: {events:?}");
    assert!(!truncated);
    let (events, truncated) = run(nest(cap - 2, "o.b.f();", false));
    assert_eq!(
        events.len(),
        1,
        "the call itself is inside the cap: {events:?}"
    );
    assert!(
        truncated,
        "`walk_under_members`' abandoned descent must set the flag"
    );

    // `call_path`'s raw-source fallback is NOT separately observable from the flag: a member
    // callee chain deep enough to degrade the rendered path is walked link-for-link by
    // `walk_under_members` too, whose guard fires on the same link and sets the flag. That
    // co-occurrence is the conservative direction -- a trace whose callee text degraded is
    // under-matched, and marking its body unresolved only loses comparisons on input no human
    // writes -- so the case is recorded here rather than contorted around.
}

/// The defect itself, at the unit boundary: what the collector could not tell apart deep,
/// it must flag as incomparable rather than report as equal.
///
/// `trace.push(1)` versus `trace.unshift(2)` inside a constructor is discriminating behavior at
/// shallow depth -- the two `call:` events differ -- and indistinguishable at 260 levels of
/// unbraced `if`, where the depth cap eats the region on both sides and leaves two equal traces
/// of `if` tokens. The equality is an artifact; the flag is what says so.
#[test]
fn behavior_truncation_flags_the_equal_traces_the_depth_cap_manufactures() {
    let sink_probe = |depth: usize, effect: &str| {
        format!(
            "test('t', () => {{ const trace: number[] = []; if (true) {}{{ \
               class Sink {{ constructor() {{ trace.{effect}; }} }} \
               new Sink(); expect(trace).toEqual([1]); }} }});",
            "if (true) ".repeat(depth - 1)
        )
    };
    let run = |source: String| {
        let mut prints = fingerprint_all(&source);
        assert_eq!(prints.len(), 1);
        let print = prints.swap_remove(0);
        (print.behavior, print.behavior_truncated)
    };

    // Shallow: the constructor effects are collected and the traces differ.
    let (push_events, push_truncated) = run(sink_probe(2, "push(1)"));
    let (unshift_events, unshift_truncated) = run(sink_probe(2, "unshift(2)"));
    assert!(!push_truncated && !unshift_truncated);
    assert_ne!(
        push_events, unshift_events,
        "at shallow depth the collector must tell push from unshift"
    );

    // Deep: both traces truncate to the same `if` tokens and would compare equal -- the flag is
    // the only honest thing left on the run.
    let (push_events, push_truncated) = run(sink_probe(260, "push(1)"));
    let (unshift_events, unshift_truncated) = run(sink_probe(260, "unshift(2)"));
    assert!(push_truncated && unshift_truncated);
    assert_eq!(
        push_events, unshift_events,
        "the truncated traces are equal -- that equality is the depth-cap artifact, and the flag is \
         what keeps any rule from reading it as similarity"
    );
    assert!(
        push_events.iter().all(|event| event == "if"),
        "nothing past the cap may survive: {push_events:?}"
    );
}

// -----------------------------------------------------------------------------------------

// Every remaining guarded recursion, landed on exactly
// -----------------------------------------------------------------------------------------
//
// The matrix above drives whole cycles past `MAX_DEPTH`, which reaches most guards but not
// the ones whose function is called once per construct rather than once per level. Those are
// reached by padding with exactly as many nested `!` (or `if (1)`) links as it takes to land
// the construct's own guarded call on `MAX_DEPTH + 1`, and asserting the degraded text is
// that construct's verbatim source -- so the guard is observed doing its job at the precise
// node, not merely existing.

/// One row: the construct's source, how many guarded frames sit between the padding's own
/// `ser_expr`/`ser_stmt` call and the guard under test, and the raw text that must degrade.
#[rustfmt::skip]
const LANDED_GUARDS: &[(&str, &str, u32, &str)] = &[
    // (name, construct source, frames below the construct's own entry, degraded raw text)
    ("ser_call", "f(1)", 1, "f(1)"),
    ("ser_argument", "f(1)", 2, "1"),
    ("ser_new", "new F(1)", 1, "new F(1)"),
    ("ser_static_member", "o.b", 1, "o.b"),
    ("ser_computed_member", "o[0]", 1, "o[0]"),
    ("ser_chain", "o?.b", 1, "o?.b"),
    ("ser_template", "`q${1}`", 1, "`q${1}`"),
    ("ser_tagged_template", "t`q`", 1, "t`q`"),
    ("ser_instantiation", "f<number>", 1, "f<number>"),
    ("ser_array_element", "[1]", 1, "1"),
    ("ser_object_property", "{a:1}", 1, "a:1"),
    // A computed key's span is the key expression itself, not its brackets.
    ("ser_property_key", "{[k]:1}", 2, "k"),
    // These five need their own parentheses to sit under a `!` at all, and the
    // parenthesized wrapper is one extra (unwrapping, but still guarded) `ser_expr` frame.
    ("ser_callable", "(()=>1)", 2, "()=>1"),
    ("ser_params", "((a)=>1)", 3, "(a)"),
    ("ser_binding_pattern", "((a)=>1)", 4, "a"),
    ("ser_assignment_target", "(o.b=1)", 2, "o.b"),
    ("ser_simple_assignment_target", "(o.b++)", 2, "o.b"),
];

/// Statement-level guards, padded with nested `if (1)` instead of `!`.
#[rustfmt::skip]
const LANDED_STATEMENT_GUARDS: &[(&str, &str, u32, &str)] = &[
    ("ser_stmts", "{1;}", 1, "1;"),
    ("ser_declarator", "const a = 1;", 1, "a = 1"),
    ("ser_catch", "try{1;}catch(e){}", 1, "catch(e){}"),
    ("ser_switch_case", "switch(1){case 2:}", 1, "case 2:"),
    ("ser_for_left", "for(const k in o)1;", 1, "const k"),
];

#[test]
fn every_guarded_recursion_degrades_at_exactly_max_depth_plus_one() {
    // `const v = <padding><construct>;` puts the construct's own `ser_expr` at depth
    // `4 + padding`, so the guard `offset` frames below it fires when
    // `4 + padding + offset == MAX_DEPTH + 1`.
    for (name, construct, offset, degraded) in LANDED_GUARDS {
        let padding = (MAX_DEPTH + 1 - 4 - offset) as usize;
        let source = format!(
            "test('t', () => {{ const v = {}{construct}; }});",
            "!".repeat(padding)
        );
        let alpha = fingerprint_all(&source).swap_remove(0).alpha;
        let expected = format!("<max-depth:{degraded}>");
        assert!(
            alpha.contains(&expected),
            "{name}: expected {expected:?} -- the guard must fire on this node and no \
                 shallower one. Tail: {}",
            &alpha[alpha.len().saturating_sub(120)..]
        );
    }

    // `<padding><statement>` puts the statement's own `ser_stmt` at depth `2 + padding`.
    for (name, construct, offset, degraded) in LANDED_STATEMENT_GUARDS {
        let padding = (MAX_DEPTH + 1 - 2 - offset) as usize;
        let source = format!(
            "test('t', () => {{ {}{construct} }});",
            "if(1)".repeat(padding)
        );
        let alpha = fingerprint_all(&source).swap_remove(0).alpha;
        let expected = format!("<max-depth:{degraded}>");
        assert!(
            alpha.contains(&expected),
            "{name}: expected {expected:?}. Tail: {}",
            &alpha[alpha.len().saturating_sub(120)..]
        );
    }
}

/// The purity allowlist is depth-capped too, and it degrades toward "has an effect" -- the
/// safe direction, since that only declines a canonicalization rather than claiming a false
/// equivalence. An intervening statement nested past the cap must therefore block inlining
/// even though every node in it is individually pure.
#[test]
fn the_purity_allowlist_degrades_toward_having_an_effect_past_max_depth() {
    let deep_statement_nesting = format!(
        "{}1;{}",
        "{".repeat(MAX_DEPTH as usize + 2),
        "}".repeat(MAX_DEPTH as usize + 2)
    );
    let subject = assertion_subject(&inlining_probe(&deep_statement_nesting));
    assert!(
        subject.starts_with("id:L"),
        "statement nesting past MAX_DEPTH must be treated as effectful, got {subject:?}"
    );

    let deep_expression_nesting = format!("const p = 1{};", "+1".repeat(MAX_DEPTH as usize + 2));
    let subject = assertion_subject(&inlining_probe(&deep_expression_nesting));
    assert!(
        subject.starts_with("id:L"),
        "expression nesting past MAX_DEPTH must be treated as effectful, got {subject:?}"
    );

    // The control: the same shapes just inside the cap stay pure, so the two assertions
    // above are about the depth and not about the shape.
    let shallow = format!("{}1;{}", "{".repeat(8), "}".repeat(8));
    assert_eq!(
        assertion_subject(&inlining_probe(&shallow)),
        "(call id:global:build <number>)",
        "shallow nesting of the same pure shape must still inline"
    );
}

// -----------------------------------------------------------------------------------------
