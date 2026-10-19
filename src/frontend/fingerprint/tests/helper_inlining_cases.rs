use super::*;

/// A `tsconfig.json` `paths` alias must resolve to the same identity as the equivalent
/// relative import -- the case lexical anchoring, which never looked at `tsconfig.json` at
/// all, could never handle.
#[test]
fn free_import_identity_matches_a_tsconfig_paths_alias_to_the_equivalent_relative_import() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("tsconfig.json"),
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@app/*":["src/*"]}}}"#,
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src").join("helpers.ts"),
        "export const helper = 1;",
    )
    .unwrap();

    let via_alias = "import { helper } from '@app/helpers';\ntest('x', () => { helper(1); });";
    let via_relative = "import { helper } from './helpers';\ntest('x', () => { helper(1); });";

    let alias = fingerprint_all_at(via_alias, &dir.path().join("src").join("a.test.ts"));
    let relative = fingerprint_all_at(via_relative, &dir.path().join("src").join("b.test.ts"));
    assert_eq!(alias.len(), 1);
    assert_eq!(relative.len(), 1);
    assert_eq!(
        alias[0].alpha, relative[0].alpha,
        "a tsconfig `paths` alias and the equivalent relative import must resolve to the \
             same module and produce the same alpha"
    );
}

/// Resolution touches the filesystem, but fingerprinting must stay deterministic --
/// running the same real-resolution setup twice must produce byte-identical fingerprints.
#[test]
fn free_import_identity_with_real_resolution_is_deterministic_across_repeated_runs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("helpers.ts"), "export const helper = 1;").unwrap();
    let source = "import { helper } from './helpers';\ntest('x', () => { helper(1); });";
    let file = dir.path().join("a.test.ts");

    let first = fingerprint_all_at(source, &file);
    let second = fingerprint_all_at(source, &file);
    assert_eq!(
        first, second,
        "two runs over the same tree must produce identical fingerprints"
    );
}

/// A7 regression: an adversarial review proved that keying a module-level (or outer-closure)
/// free identity on the bare name alone makes two files that each declare an unrelated
/// `const helper = ...` with different bodies collapse to the same `alpha` -- a false
/// `duplicate-test` between genuinely different code. Two separate `Allocator`/`Semantic`
/// instances, two different `file` paths, structurally identical test bodies aside from
/// which `helper` they close over: the fix must tell them apart.
#[test]
fn free_module_identity_is_scoped_to_its_own_file() {
    let source_a = "const helper = x => x + 1;\n\
                         test('increments', () => { expect(helper(1)).toBe(2); });";
    let source_b = "const helper = x => x * 1000;\n\
                         test('increments', () => { expect(helper(1)).toBe(2); });";
    let a = fingerprint_all_at(source_a, Path::new("a.test.ts"));
    let b = fingerprint_all_at(source_b, Path::new("b.test.ts"));
    assert_eq!(a.len(), 1);
    assert_eq!(b.len(), 1);
    assert_ne!(
        a[0].alpha, b[0].alpha,
        "two files that each declare their own, differently-behaved `helper` must not \
             produce the same alpha merely because the name matches"
    );
}

/// Reproduces a false positive observed on a large external corpus: sibling
/// `describe` blocks each declaring their own `const config = ...` with a different value,
/// and textually identical test bodies referencing `config` in each. Scoping `free:outer` by
/// file and name alone (the previous fix) could not tell these apart, because both bindings
/// share a file *and* a name -- only their declaration spans differ. This must not be a
/// duplicate: the bodies mean different things.
#[test]
fn free_outer_identity_distinguishes_sibling_scope_bindings_of_the_same_name() {
    let source = "\
            describe('page == 1', () => {\n\
                const config = 1;\n\
                test('renders', () => { expect(config).toBe(1); });\n\
            });\n\
            describe('page == last-page', () => {\n\
                const config = 2;\n\
                test('renders', () => { expect(config).toBe(1); });\n\
            });\n\
        ";
    let prints = fingerprint_all(source);
    assert_eq!(prints.len(), 2);
    assert_ne!(
        prints[0].alpha, prints[1].alpha,
        "two sibling scopes each declaring their own `config` binding must not collapse \
             just because the test bodies are textually identical and the name matches -- they \
             are different bindings with different values"
    );
}

/// The positive control for the regression above: two tests *in the same scope*, referencing
/// the *same* `config` binding, with identical bodies, must still be recognized as
/// duplicates. The fix for sibling scopes must not overcorrect into never matching anything.
#[test]
fn free_outer_identity_matches_within_the_same_scope() {
    let source = "\
            describe('page == 1', () => {\n\
                const config = 1;\n\
                test('a', () => { expect(config).toBe(1); });\n\
                test('b', () => { expect(config).toBe(1); });\n\
            });\n\
        ";
    let prints = fingerprint_all(source);
    assert_eq!(prints.len(), 2);
    assert_eq!(
        prints[0].alpha, prints[1].alpha,
        "two tests in the same scope referencing the same outer `config` binding, with \
             identical bodies, must still match"
    );
}

#[test]
fn literal_variation_matches_structurally_but_not_alpha() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/duplicates/not-duplicates-literal-variation.test.ts"
    ))
    .expect("fixture present");
    let prints = fingerprint_all(&source);
    assert_eq!(prints.len(), 2);
    assert_ne!(
        prints[0].alpha, prints[1].alpha,
        "different literal values must differ under alpha"
    );
    assert_eq!(
        prints[0].structural, prints[1].structural,
        "literal kind-collapsing must unify them structurally"
    );
}

#[test]
fn empty_body_is_trivial() {
    let prints = fingerprint_all("test('x', () => {});");
    assert_eq!(prints.len(), 1);
    assert!(prints[0].trivial);
}

#[test]
fn lone_stub_call_is_trivial() {
    let prints = fingerprint_all("test('x', () => { todo(); });");
    assert!(prints[0].trivial);
}

#[test]
fn expect_true_to_be_true_is_trivial() {
    let prints = fingerprint_all("test('x', () => { expect(true).toBe(true); });");
    assert!(prints[0].trivial);
}

#[test]
fn real_assertion_is_not_trivial() {
    let prints = fingerprint_all("test('x', () => { expect(1).toBe(1); });");
    assert!(!prints[0].trivial);
}

#[test]
fn determinism_across_repeated_runs() {
    let source = "test('x', () => { const a = compute(1); expect(a).toBe(1); });";
    let first = fingerprint_all(source);
    let second = fingerprint_all(source);
    assert_eq!(first, second);
}

/// The substrate both depth-guard tests serialize their synthetic ASTs on: a trivial
/// one-test parse, empty maps, a fresh Alpha serializer. A macro rather than a function
/// because the serializer borrows four locals that must outlive it, and the two tests
/// differ in what they measure around the `ser_expr` call itself.
macro_rules! with_trivial_serializer {
    (| $serializer:ident | $body:expr) => {{
        let trivial_allocator = Allocator::default();
        let trivial_source = "test('x', () => {});";
        let trivial_type = SourceType::from_path(std::path::Path::new("trivial.test.ts")).unwrap();
        let trivial_parsed = Parser::new(&trivial_allocator, trivial_source, trivial_type).parse();
        let trivial_semantic = SemanticBuilder::new()
            .build(&trivial_parsed.program)
            .semantic;
        let imports = ImportOrigins::new();
        let row_substitution = HashMap::new();
        let dataflow = HashMap::new();
        let mut $serializer = Serializer::new(
            FingerprintContext::new(
                &trivial_semantic,
                trivial_source,
                &imports,
                "trivial.test.ts".to_owned(),
                PathBuf::from("trivial.test.ts"),
                SPAN,
                NodeId::ROOT,
                &row_substitution,
                &dataflow,
            ),
            Mode::Alpha,
        );
        $body
    }};
}

/// A source-text version of "deeply nested input" is deliberately **not** used here.
///
/// Investigation (prompted by a coordinator report that the original version of this test
/// stack-overflowed) traced the crash to `oxc_parser` itself: parsing ~1000+ levels of
/// nested parenthesized expressions overflows the stack of the thread doing the parsing, on
/// a default-size thread, before this module's code ever sees an AST. Proof: parsing the
/// same source and then `std::mem::forget`-ing both the parse result and the `Allocator`
/// (skipping every destructor) still overflows, which rules out this module's traversal and
/// rules out `Drop` glue on the arena AST -- it isolates the crash to the parse call itself.
/// Confirms as ordinary deep recursion rather than a runaway loop: raising the test
/// thread's stack via `RUST_MIN_STACK` makes the same input parse successfully.
///
/// That is a real gap, but it is `extract.rs`'s to close (bound nesting depth, or reject
/// pathologically deep input, before calling `Parser::parse`), not this module's: by the
/// time `fingerprint` is ever called, the parse and semantic analysis already succeeded, so
/// there is no source-text input this module could construct that would exercise its own
/// depth guard via the real parser without first tripping the parser's own, much lower,
/// limit. So this test builds the deep AST directly with `AstBuilder`, bypassing the parser
/// entirely, to test the one thing this module actually owns: that `Serializer::ser_expr`'s
/// explicit depth counter stops well short of the real call stack, for an AST deeper than
/// the parser could ever hand it in practice.
#[test]
fn ser_expr_depth_guard_bounds_an_ast_deeper_than_the_parser_could_produce() {
    let allocator = Allocator::default();
    let builder = AstBuilder::new(&allocator);
    let mut expr = Expression::new_boolean_literal(SPAN, true, &builder);
    let synthetic_depth = MAX_DEPTH as usize * 8;
    for _ in 0..synthetic_depth {
        expr = Expression::new_unary_expression(SPAN, UnaryOperator::LogicalNot, expr, &builder);
    }

    // `Serializer` requires a `Semantic`, but this synthetic expression contains no
    // identifiers, so it is never consulted for symbol resolution; any valid `Semantic`
    // will do, and a trivially shallow parse is a safe way to obtain one.
    // Must not panic (stack overflow or otherwise), and must actually have engaged the
    // guard rather than trivially succeeding -- proving the cap is both safe and reachable.
    let output = with_trivial_serializer!(|serializer| serializer.ser_expr(&expr, 0));
    assert!(
        output.contains("max-depth"),
        "expected the MAX_DEPTH guard to trigger well before {synthetic_depth} nested \
             unary expressions: {output}"
    );
}

/// The other half of the depth guard, and the one it was missing: a depth cap bounds stack
/// use only together with a per-frame cost, and nothing measured that product. It measures
/// itself now -- `record_stack_low_water_mark` records the lowest address `ser_expr` reaches,
/// so the full-depth cost is an observable number rather than an assumption, and a widened
/// match arm that eats the margin fails here instead of aborting some unrelated test.
///
/// See [`MAX_DEPTH`]'s doc for where [`MAX_DEPTH_STACK_BUDGET_BYTES`] comes from.
#[test]
fn ser_expr_at_max_depth_stays_within_its_stack_budget() {
    let allocator = Allocator::default();
    let builder = AstBuilder::new(&allocator);
    let mut expr = Expression::new_boolean_literal(SPAN, true, &builder);
    // Exactly deep enough to drive `ser_expr` to its own guard: every level below the cap is
    // a real frame, and the cap stops the rest. A deeper AST costs no more stack.
    for _ in 0..(MAX_DEPTH as usize + 2) {
        expr = Expression::new_unary_expression(SPAN, UnaryOperator::LogicalNot, expr, &builder);
    }

    let (_output, used) = with_trivial_serializer!(|serializer| {
        STACK_LOW_WATER_MARK.with(|cell| cell.set(usize::MAX));
        let caller_frame = 0u8;
        let caller_address = std::ptr::addr_of!(caller_frame) as usize;
        let output = serializer.ser_expr(&expr, 0);
        let deepest = STACK_LOW_WATER_MARK.with(|cell| cell.get());
        assert!(
            output.contains("max-depth"),
            "the measurement is only meaningful if the full depth was actually walked: {output}"
        );
        assert!(
            deepest < caller_address,
            "the probe must have recorded a frame below this one"
        );
        (output, caller_address - deepest)
    });
    assert!(
        used <= MAX_DEPTH_STACK_BUDGET_BYTES,
        "a full-depth `ser_expr` consumed {used} bytes of stack, over the \
             {MAX_DEPTH_STACK_BUDGET_BYTES}-byte budget ({} bytes per frame at MAX_DEPTH={}). \
             Either shrink the frame (hoist the arm into its own method, as `ser_new`/`ser_tagged` \
             already are) or lower MAX_DEPTH -- do not raise this budget without re-deriving it \
             from the smallest stack this code runs on; see MAX_DEPTH's doc.",
        used / (MAX_DEPTH as usize + 1),
        MAX_DEPTH
    );
}

#[test]
fn behavior_records_fluent_chain_and_control_flow() {
    let prints = fingerprint_all(
        "test('x', async () => { \
                if (ready) { await page.locator('#go').click(); } \
                expect(page.url).toContain('done'); \
            });",
    );
    let behavior = &prints[0].behavior;
    assert!(
        behavior.iter().any(|event| event == "if"),
        "behavior: {behavior:?}"
    );
    assert!(
        behavior.iter().any(|event| event == "await"),
        "behavior: {behavior:?}"
    );
    assert!(
        behavior
            .iter()
            .any(|event| event.contains(".click") || event.ends_with("click")),
        "expected a click-shaped event, got {behavior:?}"
    );
    assert!(
        behavior.iter().any(|event| event.starts_with("assert:")),
        "behavior: {behavior:?}"
    );
}

#[test]
fn spread_arguments_are_part_of_behavior_identity() {
    let prints = fingerprint_all(
        "test('admin', () => { render(...adminProps); expect(screen.text()).toBe('ok'); });\n\
             test('guest', () => { render(...guestProps); expect(screen.text()).toBe('ok'); });",
    );
    assert_eq!(prints.len(), 2);
    assert_ne!(
        prints[0].behavior, prints[1].behavior,
        "spread arguments can be the scenario under test; collapsing every spread to `...` \
             erases that difference"
    );
}

#[test]
fn callback_arguments_are_part_of_behavior_identity() {
    let prints = fingerprint_all(
        "test('clock in', () => { registerEndpoint('/recent', () => ({ data: [{ type: 'clock_in' }] })); render(); expect(status.text()).toBe('勤務外'); });\n\
             test('clock out', () => { registerEndpoint('/recent', () => ({ data: [{ type: 'clock_out' }] })); render(); expect(status.text()).toBe('勤務外'); });",
    );
    assert_eq!(prints.len(), 2);
    assert_ne!(
        prints[0].behavior, prints[1].behavior,
        "inline setup callbacks can be the data under test; collapsing them to plain `fn` \
             makes different mocked responses share one near-duplicate behavior multiset"
    );
    assert!(
        prints[0]
            .behavior
            .iter()
            .any(|event| event.contains("clock_in")),
        "expected callback return data to survive in behavior: {:?}",
        prints[0].behavior
    );
}

#[test]
fn assignments_are_part_of_behavior_identity() {
    let prints = fingerprint_all(
        "test('loading', () => { state.loading = true; render(); expect(screen.text()).toBe('-'); });\n\
             test('empty', () => { state.items = []; render(); expect(screen.text()).toBe('-'); });",
    );
    assert_eq!(prints.len(), 2);
    assert_ne!(
        prints[0].behavior, prints[1].behavior,
        "setup writes are often the branch under test; erasing the assignment target/value \
             makes different branches share one near-duplicate behavior multiset"
    );
    assert!(
        prints[0]
            .behavior
            .iter()
            .any(|event| event.contains("assign:=") && event.contains(".loading")),
        "expected the assigned target to survive in behavior: {:?}",
        prints[0].behavior
    );
    assert!(
        prints[0]
            .behavior
            .iter()
            .any(|event| event.contains("<boolean:true>")),
        "expected the assigned value to survive in behavior: {:?}",
        prints[0].behavior
    );
}

#[test]
fn updates_are_part_of_behavior_identity() {
    let events = behavior_of("counter.value++; expect(counter.value).toBe(1);");
    assert!(
        events
            .iter()
            .any(|event| event.contains("update:postfix++") && event.contains(".value")),
        "expected the update target and operator to survive in behavior: {events:?}"
    );
}

#[test]
fn ordinary_call_arguments_are_part_of_behavior_identity() {
    let prints = fingerprint_all(
        "test('name', () => { wrapper.find('.name-text'); });\n\
             test('unit', () => { wrapper.find('.unit-value'); });",
    );
    assert_eq!(prints.len(), 2);
    assert_ne!(
        prints[0].behavior, prints[1].behavior,
        "a project-owned query or assertion helper's arguments can be the test's entire \
             observation and must not disappear from behavior"
    );
    assert!(
        prints[0]
            .behavior
            .iter()
            .any(|event| event.contains(".name-text")),
        "the canonical argument must be carried by the event: {:?}",
        prints[0].behavior
    );
}

/// A destructured property name is part of the behavior identity of the call it feeds.
#[test]
fn destructured_property_names_are_part_of_behavior_identity() {
    let prints = fingerprint_all(
        "test('every', () => { const { isEveryTag } = useThing(); isEveryTag(); });\n\
             test('initial', () => { const { isSameAsInitial } = useThing(); \
                 isSameAsInitial(); });",
    );
    assert_eq!(prints.len(), 2);
    assert_ne!(
        prints[0].behavior, prints[1].behavior,
        "alpha-renamed locals must retain the destructured property that says what the call \
             observes"
    );
    assert!(prints[0]
        .behavior
        .iter()
        .any(|event| event.contains(":prop:isEveryTag")));
    assert!(prints[1]
        .behavior
        .iter()
        .any(|event| event.contains(":prop:isSameAsInitial")));
}

/// A destructured callable that is never reassigned fingerprints like the direct member access.
#[test]
fn safe_destructured_callable_still_matches_direct_member_access() {
    let prints = fingerprint_all(
        "import { service } from './service';\n\
             test('destructured', () => { const { refresh } = service; refresh('now'); });\n\
             test('member', () => { service.refresh('now'); });",
    );
    assert_eq!(prints.len(), 2);
    assert_eq!(
        prints[0].behavior, prints[1].behavior,
        "preserving property identity must keep the existing safe destructure-to-member \
             canonicalization"
    );
}

// -----------------------------------------------------------------------------------------

/// A raw slice cannot be checked against a semantic model that has no node table, so the body
/// that holds one is marked unresolved instead of being compared on its text alone.
#[test]
fn a_raw_slice_without_a_node_table_marks_the_body_unresolved() {
    with_trivial_serializer!(|serializer| {
        assert!(!serializer.ctx.class_uncertain.get());
        let text = serializer.opaque(SPAN);
        assert_eq!(text, "<opaque:>");
        assert!(serializer.ctx.class_uncertain.get());
    });
}
