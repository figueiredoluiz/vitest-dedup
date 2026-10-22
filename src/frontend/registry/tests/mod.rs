/// The `skipIf` condition of a resolved declaration's first step -- the thing every
/// cross-file condition-identity test compares.
fn skip_if_condition(resolved: &ResolvedCallee) -> &String {
    let ChainStep::SkipIf { condition, .. } = &resolved.steps[0] else {
        panic!("expected a SkipIf step");
    };
    condition
}

/// Parses one standalone expression and runs `body` with it -- a macro because the
/// expression borrows the parser's arena, which must outlive it.
macro_rules! with_parsed_expression {
    ($source:expr, |$expr:ident| $body:expr) => {{
        let allocator = Allocator::default();
        let source_type = SourceType::from_path("x.ts").unwrap();
        let $expr = Parser::new(&allocator, $source, source_type)
            .parse_expression()
            .expect("parses");
        $body
    }};
}

/// Helper mirroring the existing const-fold tests.
fn fold_tagged(source: &str) -> Option<String> {
    with_parsed_expression!(source, |expr| const_fold_tagged(&expr))
}
use super::*;
use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;

/// The census counts CELLS, not analyses: one cell's refusals deduplicate within a file,
/// and a new file's identical offsets count again -- spans restart at zero per source
/// (Copilot review, PR #28 round 2). Both halves of that sentence in one test, because they
/// share one file's census state.
#[test]
fn the_census_counts_each_refused_cell_once_and_resets_per_file_scope() {
    let span = |start, end| oxc_span::Span::new(start, end);
    super::reset_cell_census_file_scope();

    // Within one file: the same span is one cell, however many times the Walker rebuilds
    // the table around it.
    assert!(mark_census_cell(span(10, 20)));
    assert!(
        !mark_census_cell(span(10, 20)),
        "same span twice is one cell"
    );
    record_census_refusal("call".to_owned());

    // A new file resets the scope: identical offsets are a DIFFERENT cell there.
    super::reset_cell_census_file_scope();
    assert!(
        mark_census_cell(span(10, 20)),
        "spans restart per file, so the reset must re-admit them"
    );
    record_census_refusal("call".to_owned());
    assert_eq!(take_cell_census(), vec![("call".to_owned(), 2)]);
    assert!(take_cell_census().is_empty(), "taking the census drains it");

    // `take_cell_census` drains; leave the scope drained too.
    super::reset_cell_census_file_scope();
}

// -----------------------------------------------------------------------------------------
// T1 census: the histogram machinery, exercised with the gate on (C5's coverage round).
// -----------------------------------------------------------------------------------------

/// Regression fixture for framework registration: cell refusal cause names every shape it
/// digs into.
#[test]
fn cell_refusal_cause_names_every_shape_it_digs_into() {
    let cause_of =
        |source: &str| with_parsed_expression!(source, |expr| cell_refusal_cause(&expr, None));
    assert_eq!(cause_of("mk()"), "call");
    assert_eq!(cause_of("new Maker()"), "new");
    assert_eq!(cause_of("() => 1"), "function");
    assert_eq!(cause_of("(function f() {})"), "function");
    // A template with one unreadable substitution names the substitution's own cause.
    assert_eq!(cause_of("`a${mk()}b`"), "template: call");
    // One with every substitution readable is the template itself.
    assert_eq!(cause_of("`a${1}b`"), "template");
    // Arrays: a spread has no enumerable elements; an unreadable element names it; a fully
    // readable array is just an array.
    assert_eq!(cause_of("[...rows]"), "array: spread or hole");
    assert_eq!(cause_of("[[...rows]]"), "array: array: spread or hole");
    assert_eq!(cause_of("[1, mk()]"), "array: call");
    assert_eq!(cause_of("[1, 2]"), "array");
    // Objects: a computed key (or spread) refuses the fields; an unreadable value names it.
    assert_eq!(
        cause_of("{ [k]: 1 }"),
        "object: computed key, spread, shorthand, or method"
    );
    assert_eq!(cause_of("{ a: mk() }"), "object: call");
    assert_eq!(cause_of("{ a: 1, b: 2 }"), "object");
    // Bare identifiers and static members, with no semantic to resolve through, name
    // themselves; a refusal that survives RESOLUTION through a binding is annotated.
    assert_eq!(cause_of("MODE"), "identifier");
    assert_eq!(cause_of("LEVELS.high"), "static member");
    // Everything else the fold refuses is "other" (a binary over unreadable operands, a
    // tagged template, ...).
    assert_eq!(cause_of("a === b"), "other");
}

/// Regression fixture for framework registration: the census records refusals end to end
/// when enabled.
#[test]
fn the_census_records_refusals_end_to_end_when_enabled() {
    with_test_census(|| {
        assert!(census_enabled());
        let titles = extracted_titles("const N = mk();\ntest.each([[N], [2]])('t %s', () => {});");
        assert_eq!(titles.len(), 2);
        assert_eq!(take_cell_census(), vec![("identifier".to_owned(), 1)]);
        assert!(take_cell_census().is_empty());
    });
    assert!(!census_enabled(), "the test override is restored");
}

/// Regression fixture for framework registration: census file scope is isolated from other
/// threads.
#[test]
fn census_file_scope_is_isolated_from_other_threads() {
    with_test_census(|| {
        let span = oxc_span::Span::new(10, 20);
        assert!(mark_census_cell(span));
        record_census_refusal("call".to_owned());
        std::thread::spawn(|| {
            assert!(!census_enabled());
            assert_eq!(extracted_titles("test('other', () => {});").len(), 1);
            assert!(take_cell_census().is_empty());
        })
        .join()
        .expect("independent extraction");
        assert!(
            !mark_census_cell(span),
            "another thread cannot reset this file's spans"
        );
        assert_eq!(take_cell_census(), vec![("call".to_owned(), 1)]);
    });
}

/// Regression fixture for framework registration: census override restores state after
/// unwinding.
#[test]
fn census_override_restores_state_after_unwinding() {
    record_census_refusal("previous".to_owned());
    let result = std::panic::catch_unwind(|| {
        with_test_census(|| {
            record_census_refusal("temporary".to_owned());
            panic!("abort the temporary extraction");
        })
    });
    assert!(result.is_err());
    assert!(!census_enabled());
    assert_eq!(take_cell_census(), vec![("previous".to_owned(), 1)]);
}

/// Extracts one source with globals enabled and returns the rendered titles -- the shared
/// driver for the const-binding and invocation-climb batteries below.
fn extracted_titles(source: &str) -> Vec<crate::model::Title> {
    crate::frontend::extract::extract(
        source,
        &crate::discovery::SourceFile {
            path: std::path::PathBuf::from("case.test.ts"),
            language: crate::discovery::SourceLanguage::TypeScript,
            module_flavor: crate::discovery::ModuleFlavor::Ambiguous,
            config_scope: None,
            project_globals: None,
        },
        Globals::Enabled,
    )
    .tests
    .into_iter()
    .map(|test| test.title)
    .collect()
}

/// Require one extracted title and report whether it remains parameterized.
fn all_parameterized(titles: &[crate::model::Title]) -> bool {
    assert_eq!(titles.len(), 1, "each refusal probe must extract one test");
    matches!(titles[0], crate::model::Title::Parameterized(_))
}

/// Regression fixture for framework registration: refusal checks reject missing
/// declarations.
#[test]
#[should_panic(expected = "each refusal probe must extract one test")]
fn refusal_checks_reject_missing_declarations() {
    all_parameterized(&[]);
}

/// Regression fixture for framework registration: cell renderers preserve numbers and
/// refuse unknown coercions.
#[test]
fn cell_renderers_preserve_numbers_and_refuse_unknown_coercions() {
    for (source, expected) in [
        ("10n", None),
        ("mk()", None),
        ("'0x10'", Some(16.0)),
        ("[null]", Some(0.0)),
    ] {
        with_parsed_expression!(source, |expr| {
            assert_eq!(js_number_value(&expr, 0, None), expected);
        });
    }
    with_parsed_expression!("1", |expr| {
        assert_eq!(js_number_value(&expr, MAX_CELL_DEPTH, None), Some(1.0));
        assert_eq!(js_number_value(&expr, MAX_CELL_DEPTH + 1, None), None);
        assert!(array_elements(&expr).is_none());
        assert!(object_fields(&expr).is_none());
        assert!(!inherits_default_string_coercion(&expr));
        assert_eq!(fold_signed_number(&expr, None), None);
    });
    with_parsed_expression!("mk()", |expr| {
        assert_eq!(render_json(&expr, 0, None), None);
    });
    assert_eq!(
        json_string("\0\u{001f}\n\r\t\"\\"),
        "\"\\u0000\\u001f\\n\\r\\t\\\"\\\\\""
    );
}

/// Regression fixture for framework registration: parsed parentheses preserve the
/// underlying cell value.
#[test]
fn parsed_parentheses_preserve_the_underlying_cell_value() {
    let allocator = Allocator::default();
    let expr = Parser::new(&allocator, "((1))", SourceType::from_path("x.ts").unwrap())
        .with_options(oxc_parser::ParseOptions {
            preserve_parens: true,
            ..Default::default()
        })
        .parse_expression()
        .unwrap();
    assert!(matches!(expr, Expression::ParenthesizedExpression(_)));
    assert_eq!(const_fold(unwrap_parens(&expr), None), Some("1".to_owned()));
}

/// Regression fixture for framework registration: census labels distinguish resolved
/// bindings.
#[test]
fn census_labels_distinguish_resolved_bindings() {
    for (source, expected) in [
        ("const N = 1; N;", "identifier (through a binding)"),
        (
            "const LEVELS = { high: 1 }; LEVELS.high;",
            "static member (through a binding)",
        ),
    ] {
        let allocator = Allocator::default();
        let program = parse(&allocator, source);
        let semantic = SemanticBuilder::new()
            .with_build_nodes(true)
            .build(&program)
            .semantic;
        let Statement::ExpressionStatement(statement) = program.body.last().unwrap() else {
            panic!("the fixture ends with the reference being classified");
        };
        assert_eq!(
            cell_refusal_cause(&statement.expression, Some(&semantic)),
            expected
        );
    }
}

/// T1 refusal arms, unit-level: each shape below must refuse, exercising one specific
/// refusal arm in `resolved_binding`/`object_is_only_member_read`/`is_immutable_value`.
#[test]
fn const_binding_refusal_arms_are_all_reachable() {
    let refuses = |source: &str| {
        all_parameterized(&extracted_titles(&format!(
            "import {{ test }} from 'vitest';\n{source}"
        )))
    };
    // 1197: a `const` redeclared by a merged TypeScript shape has two declarations.
    assert!(refuses(
        "interface MODE { x: 1 }\nconst MODE = 'x';\ntest.each([[MODE]])('runs %s', () => {});"
    ));
    assert!(refuses(
        "const MODE = !true;\ntest.each([[MODE]])('runs %s', () => {});"
    ));
    assert_eq!(
        extracted_titles("const N = +5; test.each([[N]])('n %s', () => {});"),
        vec![crate::model::Title::Static("n 5".to_owned())]
    );
    // 1251: the hop bound -- nine chained consts.
    let mut chain = String::new();
    chain.push_str("const C0 = 'x';\n");
    for i in 1..10 {
        chain.push_str(&format!("const C{i} = C{};\n", i - 1));
    }
    chain.push_str("test.each([[C9]])('runs %s', () => {});");
    assert!(refuses(&chain), "the hop bound refuses the tenth link");
    // 1277: a member read whose OBJECT is not an identifier (a call result).
    assert!(refuses(
        "const L = make();\ntest.each([[L.high]])('n %d', () => {});"
    ));
    // 1304: a spread in the const object could define or shadow the read key.
    assert!(refuses(
        "const L = { ...base, high: 3 };\ntest.each([[L.high]])('n %d', () => {});"
    ));
    // 1346: an escape to a declarator initializer (`const other = LEVELS`).
    assert!(refuses("const LEVELS = { high: 3 };\nconst other = LEVELS;\ntest.each([[LEVELS.high]])('n %d', () => {});"));
    // 1366-1370: a negated const initializer is an immutable UNARY value.
    assert_eq!(
        extracted_titles("import { test } from 'vitest';\nconst N = 5;\nconst M = -N;\ntest.each([[M]])('n %s', () => {});"),
        vec![crate::model::Title::Static("n -5".to_owned())]
    );
}

/// C5-era invocation-climb arms: every member-chain shape that can mutate the object
/// through `this` must refuse the member read.
#[test]
fn member_chain_invocation_arms_are_all_reachable() {
    // Every member is a DATA property: a method shorthand would refuse all member reads
    // off the object at the property scan, which is a different (already-covered) rule.
    // The invocation climbs below are the arms under test here.
    let decl = "const LEVELS = { high: 3, mutate: function () { this.high = 4 }, make: function () { return 1 } };\n";
    // Deep climb: two member links, then a call.
    assert!(all_parameterized(&extracted_titles(
        &(format!("{decl}LEVELS.wrap.mutate();\ntest.each([[LEVELS.high]])('n %d', () => {{}});"))
    )));
    // Computed member nested after a static member: the whole chain binds `this`.
    assert!(all_parameterized(&extracted_titles(&format!(
        "{decl}LEVELS.wrap['mutate']();\ntest.each([[LEVELS.high]])('n %d', () => {{}});"
    ))));
    // Computed-member climb.
    assert!(all_parameterized(&extracted_titles(
        &(format!("{decl}const k = 'mutate';\nLEVELS[k]();\ntest.each([[LEVELS.high]])('n %d', () => {{}});"))
    )));
    // Parenthesized callee.
    assert!(all_parameterized(&extracted_titles(
        &(format!("{decl}(LEVELS.mutate)();\ntest.each([[LEVELS.high]])('n %d', () => {{}});"))
    )));
    // NewExpression callee. `make` is a DATA property holding a function -- a method
    // shorthand would refuse every member read off the object before the climb runs.
    assert!(all_parameterized(&extracted_titles(
        &(format!("{decl}new (LEVELS.make)();\ntest.each([[LEVELS.high]])('n %d', () => {{}});"))
    )));
    // The happy path stays green: a plain member read over an object with no invoking
    // reference resolves.
    assert_eq!(
        extracted_titles(&format!(
            "{decl}test.each([[LEVELS.high]])('n %d', () => {{}});"
        )),
        vec![crate::model::Title::Static("n 3".to_owned())]
    );
}

/// Regression fixture for framework registration: row identity is exact for positional
/// named elided and unresolved rows.
#[test]
fn row_identity_is_exact_for_positional_named_elided_and_unresolved_rows() {
    let value = |kind, raw: &str| {
        Some(CellValue {
            display: raw.to_owned(),
            kind,
            raw: raw.to_owned(),
        })
    };
    let row = |cells, unresolved| Row {
        display: String::new(),
        cells,
        title_args: Vec::new(),
        unresolved,
    };

    assert_eq!(
        row(
            RowCells::Positional(vec![value("number", "1"), None]),
            false
        )
        .identity()
        .as_deref(),
        Some("P2:V6:number1:1E")
    );
    assert_eq!(
        row(
            RowCells::Named(vec![("role".to_owned(), value("string", "admin"))]),
            false
        )
        .identity()
        .as_deref(),
        Some("N1:4:roleV6:string5:admin")
    );
    assert_eq!(
        row(RowCells::Positional(vec![value("number", "1")]), true).identity(),
        None
    );
}

/// The length prefixes in [`Row::identity`] are what stop one row forging another's.
///
/// Both pairs below are ordinary `.each` tables, and each pair's two rows serialize to the
/// *same* text if the prefixes are dropped and the parts simply concatenated -- which is
/// what an unprefixed scheme, or any fixed separator, would produce. Derived by construction
/// from the format rather than guessed: a row's parts are `V`, the kind, the raw and (for a
/// named row) the field name, all of which a test author picks, so the boundary between them
/// is forgeable unless it is measured. They are rows Vitest binds differently, so collapsing
/// a pair is a `duplicate-test` at error severity on correct code.
#[test]
fn row_identity_cannot_be_forged_by_moving_text_across_a_boundary() {
    // `Vstring` is the exact punctuation-plus-kind an unprefixed encoding would emit between
    // two cells, so a cell whose *value* spells it can absorb the next cell's opening.
    // Unprefixed, both of these are `P2:VstringaVstringbVstringc`.
    let two_cells =
        each_rows("test.each([['aVstringb', 'c'], ['a', 'bVstringc']])('%s %s', () => {});");
    assert_eq!(two_cells.len(), 2);
    assert_ne!(
        two_cells[0].identity(),
        two_cells[1].identity(),
        "a cell boundary is part of the identity, not only the text either side of it"
    );

    // The same move across a *field name* boundary: an empty field name lets the previous
    // value grow by exactly the name it swallowed. Unprefixed, both are
    // `N2:aVstringpqVstringb`.
    let two_fields =
        each_rows("test.each([{ a: 'p', q: 'b' }, { a: 'pq', '': 'b' }])('$a', () => {});");
    assert_eq!(two_fields.len(), 2);
    assert_ne!(
        two_fields[0].identity(),
        two_fields[1].identity(),
        "a field-name boundary is part of the identity too"
    );

    // Positional and named addressing are different bindings whatever the contents, so the
    // two families can never meet even before any part is compared.
    let positional = each_rows("test.each([['a']])('%s', () => {});");
    let named = each_rows("test.each([{ x: 'a' }])('$x', () => {});");
    assert_ne!(
        positional[0].identity(),
        named[0].identity(),
        "positional and named rows never share an identity"
    );
}

/// Parse a TypeScript fixture and return its program for registry tests.
fn parse<'a>(allocator: &'a Allocator, source: &'a str) -> Program<'a> {
    let source_type = SourceType::from_path("file.test.ts").unwrap();
    Parser::new(allocator, source, source_type).parse().program
}

/// Resolves the callee of the first `CallExpression` encountered in source order.
///
/// Every test below only has one call worth inspecting, so a single small visitor (rather
/// than threading borrowed AST references out through a return value, which the borrow
/// checker will not allow across the arena's lifetime) is enough.
struct FirstCallResolver<'r, 's> {
    registry: &'r Registry,
    semantic: &'r Semantic<'s>,
    source: &'r str,
    file: &'r Path,
    resolved: Option<ResolvedCallee>,
}

impl<'a, 'r, 's> Visit<'a> for FirstCallResolver<'r, 's> {
    /// Resolve calls until one callee succeeds, then retain that first successful
    /// resolution.
    fn visit_call_expression(&mut self, it: &oxc_ast::ast::CallExpression<'a>) {
        if self.resolved.is_none() {
            self.resolved = resolve_callee(
                &it.callee,
                self.registry,
                self.semantic,
                self.source,
                self.file,
            );
        }
        walk::walk_call_expression(self, it);
    }
}

/// Resolve the first readable call in the standard fixture file under the requested globals
/// mode.
fn resolve_first_call(source: &str, globals: Globals) -> Option<ResolvedCallee> {
    resolve_first_call_in_file(source, globals, Path::new("file.test.ts"))
}

/// Same as [`resolve_first_call`], but lets a test pick the file path -- needed to prove an
/// identity is (or is not) scoped by file, which a fixed path can never exercise.
fn resolve_first_call_in_file(
    source: &str,
    globals: Globals,
    file: &Path,
) -> Option<ResolvedCallee> {
    let allocator = Allocator::default();
    let program = parse(&allocator, source);
    let semantic = SemanticBuilder::new().build(&program).semantic;
    let registry = Registry::new(&program, globals);
    let mut resolver = FirstCallResolver {
        registry: &registry,
        semantic: &semantic,
        source,
        file,
        resolved: None,
    };
    resolver.visit_program(&program);
    resolver.resolved
}

// HAZARD 4 / basic case: a named import from "vitest" resolves a bare `test(...)` call.
/// Regression fixture for framework registration: named import resolves bare test call.
#[test]
fn named_import_resolves_bare_test_call() {
    let resolved = resolve_first_call(
        r#"import { test } from "vitest"; test("x", () => {});"#,
        Globals::Disabled,
    )
    .expect("test(...) should resolve");
    assert_eq!(resolved.root, "test");
    assert!(resolved.steps.is_empty());
}

// Aliased imports: `it`/`suite` canonicalize, and a renamed local (`t`) still resolves.
/// Regression fixture for framework registration: renamed import resolves through its local
/// binding.
#[test]
fn renamed_import_resolves_through_its_local_binding() {
    let resolved = resolve_first_call(
        r#"import { test as t } from "vitest"; t("x", () => {});"#,
        Globals::Disabled,
    )
    .expect("t(...) should resolve");
    assert_eq!(resolved.root, "test");
}

/// Namespace imports: `vitest.test(...)` resolves the same as a named import.
#[test]
fn namespace_import_resolves_member_access() {
    let resolved = resolve_first_call(
        r#"import * as vitest from "vitest"; vitest.describe("x", () => {});"#,
        Globals::Disabled,
    )
    .expect("vitest.describe(...) should resolve");
    assert_eq!(resolved.root, "describe");
}

/// HAZARD 3: globals are opt-in per call site, matching the caller's resolved per-project flag.
#[test]
fn bare_identifier_only_resolves_when_globals_enabled() {
    let source = r#"describe("x", () => { test("y", () => {}); });"#;
    assert!(resolve_first_call(source, Globals::Disabled).is_none());
    let resolved = resolve_first_call(source, Globals::Enabled).expect("globals enabled");
    assert_eq!(resolved.root, "describe");
}

/// HAZARD 4: an import from a competing framework must never resolve, regardless of globals.
#[test]
fn import_from_other_framework_does_not_resolve() {
    let source = r#"import { test } from "@playwright/test"; test("x", async ({ page }) => {});"#;
    assert!(resolve_first_call(source, Globals::Disabled).is_none());
    assert!(resolve_first_call(source, Globals::Enabled).is_none());
}

// HAZARD 10: a local variable named `test` that shadows the real import must not resolve --
// this must hold via binding identity, not by the identifier's spelling.
/// Regression fixture for framework registration: shadowed local test does not resolve.
#[test]
fn shadowed_local_test_does_not_resolve() {
    let source = r#"
        import { test as t } from "vitest";
        const test = { name: "helper" };
        test.name;
        t("uses the real one", () => {});
    "#;
    let allocator = Allocator::default();
    let program = parse(&allocator, source);
    let semantic = SemanticBuilder::new().build(&program).semantic;
    let registry = Registry::new(&program, Globals::Disabled);

    struct IdentifierResolutions<'r> {
        registry: &'r Registry,
        scoping: &'r Scoping,
        resolutions: Vec<(String, bool)>,
    }
    impl<'a, 'r> Visit<'a> for IdentifierResolutions<'r> {
        /// Record whether each test-named identifier reference resolves through the fixture
        /// registry.
        fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'a>) {
            if it.name.as_str() == "test" {
                let resolved = resolve_identifier(it, self.registry, self.scoping);
                self.resolutions
                    .push((it.name.as_str().to_owned(), resolved.is_some()));
            }
        }
    }
    let mut finder = IdentifierResolutions {
        registry: &registry,
        scoping: semantic.scoping(),
        resolutions: Vec::new(),
    };
    finder.visit_program(&program);
    // The only bare `test` reference is `test.name`, which must resolve to nothing -- it is
    // the shadowing local, not the vitest import (which is only reachable through `t`).
    assert_eq!(finder.resolutions, vec![("test".to_owned(), false)]);
}

/// `test.each(table)` is a call whose callee is itself a call: the `.each` member must resolve
/// off the *inner* call, and the table rows must be enumerable.
#[test]
fn each_call_of_a_call_resolves_with_literal_rows() {
    let resolved = resolve_first_call(
        r#"import { test } from "vitest"; test.each([[1, 2], [3, 4]])("case %i", (a, b) => {});"#,
        Globals::Disabled,
    )
    .expect("test.each(...)(...) should resolve");
    assert_eq!(resolved.root, "test");
    let ChainStep::Each { is_for, rows } = &resolved.steps[0] else {
        panic!("expected an Each step, got {:?}", resolved.steps);
    };
    assert!(!is_for);
    let rows = rows.as_ref().expect("literal array table has known rows");
    assert_eq!(rows.len(), 2);
}

/// HAZARD 5: a non-literal `.each` table must degrade to "unresolved", not zero rows.
#[test]
fn each_with_nonliteral_table_has_no_rows() {
    let resolved = resolve_first_call(
        r#"import { test } from "vitest"; test.each(getCases())("x", () => {});"#,
        Globals::Disabled,
    )
    .expect("should still resolve as an each declaration");
    let ChainStep::Each { rows, .. } = &resolved.steps[0] else {
        panic!("expected an Each step");
    };
    assert!(
        rows.is_err(),
        "a non-literal table must not report zero rows"
    );
}

/// Two different unreadable `.each` tables must be told apart by what is knowable about each
/// expression, not collapsed to the same "unresolved" marker.
#[test]
fn two_different_unreadable_tables_have_different_identities() {
    let a = resolve_first_call(
        r#"import { test } from "vitest"; test.each(getCasesA())("x", () => {});"#,
        Globals::Disabled,
    )
    .expect("should still resolve as an each declaration");
    let b = resolve_first_call(
        r#"import { test } from "vitest"; test.each(getCasesB())("x", () => {});"#,
        Globals::Disabled,
    )
    .expect("should still resolve as an each declaration");
    let ChainStep::Each { rows: rows_a, .. } = &a.steps[0] else {
        panic!("expected an Each step");
    };
    let ChainStep::Each { rows: rows_b, .. } = &b.steps[0] else {
        panic!("expected an Each step");
    };
    let identity_a = rows_a.as_ref().expect_err("a call is not a literal table");
    let identity_b = rows_b.as_ref().expect_err("a call is not a literal table");
    assert_ne!(
        identity_a, identity_b,
        "two calls to different helpers must not report the same unreadable identity"
    );
}

/// The same unreadable table, resolved twice, must report the same identity both times --
/// otherwise a rule that groups on it could never match a table against itself.
#[test]
fn the_same_unreadable_table_has_a_stable_identity() {
    let source = r#"import { test } from "vitest"; test.each(getCases())("x", () => {});"#;
    let first = resolve_first_call(source, Globals::Disabled)
        .expect("should still resolve as an each declaration");
    let second = resolve_first_call(source, Globals::Disabled)
        .expect("should still resolve as an each declaration");
    let ChainStep::Each { rows: rows_a, .. } = &first.steps[0] else {
        panic!("expected an Each step");
    };
    let ChainStep::Each { rows: rows_b, .. } = &second.steps[0] else {
        panic!("expected an Each step");
    };
    assert_eq!(
        rows_a.as_ref().expect_err("a call is not a literal table"),
        rows_b.as_ref().expect_err("a call is not a literal table"),
        "the same unreadable table must report the same identity every time"
    );
}

/// `canonicalize` maps the `it` and `suite` aliases to `test` and `describe` and rejects any other name.
#[test]
fn canonicalizes_aliases() {
    assert_eq!(canonicalize("it"), Some("test"));
    assert_eq!(canonicalize("suite"), Some("describe"));
    assert_eq!(canonicalize("bogus"), None);
}

/// HAZARD 6 / fixture #7: a const-foldable template substitution resolves statically, but this
/// extractor never claims to reproduce Vitest's own (version-dependent) runtime rendering
/// beyond values it can prove from source.
#[test]
fn const_fold_handles_simple_arithmetic() {
    assert_eq!(
        with_parsed_expression!("1 + 1", |expr| const_fold(&expr, None)),
        Some("2".to_owned())
    );
}

/// The tagged fold keeps literals of different kinds apart even when their raw text is the same.
#[test]
fn tagged_folding_separates_literals_that_share_raw_text() {
    // The defect this guards: untagged folding rendered all of these as the same string, so
    // two `test.extend` fixtures bound to different-kinded literals produced an equivalent
    // context and an error-severity duplicate on correct code.
    let number = fold_tagged("1000").expect("number folds");
    let string = fold_tagged("'1000'").expect("string folds");
    assert_ne!(number, string, "numeric 1000 must not equal string '1000'");

    let boolean = fold_tagged("true").expect("boolean folds");
    let boolean_text = fold_tagged("'true'").expect("string folds");
    assert_ne!(
        boolean, boolean_text,
        "boolean true must not equal string 'true'"
    );

    let null_literal = fold_tagged("null").expect("null folds");
    let null_text = fold_tagged("'null'").expect("string folds");
    assert_ne!(
        null_literal, null_text,
        "null must not equal the string 'null'"
    );
}

/// Control for the tagged fold: identical literals still fold to equal values.
#[test]
fn tagged_folding_still_matches_identical_literals() {
    // Positive control: the fix must not make every fixture context unequal, which would
    // silence the rule rather than correct it.
    assert_eq!(fold_tagged("1000"), fold_tagged("1000"));
    assert_eq!(fold_tagged("'a'"), fold_tagged("'a'"));
}

/// The untagged fold keeps the raw literal form that `.each` titles interpolate.
#[test]
fn untagged_folding_is_unchanged_for_title_rendering() {
    // `.each` titles interpolate the raw form; `<number:1000>` in a title would be nonsense.
    // The two functions exist precisely because one output cannot serve both consumers.
    let allocator = Allocator::default();
    let source_type = SourceType::from_path("x.ts").unwrap();
    let expr = Parser::new(&allocator, "1000", source_type)
        .parse_expression()
        .expect("parses");
    assert_eq!(const_fold(&expr, None), Some("1000".to_owned()));
}

/// A runtime identifier is not a value `const_fold` can prove.
#[test]
fn const_fold_returns_none_for_runtime_identifiers() {
    assert_eq!(
        with_parsed_expression!("runtimeVar", |expr| const_fold(&expr, None)),
        None
    );
}

// Defect 5: two sibling `const enabled` bindings with opposite values must never fingerprint
// identically just because they share a name -- that is exactly what let a false
// `duplicate-test` fire at error severity on genuinely different `skipIf` conditions.
/// Regression fixture for framework registration: defect5 sibling bindings with the same
/// name fingerprint differently.
#[test]
fn defect5_sibling_bindings_with_the_same_name_fingerprint_differently() {
    let source = r#"
        import { test } from "vitest";
        function withA() {
            const enabled = true;
            test.skipIf(enabled)("in a", () => {});
        }
        function withB() {
            const enabled = false;
            test.skipIf(enabled)("in b", () => {});
        }
    "#;
    let allocator = Allocator::default();
    let program = parse(&allocator, source);
    let semantic = SemanticBuilder::new().build(&program).semantic;
    let registry = Registry::new(&program, Globals::Disabled);
    let file = Path::new("file.test.ts");

    struct AllCallResolver<'r, 's> {
        registry: &'r Registry,
        semantic: &'r Semantic<'s>,
        source: &'r str,
        file: &'r Path,
        resolved: Vec<ResolvedCallee>,
    }
    impl<'a, 'r, 's> Visit<'a> for AllCallResolver<'r, 's> {
        /// Collect every successfully resolved call while traversing the fixture program.
        fn visit_call_expression(&mut self, it: &oxc_ast::ast::CallExpression<'a>) {
            if let Some(resolved) = resolve_callee(
                &it.callee,
                self.registry,
                self.semantic,
                self.source,
                self.file,
            ) {
                self.resolved.push(resolved);
            }
            walk::walk_call_expression(self, it);
        }
    }
    let mut resolver = AllCallResolver {
        registry: &registry,
        semantic: &semantic,
        source,
        file,
        resolved: Vec::new(),
    };
    resolver.visit_program(&program);

    let conditions: Vec<(String, bool)> = resolver
        .resolved
        .iter()
        .filter_map(|resolved| {
            resolved.steps.iter().find_map(|step| match step {
                ChainStep::SkipIf {
                    condition,
                    unresolved,
                } => Some((condition.clone(), *unresolved)),
                _ => None,
            })
        })
        .collect();
    assert_eq!(
        conditions.len(),
        2,
        "expected exactly two skipIf(...) calls resolved"
    );
    assert_ne!(
        conditions[0].0, conditions[1].0,
        "sibling `enabled` bindings must not fingerprint the same"
    );
    assert!(
        !conditions[0].1 && !conditions[1].1,
        "a bare identifier bound to a local declaration is resolvable, not unresolved"
    );
}

/// A composite condition carries exact syntax plus binding identities. Its truth value remains
/// unknown, but equality is comparable and must not taint the whole test context.
#[test]
fn composite_condition_is_comparable_without_being_evaluated() {
    let resolved = resolve_first_call(
        r#"import { test } from "vitest"; test.skipIf(a && b)("x", () => {});"#,
        Globals::Disabled,
    )
    .expect("should still resolve as a skipIf declaration");
    let ChainStep::SkipIf { unresolved, .. } = &resolved.steps[0] else {
        panic!("expected a SkipIf step, got {:?}", resolved.steps);
    };
    assert!(
        !unresolved,
        "the expression has a stable syntax-and-binding identity"
    );
}

/// Defect 5: a literal condition is a genuinely known, comparable value and must keep comparing
/// equal to another occurrence of the same literal -- the fix must not make every condition
/// incomparable, only the ones that deserve it.
#[test]
fn defect5_literal_condition_still_compares_equal() {
    let a = resolve_first_call(
        r#"import { test } from "vitest"; test.skipIf(true)("x", () => {});"#,
        Globals::Disabled,
    )
    .unwrap();
    let b = resolve_first_call(
        r#"import { test } from "vitest"; test.skipIf(true)("y", () => {});"#,
        Globals::Disabled,
    )
    .unwrap();
    let ChainStep::SkipIf {
        condition: condition_a,
        unresolved: unresolved_a,
    } = &a.steps[0]
    else {
        panic!("expected a SkipIf step");
    };
    let ChainStep::SkipIf {
        condition: condition_b,
        unresolved: unresolved_b,
    } = &b.steps[0]
    else {
        panic!("expected a SkipIf step");
    };
    assert_eq!(condition_a, condition_b);
    assert!(!unresolved_a && !unresolved_b);
}

/// Copilot review #4, defect 1: a local `skipIf`/`runIf` condition's identity was scoped to
/// `<start>-<end>:name>`, unique only *within* one file. Two different files can declare a
/// same-named local binding at byte-for-byte identical offsets (identical boilerplate up to
/// that point) with opposite values -- `CrossFileKey` (`analysis.rs`) deliberately drops the
/// path when grouping for `cross-file-duplicate`, so a span-only identity collided these into a
/// false match. `const enabled = ` is the same length in both sources below, so `enabled`'s own
/// declaration span is byte-identical in each -- the only thing that can now tell them apart is
/// the file.
#[test]
fn local_condition_identity_is_scoped_by_file() {
    let source_a = r#"import { test } from "vitest"; const enabled = true; test.skipIf(enabled && isCI)("x", () => {});"#;
    let source_b = r#"import { test } from "vitest"; const enabled = false; test.skipIf(enabled && isCI)("x", () => {});"#;
    let a = resolve_first_call_in_file(source_a, Globals::Disabled, Path::new("a/file.test.ts"))
        .expect("should resolve as a skipIf declaration");
    let b = resolve_first_call_in_file(source_b, Globals::Disabled, Path::new("b/file.test.ts"))
        .expect("should resolve as a skipIf declaration");
    assert_ne!(
        skip_if_condition(&a),
        skip_if_condition(&b),
        "same-span local bindings in different files must not fingerprint identically"
    );
}

/// Positive control for the fix above: only the *local* identity branch needed a file
/// component. A condition that resolves to a true global (no local declaration in either
/// file) must still fingerprint identically across files -- two references to the same
/// undeclared global really do mean the same thing, and the fix must not overreach into
/// breaking that legitimate cross-file match.
#[test]
fn global_condition_identity_still_matches_across_files() {
    let source = r#"test.skipIf(isCI && !isWindows)("x", () => {});"#;
    let a = resolve_first_call_in_file(source, Globals::Enabled, Path::new("a/file.test.ts"))
        .expect("should resolve as a skipIf declaration");
    let b = resolve_first_call_in_file(source, Globals::Enabled, Path::new("b/file.test.ts"))
        .expect("should resolve as a skipIf declaration");
    assert_eq!(
        skip_if_condition(&a),
        skip_if_condition(&b),
        "a true global condition must still match across files"
    );
}

/// Defect 4: a `.each` table can be a literal array (so `literal_table_rows` returns `Some`)
/// while a specific row's cell is an identifier or call this extractor cannot read. That row
/// must be marked unresolved, independent of the table's own shape being fully known.
#[test]
fn defect4_row_with_identifier_cell_is_unresolved() {
    let source = "[[flag, 1], [\"known\", 2]]";
    let allocator = Allocator::default();
    let program = parse(&allocator, source);
    let semantic = SemanticBuilder::new().build(&program).semantic;
    let Statement::ExpressionStatement(statement) = &program.body[0] else {
        panic!("expected an expression statement");
    };
    let rows = literal_table_rows(
        &statement.expression,
        usize::MAX,
        &semantic,
        source,
        Path::new("x.ts"),
    )
    .expect("a literal array of literal-shaped rows is enumerable");
    assert_eq!(rows.len(), 2);
    assert!(
        rows[0].unresolved,
        "a row referencing an identifier cell must be unresolved"
    );
    assert!(
        !rows[1].unresolved,
        "a row whose cells are all literals must stay resolved"
    );
}

/// Parses `source` as a single expression and hands it to [`analyze_row`], returning its
/// `Named` fields. Panics if the row is not object-shaped -- every test below is.
fn analyze_named_row(source: &str) -> (Vec<(String, Option<CellValue>)>, bool) {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path("x.ts").unwrap();
    let expr = Parser::new(&allocator, source, source_type)
        .parse_expression()
        .expect("parses");
    let row = analyze_row(&expr, source, None);
    let RowCells::Named(fields) = row.cells else {
        panic!("expected a Named row, got {:?}", row.cells);
    };
    (fields, row.unresolved)
}

/// Copilot review #8: a JS object literal resolves a repeated static key to its LAST value
/// (`{ a: 1, a: 2 }` binds `a` to `2` at runtime), but `RowCells::Named` used to preserve every
/// occurrence, letting a `.find`-based lookup observe the FIRST one instead -- a value Vitest
/// itself never produces. This is the core of the fix: the duplicate must collapse to one
/// field holding the last-written value.
#[test]
fn duplicate_named_key_resolves_to_last_value() {
    let (fields, unresolved) = analyze_named_row("{ a: 1, a: 2 }");
    assert_eq!(fields.len(), 1, "a repeated key must collapse to one field");
    assert_eq!(fields[0].0, "a");
    assert_eq!(
        fields[0].1.as_ref().map(|cell| cell.raw.as_str()),
        Some("2"),
        "the last value written under the key must win"
    );
    assert!(!unresolved);
}

/// The case this fix is most likely to get backwards: when the LAST occurrence of a duplicate
/// key is itself not statically readable, that unreadable value still wins -- it is what
/// Vitest would actually bind -- so the row must be unresolved even though an earlier
/// occurrence under the same key was perfectly readable.
#[test]
fn duplicate_named_key_unreadable_last_value_leaves_row_unresolved() {
    let (fields, unresolved) = analyze_named_row("{ a: 1, a: getValue() }");
    assert_eq!(fields.len(), 1);
    assert!(
        fields[0].1.is_none(),
        "the surviving value is the unreadable one, not the earlier readable one"
    );
    assert!(
        unresolved,
        "the row must be unresolved: the last value wins even when it is the unknown one"
    );
}

/// The mirror image, which the fix must also get right: when only an EARLIER, discarded
/// occurrence of a duplicate key was unreadable, and the LAST occurrence is a plain literal,
/// the row must stay resolved -- nothing at runtime ever observes that earlier value, so its
/// unreadability must not taint the row.
#[test]
fn duplicate_named_key_earlier_unreadable_value_is_discarded() {
    let (fields, unresolved) = analyze_named_row("{ a: getValue(), a: 2 }");
    assert_eq!(fields.len(), 1);
    assert_eq!(
        fields[0].1.as_ref().map(|cell| cell.raw.as_str()),
        Some("2"),
        "the last, readable value must win"
    );
    assert!(
        !unresolved,
        "a discarded earlier value's unreadability must not make the row unresolved"
    );
}

/// Field order pins JS's own semantics: a repeated key keeps its FIRST-insertion position
/// (`Object.keys({ a: 1, b: 2, a: 3 })` is `["a", "b"]`, not `["b", "a"]`) -- only the value
/// moves to the last occurrence, never the position.
#[test]
fn duplicate_named_key_keeps_first_insertion_position() {
    let (fields, unresolved) = analyze_named_row("{ a: 1, b: 2, a: 3 }");
    let names: Vec<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        vec!["a", "b"],
        "a repeated key keeps its first-insertion position"
    );
    assert_eq!(
        fields[0].1.as_ref().map(|cell| cell.raw.as_str()),
        Some("3"),
        "the last value for the repeated key must still win"
    );
    assert!(!unresolved);
}

// -----------------------------------------------------------------------------------------
// `.each` / `.for` tables
// -----------------------------------------------------------------------------------------
//
// The table half of this module decides, per row, both what a title interpolates and whether
// `fingerprint.rs` may substitute the row's cells into the body at all. The two failure
// directions are not symmetric: a table whose rows cannot be read must stay *unreadable*
// (hazard 5 -- never an empty `Ok(vec![])`, never a guess), and two different unreadable
// tables must stay distinct from each other, or a rule reading `EachTable` without going
// through `Context` would read them as the same table. Every assertion below is on the
// resolved value rather than on "it resolved", because an `Err` carrying the wrong identity
// and an `Ok` carrying the wrong rows both resolve perfectly well.

/// The first `.each`/`.for` step of the first resolvable call in `source`.
fn each_step(source: &str) -> (bool, Result<Vec<Row>, String>) {
    let resolved = resolve_first_call(source, Globals::Enabled)
        .unwrap_or_else(|| panic!("`{source}` should resolve to a Vitest declaration"));
    for step in resolved.steps {
        if let ChainStep::Each { is_for, rows } = step {
            return (is_for, rows);
        }
    }
    panic!("`{source}` produced no `.each`/`.for` step");
}

/// Return readable fixture table rows, panicking with the table identity when reading is
/// refused.
fn each_rows(source: &str) -> Vec<Row> {
    each_step(source)
        .1
        .unwrap_or_else(|identity| panic!("`{source}` should be readable, got {identity}"))
}

/// The identity of a table whose rows could not be read.
fn unreadable_table(source: &str) -> String {
    each_step(source)
        .1
        .expect_err(&format!("`{source}` should be unreadable"))
}

/// A row's cell display texts in order, however the row is addressed.
fn cell_displays(row: &Row) -> Vec<Option<String>> {
    match &row.cells {
        RowCells::Positional(values) => values
            .iter()
            .map(|value| value.as_ref().map(|cell| cell.display.clone()))
            .collect(),
        RowCells::Named(fields) => fields
            .iter()
            .map(|(_, value)| value.as_ref().map(|cell| cell.display.clone()))
            .collect(),
    }
}

/// A named row's `(field, display)` pairs -- empty for a positionally-addressed row, which
/// has no field names to offer.
fn named_displays(row: &Row) -> Vec<(String, Option<String>)> {
    match &row.cells {
        RowCells::Named(fields) => fields
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    value.as_ref().map(|cell| cell.display.clone()),
                )
            })
            .collect(),
        RowCells::Positional(_) => Vec::new(),
    }
}

/// A table of arrays binds each row's cells to the callback parameters by position.
#[test]
fn an_array_of_arrays_table_is_read_positionally() {
    let rows = each_rows("test.each([[1, 'a'], [2, 'b']])('%s', () => {});");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].display, "[1, 'a']");
    assert_eq!(
        cell_displays(&rows[0]),
        vec![Some("1".to_owned()), Some("a".to_owned())]
    );
    assert!(!rows[0].unresolved && !rows[1].unresolved);
    assert!(
        named_displays(&rows[0]).is_empty(),
        "a positional row offers no field names"
    );

    // `.for` is the same table with a different binding convention, and the flag says which.
    assert!(!each_step("test.each([[1]])('x', () => {});").0, "`.each`");
    assert!(each_step("test.for([[1]])('x', () => {});").0, "`.for`");
}

/// A table of objects binds each row's cells to the callback parameters by field name.
#[test]
fn an_array_of_objects_table_is_read_by_field_name() {
    let rows = each_rows("test.each([{ a: 1, 'b-c': 2 }])('$a', () => {});");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        named_displays(&rows[0]),
        vec![
            ("a".to_owned(), Some("1".to_owned())),
            ("b-c".to_owned(), Some("2".to_owned())),
        ],
        "a string key names a field no identifier could spell"
    );
    assert!(!rows[0].unresolved);
    assert_eq!(
        cell_displays(&rows[0]),
        vec![Some("1".to_owned()), Some("2".to_owned())],
        "a named row's cells are also addressable in order"
    );

    // A repeated static key resolves to its *last* value at runtime, keeping its first
    // insertion position -- so every consumer that looks a name up with `.find` observes the
    // value Vitest itself produces.
    let rows = each_rows("test.each([{ a: 1, b: 2, a: 3 }])('$a', () => {});");
    assert_eq!(
        named_displays(&rows[0]),
        vec![
            ("a".to_owned(), Some("3".to_owned())),
            ("b".to_owned(), Some("2".to_owned())),
        ]
    );
    assert!(
        !rows[0].unresolved,
        "an unreadable *earlier* occurrence is never observed at runtime either"
    );
    // ...and the reverse: the surviving value being unreadable makes the row unresolved even
    // though the discarded one was fine.
    let rows = each_rows("test.each([{ a: 1, a: mk() }])('$a', () => {});");
    assert!(rows[0].unresolved);
}

/// A non-array row, or any row of a table that mixes arrays with other values, binds whole instead of being spread.
#[test]
fn a_single_value_row_and_a_mixed_table_bind_the_row_whole() {
    // Every row a scalar: one positional cell each.
    let rows = each_rows("test.each([1, 2])('%s', () => {});");
    assert_eq!(rows.len(), 2);
    assert_eq!(cell_displays(&rows[0]), vec![Some("1".to_owned())]);

    // A *mixed* table binds each row whole, so an array row inside one is a single value --
    // not a positional spread. Vitest only spreads when every row is an array, and treating
    // this array row as a spread made its first cell collide with the scalar row.
    let rows = each_rows("test.each([1, [2, 3]])('%s', () => {});");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].display, "[2, 3]");
    assert_eq!(
        cell_displays(&rows[1]),
        vec![Some("[ 2, 3 ]".to_owned())],
        "an array bound whole is ONE cell holding the whole array, not a positional spread"
    );
    assert!(
        !rows[1].unresolved,
        "and it is a value this extractor can read, so the row is comparable"
    );
    assert!(!rows[0].unresolved, "...as is the scalar row beside it");

    // The collision this whole binding rule exists to prevent, now pinned rather than
    // avoided by refusing to read the array at all. Vitest binds `1` and `[1]` as two
    // different values to the same parameter, so they must not compare equal -- the `number`
    // and `array` kind tags are what keeps them apart.
    let rows = each_rows("test.each([1, [1]])('%s', () => {});");
    assert!(!rows[0].unresolved && !rows[1].unresolved);
    let RowCells::Positional(scalar) = &rows[0].cells else {
        panic!("positional");
    };
    let RowCells::Positional(array) = &rows[1].cells else {
        panic!("positional");
    };
    let key = |cell: &Option<CellValue>| {
        let value = cell.as_ref().expect("readable");
        (value.kind, value.raw.clone())
    };
    assert_ne!(
        key(&scalar[0]),
        key(&array[0]),
        "the scalar row and the one-element array row must not fold to the same value"
    );
}

/// An array hole is a known `undefined` cell; a cell the extractor cannot read stays unresolved.
#[test]
fn a_hole_is_a_known_undefined_but_an_unreadable_cell_is_not() {
    // An elided *row* is statically `undefined`: known, so not unresolved.
    let rows = each_rows("test.each([[1], , [2]])('%s', () => {});");
    assert_eq!(rows.len(), 3);
    assert!(!rows[1].unresolved, "an elided row is a known `undefined`");
    assert_eq!(cell_displays(&rows[1]), Vec::new());

    // An elided *cell* is the same.
    let rows = each_rows("test.each([[1, , 2]])('%s', () => {});");
    assert_eq!(
        cell_displays(&rows[0]),
        vec![Some("1".to_owned()), None, Some("2".to_owned())]
    );
    assert!(!rows[0].unresolved);

    // A cell this extractor cannot read is not.
    let rows = each_rows("test.each([[1, mk()]])('%s', () => {});");
    assert!(
        rows[0].unresolved,
        "an unreadable cell makes its row unresolved"
    );

    // A spread inside a row, and a spread property inside an object row, are both
    // unenumerable contributions rather than readable values.
    let rows = each_rows("test.each([[...xs]])('%s', () => {});");
    assert!(
        rows[0].unresolved,
        "a spread cell is not statically enumerable"
    );
    let rows = each_rows("test.each([{ ...o }])('$a', () => {});");
    assert!(rows[0].unresolved, "a spread property is not either");
    let rows = each_rows("test.each([{ [k]: 1 }])('$a', () => {});");
    assert!(rows[0].unresolved, "nor is a computed key");
}

/// A table that cannot be read as rows is identified by its own source, so two different ones never compare equal.
#[test]
fn a_table_whose_rows_cannot_be_read_keeps_its_own_identity() {
    // A call or member expression keeps exact syntax plus referenced binding identities, so
    // two different unreadable tables are told apart without conflating shadowed locals.
    let called = unreadable_table("test.each(getCases())('%s', () => {});");
    assert!(called.starts_with("<expr:") && called.contains("getCases()"));
    assert_ne!(
        unreadable_table("test.each(getCasesA())('%s', () => {});"),
        unreadable_table("test.each(getCasesB())('%s', () => {});"),
        "two different unreadable tables must not collapse to one identity"
    );

    // A spread anywhere in the array makes the row *count* unknown, so the whole table is
    // identified rather than partially enumerated.
    let spread = unreadable_table("test.each([[1], ...rest])('%s', () => {});");
    assert!(
        spread.starts_with("<expr:") && spread.contains("[[1], ...rest]"),
        "the identity covers the whole array, including what preceded the spread: {spread}"
    );

    // No argument at all is as unreadable as any other shape, and says so distinctly.
    assert_eq!(
        unreadable_table("test.each()('%s', () => {});"),
        "<unresolved:no-table-argument>"
    );

    // A literal (not an array) folds to its own kind-tagged value.
    assert_eq!(
        unreadable_table("test.each(1)('%s', () => {});"),
        "<number:1>"
    );
    assert_eq!(
        unreadable_table("test.each('a')('%s', () => {});"),
        "<string:a>"
    );

    // A bare identifier resolves to the *symbol* it names, so two sibling scopes that each
    // declare their own same-named table are never conflated...
    let local = unreadable_table("const t = f(); test.each(t)('%s', () => {});");
    assert!(
        local.starts_with("<local:"),
        "a resolved local table is keyed on its declaration, got {local}"
    );
    // ...while an undeclared global really does mean the same thing everywhere.
    assert_eq!(
        unreadable_table("test.each(cases)('%s', () => {});"),
        "<global:cases>"
    );
}

/// A tagged-template table is read as rows keyed by its header row.
#[test]
fn a_tagged_template_table_is_read_from_its_header_row() {
    let rows = each_rows("test.each`a | b\n${1} | ${2}\n${3} | ${4}`('$a', () => {});");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].display, "1 | 2");
    assert_eq!(
        named_displays(&rows[0]),
        vec![
            ("a".to_owned(), Some("1".to_owned())),
            ("b".to_owned(), Some("2".to_owned())),
        ]
    );
    assert!(!rows[0].unresolved);
    let rows = each_rows("test.for`a\n${1}`('$a', () => {});");
    assert_eq!(rows.len(), 1);
    assert!(each_step("test.for`a\n${1}`('$a', () => {});").0, "`.for`");

    // An unreadable cell in a tagged row is still an unresolved row.
    let rows = each_rows("test.each`a\n${mk()}`('$a', () => {});");
    assert!(rows[0].unresolved);
}

/// A tagged-template table the extractor cannot parse keeps its raw text as its identity.
#[test]
fn a_tagged_template_table_this_extractor_cannot_parse_keeps_its_raw_text() {
    // No header columns at all, an interpolation count that is not a multiple of the column
    // count, and no interpolations at all: each is unreadable, and each keeps the template's
    // own source so two different ones stay distinct.
    for source in [
        "test.each`|`('x', () => {});",
        "test.each`a | b\n${1}`('x', () => {});",
        "test.each`a | b`('x', () => {});",
    ] {
        let identity = unreadable_table(source);
        assert!(
            identity.starts_with("<unresolved:"),
            "`{source}` must stay unreadable, got {identity}"
        );
    }
    assert_ne!(
        unreadable_table("test.each`a | b`('x', () => {});"),
        unreadable_table("test.each`c | d`('x', () => {});"),
        "two different unparseable templates must not collapse to one identity"
    );

    // A tagged template can only close over a bare `.each`/`.for` member: tagging the
    // *result* of an interior call has no member left to convert.
    assert!(
        resolve_outermost_call(
            "test.each([[1]])`a\n${1}`('x', () => {});",
            Globals::Enabled
        )
        .is_none(),
        "an already-converted `.each` is not a member a template can close over"
    );

    // A first quasi whose cooked text does not exist (an invalid escape, which a *tagged*
    // template is allowed to carry) leaves no header row to parse.
    let identity = unreadable_table("test.each`\\u${1}`('x', () => {});");
    assert!(
        identity.starts_with("<unresolved:"),
        "a template with no cooked header must stay unreadable, got {identity}"
    );

    // A `.for`/`.each` is the only member a tagged template link can be; anything else is
    // not a shape this extractor models.
    assert!(
        resolve_first_call("test.frobnicate`a\n${1}`('x', () => {});", Globals::Enabled).is_none(),
        "an unknown tagged member must not resolve"
    );
}

/// Constant folding yields a value only for the literals and operations it can prove from source.
#[test]
fn constant_folding_reads_exactly_the_values_it_can_prove() {
    let cell = |literal: &str| {
        let rows = each_rows(&format!("test.each([[{literal}]])('%s', () => {{}});"));
        let RowCells::Positional(values) = &rows[0].cells else {
            panic!("positional");
        };
        values[0].clone()
    };

    #[rustfmt::skip]
    let readable: &[(&str, &str, &str, &str)] = &[
        // (source literal, $field display, kind, canonical raw)
        ("'s'", "s", "string", "s"),
        ("1", "1", "number", "1"),
        ("1.5", "1.5", "number", "1.5"),
        ("true", "true", "boolean", "true"),
        ("false", "false", "boolean", "false"),
        ("null", "null", "null", "null"),
        ("(1)", "1", "number", "1"),
        ("1 + 2", "3", "number", "3"),
        ("3 - 1", "2", "number", "2"),
        ("3 * 4", "12", "number", "12"),
        ("6 / 4", "1.5", "number", "1.5"),
        ("1 + 2 * 3", "7", "number", "7"),
        // A leading sign. `-1` is the number -1 in a table, even though the serializer
        // renders a directly written `-1` as a computation -- see `fold_signed_number`.
        ("-1", "-1", "number", "-1"),
        ("+2", "2", "number", "2"),
        ("-1.5", "-1.5", "number", "-1.5"),
        ("-(3 - 1)", "-2", "number", "-2"),
        // A template with no substitutions is exactly the string it spells, and shares the
        // `string` kind with one written in quotes because they are the same value.
        ("`t`", "t", "string", "t"),
        ("``", "", "string", ""),
        // BigInt keeps its own kind so `100n` can never collide with `100`. The `n` shows up
        // in the display, which is a Vitest title, and not in the raw the fingerprint reads.
        ("100n", "100n", "bigint", "100"),
        // TypeScript casts carry no runtime value and the serializer descends straight
        // through them, so a cell wearing one folds to exactly what it wraps.
        ("1 as const", "1", "number", "1"),
        ("'s' as string", "s", "string", "s"),
        ("1 satisfies number", "1", "number", "1"),
        ("(1 as const)", "1", "number", "1"),
        ("-1 as const", "-1", "number", "-1"),
        // The two casts that are not written with a keyword. `x!` asserts non-nullness and
        // `<T>x` is the pre-`as` assertion spelling; both erase at runtime exactly like
        // `as`, and `unwrap_transparent` has to strip them for the same reason.
        ("1!", "1", "number", "1"),
        ("<number>1", "1", "number", "1"),
        // `+` concatenates when either operand is a string, and the result is tagged
        // `string` -- never `number`, or `'1' + '2'` would collide with the number 12.
        ("'a' + 'b'", "ab", "string", "ab"),
        ("'v' + 1", "v1", "string", "v1"),
        ("1 + 'v'", "1v", "string", "1v"),
        ("'1' + '2'", "12", "string", "12"),
        // A template interpolating only statically known values is the string it spells.
        ("`a${1}b`", "a1b", "string", "a1b"),
        ("`${1}-${2}`", "1-2", "string", "1-2"),
        ("`${'x'}`", "x", "string", "x"),
        // A substitution is interpolated with string coercion, so an array joins.
        ("`${[1, 2]}`", "1,2", "string", "1,2"),
        // ToString, not the `%s` renderer. Folding these with the Vitest renderer made
        // `'x' + { a: 1 }` produce `x{ a: 1 }`, byte-identical to the unrelated literal
        // `'x{ a: 1 }'` below it -- a `duplicate-test` false positive at error severity.
        // Every expected value here is `String(...)` as Node evaluates it.
        ("'x' + { a: 1 }", "x[object Object]", "string", "x[object Object]"),
        ("'x{ a: 1 }'", "x{ a: 1 }", "string", "x{ a: 1 }"),
        ("`${{ a: 1 }}`", "[object Object]", "string", "[object Object]"),
        // A BigInt drops its `n` under ToString and keeps it under `%s`.
        ("`${1n}`", "1", "string", "1"),
        ("'x' + 10n", "x10", "string", "x10"),
        // `String(-0)` is `"0"`; only `%s` prints the sign.
        ("'x' + -0", "x0", "string", "x0"),
        ("`${-0}`", "0", "string", "0"),
        ("'x' + [1, 2]", "x1,2", "string", "x1,2"),
        ("'x' + [{ a: 1 }]", "x[object Object]", "string", "x[object Object]"),
        ("'x' + [null]", "x", "string", "x"),
        // A getter or a method on an ordinary key does not intercept coercion, so these
        // really are `[object Object]` -- verified against Node, not assumed.
        ("'x' + { get a() { return 1 } }", "x[object Object]", "string", "x[object Object]"),
        ("'x' + { a() { return 1 } }", "x[object Object]", "string", "x[object Object]"),
        ("'x' + { 1: 2 }", "x[object Object]", "string", "x[object Object]"),
        ("'x' + null", "xnull", "string", "xnull"),
        ("'x' + true", "xtrue", "string", "xtrue"),
        // ECMAScript's shortest decimal is not always Rust's. This exact binary float is
        // `...62` under JavaScript even though `f64::to_string` chooses `...63`.
        ("'x' + 201562347225087.63", "x201562347225087.62", "string", "x201562347225087.62"),
        // Exponent notation follows JavaScript's thresholds and includes the explicit `+`.
        ("'x' + 1e21", "x1e+21", "string", "x1e+21"),
        ("'x' + 1e-7", "x1e-7", "string", "x1e-7"),
        // Composites read as their canonical contents under their own kind tags. `alpha`
        // keeps these apart by raw; `structural` drops the raw and sees `<array>`/`<object>`.
        ("[1]", "[ 1 ]", "array", "[<number:1>]"),
        ("[1, 'a']", "[ 1, 'a' ]", "array", "[<number:1>,<string:a>]"),
        ("[]", "[]", "array", "[]"),
        ("{ a: 1 }", "{ a: 1 }", "object", "{1:a:<number:1>}"),
        ("{}", "{}", "object", "{}"),
        ("{ 'a-b': 1 }", "{ 'a-b': 1 }", "object", "{3:a-b:<number:1>}"),
        // A repeated key binds its last value at runtime, at its first position.
        ("{ a: 1, a: 2 }", "{ a: 2 }", "object", "{1:a:<number:2>}"),
        // Nesting, and a cast inside one.
        ("[[1], { a: [2] }]", "[ [ 1 ], { a: [ 2 ] } ]", "array", "[[<number:1>],{1:a:[<number:2>]}]"),
        ("[1 as const]", "[ 1 ]", "array", "[<number:1>]"),
        // A string that spells a delimiter cannot forge one: it is escaped, so this stays
        // distinguishable from a nested number.
        (r#"['<number:1\u003e']"#, r"[ '<number:1>' ]", "array", r"[<string:\<number:1\>>]"),
        // A key that spells the framing cannot forge it either: the length prefix pins
        // where the key ends, so these four stay two-by-two distinct where the bare
        // rendering was one string.
        ("{ a: [], b: 1 }", "{ a: [], b: 1 }", "object", "{1:a:[],1:b:<number:1>}"),
        ("{ 'a:[],b': 1 }", "{ 'a:[],b': 1 }", "object", "{6:a:[],b:<number:1>}"),
        ("{ a: {}, b: 2 }", "{ a: {}, b: 2 }", "object", "{1:a:{},1:b:<number:2>}"),
        ("{ 'a:{},b': 2 }", "{ 'a:{},b': 2 }", "object", "{6:a:{},b:<number:2>}"),
        // The prefix counts the key as rendered, escapes included: `a>b` is three bytes
        // written and four once its delimiter is escaped.
        (r#"{ 'a>b': 1 }"#, r#"{ 'a>b': 1 }"#, "object", r"{4:a\>b:<number:1>}"),
    ];
    for (literal, display, kind, raw) in readable {
        let value =
            cell(literal).unwrap_or_else(|| panic!("`{literal}` should fold to a readable value"));
        assert_eq!(&value.display, display, "display of `{literal}`");
        assert_eq!(&value.kind, kind, "kind of `{literal}`");
        assert_eq!(&value.raw, raw, "raw of `{literal}`");
    }

    // Anything this cannot prove from source alone stays unread rather than guessed at --
    // including an operator it does not evaluate and arithmetic over a non-number.
    for literal in [
        "x",
        "mk()",
        "o.b",
        // A composite is only as readable as its contents.
        "[x]",
        "[1, mk()]",
        "{ a: x }",
        "{ ...spread }",
        "{ [key]: 1 }",
        "{ a }",
        "{ get a() { return 1 } }",
        // A hole has no literal kind to stand for `undefined`.
        "[1, , 2]",
        "[...rest]",
        "1 % 2",
        // A template is only as readable as what it interpolates.
        "`t${x}`",
        "`t${mk()}`",
        // `+` over two runtime values, and an operator this does not evaluate.
        "'a' + x",
        // An object that can intercept its own string coercion is not `[object Object]`:
        // Node gives `xy`, `x5` and `xz` for these. Refused rather than folded, because
        // folding them to `x[object Object]` collided with that exact literal -- the second
        // error-severity false positive this branch had to fix.
        "'x' + { toString: () => 'y' }",
        "'x' + { valueOf: () => 5 }",
        "'x' + { 'toString': () => 'y' }",
        "'x' + { __proto__: { toString: () => 'z' } }",
        "'x' + { [key]: 1 }",
        "'x' + { ...rest }",
        // A BigInt key is written without brackets but is not one of the three key shapes
        // this can read, so it cannot prove the object is plain -- refused, which costs a
        // comparison where Node would say `x[object Object]`.
        "'x' + { 1n: 2 }",
        "{ 1n: 2 }",
        // Through a join, too: the element's own coercion is what `join` calls.
        "'x' + [{ toString: () => 'y' }]",
        // A sign is only readable over a number it can already prove.
        "-x",
        "-'s'",
        // An operator this does not evaluate stays unread even under a cast.
        "(1 % 2) as const",
    ] {
        assert!(
            cell(literal).is_none(),
            "`{literal}` is not a value this extractor can prove"
        );
    }

    // The kind tag is what keeps a numeric `1000` from comparing equal to a string `'1000'`.
    let number = cell("1000").expect("readable");
    let string = cell("'1000'").expect("readable");
    assert_eq!(number.display, string.display, "...they do share a display");
    assert_ne!(
        (number.kind, number.raw),
        (string.kind, string.raw),
        "...but never a compared identity"
    );
}

// -----------------------------------------------------------------------------------------
// Title-directive rendering (`%s`, `%o`, `%j`, `$field`)
// -----------------------------------------------------------------------------------------
//
// One value, four renderings, and they genuinely disagree: `%s` of `'a'` is `a`, `%o` is
// `'a'`, `%j` is `"a"`. Each is asserted as its exact text rather than as "something was
// rendered" -- a directive that produces the wrong string still produces a string, and that
// string becomes part of a test's title and therefore of its identity.

/// The four renderings of one cell. `test.each([[cell]])` is a uniform table, so the row's
/// array spreads into one printf argument per cell and `title_args[0]` is this cell's.
fn title_arg_of(literal: &str) -> Option<TitleArg> {
    let rows = each_rows(&format!("test.each([[{literal}]])('%s', () => {{}});"));
    rows[0].title_args[0].clone()
}

/// The `$field` rendering of one row's first printf argument.
fn field_of(literal: &str) -> Option<String> {
    title_arg_of(literal).map(|arg| arg.field)
}

/// The `%o` rendering of one row's first printf argument.
fn inspected_of(literal: &str) -> Option<String> {
    title_arg_of(literal).map(|arg| arg.inspected)
}

/// T6: the title-budget mirror reproduces `docs/vitest-surface.md` §4's measured rows
/// byte-for-byte. Each assertion below is a row of that table, measured on Vitest 5.0.3.
#[test]
fn the_title_budget_mirrors_the_measured_truncation_rows() {
    // Under the budget: full, whatever the nesting.
    assert_eq!(
        inspected_of("[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]").unwrap(),
        "[ 0, 1, 2, 3, 4, 5, 6, 7, 8, 9 ]",
        "10 one-digit items, 32 characters"
    );
    assert_eq!(
        inspected_of("{ a: { a: { a: { a: { a: 1 } } } } }").unwrap(),
        "{ a: { a: { a: { a: { a: 1 } } } } }",
        "5-deep object, 37 characters: depth alone never truncates"
    );

    // Elision: the largest item count that fits, `…(k)` for the tail.
    let twenty: String = (0..20)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    assert_eq!(
        inspected_of(&format!("[{twenty}]")).unwrap(),
        "[ 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, …(10) ]",
        "20 items: ten shown, ten omitted"
    );
    assert_eq!(
        inspected_of("[90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100]").unwrap(),
        "[ 90, 91, 92, 93, 94, 95, 96, 97, …(3) ]",
        "the shown count is searched to fit the budget, not a fixed 10"
    );

    // The search can find width 0: even one item cannot fit.
    let long_key = "a".repeat(46);
    assert_eq!(
        inspected_of(&format!("{{ {long_key}: 1 }}")).unwrap(),
        "{ …(1) }"
    );

    // Depth halving: a 6-deep object lands at maxDepth 2, composites past the limit
    // printing [Object].
    assert_eq!(
        inspected_of("{ a: { a: { a: { a: { a: { a: 1 } } } } } }").unwrap(),
        "{ a: { a: [Object] } }"
    );

    // The string cut: 45 a's render as 38 characters then `…`, inside the quotes.
    let forty_five = "a".repeat(45);
    assert_eq!(
        inspected_of(&format!("'{forty_five}'")).unwrap(),
        format!("'{}…'", "a".repeat(38))
    );
    // `$field` of the same string: 39 kept, no quotes -- a different function
    // (`truncateString`) with a different cut.
    assert_eq!(
        field_of(&format!("'{forty_five}'")).unwrap(),
        format!("{}…", "a".repeat(39))
    );
}

/// The two string cuts at their edges, one input past each. Every expected value is the
/// title Vitest 5.0.3 (Node 24) produced for exactly this input; the probe is recorded in
/// `docs/vitest-surface.md` §4. A test at 45 only cannot tell a `> 40` threshold from a
/// `> 38` one, nor a 38-unit cut from a 39-unit one.
#[test]
fn the_string_cuts_are_probed_at_their_edges() {
    let a = |n: usize| "a".repeat(n);
    let quoted = |n: usize| format!("'{}'", a(n));
    // `$field`: `truncateString(value, 40)` -- unchanged through 40, then 39 kept.
    assert_eq!(field_of(&quoted(38)).unwrap(), a(38));
    assert_eq!(field_of(&quoted(39)).unwrap(), a(39));
    assert_eq!(
        field_of(&quoted(40)).unwrap(),
        a(40),
        "exactly 40 is not over"
    );
    assert_eq!(field_of(&quoted(41)).unwrap(), format!("{}…", a(39)));
    // `%o`: the QUOTED rendering is compared, so 38 content units (40 quoted) is the last
    // untouched length, and the cut keeps 38 from 39 onward.
    assert_eq!(inspected_of(&quoted(38)).unwrap(), quoted(38));
    assert_eq!(
        inspected_of(&quoted(39)).unwrap(),
        format!("'{}…'", a(38)),
        "39 content units quote to 41, one over"
    );
    assert_eq!(inspected_of(&quoted(40)).unwrap(), format!("'{}…'", a(38)));
    assert_eq!(inspected_of(&quoted(41)).unwrap(), format!("'{}…'", a(38)));
}

/// The budget counts UTF-16 code units, so `😀` is two. The expected titles are Vitest
/// 5.0.3's for these inputs; a scalar-counting budget reads `'😀'×20` as 20 and leaves every
/// one of these untouched.
#[test]
fn the_budget_counts_utf16_code_units_and_never_splits_a_surrogate_pair() {
    let e = |n: usize| "😀".repeat(n);
    let quoted = |n: usize| format!("'{}'", e(n));
    // `$field`: 20 emoji are 40 units, unchanged; 21 are cut, and unit 39 is the high
    // surrogate of the 20th emoji, so the back-off keeps 19.
    assert_eq!(field_of(&quoted(20)).unwrap(), e(20));
    assert_eq!(field_of(&quoted(21)).unwrap(), format!("{}…", e(19)));
    // `%o`: 19 emoji quote to 40 units, unchanged; 20 quote to 42 and keep 19.
    assert_eq!(inspected_of(&quoted(19)).unwrap(), quoted(19));
    assert_eq!(inspected_of(&quoted(20)).unwrap(), format!("'{}…'", e(19)));
    // The elision search measures in units too: `[ '😀'×10, '😀'×10, 1 ]` is 33 scalars
    // but 53 units, and one item is all that fits.
    assert_eq!(
        inspected_of(&format!("[{}, {}, 1]", quoted(10), quoted(10))).unwrap(),
        format!("[ {}, …(2) ]", quoted(10))
    );
}

/// `%s` of a plain object is Vitest's `inspect` with the same budget -- the object has no
/// own `toString`, so the `%s` arm falls through to the formatter. Measured on 5.0.3 for
/// exactly this object (63 characters in full).
#[test]
fn coerce_of_a_plain_object_runs_through_the_budget() {
    let row = each_rows(
        "test.each([[{ aaaaaaaaaaaaaaa: 1, bbbbbbbbbbbbbbbbbb: 2, ccccccccccccccc: 3 }]])('%s', () => {});",
    );
    let arg = row[0].title_args[0].clone().unwrap();
    assert_eq!(arg.coerced, "{ aaaaaaaaaaaaaaa: 1, …(2) }");
    assert_eq!(
        arg.coerced, arg.inspected,
        "`%s` and `%o` agree on an object"
    );
}

/// The mirror's contract on distinct values: two arrays sharing their first ten items render
/// identically, because Vitest titles them identically -- the elision sees only the shown
/// prefix and the omitted count. This non-injectivity is the POINT of the mirror (it is what
/// aligns rendered titles with runtime strings); precision is carried by the body and
/// context fingerprints, which still see the full values.
#[test]
fn values_truncating_to_the_same_title_render_alike() {
    let mut second: Vec<String> = (0..19).map(|i| i.to_string()).collect();
    second.push("999".to_owned());
    let first = inspected_of(&format!(
        "[{}]",
        (0..20)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
    .unwrap();
    let second = inspected_of(&format!("[{}]", second.join(", "))).unwrap();
    assert_eq!(
        first, second,
        "distinct values, same truncated title -- as Vitest renders them"
    );
}

/// Exactly 40 characters is not over the budget: the halving loop runs (its threshold is
/// at-or-over), but the maxDepth-1 rendering of exactly 40 is returned without the elision
/// search.
#[test]
fn a_rendering_of_exactly_the_budget_is_returned_untouched() {
    let exactly_forty = "[10, 11, 12, 13, 14, 15, 16, 17, 'ab']";
    assert_eq!(inspected_of(exactly_forty).unwrap().chars().count(), 40);
    assert_eq!(
        inspected_of(exactly_forty).unwrap(),
        "[ 10, 11, 12, 13, 14, 15, 16, 17, 'ab' ]"
    );
}

/// The depth refusal is a separate mechanism and survives the mirror: a composite nested
/// past MAX_CELL_DEPTH is refused outright (its row keeps the unrendered pattern), never
/// partially rendered -- §4's "a refusal, not a rendering difference".
#[test]
fn a_composite_past_the_cell_depth_bound_is_still_refused_not_truncated() {
    let deep = format!("{}1{}", "[".repeat(9), "]".repeat(9));
    assert_eq!(
        title_arg_of(&deep),
        None,
        "9-deep: refused, same as before the mirror"
    );
}

/// The `%s` rendering of each of a row's printf arguments, in order.
fn coerced_args(row: &Row) -> Vec<Option<String>> {
    row.title_args
        .iter()
        .map(|arg| arg.as_ref().map(|arg| arg.coerced.clone()))
        .collect()
}

/// Binding and title formatting disagree inside Vitest itself, and this is where.
///
/// A mixed table binds each row WHOLE -- a rest-parameter callback receives `[[2, 3]]`, one
/// argument -- while the title formatter spreads that same array into one `%s` per element.
/// Measured against Vitest 5.0.1. `a_single_value_row_and_a_mixed_table_bind_the_row_whole`
/// pins the binding half; this pins the title half, because a change that made either follow
/// the other's rule would leave that test green.
#[test]
fn a_mixed_tables_array_row_binds_whole_while_its_title_spreads() {
    let rows = each_rows("test.each([1, [2, 3]])('%s %s', () => {});");
    assert_eq!(rows.len(), 2);

    // One bound cell, holding the whole array under the `array` kind.
    let RowCells::Positional(bound) = &rows[1].cells else {
        unreachable!("a mixed table's rows are addressed positionally")
    };
    assert_eq!(bound.len(), 1, "the array row binds as a single value");
    assert_eq!(
        bound[0].as_ref().map(|cell| (cell.kind, cell.raw.as_str())),
        Some(("array", "[<number:2>,<number:3>]")),
        "the whole array is the bound value, tagged so it cannot collide with a scalar"
    );

    // ...and two printf arguments, from the very same array.
    assert_eq!(
        coerced_args(&rows[1]),
        vec![Some("2".to_owned()), Some("3".to_owned())],
        "the title formatter spreads the array the binding kept whole"
    );
    assert_eq!(
        coerced_args(&rows[0]),
        vec![Some("1".to_owned())],
        "the scalar row beside it still supplies exactly one argument"
    );
    assert_ne!(
        rows[0].identity(),
        rows[1].identity(),
        "and the two rows are never the same case"
    );

    // A uniform table spreads on both sides, so binding and title agree there -- which is
    // what makes the mixed case above a genuine disagreement rather than a modelling choice.
    let uniform = each_rows("test.each([[2, 3]])('%s %s', () => {});");
    assert_eq!(
        cell_displays(&uniform[0]),
        vec![Some("2".to_owned()), Some("3".to_owned())]
    );
    assert_eq!(
        coerced_args(&uniform[0]),
        vec![Some("2".to_owned()), Some("3".to_owned())]
    );
}

/// A tagged-template row binds ONE object, so `%s` gets one argument: that object's inspect
/// form, assembled from the header columns in declaration order.
///
/// The three directives do not agree here the way they do for a value read from an array
/// literal: `%j` deliberately has no answer, because reconstructing JSON out of columns that
/// have already been inspected is a guess rather than a rendering.
#[test]
fn a_tagged_template_row_titles_as_the_single_object_it_binds() {
    let rows = each_rows("test.each`a | b\n${1} | ${'s'}`('%s', () => {});");
    assert_eq!(rows.len(), 1);
    let arg = rows[0].title_args[0]
        .as_ref()
        .expect("a readable tagged row renders a title argument");
    assert_eq!(
        arg.coerced, "{ a: 1, b: 's' }",
        "one argument, holding every column"
    );
    assert_eq!(
        arg.inspected, arg.coerced,
        "an object has no own `toString`, so `%s` and `%o` agree"
    );
    assert_eq!(arg.field, arg.coerced, "and a positional `$0` follows suit");
    assert_eq!(
        arg.json, None,
        "`%j` keeps its pattern rather than reassembling JSON from inspected columns"
    );

    // A column this extractor cannot read takes the whole title argument with it: a title
    // assembled from only the columns that happened to be readable would be a different
    // string from the one Vitest prints.
    let rows = each_rows("test.each`a | b\n${1} | ${mk()}`('%s', () => {});");
    assert_eq!(coerced_args(&rows[0]), vec![None]);
}

/// `%j` is `JSON.stringify` and nothing looser.
///
/// Every expectation below was read off Node rather than reasoned about, because this module
/// reimplements `JSON.stringify` (escapes, compact separators, number spelling, key
/// coercion) instead of calling it. A rendering that drifts from the real thing puts a title
/// in a test's identity that Vitest never printed -- and two tests whose only difference
/// lives inside a `%j` title stop being told apart.
#[test]
fn the_json_directive_renders_exactly_what_json_stringify_prints() {
    #[rustfmt::skip]
    let cases: &[(&str, &str)] = &[
        // JSON's escapes are not JavaScript's: a tab is `\t`, and a control character with
        // no name of its own is a `\u` escape -- four lower-case hex digits.
        (r"'a'",                 r#""a""#),
        (r#"'a"b'"#,             r#""a\"b""#),
        (r"'a\\b'",              r#""a\\b""#),
        (r"'a\nb'",              r#""a\nb""#),
        (r"'a\rb'",              r#""a\rb""#),
        (r"'a\tb'",              r#""a\tb""#),
        (r"'\u0001'",          r#""\u0001""#),
        (r"'x\u001fy'",        r#""x\u001fy""#),
        (r"`t`",                 r#""t""#),
        ("1",                    "1"),
        ("1.5",                  "1.5"),
        ("true",                 "true"),
        ("false",                "false"),
        ("null",                 "null"),
        // JSON has no signed zero, even though `%s` prints one for the same cell.
        ("-0",                   "0"),
        ("-1",                   "-1"),
        // The number spelling is ECMAScript's, exponent notation and explicit `+` included.
        ("1e21",                 "1e+21"),
        ("1e-7",                 "1e-7"),
        // Arithmetic folds first, then stringifies as whichever type it produced.
        ("1 + 2",                "3"),
        ("'a' + 'b'",            r#""ab""#),
        // Compact separators -- `{"a":1}`, never `{ "a": 1 }`.
        ("[1, 'a', true, null]", r#"[1,"a",true,null]"#),
        ("{ a: 1, 'b-c': 'x' }", r#"{"a":1,"b-c":"x"}"#),
        ("[]",                   "[]"),
        ("{}",                   "{}"),
        ("{ a: [1, { b: 2 }] }", r#"{"a":[1,{"b":2}]}"#),
        // A numeric key is a string key in JSON, and a repeated key keeps its last value --
        // both exactly as `JSON.stringify` resolves them.
        ("{ 1: 2 }",             r#"{"1":2}"#),
        ("{ a: 1, a: 2 }",       r#"{"a":2}"#),
    ];
    for (literal, expected) in cases {
        assert_eq!(
            title_arg_of(literal).and_then(|arg| arg.json).as_deref(),
            Some(*expected),
            "`%j` of `{literal}`"
        );
    }

    // Where `JSON.stringify` has no string to give, neither does this: `%j` keeps its
    // pattern rather than inventing a representation. A BigInt throws outright, and a value
    // holding one has no more of an answer than the BigInt itself does.
    for literal in ["100n", "[10n]", "{ a: 10n }"] {
        let arg = title_arg_of(literal)
            .unwrap_or_else(|| panic!("`{literal}` renders under the other directives"));
        assert_eq!(
            arg.json, None,
            "`%j` of `{literal}` has no honest rendering"
        );
    }
}

/// `%s`, `%o` and `$field` are three renderings of one value, computed separately because
/// they disagree. Every row below is a value where at least two of them differ; a change
/// that collapsed any pair into one function would fail here rather than quietly relabel
/// every parameterized test that uses the losing directive.
#[test]
fn the_string_inspect_and_field_directives_disagree_where_vitest_does() {
    #[rustfmt::skip]
    let cases: &[(&str, &str, &str, &str)] = &[
        // (cell,        `%s`,     `%o`,       `$field`)
        ("'a'",          "a",      "'a'",      "a"),
        // An array coerces through `join`, which is ToString per element, so `%s` loses the
        // brackets the inspect form keeps -- and loses the BigInt's `n` and the signed zero
        // with them, because `join` calls ToString and ToString drops both.
        ("[1, 2]",       "1,2",    "[ 1, 2 ]", "[ 1, 2 ]"),
        ("[10n]",        "10",     "[ 10n ]",  "[ 10n ]"),
        ("[-0]",         "0",      "[ -0 ]",   "[ -0 ]"),
        ("[null]",       "",       "[ null ]", "[ null ]"),
        ("[]",           "",       "[]",       "[]"),
        // A plain object has no own `toString`, so `%s` inspects it and the three agree.
        ("{ a: 1 }",     "{ a: 1 }", "{ a: 1 }", "{ a: 1 }"),
        ("{}",           "{}",     "{}",       "{}"),
        // At the top level a BigInt and a signed zero keep what the array erased.
        ("10n",          "10n",    "10n",      "10n"),
        ("-0",           "-0",     "-0",       "-0"),
        // A string is bare under `%s` and `$field`, quoted under `%o` -- and quoted again
        // when it is nested, however it was spelled.
        ("`t`",          "t",      "'t'",      "t"),
        ("'a' + 'b'",    "ab",     "'ab'",     "ab"),
        ("['a']",        "a",      "[ 'a' ]",  "[ 'a' ]"),
    ];
    for (literal, coerced, inspected, field) in cases {
        let arg = title_arg_of(literal)
            .unwrap_or_else(|| panic!("`{literal}` should render under every directive"));
        assert_eq!(&arg.coerced, coerced, "`%s` of `{literal}`");
        assert_eq!(&arg.inspected, inspected, "`%o` of `{literal}`");
        assert_eq!(&arg.field, field, "`$field` of `{literal}`");
    }
}

/// The inspect form leaves a key bare only when it matches `[A-Za-z_][A-Za-z0-9_]*`.
///
/// `$` is deliberately outside that set (measured on Vitest 5.0.1, and recorded in
/// [`is_bare_inspect_key`]'s own doc), so a key spelled `$x` is quoted where `_x` is not.
/// If this fails, every `%o`/`$field` title over an object with an unusual key is being
/// reconstructed with the wrong punctuation.
#[test]
fn the_inspect_form_quotes_exactly_the_keys_vitest_quotes() {
    for (literal, expected) in [
        ("{ a: 1 }", "{ a: 1 }"),
        ("{ _y: 1 }", "{ _y: 1 }"),
        ("{ a1_b: 1 }", "{ a1_b: 1 }"),
        ("{ '$x': 1 }", "{ '$x': 1 }"),
        ("{ '1a': 1 }", "{ '1a': 1 }"),
        ("{ 'a-b': 1 }", "{ 'a-b': 1 }"),
        ("{ '': 1 }", "{ '': 1 }"),
        // A numeric key is written without quotes in source and printed with them.
        ("{ 1: 2 }", "{ '1': 2 }"),
    ] {
        let arg = title_arg_of(literal)
            .unwrap_or_else(|| panic!("`{literal}` should render under every directive"));
        assert_eq!(&arg.inspected, expected, "`%o` of `{literal}`");
    }
}

// -----------------------------------------------------------------------------------------
// Constant folding, in each of its three output shapes
// -----------------------------------------------------------------------------------------

/// [`const_fold`]'s untagged text, for the same direct-call style as
/// `const_fold_handles_simple_arithmetic`.
fn fold_untagged(source: &str) -> Option<String> {
    with_parsed_expression!(source, |expr| const_fold(&expr, None))
}

/// [`const_fold`] is the *untagged* value a title interpolates and a `for`-`of` loop binding
/// reads -- `extract.rs` calls it for both. Each literal kind it accepts is pinned to its
/// exact text here: a kind that silently stops folding costs every test carrying it its
/// comparability, and a kind that folds to the wrong text puts the wrong string in a title.
#[test]
fn untagged_folding_reads_every_literal_kind_and_refuses_operators_it_cannot_evaluate() {
    for (source, expected) in [
        ("'s'", "s"),
        ("1", "1"),
        ("true", "true"),
        ("false", "false"),
        ("null", "null"),
        // The `n` belongs in a title -- Vitest formats a BigInt row as `100n` -- and not in
        // the raw text a fingerprint compares, which `const_fold_kind` renders separately.
        ("100n", "100n"),
        ("`t`", "t"),
        ("[1, 'a']", "[<number:1>,<string:a>]"),
        ("{ a: 1 }", "{1:a:<number:1>}"),
        // A key that spells the framing cannot forge it: the length prefix keeps these
        // two tables' identities apart where the bare rendering was one string.
        ("{ a: [], b: 1 }", "{1:a:[],1:b:<number:1>}"),
        ("{ 'a:[],b': 1 }", "{6:a:[],b:<number:1>}"),
        // A signed operand inside arithmetic is a unary expression, not a negative literal;
        // folding it is what makes the sum readable at all.
        ("1 + -2", "-1"),
        ("-2 + 1", "-1"),
    ] {
        assert_eq!(
            fold_untagged(source).as_deref(),
            Some(expected),
            "fold of `{source}`"
        );
    }

    // A unary operator this does not evaluate, even over an operand it can read perfectly
    // well. Refusing costs a comparison; guessing would invent a value.
    for source in ["~1", "void 0", "typeof 1", "!true", "1 % 2"] {
        assert_eq!(fold_untagged(source), None, "`{source}` is not folded");
    }
}

/// [`const_fold_tagged`] is the identity `test.extend` fixture bodies, `.skipIf`/`.runIf`
/// conditions and unreadable `.each` tables all compare on, so every literal kind it reads
/// has to carry its own tag. An erased kind is how a fixture bound to `100n` came to compare
/// equal to one bound to `100`, and a `'1' + '2'` condition to the number `12`.
#[test]
fn tagged_folding_tags_every_literal_kind_it_can_read() {
    for (literal, expected) in [
        ("'s'", "<string:s>"),
        ("1", "<number:1>"),
        ("true", "<boolean:true>"),
        ("null", "<null>"),
        ("100n", "<bigint:100>"),
        ("`t`", "<string:t>"),
        ("`a${1}b`", "<string:a1b>"),
        ("1 + 2", "<number:3>"),
        ("-1", "<number:-1>"),
        ("'1' + '2'", "<string:12>"),
        ("[1, 'a']", "<array:[<number:1>,<string:a>]>"),
        ("[]", "<array:[]>"),
        ("{ a: 1 }", "<object:{1:a:<number:1>}>"),
        ("{}", "<object:{}>"),
    ] {
        assert_eq!(
            fold_tagged(literal).as_deref(),
            Some(expected),
            "tagged fold of `{literal}`"
        );
    }

    // The collisions the tags exist to prevent, each once real in this codebase.
    assert_ne!(
        fold_tagged("100n"),
        fold_tagged("100"),
        "a BigInt must not compare equal to the number spelled the same way"
    );
    assert_ne!(
        fold_tagged("'1' + '2'"),
        fold_tagged("12"),
        "a concatenation is a string, never the number its digits spell"
    );
    assert_ne!(
        fold_tagged("[1]"),
        fold_tagged("['1']"),
        "a composite's contents keep their own kinds"
    );
    assert_ne!(
        fold_tagged("{ a: 1 }"),
        fold_tagged("[1]"),
        "an object and an array never share a tag"
    );

    for literal in ["x", "mk()", "[x]", "{ a: x }", "1 % 2"] {
        assert_eq!(
            fold_tagged(literal),
            None,
            "`{literal}` is not a value this extractor can prove"
        );
    }
}

/// A composite nested past [`MAX_CELL_DEPTH`] is refused, not recursed into.
///
/// The bound exists so that pathologically nested input costs a `not-compared` test instead
/// of a stack overflow, and the assertion is placed one level *past* the edge on purpose: a
/// probe comfortably inside the limit would pass whatever the limit was. The readable side
/// is asserted too, so raising the bound is a deliberate edit and lowering it fails here.
#[test]
fn a_cell_nested_past_the_depth_bound_is_refused_rather_than_read() {
    let nested = |depth: usize| format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
    let cell = |literal: &str| {
        let rows = each_rows(&format!("test.each([[{literal}]])('%s', () => {{}});"));
        let RowCells::Positional(values) = &rows[0].cells else {
            unreachable!("an array-of-arrays row is addressed positionally")
        };
        values[0].clone()
    };

    let deepest_readable = cell(&nested(MAX_CELL_DEPTH as usize));
    assert_eq!(
        deepest_readable.map(|value| value.raw),
        Some("[[[[[[[[<number:1>]]]]]]]]".to_owned()),
        "a cell exactly at the bound still reads, contents and all"
    );
    assert!(
        cell(&nested(MAX_CELL_DEPTH as usize + 1)).is_none(),
        "one level past the bound is refused, not truncated or partially read"
    );

    // The two halves of a cell stop at slightly different nesting counts, because they
    // count from different roots: the renderer descends one level per bracket from the
    // outermost array, while the canonical fold only recurses on a nested *composite*. Both
    // are asserted rather than only the first to fail, so a change to either bound is a
    // deliberate edit and not a silent widening of the other.
    assert!(
        fold_tagged(&nested(MAX_CELL_DEPTH as usize + 1)).is_some(),
        "the canonical fold still reads one level past where the renderer stops"
    );
    assert!(
        fold_tagged(&nested(MAX_CELL_DEPTH as usize + 2)).is_none(),
        "...and refuses the level after that"
    );

    // A nested template bottoms out at *half* the bracket count, which is the part a reader
    // would get wrong: the bound is carried by the ToString renderer, and folding a template
    // and coercing its substitution alternate, so each nesting step spends two levels of it
    // rather than one.
    let template = |depth: usize| {
        let mut source = "1".to_owned();
        for _ in 0..depth {
            source = format!("`${{{source}}}`");
        }
        source
    };
    assert_eq!(
        fold_untagged(&template(4)).as_deref(),
        Some("1"),
        "four nested templates still fold to the value they spell"
    );
    assert_eq!(
        fold_untagged(&template(5)),
        None,
        "the fifth level is past the bound and is refused"
    );
}

// -----------------------------------------------------------------------------------------
// `test.extend` fixtures
// -----------------------------------------------------------------------------------------

/// Resolve the first fixture call and return its retained fixture identities and
/// uncertainty flag.
fn fixtures_of(source: &str) -> (Vec<FixtureBinding>, bool) {
    let resolved = resolve_first_call(source, Globals::Enabled)
        .unwrap_or_else(|| panic!("`{source}` should resolve"));
    (resolved.fixtures, resolved.fixtures_unresolved)
}

/// A `test.extend` fixture is a setup body, a literal, or unresolved, and nothing else.
#[test]
fn extend_reads_a_fixture_as_a_setup_body_a_literal_or_unresolved() {
    let (fixtures, unresolved) = fixtures_of(
        "test.extend({ a: () => 1, b: function () { return 2; }, c: 3, d: 'x' })\
         ('t', () => {});",
    );
    assert!(!unresolved, "every fixture above is statically readable");
    let names: Vec<&str> = fixtures.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "c", "d"]);
    assert!(
        matches!(&fixtures[0].body, FixtureValue::Setup(alpha) if alpha.starts_with("(fn ")),
        "an arrow fixture is a setup body: {:?}",
        fixtures[0].body
    );
    assert!(
        matches!(&fixtures[1].body, FixtureValue::Setup(alpha) if alpha.starts_with("(fn ")),
        "and so is a `function` fixture: {:?}",
        fixtures[1].body
    );
    assert_eq!(
        fixtures[2].body,
        FixtureValue::Literal("<number:3>".to_owned()),
        "a plain value is a comparable literal, kind tag included"
    );
    assert_eq!(
        fixtures[3].body,
        FixtureValue::Literal("<string:x>".to_owned())
    );
    // The kind tag again: a fixture bound to `1000` must never compare equal to one bound to
    // `'1000'`, which was a real error-severity false duplicate.
    let (number, _) = fixtures_of("test.extend({ a: 1000 })('t', () => {});");
    let (string, _) = fixtures_of("test.extend({ a: '1000' })('t', () => {});");
    assert_ne!(number[0].body, string[0].body);

    // Two setup bodies that differ only in a local's name are the same fixture; two that
    // differ in what they do are not.
    let (renamed, _) =
        fixtures_of("test.extend({ a: () => { const p = mk(); return p; } })('t', () => {});");
    let (same, _) =
        fixtures_of("test.extend({ a: () => { const q = mk(); return q; } })('t', () => {});");
    assert_eq!(renamed[0].body, same[0].body);
    let (different, _) =
        fixtures_of("test.extend({ a: () => { const p = other(); return p; } })('t', () => {});");
    assert_ne!(renamed[0].body, different[0].body);
}

/// Every `test.extend` fixture shape that cannot be read statically is reported as unresolved.
#[test]
fn extend_reports_every_fixture_shape_it_cannot_statically_read() {
    #[rustfmt::skip]
    let cases: &[(&str, &str)] = &[
        ("a value that is not provable", "test.extend({ a: makeRealDb() })('t', () => {});"),
        ("a computed key", "test.extend({ [k]: () => 1 })('t', () => {});"),
        ("a spread", "test.extend({ ...shared })('t', () => {});"),
        ("no argument at all", "test.extend()('t', () => {});"),
        ("an argument this extractor does not model", "test.extend(getFixtures())('t', () => {});"),
        ("builder syntax with no value", "test.extend('a')('t', () => {});"),
    ];
    for (name, source) in cases {
        let (_, unresolved) = fixtures_of(source);
        assert!(
            unresolved,
            "{name}: `{source}` is not statically readable and must say so"
        );
    }

    // "Nothing statically knowable" must never be mistaken for "no fixtures".
    let (fixtures, unresolved) = fixtures_of("test.extend(getFixtures())('t', () => {});");
    assert!(fixtures.is_empty() && unresolved);

    // Builder syntax (`.extend('name', setup)`) names one fixture per call.
    let (fixtures, unresolved) = fixtures_of("test.extend('db', async () => 1)('t', () => {});");
    assert!(!unresolved);
    assert_eq!(fixtures.len(), 1);
    assert_eq!(fixtures[0].name, "db");
    assert!(matches!(fixtures[0].body, FixtureValue::Setup(_)));

    // `.extend().extend()` unions every link in the chain rather than keeping the last.
    let (fixtures, _) = fixtures_of("test.extend({ a: 1 }).extend({ b: 2 })('t', () => {});");
    let names: Vec<&str> = fixtures.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b"]);
    // ...and one unreadable link taints the whole chain.
    let (_, unresolved) = fixtures_of("test.extend({ a: 1 }).extend({ b: mk() })('t', () => {});");
    assert!(unresolved);
}

// -----------------------------------------------------------------------------------------
// `.skipIf` / `.runIf` conditions
// -----------------------------------------------------------------------------------------

/// Resolve a conditional modifier and return its condition identity and unresolved flag,
/// panicking if absent.
fn condition_of(source: &str) -> (String, bool) {
    let resolved = resolve_first_call(source, Globals::Enabled)
        .unwrap_or_else(|| panic!("`{source}` should resolve"));
    for step in resolved.steps {
        match step {
            ChainStep::SkipIf {
                condition,
                unresolved,
            }
            | ChainStep::RunIf {
                condition,
                unresolved,
            } => return (condition, unresolved),
            _ => {}
        }
    }
    panic!("`{source}` produced no conditional step");
}

/// A `skipIf`/`runIf` condition is comparable only as a literal or as a resolved symbol.
#[test]
fn a_condition_is_vouched_for_only_as_a_literal_or_a_symbol() {
    assert_eq!(
        condition_of("test.skipIf(true)('t', () => {});"),
        ("<boolean:true>".to_owned(), false)
    );
    assert_eq!(
        condition_of("test.runIf(1 + 1)('t', () => {});"),
        ("<number:2>".to_owned(), false)
    );

    // An undeclared global really does mean the same thing in every file, so it keeps its
    // bare name and no file component.
    assert_eq!(
        condition_of("test.skipIf(isCI)('t', () => {});"),
        ("<global:isCI>".to_owned(), false)
    );

    // A resolved local is keyed on its declaration and its file: a span alone is only unique
    // within one file, and `cross-file-duplicate` drops the path when grouping.
    let (local, unresolved) =
        condition_of("const isCI = detect(); test.skipIf(isCI)('t', () => {});");
    assert!(local.starts_with("<local:") && !unresolved, "got {local}");
    let elsewhere = {
        let resolved = resolve_first_call_in_file(
            "const isCI = detect(); test.skipIf(isCI)('t', () => {});",
            Globals::Enabled,
            Path::new("other.test.ts"),
        )
        .expect("resolves");
        match resolved.steps.into_iter().next().expect("a step") {
            ChainStep::SkipIf { condition, .. } => condition,
            other => panic!("expected skipIf, got {other:?}"),
        }
    };
    assert_ne!(
        local, elsewhere,
        "byte-identical boilerplate in two files must not collide into one condition"
    );

    // A composite expression is comparable by exact syntax and referenced bindings without
    // claiming its runtime value is known.
    let composite = condition_of("test.skipIf(process.platform === 'win32')('t', () => {});");
    assert!(composite.0.starts_with("<expr:"), "got {}", composite.0);
    assert!(!composite.1);
    assert_eq!(
        condition_of("test.skipIf()('t', () => {});"),
        (String::new(), true),
        "no argument at all is unresolved, not vacuously true"
    );
}

/// A composite condition's identity has to move with its *syntax* as well as its bindings.
///
/// `defect5_sibling_bindings_with_the_same_name_fingerprint_differently` pins the bindings
/// half: same text, different declarations, different identity. This is the other half --
/// same declarations, different text -- which an identity built only from the referenced
/// symbols would collapse. `skipIf(a && b)` and `skipIf(a || b)` skip on opposite inputs,
/// so a collapse there suppresses or invents a skip and reports two differently-conditioned
/// tests as one.
#[test]
fn a_composite_condition_moves_with_its_operator_as_well_as_its_bindings() {
    let condition = |expr: &str| condition_of(&format!("test.skipIf({expr})('t', () => {{}});")).0;

    #[rustfmt::skip]
    let distinct: &[(&str, &str)] = &[
        ("a && b",       "a || b"),
        ("a === b",      "a !== b"),
        ("a > b",        "a < b"),
        ("a.b",          "a.c"),
        ("!a",           "a"),
        ("f(a)",         "f(b)"),
        ("f(a)",         "g(a)"),
        // Argument order is part of the spelling, and the two call different things.
        ("f(a, b)",      "f(b, a)"),
    ];
    for (left, right) in distinct {
        assert_ne!(condition(left), condition(right), "`{left}` / `{right}`");
    }

    // The positive control: the same spelling over the same bindings still compares equal,
    // or the rule would be silenced rather than corrected.
    assert_eq!(condition("a && b"), condition("a && b"));
    // ...and whitespace inside it is not a binding or an operator, but it *is* part of the
    // exact source this records -- pinned so the choice is visible rather than assumed.
    assert_ne!(
        condition("a && b"),
        condition("a  &&  b"),
        "the identity is exact source text, reformatting included"
    );
}

// -----------------------------------------------------------------------------------------
// Binding sources and chain shapes
// -----------------------------------------------------------------------------------------

/// A namespace import resolves a member call such as `vitest.test(...)` but never a bare call of the namespace.
#[test]
fn a_namespace_import_resolves_a_member_call_but_never_a_bare_call() {
    let resolved = resolve_first_call(
        "import * as v from 'vitest'; v.test('x', () => {});",
        Globals::Disabled,
    )
    .expect("`v.test(...)` resolves through the namespace");
    assert_eq!(resolved.root, "test");
    assert!(
        resolved.steps.is_empty(),
        "the namespace is not a chain step"
    );

    // An alias on the member is canonicalized the same way an import alias is.
    let resolved = resolve_first_call(
        "import * as v from 'vitest'; v.it('x', () => {});",
        Globals::Disabled,
    )
    .expect("`v.it(...)` resolves");
    assert_eq!(resolved.root, "test", "`it` canonicalizes to `test`");

    // The namespace object itself is not a declaration.
    assert!(
        resolve_first_call(
            "import * as v from 'vitest'; v('x', () => {});",
            Globals::Disabled
        )
        .is_none(),
        "calling the namespace itself is not a Vitest declaration"
    );
    // Nor is a member that is not an export.
    assert!(resolve_first_call(
        "import * as v from 'vitest'; v.frobnicate('x', () => {});",
        Globals::Disabled
    )
    .is_none());
    // And a member of an ordinary object is not a namespace access.
    assert!(
        resolve_first_call("const v = {}; v.test('x', () => {});", Globals::Disabled).is_none()
    );

    // A modifier hangs off a namespace member exactly as it does off a named import: the
    // namespace check has to decline `v.test` (an object that is not itself the namespace
    // identifier) and let the chain resolve through it, rather than reading `skip` as the
    // export name.
    let resolved = resolve_first_call(
        "import * as v from 'vitest'; v.test.skip('x', () => {});",
        Globals::Disabled,
    )
    .expect("`v.test.skip(...)` resolves through the namespace");
    assert_eq!(resolved.root, "test");
    assert!(
        matches!(resolved.steps.as_slice(), [ChainStep::Member(name)] if name == "skip"),
        "expected a single `skip` member step, got {:?}",
        resolved.steps
    );
}

/// Import forms that bring no Vitest binding into scope resolve nothing.
#[test]
fn import_forms_that_bind_nothing_useful_bind_nothing() {
    // A side-effect-only import of vitest declares no names at all.
    assert!(
        resolve_first_call("import 'vitest'; test('x', () => {});", Globals::Disabled).is_none(),
        "`import 'vitest'` binds no local name"
    );
    // Vitest has no meaningful default export for this extractor's purposes.
    assert!(resolve_first_call(
        "import d from 'vitest'; d('x', () => {});",
        Globals::Disabled
    )
    .is_none());
    // A string export name is still an export name.
    let resolved = resolve_first_call(
        "import { 'test' as t } from 'vitest'; t('x', () => {});",
        Globals::Disabled,
    )
    .expect("a string-named import resolves");
    assert_eq!(resolved.root, "test");
    // An import from somewhere else is not Vitest's.
    assert!(resolve_first_call(
        "import { test } from './helpers'; test('x', () => {});",
        Globals::Disabled
    )
    .is_none());
    // A real Vitest export that is not a declaration binding is still not one. `assert` is
    // imported from "vitest" and resolves to a symbol; `canonicalize` declining it is the
    // only thing that keeps `assert(...)` from being reported as a test declaration.
    assert!(resolve_first_call(
        "import { assert } from 'vitest'; assert('x', () => {});",
        Globals::Disabled
    )
    .is_none());
}

/// A declaration chain resolves through parentheses and optional links.
#[test]
fn a_chain_is_resolved_through_parentheses_and_optional_links() {
    for (name, source) in [
        ("parenthesized root", "(test)('x', () => {});"),
        ("parenthesized member", "(test.skip)('x', () => {});"),
        ("optional member", "test?.skip('x', () => {});"),
        ("optional call link", "test?.each([[1]])('x', () => {});"),
    ] {
        let resolved = resolve_first_call(source, Globals::Enabled)
            .unwrap_or_else(|| panic!("{name}: `{source}` should resolve"));
        assert_eq!(resolved.root, "test", "{name}");
    }

    // Shapes this extractor deliberately does not model. These have to be asked of the
    // *outermost* call: `resolve_first_call` falls through to the inner one, which resolves
    // perfectly well on its own and would hide the decline.
    for (name, source) in [
        ("calling the root's result again", "test(1)(2);"),
        (
            "calling an already-called `.each`",
            "test.each([[1]])('x', () => {})();",
        ),
        (
            "an unknown interior call",
            "test.frobnicate(1)('x', () => {});",
        ),
    ] {
        assert!(
            resolve_outermost_call(source, Globals::Enabled).is_none(),
            "{name}: `{source}` is not a shape this extractor models"
        );
    }
    // The control, through the same helper: an interior call it *does* model resolves, and
    // the helper stops at the first call rather than walking into later ones.
    let resolved = resolve_outermost_call(
        "test.each([[1]])('x', () => {}); test('y', () => {});",
        Globals::Enabled,
    )
    .expect("`test.each(table)(...)` resolves");
    assert_eq!(resolved.root, "test");
    assert!(matches!(
        resolved.steps.as_slice(),
        [ChainStep::Each { .. }]
    ));
}

/// Resolves only the first `CallExpression` in source order, resolving or not -- unlike
/// [`resolve_first_call`], which keeps looking until something resolves and so cannot observe
/// an outer call being declined.
struct OutermostCallResolver<'r, 's> {
    registry: &'r Registry,
    semantic: &'r Semantic<'s>,
    source: &'r str,
    file: &'r Path,
    seen: bool,
    resolved: Option<ResolvedCallee>,
}

impl<'a> Visit<'a> for OutermostCallResolver<'_, '_> {
    /// Attempt resolution only for the first visited call, without descending into it.
    fn visit_call_expression(&mut self, it: &oxc_ast::ast::CallExpression<'a>) {
        if self.seen {
            return;
        }
        self.seen = true;
        self.resolved = resolve_callee(
            &it.callee,
            self.registry,
            self.semantic,
            self.source,
            self.file,
        );
    }
}

/// Parse the fixture and resolve its first call without descending through that call before
/// selection.
fn resolve_outermost_call(source: &str, globals: Globals) -> Option<ResolvedCallee> {
    let allocator = Allocator::default();
    let program = parse(&allocator, source);
    let semantic = SemanticBuilder::new().build(&program).semantic;
    let registry = Registry::new(&program, globals);
    let mut resolver = OutermostCallResolver {
        registry: &registry,
        semantic: &semantic,
        source,
        file: Path::new("file.test.ts"),
        seen: false,
        resolved: None,
    };
    resolver.visit_program(&program);
    resolver.resolved
}
/// An optional-chain expression in *callee* position is the only way into `resolve_callee`'s
/// own `ChainElement` match: `test?.each(t)(...)` puts the call inside the chain, so the
/// callee it sees is a plain member access. `(test?.each(t))(...)` puts the chain in the
/// callee, and each element kind takes a different arm.
#[test]
fn an_optional_chain_in_callee_position_is_resolved_element_by_element() {
    let resolved = resolve_outermost_call("(test?.skip)('x', () => {});", Globals::Enabled)
        .expect("a chained member callee resolves");
    assert_eq!(resolved.root, "test");
    assert!(
        matches!(resolved.steps.as_slice(), [ChainStep::Member(name)] if name == "skip"),
        "got {:?}",
        resolved.steps
    );

    let resolved = resolve_outermost_call("(test?.each([[1]]))('x', () => {});", Globals::Enabled)
        .expect("a chained interior call callee resolves");
    assert_eq!(resolved.root, "test");
    assert!(
        matches!(resolved.steps.as_slice(), [ChainStep::Each { rows: Ok(rows), .. }] if rows.len() == 1),
        "got {:?}",
        resolved.steps
    );

    // A computed member is not a chain element this extractor models.
    assert!(
        resolve_outermost_call("(test?.[0])('x', () => {});", Globals::Enabled).is_none(),
        "a computed chain element is not a Vitest member"
    );
}

/// A tagged template is only a table when it closes over a `.each`/`.for` *member*. Tagging
/// the root itself (`` test`...` ``) closes over no member at all.
#[test]
fn a_tagged_template_on_the_root_itself_is_not_a_table() {
    assert!(
        resolve_outermost_call("test`a\n${1}`('x', () => {});", Globals::Enabled).is_none(),
        "there is no `.each`/`.for` member for the template to close over"
    );
}

/// The ECMAScript number spelling, pinned where it is a *value identity* (keys, folds) and
/// where it is display text, plus the one deliberate divergence.
///
/// Rust's `f64` Display and ECMAScript differ at least wherever ECMAScript switches notation
/// (`|x| >= 1e21`, `|x| < 1e-6`) and can also pick different shortest digits on in-range
/// ties, so no magnitude is trusted to agree by accident. `1e21` as an object key is `"1e+21"` at
/// runtime (Node v26.0.0: `Object.keys({1e21: 1})` is `["1e+21"]`), so any fold that spelled it
/// as `1000000000000000000000` merged the key with the unrelated decimal string and split it
/// from the equivalent string `'1e+21'`. Both directions are pinned here at the spelling level
/// and in the corpus fixtures at the finding level.
#[test]
fn number_folds_take_the_ecmascript_spelling_except_signed_zero() {
    #[rustfmt::skip]
    let cases: &[(f64, &str)] = &[
        (0.0, "0"),
        (1.0, "1"),
        (-1.0, "-1"),
        (0.5, "0.5"),
        (123.456, "123.456"),
        // The notation switch Rust's Display does not make.
        (1e20, "100000000000000000000"),
        (1e21, "1e+21"),
        (-1e21, "-1e+21"),
        (1e-6, "0.000001"),
        (1e-7, "1e-7"),
        (f64::NAN, "NaN"),
        (f64::INFINITY, "Infinity"),
        (f64::NEG_INFINITY, "-Infinity"),
    ];
    for (value, expected) in cases {
        assert_eq!(&format_number(*value), expected, "format_number({value})");
        assert_eq!(
            &js_number_to_string(*value),
            expected,
            "js_number_to_string({value})"
        );
    }
    // The one deliberate divergence: the FINGERPRINT keeps `-0` distinct from `0`
    // (`Object.is(0, -0)` is false, so two fixture values must not fold together), while
    // ToString text spells both as `0`, exactly as JavaScript does.
    assert_eq!(format_number(-0.0), "-0");
    assert_eq!(js_number_to_string(-0.0), "0");
    // The spelling round-trips: `Number('1e+21')` is the value it came from.
    assert_eq!(number_of_string("1e+21"), Some(1e21));
    assert_eq!(number_of_string("1e-7"), Some(1e-7));
}

/// Numeric object keys canonicalize to the runtime key they bind, so the two spellings of
/// `1e+21` fold together and the look-alike decimal string stays its own key.
#[test]
fn object_key_canonicals_follow_the_runtime_key() {
    let display_of = |object: &str| {
        let rows = each_rows(&format!("test.each([[{object}]])('%s', () => {{}});"));
        cell_displays(&rows[0])[0].clone()
    };
    let exponent = display_of("{1e21: 1}").expect("the numeric-key object folds");
    let spelled = display_of("{'1e+21': 1}").expect("the string-key object folds");
    let decimal = display_of("{'1000000000000000000000': 1}").expect("the decimal string folds");
    assert_eq!(
        exponent, spelled,
        "numeric 1e21 and the string '1e+21' bind one runtime key and must fold alike"
    );
    assert_ne!(
        exponent, decimal,
        "the decimal string is a different runtime key however similar it looks"
    );
    assert!(
        exponent.contains("1e+21"),
        "the shared key spells the ECMAScript form, got {exponent:?}"
    );
    assert!(
        decimal.contains("1000000000000000000000"),
        "the decimal key keeps its own text, got {decimal:?}"
    );
}
