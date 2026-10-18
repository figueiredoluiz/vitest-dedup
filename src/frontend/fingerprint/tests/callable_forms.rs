use super::*;

// Triviality
// -----------------------------------------------------------------------------------------

/// `trivial` for a test whose callback body is the single expression `expr`.
fn trivial_of_expression(expr: &str) -> bool {
    let prints = fingerprint_all(&format!("test('t', () => {expr});"));
    assert_eq!(prints.len(), 1);
    prints[0].trivial
}

/// `trivial` for a test whose callback has a block body.
fn trivial_of_block(body: &str) -> bool {
    let prints = fingerprint_all(&format!("test('t', () => {{ {body} }});"));
    assert_eq!(prints.len(), 1);
    prints[0].trivial
}

/// Trivial bodies are dropped from comparison upstream, so both directions of this predicate
/// are load-bearing: a false positive silences a real test, a false negative compares a stub
/// against every other stub. Every row is one edit away from the other list.
#[test]
fn triviality_recognizes_exactly_the_stub_shapes() {
    #[rustfmt::skip]
    let trivial_expressions: &[&str] = &[
        "null",
        "undefined",
        "void 0",
        "(null)",
        "((undefined))",
        "Promise.resolve()",
        "todo()",
        "pending()",
        "notImplemented()",
        "not_implemented()",
        "o.todo()",
        "expect(true).toBe(true)",
    ];
    for expr in trivial_expressions {
        assert!(
            trivial_of_expression(expr),
            "`() => {expr}` is a recognized stub and must be trivial"
        );
    }
    assert!(
        {
            let prints = fingerprint_all("test('t', async () => await null);");
            prints[0].trivial
        },
        "`async () => await null` is a stub behind an await"
    );

    #[rustfmt::skip]
    let real_expressions: &[&str] = &[
        "1",                          // not a stub at all
        "other",                      // an identifier that is not `undefined`
        "void 1",                     // `void`, but not of `0`
        "void 'x'",                   // `void`, but not of a number
        "!0",                         // numeric 0, but not behind `void`
        "Promise.resolve(1)",         // the marker call, but with an argument
        "Promise.reject()",           // `Promise`, but not `.resolve`
        "Q.resolve()",                // `.resolve`, but not on `Promise`
        "expect(true).toBe(false)",   // the stub assertion's shape, wrong expected value
        "expect(false).toBe(true)",   // ...wrong subject
        "expect(true).toEqual(true)", // ...wrong matcher
        "check(true)",                // a call whose name is not a pending marker
    ];
    for expr in real_expressions {
        assert!(
            !trivial_of_expression(expr),
            "`() => {expr}` is one edit away from a stub but is not one, so it must be \
                 compared like any other body"
        );
    }

    #[rustfmt::skip]
    let trivial_blocks: &[&str] = &[
        "",                              // no statements at all
        "return;",                       // a bare `return`
        "return null;",                  // a returned stub
        "return expect(true).toBe(true);",
        "null;",                         // a stub expression statement
        "expect(true).toBe(true);",
    ];
    for body in trivial_blocks {
        assert!(
            trivial_of_block(body),
            "`() => {{ {body} }}` must be trivial"
        );
    }

    #[rustfmt::skip]
    let real_blocks: &[&str] = &[
        "f();",
        "return 1;",
        "null; null;",     // two statements: never trivial, whatever they are
        "return; return;",
    ];
    for body in real_blocks {
        assert!(
            !trivial_of_block(body),
            "`() => {{ {body} }}` must not be trivial"
        );
    }
}

// -----------------------------------------------------------------------------------------
// `await`/`.then` canonicalization
// -----------------------------------------------------------------------------------------

/// `then_continuation`'s preconditions, each one asserted by the event list it changes.
///
/// Every arm of that function is a precondition rather than a parsing convenience, so each
/// negative row below states a shape that must *not* be canonicalized -- a rejection handler
/// observes a code path a bare `await` never can, a chained `.then` is not "a single awaited
/// call", and a callback taking the resolved value is not something this module correlates.
#[test]
fn a_single_awaited_call_written_as_a_then_chain_canonicalizes_to_await() {
    let awaited = vec![
        "await".to_owned(),
        "call:global:f".to_owned(),
        "call:global:g".to_owned(),
    ];
    #[rustfmt::skip]
    let canonicalized: &[(&str, &str)] = &[
        ("returned-arrow-callback", "return f().then(() => { g(); });"),
        ("returned-function-callback", "return f().then(function(){ g(); });"),
        ("inner-callee-is-a-method", "return o.load().then(() => { g(); });"),
    ];
    for (name, body) in canonicalized {
        let events = behavior_of(body);
        let expected = if *name == "inner-callee-is-a-method" {
            vec![
                "await".to_owned(),
                "call:global:o.load".to_owned(),
                "call:global:g".to_owned(),
            ]
        } else {
            awaited.clone()
        };
        assert_eq!(
            events, expected,
            "{name}: `{body}` must read as one awaited call"
        );
    }

    // The expression-bodied-arrow twin of the `return` form: `() => f().then(cb)`.
    let prints = fingerprint_all("test('t', () => f().then(() => { g(); }));");
    assert_eq!(
        prints[0].behavior, awaited,
        "an expression-bodied arrow gets the same canonicalization as `return`"
    );

    #[rustfmt::skip]
    let left_alone: &[(&str, &str)] = &[
        ("rejection-handler", "return f().then(() => { g(); }, h);"),
        ("chained-then", "return f().then(() => { }).then(() => { g(); });"),
        ("callback-takes-the-value", "return f().then((r) => { g(); });"),
        ("callback-has-a-rest-parameter", "return f().then((...r) => { g(); });"),
        ("expression-bodied-callback", "return f().then(() => g());"),
        ("object-is-not-a-call", "return p.then(() => { g(); });"),
        ("no-callback-at-all", "return f().then();"),
        ("not-then", "return f().catch(() => { g(); });"),
    ];
    for (name, body) in left_alone {
        let events = behavior_of(body);
        assert!(
            !events.first().is_some_and(|event| event == "await"),
            "{name}: `{body}` is not a single awaited call and must keep its own shape: \
                 {events:?}"
        );
        assert_eq!(
            events.first().map(String::as_str),
            Some("return"),
            "{name}: ...which starts with the `return` it is: {events:?}"
        );
    }
}

// -----------------------------------------------------------------------------------------

// The `function` callback form
// -----------------------------------------------------------------------------------------

/// Finds the first `test(...)`/`it(...)` callback written as a `function` expression rather
/// than an arrow, and fingerprints it. The whole [`FunctionLike::Function`] half of this
/// module -- its span, its statement list, its `this`/`arguments` binding, its triviality --
/// is only reachable this way.
struct FunctionBodyCollector<'a, 'b> {
    semantic: &'b Semantic<'a>,
    source: &'b str,
    results: Vec<BodyFingerprint>,
}

impl<'a> Visit<'a> for FunctionBodyCollector<'a, '_> {
    /// Collect ordinary-function test callback fingerprints while traversing fixture calls.
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let is_test_call = matches!(&it.callee, Expression::Identifier(ident) if matches!(ident.name.as_str(), "test" | "it"));
        if is_test_call {
            if let Some(Argument::FunctionExpression(function)) = it.arguments.get(1) {
                self.results.push(fingerprint(
                    FunctionLike::Function(function),
                    self.semantic,
                    self.source,
                    Path::new("case.test.ts"),
                    None,
                ));
            }
        }
        walk::walk_call_expression(self, it);
    }
}

/// Parse a source fixture and collect fingerprints for ordinary-function test callbacks.
fn fingerprint_function_bodies(source: &str) -> Vec<BodyFingerprint> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(std::path::Path::new("case.test.ts")).unwrap();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    assert!(!parsed.fatal_error, "fixture failed to parse: {source}");
    let program: &Program = &parsed.program;
    let semantic = SemanticBuilder::new().build(program).semantic;
    let mut collector = FunctionBodyCollector {
        semantic: &semantic,
        source,
        results: Vec::new(),
    };
    collector.visit_program(program);
    collector.results
}

/// A `function () {}` callback must fingerprint exactly like the equivalent arrow, except
/// where the two genuinely differ: a `function` binds its own `this`/`arguments`, so a body
/// that observes either prints the specific form instead of the interchangeable `fn`.
#[test]
fn a_function_callback_is_fingerprinted_like_its_arrow_twin() {
    let arrow = fingerprint_all("test('t', () => { const x = f(); g(x); });");
    let function = fingerprint_function_bodies("test('t', function () { const x = f(); g(x); });");
    assert_eq!(
        function.len(),
        1,
        "expected the function-expression callback"
    );
    assert_eq!(
        function[0].alpha, arrow[0].alpha,
        "the two callback spellings are interchangeable when neither observes `this`"
    );
    assert_eq!(
        function[0].behavior, arrow[0].behavior,
        "...including the dataflow canonicalization over its statement list"
    );

    // `this` and `arguments` are what make the forms differ, and only inside a `function`.
    let prints = fingerprint_function_bodies("test('t', function () { g(this); });");
    assert!(
        prints[0].alpha.starts_with("(function-fn () "),
        "a `function` that observes `this` must print its own form: {}",
        prints[0].alpha
    );
    let prints = fingerprint_function_bodies("test('t', function () { g(arguments); });");
    assert!(
        prints[0].alpha.starts_with("(function-fn () "),
        "...and the same for `arguments`: {}",
        prints[0].alpha
    );

    // Triviality, the `Function` branch: an empty body and a stub body.
    let prints = fingerprint_function_bodies("test('t', function () {});");
    assert!(prints[0].trivial, "an empty `function` body is trivial");
    let prints = fingerprint_function_bodies("test('t', function () { return null; });");
    assert!(prints[0].trivial, "a stubbed `function` body is trivial");
    let prints = fingerprint_function_bodies("test('t', function () { f(); });");
    assert!(!prints[0].trivial, "a real `function` body is not trivial");
}

/// A TypeScript overload signature has no body at all, which every one of this module's
/// statement-list consumers has to survive.
#[test]
fn an_overload_signature_has_no_body_to_serialize() {
    let alpha = alpha_of("function h(a: number): void; function h(a: any): void { f(); }");
    assert!(
        alpha.contains("(no-body)"),
        "the signature declares no statements to compare: {alpha}"
    );
    assert!(
        alpha.contains("(function-decl L0 (fn (L1) (no-body)))"),
        "...and its declaration and parameter bindings are still compared: {alpha}"
    );
    assert!(
        alpha.contains("(block (stmt (call id:global:f )))"),
        "...while the implementation's own body is serialized as usual: {alpha}"
    );
}

// -----------------------------------------------------------------------------------------
