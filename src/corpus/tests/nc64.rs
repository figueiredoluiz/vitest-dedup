//! End-to-end row-value recovery against the permanent runtime fixtures.
use super::*;
use crate::model::Rule;

/// Parentheses and erased type wrappers cannot hide deletes or updates to literal members.
#[test]
fn nc64_wrapped_member_mutations_refuse_scalar_claims() {
    let source =
        include_str!("../../../fixtures/oracle/row-value-tables/wrapped-mutations.test.ts");
    assert_owned_row_findings(
        counted_corpus(source, "cases.test.ts", 10, 0),
        &[("stable control", 2)],
    );
}

/// Implicit arguments retain tagged inputs while argument-independent arrows can still compare.
#[test]
fn nc64_tagged_arguments_cannot_erase_invocation_data() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/tagged-arguments.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 6, 4);
    let outcome = owned_findings("cases.test.ts", corpus);
    let exact = outcome
        .findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateTest)
        .collect::<Vec<_>>();
    assert_eq!(exact.len(), 1, "{exact:?}");
    assert_eq!(exact[0].titles, ["arrow control one", "arrow control two"]);
}

/// The suite's own options must be inert before removing one of its repeated invocations.
#[test]
fn nc64_own_suite_options_require_collection_purity() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/own-options.test.ts");
    assert_owned_row_findings(
        counted_corpus(source, "cases.test.ts", 5, 0),
        &[("own literal control", 2)],
    );
}

/// Header-line interpolations cannot invent rows, even when the template starts with a newline.
#[test]
fn nc64_tagged_headers_end_before_the_first_cell() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/tagged-header.test.ts");
    assert_owned_row_findings(
        counted_corpus(source, "cases.test.ts", 4, 0),
        &[("terminated header", 2)],
    );
}

/// Executed concise expressions cannot borrow the empty-block collection proof, even when nested.
#[test]
fn nc64_concise_collection_effects_do_not_claim_removable_suite_rows() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/concise-effects.test.ts");
    assert_owned_row_findings(
        counted_corpus(source, "cases.test.ts", 7, 1),
        &[("block control", 2)],
    );
}

/// Destructuring does not bind the initializer object; plain identifiers retain member proofs.
#[test]
fn nc64_destructured_members_do_not_inherit_the_initializer_object() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/destructured.test.ts");
    assert_owned_row_findings(
        counted_corpus(source, "cases.test.ts", 8, 0),
        &[("plain control", 2)],
    );
}

/// Lazy safety checks retain both local and imported row claims from the runtime fixture.
#[test]
fn nc64_lazy_import_checks_preserve_local_and_imported_claims() {
    let corpus = corpora(
        &[
            (
                "cases.test.ts",
                include_str!("../../../fixtures/oracle/row-value-tables/literal-imports.test.ts"),
            ),
            (
                "scalar-values.ts",
                include_str!("../../../fixtures/oracle/row-value-tables/scalar-values.ts"),
            ),
        ],
        &["cases.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 4);
    assert!(corpus.tests.iter().all(|case| case.exclusion.is_none()));
    assert_owned_row_findings(corpus, &[("local control", 2), ("imported control", 2)]);
}

/// Select owned repeated-row claims after the caller has checked the extracted population.
fn row_claims(corpus: FileCorpus) -> Vec<crate::model::Finding> {
    owned_findings("cases.test.ts", corpus)
        .findings
        .into_iter()
        .filter(|finding| finding.rule == Rule::DuplicateEachRow)
        .collect()
}

/// Read one permanent recall project with its own module graph and count before claiming recovery.
fn recall_project(name: &str) -> FileCorpus {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/recall")
        .join(name);
    let path = std::fs::read_dir(&directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().ends_with(".test.ts"))
        .unwrap();
    let source = std::fs::read_to_string(&path).unwrap();
    file(
        &path,
        &source,
        Setting {
            dialect: &DIALECT,
            globals: false,
            root: &directory,
            benchmarks: false,
        },
        &mut Run::default(),
    )
    .unwrap()
}

/// Scalar and tagged-table admissions recover all sixteen non-suite held expectations together.
#[test]
fn nc64_scalar_and_tagged_rows_recover_the_mapped_expectations() {
    for (name, tests, pairs) in [
        ("duplicate-each-row-imported-constants", 20, 10),
        ("duplicate-each-row-global-constants", 8, 4),
        ("duplicate-each-row-global-signed-zero", 2, 1),
        ("duplicate-each-row-const-member", 2, 1),
    ] {
        let corpus = recall_project(name);
        assert_eq!(corpus.tests.len(), tests, "{name}");
        assert!(corpus.tests.iter().all(|c| c.exclusion.is_none()), "{name}");
        let outcome = owned_findings("values.test.ts", corpus);
        let rows = outcome
            .findings
            .iter()
            .filter(|f| f.rule == Rule::DuplicateEachRow)
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), pairs, "{name}: {:?}", outcome.findings);
    }
}

/// Suite rows retain their full child population and emit the seventeenth row claim at its owner.
#[test]
fn nc64_suite_rows_recover_children_titles_and_the_repeated_row_claim() {
    let corpus = recall_project("duplicate-each-row-describe-each");
    assert_eq!(corpus.tests.len(), 6);
    assert!(corpus.tests.iter().all(|c| c.exclusion.is_none()));
    assert_eq!(
        corpus
            .tests
            .iter()
            .filter(|c| c
                .full_title
                .as_deref()
                .is_some_and(|s| s.starts_with("admin permissions >")))
            .count(),
        4
    );
    let config = crate::config::Config::load(Path::new("."), Default::default()).unwrap();
    let outcome = crate::rules::run_with_hooks(
        &[("permissions.test.ts".into(), corpus.tests)],
        &[("permissions.test.ts".into(), corpus.suites)],
        &[],
        &config,
        &[],
    );
    let rows = outcome
        .findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateEachRow)
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1, "{:?}", outcome.findings);
    assert_eq!(rows[0].titles, ["admin permissions", "admin permissions"]);
    assert_eq!(
        rows[0].cases.len(),
        4,
        "charge the four duplicated children"
    );
}

/// Typed equality must never follow a display collision or an unknown coercion.
#[test]
fn nc64_scalar_conflicts_and_unknowns_retain_only_the_positive_control() {
    let source = format!(
        "{HEADER}\n\
        test.each([[0], [-0]])('zero', v => expect(v).toBeDefined());\n\
        test.each([[1], ['1']])('type', v => expect(v).toBeDefined());\n\
        test.each([[unknown], [undefined]])('unknown', v => expect(v).toBeDefined());\n\
        test.each([[NaN], [0 / 0]])('nan', v => expect(v).toBeNaN());"
    );
    let corpus = counted_corpus(&source, "cases.test.ts", 8, 0);
    assert_owned_row_findings(corpus, &[("nan", 2)]);
}

/// Literal own-member reads cease to be proofs as soon as the object can change or escape.
#[test]
fn nc64_member_mutation_and_escape_refuse_row_claims() {
    for effect in [
        "box.n = 2",
        "box.n++",
        "delete box.n",
        "const alias = box",
        "consume(box)",
    ] {
        let source = format!(
            "{HEADER}\nconst box = {{ n: 1 }}; {effect};\n\
            test.each([[box.n], [1]])('member', n => expect(n).toBeDefined());"
        );
        let corpus = corpora(&[("cases.test.ts", &source)], &["cases.test.ts"]).remove(0);
        assert_eq!(corpus.tests.len(), 2, "{effect}");
        assert_owned_row_findings(corpus, &[]);
    }
    let source = format!(
        "{HEADER}\nconst box = {{ n: 1 }};\n\
        test.each([[box.n], [1]])('member', n => expect(n).toBe(1));"
    );
    assert_owned_row_findings(
        counted_corpus(&source, "cases.test.ts", 2, 0),
        &[("member", 2)],
    );
}

/// Export aliases share primitive proofs while erased, mutable and cyclic exports remain opaque.
#[test]
fn nc64_import_routes_preserve_runtime_binding_requirements() {
    let source = "import {test,expect} from 'vitest';\nimport { VALUE } from './values';\n\
        test.each([[VALUE], [1]])('import', n => expect(n).toBe(1));";
    for (values, extra, positive) in [
        ("const n=1; export {n as VALUE}", "export const n=1", true),
        (
            "export {n as VALUE} from './other'",
            "export const n=1",
            true,
        ),
        ("export let VALUE=1", "export const n=1", false),
        ("export declare const VALUE=1", "export const n=1", false),
        (
            "export type {n as VALUE} from './other'",
            "export const n=1",
            false,
        ),
        (
            "export {n as VALUE} from './other'",
            "export {VALUE as n} from './values'",
            false,
        ),
    ] {
        let corpus = corpora(
            &[
                ("cases.test.ts", source),
                ("values.ts", values),
                ("other.ts", extra),
            ],
            &["cases.test.ts"],
        )
        .remove(0);
        assert_eq!(corpus.tests.len(), 2, "{values}");
        assert_owned_row_findings(corpus, if positive { &[("import", 2)] } else { &[] });
    }
}

/// Nested suite and test rows retain eight separate runtime gate identities and exact ancestry.
#[test]
fn nc64_nested_tables_keep_cartesian_population_and_gate_identity() {
    let source = format!(
        "{HEADER}\n\
        describe.each([1,2])('outer %s', outer => {{\n\
          describe.each([3,4])('inner %s', inner => {{\n\
            test.each([5,6])('leaf %s', leaf => expect([outer,inner,leaf]).toHaveLength(3));\n\
          }});\n\
        }});"
    );
    let corpus = counted_corpus(&source, "cases.test.ts", 8, 0);
    let ids = corpus
        .tests
        .iter()
        .map(|c| c.gate_row())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 8);
    let names = corpus
        .tests
        .iter()
        .map(|c| c.full_title.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(names.len(), 8);
    assert!(names.contains(&Some("outer 2 > inner 4 > leaf 6".into())));
    assert_owned_row_findings(corpus, &[]);
}

/// Unknown suite cells cannot supply equal children through the shared lexical callback.
#[test]
fn nc64_unknown_suite_rows_are_visible_refusals() {
    let source=format!("{HEADER}\n\
        describe.each([unknown,unknown])('suite %s', n => {{ test('child', () => expect(n).toBeDefined()) }});");
    let corpus = counted_corpus(&source, "cases.test.ts", 2, 2);
    assert!(corpus
        .tests
        .iter()
        .all(|c| c.exclusion == Some(Exclusion::UnprovedSuiteRow)));
    assert_owned_row_findings(corpus, &[]);
}

/// The same owned runtime project pins typed conflicts, tagged rows and nested child population.
#[test]
fn nc64_owned_runtime_project_keeps_positive_and_conflicting_controls() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/cases.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 22, 0);
    assert_owned_row_findings(
        corpus,
        &[
            ("member", 2),
            ("nan", 2),
            ("decimal 0", 2),
            ("float -0", 2),
            ("bigint 10n", 2),
        ],
    );
}

/// A sparse whole-row argument keeps its hole and cannot collide with a shifted title.
#[test]
fn nc64_sparse_title_arguments_keep_their_positions() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/sparse.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 2, 0);
    assert_eq!(
        corpus.tests[0].full_title.as_deref(),
        Some("sparse undefined 1")
    );
    let findings = owned_findings("cases.test.ts", corpus);
    assert!(!findings
        .findings
        .iter()
        .any(|f| f.rule == Rule::DuplicateTestName));
}

/// Ordinary object rows bind their fields rather than refusing the complete object.
#[test]
fn nc64_object_suite_rows_preserve_the_children_and_distinct_bindings() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/objects.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 2, 0);
    assert_ne!(corpus.tests[0].body, corpus.tests[1].body);
    assert_eq!(
        corpus.tests[1].full_title.as_deref(),
        Some("object 2 > child")
    );
    assert_owned_row_findings(corpus, &[]);
}

/// Collection-time tagged interpolations cannot establish a removable suite invocation.
#[test]
fn nc64_tagged_collection_effects_refuse_suite_row_claims() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/tagged-effects.test.ts");
    let corpus = corpora(&[("cases.test.ts", source)], &["cases.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 2);
    assert_owned_row_findings(corpus, &[]);
}

/// A dependency's eval invalidates its exported scalar proof, even when the importer has no eval.
#[test]
fn nc64_imported_eval_cannot_supply_scalar_equality() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/eval.test.ts");
    let values = include_str!("../../../fixtures/oracle/row-value-tables/eval-values.ts");
    let corpus = corpora(
        &[("cases.test.ts", source), ("eval-values.ts", values)],
        &["cases.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 2);
    assert_owned_row_findings(corpus, &[]);
}

/// Inherited object fields and callback arguments remain outside primitive suite binding proofs.
#[test]
fn nc64_suite_invocation_objects_are_not_replaced_with_undefined() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/bindings.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 3, 3);
    assert!(corpus
        .tests
        .iter()
        .all(|c| c.exclusion == Some(Exclusion::UnprovedSuiteRow)));
    assert_owned_row_findings(corpus, &[]);
}

/// A row repeated in only one parent invocation charges only that invocation's children.
#[test]
fn nc64_suite_row_coverage_preserves_the_parent_invocation() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/parent-cases.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 4, 0);
    let rows = row_claims(corpus);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        rows[0].cases.len(),
        2,
        "charge only the duplicated parent invocation"
    );
}

/// Each outer invocation retains its repeated-suite-row claim and two actual child cases.
#[test]
fn nc64_nested_suite_claims_keep_each_parent_and_its_children() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/parent-suites.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 4, 0);
    let rows = row_claims(corpus);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|f| f.cases.len() == 2));
    let cases = rows
        .iter()
        .flat_map(|f| f.cases.iter())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(cases.len(), 4);
}

/// Two tagged declarations capturing successive values cannot share an exact body proof.
#[test]
fn nc64_tagged_captures_are_not_replayed_inside_callback_bodies() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/tagged-captures.test.ts");
    let corpus = corpora(&[("cases.test.ts", source)], &["cases.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 2);
    let findings = owned_findings("cases.test.ts", corpus);
    assert!(
        !findings
            .findings
            .iter()
            .any(|f| matches!(f.rule, Rule::DuplicateTest | Rule::DuplicateEachRow)),
        "{:?}",
        findings.findings
    );
}

/// Mutable Vitest module objects and exported object aliases cannot certify named const values.
#[test]
fn nc64_imported_values_refuse_namespace_rewrites_and_exported_object_escapes() {
    let cases = [
        (include_str!("../../../fixtures/recall/duplicate-each-row-imported-constants-negative/cycle.test.ts"),
         vec![("cycle.ts",include_str!("../../../fixtures/recall/duplicate-each-row-imported-constants-negative/cycle.ts")),
              ("cycle-writer.ts",include_str!("../../../fixtures/recall/duplicate-each-row-imported-constants-negative/cycle-writer.ts"))]),
        (include_str!("../../../fixtures/recall/duplicate-each-row-imported-constants-negative/defined.test.ts"),
         vec![("defined.ts",include_str!("../../../fixtures/recall/duplicate-each-row-imported-constants-negative/defined.ts")),
              ("defined-barrel.ts",include_str!("../../../fixtures/recall/duplicate-each-row-imported-constants-negative/defined-barrel.ts"))]),
        (include_str!("../../../fixtures/recall/duplicate-each-row-imported-constants-negative/deleted.test.ts"),
         vec![("deleted.ts",include_str!("../../../fixtures/recall/duplicate-each-row-imported-constants-negative/deleted.ts"))]),
    ];
    for (source, mut files) in cases {
        files.push(("cases.test.ts", source));
        let corpus = corpora(&files, &["cases.test.ts"]).remove(0);
        assert_eq!(corpus.tests.len(), 2);
        assert!(corpus.tests.iter().all(|c| c.exclusion.is_none()));
        assert_owned_row_findings(corpus, &[]);
    }
}

/// A tag without interpolation is one string row in Vitest, including the empty string.
#[test]
fn nc64_tags_without_interpolations_preserve_each_invocation() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/no-cells.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 6, 0);
    assert!(corpus
        .tests
        .iter()
        .all(|c| c.row_class.is_some() || !c.suite_rows.is_empty()));
    assert_owned_row_findings(corpus, &[]);
}

/// Resolving an imported null preserves the different each/for title argument conventions.
#[test]
fn nc64_imported_null_titles_keep_whole_row_argument_rules() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/null.test.ts");
    let values = include_str!("../../../fixtures/oracle/row-value-tables/null-values.ts");
    let corpus = corpora(
        &[("cases.test.ts", source), ("null-values.ts", values)],
        &["cases.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 4);
    assert!(corpus.tests.iter().all(|c| c.exclusion.is_none()));
    assert_eq!(
        corpus
            .tests
            .iter()
            .map(|c| c.full_title.as_deref())
            .collect::<Vec<_>>(),
        [
            Some("whole undefined"),
            Some("whole null"),
            Some("spread null"),
            Some("suite undefined > nested")
        ]
    );
    assert!(!owned_findings("cases.test.ts", corpus)
        .findings
        .iter()
        .any(|f| f.rule == Rule::DuplicateTestName));
}

/// Float prefixes depend on their occurrence ordinal, separately from the consumed argument.
#[test]
fn nc64_float_titles_preserve_vitest_template_interactions() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/mixed-float.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 4, 0);
    assert_eq!(
        corpus
            .tests
            .iter()
            .map(|c| c.full_title.as_deref())
            .collect::<Vec<_>>(),
        [
            Some("mixed 1 0"),
            Some("mixed 1 -0"),
            Some("prefix -0 -1"),
            Some("floats -0 -0 -0")
        ]
    );
    assert!(!owned_findings("cases.test.ts", corpus)
        .findings
        .iter()
        .any(|f| f.rule == Rule::DuplicateTestName));
}

/// Inherited receiver methods invalidate own-member proofs even without syntactic assignment.
#[test]
fn nc64_receiver_calls_refuse_member_scalar_proofs() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/receiver.test.ts");
    assert_owned_row_findings(counted_corpus(source, "cases.test.ts", 2, 0), &[]);
}

/// Significant header whitespace cannot become a different proven object field.
#[test]
fn nc64_header_whitespace_does_not_invent_a_field() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/header.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 2, 0);
    assert!(!owned_findings("cases.test.ts", corpus)
        .findings
        .iter()
        .any(|f| f.rule == Rule::DuplicateTest));
}

/// Existing literal-array suite inputs retain comparison, without equating different invocations.
#[test]
fn nc64_readonly_suite_arrays_preserve_comparison_within_each_invocation() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/arrays.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 4, 0);
    assert_ne!(corpus.tests[0].key, corpus.tests[1].key);
    assert_eq!(corpus.tests[0].key, corpus.tests[2].key);
    let findings = owned_findings("cases.test.ts", corpus);
    let exact = findings
        .findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateTest)
        .collect::<Vec<_>>();
    assert_eq!(exact.len(), 1, "{exact:?}");
    assert_eq!(exact[0].cases.len(), 4);
}

/// Mutations, aliases, escapes and inherited array accesses cannot enter the retained route.
#[test]
fn nc64_suite_arrays_refuse_mutation_and_escape() {
    for operation in [
        "scope.push('write')",
        "scope.length = 0",
        "delete scope[0]",
        "const alias = scope",
        "consume(scope)",
        "consume(scope[99])",
    ] {
        let source = format!(
            "{HEADER}\n\
          describe.each([[['read']], [['read','write']]])('scope', scope => {{\n\
            {operation}; test('child', () => expect(scope.length).toBeGreaterThan(0));\n\
          }});"
        );
        let corpus = counted_corpus(&source, "cases.test.ts", 2, 2);
        assert_owned_row_findings(corpus, &[]);
    }
}

/// The pre-existing hook/suite corpus remains comparable with its own literal-array inputs.
#[test]
fn nc64_existing_hook_suite_array_fixture_keeps_its_eligible_children() {
    let source =
        include_str!("../../../fixtures/duplicates/redundant-hook-describe-each-suite.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 2, 0);
    assert_eq!(
        corpus.tests[0].full_title.as_deref(),
        Some("admin permissions > grants its scope")
    );
    assert_eq!(
        corpus.tests[1].full_title.as_deref(),
        Some("viewer permissions > grants its scope")
    );
}

/// A different member read can invoke an inherited getter and invalidate the selected own value.
#[test]
fn nc64_inherited_reads_cannot_certify_a_constant_own_member() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/prototype.test.ts");
    assert_owned_row_findings(counted_corpus(source, "cases.test.ts", 2, 0), &[]);
}

/// Closed registration admits literal options and tags, while calls and accessors stay unproved.
#[test]
fn nc64_suite_claims_require_closed_collection_and_keep_literal_options() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/suite-options.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 8, 0);
    let rows = row_claims(corpus);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        rows.iter()
            .map(|f| f.titles[0].as_str())
            .collect::<Vec<_>>(),
        ["options", "tagged"]
    );
}

/// A local API alias remains outside the prefix proof while its direct-import twin stays claimed.
#[test]
fn nc64_direct_suite_prefix_control_retains_the_derived_refusal() {
    let source = include_str!("../../../fixtures/oracle/row-value-tables/prefix.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 4, 0);
    let rows = row_claims(corpus);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0]
        .titles
        .iter()
        .all(|title| title.starts_with("direct >")));
}
