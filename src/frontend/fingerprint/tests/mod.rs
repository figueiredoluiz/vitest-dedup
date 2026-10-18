use super::*;
use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_ast::builder::AstBuilder;
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::{SourceType, SPAN};

/// Finds every top-level `test(...)`/`it(...)` arrow-function callback body, in source
/// order, and fingerprints each one immediately during the visit.
///
/// Fingerprinting inline (rather than collecting borrowed node references to process
/// afterward) sidesteps a real lifetime constraint, not just a style preference: the `Visit`
/// trait hands each callback a reference whose lifetime is fresh per call, not tied to the
/// arena lifetime `'a` in a way this crate's types can name. Producing an owned
/// [`Fingerprints`] before returning from the callback avoids needing to smuggle that
/// reference out, which in turn avoids the raw-pointer-and-`unsafe` workaround this crate's
/// `#![forbid(unsafe_code)]` rules out anyway.
struct BodyCollector<'a, 'b> {
    semantic: &'b Semantic<'a>,
    source: &'b str,
    file: &'b Path,
    results: Vec<BodyFingerprint>,
}

impl<'a, 'b> Visit<'a> for BodyCollector<'a, 'b> {
    /// Collect arrow-function test callback fingerprints, then continue walking nested call
    /// expressions.
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let is_test_call = matches!(&it.callee, Expression::Identifier(ident) if matches!(ident.name.as_str(), "test" | "it"));
        if is_test_call {
            if let Some(Argument::ArrowFunctionExpression(arrow)) = it.arguments.get(1) {
                self.results.push(fingerprint(
                    FunctionLike::Arrow(arrow),
                    self.semantic,
                    self.source,
                    self.file,
                    None,
                ));
            }
        }
        walk::walk_call_expression(self, it);
    }
}

/// Fingerprints every test body in `source`, as if it were the file at `file` -- callers that
/// don't care which path is used (most of them; the shared identity helpers are what's under
/// test elsewhere) should use [`fingerprint_all`] instead.
/// Both bodies' behavior events, each sorted, asserting exactly two bodies: near-duplicate
/// comparison is a multiset comparison, so order differences must not survive into what
/// these tests assert on.
fn sorted_behavior_pair(prints: &[BodyFingerprint]) -> (Vec<String>, Vec<String>) {
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    let mut a = prints[0].behavior.clone();
    let mut b = prints[1].behavior.clone();
    a.sort();
    b.sort();
    (a, b)
}

/// Parse and analyze a source fixture, then fingerprint its arrow test bodies using the
/// supplied file identity.
fn fingerprint_all_at(source: &str, file: &Path) -> Vec<BodyFingerprint> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(file).unwrap();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    assert!(
        !parsed.fatal_error,
        "fixture failed to parse: {:?}",
        parsed.diagnostics
    );
    let program: &Program = &parsed.program;
    let semantic_ret = SemanticBuilder::new().with_build_nodes(true).build(program);
    assert!(
        semantic_ret.diagnostics.is_empty(),
        "unexpected semantic diagnostics: {:?}",
        semantic_ret.diagnostics
    );
    let semantic = semantic_ret.semantic;

    let mut collector = BodyCollector {
        semantic: &semantic,
        source,
        file,
        results: Vec::new(),
    };
    collector.visit_program(program);
    collector.results
}

/// Fingerprint arrow test callbacks in the standard case.test.ts fixture file.
fn fingerprint_all(source: &str) -> Vec<BodyFingerprint> {
    fingerprint_all_at(source, Path::new("case.test.ts"))
}

/// [`alpha_of`] in `.tsx`, so JSX tests read like the corpus. `fingerprint_all_at` keys the
/// parser's `SourceType` off the path it is handed, which is what makes a JSX source type a
/// one-argument choice here.
fn alpha_of_tsx(body: &str) -> String {
    alpha_in(Path::new("case.test.tsx"), body)
}

/// [`structural_of`] in `.tsx`, for the `parameterization-candidate` half of every JSX claim.
fn structural_of_tsx(body: &str) -> String {
    structural_in(Path::new("case.test.tsx"), body)
}

/// [`alpha_of`] with the file the body is parsed as, so a JSX-parsing source type can ask the
/// same question. One parameterized body for the three `alpha` helpers, because the
/// duplication gate holds test code to a recorded ceiling too.
fn alpha_in(file: &Path, body: &str) -> String {
    let prints = fingerprint_all_at(&format!("test('t', () => {{ {body} }});"), file);
    assert_eq!(prints.len(), 1, "expected one test body for {body:?}");
    prints.into_iter().next().expect("one body").alpha
}

/// [`structural_of`] with the file, on the same terms as [`alpha_in`].
fn structural_in(file: &Path, body: &str) -> String {
    let prints = fingerprint_all_at(&format!("test('t', () => {{ {body} }});"), file);
    assert_eq!(prints.len(), 1, "expected one test body for {body:?}");
    prints.into_iter().next().expect("one body").structural
}

/// The import map can be read from a parse the caller already holds (M12 groundwork): a `.ts`
/// file with a legacy `<T>value` cast yields nothing through [`import_origins`]'s TSX parse,
/// and every non-type import through [`import_origins_from_program`] on its real-type parse.
#[test]
fn import_origins_from_program_reads_what_the_tsx_parse_drops() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("cast.test.ts");
    let source = concat!(
        "import { a as alias, type T } from './m'\n",
        "import def from './d'\n",
        "import * as ns from 'pkg'\n",
        "import type { U } from './t'\n",
        "const cast = <number>(1 as unknown)\n",
    );
    assert!(
        import_origins(source, &file).is_empty(),
        "the TSX parse rejects the cast"
    );

    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::from_path(&file).unwrap()).parse();
    assert!(
        !parsed.fatal_error && parsed.diagnostics.is_empty(),
        "the real-type parse accepts the file: {:?}",
        parsed.diagnostics
    );
    let origins = import_origins_from_program(&parsed.program, &file);
    let mut entries: Vec<_> = origins
        .iter()
        .map(|(local, origin)| {
            (
                local.as_str(),
                origin.exported.as_str(),
                origin.specifier.as_str(),
            )
        })
        .collect();
    entries.sort_unstable();
    assert_eq!(
        entries,
        vec![
            ("alias", "a", "./m"),
            ("def", "default", "./d"),
            ("ns", "*", "pkg")
        ],
        "type-only imports are skipped"
    );
}

/// A local binding and an import with the same name are different identities, so their
/// bodies are not alpha-equal.
#[test]
fn local_vs_imported_name_are_not_alpha_equal() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/duplicates/not-duplicates-local-vs-imported-name.test.ts"
    ))
    .expect("fixture present");
    let prints = fingerprint_all(&source);
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    assert_ne!(
        prints[0].alpha, prints[1].alpha,
        "an imported `page` and a body-local `page` must not normalize to the same alpha \
             identity -- this is the whole reason symbol resolution replaces name matching"
    );
}

#[test]
fn alpha_equivalent_bodies_match() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/duplicates/duplicates-alpha-equivalent.test.ts"
    ))
    .expect("fixture present");
    let prints = fingerprint_all(&source);
    assert_eq!(prints.len(), 2);
    assert_eq!(
        prints[0].alpha, prints[1].alpha,
        "same structure with differently-named locals, and the same imported `compute` free \
             reference, must normalize to the same alpha"
    );
}

// -----------------------------------------------------------------------------------------

mod assertions;
mod callable_forms;
mod classes;
mod dataflow;
mod depth;
mod helper_inlining;
mod helper_inlining_cases;
mod jsx;
mod node_matrix;
mod row_substitution;
mod row_substitution_each;
mod type_arguments;

/// `alpha` for one test callback body.
fn alpha_of(body: &str) -> String {
    alpha_in(Path::new("case.test.ts"), body)
}

/// Assert the prohibited merge outcomes without weakening extraction or uncertainty checks.
fn assert_class_distinct(findings: &[crate::model::Finding], source: &str) {
    use crate::model::Rule;
    assert!(
        !findings.iter().any(|finding| matches!(
            finding.rule,
            Rule::DuplicateTest | Rule::NearDuplicateTest | Rule::SubsumedTest
        )),
        "{source}: {findings:?}"
    );
}

/// The rendered `subject=` shape of the first assertion event in `body`.
fn assertion_subject(body: &str) -> String {
    let events = behavior_of(body);
    let event = events
        .iter()
        .find(|event| event.starts_with("assert:"))
        .unwrap_or_else(|| panic!("no assertion event for `{body}`: {events:?}"));
    let start = event.find("subject=").expect("subject in event") + "subject=".len();
    let end = event.find("; value=").expect("value in event");
    event[start..end].to_owned()
}

/// `alpha` for one `async` test callback body.
fn async_alpha_of(body: &str) -> String {
    let prints = fingerprint_all(&format!("test('t', async () => {{ {body} }});"));
    assert_eq!(prints.len(), 1, "expected one test body for {body:?}");
    prints.into_iter().next().expect("one body").alpha
}

/// `behavior` for one test callback body.
fn behavior_of(body: &str) -> Vec<String> {
    let prints = fingerprint_all(&format!("test('t', () => {{ {body} }});"));
    assert_eq!(prints.len(), 1, "expected one test body for {body:?}");
    prints.into_iter().next().expect("one body").behavior
}

/// Assert extraction, unresolved scope and final rule results for an owned class matrix.
fn class_outcome_findings(
    source: &str,
    tests: usize,
    unresolved: usize,
) -> Vec<crate::model::Finding> {
    class_outcome_findings_with_files(source, tests, unresolved, &[])
}

/// Resolve owned dependency files before asserting extraction and final rule outcomes.
fn class_outcome_findings_with_files(
    source: &str,
    tests: usize,
    unresolved: usize,
    files: &[(&str, &str)],
) -> Vec<crate::model::Finding> {
    use crate::config::{Config, ConfigOverrides};
    use crate::discovery::{ModuleFlavor, SourceFile, SourceLanguage};
    let directory = tempfile::tempdir().unwrap();
    for (name, content) in files {
        std::fs::write(directory.path().join(name), content).unwrap();
    }
    let config_path = directory.path().join("settings.json");
    let settings = files
        .iter()
        .find(|(name, _)| *name == "settings.json")
        .map_or("{}", |(_, contents)| *contents);
    std::fs::write(&config_path, settings).unwrap();
    let config = Config::load(
        directory.path(),
        ConfigOverrides {
            config_file: Some(config_path),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();
    let file = SourceFile::without_project_context(
        directory.path().join("constructor.test.ts"),
        SourceLanguage::TypeScript,
        ModuleFlavor::ForcedEsm,
    );
    let extraction = crate::frontend::extract(source, &file, crate::frontend::Globals::Disabled);
    assert_eq!(extraction.tests.len(), tests, "{source}");
    assert!(!extraction.incomplete, "{:?}", extraction.diagnostics);
    let result =
        crate::analysis::analyze(extraction.tests, &config, 1, Default::default()).unwrap();
    assert_eq!(result.unresolved_tests, unresolved, "{source}");
    assert_eq!(result.incomplete(), unresolved > 0, "{source}");
    result.findings
}

/// Analyze two owned tests with explicit extraction and completeness assertions.
fn complete_pair_findings(source: &str) -> Vec<crate::model::Finding> {
    class_outcome_findings(source, 2, 0)
}

/// The statement whose purity is under test, wrapped between a single-use `const` and its
/// one read. `@` is the intervening statement.
fn inlining_probe(intervening: &str) -> String {
    format!("let y; const subject = build(1); {intervening} expect(subject).toBe(3);")
}
