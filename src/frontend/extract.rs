//! Parses one source file and walks it into the owned [`TestCase`] tree.
//!
//! `registry.rs` answers "does this callee refer to Vitest, and through what chain"; this module
//! is everything downstream of that answer: recursing into `describe` bodies while tracking
//! suite paths and hook scopes, inlining a local helper's body at each site that **calls** it,
//! deciding what a title statically is, expanding `.each`/`.for` rows, and assembling the final
//! [`TestCase`] values. Parsing happens inside one arena scope per
//! file, and only owned data crosses back out: oxc's `Allocator` owns a bump arena, and the
//! `Program`, every AST node inside it, and the `Semantic` built from it all borrow that arena for
//! its lifetime. Nothing arena-backed can outlive the allocator, so this module extracts `String`s
//! and `Copy` `Span` offsets while the allocator is alive rather than stashing a node reference to
//! read later.

use crate::discovery::{ModuleFlavor, SourceFile};
use crate::model::{
    Context, DeclarationKind, EachRows, EachTable, Fingerprints, FixtureBinding, Hook, HookKind,
    Location, MockFactory, MockedModule, Modifier, Position, TestCase, Title,
};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionBody, AssignmentOperator, AssignmentTarget,
    BindingPattern, CallExpression, Expression, ForOfStatement, ForStatementLeft, FormalParameters,
    Function, FunctionType, IdentifierReference, ImportDeclarationSpecifier, ObjectExpression,
    ObjectPropertyKind, Program, Statement, TemplateLiteral, UnaryOperator, VariableDeclarator,
};
use oxc_ast::AstKind;
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::{ScopeFlags, Scoping, Semantic, SemanticBuilder, SymbolId};
use oxc_span::{GetSpan, SourceType, Span};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

mod imported;

use super::fingerprint::{fingerprint, FunctionLike, RowSubstitution};
use super::registry::{
    canonicalize_export_name, const_fold, is_ecmascript_trim_whitespace, js_number_to_string,
    js_radix_digits_value, property_key_name, resolve_callee, source_text, split_radix_prefix,
    CellValue, ChainStep, Globals, Registry, ResolvedCallee, Row, RowCells, TitleArg, TitleNumber,
};
use super::FileExtraction;

/// Parses `source` and extracts every Vitest declaration it can prove statically.
///
/// Never panics: a parse failure, a fatal parser error, or an internal bug that would otherwise
/// unwind is converted into an empty, `incomplete` extraction with a diagnostic string, per the
/// product rule that absence of a finding must never be mistaken for proof of absence.
pub(super) fn extract(source: &str, file: &SourceFile, globals: Globals) -> FileExtraction {
    extract_with_imports(source, file, globals, true)
}

/// Extracts owned declarations under an explicit current-tree or historical import policy.
pub(super) fn extract_with_imports(
    source: &str,
    file: &SourceFile,
    globals: Globals,
    allow_imported: bool,
) -> FileExtraction {
    if let Some(reason) = parse_refusal(source) {
        return refused_extraction(reason);
    }
    // A finite input budget and an explicit stack work together: stack size alone only moves
    // the crash threshold, while a lexical bracket count alone misses regexes and operator chains.
    let result = std::thread::scope(|scope| {
        #[cfg(test)]
        let census = super::registry::test_cell_census_snapshot();
        let worker = std::thread::Builder::new()
            .name("vitest-dedup-extract".to_owned())
            .stack_size(EXTRACTION_STACK_BYTES)
            .spawn_scoped(scope, move || {
                #[cfg(test)]
                super::registry::restore_test_cell_census(census);
                super::registry::reset_cell_census_file_scope();
                let extraction = run(source, file, globals, allow_imported);
                #[cfg(test)]
                let extraction = (extraction, super::registry::test_cell_census_snapshot());
                extraction
            })
            .map_err(|error| format!("could not start bounded extraction thread: {error}"))?;
        let extraction = worker
            .join()
            .map_err(|_| "internal error: extraction panicked and was recovered".to_owned())?;
        #[cfg(test)]
        let extraction = {
            let (extraction, census) = extraction;
            super::registry::restore_test_cell_census(census);
            extraction
        };
        Ok(extraction)
    });
    result.unwrap_or_else(refused_extraction)
}

/// Visible refusal shared by preflight, thread creation failure and recovered worker panics.
fn refused_extraction(reason: String) -> FileExtraction {
    FileExtraction {
        helper_sources: Default::default(),
        tests: Vec::new(),
        incomplete: true,
        diagnostics: vec![reason],
        proven_empty: false,
    }
}

/// Explicit stack headroom for parsing, semantic construction, AST visitors and helper frames.
/// The independent raw syntax budget below is essential: this stack is finite, not crash recovery.
const EXTRACTION_STACK_BYTES: usize = 128 * 1024 * 1024;

/// Maximum lexical bracket depth, retaining the existing tighter limit on ordinary source.
/// This scan is an admission heuristic, not a safety bound: regex/JSX can confuse its masking.
pub(super) const MAX_NESTING_DEPTH: usize = 256;

/// Independent upper bound on recursion-bearing raw syntax units. It deliberately also counts
/// text in strings/comments/regexes, so lexical ambiguity cannot hide recursive syntax. Wide or
/// punctuation-heavy generated inputs may be refused even when their actual syntax is shallow.
const MAX_RAW_SYNTAX_UNITS: usize = 8_192;

/// Checks every production parser entry before Oxc sees source. No JS tokenization or recursion.
/// Keywords cover bracket-free unary/control/type chains; punctuation covers calls, member and
/// label chains, arrows, JSX, operators, nested templates and bracket nesting. Commas, digits and
/// ordinary identifier text are uncharged, retaining flat data without pretending to parse JS.
pub(super) fn parse_refusal(source: &str) -> Option<String> {
    if source.len() > crate::discovery::MAX_SOURCE_FILE_BYTES as usize {
        return Some(
            "file exceeds the maximum supported source size; skipped before parsing".to_owned(),
        );
    }
    let bytes = source.as_bytes();
    let mut units = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || matches!(bytes[i], b'_' | b'$') {
            let start = i;
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'$'))
            {
                i += 1;
            }
            if matches!(
                &bytes[start..i],
                b"if"
                    | b"else"
                    | b"for"
                    | b"while"
                    | b"do"
                    | b"new"
                    | b"void"
                    | b"typeof"
                    | b"delete"
                    | b"await"
                    | b"yield"
                    | b"return"
                    | b"throw"
                    | b"in"
                    | b"instanceof"
                    | b"as"
                    | b"satisfies"
                    | b"keyof"
                    | b"readonly"
                    | b"infer"
                    | b"unique"
                    | b"extends"
                    | b"declare"
            ) {
                units += 1;
            }
        } else {
            if matches!(
                bytes[i],
                b'(' | b'['
                    | b'{'
                    | b'<'
                    | b'>'
                    | b'='
                    | b'.'
                    | b':'
                    | b'?'
                    | b'!'
                    | b'+'
                    | b'-'
                    | b'*'
                    | b'/'
                    | b'%'
                    | b'&'
                    | b'|'
                    | b'^'
                    | b'~'
                    | b'`'
                    | b'@'
                    | b'\\'
            ) {
                units += 1;
            }
            i += 1;
        }
        if units > MAX_RAW_SYNTAX_UNITS {
            return Some(format!("file exceeds the maximum supported raw syntax budget ({MAX_RAW_SYNTAX_UNITS} units); skipped before parsing to avoid a stack overflow"));
        }
    }
    let depth = max_nesting_depth(source);
    (depth > MAX_NESTING_DEPTH).then(|| format!(
        "file exceeds the maximum supported bracket nesting depth ({depth} > {MAX_NESTING_DEPTH}); skipped without parsing to avoid a parser stack overflow"
    ))
}

/// How many helper frames [`Walker::begin_helper_expansion`] will hold open at once.
///
/// S11's fix inlines a called helper's body at the call site, so a helper that calls another
/// helper nests one Rust recursion (`walk_statement` -> `walk_helper_call` -> `walk_statement`)
/// per level, on top of whatever statement nesting each body already contributes. This is the
/// stack-safety half of the bound, the same concern [`MAX_NESTING_DEPTH`] exists for, and it also
/// caps the *shape* of the search: with `B` helper calls per body the expansion tree is at most
/// `B^8` nodes rather than unbounded. Real helper chains in test files are one or two deep (the
/// files this fix was measured against are one). Past eight the declarations
/// below that depth are not counted -- the same declarations the walker missed for every helper
/// body before S11, except that this says so: refusing raises a diagnostic and marks the file
/// incomplete rather than dropping them silently (see [`Walker::refuse_helper_expansion`]).
const MAX_HELPER_EXPANSION_DEPTH: usize = 8;

/// How many helper bodies one file may expand in total, across both walker passes.
///
/// [`MAX_HELPER_EXPANSION_DEPTH`] bounds the *depth* of the expansion tree but not its *size*:
/// `B^8` is a bound only in the sense that `B` is bounded by the file's own length, so a
/// machine-generated file with a hundred helper calls per body could still cost `10^16` body
/// walks. This budget makes the total work linear instead -- at most this many body walks per
/// file, whatever its call structure -- so a hostile input costs a diagnostic rather than a hang.
/// Two thousand is far beyond what any hand-written test file reaches: measured, the two
/// measured files carrying this shape spend three of it each per pass, six per file in total.
const MAX_HELPER_EXPANSIONS: usize = 2_000;

/// Estimates bracket nesting with a lexical masking heuristic, without parsing `source`.
///
/// A naive byte-level scan counting `(`/`[`/`{` against `)`/`]`/`}` cannot tell a real bracket
/// from one that is only *text* -- inside a string literal, a comment, or a template literal's
/// literal-text portions -- and decrementing on one of those is unsound: a string containing a
/// bare `]` (or any other closer) drags the running counter down without a matching real open,
/// so genuinely deep nesting that follows can under-count all the way back below the limit. That
/// is not merely imprecise, it defeats the one purpose this guard exists for (see
/// [`MAX_NESTING_DEPTH`]'s doc) -- oxc's stack overflow on deep nesting aborts the whole process
/// and is not something `catch_unwind` can recover from.
///
/// So this scan masks the same three regions `crate::vitest_config`'s `mask_non_code` does (read
/// for the approach; this is a separate, self-contained implementation, not a shared one, per
/// this frontend's parser-containment boundary): line comments, block comments, `'`/`"` strings,
/// and a template literal's literal-text portions, while still counting brackets that are real
/// code, including a template literal's `${ ... }` holes, recursively. *Opening* brackets found
/// inside one of those regions are masked too, not just closing ones -- the goal is to never let
/// non-code content move the counter in either direction, since a mask that only suppressed
/// decrements while still counting opens would let a string inflate `depth` past the limit and
/// reject a file that never actually nests that deep (a false rejection, which is safe, but an
/// unnecessary one this scan can just as easily avoid).
///
/// For actual quoted text, an unterminated string masks to the end of its line; an unterminated
/// block comment or template masks to EOF. Regex literals and JSX are not recognized, however,
/// so this heuristic can undercount. The independent raw syntax budget in [`parse_refusal`]
/// bounds admission even when masking mistakes code for text.
pub(super) fn max_nesting_depth(source: &str) -> usize {
    /// One entry of the template-literal context stack.
    enum Frame {
        /// Between a template literal's backticks and its next `${` (or closing backtick):
        /// literal text, which can never contain a real bracket.
        TemplateText,
        /// Inside a `${ ... }` hole, which is ordinary code. Carries the brace depth already
        /// opened within this specific hole (starting at `0` right after `${` is seen), so the
        /// hole's own closing `}` can be told apart from one belonging to a nested literal inside
        /// it.
        Hole(u32),
    }

    let bytes = source.as_bytes();
    let n = bytes.len();
    let mut depth: usize = 0;
    let mut max_depth: usize = 0;
    let mut frames: Vec<Frame> = Vec::new();
    let mut i = 0usize;

    while i < n {
        if matches!(frames.last(), Some(Frame::TemplateText)) {
            match bytes[i] {
                b'\\' if i + 1 < n => i += 2,
                b'`' => {
                    frames.pop();
                    i += 1;
                }
                b'$' if i + 1 < n && bytes[i + 1] == b'{' => {
                    // Mask only the `$`; the `{` is real code and is left for the ordinary
                    // `b'(' | b'[' | b'{'` arm below to count on the next iteration -- consuming
                    // it here too would double-count it.
                    frames.push(Frame::Hole(0));
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }

        // Ordinary code: top level, or inside a `${ ... }` hole.
        match bytes[i] {
            b'/' if i + 1 < n && bytes[i + 1] == b'/' => {
                i += 2;
                while i < n && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < n && bytes[i + 1] == b'*' => {
                i += 2;
                while i < n && !(bytes[i] == b'*' && i + 1 < n && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = if i < n { i + 2 } else { n };
            }
            quote @ (b'\'' | b'"') => {
                i += 1;
                while i < n {
                    if bytes[i] == b'\\' && i + 1 < n {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == quote {
                        i += 1;
                        break;
                    }
                    if bytes[i] == b'\n' {
                        // An unescaped newline never legally appears inside a single/double-
                        // quoted JS string; resume ordinary scanning right here rather than mask
                        // further (possibly real) content.
                        break;
                    }
                    i += 1;
                }
            }
            b'`' => {
                frames.push(Frame::TemplateText);
                i += 1;
            }
            open @ (b'(' | b'[' | b'{') => {
                depth += 1;
                max_depth = max_depth.max(depth);
                if open == b'{' {
                    if let Some(Frame::Hole(hole_depth)) = frames.last_mut() {
                        *hole_depth += 1;
                    }
                }
                i += 1;
            }
            close @ (b')' | b']' | b'}') => {
                depth = depth.saturating_sub(1);
                if close == b'}' {
                    if let Some(Frame::Hole(hole_depth)) = frames.last_mut() {
                        *hole_depth = hole_depth.saturating_sub(1);
                        if *hole_depth == 0 {
                            frames.pop();
                        }
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
    }

    max_depth
}

/// Builds one bounded parser/semantic frame and returns only owned extraction records.
fn run(source: &str, file: &SourceFile, globals: Globals, allow_imported: bool) -> FileExtraction {
    let Some(source_type) = build_source_type(file) else {
        return FileExtraction {
            helper_sources: Default::default(),
            tests: Vec::new(),
            incomplete: true,
            diagnostics: vec!["unsupported source type".to_owned()],
            proven_empty: false,
        };
    };

    let allocator = Allocator::default();
    let parser_return = Parser::new(&allocator, source, source_type).parse();
    if parser_return.fatal_error {
        return FileExtraction {
            helper_sources: Default::default(),
            tests: Vec::new(),
            incomplete: true,
            diagnostics: vec!["fatal parse error".to_owned()],
            proven_empty: false,
        };
    }
    let mut diagnostics = Vec::new();
    if !parser_return.diagnostics.is_empty() {
        diagnostics.push(format!(
            "{} parse diagnostic(s)",
            parser_return.diagnostics.len()
        ));
    }

    let program = &parser_return.program;
    // `with_build_nodes` is off by default (`SemanticBuilder`'s own doc: the compiler pipeline
    // serves ancestry from a lightweight stack and leaves `Semantic::nodes` empty). S11 needs
    // random access to it -- `Semantic::symbol_declaration` is how `Walker::resolve_local_helper`
    // gets from a called identifier's binding to the `function` that declares it, by symbol
    // identity rather than by name. Without this the node store is empty and that lookup panics
    // out of bounds, which `extract`'s `catch_unwind` turns into an empty, `incomplete`
    // extraction -- silently the pre-S11 answer, arrived at the worst possible way. The store is
    // per-file and dropped with the allocator scope at the end of `run`, so nothing it costs is
    // retained across the corpus.
    let semantic_return = SemanticBuilder::new().with_build_nodes(true).build(program);
    if !semantic_return.diagnostics.is_empty() {
        diagnostics.push(format!(
            "{} semantic diagnostic(s)",
            semantic_return.diagnostics.len()
        ));
    }
    let semantic = &semantic_return.semantic;

    let mut registry = Registry::new(program, globals);
    let mut walker = Walker::new(source, file, program, semantic, globals);
    walker.allow_imported = allow_imported;
    walker.walk_statements(&program.body, &mut registry);
    diagnostics.extend(walker.diagnostics);

    let incomplete = !diagnostics.is_empty();
    // See `Walker::proven_empty_tables`'s doc: only meaningful when nothing else in this file
    // contributed a test (otherwise `tests` would not be empty) and extraction found nothing else
    // wrong with it -- a diagnostic alongside a proven-empty table means this file's zero-test
    // outcome is not *fully* explained by the empty table alone, so the ordinary explanation must
    // still apply.
    let proven_empty =
        walker.tests.is_empty() && diagnostics.is_empty() && walker.proven_empty_tables > 0;
    FileExtraction {
        tests: walker.tests,
        helper_sources: walker.helper_sources,
        incomplete,
        diagnostics,
        proven_empty,
    }
}

/// Derive the parser source type from the path and apply any forced module flavor; return None
/// for unsupported paths.
fn build_source_type(file: &SourceFile) -> Option<SourceType> {
    let source_type = SourceType::from_path(&file.path).ok()?;
    Some(match file.module_flavor {
        ModuleFlavor::Ambiguous => source_type,
        ModuleFlavor::ForcedEsm => source_type.with_module(true),
        ModuleFlavor::ForcedCjs => source_type.with_module(false),
    })
}

/// Maps every imported local symbol (from any specifier, not just `"vitest"`) to an anchored
/// identity for the module it came from, for [`Context::imports`]. This is deliberately broader
/// than the Vitest-specific [`Registry`]: a test body's free identifiers can resolve to any
/// helper module.
///
/// The value used to be the specifier's raw text (`./helpers`), file-relative and
/// un-anchored -- the same "unique in one scope, wrong in a wider one" defect
/// [`crate::model::MockedModule::specifier`]'s doc catalogs the rest of the instances of. Routed
/// through [`super::fingerprint::specifier_identity`] instead -- reused, not reinvented, the same
/// `resolved:`/`unresolved:` scheme `collect_mocked_modules` already uses for `vi.mock`/
/// `vi.unmock` targets -- so two specifiers naming the same real file agree regardless of
/// spelling, and an unresolvable specifier is tagged distinctly from every resolved one rather
/// than falling back to the very raw text this exists to stop comparing.
/// Module-top-level **awaited** call expressions, in source order, as normalized source text.
///
/// See [`crate::model::Context::module_setup`] for why `await` is the admission rule and why the
/// payload is text. Only `program.body` is scanned -- an awaited call nested inside an `if`, a
/// `describe`, or a function body is not a fact every test in the file inherits, and the same
/// "is it at module top level" question `collect_mocked_modules` answers structurally is answered
/// structurally here rather than by walking and hoping.
fn collect_module_setup(
    program: &Program,
    source: &str,
    semantic: &Semantic,
    import_specifiers: &HashMap<SymbolId, Arc<str>>,
    exported_as: &HashMap<SymbolId, String>,
) -> Vec<String> {
    let mut calls = Vec::new();
    for statement in &program.body {
        let Statement::ExpressionStatement(statement) = statement else {
            continue;
        };
        let admitted = match &statement.expression {
            Expression::AwaitExpression(await_expression) => {
                matches!(&await_expression.argument, Expression::CallExpression(_))
                    .then(|| await_expression.argument.span())
            }
            Expression::CallExpression(call) => {
                is_ambient_state_call(call, semantic, import_specifiers, exported_as)
                    .then(|| call.span())
            }
            _ => None,
        };
        let Some(span) = admitted else { continue };
        // RAW text, deliberately. This used to collapse whitespace so that reformatting a config
        // object would not change the key — which also collapsed whitespace INSIDE string
        // literals, so `vi.stubEnv('MODE', 'a  b')` and `vi.stubEnv('MODE', 'a b')` produced one
        // entry and two files stubbing different values compared equal. Measured: that is the very
        // `cross-file-duplicate` false positive this function was just widened to prevent,
        // reintroduced through the normalizer.
        //
        // The same class was settled once already, in `config_test_scope`: normalizing formatting
        // without changing meaning needs a JavaScript lexer, a literal-aware hand-rolled scanner
        // still gets regex literals wrong, and the failure is always in the direction that MERGES
        // two different things. Raw text over-separates instead — reformatting an awaited setup
        // call changes its key and suppresses a finding — which is the safe direction for a field
        // whose whole job is to stop two files being called equal.
        calls.push(source_text(source, span).to_owned());
    }
    calls
}

/// Whether a module-top-level call establishes ambient state every test in the file inherits.
///
/// The second admission rule for [`collect_module_setup`], and the narrowest one that closes a
/// four-instance false-positive class. `Context` saw only AWAITED module-scope calls, so a
/// synchronous one was invisible and two files differing *only* in it compared equal —
/// `cross-file-duplicate` then reported them "under an equivalent context" for:
///
/// ```text
/// vi.useFakeTimers()                          vs  vi.useRealTimers()
/// vi.spyOn(flags, 'isEnabled').mockReturnValue(true)  vs  ...(false)
/// vi.stubEnv('MODE', 'production')            vs  ...'development'
/// expect.extend({ toBeWithin: inclusive })    vs  ...exclusive
/// ```
///
/// In the last of those, exactly one of the two "identical" tests passes.
///
/// **Why not simply admit every module-scope call.** [`crate::model::Context::module_setup`]'s
/// doc rules it out in as many words: since S11 a module-scope helper call can *declare tests*,
/// and folding it into the context key would key those tests on their own declaration. So the
/// rule keys on the CALLEE instead — a member call on a resolved `vi` or `expect` namespace is
/// the framework changing ambient behaviour, and a call to anything else is ordinary program
/// text. That resolution is [`is_vitest_namespace`], the same three-state binding check
/// `redundant-hook` uses, so a file-local `const vi = createFakeVi()` does not qualify here
/// either.
///
/// **Whole-namespace, not an allowlist.** Naming the four APIs that were reported would leave
/// `vi.stubGlobal` and everything Vitest adds next behaving the old way — this repository's most
/// repeated defect is a fix that lands for the reported site and not its siblings.
///
/// `vi.mock`/`vi.unmock` are excluded: [`collect_mocked_modules`] already folds them into
/// `Context::mocked_modules`, and admitting them twice would move every existing `contextKey`
/// that has one without changing what the key distinguishes.
fn is_ambient_state_call(
    call: &CallExpression,
    semantic: &Semantic,
    import_specifiers: &HashMap<SymbolId, Arc<str>>,
    exported_as: &HashMap<SymbolId, String>,
) -> bool {
    /// Already modelled as [`crate::model::Context::mocked_modules`].
    const ALREADY_MODELLED: [&str; 2] = ["mock", "unmock"];

    // Walk to the root of the member/call chain. `vi.spyOn(f, 'x').mockReturnValue(true)` is a
    // call whose callee's object is itself a CALL, not an identifier, so stopping at the first
    // member expression misses exactly the case with the widest blast radius -- measured: the
    // spy false positive survived a first version of this function that did.
    let mut cursor = &call.callee;
    let mut root_property: Option<&str> = None;
    loop {
        match cursor {
            Expression::StaticMemberExpression(member) => {
                root_property = Some(member.property.name.as_str());
                cursor = &member.object;
            }
            Expression::CallExpression(inner) => cursor = &inner.callee,
            Expression::Identifier(object) => {
                // `vi.mock(...)` is excluded by the property directly on the namespace, which is
                // the one this walk ends holding.
                if is_vitest_namespace(object, "vi", semantic, import_specifiers, exported_as) {
                    return !root_property.is_some_and(|p| ALREADY_MODELLED.contains(&p));
                }
                return is_vitest_namespace(
                    object,
                    "expect",
                    semantic,
                    import_specifiers,
                    exported_as,
                );
            }
            _ => return false,
        }
    }
}

/// Collects value-import context while excluding erased TypeScript bindings.
fn collect_import_specifiers(
    program: &Program,
    file: &std::path::Path,
) -> (HashMap<SymbolId, Arc<str>>, HashMap<SymbolId, String>) {
    let mut map = HashMap::new();
    // Local symbol to the name it was EXPORTED under, which is not always the name it is used by:
    // `import { vi as v } from 'vitest'` binds `v` to the export `vi`. Without this, a namespace
    // check comparing the LOCAL name rejects an aliased import, so `v.useFakeTimers()` never
    // reaches `module_setup` and the false positive C1 removes comes straight back. Measured.
    let mut exported_as = HashMap::new();
    // One shared allocation per distinct specifier in the file: every binding of it, across
    // however many declarations, and every test that references one of them, holds an `Arc` to
    // this same text. Keyed by the specifier as written; `file` is fixed, so the same text always
    // resolves to the same identity and a repeat declaration skips the resolution too.
    let mut identities: HashMap<&str, Arc<str>> = HashMap::new();
    for statement in &program.body {
        let Statement::ImportDeclaration(import) = statement else {
            continue;
        };
        if matches!(import.import_kind, oxc_ast::ast::ImportOrExportKind::Type) {
            continue;
        }
        let source = import.source.value.as_str();
        let identity = Arc::clone(identities.entry(source).or_insert_with(|| {
            Arc::from(super::fingerprint::specifier_identity(file, source).as_str())
        }));
        let Some(specifiers) = &import.specifiers else {
            continue;
        };
        for specifier in specifiers {
            let (local, export) = match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(s) => {
                    if matches!(s.import_kind, oxc_ast::ast::ImportOrExportKind::Type) {
                        continue;
                    }
                    (&s.local, Some(s.imported.name().to_string()))
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    (&s.local, Some("default".to_owned()))
                }
                // A namespace import binds the whole module, so there is no single export name to
                // record. `vitest.vi.useFakeTimers()` is a two-level chain the namespace check does
                // not model either way; left as it was rather than half-handled here.
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => (&s.local, None),
            };
            if let Some(symbol_id) = local.symbol_id.get() {
                map.insert(symbol_id, Arc::clone(&identity));
                if let Some(export) = export {
                    exported_as.insert(symbol_id, export);
                }
            }
        }
    }
    (map, exported_as)
}

/// Reads the `vi` module-mocking family and folds the statically safe subset of it into the
/// file-wide mocked-module set.
///
/// **Modelled**: module-top-level `vi.mock(<string literal>, ...)` and
/// `vi.unmock(<string literal>)`. Both are hoisted above the file's imports (fixture
/// `edge/vi-mock-hoisting.test.ts`), and hoisted calls keep their relative source order, so the
/// registry state every test in the file observes is "last call wins, per specifier". That
/// resolution rule was verified by running both orderings under a real Vitest 5 against a module
/// exporting `value = 99` with a factory returning `value: 1`, not derived from prose, because the
/// natural assumption ("unmock wins") is wrong:
///
/// ```text
/// vi.mock('./m', () => ({ value: 1 }))   ->  value === 99, the real module   (unmock is last)
/// vi.unmock('./m')
///
/// vi.unmock('./m')                       ->  value === 1, the factory       (mock is last)
/// vi.mock('./m', () => ({ value: 1 }))
/// ```
///
/// Folding both kinds of event in source order is what makes "mocks `./m`", "mocks then unmocks
/// `./m`", and "never mentions `./m`" three distinct contexts, which is what they are at runtime.
/// It also gives the third case for free: an `unmock` of a specifier this file never mocked
/// removes nothing and records nothing, so such a file is context-equivalent to one that does no
/// mocking at all -- again exactly what Vitest does.
///
/// **Still tainting** the whole file (the returned `bool`, which the caller folds into every
/// test's [`Context::unresolved`]), each for its own reason:
///
/// - **`vi.doMock` / `vi.doUnmock`** (F4): not hoisted. They take effect where they execute, and
///   only for dynamic imports evaluated after that point -- idiomatically inside one test body.
///   Attributing them to the tests they actually affect would mean tracking dynamic `import()`s
///   reachable from each body and ordering them against each call, a call-graph question this
///   structural extractor does not answer anywhere else either. Modelling them file-wide the way
///   `vi.mock` is modelled -- the original defect this arm exists to stop repeating -- put one
///   test's runtime-only mock into every other test's `Context`.
/// - **A specifier that is not a readable string literal** (A3), or **a `vi.mock` factory that is
///   present but is not an inline arrow/function** (A4): recording nothing would be
///   indistinguishable from a file with no mock at all.
/// - **A hoisted member called somewhere other than module top level** -- inside a `describe`, a
///   test body, an `if`. Vitest 5 refuses to run such a file at all: its `vitest:mocks` plugin
///   raises `"N call(s) ... defined outside of the module's top level scope"` at transform time,
///   which was confirmed by running exactly these three shapes. (The message explains why: the
///   call *is* hoisted, so `if (flag) vi.mock(p)` is not conditional at all and the nesting lies
///   about execution order -- Vitest would rather fail than let that read as it looks.) A file the
///   runner rejects has no runtime context for this analyzer to model, and on older Vitest it has
///   the file-wide-but-unconditional one, so the honest answer for both is "unknown". Note this is
///   a *narrowing* of what this scan used to model: it walked the entire program with a [`Visit`]
///   and treated every `vi.mock` it found anywhere as file-wide, nesting included. Events are now
///   read only from direct top-level expression statements, and a second pass taints on every
///   mocking call the first pass did not read.
///
/// Returns the modelled mock set plus that taint flag. Both are file-wide facts: a mock this scan
/// cannot vouch for makes every test in the file unsafe to compare, exactly as a genuinely mocked
/// test silently comparing equal to an unmocked one would be.
fn collect_mocked_modules(
    program: &Program,
    registry: &Registry,
    semantic: &Semantic,
    source: &str,
    file: &std::path::Path,
) -> (Vec<MockedModule>, bool) {
    /// One hoisted registration event, read in source order and folded last-wins per specifier.
    enum MockEvent {
        /// `vi.mock(spec, ...)`: registers, or re-registers, the specifier.
        Mock(MockedModule),
        /// `vi.unmock(spec)`: removes the specifier from the registry, if it is in it.
        Unmock(String),
    }

    /// The member name of a `vi.<mock|unmock|doMock|doUnmock>(...)` call, or `None` for any other
    /// call. Shared by both passes so "is this a mocking call" is decided in exactly one place --
    /// a second, subtly different predicate in the nesting pass would let a call be modelled by
    /// one pass and tainted by neither.
    fn mocking_member(
        call: &CallExpression<'_>,
        registry: &Registry,
        semantic: &Semantic<'_>,
        source: &str,
        file: &std::path::Path,
    ) -> Option<String> {
        let resolved = resolve_callee(&call.callee, registry, semantic, source, file)?;
        if resolved.root != "vi" {
            return None;
        }
        match resolved.steps.last() {
            Some(ChainStep::Member(name))
                if matches!(name.as_str(), "mock" | "unmock" | "doMock" | "doUnmock") =>
            {
                Some(name.clone())
            }
            _ => None,
        }
    }

    let mut unresolved = false;
    let mut events: Vec<MockEvent> = Vec::new();
    // Spans of the mocking calls the top-level pass has already accounted for. The nesting pass
    // below walks the same program and taints on any mocking call absent from this set, which by
    // construction is every one that is not a direct top-level expression statement.
    let mut read_at_top_level: HashSet<(u32, u32)> = HashSet::new();

    for statement in &program.body {
        let Statement::ExpressionStatement(expression_statement) = statement else {
            continue;
        };
        let Expression::CallExpression(call) = &expression_statement.expression else {
            continue;
        };
        let Some(member) = mocking_member(call, registry, semantic, source, file) else {
            continue;
        };
        read_at_top_level.insert((call.span.start, call.span.end));
        match member.as_str() {
            "mock" => match call.arguments.first() {
                Some(Argument::StringLiteral(literal)) => {
                    let factory = match call.arguments.get(1) {
                        // No second argument at all: a real automock.
                        None => MockFactory::Automock,
                        Some(argument) => match as_function_like(argument) {
                            Some(function_like) => MockFactory::Factory(
                                fingerprint(
                                    function_like,
                                    semantic,
                                    source,
                                    file,
                                    // A mock factory is never a `.each` row callback -- nothing
                                    // to substitute.
                                    None,
                                )
                                .into_alpha_source(),
                            ),
                            // A4: an imported or computed factory (identifier, call, member
                            // expression, ...) is not automock and must never be treated as one.
                            None => {
                                unresolved = true;
                                MockFactory::Unresolved
                            }
                        },
                    };
                    // The sixth instance of "unique in one scope, wrong in a wider one":
                    // `literal.value` is file-relative exactly like an `import` specifier is
                    // (`vi.mock('./dep')` in two different directories names two different
                    // modules), but unlike an `import`, nothing about this call site's identity
                    // otherwise reaches per-file scoping. `specifier_identity` is the same
                    // `resolved:`/`unresolved:` anchoring `import_origins` uses for free-import
                    // identity -- reused, not reinvented, so a bare specifier (`vitest`) stays
                    // file-independent while a relative one is anchored to `file`. The `unmock`
                    // arm below must route through it too, or it would compute a key that could
                    // never match the `mock` it is meant to cancel.
                    events.push(MockEvent::Mock(MockedModule {
                        specifier: std::sync::Arc::from(
                            super::fingerprint::specifier_identity(file, literal.value.as_str())
                                .as_str(),
                        ),
                        factory,
                    }));
                }
                // A3: the specifier itself is not statically readable (a variable, a template
                // with substitutions, a computed expression, ...). Recording nothing here would
                // be indistinguishable from a file with no mock at all, so the whole file must be
                // flagged unresolved instead.
                _ => unresolved = true,
            },
            "unmock" => match call.arguments.first() {
                Some(Argument::StringLiteral(literal)) => {
                    events.push(MockEvent::Unmock(super::fingerprint::specifier_identity(
                        file,
                        literal.value.as_str(),
                    )));
                }
                // A3 again, and for the same reason in reverse: an `unmock` whose target cannot
                // be read might cancel any of this file's mocks, so no mock in it can be trusted.
                _ => unresolved = true,
            },
            // F4: the two non-hoisted members. See this function's doc for why they are tainted
            // rather than folded into the events above.
            _ => unresolved = true,
        }
    }

    struct NestedMockScan<'r, 'a> {
        registry: &'r Registry,
        semantic: &'r Semantic<'a>,
        source: &'r str,
        file: &'r std::path::Path,
        read_at_top_level: &'r HashSet<(u32, u32)>,
        found: bool,
    }
    impl<'a> Visit<'a> for NestedMockScan<'_, 'a> {
        /// Flag recognized mock calls outside the pre-read top-level set, then continue
        /// scanning nested calls.
        fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
            if !self
                .read_at_top_level
                .contains(&(it.span.start, it.span.end))
                && mocking_member(it, self.registry, self.semantic, self.source, self.file)
                    .is_some()
            {
                self.found = true;
            }
            walk::walk_call_expression(self, it);
        }
    }
    let mut nested = NestedMockScan {
        registry,
        semantic,
        source,
        file,
        read_at_top_level: &read_at_top_level,
        found: false,
    };
    nested.visit_program(program);
    unresolved = unresolved || nested.found;

    // The fold. A `Mock` overwrites whatever that specifier last had and an `Unmock` clears it,
    // so what survives is each specifier's final state -- "last in source order wins", per the
    // verified ordering in this function's doc. Order within the map is irrelevant: the result is
    // sorted before it is returned, exactly as the previous list-of-registrations was.
    let mut state: HashMap<std::sync::Arc<str>, MockFactory> = HashMap::new();
    for event in events {
        match event {
            MockEvent::Mock(module) => {
                state.insert(module.specifier, module.factory);
            }
            MockEvent::Unmock(specifier) => {
                state.remove(specifier.as_str());
            }
        }
    }
    let mut found: Vec<MockedModule> = state
        .into_iter()
        .map(|(specifier, factory)| MockedModule { specifier, factory })
        .collect();
    found.sort();
    (found, unresolved)
}

/// One open helper-body expansion (S11), pushed on [`Walker::helper_frames`] for the duration of
/// the inlined walk and popped by [`Walker::end_helper_expansion`].
#[derive(Clone, Copy)]
struct HelperFrame<'a> {
    /// The helper's declaration, whose `body` is what gets walked.
    function: &'a Function<'a>,
    /// How many entries this frame pushed onto [`Walker::param_bindings`], to truncate back to on
    /// the way out.
    bindings: usize,
}

#[derive(Default, Clone)]
struct ScopeHooks {
    unresolved: bool,
    before_all: Vec<Hook>,
    before_each: Vec<Hook>,
    after_each: Vec<Hook>,
    after_all: Vec<Hook>,
}

/// Carries everything the statement walker accumulates as it descends into nested `describe`
/// blocks: the enclosing suite titles, the stack of hook scopes, and the tests found so far.
struct Walker<'a> {
    source: &'a str,
    semantic: &'a Semantic<'a>,
    file: &'a SourceFile,
    line_index: LineIndex<'a>,
    mocked_modules: Vec<MockedModule>,
    /// Module-scope awaited calls, file-wide like `mocked_modules`; see
    /// [`crate::model::Context::module_setup`].
    module_setup: Vec<String>,
    /// Module-scope ambient-state calls, file-wide like `mocked_modules`; see
    /// Set when some hoisted mocking call in this file could not be fully read (A3/A4), when the
    /// file contains a non-hoisted `vi.doMock`/`vi.doUnmock` at all, or when any mocking call
    /// appears somewhere other than module top level (F4 -- see `collect_mocked_modules`'s doc for
    /// all three). Applies file-wide, exactly like `mocked_modules` itself: folded into every
    /// test's `Context::unresolved` in [`Walker::push_test_case`].
    mocks_unresolved: bool,
    import_specifiers: HashMap<SymbolId, Arc<str>>,
    exported_as: HashMap<SymbolId, String>,
    suite_path: Vec<Title>,
    hook_scopes: Vec<ScopeHooks>,
    tests: Vec<TestCase>,
    helper_sources: std::collections::BTreeMap<std::path::PathBuf, String>,
    allow_imported: bool,
    /// Diagnostics accumulated while walking declarations, distinct from the parse/semantic
    /// diagnostics `run()` already collects. Currently used for statement forms the walker
    /// deliberately declines to recurse into (see [`Walker::walk_statement`]'s `WithStatement`
    /// arm): silently skipping a construct that can hide a test/describe declaration is never an
    /// acceptable outcome, so anything not recursed into must say so here.
    diagnostics: Vec<String>,
    /// Number of enclosing `describe` scopes currently pushed whose own title is unresolved
    /// ([`Title::Dynamic`] or, per F5, [`Title::Parameterized`] -- see [`title_is_unresolved`]).
    /// Such a title's runtime identity is not known, so two different runtime suite instances can
    /// normalize to the same title text -- every test declared inside one must therefore be
    /// treated as unresolved too, not just the suite's own record, or a nested test could still
    /// be compared and falsely matched against its counterpart in the other instance. A counter
    /// (rather than a single bool) so nested unresolved suites compose correctly when a scope is
    /// popped.
    unresolved_suite_depth: usize,
    /// F3: the flattened, outer-to-inner list of every *currently open* enclosing suite's own
    /// behavior-significant modifiers ([`behavior_modifiers`] -- `Each`/`SkipIf`/`RunIf`, the
    /// subset `analysis.rs`'s `ModifierKey` keys `duplicate-test` on).
    ///
    /// Before this field existed, entering a suite pushed its title and hook scope but dropped
    /// its own `.each`/`.skipIf`/`.runIf` on the floor: a nested test's own `modifiers` only ever
    /// held *its own* chain, never anything from an enclosing `describe`. Two nested tests with
    /// identical bodies, declared inside two suites that differ only in an enclosing
    /// `.each`/`.skipIf`/`.runIf` (and whose suite titles do not happen to differ either -- e.g.
    /// two `describe.skipIf(cond)('feature', ...)` blocks with different, unfoldable `cond`s),
    /// would then compare equal at Error severity: `ModifierKey` treats those chain steps as
    /// behavior-significant precisely because they change whether/how a test runs, but a nested
    /// test carried none of its enclosing suite's copy.
    ///
    /// [`Walker::emit_describe`] pushes this suite's own `behavior_modifiers` onto this stack
    /// before walking its body and truncates back afterward (mirroring `suite_path`/
    /// `hook_scopes`'s own push/pop discipline), and prefixes it onto every declaration's own
    /// `modifiers` -- both the suite's own [`TestCase`] record and every test nested inside it --
    /// so `ModifierKey` sees the full ancestor chain, not just the innermost declaration's own.
    suite_modifiers: Vec<Modifier>,
    /// Loop variable bindings currently in scope, from innermost to outermost: the symbol a
    /// `for...of` loop over a literal array bound its element to, paired with that element's
    /// const-folded display text for the iteration currently being walked. See
    /// [`Walker::walk_for_of_statement`] and [`Walker::fold_expr`].
    loop_bindings: Vec<(SymbolId, String)>,
    /// S11: parameter bindings of every helper frame currently open, innermost last: the symbol a
    /// helper's formal parameter declares, paired with the const-folded text of the argument
    /// *this* call site passed for it. Consulted by [`Walker::fold_expr`] exactly like
    /// [`Walker::loop_bindings`] is, and for the same reason -- a value the extractor can prove
    /// from source resolves, everything else falls through to the ordinary unresolved path.
    ///
    /// A parameter is bound only when its argument const-folds; an unfoldable or missing argument
    /// leaves the parameter unbound, so a title that reads it degrades to [`Title::Dynamic`]
    /// through the existing [`Walker::extract_title`] fallthrough rather than being rendered from
    /// a value nobody proved. Keyed by [`SymbolId`], so a shadowing binding inside the helper's
    /// own body resolves to itself, never to the parameter.
    param_bindings: Vec<(SymbolId, String)>,
    /// Local AST/parameter frames. The shared expansion state guards cycles by canonical
    /// source path and callable span, so symbols from different parser frames cannot collide.
    helper_frames: Vec<HelperFrame<'a>>,
    /// S11: helper body expansions this file has left before [`MAX_HELPER_EXPANSIONS`] is spent.
    expansion: Rc<RefCell<imported::ExpansionState>>,
    runner_imports: HashMap<SymbolId, imported::RunnerImport>,
    globals: Globals,
    registration_file: Option<std::path::PathBuf>,
    /// Whether a helper expansion refusal has already been reported for this file. One diagnostic
    /// is enough to mark the file `incomplete`; a hostile file that trips the budget thousands of
    /// times should not emit thousands of near-identical strings.
    helper_refusal_reported: bool,
    /// Test-only count of existing declarations visited while tainting refused scopes.
    #[cfg(test)]
    helper_refusal_taint_visits: usize,
    /// F2 declarations seen so far: a `test.each([])`/`describe.each([])`/`.for([])` whose table
    /// was a literal array statically known to have zero rows. Vitest runs such a table zero
    /// times, so it contributes no [`TestCase`] -- but that is a *proven* zero, not a failure to
    /// recognize the file as Vitest. `run()` uses this count (compared against `self.tests`
    /// staying empty and no diagnostic being raised) to tell that apart from the ordinary
    /// zero-test outcome, so `main.rs`'s "extracted no tests" guard can name the real cause
    /// instead of always blaming `test.globals`.
    proven_empty_tables: usize,
}

impl<'a> Walker<'a> {
    /// Initializes a walker from this file's own semantic frame, imports and module effects.
    fn new(
        source: &'a str,
        file: &'a SourceFile,
        program: &'a Program<'a>,
        semantic: &'a Semantic<'a>,
        globals: Globals,
    ) -> Self {
        let (import_specifiers, exported_as) = collect_import_specifiers(program, &file.path);
        let registry = Registry::new(program, globals);
        let (mocked_modules, mocks_unresolved) =
            collect_mocked_modules(program, &registry, semantic, source, &file.path);
        let module_setup =
            collect_module_setup(program, source, semantic, &import_specifiers, &exported_as);
        Self {
            source,
            semantic,
            file,
            line_index: LineIndex::new(source),
            mocked_modules,
            mocks_unresolved,
            module_setup,
            import_specifiers,
            exported_as,
            suite_path: Vec::new(),
            hook_scopes: vec![ScopeHooks::default()],
            tests: Vec::new(),
            helper_sources: Default::default(),
            allow_imported: true,
            diagnostics: Vec::new(),
            unresolved_suite_depth: 0,
            suite_modifiers: Vec::new(),
            loop_bindings: Vec::new(),
            param_bindings: Vec::new(),
            helper_frames: Vec::new(),
            expansion: Rc::new(RefCell::new(imported::ExpansionState::default())),
            runner_imports: imported::collect_imports(program, &file.path),
            globals,
            registration_file: None,
            helper_refusal_reported: false,
            #[cfg(test)]
            helper_refusal_taint_visits: 0,
            proven_empty_tables: 0,
        }
    }

    /// Registers every hook (bare or scoped) and `test.extend`/`import.meta.vitest` binding
    /// declared directly in `statements`, without recursing into nested suites.
    ///
    /// This must run to completion for a scope *before* any test in that scope is emitted:
    /// Vitest hooks apply to every test in their enclosing scope regardless of whether the hook
    /// was registered before or after that test textually (`fixtures/hooks/hooks-nested-order.
    /// test.ts` registers `afterEach` after the nested `describe` it must still apply to). A
    /// single top-to-bottom pass would only see hooks that happened to come first.
    fn prepare_statements(&mut self, statements: &[Statement<'a>], registry: &mut Registry) {
        for statement in statements {
            self.prepare_statement(statement, registry);
        }
    }

    /// Pre-register bindings and scope effects from one statement and supported nested
    /// statement containers.
    fn prepare_statement(&mut self, statement: &Statement<'a>, registry: &mut Registry) {
        match statement {
            Statement::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    self.register_variable_declarator(declarator, registry);
                }
            }
            Statement::ExpressionStatement(expression_statement) => {
                if let Expression::CallExpression(call) = &expression_statement.expression {
                    self.prepare_call(call, registry);
                }
            }
            // A bare block does not introduce a new hook scope; its hooks belong to the
            // enclosing one. Same for every other construct below: a hook declared inside an
            // `if`/`for`/`try`/`switch` at this level still belongs to the current suite's own
            // hook scope, not a new one -- only a `describe` body pushes a fresh `ScopeHooks`
            // (see `emit_describe`).
            Statement::BlockStatement(block) => self.prepare_statements(&block.body, registry),
            // A generic `if` recurses into both branches unconditionally, without evaluating the
            // condition -- this also covers hazard 9's `if (import.meta.vitest) { ... }` idiom as
            // a special case of the same rule, rather than a separate one.
            Statement::IfStatement(if_statement) => {
                self.prepare_body_like_statement(&if_statement.consequent, registry);
                if let Some(alternate) = &if_statement.alternate {
                    self.prepare_body_like_statement(alternate, registry);
                }
            }
            Statement::ForStatement(for_statement) => {
                self.prepare_body_like_statement(&for_statement.body, registry)
            }
            Statement::ForInStatement(for_in) => {
                self.prepare_body_like_statement(&for_in.body, registry)
            }
            // Unlike `walk_statement`, hook registration does not unroll a literal `for...of`
            // per element -- a `beforeEach` registered once per iteration would run its body
            // once per iteration too, which is not what this scope's hook list should record.
            Statement::ForOfStatement(for_of) => {
                self.prepare_body_like_statement(&for_of.body, registry)
            }
            Statement::WhileStatement(while_statement) => {
                self.prepare_body_like_statement(&while_statement.body, registry)
            }
            Statement::DoWhileStatement(do_while) => {
                self.prepare_body_like_statement(&do_while.body, registry)
            }
            Statement::TryStatement(try_statement) => {
                self.prepare_statements(&try_statement.block.body, registry);
                if let Some(handler) = &try_statement.handler {
                    self.prepare_statements(&handler.body.body, registry);
                }
                if let Some(finalizer) = &try_statement.finalizer {
                    self.prepare_statements(&finalizer.body, registry);
                }
            }
            Statement::SwitchStatement(switch_statement) => {
                for case in &switch_statement.cases {
                    self.prepare_statements(&case.consequent, registry);
                }
            }
            Statement::LabeledStatement(labeled) => {
                self.prepare_body_like_statement(&labeled.body, registry)
            }
            // `with` is deliberately unsupported (see the matching arm in `walk_statement`); the
            // diagnostic it pushes there covers hooks hidden inside one too, so nothing more is
            // needed here.
            _ => {}
        }
    }

    /// Prepare all statements of a block body, otherwise prepare the single body statement.
    fn prepare_body_like_statement(&mut self, statement: &Statement<'a>, registry: &mut Registry) {
        match statement {
            Statement::BlockStatement(block) => self.prepare_statements(&block.body, registry),
            other => self.prepare_statement(other, registry),
        }
    }

    /// Whether `ident` binds to nothing in this file -- no import, no declaration, no parameter.
    ///
    /// `oxc_semantic` answers this, not a name check: a reference with no `symbol_id` resolved to
    /// no binding in any enclosing scope, which is exactly "ambient" for the purposes of
    /// [`Self::prepare_call`]'s R10 arm.
    fn is_free_reference(&self, ident: &IdentifierReference) -> bool {
        ident
            .reference_id
            .get()
            .and_then(|reference| self.semantic.scoping().get_reference(reference).symbol_id())
            .is_none()
    }

    /// Resolves a call's callee against this extractor's own file -- the exact prologue both
    /// halves of the prepare/walk split (`prepare_call`, `classify_call`) and the
    /// `test.extend` binding pass need, so the three cannot drift on which context resolves.
    fn resolve_call_callee(
        &self,
        callee: &Expression<'a>,
        registry: &Registry,
    ) -> Option<ResolvedCallee> {
        resolve_callee(
            callee,
            registry,
            self.semantic,
            self.source,
            &self.file.path,
        )
    }

    /// Pre-register imported, scoped and free-identifier ambient hooks for this scope.
    /// If registry resolution fails and no ambient hook is recorded, pre-scan a possible local or
    /// first-party imported helper body. Fixture bindings are prepared separately.
    fn prepare_call(&mut self, call: &CallExpression<'a>, registry: &mut Registry) {
        let Some(resolved) = self.resolve_call_callee(&call.callee, registry) else {
            // S8/R10: an AMBIENT hook -- `beforeAll(...)` with no import and no declaration
            // anywhere in the file. `resolve_callee` cannot claim it, because under a
            // `Globals::Disabled` run there is nothing to resolve it to.
            //
            // That silence is the observed text-splitting defect. The file imports
            // `{ describe, it, expect, vi }` and NOT `beforeAll` -- legal, because the project
            // sets `globals: true` in a `test.projects` config the scan could not read. The
            // unregistered hook never reached `ContextKey`, so a describe whose `beforeAll` runs
            // `vi.stubGlobal('Intl', ...)` compared EQUAL to its sibling that does not, and
            // `duplicate-test` fired at ERROR severity on two tests whose whole purpose is to run
            // under opposite global state.
            //
            // Recording it is right under either reading of the file, which is what makes this
            // safe rather than a guess: if the project really is `globals: true`, the hook runs
            // and belongs in the context; if it really is `globals: false`, the file throws at
            // runtime and the two describes are not equivalent either. Both readings say "do not
            // call these the same test", and recording a hook only ever makes contexts differ --
            // it can suppress a finding, never invent one.
            //
            // A FREE reference is the whole discriminator. A file declaring its own
            // `function beforeAll()` resolves to that binding and is not this shape; only an
            // identifier that binds to nothing qualifies.
            if let Expression::Identifier(ident) = &call.callee {
                if let Some(kind) = hook_kind(ident.name.as_str()) {
                    if self.is_free_reference(ident) {
                        self.record_hook(call, kind);
                        return;
                    }
                }
            }
            // Not a Vitest callee. S11: it may still be a local helper this file calls here, and
            // a hook registered inside that helper's body belongs to *this* scope, exactly like
            // one written here directly -- so the helper's body is pre-scanned on the same terms.
            self.prepare_helper_call(call, registry);
            return;
        };
        let kind = if resolved.steps.is_empty() {
            // A bare hook import: `beforeEach(...)`.
            hook_kind(resolved.root)
        } else if let [ChainStep::Member(name)] = resolved.steps.as_slice() {
            // A scoped hook hung off a `test.extend` result: `myTest.beforeEach(...)`.
            hook_kind(name)
        } else {
            None
        };
        if let Some(kind) = kind {
            self.record_hook(call, kind);
        }
    }

    /// The real declaration walk: `describe`/`test`/`bench` emission, recursing into nested
    /// suites. Hooks and bindings were already registered for this whole scope by
    /// [`Self::prepare_statements`], so this pass does not touch them again.
    fn walk_statements(&mut self, statements: &[Statement<'a>], registry: &mut Registry) {
        self.prepare_statements(statements, registry);
        for statement in statements {
            self.walk_statement(statement, registry);
        }
    }

    /// The declaration walk's core dispatch. Every statement form that can hold a nested
    /// declaration -- directly, or through ordinary control flow -- must either recurse into it
    /// here or fall through to the final arm, which flags the file `incomplete` with a diagnostic
    /// naming the construct. A silently-dropped declaration is the worst failure mode this tool
    /// has (a clean run that quietly analyzed less than it claimed to), so the wildcard arm is
    /// never allowed to mean "ignore silently" here the way it safely can for a leaf statement
    /// kind that structurally cannot contain a `Statement` at all (`return`, `break`, `throw`,
    /// `debugger`, ...) -- see the final match arm's own comment for exactly where the line is
    /// drawn.
    fn walk_statement(&mut self, statement: &Statement<'a>, registry: &mut Registry) {
        match statement {
            Statement::ExpressionStatement(expression_statement) => {
                if let Expression::CallExpression(call) = &expression_statement.expression {
                    self.classify_call(call, registry);
                }
            }
            // Already handled by `prepare_statements`.
            Statement::VariableDeclaration(_) => {}
            Statement::BlockStatement(block) => self.walk_statement_list(&block.body, registry),
            // A generic `if` recurses into both branches unconditionally: the condition's runtime
            // value is never evaluated, so a declaration reachable through either branch must be
            // visible regardless of which one a reader expects to run. This also covers hazard
            // 9's `if (import.meta.vitest) { ... }` idiom as a special case of the same rule,
            // rather than a separate one -- its body is walked in the same suite/hook context as
            // the surrounding statements, since in-source tests are module scope, not nested in
            // any suite.
            Statement::IfStatement(if_statement) => {
                self.walk_body_like_statement(&if_statement.consequent, registry);
                if let Some(alternate) = &if_statement.alternate {
                    self.walk_body_like_statement(alternate, registry);
                }
            }
            Statement::ForStatement(for_statement) => {
                self.walk_body_like_statement(&for_statement.body, registry)
            }
            Statement::ForInStatement(for_in) => {
                self.walk_body_like_statement(&for_in.body, registry)
            }
            Statement::ForOfStatement(for_of) => self.walk_for_of_statement(for_of, registry),
            Statement::WhileStatement(while_statement) => {
                self.walk_body_like_statement(&while_statement.body, registry)
            }
            Statement::DoWhileStatement(do_while) => {
                self.walk_body_like_statement(&do_while.body, registry)
            }
            Statement::TryStatement(try_statement) => {
                self.walk_statement_list(&try_statement.block.body, registry);
                if let Some(handler) = &try_statement.handler {
                    self.walk_statement_list(&handler.body.body, registry);
                }
                if let Some(finalizer) = &try_statement.finalizer {
                    self.walk_statement_list(&finalizer.body, registry);
                }
            }
            Statement::SwitchStatement(switch_statement) => {
                for case in &switch_statement.cases {
                    self.walk_statement_list(&case.consequent, registry);
                }
            }
            Statement::LabeledStatement(labeled) => {
                self.walk_body_like_statement(&labeled.body, registry)
            }
            // Deliberately unsupported: `with` is disallowed in strict-mode/ESM source (which
            // every real Vitest file is) and its dynamic, unresolvable scoping would make any
            // identifier resolution performed inside it -- including this frontend's own
            // binding-identity resolution in `registry.rs` -- unsound to trust even if the walker
            // did recurse. It is also vanishingly unlikely to appear in real test code. Rather
            // than repeat the exact defect this fix closes by staying silent, any declaration
            // that might be hiding inside one is reported as incomplete instead.
            Statement::WithStatement(_) => {
                self.diagnostics.push(
                    "a `with` statement body was not analyzed; any test/describe declarations \
                     inside it were not counted"
                        .to_owned(),
                );
            }
            // Every remaining statement kind structurally cannot contain a nested `Statement`
            // that could hide a declaration (`return`, `break`, `continue`, `throw`, `debugger`,
            // an empty statement, ...), or is a `Declaration`/`ModuleDeclaration` form
            // (`function`/`class`/`import`/`export`, ...) whose body only runs when separately
            // invoked or is syntactically restricted to module top level. A `function`
            // declaration is reached here, and still contributes nothing *on its own*: since S11
            // its body is walked from the **call** instead (see `walk_helper_call`), so a helper
            // that is defined but never called declares nothing, which is exactly what Vitest
            // does with it. The wider call-graph question -- which function a reassignable
            // binding or a passed callback holds at call time -- is still not answered here, the
            // same boundary the "opaque body" hazard (C1) draws for a test body that references a
            // named helper instead of an inline callback. Nothing here needs a diagnostic because
            // nothing here can silently drop a *reachable* declaration the way an unhandled
            // control-flow construct would.
            _ => {}
        }
    }

    /// Walk every statement in source order without preparing the enclosing scope again;
    /// its bindings and hook effects have already been collected.
    fn walk_statement_list(&mut self, statements: &[Statement<'a>], registry: &mut Registry) {
        for statement in statements {
            self.walk_statement(statement, registry);
        }
    }

    /// Walk a block's statement list or a single non-block body with the same registry.
    fn walk_body_like_statement(&mut self, statement: &Statement<'a>, registry: &mut Registry) {
        match statement {
            Statement::BlockStatement(block) => self.walk_statement_list(&block.body, registry),
            other => self.walk_statement(other, registry),
        }
    }

    /// A `for...of` loop over a literal array is unrolled once per element, mirroring `.each` row
    /// semantics (`registry::literal_table_rows`): a `test`/`describe` call declared inside such a
    /// loop really does register one Vitest declaration per element at runtime, and the loop
    /// variable's value is exactly as knowable here as an `.each` row's own cells are -- see
    /// [`literal_for_of_elements`].
    ///
    /// A loop this extractor cannot enumerate (an identifier, a call, a spread, a destructuring
    /// pattern, ...) is walked exactly once instead. Any title that interpolates the loop variable
    /// still degrades to `Title::Dynamic` through the ordinary const-fold path in that case, and
    /// [`Walker::push_test_case`] already marks that unresolved -- so this stays consistent with
    /// how every other dynamic title is already handled, per the item this fixes.
    fn walk_for_of_statement(&mut self, for_of: &ForOfStatement<'a>, registry: &mut Registry) {
        match literal_for_of_elements(for_of) {
            Some((symbol_id, values)) => {
                for value in values {
                    self.loop_bindings.push((symbol_id, value));
                    self.walk_body_like_statement(&for_of.body, registry);
                    self.loop_bindings.pop();
                }
            }
            None => self.walk_body_like_statement(&for_of.body, registry),
        }
    }

    /// Folds `expr` to a display string it can prove statically, consulting the current
    /// `for...of` loop bindings first (an identifier bound by an enclosing unrolled loop resolves
    /// to *this* iteration's element, by symbol identity rather than name -- the same
    /// binding-identity discipline `registry.rs` uses everywhere else) before falling back to
    /// ordinary constant folding.
    fn fold_expr(&self, expr: &Expression) -> Option<String> {
        if let Expression::Identifier(ident) = expr {
            if let Some(reference_id) = ident.reference_id.get() {
                if let Some(symbol_id) = self
                    .semantic
                    .scoping()
                    .get_reference(reference_id)
                    .symbol_id()
                {
                    // Innermost binding first, and both stacks are keyed by symbol identity, so
                    // which list a binding came from cannot change the answer: a symbol appears
                    // on at most one of them. S11's `param_bindings` is consulted on exactly the
                    // same terms as the loop bindings above it.
                    for (bound_symbol, value) in self
                        .loop_bindings
                        .iter()
                        .rev()
                        .chain(self.param_bindings.iter().rev())
                    {
                        if *bound_symbol == symbol_id {
                            return Some(value.clone());
                        }
                    }
                }
            }
        }
        const_fold(expr, None)
    }

    /// Binds a local symbol introduced outside a plain `import` statement: either an
    /// `import.meta.vitest` destructure, or a `test.extend(...)` result.
    fn register_variable_declarator(
        &mut self,
        declarator: &VariableDeclarator<'a>,
        registry: &mut Registry,
    ) {
        let Some(init) = &declarator.init else { return };

        if is_import_meta_vitest_expr(init) {
            if let BindingPattern::ObjectPattern(pattern) = &declarator.id {
                for property in &pattern.properties {
                    let Some(name) = property_key_name(&property.key) else {
                        continue;
                    };
                    let Some(canonical) = canonicalize_export_name(&name) else {
                        continue;
                    };
                    if let BindingPattern::BindingIdentifier(binding) = &property.value {
                        if let Some(symbol_id) = binding.symbol_id.get() {
                            registry.bind_local(symbol_id, canonical, Vec::new(), false);
                        }
                    }
                }
            }
            return;
        }

        // `const myTest = baseTest.extend({...})` (or a plain re-export like `const x = test`):
        // either way, `myTest`/`x` should resolve exactly like `resolved` did, fixtures included.
        let Some(resolved) = self.resolve_call_callee(init, registry) else {
            return;
        };
        if resolved.root != "test" {
            return;
        }
        if let BindingPattern::BindingIdentifier(binding) = &declarator.id {
            if let Some(symbol_id) = binding.symbol_id.get() {
                registry.bind_local(
                    symbol_id,
                    "test",
                    resolved.fixtures,
                    resolved.fixtures_unresolved,
                );
            }
        }
    }

    /// Dispatch recognized test/suite calls for emission; attempt helper expansion for
    /// unresolved callees.
    fn classify_call(&mut self, call: &CallExpression<'a>, registry: &mut Registry) {
        let Some(resolved) = self.resolve_call_callee(&call.callee, registry) else {
            // S11: not Vitest, but possibly a local helper that declares tests. See
            // `walk_helper_call`.
            self.walk_helper_call(call, registry);
            return;
        };

        // Single bare-member shapes that are not declarations at all: scoped hooks hung off a
        // `test.extend` result (already recorded by `prepare_call`), and fixture-configuration
        // calls whose outer invocation carries no title/body (`myTest.extend(...)`,
        // `myTest.override(...)`).
        if let [ChainStep::Member(name)] = resolved.steps.as_slice() {
            if hook_kind(name).is_some()
                || matches!(name.as_str(), "extend" | "override" | "scoped")
            {
                return;
            }
        }

        match resolved.root {
            "test" => self.emit_test(call, DeclarationKind::Test, resolved),
            "bench" => self.emit_test(call, DeclarationKind::Benchmark, resolved),
            "describe" => self.emit_describe(call, resolved, registry),
            // Bare hooks ("beforeAll" et al.) were already recorded by `prepare_call`; "vi" is
            // handled file-wide by `collect_mocked_modules`; anything else has no
            // declaration-tree effect at this level.
            _ => {}
        }
    }

    /// Fingerprint a readable hook callback and record its kind, location and idempotence in
    /// the current scope.
    fn record_hook(&mut self, call: &CallExpression<'a>, kind: HookKind) {
        let Some(function_like) = call.arguments.first().and_then(as_function_like) else {
            return;
        };
        // Computed before `fingerprint` consumes the callable.
        let idempotent = hook_body_is_idempotent(
            &function_like,
            self.semantic,
            &self.import_specifiers,
            &self.exported_as,
        );
        // A hook body is never a `.each` row callback -- nothing to substitute.
        let body = fingerprint(
            function_like,
            self.semantic,
            self.source,
            &self.file.path,
            None,
        );
        let hook = Hook {
            kind,
            location: self.location(call.span),
            idempotent,
            body: body.digest(),
            // Kept, not dropped: `analysis`'s `ContextKey` publishes this text's JSON inside
            // `contextKey`. See `Hook::alpha_source`.
            alpha_source: body.into_alpha_source(),
        };
        let scope = self
            .hook_scopes
            .last_mut()
            .expect("root hook scope always present");
        match kind {
            HookKind::BeforeAll => scope.before_all.push(hook),
            HookKind::BeforeEach => scope.before_each.push(hook),
            HookKind::AfterEach => scope.after_each.push(hook),
            HookKind::AfterAll => scope.after_all.push(hook),
        }
    }

    /// Emit declaration records with row-specific titles, modifiers, context and body
    /// fingerprints.
    fn emit_test(
        &mut self,
        call: &CallExpression<'a>,
        kind: DeclarationKind,
        resolved: ResolvedCallee,
    ) {
        let (own_modifiers, each, condition_unresolved) = split_modifiers_and_each(resolved.steps);
        // C1: an `.each`/`.for` table this extractor could not enumerate (hazard 5) is a fact the
        // context cannot vouch for -- distinct from a literal-but-empty table, which is fully
        // known and does not make the context unresolved.
        let each_unresolved = each.as_ref().is_some_and(|info| info.rows.is_err());
        // A missing/non-expression `.skipIf`/`.runIf` argument has no identity and is exactly as
        // untrustworthy for comparison as an unresolved fixture or mock. Composite expressions
        // are comparable through `registry::condition_fingerprint` without evaluating them.
        let fixtures_unresolved = resolved.fixtures_unresolved || condition_unresolved;
        // F3: this test's own chain, prefixed with every enclosing suite's own
        // `Each`/`SkipIf`/`RunIf` -- see `Walker::suite_modifiers`'s doc. Combined with
        // `own_modifiers` lazily (per branch, via `combined_modifiers` below) rather than eagerly
        // here: the `.each`-row branch needs to build a *row-specific* combination (see its own
        // comment), so `own_modifiers` must survive independently that long.
        let suite_prefix = self.suite_modifiers.clone();
        let combined_modifiers =
            |own: Vec<Modifier>| -> Vec<Modifier> { [suite_prefix.clone(), own].concat() };
        let args: &[Argument<'a>] = &call.arguments;

        // `test(function myTestName() { ... })`: the title comes from the function's own name,
        // and the function itself is the body -- it is never invoked to produce the title.
        if let Some(Argument::FunctionExpression(function)) = args.first() {
            if let Some(id) = &function.id {
                let title = Title::Static(id.name.as_str().to_owned());
                self.push_test_case(
                    kind,
                    title,
                    combined_modifiers(own_modifiers),
                    args.first(),
                    call,
                    resolved.fixtures,
                    fixtures_unresolved || each_unresolved,
                    // A named-function body is never a `.each` row callback -- see the loop
                    // below, the only site that ever has a `Row` to substitute from.
                    None,
                    true,
                );
                return;
            }
        }

        let (title_arg, options, body_arg) = split_declaration_args(args);
        let mut own_modifiers = own_modifiers;
        if let Some(options) = options {
            union_option_modifiers(&mut own_modifiers, options);
        }

        if let Some(each) = each {
            let pattern = raw_title_pattern(title_arg, self.source);
            match &each.rows {
                Ok(rows) if !rows.is_empty() => {
                    let row_classes = table_row_classes(rows);
                    for (index, (row, row_class)) in rows.iter().zip(row_classes).enumerate() {
                        let title = render_each_title(&pattern, row, index);
                        // Substitute this row's own statically-known cell values into its
                        // body fingerprint -- `None` (nothing to substitute) whenever the row
                        // itself is not fully known; see `row_substitution`'s own doc.
                        let substitution = row_substitution(row);
                        // Once a row is substituted, this declaration's own `Each` modifier is
                        // dropped from *that row's* modifiers (an enclosing suite's own
                        // `describe.each`, in `suite_prefix`, is untouched -- substitution does
                        // not cover describe-level rows, see `fingerprint::RowSubstitution`'s
                        // doc, and still needs its own `Each` for differentiation there).
                        //
                        // `Each` exists on `ModifierKey` (`analysis.rs`) to stop two *different*
                        // `.each` tables driving the same-shaped callback over different data from
                        // merging on body alone (the EMAILS/PHONES case that module's doc
                        // describes) -- but a substituted row's body no longer needs that help:
                        // different row data now serializes to a genuinely different `alpha`
                        // directly (the substituted literal), so two tables that would have
                        // collided on `Each` alone now also differ on body, and two rows that
                        // agree on both really do compute the same test. Leaving `Each` attached
                        // here would instead permanently block the one link substitution exists
                        // to make:
                        // a hand-written duplicate never carries an `Each` modifier, so keeping it
                        // on the row's side would make `ModifierKey` disagree with every
                        // hand-written twin regardless of how well the bodies match.
                        let row_own_modifiers: Vec<Modifier> = if substitution.is_some() {
                            own_modifiers
                                .iter()
                                .filter(|modifier| !matches!(modifier, Modifier::Each(_)))
                                .cloned()
                                .collect()
                        } else {
                            own_modifiers.clone()
                        };
                        self.push_test_case(
                            kind,
                            title,
                            combined_modifiers(row_own_modifiers),
                            body_arg,
                            call,
                            resolved.fixtures.clone(),
                            // Defect 4: the table as a whole may be a literal array (`rows` is
                            // `Some`), but this specific row can still carry a cell this
                            // extractor could not read (an identifier, a call, ...) -- that row's
                            // own `unresolved` flag, not just the table-level one, must gate it.
                            fixtures_unresolved || row.unresolved,
                            substitution,
                            row_cells_fully_observable(body_arg, row),
                        );
                        // `push_test_case` pushed exactly this row's case last. Resolved row
                        // values, rather than shared declaration location, are what
                        // `duplicate-each-row` compares.
                        if let Some(case) = self.tests.last_mut() {
                            case.table_row_class = row_class;
                        }
                    }
                }
                // F2: a literal-but-*known-empty* table (`test.each([])`) registers zero tests
                // at runtime -- Vitest does not run a representative case for it, so extracting
                // one here would be a synthetic test with no runtime counterpart. Left
                // unhandled, that synthetic case could collide with a real test at Error
                // severity, inflated `testsAnalyzed`, and bypassed the zero-extraction guard.
                // `each_unresolved` is `false` here (it is only `true` when `rows` is `Err`), so
                // this arm never marks anything unresolved -- it emits nothing at all.
                Ok(_) => self.proven_empty_tables += 1,
                // Genuinely unresolved (hazard 5: an identifier, a call, a spread, ... this
                // extractor could not enumerate): the table's row count is not known, so unlike
                // the known-empty case above this cannot be proven to register zero tests either
                // -- exactly one representative case, marked unresolved via `each_unresolved`
                // (`true` here since `rows` is `Err`), never silently zero.
                Err(_) => {
                    let title = Title::Parameterized(pattern.text);
                    self.push_test_case(
                        kind,
                        title,
                        combined_modifiers(own_modifiers),
                        body_arg,
                        call,
                        resolved.fixtures,
                        fixtures_unresolved || each_unresolved,
                        // The table itself is not statically enumerable -- there is no `Row` to
                        // substitute from.
                        None,
                        true,
                    );
                }
            }
        } else {
            let title = self.extract_title(title_arg);
            self.push_test_case(
                kind,
                title,
                combined_modifiers(own_modifiers),
                body_arg,
                call,
                resolved.fixtures,
                fixtures_unresolved,
                // Not a `.each` declaration at all.
                None,
                true,
            );
        }
    }

    /// Emit suite records and walk readable suite bodies under their derived scope and row
    /// context.
    fn emit_describe(
        &mut self,
        call: &CallExpression<'a>,
        resolved: ResolvedCallee,
        registry: &mut Registry,
    ) {
        let (own_modifiers, each, condition_unresolved) = split_modifiers_and_each(resolved.steps);
        // C1: see the identical comment in `emit_test`.
        let each_unresolved = each.as_ref().is_some_and(|info| info.rows.is_err());
        // Defect 5: see the identical comment in `emit_test`.
        let fixtures_unresolved = resolved.fixtures_unresolved || condition_unresolved;
        let args: &[Argument<'a>] = &call.arguments;
        let (title_arg, _options, body_arg) = split_declaration_args(args);
        let body = describe_body(body_arg);

        // F3: this suite's own `Each`/`SkipIf`/`RunIf`, to be pushed onto `self.suite_modifiers`
        // for everything nested inside it -- see `Walker::suite_modifiers`'s doc. Computed from
        // `own_modifiers` *before* it is merged with the already-open ancestor chain below, so
        // nesting depth N does not push ancestor entries N times over.
        let own_behavior_modifiers = behavior_modifiers(&own_modifiers);
        // This suite's own record, prefixed with every *already-open* enclosing suite's own
        // behavior-significant modifiers.
        let mut modifiers = self.suite_modifiers.clone();
        modifiers.extend(own_modifiers);

        // Paired with each title, whether *this specific* instance's identity is unresolved --
        // defect 4's per-row cell, not just the table-level `each_unresolved` fact that already
        // applies uniformly below.
        let titles: Vec<(Title, bool, Option<u32>)> = match &each {
            Some(each) => {
                let pattern = raw_title_pattern(title_arg, self.source);
                match &each.rows {
                    Ok(rows) if !rows.is_empty() => rows
                        .iter()
                        .zip(table_row_classes(rows))
                        .enumerate()
                        .map(|(index, (row, row_class))| {
                            (
                                render_each_title(&pattern, row, index),
                                row.unresolved,
                                row_class,
                            )
                        })
                        .collect(),
                    // F2: a literal-but-*known-empty* table (`describe.each([])`) instantiates
                    // zero suites at runtime -- no `Suite` record, and nothing nested inside it
                    // is ever reached, exactly mirroring `emit_test`'s identical case.
                    Ok(_) => {
                        self.proven_empty_tables += 1;
                        Vec::new()
                    }
                    // Genuinely unresolved (hazard 5): the instance count is not known, so
                    // unlike the known-empty case above this cannot be proven to register zero
                    // suites either -- exactly one representative instance, marked unresolved
                    // via `each_unresolved` below, never silently zero.
                    Err(_) => vec![(Title::Parameterized(pattern.text), false, None)],
                }
            }
            None => vec![(self.extract_title(title_arg), false, None)],
        };

        for (title, row_unresolved, table_row_class) in titles {
            // Defect 3 / F5: a dynamic suite title's runtime identity is not known, and neither
            // is a `Parameterized` one -- both retain a normalized shape rather than a proven
            // runtime value, so two different runtime suite instances can normalize to the same
            // title text. That must taint every test declared inside this instance, not just
            // this `Suite` record's own `context` -- otherwise a nested test's `full_title`
            // still looks fully resolved and can falsely match its counterpart in the *other*
            // instance.
            let title_is_unresolved = title_is_unresolved(&title);
            // The `describe` declaration itself is a `Suite`-kind case, distinct from the tests
            // nested inside it: its own modifiers (e.g. `describe.skip`) make it independently
            // inert without that having to propagate through every nested test's own modifiers.
            self.push_test_case(
                DeclarationKind::Suite,
                title.clone(),
                modifiers.clone(),
                body_arg,
                call,
                resolved.fixtures.clone(),
                fixtures_unresolved || each_unresolved || row_unresolved,
                // Substitution covers `test.each`/`test.for` row callbacks only -- a
                // `describe.each`
                // row's "body" is the whole suite block, shared by every nested declaration
                // inside it (each of which is fingerprinted separately, on its own), not a single
                // callback this row's cells bind into. Out of scope here; see the module doc on
                // `fingerprint::RowSubstitution`.
                None,
                true,
            );
            if let Some(case) = self.tests.last_mut() {
                case.table_row_class = table_row_class;
            }
            match &body {
                // `.todo` suite (no callback at all): nothing to recurse into.
                None => continue,
                Some(DescribeBody::Block(statements)) => {
                    if statements.is_empty() {
                        continue;
                    }
                    self.enter_suite_scope(title, title_is_unresolved, &own_behavior_modifiers);
                    self.walk_statements(statements, registry);
                    self.exit_suite_scope(&own_behavior_modifiers, title_is_unresolved);
                }
                // Concise-body arrow (`describe("x", () => test(...))`): legal Vitest, and a
                // silently-missed suite here is the worst failure mode this tool has -- a clean
                // run that quietly dropped tests. There is no block to pre-scan for hooks, so
                // the prepare/walk split ordinary bodies need is moot for a single expression.
                Some(DescribeBody::Concise(expr)) => {
                    self.enter_suite_scope(title, title_is_unresolved, &own_behavior_modifiers);
                    self.walk_concise_body(expr, registry);
                    self.exit_suite_scope(&own_behavior_modifiers, title_is_unresolved);
                }
            }
        }
    }

    /// Enters a nested suite's scope for the duration of one body walk: title path, hook scope,
    /// unresolved-title taint, and behavior modifiers. Every arm that recurses into a suite body
    /// -- block or concise -- runs this and [`Self::exit_suite_scope`] around its walk, because
    /// the bookkeeping is the suite, independent of what shape the body takes.
    fn enter_suite_scope(
        &mut self,
        title: Title,
        title_is_unresolved: bool,
        own_behavior_modifiers: &[Modifier],
    ) {
        self.suite_path.push(title);
        self.hook_scopes.push(ScopeHooks::default());
        if title_is_unresolved {
            self.unresolved_suite_depth += 1;
        }
        self.suite_modifiers
            .extend(own_behavior_modifiers.iter().cloned());
    }

    /// Leaves the scope [`Self::enter_suite_scope`] opened, unwinding exactly what it pushed --
    /// nothing else, so a mid-walk mutation of these stacks would be silently mis-unwound.
    fn exit_suite_scope(&mut self, own_behavior_modifiers: &[Modifier], title_is_unresolved: bool) {
        self.suite_modifiers
            .truncate(self.suite_modifiers.len() - own_behavior_modifiers.len());
        if title_is_unresolved {
            self.unresolved_suite_depth -= 1;
        }
        self.hook_scopes.pop();
        self.suite_path.pop();
    }

    /// Handles a concise-body arrow callback's single expression the same way an
    /// [`Expression::CallExpression`] inside an ordinary block would be handled, since a
    /// concise body cannot contain more than one "statement" for prepare/walk ordering to matter.
    fn walk_concise_body(&mut self, expr: &Expression<'a>, registry: &mut Registry) {
        if let Expression::CallExpression(call) = expr {
            self.prepare_call(call, registry);
            self.classify_call(call, registry);
        }
    }

    /// S11: the declaration walk's half of helper inlining -- `runWithConfig('default', {})` at
    /// module scope, with the `it.concurrent(name, ...)` living inside `runWithConfig`'s body.
    ///
    /// Vitest registers whatever that body declares, because the call really runs during the
    /// module's evaluation; before this, the structural walk stopped at every `function`
    /// declaration (a "call-graph question"), so such a file analyzed to zero tests. The bounding
    /// rule is the call, not the declaration: a helper that is *defined* but never called
    /// contributes nothing (pinned by R25), because extracting from it would invent tests Vitest
    /// never runs.
    ///
    /// First-party named imports can also contribute supported static declarations, through
    /// `imported_helper_call`. Its parsed frame owns the helper's lexical/source identity while
    /// inheriting caller registration context and the same depth/budget. Installed package
    /// bodies remain refused: the original package-runner motivating shape belongs to C11.
    fn walk_helper_call(&mut self, call: &CallExpression<'a>, registry: &mut Registry) {
        if self.resolve_local_helper(&call.callee).is_some() {
            self.with_helper_body(call, registry, Self::walk_statement_list);
        } else {
            self.imported_helper_call(call, false);
        }
    }

    /// [`Self::walk_helper_call`]'s pre-scan counterpart: registers the hooks and bindings an
    /// inlined helper body declares into the *calling* scope.
    ///
    /// A helper body is not a Vitest scope -- a `beforeEach` inside one attaches to whichever
    /// suite (or the file) the call sits in -- so its hooks belong to the scope
    /// [`Self::prepare_statements`] is currently building, exactly as if they had been written at
    /// the call site. Running this in the prepare pass rather than only in the walk keeps Vitest's
    /// "a hook applies to every test in its scope regardless of textual order" rule intact for a
    /// helper called after the tests it affects.
    fn prepare_helper_call(&mut self, call: &CallExpression<'a>, registry: &mut Registry) {
        if self.resolve_local_helper(&call.callee).is_some() {
            self.with_helper_body(call, registry, Self::prepare_statements);
        } else {
            self.imported_helper_call(call, true);
        }
    }

    /// The shape both helper-expansion passes share: open the frame, run one walk over the
    /// helper's statement list, close the frame. `walk` is a plain method (not a closure) so the
    /// borrow of `self` inside it cannot tangle with the frame `begin_helper_expansion` handed
    /// back -- the two walks that exist today are `Self::walk_statement_list` (declaration walk)
    /// and `Self::prepare_statements` (hook/binding pre-scan).
    fn with_helper_body(
        &mut self,
        call: &CallExpression<'a>,
        registry: &mut Registry,
        walk: fn(&mut Self, &[Statement<'a>], &mut Registry),
    ) {
        let Some(frame) = self.begin_helper_expansion(call) else {
            return;
        };
        if let Some(body) = &frame.function.body {
            walk(self, &body.statements, registry);
        }
        self.end_helper_expansion(frame);
    }

    /// Resolves `call` to a helper this file declares and opens a frame for it, or returns `None`
    /// (silently, when `call` is simply not a local helper; with a diagnostic when it is one this
    /// walker refuses to expand -- see [`Self::refuse_helper_expansion`]).
    fn begin_helper_expansion(&mut self, call: &CallExpression<'a>) -> Option<HelperFrame<'a>> {
        let (_, function) = self.resolve_local_helper(&call.callee)?;

        let identity = (
            self.file
                .path
                .canonicalize()
                .unwrap_or_else(|_| self.file.path.clone()),
            function.span.start,
        );
        let admission = self.expansion.borrow_mut().begin(identity);
        if let Err(reason) = admission {
            self.refuse_helper_expansion(reason);
            return None;
        }

        let before = self.param_bindings.len();
        self.bind_helper_parameters(function, call);
        let frame = HelperFrame {
            function,
            bindings: self.param_bindings.len() - before,
        };
        self.helper_frames.push(frame);
        Some(frame)
    }

    /// Restores caller parameter bindings and removes the completed local expansion identity.
    fn end_helper_expansion(&mut self, frame: HelperFrame<'a>) {
        self.helper_frames.pop();
        self.expansion.borrow_mut().active.pop();
        self.param_bindings
            .truncate(self.param_bindings.len() - frame.bindings);
    }

    /// Records that a helper body was *not* expanded, so the file reads `incomplete` instead of
    /// reporting a smaller test count as if it were the whole truth.
    ///
    /// Existing records are tainted once per scope; later records inherit its unresolved flag.
    /// The scan guard does not suppress the file's first refusal diagnostic.
    ///
    /// At most one such diagnostic per file (see [`Walker::helper_refusal_reported`]): the flag
    /// this sets is what makes the run visibly degraded, and repeating it per call site would
    /// bury every other diagnostic under a machine-generated file's thousands of copies.
    fn refuse_helper_expansion(&mut self, reason: &str) {
        let already_tainted = self
            .hook_scopes
            .last()
            .is_some_and(|scope| scope.unresolved);
        if let Some(scope) = self.hook_scopes.last_mut() {
            scope.unresolved = true;
        }
        if !already_tainted {
            for case in &mut self.tests {
                #[cfg(test)]
                {
                    self.helper_refusal_taint_visits += 1;
                }
                if case.suite_path.starts_with(&self.suite_path)
                    || (case.kind == DeclarationKind::Suite
                        && self
                            .suite_path
                            .last()
                            .is_some_and(|title| title == &case.title)
                        && case.suite_path == self.suite_path[..self.suite_path.len() - 1])
                {
                    case.context.unresolved = true;
                }
            }
        }
        if self.helper_refusal_reported {
            return;
        }
        self.helper_refusal_reported = true;
        // No path in the text. `FileExtraction::diagnostics` reaches callers that prefix every
        // entry with the file's own relativized path (see this module's header), so embedding one
        // printed the file twice -- once relative, once ABSOLUTE, which is the S12 leak this
        // branch fixed everywhere else. Found in review: `no_stderr_line_contains_the_analysis_
        // root_prefix` could not catch it because its fixture never built a helper deep enough,
        // recursive enough or expensive enough to be refused. That fixture now does.
        self.diagnostics.push(format!(
            "a called helper's body was not analyzed ({reason}); any test/describe \
             declarations inside it were not counted"
        ));
    }

    /// The callee of `expr`, when it names a `function` declaration in *this* file.
    ///
    /// Deliberately narrow, and every restriction is load-bearing:
    ///
    /// - a bare identifier only, resolved through `oxc_semantic` to its binding rather than by
    ///   name -- `ruleTester.run(...)` (a member expression) is not a local helper, and neither is
    ///   a same-named import, whose declaration node is an import specifier and not a `Function`;
    /// - a `FunctionType::FunctionDeclaration`, so a `const f = () => {}` binding (whose declaring
    ///   node is a `VariableDeclarator`) is left alone: a `let`/`var` binding can be reassigned
    ///   between declaration and call, and proving which function a name holds at call time is the
    ///   call-graph question this fix deliberately does not open;
    /// - a body present, which excludes a TypeScript overload signature or `declare function`.
    ///
    /// Anything outside that set falls through to the pre-S11 behaviour -- the call contributes
    /// nothing -- rather than being guessed at.
    fn resolve_local_helper(&self, expr: &Expression) -> Option<(SymbolId, &'a Function<'a>)> {
        let Expression::Identifier(ident) = expr else {
            return None;
        };
        let semantic: &'a Semantic<'a> = self.semantic;
        let reference_id = ident.reference_id.get()?;
        let symbol = semantic.scoping().get_reference(reference_id).symbol_id()?;
        let AstKind::Function(function) = semantic.symbol_declaration(symbol).kind() else {
            return None;
        };
        if function.r#type != FunctionType::FunctionDeclaration || function.body.is_none() {
            return None;
        }
        Some((symbol, function))
    }

    /// Binds each of `function`'s formal parameters to the const-folded text of the argument this
    /// call site passed for it, for [`Walker::fold_expr`].
    ///
    /// Positional, and therefore refused wholesale the moment the positions stop being knowable:
    /// a spread argument (`run(...args)`) makes every position after it unknown, and this declines
    /// to bind *any* parameter rather than shift them by a guess. A destructuring or rest
    /// parameter binds no single symbol and is skipped; so is a parameter whose argument is
    /// missing or does not const-fold. Every skip leaves the parameter unbound, which is what
    /// makes a title reading it degrade to [`Title::Dynamic`] instead of rendering a value nobody
    /// proved.
    fn bind_helper_parameters(&mut self, function: &'a Function<'a>, call: &CallExpression<'a>) {
        let bound = self.fold_parameter_arguments(&function.params, call);
        self.param_bindings.extend(bound);
    }

    /// Pairs each plain parameter symbol of `parameters` with its folded call-site argument.
    /// The caller folds, so a callee in another parsed frame can take the owned text under its
    /// own symbols. A spread argument yields no pairs.
    fn fold_parameter_arguments(
        &self,
        parameters: &FormalParameters<'_>,
        call: &CallExpression<'_>,
    ) -> Vec<(SymbolId, String)> {
        if call
            .arguments
            .iter()
            .any(|argument| matches!(argument, Argument::SpreadElement(_)))
        {
            return Vec::new();
        }
        let mut bound = Vec::new();
        for (index, parameter) in parameters.items.iter().enumerate() {
            let BindingPattern::BindingIdentifier(binding) = &parameter.pattern else {
                continue;
            };
            let Some(symbol) = binding.symbol_id.get() else {
                continue;
            };
            let Some(value) = call
                .arguments
                .get(index)
                .and_then(Argument::as_expression)
                .and_then(|argument| self.fold_expr(argument))
            else {
                continue;
            };
            bound.push((symbol, value));
        }
        bound
    }

    /// Assembles one extracted [`TestCase`] from a recognized declaration call. The parameters
    /// are the extracted declaration itself; grouping them into a struct here would just rename
    /// `TestCase`'s own fields one step early.
    #[allow(clippy::too_many_arguments)]
    fn push_test_case(
        &mut self,
        kind: DeclarationKind,
        title: Title,
        modifiers: Vec<Modifier>,
        body_arg: Option<&Argument<'a>>,
        call: &CallExpression<'a>,
        mut fixtures: Vec<FixtureBinding>,
        fixtures_unresolved: bool,
        // `Some` only for a `test.each`/`test.for` row whose own cells were all statically
        // readable -- see `row_substitution`'s doc and `fingerprint::RowSubstitution`'s.
        row_substitution: Option<RowSubstitution>,
        // `true` unless this is a `.each` row whose callback provably drops the cells past its
        // fixed parameters; see `crate::model::TestCase::row_cells_fully_observable`.
        row_cells_fully_observable: bool,
    ) {
        let has_body = body_arg.and_then(as_function_like).is_some();
        let body_fingerprint = body_arg.and_then(as_function_like).map(|function_like| {
            fingerprint(
                function_like,
                self.semantic,
                self.source,
                &self.file.path,
                row_substitution.as_ref(),
            )
        });
        // A test body's `alpha` text has no consumer -- every rule compares the digest, and
        // the published record carries the digest's own `published` half -- so it is dropped
        // here and freed immediately rather than retained for the run. Only the digest and the
        // inlined-import report survive this block.
        let body = match &body_fingerprint {
            Some(prints) => prints.digest(),
            None => Fingerprints {
                trivial: true,
                ..Fingerprints::default()
            },
        };
        let mut imports = match body_arg.and_then(as_function_like) {
            Some(function_like) => self.imports_used_by(function_like),
            None => Vec::new(),
        };
        // Imported-helper inlining's context half: the alpha pass reports the modules whose
        // every reference it rendered as a helper body (see `fingerprint`'s construction of the
        // report), and exactly those drop out of the compared imports -- their content is
        // compared directly, and keeping the identity would block the cross-module recognition
        // inlining exists to make. Taken from the pass's own accounting rather than re-derived
        // beside it, because a re-derivation can disagree with the renderer exactly where the
        // renderer declines to render (an opaque class body hides a call from `alpha` while
        // still showing it to a reference-counting visitor) -- a review-found false merge.
        if let Some(prints) = &body_fingerprint {
            imports.retain(|component| {
                !prints
                    .inlined_imports
                    .iter()
                    .any(|inlined| inlined.as_str() == &**component)
            });
            imports.shrink_to_fit();
        }
        fixtures.sort();
        fixtures.dedup();
        fixtures.shrink_to_fit();

        // C1: every other ambient fact this extractor could not resolve, folded into one flag.
        //
        // - `self.mocks_unresolved`: a hoisted `vi.mock`/`vi.unmock` this scan could not fully
        //   read (A3/A4), any `vi.doMock`/`vi.doUnmock` call at all, or any mocking call outside
        //   module top level (F4).
        // - an unresolved title (`Title::Dynamic`, or per F5 `Title::Parameterized`): its runtime
        //   value is not statically known, so two differently-behaving tests could otherwise
        //   collide on the same normalized shape -- see [`title_is_unresolved`].
        // - `opaque_body`: a body argument was supplied but is not a shape this extractor
        //   recurses into (e.g. a named helper function reference rather than an inline
        //   arrow/function expression, or a `describe` callback that is neither a block nor a
        //   concise-arrow expression). The body is opaque, not absent -- unlike a genuine
        //   `.todo`/bodyless declaration, where `body_arg` is `None` and this stays `false`.
        let opaque_body = body_arg.is_some() && !has_body;
        let is_unresolved_title = title_is_unresolved(&title);
        // Defect 3 / F5: nested inside a `describe` whose own title is dynamic or parameterized,
        // this case's identity is exactly as unknown as that suite's, even when its own title and
        // everything else about it is fully static -- see the doc on
        // `Walker::unresolved_suite_depth`.
        let unresolved = fixtures_unresolved
            || self.mocks_unresolved
            || is_unresolved_title
            || opaque_body
            // S6 mechanism 2: a nested config that EXISTS and cannot be read decides how these
            // tests run, and not knowing how is the definition of an unresolved context. Folded
            // in here rather than silently treated as "no nested config", which would compare
            // the file as if the analysis root governed it alone.
            || matches!(self.file.config_scope, Some(Err(_)))
            || self.unresolved_suite_depth > 0
            || self.hook_scopes.iter().any(|scope| scope.unresolved);

        self.tests.push(TestCase {
            registration_file: self.registration_file.clone(),
            location: self.location(call.span),
            kind,
            suite_path: self.suite_path.clone(),
            title,
            modifiers,
            context: Context {
                hooks: self.effective_hooks(),
                mocked_modules: self.mocked_modules.clone(),
                module_setup: self.module_setup.clone(),
                // S6 mechanism 2. An unreadable nested config is NOT "no config": it is a config
                // that exists and whose contents decide how these tests run, so the context is
                // marked unresolved rather than compared as if the file were governed by the
                // root alone. `unresolved` is folded in below.
                config_scope: match &self.file.config_scope {
                    Some(Ok(scope)) => Some(scope.clone()),
                    Some(Err(_)) | None => None,
                },
                fixtures,
                imports,
                unresolved,
            },
            table_row_class: None,
            row_cells_fully_observable,
            body,
            has_body,
        });
    }

    /// Free identifiers in `function_like`'s body that resolve (via `oxc_semantic`, not name
    /// matching) to an imported symbol, mapped to that import's anchored module identity (see
    /// [`collect_import_specifiers`]). The imported-helper inlining drop is applied by the
    /// caller, from the fingerprint pass's own report -- not here, so there is exactly one
    /// authority on what got inlined.
    fn imports_used_by(&self, function_like: FunctionLike<'a, '_>) -> Vec<Arc<str>> {
        struct Collector<'r> {
            scoping: &'r Scoping,
            import_specifiers: &'r HashMap<SymbolId, Arc<str>>,
            found: Vec<Arc<str>>,
        }
        impl<'a, 'r> Visit<'a> for Collector<'r> {
            /// Collect anchored module identities for references resolved to imported symbols.
            fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'a>) {
                if let Some(reference_id) = it.reference_id.get() {
                    if let Some(symbol_id) = self.scoping.get_reference(reference_id).symbol_id() {
                        if let Some(specifier) = self.import_specifiers.get(&symbol_id) {
                            self.found.push(specifier.clone());
                        }
                    }
                }
            }
        }
        let mut collector = Collector {
            scoping: self.semantic.scoping(),
            import_specifiers: &self.import_specifiers,
            found: Vec::new(),
        };
        match function_like {
            FunctionLike::Arrow(arrow) => collector.visit_arrow_function_expression(arrow),
            FunctionLike::Function(function) => {
                collector.visit_function(function, ScopeFlags::Function)
            }
        }
        // One entry was pushed per identifier REFERENCE, so after dedup the vector's capacity
        // is the reference count, not the import count -- retained on every test as empty
        // slots. Shrink to what survived.
        collector.found.sort();
        collector.found.dedup();
        collector.found.shrink_to_fit();
        collector.found
    }

    /// The test's effective hook chain: every enclosing scope's `beforeAll`/`beforeEach` from the
    /// file root down (outermost first), then every scope's `afterEach`/`afterAll` back up
    /// (innermost first) -- per `docs/vitest-surface.md`'s "Hooks" ordering rule.
    fn effective_hooks(&self) -> Vec<Hook> {
        let mut hooks = Vec::new();
        for scope in &self.hook_scopes {
            hooks.extend(scope.before_all.iter().cloned());
            hooks.extend(scope.before_each.iter().cloned());
        }
        for scope in self.hook_scopes.iter().rev() {
            hooks.extend(scope.after_each.iter().cloned());
            hooks.extend(scope.after_all.iter().cloned());
        }
        hooks
    }

    /// Convert a parser byte span into an owned source location with line and UTF-16 column
    /// positions.
    fn location(&self, span: Span) -> Location {
        Location {
            path: self.file.path.clone(),
            span: crate::model::Span {
                start: self.line_index.position(span.start),
                end: self.line_index.position(span.end),
                start_byte: span.start,
                end_byte: span.end,
            },
        }
    }
}

/// Map the four supported lifecycle-hook names to their owned hook kinds, returning None for
/// other names.
fn hook_kind(name: &str) -> Option<HookKind> {
    match name {
        "beforeAll" => Some(HookKind::BeforeAll),
        "beforeEach" => Some(HookKind::BeforeEach),
        "afterEach" => Some(HookKind::AfterEach),
        "afterAll" => Some(HookKind::AfterAll),
        _ => None,
    }
}

/// Borrow an inline arrow or function expression argument as a callable, declining other
/// argument forms.
fn as_function_like<'a, 'b>(arg: &'b Argument<'a>) -> Option<FunctionLike<'a, 'b>> {
    match arg {
        Argument::ArrowFunctionExpression(arrow) => Some(FunctionLike::Arrow(arrow)),
        Argument::FunctionExpression(function) => Some(FunctionLike::Function(function)),
        _ => None,
    }
}

/// Recognizes `for (const x of [a, b, ...])` -- a simple identifier bound over a literal array --
/// and returns the loop variable's symbol plus every element's const-folded display text, in
/// iteration order.
///
/// Returns `None` for anything this cannot enumerate just as conservatively as
/// `registry::literal_table_rows` does for `.each`: a destructuring pattern, a `let`/`var`-free
/// assignment target, a non-array iterable (an identifier, a call, a spread inside the array, an
/// element that does not itself const-fold, ...). [`Walker::walk_for_of_statement`] falls back to
/// walking the loop body once, unrolled, in every one of those cases.
fn literal_for_of_elements(for_of: &ForOfStatement) -> Option<(SymbolId, Vec<String>)> {
    let ForStatementLeft::VariableDeclaration(declaration) = &for_of.left else {
        return None;
    };
    let [declarator] = declaration.declarations.as_slice() else {
        return None;
    };
    let BindingPattern::BindingIdentifier(binding) = &declarator.id else {
        return None;
    };
    let symbol_id = binding.symbol_id.get()?;
    let Expression::ArrayExpression(array) = &for_of.right else {
        return None;
    };
    let mut values = Vec::with_capacity(array.elements.len());
    for element in &array.elements {
        match element {
            ArrayExpressionElement::SpreadElement(_) | ArrayExpressionElement::Elision(_) => {
                return None;
            }
            _ => values.push(const_fold(element.as_expression()?, None)?),
        }
    }
    if values.is_empty() {
        return None;
    }
    Some((symbol_id, values))
}

/// A `describe` callback's body: either an ordinary block, or a concise-body arrow
/// (`describe("x", () => test(...))`) whose entire body is a single expression.
enum DescribeBody<'r, 'a> {
    Block(&'r [Statement<'a>]),
    Concise(&'r Expression<'a>),
}

/// Borrow a readable suite callback as block statements or a concise arrow expression.
fn describe_body<'r, 'a>(body_arg: Option<&'r Argument<'a>>) -> Option<DescribeBody<'r, 'a>> {
    match body_arg.and_then(as_function_like) {
        Some(FunctionLike::Arrow(arrow)) => match &arrow.body {
            ArrowFunctionBody::FunctionBody(body) => Some(DescribeBody::Block(&body.statements)),
            concise => concise.as_expression().map(DescribeBody::Concise),
        },
        Some(FunctionLike::Function(function)) => function
            .body
            .as_deref()
            .map(|body| DescribeBody::Block(body.statements.as_slice())),
        None => None,
    }
}

/// Separate title, optional literal options object and body arguments by inspecting the second
/// argument's shape.
fn split_declaration_args<'r, 'a>(
    args: &'r [Argument<'a>],
) -> (
    Option<&'r Argument<'a>>,
    Option<&'r ObjectExpression<'a>>,
    Option<&'r Argument<'a>>,
) {
    let title = args.first();
    match args.get(1) {
        // Hazard 7: options-vs-body is a type disambiguation, not a positional one.
        Some(Argument::ObjectExpression(options)) => (title, Some(options), args.get(2)),
        other => (title, None, other),
    }
}

/// Add recognized modifiers whose options-object properties are literal true, without
/// duplicating modifiers.
fn union_option_modifiers(modifiers: &mut Vec<Modifier>, options: &ObjectExpression) {
    for property in &options.properties {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            continue;
        };
        let Some(name) = property_key_name(&property.key) else {
            continue;
        };
        let is_true =
            matches!(&property.value, Expression::BooleanLiteral(literal) if literal.value);
        if !is_true {
            continue;
        }
        let modifier = match name.as_str() {
            "skip" => Modifier::Skip,
            "only" => Modifier::Only,
            "todo" => Modifier::Todo,
            "fails" => Modifier::Fails,
            "concurrent" => Modifier::Concurrent,
            _ => continue,
        };
        if !modifiers.contains(&modifier) {
            modifiers.push(modifier);
        }
    }
}

/// The parsed, but not-yet-title-rendered, `.each`/`.for` table: kept separate from
/// [`Modifier::Each`] because title rendering needs the row cells, not just their display text.
struct EachInfo {
    rows: Result<Vec<Row>, String>,
}

/// Assigns exact-equality classes within one table without retaining each row's canonical text.
///
/// The class number is meaningful only together with the declaration location, which is exactly
/// how `duplicate-each-row` groups it. Assignment follows first occurrence order; a `HashMap` is
/// only used for lookup, so randomized iteration cannot affect the result. Unresolved rows get no
/// class and remain excluded from comparison.
fn table_row_classes(rows: &[Row]) -> Vec<Option<u32>> {
    let mut classes = HashMap::<String, u32>::new();
    let mut next = 0_u32;
    rows.iter()
        .map(|row| {
            let identity = row.identity()?;
            Some(*classes.entry(identity).or_insert_with(|| {
                let class = next;
                next = next.saturating_add(1);
                class
            }))
        })
        .collect()
}

/// Splits a resolved chain into display modifiers, its `.each`/`.for` table (if any), and whether
/// any `.skipIf`/`.runIf` call along the chain had no expression argument to fingerprint -- see
/// `registry::condition_fingerprint`.
fn split_modifiers_and_each(steps: Vec<ChainStep>) -> (Vec<Modifier>, Option<EachInfo>, bool) {
    let mut modifiers = Vec::new();
    let mut each = None;
    let mut condition_unresolved = false;
    for step in steps {
        match step {
            ChainStep::Member(name) => modifiers.push(modifier_for_member(&name)),
            ChainStep::Each { is_for, rows } => {
                let table_rows = match &rows {
                    Ok(rows) => {
                        EachRows::Literal(rows.iter().map(|row| row.display.clone()).collect())
                    }
                    Err(identity) => EachRows::Unreadable(identity.clone()),
                };
                modifiers.push(Modifier::Each(EachTable {
                    is_for,
                    rows: table_rows,
                }));
                each = Some(EachInfo { rows });
            }
            ChainStep::SkipIf {
                condition,
                unresolved,
            } => {
                modifiers.push(Modifier::SkipIf(condition));
                condition_unresolved |= unresolved;
            }
            ChainStep::RunIf {
                condition,
                unresolved,
            } => {
                modifiers.push(Modifier::RunIf(condition));
                condition_unresolved |= unresolved;
            }
        }
    }
    (modifiers, each, condition_unresolved)
}

/// The subset of `modifiers` that `analysis.rs`'s `ModifierKey` treats as behavior-significant
/// for `duplicate-test` (`Each`/`SkipIf`/`RunIf` -- everything else, `.skip`/`.only`/`.concurrent`/
/// etc., does not change which key two tests compare equal under). Used to decide what a suite's
/// own modifiers contribute to [`Walker::suite_modifiers`] when nested declarations are walked --
/// see that field's doc for F3, the defect this exists to close.
fn behavior_modifiers(modifiers: &[Modifier]) -> Vec<Modifier> {
    modifiers
        .iter()
        .filter(|modifier| {
            matches!(
                modifier,
                Modifier::Each(_) | Modifier::SkipIf(_) | Modifier::RunIf(_)
            )
        })
        .cloned()
        .collect()
}

/// Whether a title's runtime value is *not* proven -- true for [`Title::Dynamic`] and, per F5,
/// [`Title::Parameterized`] as well.
///
/// F5: a `Parameterized` title is not just "the value happens to still show placeholders" the way
/// `Dynamic`'s normalized shape is -- `render_each_title` falls back to it verbatim (the raw,
/// unrendered pattern, identical across every row it fails to render) whenever any referenced
/// value cannot be statically rendered into the title text, which is a strictly *broader*
/// condition than "this row's own cell was unresolved": a named `.each` placeholder needing a
/// dotted path into a row's cell (`'$user.name'`) falls back the same way even when the cell
/// itself was perfectly readable, because the title *renderer* -- not the row -- is what gives
/// up. Before this fix only `Dynamic` set the caller's unresolved flag, so two declarations of
/// `'case $value'` with different, non-foldable runtime expressions could render to the exact
/// same `Parameterized(pattern)` title and, if their bodies also matched, compare equal at Error
/// severity.
fn title_is_unresolved(title: &Title) -> bool {
    matches!(title, Title::Dynamic(_) | Title::Parameterized(_))
}

/// Map a declaration member name to its modifier, retaining unsupported names as Unknown.
fn modifier_for_member(name: &str) -> Modifier {
    match name {
        "skip" => Modifier::Skip,
        "only" => Modifier::Only,
        "todo" => Modifier::Todo,
        "fails" => Modifier::Fails,
        "concurrent" => Modifier::Concurrent,
        // [v5 CHANGE] removed in v5, but pre-v5 corpora still contain it (hazard 2) -- it must
        // surface as a real modifier, not be swallowed into `Unknown`.
        "sequential" => Modifier::Sequential,
        "shuffle" => Modifier::Shuffle,
        other => Modifier::Unknown(other.to_owned()),
    }
}

/// A `.each` title pattern, and whether it is the declaration's real pattern or its source text.
///
/// The distinction is S7's title half. The first two arms below produce the string Vitest itself
/// would format row values into; the third produces raw SOURCE TEXT for a title expression this
/// extractor could not fold -- a template literal with a live substitution, an identifier, a
/// concatenation. Rendering `%s` into `` `ready ${mk()}` `` "succeeds" and hands the reader a
/// template nobody evaluated, identical for every row of the declaration by construction, which
/// is what let `duplicate-each-row` group 18 of its 41 false positives on a title that never
/// discriminated (all nine findings in one HTTP client are a single `it.each` in a `for` loop).
///
/// Structural rather than textual on purpose: scanning the pattern for `${` would misread
/// `it.each(rows)('cost is ${x}', ...)` -- a single-quoted string whose `${x}` is literal text
/// Vitest prints verbatim -- as unresolvable, and start declining titles that do render.
struct TitlePattern {
    text: String,
    /// `false` when `text` is source text standing in for a value this extractor could not read.
    resolved: bool,
}

/// Read a title pattern and whether it was resolved, retaining source text for unsupported
/// dynamic forms.
fn raw_title_pattern(title_arg: Option<&Argument>, source: &str) -> TitlePattern {
    match title_arg {
        Some(Argument::StringLiteral(literal)) => TitlePattern {
            text: literal.value.as_str().to_owned(),
            resolved: true,
        },
        Some(Argument::TemplateLiteral(template)) if template.is_no_substitution_template() => {
            TitlePattern {
                text: template
                    .quasis
                    .first()
                    .and_then(|quasi| quasi.value.cooked.as_ref())
                    .map(|cooked| cooked.as_str().to_owned())
                    .unwrap_or_default(),
                resolved: true,
            }
        }
        Some(argument) => TitlePattern {
            text: argument
                .as_expression()
                .map(|expr| source_text(source, expr.span()))
                .unwrap_or_default(),
            resolved: false,
        },
        // No title argument at all: there is nothing to render and nothing to misrepresent.
        // `resolved` is true so this behaves exactly as it did before -- an empty pattern renders
        // to an empty title, which is what Vitest does with a missing name.
        None => TitlePattern {
            text: String::new(),
            resolved: true,
        },
    }
}

impl<'a> Walker<'a> {
    /// Extracts a declaration's title, consulting the current `for...of` loop bindings (via
    /// [`Walker::fold_expr`]) so a title built from an unrolled loop's own variable renders the
    /// value for *this* iteration rather than degrading to [`Title::Dynamic`].
    fn extract_title(&self, title_arg: Option<&Argument>) -> Title {
        match title_arg {
            Some(Argument::StringLiteral(literal)) => {
                Title::Static(literal.value.as_str().to_owned())
            }
            Some(Argument::TemplateLiteral(template)) => {
                if template.is_no_substitution_template() {
                    let text = template
                        .quasis
                        .first()
                        .and_then(|quasi| quasi.value.cooked.as_ref())
                        .map(|cooked| cooked.as_str().to_owned())
                        .unwrap_or_default();
                    Title::Static(text)
                } else {
                    self.render_template_title(template)
                }
            }
            Some(argument) => {
                let expr = argument.as_expression();
                if let Some(value) = expr.and_then(|expr| self.fold_expr(expr)) {
                    return Title::Static(value);
                }
                let text = expr
                    .map(|expr| source_text(self.source, expr.span()))
                    .unwrap_or_default();
                Title::Dynamic(text)
            }
            None => Title::Dynamic(String::new()),
        }
    }

    /// Renders a template-literal title when every substitution folds to a known value (via
    /// [`Walker::fold_expr`], which resolves an unrolled loop's own variable to this iteration's
    /// element before falling back to ordinary constant folding), else normalizes every
    /// substitution to a placeholder (hazard 6: never claim a runtime value the extractor cannot
    /// prove from source, and never mix "some substitutions folded, some didn't" into one shape).
    fn render_template_title(&self, template: &TemplateLiteral) -> Title {
        let folded: Vec<Option<String>> = template
            .expressions
            .iter()
            .map(|expr| self.fold_expr(expr))
            .collect();
        if folded.iter().all(Option::is_some) {
            let mut rendered = String::new();
            for (index, quasi) in template.quasis.iter().enumerate() {
                rendered.push_str(
                    quasi
                        .value
                        .cooked
                        .as_ref()
                        .map(|cooked| cooked.as_str())
                        .unwrap_or(""),
                );
                if let Some(Some(value)) = folded.get(index) {
                    rendered.push_str(value);
                }
            }
            Title::Static(rendered)
        } else {
            let mut shape = String::new();
            for (index, quasi) in template.quasis.iter().enumerate() {
                shape.push_str(
                    quasi
                        .value
                        .cooked
                        .as_ref()
                        .map(|cooked| cooked.as_str())
                        .unwrap_or(""),
                );
                if index < template.expressions.len() {
                    shape.push_str("${_}");
                }
            }
            Title::Dynamic(shape)
        }
    }
}

/// Builds the [`RowSubstitution`] `fingerprint` should apply for one `.each`/`.for` row's body, or
/// `None` when nothing should be substituted at all.
///
/// `row.unresolved` already means "at least one cell in this row is not a value this extractor
/// could read" (`registry::analyze_row`) -- a row that is not fully known must never be
/// half-substituted (see `fingerprint.rs`'s row-substitution doc for why), so this bails out
/// immediately in that case, before even looking at individual cells. When `row.unresolved` is
/// `false`, every [`CellValue`] this row does carry is guaranteed genuinely known (an elided
/// array hole is the one exception -- a `None` cell that is *not* an unresolved failure, just a
/// statically-known `undefined` this module does not model as a substitutable value -- and
/// [`RowSubstitution`]'s own gap handling covers that uniformly with "no cell at this position at
/// all", which is exactly the right treatment: see its doc).
/// Whether `body_arg`'s callback can observe cells of `row` beyond the ones it binds by position.
///
/// See [`crate::model::TestCase::row_cells_fully_observable`] for why the rule turns on this.
/// Conservative in the direction that suppresses findings: anything this cannot read as a plain
/// fixed parameter list is treated as observing everything.
/// Whether every statement of a hook body is a provably idempotent operation.
///
/// S5. `redundant-hook` scored **0 of 10, exhaustive** on the 2026-09-27 scan because it treated
/// a repeated NON-idempotent statement as redundant setup. One React primitives library repeats
/// `fireEvent.click(two)` under a describe titled "and clicking `Two` again" -- the second click
/// IS the behaviour. A test framework's own hook suite repeats `count += 1`, in the very file that
/// tests that nested hooks accumulate. Deleting either changes the result, so "you may delete one" was
/// wrong every time it was said.
///
/// This fails **closed**: a body qualifies only when every statement is one of three shapes this
/// function can argue about, and anything else -- any ordinary call, any compound assignment --
/// makes the whole body `false`. On the measured corpus that takes the rule from 10 findings and
/// 0 true to 0 findings; forcing `--globals` (which admits the files a config blind spot hides)
/// it goes to 1 finding and 1 true. That is the trade taken deliberately: the rule's claim is
/// "deleting one loses no coverage", and it had never once been true.
///
/// The three shapes:
///
/// 1. **Constant overwrite** -- `TARGET = <primitive literal>` with the plain `=` operator.
///    Running it twice leaves the same value. Primitives only: `X = { a: 1 }` allocates a fresh
///    object each run, which is not a no-op for anything holding a reference. `count += 1` is
///    excluded by the operator, which is what silences the `vitest` accumulator.
/// 2. **Guarded initialization** -- `X ??= E`, `X ||= E`, and `if (!X) X = E;`, where the guard
///    tests the very binding being assigned. The second run's guard is false, so nothing happens.
/// 3. **Framework reset vocabulary**, resolved through the import graph rather than by name:
///    `cleanup` from a `@testing-library/*` package, and `vi`'s own reset family. Resolution by
///    module is what keeps this from being the name heuristic
///    `not-redundant-hook-guarded-connect.test.ts` exists to forbid -- an unrelated local
///    function called `cleanup` resolves elsewhere and does not qualify.
///
/// Composition is checked, not assumed: two individually idempotent statements are not jointly
/// idempotent if a later one writes what an earlier guard tested (`if (!x) x = f(); x = false;`
/// re-runs `f()` on every pass). A body whose guard target is written by any other statement is
/// refused.
fn hook_body_is_idempotent(
    function_like: &FunctionLike,
    semantic: &Semantic,
    import_specifiers: &HashMap<SymbolId, Arc<str>>,
    exported_as: &HashMap<SymbolId, String>,
) -> bool {
    let Some(statements) = function_like_statements(function_like) else {
        // A concise-arrow body (`() => expr`) is one expression; treat it as a single statement.
        return false;
    };
    if statements.is_empty() {
        // An empty body is trivial and never reaches this rule, but "vacuously idempotent" is the
        // wrong answer to hand back if it ever did.
        return false;
    }

    let mut guard_targets: Vec<String> = Vec::new();
    let mut written_targets: Vec<String> = Vec::new();

    for statement in statements {
        match idempotent_statement(statement, semantic, import_specifiers, exported_as) {
            Some(IdempotentStatement::Guarded { target }) => {
                guard_targets.push(target.clone());
                written_targets.push(target);
            }
            Some(IdempotentStatement::Overwrite { target }) => written_targets.push(target),
            Some(IdempotentStatement::Reset) => {}
            None => return false,
        }
    }

    // The composition hole: a guard is only self-disabling while nothing else re-arms it.
    !guard_targets.iter().any(|guarded| {
        written_targets
            .iter()
            .filter(|written| *written == guarded)
            .count()
            > 1
    })
}

/// One statement's idempotence classification; see [`hook_body_is_idempotent`].
enum IdempotentStatement {
    /// `TARGET = <primitive literal>`.
    Overwrite { target: String },
    /// `X ??= E`, `X ||= E`, `if (!X) X = E;`.
    Guarded { target: String },
    /// A framework reset call, which writes nothing this function tracks.
    Reset,
}

/// Borrow block-body statements from a callable, returning None for concise arrows or absent
/// function bodies.
fn function_like_statements<'a, 'b>(
    function_like: &'b FunctionLike<'a, 'b>,
) -> Option<&'b oxc_allocator::Vec<'a, Statement<'a>>> {
    match function_like {
        FunctionLike::Arrow(arrow) => match &arrow.body {
            ArrowFunctionBody::FunctionBody(body) => Some(&body.statements),
            _ => None,
        },
        FunctionLike::Function(function) => function.body.as_ref().map(|body| &body.statements),
    }
}

/// The rendered path of an assignment target this function can argue about: a bare identifier or
/// a dotted static-member chain rooted in one. Anything else (a computed member, a call result)
/// returns `None` and refuses the statement.
fn simple_target_path(expression: &Expression) -> Option<String> {
    match expression {
        Expression::Identifier(ident) => Some(ident.name.as_str().to_owned()),
        Expression::StaticMemberExpression(member) => Some(format!(
            "{}.{}",
            simple_target_path(&member.object)?,
            member.property.name.as_str()
        )),
        _ => None,
    }
}

/// Render an identifier or static member-chain assignment target, declining unsupported target
/// forms.
fn assignment_target_path(target: &AssignmentTarget) -> Option<String> {
    match target {
        AssignmentTarget::AssignmentTargetIdentifier(ident) => Some(ident.name.as_str().to_owned()),
        AssignmentTarget::StaticMemberExpression(member) => Some(format!(
            "{}.{}",
            simple_target_path(&member.object)?,
            member.property.name.as_str()
        )),
        _ => None,
    }
}

/// Whether `expression` is a primitive literal -- a value re-assigning cannot make new.
fn is_primitive_literal(expression: &Expression) -> bool {
    matches!(
        expression,
        Expression::StringLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::BigIntLiteral(_)
    )
}

/// Classify supported overwrite, guarded-write and framework-reset statements, declining other
/// effects.
fn idempotent_statement(
    statement: &Statement,
    semantic: &Semantic,
    import_specifiers: &HashMap<SymbolId, Arc<str>>,
    exported_as: &HashMap<SymbolId, String>,
) -> Option<IdempotentStatement> {
    match statement {
        Statement::ExpressionStatement(statement) => match &statement.expression {
            Expression::AssignmentExpression(assignment) => {
                let target = assignment_target_path(&assignment.left)?;
                match assignment.operator {
                    AssignmentOperator::Assign if is_primitive_literal(&assignment.right) => {
                        Some(IdempotentStatement::Overwrite { target })
                    }
                    // `??=` and `||=` assign only while the target is nullish/falsy, so the
                    // second run is a no-op -- for a target the first run made non-nullish. A
                    // guard whose initializer can yield `''`/`0`/`null` re-fires; that is the
                    // same hole `not-redundant-hook-guarded-connect.test.ts`'s header names, and
                    // it is why this is `Guarded` (composition-checked) rather than unconditional.
                    AssignmentOperator::LogicalNullish | AssignmentOperator::LogicalOr => {
                        Some(IdempotentStatement::Guarded { target })
                    }
                    _ => None,
                }
            }
            Expression::CallExpression(call) => {
                is_framework_reset_call(call, semantic, import_specifiers, exported_as)
                    .then_some(IdempotentStatement::Reset)
            }
            _ => None,
        },
        Statement::IfStatement(statement) => {
            // `if (!X) X = E;` -- and nothing else. No `else`, one assignment in the consequent,
            // and the guard must test the very binding assigned.
            if statement.alternate.is_some() {
                return None;
            }
            let Expression::UnaryExpression(unary) = &statement.test else {
                return None;
            };
            if unary.operator != UnaryOperator::LogicalNot {
                return None;
            }
            let guarded = simple_target_path(&unary.argument)?;
            let assignment = match &statement.consequent {
                Statement::ExpressionStatement(inner) => &inner.expression,
                Statement::BlockStatement(block) => match block.body.as_slice() {
                    [Statement::ExpressionStatement(inner)] => &inner.expression,
                    _ => return None,
                },
                _ => return None,
            };
            let Expression::AssignmentExpression(assignment) = assignment else {
                return None;
            };
            if assignment.operator != AssignmentOperator::Assign {
                return None;
            }
            let target = assignment_target_path(&assignment.left)?;
            (target == guarded).then_some(IdempotentStatement::Guarded { target })
        }
        _ => None,
    }
}

/// What a reference binds to, in the three states that matter when deciding whether a callee is
/// the framework's own.
///
/// Collapsing the last two was a real defect: `None` meant both "free reference" and "bound to
/// something that is not an import", so a file-local `const vi = createFakeVi()` was accepted as
/// Vitest's own. Shared by [`is_framework_reset_call`] and [`collect_module_setup`] rather than
/// written twice — the production duplication ceiling is 0.00%, and a second copy of a security-
/// relevant binding check is how the two drift apart.
enum Binding {
    /// Binds to nothing: the ambient `vi` of a `globals: true` run.
    Free,
    /// Binds to an import, with its anchored module identity.
    Imported(String),
    /// Binds to something declared in this file. Never Vitest's.
    Local,
}

/// Classify an identifier as unbound, locally declared or imported with its anchored module
/// identity.
fn binding_of(
    ident: &IdentifierReference,
    semantic: &Semantic,
    import_specifiers: &HashMap<SymbolId, Arc<str>>,
) -> Binding {
    let Some(symbol) = ident
        .reference_id
        .get()
        .and_then(|reference| semantic.scoping().get_reference(reference).symbol_id())
    else {
        return Binding::Free;
    };
    match import_specifiers.get(&symbol) {
        Some(module) => Binding::Imported(module.to_string()),
        None => Binding::Local,
    }
}

/// Whether `ident` is Vitest's own namespace of the given name (`vi` or `expect`).
///
/// A free reference qualifies because that is exactly what `globals: true` produces; an import
/// qualifies only when it really comes from the `vitest` package; a file-local binding never does.
/// Which name is compared depends on the binding, and conflating the two was a review finding.
/// A FREE reference has only its own name — that is the ambient `vi` of a `globals: true` run, so
/// the local name is the evidence. An IMPORTED one must be compared on the name it was exported
/// under: `import { vi as v } from 'vitest'` is Vitest's `vi` however the file spells it, and
/// rejecting it on the local name `v` let the C1 false positive back in for aliased imports.
/// Measured before the fix, and the same gap existed in `is_framework_reset_call`, which this now
/// closes too.
fn is_vitest_namespace(
    ident: &IdentifierReference,
    name: &str,
    semantic: &Semantic,
    import_specifiers: &HashMap<SymbolId, Arc<str>>,
    exported_as: &HashMap<SymbolId, String>,
) -> bool {
    match binding_of(ident, semantic, import_specifiers) {
        Binding::Free => ident.name.as_str() == name,
        Binding::Imported(module) => {
            if !import_is_package(&module, "vitest") {
                return false;
            }
            let exported = ident
                .reference_id
                .get()
                .and_then(|reference| semantic.scoping().get_reference(reference).symbol_id())
                .and_then(|symbol| exported_as.get(&symbol));
            // No recorded export name means a namespace import, which this check does not model.
            exported.is_some_and(|exported| exported == name)
        }
        Binding::Local => false,
    }
}

/// Whether `call` is one of the framework reset operations, resolved through the import graph.
///
/// Never by bare name. `not-redundant-hook-guarded-connect.test.ts` exists to forbid an
/// implementation that keys idempotence on what a callee is *called*, and a project's own
/// `cleanup()` is not Testing Library's.
fn is_framework_reset_call(
    call: &CallExpression,
    semantic: &Semantic,
    import_specifiers: &HashMap<SymbolId, Arc<str>>,
    exported_as: &HashMap<SymbolId, String>,
) -> bool {
    /// `vi`'s reset family: each replaces a whole registry's state, so running it twice leaves
    /// what running it once left.
    const VI_RESETS: [&str; 6] = [
        "clearAllMocks",
        "resetAllMocks",
        "restoreAllMocks",
        "resetModules",
        "unstubAllEnvs",
        "unstubAllGlobals",
    ];

    match &call.callee {
        // `cleanup()` -- and only Testing Library's. A project's own function of that name
        // resolves elsewhere and does not qualify, which is the constraint
        // `redundant-hook-guarded-connect.test.ts` exists to enforce. An ambient `cleanup` is NOT
        // accepted: unlike `vi`, Testing Library exports no global, so a free `cleanup` is some
        // other file's.
        Expression::Identifier(ident) => match binding_of(ident, semantic, import_specifiers) {
            Binding::Imported(module) => import_is_package(&module, "@testing-library/"),
            Binding::Free | Binding::Local => false,
        },
        // `vi.clearAllMocks()` and family.
        Expression::StaticMemberExpression(member) => {
            let Expression::Identifier(object) = &member.object else {
                return false;
            };
            VI_RESETS.contains(&member.property.name.as_str())
                && is_vitest_namespace(object, "vi", semantic, import_specifiers, exported_as)
        }
        _ => false,
    }
}

/// Whether an anchored module identity names `package` (or, for a scope like
/// `@testing-library/`, any package under it).
///
/// Written against the two shapes [`crate::frontend::fingerprint::specifier_identity`] actually
/// produces, which is why a bare `module == "vitest"` comparison would match neither: an
/// unresolvable bare specifier becomes `unresolved:vitest`, and a resolvable one becomes
/// `resolved:<canonical path>`. A substring test over the whole identity was the other half of
/// the review finding -- `resolved:/repo/test/vitest-utils.ts` contains "vitest" and is not the
/// package.
fn import_is_package(module: &str, package: &str) -> bool {
    let scope = package.ends_with('/');
    if let Some(specifier) = module.strip_prefix("unresolved:") {
        return if scope {
            specifier.starts_with(package)
        } else {
            specifier == package || specifier.starts_with(&format!("{package}/"))
        };
    }
    if let Some(path) = module.strip_prefix("resolved:") {
        let marker = format!("/node_modules/{package}");
        return if scope {
            path.contains(&marker)
        } else {
            path.contains(&format!("{marker}/"))
        };
    }
    false
}

/// Keep all row cells observable for named rows or unreadable, rest, defaulted or destructuring
/// callback bindings.
fn row_cells_fully_observable(body_arg: Option<&Argument>, row: &Row) -> bool {
    // An object row binds whole. Even a callback that destructures only some members holds a
    // binding through which the rest are reachable, so no member is provably unread.
    if matches!(row.cells, RowCells::Named(_)) {
        return true;
    }
    let Some(function_like) = body_arg.and_then(as_function_like) else {
        // No readable callback: nothing is provably dropped.
        return true;
    };
    let params = function_like.params();
    // A rest element receives every cell past the fixed parameters, which is exactly the
    // `vitest` `utils-display.spec.ts` shape.
    params.rest.is_some()
        // A destructuring or defaulted parameter is not a plain positional binding this function
        // claims to understand; refuse rather than assume it drops anything. Same two refusals
        // `fingerprint.rs`'s `resolve_row_bindings` already makes before substituting at all.
        || params
            .items
            .iter()
            .any(|item| {
                item.initializer.is_some()
                    || !matches!(item.pattern, BindingPattern::BindingIdentifier(_))
            })
}

/// Build positional or named substitutions from a resolved table row, returning None for
/// unresolved rows.
fn row_substitution(row: &Row) -> Option<RowSubstitution> {
    if row.unresolved {
        return None;
    }
    let cell = |value: &Option<CellValue>| value.as_ref().map(|c| (c.kind, c.raw.clone()));
    Some(match &row.cells {
        RowCells::Positional(cells) => {
            RowSubstitution::Positional(cells.iter().map(cell).collect())
        }
        RowCells::Named(fields) => RowSubstitution::Named(
            fields
                .iter()
                .filter_map(|(name, value)| cell(value).map(|resolved| (name.clone(), resolved)))
                .collect(),
        ),
    })
}

/// Renders one `.each`/`.for` row's title, falling back to the raw pattern (with its
/// placeholders intact) when any referenced value is not statically known.
fn render_each_title(pattern: &TitlePattern, row: &Row, index: usize) -> Title {
    // An unresolved pattern is never rendered, whatever the row holds: substituting values into
    // source text produces a string no reader was ever shown. `Parameterized` is the honest
    // answer, and `emit_duplicate_each_row` refuses to group on it.
    if !pattern.resolved {
        return Title::Parameterized(pattern.text.clone());
    }
    match try_render_each_title(&pattern.text, row, index) {
        Some(rendered) => Title::Static(rendered),
        None => Title::Parameterized(pattern.text.clone()),
    }
}

/// Render supported printf/named row-title placeholders, declining unresolved substitutions
/// and unsupported rendering cases such as %% or a NaN numeric coercion.
fn try_render_each_title(pattern: &str, row: &Row, index: usize) -> Option<String> {
    // The printf arguments come from the row itself, not from its cells: an object row is ONE
    // argument (`{ a: 1 }`), and an array row spreads even where its binding does not. See
    // `Row::title_args`.
    let positional: Vec<Option<&TitleArg>> = row.title_args.iter().map(Option::as_ref).collect();
    let named = |name: &str| -> Option<&str> {
        match &row.cells {
            RowCells::Named(fields) => fields
                .iter()
                .find(|(key, _)| key == name)
                .and_then(|(_, value)| value.as_ref().map(|cell| cell.display.as_str())),
            RowCells::Positional(_) => None,
        }
    };

    let chars: Vec<char> = pattern.chars().collect();
    let mut output = String::new();
    let mut positional_cursor = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '%' && i + 1 < chars.len() {
            match chars[i + 1] {
                // NOT an escape in Vitest, whatever the documentation implies. `%%` emits a
                // percent AND consumes the next argument, printing it after a space, so every
                // directive after it shifts by one. Measured on 5.0.1 and re-measured on 5.0.3,
                // where `'%% x %s y %s z %s'` over `['A','B','C']` renders
                // `% A x B y C z undefined` and `'x %s y %% z %s'` renders `x A y % B z C`.
                // `'only %s'` over the same row renders `only A`, which rules out the competing
                // explanation that leftover arguments are being appended: they are not, `%%` is
                // behaving as a directive.
                //
                // Refusing to render is not squeamishness about mirroring a defect. This scan
                // never executes the project's configuration, so the installed Vitest version is
                // not a fact available to it -- and the two behaviours disagree about which rows
                // collide. Reproducing today's would silently give a wrong answer to anyone on a
                // version where this is fixed. Keeping the pattern un-rendered says "this title is
                // not modelled", which stays true under both. See `docs/vitest-surface.md` for the
                // measured table.
                '%' => return None,
                '#' => {
                    output.push_str(&index.to_string());
                    i += 2;
                    continue;
                }
                '$' => {
                    output.push_str(&(index + 1).to_string());
                    i += 2;
                    continue;
                }
                // Each directive consumes the next argument, and they do NOT all coerce it the
                // same way. Measured against Vitest 5.0.1 with
                // `['a', 1.5, null, true, [2], { k: 1 }]`: `%d` is `Number` (`NaN` for `'a'`),
                // `%i` is `parseInt` (`1` for `1.5`), `%f` is `parseFloat`, `%j` is
                // `JSON.stringify`, `%o` is the inspect form, and `%s` is string coercion.
                directive @ ('s' | 'd' | 'i' | 'f' | 'j' | 'o') => {
                    // Two different absences, and merging them was a defect the directive
                    // test caught: an index PAST the end of the argument list is a real
                    // `undefined` (an empty array row spreads to nothing, and Vitest prints
                    // `undefined` for it), while an argument that exists but could not be READ
                    // is a value this extractor does not know. The first renders; the second
                    // must degrade to the un-rendered pattern, or `test.each([[mk()]])` would
                    // report a title claiming the cell was `undefined`.
                    let argument = match positional.get(positional_cursor) {
                        None => None,
                        Some(None) => return None,
                        Some(Some(argument)) => Some(argument),
                    };
                    positional_cursor += 1;
                    match directive {
                        // Running out of arguments is not a failure: `%s` on an exhausted list
                        // prints `undefined`, which is what an empty array row does (it spreads
                        // to nothing). Refusing here would leave that row unresolved over a
                        // title Vitest renders perfectly well.
                        's' => output.push_str(argument.map_or("undefined", |arg| &arg.coerced)),
                        'o' => output.push_str(argument.map_or("undefined", |arg| &arg.inspected)),
                        'j' => match argument {
                            // `JSON.stringify` of a BigInt throws, so there is no answer to give.
                            Some(arg) => output.push_str(arg.json.as_deref()?),
                            None => output.push_str("undefined"),
                        },
                        _ => {
                            // `%d`/`%i`/`%f`, which disagree with each other on the same value
                            // (see `TitleArg::number` and `parse_numeric_prefix`). A coercion
                            // that yields `NaN` is REFUSED here, deliberately: `NaN` is a title
                            // that does not discriminate between rows -- 18 of
                            // `duplicate-each-row`'s 41 false positives on the 2026-09-27 scan
                            // were rows sharing one non-discriminating title -- so rendering it
                            // would trade a recall gap for a precision one. The refusal keeps
                            // the whole pattern unrendered, which opts the declaration out of
                            // title comparison rather than comparing it on a wrong key.
                            let number =
                                numeric_directive(argument.map(|argument| &**argument), directive)?;
                            output.push_str(&number);
                        }
                    }
                    i += 2;
                    continue;
                }
                _ => {} // unrecognized directive: copy the '%' literally below
            }
        }
        if c == '$' {
            let start = i + 1;
            let field_len = chars[start..]
                .iter()
                .take_while(|c| c.is_alphanumeric() || **c == '_' || **c == '.')
                .count();
            if field_len > 0 {
                let field: String = chars[start..start + field_len].iter().collect();
                let value = if let Ok(positional_index) = field.parse::<usize>() {
                    positional
                        .get(positional_index)
                        .copied()
                        .flatten()?
                        .field
                        .as_str()
                } else if field.contains('.') {
                    // One level of dotted-path support is documented (`$a.val`), but resolving it
                    // requires descending into a nested object literal, which this extractor does
                    // not evaluate -- falling back to `Parameterized` is the honest outcome rather
                    // than guessing.
                    return None;
                } else {
                    named(&field)?
                };
                output.push_str(value);
                i = start + field_len;
                continue;
            }
        }
        output.push(c);
        i += 1;
    }
    Some(output)
}

/// One numeric directive over one printf argument, or the refusal that keeps the pattern
/// unrendered.
///
/// `%d` is `Number(value)` -- read from [`TitleArg::number`], which was computed from the value
/// and not the text, because the two disagree on exactly the coercions worth recovering
/// (`null`→0, `true`→1, `[]`→0). `%i`/`%f` are `parseInt`/`parseFloat` over the value's *string*
/// form, modelled here as a prefix parse over the `%s` rendering: the two differ wherever the
/// directives disagree anyway (an object's inspect form, a BigInt's `n` suffix), so the parse is
/// faithful where it is answerable and refused where it is not.
///
/// `NaN` is never rendered -- see the call site for why that is a decision and not a gap.
fn numeric_directive(argument: Option<&TitleArg>, directive: char) -> Option<String> {
    let argument = argument?;
    match directive {
        // `%d` is `Number(value).toString()`; `String(Number(-0))` is `"0"`, and `ryu-js` spells
        // ECMAScript, so the sign drops here.
        'd' => match argument.number.as_ref()? {
            // A BigInt keeps its own spelling under `%d` (measured: `10n`), which is why it is
            // carried on the argument rather than folded to `Number(10n)`'s `10`.
            TitleNumber::BigInt(text) => Some(text.clone()),
            TitleNumber::Number(value) => Some(js_number_to_string(*value)),
        },
        // `%i` is `parseInt(String(value))` -- a prefix parse over the string form, not
        // `Number`: a prefixed string parses here and refuses under `%d`, and `1.5` truncates.
        // The BigInt exception is carried on the argument (measured: `%i` prints `10n`).
        'i' => match argument.number.as_ref() {
            Some(TitleNumber::BigInt(text)) => Some(text.clone()),
            _ => parse_numeric_prefix(&argument.coerced, false).map(js_number_to_string),
        },
        // `%f` has no BigInt case in Vitest (`parseFloat(String(10n))` is `10`), so the prefix
        // parse over the `%s` rendering is the whole story. Vitest special-cases `%f` over `-0`
        // (its own `formatTitle` rewrites the directive to `-%f` for a negative-zero item), so
        // the sign is restored here and nowhere else.
        'f' => {
            let value = parse_numeric_prefix(&argument.coerced, true)?;
            Some(if value == 0.0 && value.is_sign_negative() {
                "-0".to_owned()
            } else {
                js_number_to_string(value)
            })
        }
        _ => unreachable!("the directive match admits only d/i/f here"),
    }
}

/// `parseInt` (`float == false`) or `parseFloat` (`float == true`) over an already-rendered
/// string: trim JavaScript whitespace, take an optional sign, then the longest prefix those
/// functions accept -- decimal digits only for `parseInt` (a `.` stops it, so `'1.5'` is `1`),
/// digits with a fraction and exponent for `parseFloat`, `Infinity` for `parseFloat` only, and
/// the `0x`/`0X` hex prefix for `parseInt` only. `None` when no numeric prefix exists, which is
/// the `NaN` the caller refuses.
fn parse_numeric_prefix(rendered: &str, float: bool) -> Option<f64> {
    // The trim set is ECMAScript's exactly (shared with `number_of_string`; Copilot review,
    // PR #28, corrected in round 3): the one Rust-whitespace character `parseInt` does NOT
    // trim is U+0085, and every other difference runs the other way -- the Zs separators are
    // JavaScript whitespace too.
    let trimmed = rendered.trim_matches(is_ecmascript_trim_whitespace);
    let bytes = trimmed.as_bytes();
    let mut index = match bytes.first() {
        Some(b'+' | b'-') => 1,
        _ => 0,
    };
    // The sign applies to every parse below, radix prefixes included (`parseInt('-0x1F')` is
    // `-31`; Copilot review, PR #28).
    let sign: f64 = if bytes.first() == Some(&b'-') {
        -1.0
    } else {
        1.0
    };
    // `parseFloat` reads the `Infinity` spelling; `parseInt` does not.
    if float && trimmed[index..].starts_with("Infinity") {
        return Some(if bytes.first() == Some(&b'-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        });
    }
    // `parseInt` recognizes ONLY the 0x/0X hex prefix (Copilot review, PR #28 round 2, verified
    // in V8 against the previous round's claim: `parseInt('0b11')` is `0` and `parseInt('0o17')`
    // is `0` -- only `Number` reads the binary/octal prefixes, on the `%d` path, via
    // `split_radix_prefix`). Only the leading hex digits count, so `parseInt('0x12g')` is `18`
    // and a prefix this parse does not read (`'0b11'`) falls through to the leading decimal
    // digits, which is where its `0` comes from.
    if !float {
        if let Some((16, digits)) = split_radix_prefix(&trimmed[index..]) {
            let end = digits
                .bytes()
                .take_while(|&byte| byte.is_ascii_hexdigit())
                .count();

            // `parseInt` reads the hex prefix's digits as an exact mathematical value and
            // rounds once to Number, so arbitrary length is fine -- the shared converter
            // implements exactly that (round 5 removed the i128 bound here too). A bare
            // prefix (`'0x'`, `'0xg'`, `'0x-1'` -- all NaN, verified in node, NOT the
            // prefix's leading 0) reaches the converter as an empty digit run and refuses
            // there; the decimal fallback below never sees a prefix-bearing string.
            return js_radix_digits_value(&digits[..end], 16).map(|value| sign * value);
        }
    }
    let mantissa_start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    let mut saw_digits = index > mantissa_start;
    if float && index < bytes.len() && bytes[index] == b'.' {
        let fraction_start = index + 1;
        let mut end = fraction_start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        // `'.5'` and `'1.'` are both valid mantissas; a bare `.` with no digits anywhere is not.
        if saw_digits || end > fraction_start {
            saw_digits = true;
            index = end;
        }
    }
    if !saw_digits {
        return None;
    }
    if float && index < bytes.len() && (bytes[index] == b'e' || bytes[index] == b'E') {
        let exponent_start = index + 1;
        let mut end = match bytes.get(exponent_start) {
            Some(b'+' | b'-') => exponent_start + 1,
            _ => exponent_start,
        };
        let digits_start = end;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        // An `e` not followed by digits is not part of the number: `'1e'` parses as `1`.
        if end > digits_start {
            index = end;
        }
    }
    trimmed[..index].parse::<f64>().ok()
}

/// Recognize the static import.meta.vitest member expression used by in-source tests.
fn is_import_meta_vitest_expr(expr: &Expression) -> bool {
    matches!(
        expr,
        Expression::StaticMemberExpression(member)
            if matches!(&member.object, Expression::ImportMeta(_)) && member.property.name.as_str() == "vitest"
    )
}

struct LineIndex<'a> {
    source: &'a str,
    line_starts: Vec<u32>,
}

impl<'a> LineIndex<'a> {
    /// Build a byte-offset line index with the start of every source line.
    fn new(source: &'a str) -> Self {
        let mut line_starts = vec![0u32];
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push((index + 1) as u32);
            }
        }
        LineIndex {
            source,
            line_starts,
        }
    }

    /// 1-indexed line/UTF-16-column [`Position`] for a byte offset, matching editor and SARIF
    /// conventions per `crate::model::Position`'s contract.
    fn position(&self, byte_offset: u32) -> Position {
        let line_index = match self.line_starts.binary_search(&byte_offset) {
            Ok(exact) => exact,
            Err(insert_at) => insert_at.saturating_sub(1),
        };
        let line_start = self.line_starts[line_index] as usize;
        let column = self
            .source
            .get(line_start..byte_offset as usize)
            .unwrap_or_default()
            .encode_utf16()
            .count() as u32;
        Position {
            line: (line_index + 1) as u32,
            column: column + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::SourceLanguage;
    use std::path::PathBuf;

    /// Build a source-file fixture with language and module flavor inferred from its extension.
    fn file(path: &str) -> SourceFile {
        let language = if path.ends_with(".ts") || path.ends_with(".mts") || path.ends_with(".cts")
        {
            SourceLanguage::TypeScript
        } else {
            SourceLanguage::JavaScript
        };
        let module_flavor = if path.ends_with(".mjs") || path.ends_with(".mts") {
            ModuleFlavor::ForcedEsm
        } else if path.ends_with(".cjs") || path.ends_with(".cts") {
            ModuleFlavor::ForcedCjs
        } else {
            ModuleFlavor::Ambiguous
        };
        SourceFile {
            path: PathBuf::from(path),
            language,
            module_flavor,
            config_scope: None,
            project_globals: None,
        }
    }

    /// Module-scope ambient state reaches `Context`, and test-declaring helper calls still do not.
    ///
    /// C1. `module_setup` admitted only AWAITED module-scope calls, so a synchronous one was
    /// invisible and two files differing *only* in it compared equal — `cross-file-duplicate`
    /// reported four such pairs as "under an equivalent context", including one where exactly one
    /// of the two tests passes (`expect.extend` defining a matcher inclusively in one file and
    /// exclusively in the other).
    ///
    /// The rule keys on the CALLEE, not on being a call: a member call on a resolved `vi` or
    /// `expect` namespace changes ambient behaviour, anything else is ordinary program text. The
    /// second assertion below is why it cannot simply be "every module-scope call" —
    /// `Context::module_setup`'s own doc rules that out, because since S11 a module-scope helper
    /// call can declare tests and folding it in would key those tests on their own declaration.
    #[test]
    fn module_scope_ambient_state_is_context_but_a_test_declaring_helper_is_not() {
        let setup_of = |source: &str| {
            extract_source(source, "a.test.ts", Globals::Disabled)
                .tests
                .first()
                .map(|test| test.context.module_setup.clone())
                .unwrap_or_default()
        };
        let body = "\ntest('t', () => { expect(1).toBe(1); });";

        // The four APIs that were reported, plus the fifth nobody filed.
        for call in [
            "vi.useFakeTimers()",
            "vi.stubEnv('MODE', 'production')",
            "vi.stubGlobal('fetch', f)",
            "expect.extend({ toBeWithin: m })",
        ] {
            assert_eq!(
                setup_of(&format!(
                    "import {{ test, expect, vi }} from 'vitest';\n{call};{body}"
                )),
                vec![call.to_owned()],
                "{call} changes what every test in the file does"
            );
        }

        // Chained: `vi.spyOn(...)` returns a mock and the statement's callee is therefore a CALL,
        // not an identifier. A first version stopped at the first member expression and missed
        // exactly this — the case with the widest blast radius, since the uncaptured part is a
        // VALUE. Measured against the release binary before the walk was added.
        assert_eq!(
            setup_of(&format!(
                "import {{ test, expect, vi }} from 'vitest';\nimport * as flags from './flags';\nvi.spyOn(flags, 'isEnabled').mockReturnValue(true);{body}"
            )),
            vec!["vi.spyOn(flags, 'isEnabled').mockReturnValue(true)".to_owned()],
            "the spied VALUE is the whole difference between two otherwise identical tests"
        );

        // S11: a module-scope helper that DECLARES tests is not ambient setup.
        assert_eq!(
            setup_of(
                "import { test, expect } from 'vitest';\nimport { declareShared } from './shared';\ndeclareShared();\ntest('t', () => { expect(1).toBe(1); });"
            ),
            Vec::<String>::new(),
            "folding a test-declaring helper into the context key would key those tests on their \
             own declaration"
        );

        // A file-local `vi` is not Vitest's, the same discriminator S5 established for hooks.
        assert_eq!(
            setup_of(&format!(
                "import {{ test, expect }} from 'vitest';\nconst vi = createFakeVi();\nvi.useFakeTimers();{body}"
            )),
            Vec::<String>::new(),
            "deciding this by the name `vi` is the heuristic the binding check exists to avoid"
        );

        // `vi.mock` is already `Context::mocked_modules`; admitting it twice would move every
        // existing contextKey that has one without changing what the key distinguishes.
        assert_eq!(
            setup_of(&format!(
                "import {{ test, expect, vi }} from 'vitest';\nvi.mock('./db');{body}"
            )),
            Vec::<String>::new(),
            "already modelled, and double-counting it is a silent digest change"
        );

        // An ALIASED import is still Vitest's. Comparing the LOCAL name rejected it, so
        // `v.useFakeTimers()` never reached the context and the false positive came back for any
        // file spelling the import that way. Found in review, reproduced against the binary.
        assert_eq!(
            setup_of(&format!(
                "import {{ test, expect, vi as v }} from 'vitest';\nv.useFakeTimers();{body}"
            )),
            vec!["v.useFakeTimers()".to_owned()],
            "`import {{ vi as v }}` is Vitest's `vi` however the file spells it"
        );
        // ...and the local-shadow rejection survives the change: a file-local `v` is not Vitest's
        // merely because something exported under that name exists elsewhere.
        assert_eq!(
            setup_of(&format!(
                "import {{ test, expect }} from 'vitest';\nconst v = createFakeVi();\nv.useFakeTimers();{body}"
            )),
            Vec::<String>::new(),
            "a local binding is never the framework's, whatever it is called"
        );

        // WHITESPACE INSIDE A LITERAL IS THE VALUE. The entry used to be whitespace-collapsed, so
        // these two stubbed different values and produced one key -- the same
        // `cross-file-duplicate` false positive this whole function exists to prevent, arriving
        // through the normalizer. Measured before the fix. Same class as the `config_test_scope`
        // normalizer that was deleted for it.
        assert_ne!(
            setup_of(&format!(
                "import {{ test, expect, vi }} from 'vitest';\nvi.stubEnv('G', 'a  b');{body}"
            )),
            setup_of(&format!(
                "import {{ test, expect, vi }} from 'vitest';\nvi.stubEnv('G', 'a b');{body}"
            )),
            "'a  b' and 'a b' are different environment values and must not share a context key"
        );
    }

    /// Every shape `idempotent_statement` recognizes, and the ones it refuses.
    ///
    /// These branches were covered only by `fixtures/duplicates/redundant-hook-*`, which run the
    /// release binary out of process — so `cargo llvm-cov` saw roughly ninety contiguous lines of
    /// `extract.rs` as uncovered and the module sat 1.84pp under its floor. The behaviour was
    /// tested; the instrument could not see it. That is a real gap either way: an out-of-process
    /// fixture proves the end-to-end verdict but cannot say which arm produced it, so a refactor
    /// that collapsed two arms into one would keep every fixture green.
    ///
    /// Asserted through `context.hooks[].idempotent`, the only observable this classification has.
    #[test]
    fn hook_idempotence_recognizes_each_shape_and_refuses_the_rest() {
        let idempotent = |body: &str| extracted_hook(body).idempotent;

        // Overwrite: a plain assignment of a primitive runs to the same value every time.
        assert!(idempotent("flag = 5;"), "assigning a literal is idempotent");
        assert!(idempotent("flag = 'x';"));
        assert!(idempotent("flag = true;"));
        assert!(idempotent("config.ready = 1;"), "a member target too");

        // Guarded: `??=` and `||=` only assign while the target is nullish/falsy.
        assert!(idempotent("flag ??= makeThing();"));
        assert!(idempotent("flag ||= makeThing();"));

        // Guarded by an explicit `if (!X) X = E`, in both statement and block form.
        assert!(idempotent("if (!flag) flag = makeThing();"));
        assert!(idempotent("if (!flag) { flag = makeThing(); }"));

        // A reset call, which is what the S5 discriminator governs.
        assert!(idempotent("vi.clearAllMocks();"));

        // ...and the refusals, each for its own reason.
        assert!(
            !idempotent("flag = makeThing();"),
            "a non-literal right-hand side can differ between runs"
        );
        assert!(
            !idempotent("flag += 1;"),
            "a compound assignment accumulates, which is the opposite of idempotent"
        );
        assert!(
            !idempotent("if (!flag) { flag = 1; other = 2; }"),
            "a guard around TWO statements is not the recognized shape"
        );
        assert!(
            !idempotent("if (!flag) flag = 1; else flag = 2;"),
            "an `else` branch means the second run can take a different path"
        );
        assert!(
            !idempotent("if (!flag) other = 1;"),
            "the guard must test the very binding it assigns, or it re-fires"
        );
        assert!(
            !idempotent("if (flag) flag = 1;"),
            "the guard must be a negation; this one assigns only when already set"
        );
        assert!(
            !idempotent("send();"),
            "an ordinary call is not a framework reset"
        );

        // The target path is built by walking members, so a nested one exercises the recursion and
        // a COMPUTED one must refuse: `a[k]` names a different property per run of the key
        // expression, so "the guard tests the binding it assigns" cannot be established from syntax.
        assert!(
            idempotent("if (!config.db.pool) config.db.pool = makeThing();"),
            "a nested member guard is still the recognized shape"
        );
        assert!(
            !idempotent("config[key] = 1;"),
            "a computed target is not a path this classification can compare"
        );
        assert!(
            !idempotent("if (!config[key]) config[key] = 1;"),
            "and neither is a computed guard, even though the two sides look identical in source"
        );
        assert!(
            !idempotent("[flag] = [1];"),
            "a destructuring target is not a simple path either"
        );
    }

    /// `import_is_package` on both identity forms, including the one a review already caught.
    ///
    /// Reached through the binding checks today, which only ever hand it `unresolved:` identities in
    /// tests -- the `resolved:` branch needs a real `node_modules` path on disk, so it had no
    /// coverage at all. It is a pure function over two strings, so asking it directly is both
    /// cheaper and sharper than building a filesystem to reach it.
    ///
    /// Sharper matters here: this is the discrimination that keeps a project's own helper from
    /// passing as the framework. A substring test over the whole identity was the original defect --
    /// `resolved:/repo/test/vitest-utils.ts` contains "vitest" and is not the package -- and that is
    /// the first case below.
    #[test]
    fn import_is_package_distinguishes_a_package_from_a_path_that_merely_contains_its_name() {
        // The review finding, in both identity forms.
        assert!(!import_is_package(
            "resolved:/repo/test/vitest-utils.ts",
            "vitest"
        ));
        assert!(!import_is_package("unresolved:./vitest-utils", "vitest"));

        // A real dependency, resolved into node_modules.
        assert!(import_is_package(
            "resolved:/repo/node_modules/vitest/dist/index.js",
            "vitest"
        ));
        // ...and a package whose name merely starts with it is still not it.
        assert!(!import_is_package(
            "resolved:/repo/node_modules/vitest-utils/index.js",
            "vitest"
        ));

        // A bare specifier that never resolved is still recognizable as the package.
        assert!(import_is_package("unresolved:vitest", "vitest"));
        assert!(import_is_package("unresolved:vitest/config", "vitest"));
        assert!(!import_is_package("unresolved:vitest-utils", "vitest"));

        // A SCOPE, where any package underneath it counts -- the `@testing-library/` form.
        assert!(import_is_package(
            "resolved:/repo/node_modules/@testing-library/react/dist/index.js",
            "@testing-library/"
        ));
        assert!(import_is_package(
            "unresolved:@testing-library/react",
            "@testing-library/"
        ));
        assert!(
            !import_is_package("unresolved:@testing-librarian/react", "@testing-library/"),
            "a scope is matched as a prefix including its slash, so a longer scope name is not it"
        );
    }

    /// S5's discriminator: a file's OWN `vi` is not Vitest's.
    ///
    /// `is_framework_reset_call` resolves the callee through the import graph precisely so that
    /// idempotence is never decided by what something is *called*. A first version collapsed
    /// "binds to nothing" and "binds to a non-import" into one `None`, so a file-local
    /// `const vi = createFakeVi()` was accepted as Vitest's own -- reproduced against the release
    /// binary, which then reported `redundant-hook` on a pair whose repetition may be the
    /// behaviour, the exact defect S5 exists to stop. Found in review, not by this suite.
    ///
    /// Three bindings, three answers, all asserted.
    #[test]
    fn only_vitests_own_vi_makes_a_hook_body_idempotent() {
        let idempotence_of = |source: &str| {
            extract_source(source, "a.test.ts", Globals::Disabled)
                .tests
                .iter()
                .flat_map(|test| test.context.hooks.iter().map(|hook| hook.idempotent))
                .collect::<Vec<_>>()
        };
        let body = "describe('s', () => { beforeEach(() => { vi.clearAllMocks(); }); it('t', () => { expect(1).toBe(1); }); });";

        assert_eq!(
            idempotence_of(&format!(
                "import {{ describe, it, expect, beforeEach, vi }} from 'vitest';\n{body}"
            )),
            vec![true],
            "Vitest's own `vi` is the case this vocabulary is for"
        );
        assert_eq!(
            idempotence_of(&format!(
                "import {{ describe, it, expect, beforeEach }} from 'vitest';\nconst vi = createFakeVi();\n{body}"
            )),
            vec![false],
            "a file-local `vi` is not Vitest's, and deciding otherwise is the name heuristic this \
             function exists to avoid"
        );
        // A substring test over the module identity accepted `resolved:/repo/test/vitest-utils.ts`.
        assert_eq!(
            idempotence_of(&format!(
                "import {{ describe, it, expect, beforeEach }} from 'vitest';\nimport {{ vi }} from './vitest-utils';\n{body}"
            )),
            vec![false],
            "`./vitest-utils` is not the `vitest` package"
        );
    }

    /// The other half of S6 mechanism 2: what `extract` does with the scope discovery attached.
    ///
    /// Reverting the fail-closed arm changed NO test at all, which is why this exists. A nested
    /// config that exists and cannot be read decides how these tests run, and not knowing how is
    /// the definition of an unresolved context -- treating it as "no nested config" would compare
    /// the file as if the analysis root governed it alone.
    #[test]
    fn an_unreadable_nested_config_marks_the_context_unresolved() {
        let with_scope = |scope: Option<Result<String, String>>| {
            let mut source_file = file("pkg/a.test.ts");
            source_file.config_scope = scope;
            let extraction = extract(
                "import { test, expect } from 'vitest';\ntest('t', () => { expect(1).toBe(1); });",
                &source_file,
                Globals::Disabled,
            );
            let case = extraction.tests.into_iter().next().expect("one test");
            (case.context.unresolved, case.context.config_scope)
        };

        // Readable: the scope travels into the context and the context stays comparable.
        let (unresolved, scope) = with_scope(Some(Ok("{ environment: 'node' }".to_owned())));
        assert!(!unresolved, "a readable nested config resolves");
        assert_eq!(scope.as_deref(), Some("{ environment: 'node' }"));

        // Absent: unchanged from before this field existed.
        let (unresolved, scope) = with_scope(None);
        assert!(!unresolved);
        assert_eq!(
            scope, None,
            "no nested config leaves the context as the root governs it"
        );

        // Present and unreadable: fail closed.
        let (unresolved, scope) = with_scope(Some(Err("unterminated string".to_owned())));
        assert!(
            unresolved,
            "a config that exists and cannot be read makes the context unsafe to compare"
        );
        assert_eq!(scope, None, "and contributes no scope to compare ON");
    }

    /// Extract the supplied source using a path-derived fixture file and the requested globals
    /// mode.
    fn extract_source(source: &str, path: &str, globals: Globals) -> FileExtraction {
        extract(source, &file(path), globals)
    }

    /// A test body's serialized fingerprint text must not survive into the IR.
    ///
    /// This is the property the whole `Fingerprints` -> `BodyDigest` reduction exists for, and
    /// the only one an in-repo test can observe portably: no allocator hook, no RSS reading, just
    /// "is the text still in there". The marker is a *string literal*, which `alpha` preserves
    /// verbatim (`structural` collapses it to its kind, so only `alpha` carries it) -- so if
    /// `Fingerprints::alpha` ever holds text again, the marker reappears in the serialized
    /// `TestCase` and this fails. It also fails if a future field starts retaining any other
    /// serialized view of the body.
    ///
    /// The second half is what keeps the first from being vacuous. `Hook::alpha_source` retains
    /// its text **on purpose** (see its doc: `contextKey` publishes a hash of it), so the same
    /// marker in a hook body *is* found -- which proves the assertion above can observe a
    /// retained marker at all, rather than passing because the marker was misspelled, never
    /// reached `alpha`, or was dropped by the serializer.
    #[test]
    fn a_test_bodys_serialized_text_is_not_retained_but_a_hooks_is() {
        const MARKER: &str = "zq7-retention-marker";

        let in_test_body = format!("test('t', () => {{ expect(compute()).toBe('{MARKER}'); }});");
        let extraction = extract_source(&in_test_body, "retention.test.ts", Globals::Enabled);
        assert_eq!(extraction.tests.len(), 1);
        let serialized = serde_json::to_string(&extraction.tests[0]).expect("TestCase serializes");
        assert!(
            !serialized.contains(MARKER),
            "a test body's alpha text must not be retained, but the IR still carries it: \
             {serialized}"
        );

        let in_hook_body = format!(
            "beforeEach(() => {{ setup('{MARKER}'); }}); \
             test('t', () => {{ expect(compute()).toBe(1); }});"
        );
        let extraction = extract_source(&in_hook_body, "retention.test.ts", Globals::Enabled);
        assert_eq!(extraction.tests.len(), 1);
        let serialized = serde_json::to_string(&extraction.tests[0]).expect("TestCase serializes");
        assert!(
            serialized.contains(MARKER),
            "control: a hook's alpha_source is retained on purpose, so this marker must be \
             findable -- if it is not, the assertion above proves nothing: {serialized}"
        );
    }

    /// Check declaration extraction: test case table row class round trips and defaults when
    /// absent.
    #[test]
    fn test_case_table_row_class_round_trips_and_defaults_when_absent() {
        let extraction = extract_source(
            "test.each([1])('%s', (value) => { expect(value).toBe(1); });",
            "row.test.ts",
            Globals::Enabled,
        );
        let case = &extraction.tests[0];
        assert_eq!(case.table_row_class, Some(0));

        let encoded = serde_json::to_string(case).expect("TestCase serializes");
        let decoded: TestCase = serde_json::from_str(&encoded).expect("TestCase round-trips");
        assert_eq!(decoded, *case);

        let without_class = encoded.replace(",\"table_row_class\":0", "");
        assert_ne!(
            without_class, encoded,
            "the serialized field must be removed"
        );
        let decoded: TestCase = serde_json::from_str(&without_class)
            .expect("a TestCase written before the field existed still deserializes");
        assert_eq!(decoded.table_row_class, None);
    }

    /// The text a hook or a `vi.mock` factory *does* retain must be stored once per declaration,
    /// not once per test.
    ///
    /// `effective_hooks` attaches every enclosing hook to every test under it and
    /// `mocked_modules` is copied into every test in the file, so an owned `String` in either
    /// place is duplicated by the test count -- the single largest remaining retention on a
    /// 24k-test corpus before these became `Arc<str>`. Pointer equality is the only way to state
    /// "shared, not copied": a round-trip through `String` anywhere on the path (the exact
    /// regression this guards -- it compiles, it passes every value-equality assertion, and it
    /// silently restores the duplication) leaves the values equal and the pointers different.
    #[test]
    fn hook_and_mock_text_is_shared_across_the_tests_that_carry_it() {
        let source = "vi.mock('./dep', () => ({ value: 1 }));\
                      beforeEach(() => { setup(); });\
                      test('one', () => { expect(a()).toBe(1); });\
                      test('two', () => { expect(b()).toBe(2); });";
        let extraction = extract_source(source, "shared.test.ts", Globals::Enabled);
        assert_eq!(extraction.tests.len(), 2);
        let (first, second) = (&extraction.tests[0], &extraction.tests[1]);

        assert_eq!(
            first.context.hooks.len(),
            1,
            "one beforeEach reaches both tests"
        );
        assert!(
            std::sync::Arc::ptr_eq(
                &first.context.hooks[0].alpha_source,
                &second.context.hooks[0].alpha_source
            ),
            "one hook declaration must contribute one allocation, not one per test"
        );

        assert_eq!(first.context.mocked_modules.len(), 1);
        assert!(
            std::sync::Arc::ptr_eq(
                &first.context.mocked_modules[0].specifier,
                &second.context.mocked_modules[0].specifier
            ),
            "one vi.mock specifier must contribute one allocation, not one per test"
        );
        match (
            &first.context.mocked_modules[0].factory,
            &second.context.mocked_modules[0].factory,
        ) {
            (MockFactory::Factory(left), MockFactory::Factory(right)) => assert!(
                std::sync::Arc::ptr_eq(left, right),
                "one vi.mock factory must contribute one allocation, not one per test"
            ),
            (left, right) => panic!("expected an inline factory on both, got {left:?}/{right:?}"),
        }
    }

    // HAZARD 1: `bench` changed from a top-level import (v2-v4) to a `TestContext` fixture (v5).
    // Both shapes appear in real corpora and must not crash or double-count.
    /// Check declaration extraction: hazard1 legacy top level bench is a benchmark declaration.
    #[test]
    fn hazard1_legacy_top_level_bench_is_a_benchmark_declaration() {
        let source = include_str!("../../fixtures/declarations/bench-legacy-toplevel.bench.ts");
        let extraction =
            extract_source(source, "bench-legacy-toplevel.bench.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert_eq!(extraction.tests[0].kind, DeclarationKind::Benchmark);
        assert!(extraction.tests[0].modifiers.contains(&Modifier::Skip));
    }

    /// Check declaration extraction: hazard1 v5 fixture style bench is not a separate
    /// declaration.
    #[test]
    fn hazard1_v5_fixture_style_bench_is_not_a_separate_declaration() {
        let source = include_str!("../../fixtures/declarations/bench-v5-fixture-style.test.ts");
        let extraction =
            extract_source(source, "bench-v5-fixture-style.test.ts", Globals::Disabled);
        // Exactly the one enclosing `test`; the destructured `bench` fixture used inside its body
        // is a parameter, not an import, and must not resolve as a declaration of its own.
        assert_eq!(extraction.tests.len(), 1);
        assert_eq!(extraction.tests[0].kind, DeclarationKind::Test);
    }

    // HAZARD 2: `.sequential` was removed in v5 but must still surface as a real modifier for
    // pre-v5 corpora, not be dropped or folded into `Unknown`.
    /// Check declaration extraction: hazard2 legacy sequential modifier is recognized.
    #[test]
    fn hazard2_legacy_sequential_modifier_is_recognized() {
        let source = include_str!("../../fixtures/modifiers/modifier-sequential-legacy.test.ts");
        let extraction = extract_source(
            source,
            "modifier-sequential-legacy.test.ts",
            Globals::Disabled,
        );
        assert_eq!(extraction.tests.len(), 1);
        assert!(extraction.tests[0]
            .modifiers
            .contains(&Modifier::Sequential));
    }

    // HAZARD 3: globals resolution is per file (standing in for "per project"), not a single
    // repo-wide boolean -- the same source extracts differently depending only on the flag.
    /// Check declaration extraction: hazard3 globals flag gates ambient bindings.
    #[test]
    fn hazard3_globals_flag_gates_ambient_bindings() {
        let source = include_str!("../../fixtures/resolution/globals-ambient.test.ts");
        let disabled = extract_source(source, "globals-ambient.test.ts", Globals::Disabled);
        assert_eq!(
            disabled.tests.len(),
            0,
            "no import and no globals flag: nothing resolves"
        );
        let enabled = extract_source(source, "globals-ambient.test.ts", Globals::Enabled);
        // One `describe` (Suite) + one nested `test` (Test).
        assert_eq!(enabled.tests.len(), 2);
    }

    // HAZARD 4: look-alikes share call shape with Vitest but must never be extracted, because
    // import-specifier resolution (not name matching) is the only sound signal.
    /// Check declaration extraction: hazard4 lookalikes are never extracted.
    #[test]
    fn hazard4_lookalikes_are_never_extracted() {
        let cases: &[(&str, &str)] = &[
            (
                include_str!("../../fixtures/lookalikes/lookalike-playwright.spec.ts"),
                "lookalike-playwright.spec.ts",
            ),
            (
                include_str!("../../fixtures/lookalikes/lookalike-jest-globals.test.ts"),
                "lookalike-jest-globals.test.ts",
            ),
            (
                include_str!("../../fixtures/lookalikes/lookalike-cypress.cy.ts"),
                "lookalike-cypress.cy.ts",
            ),
            (
                include_str!("../../fixtures/lookalikes/lookalike-node-test.test.mjs"),
                "lookalike-node-test.test.mjs",
            ),
            (
                include_str!("../../fixtures/lookalikes/lookalike-mocha-before-after.test.ts"),
                "lookalike-mocha-before-after.test.ts",
            ),
        ];
        for (source, path) in cases {
            let extraction = extract_source(source, path, Globals::Disabled);
            assert_eq!(
                extraction.tests.len(),
                0,
                "{path} must not be extracted as Vitest"
            );
            assert!(
                extraction.diagnostics.is_empty(),
                "{path} names a competing runner explicitly or has no imported helper call; it \
                 must not be reported as a Vitest helper the analyzer failed to read: {:?}",
                extraction.diagnostics
            );
            assert!(
                !extraction.incomplete,
                "rejecting another runner is a complete classification, not partial Vitest \
                 extraction: {path}"
            );
        }
    }

    // Sharper version of hazard 4: an import from a *specific* competing framework must dominate
    // even when the caller (incorrectly, or for an unrelated project) passes `Globals::Enabled`.
    /// Check declaration extraction: hazard4 competing import dominates over globals flag.
    #[test]
    fn hazard4_competing_import_dominates_over_globals_flag() {
        let source = include_str!("../../fixtures/lookalikes/lookalike-playwright.spec.ts");
        let extraction = extract_source(source, "lookalike-playwright.spec.ts", Globals::Enabled);
        assert_eq!(extraction.tests.len(), 0);
    }

    // D2: `hazard4_lookalikes_are_never_extracted` only proves the Cypress/Mocha cases are
    // rejected under `Globals::Disabled`, where *nothing* bare-identifier resolves -- so it would
    // pass even if the exclusion logic were deleted outright. Both fixtures are pure ambient
    // `describe`/`it` with no import to anchor resolution to, unlike Playwright's own sharp test
    // above. This is a genuine, reported limitation, not a bug fixed here: statically, a
    // `globals: true` project's ambient `describe(string, fn)`/`it(string, fn)` is indistinguishable
    // from a `.cy.ts`/Mocha spec using the exact same shape, and this frontend has no signal (file
    // extension, `cy.*`/`before`/`after` calls are not consulted) that would let it tell them
    // apart. Proving this is genuinely undecidable statically, not just unimplemented, would
    // require the caller to feed in path-based project routing (e.g. Cypress's own
    // `cypress.config.*` `specPattern`) the same way `Globals` itself is already threaded in from
    // outside this frontend.
    /// Check declaration extraction: hazard4 cypress ambient shape resolves as vitest under
    /// globals enabled.
    #[test]
    fn hazard4_cypress_ambient_shape_resolves_as_vitest_under_globals_enabled() {
        let source = include_str!("../../fixtures/lookalikes/lookalike-cypress.cy.ts");
        let extraction = extract_source(source, "lookalike-cypress.cy.ts", Globals::Enabled);
        assert_eq!(
            extraction.tests.len(),
            2,
            "ambient describe/it has no import to anchor resolution to, so it resolves as \
             Vitest under globals:true exactly like hazard3's ambient fixture -- a genuine \
             static-analysis limitation, not a regression"
        );
    }

    /// Check declaration extraction: hazard4 mocha ambient shape resolves as vitest under
    /// globals enabled.
    #[test]
    fn hazard4_mocha_ambient_shape_resolves_as_vitest_under_globals_enabled() {
        let source = include_str!("../../fixtures/lookalikes/lookalike-mocha-before-after.test.ts");
        let extraction = extract_source(
            source,
            "lookalike-mocha-before-after.test.ts",
            Globals::Enabled,
        );
        assert_eq!(
            extraction.tests.len(),
            2,
            "ambient describe/it resolves as Vitest under globals:true; Mocha's before/after are \
             not Vitest hook names and are simply ignored, not mistaken for beforeAll/afterAll"
        );
    }

    // HAZARD 5: a non-literal `.each` table must degrade to "unresolved", not zero tests.
    /// Check declaration extraction: hazard5 nonliteral each table yields one unresolved case.
    #[test]
    fn hazard5_nonliteral_each_table_yields_one_unresolved_case() {
        let source = include_str!("../../fixtures/each/each-nonliteral-table.test.ts");
        let extraction = extract_source(source, "each-nonliteral-table.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        let each = extraction.tests[0]
            .modifiers
            .iter()
            .find_map(|m| {
                if let Modifier::Each(table) = m {
                    Some(table)
                } else {
                    None
                }
            })
            .expect("each modifier present");
        assert!(matches!(each.rows, EachRows::Unreadable(_)));
        assert!(matches!(extraction.tests[0].title, Title::Parameterized(_)));
        // C1: an unresolvable `.each` table is exactly the kind of ambient fact that must not
        // let this test compare as if fully known.
        assert!(extraction.tests[0].context.unresolved);
    }

    // HAZARD 6: title rendering is version-dependent, so a literal table renders concrete titles
    // only when every referenced value is provably static; otherwise the extractor must not
    // fabricate a rendering and falls back to the raw, unrendered pattern.
    /// Check declaration extraction: hazard6 literal each table renders static titles per row.
    #[test]
    fn hazard6_literal_each_table_renders_static_titles_per_row() {
        let source = include_str!("../../fixtures/each/each-array-of-arrays.test.ts");
        let extraction = extract_source(source, "each-array-of-arrays.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 3);
        assert_eq!(
            extraction.tests[0].title,
            Title::Static("add(1, 1) = 2".to_owned())
        );
        assert_eq!(
            extraction.tests[1].title,
            Title::Static("add(1, 2) = 3".to_owned())
        );
        assert_eq!(
            extraction.tests[2].title,
            Title::Static("add(2, 1) = 3".to_owned())
        );
    }

    /// Check declaration extraction: hazard6 directives exceeding row values fall back to
    /// parameterized.
    #[test]
    fn hazard6_directives_exceeding_row_values_fall_back_to_parameterized() {
        let source = include_str!("../../fixtures/each/each-printf-directives.test.ts");
        let extraction =
            extract_source(source, "each-printf-directives.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert!(
            matches!(extraction.tests[0].title, Title::Parameterized(_)),
            "more directives than row values must not be guessed at: {:?}",
            extraction.tests[0].title
        );
        // F5 regression: this row's own cells are all perfectly literal (`row.unresolved` is
        // `false` here) -- the *renderer* is what gives up, not the row. Before F5, only
        // `Title::Dynamic` set `context.unresolved`, so this case (and the dotted-path
        // `.each` case below) would render a shared, non-row-specific `Parameterized(pattern)`
        // title and still compare as fully resolved -- letting two declarations of the same
        // pattern with different, non-foldable row data collide at Error severity.
        assert!(
            extraction.tests[0].context.unresolved,
            "a title that fell back to the raw, unrendered pattern must never look fully known"
        );
    }

    // F5: a dotted-path `.each` placeholder (`$a.val`) is not resolved by the title renderer even
    // when the referenced cell itself is a perfectly literal, fully-known value -- the renderer's
    // own documented limitation, not row's `unresolved` flag, is what forces the fallback. Before
    // F5 this case's `context.unresolved` was `false`: only `Title::Dynamic` tainted it.
    /// Check declaration extraction: f5 dotted path each placeholder marks context unresolved.
    #[test]
    fn f5_dotted_path_each_placeholder_marks_context_unresolved() {
        let source = include_str!("../../fixtures/each/each-dollar-indexed.test.ts");
        let extraction = extract_source(source, "each-dollar-indexed.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 2);
        for test in &extraction.tests {
            assert!(
                matches!(test.title, Title::Parameterized(_)),
                "a dotted-path placeholder must fall back to the raw pattern, not fabricate a \
                 rendering: {:?}",
                test.title
            );
            assert!(
                test.context.unresolved,
                "a title the renderer could not fully resolve must never be treated as fully \
                 known, even though every row cell involved was itself fully literal"
            );
        }
    }

    // HAZARD 7: an options-object modifier must union with (not replace) chain-based modifiers,
    // and a bare-function 2nd argument must not be mistaken for an options object.
    /// Check declaration extraction: hazard7 options object modifier is recognized.
    #[test]
    fn hazard7_options_object_modifier_is_recognized() {
        let source = include_str!("../../fixtures/declarations/options-object-modifiers.test.ts");
        let extraction = extract_source(
            source,
            "options-object-modifiers.test.ts",
            Globals::Disabled,
        );
        assert_eq!(extraction.tests.len(), 1);
        assert!(extraction.tests[0].modifiers.contains(&Modifier::Skip));
        assert!(extraction.tests[0].has_body);
    }

    /// Check declaration extraction: hazard7 options object 2nd arg does not break body
    /// detection.
    #[test]
    fn hazard7_options_object_2nd_arg_does_not_break_body_detection() {
        let source = include_str!("../../fixtures/declarations/options-object-2nd-arg.test.ts");
        let extraction =
            extract_source(source, "options-object-2nd-arg.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert!(extraction.tests[0].has_body);
    }

    // HAZARD 8: a fixture name alone is ambiguous with a built-in `TestContext` member, and two
    // `test.extend` bindings can reuse a fixture name for unrelated setup -- so a fixture must
    // carry a fingerprint of its own setup function, not just its name, cross-referenced against
    // the specific `.extend` chain that produced the local `test` binding (via `registry.rs`'s
    // binding resolution, not name matching).
    /// Check declaration extraction: hazard8 extend fixture is tracked by name and setup
    /// fingerprint.
    #[test]
    fn hazard8_extend_fixture_is_tracked_by_name_and_setup_fingerprint() {
        let source = include_str!("../../fixtures/fixtures/fixtures-basic-extend.test.ts");
        let extraction = extract_source(source, "fixtures-basic-extend.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert_eq!(
            extraction.tests[0].title,
            Title::Static("uses db fixture".to_owned())
        );
        assert_eq!(extraction.tests[0].context.fixtures.len(), 1);
        assert_eq!(extraction.tests[0].context.fixtures[0].name, "db");
        assert!(matches!(
            extraction.tests[0].context.fixtures[0].body,
            crate::model::FixtureValue::Setup(_)
        ));
        assert!(!extraction.tests[0].context.unresolved);
    }

    /// Check declaration extraction: hazard8 extend dependency chain unions fixtures from every
    /// link.
    #[test]
    fn hazard8_extend_dependency_chain_unions_fixtures_from_every_link() {
        let source = include_str!("../../fixtures/fixtures/fixtures-dependency-chain.test.ts");
        let extraction = extract_source(
            source,
            "fixtures-dependency-chain.test.ts",
            Globals::Disabled,
        );
        assert_eq!(extraction.tests.len(), 1);
        let names: Vec<&str> = extraction.tests[0]
            .context
            .fixtures
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, vec!["a", "b"]);
        assert!(!extraction.tests[0].context.unresolved);
    }

    // A fixture with a computed key defeats static naming and must flag the context as
    // unresolved rather than silently drop the fixture or guess at its identity.
    /// Check declaration extraction: hazard8 computed fixture key marks context unresolved.
    #[test]
    fn hazard8_computed_fixture_key_marks_context_unresolved() {
        let source = r#"
            import { test as baseTest } from "vitest";
            const key = "db";
            const test = baseTest.extend({ [key]: async ({}, use) => { await use(1); } });
            test("uses computed fixture", ({ db }) => {});
        "#;
        let extraction = extract_source(source, "computed-fixture.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert!(extraction.tests[0].context.unresolved);
    }

    // HAZARD 9: in-source tests bind off `import.meta.vitest`, not an import specifier or globals.
    /// Check declaration extraction: hazard9 in source test is extracted via import meta
    /// vitest.
    #[test]
    fn hazard9_in_source_test_is_extracted_via_import_meta_vitest() {
        let source = include_str!("../../fixtures/insource/in-source-basic.ts");
        let extraction = extract_source(source, "in-source-basic.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert_eq!(
            extraction.tests[0].title,
            Title::Static("add works".to_owned())
        );
    }

    // HAZARD 10: a local variable can shadow `test`/`describe`/`it` without being Vitest's --
    // binding resolution, not name matching, must be what decides this.
    /// Check declaration extraction: hazard10 shadowed local is not misidentified.
    #[test]
    fn hazard10_shadowed_local_is_not_misidentified() {
        let source = include_str!(
            "../../fixtures/resolution/mixed-file-vitest-and-helper-named-test.test.ts"
        );
        let extraction = extract_source(
            source,
            "mixed-file-vitest-and-helper-named-test.test.ts",
            Globals::Disabled,
        );
        assert_eq!(
            extraction.tests.len(),
            1,
            "only the real `t(...)` call is a declaration"
        );
        assert_eq!(
            extraction.tests[0].title,
            Title::Static("uses vitest import not shadowed test".to_owned())
        );
    }

    // Resolution forms beyond aliasing/shadowing: namespace import.
    /// Check declaration extraction: namespace import is extracted.
    #[test]
    fn namespace_import_is_extracted() {
        let source = include_str!("../../fixtures/resolution/imports-namespace.test.ts");
        let extraction = extract_source(source, "imports-namespace.test.ts", Globals::Disabled);
        // One `describe` (Suite) + one nested `test` (Test).
        assert_eq!(extraction.tests.len(), 2);
    }

    // ---------------------------------------------------------------------------------------
    // `Context::imports` anchors specifiers to a resolved module identity instead of storing raw
    // specifier text.
    // ---------------------------------------------------------------------------------------

    /// Builds a real file on disk and extracts it, so [`super::fingerprint::specifier_identity`]
    /// -- which resolves through the real filesystem via `oxc_resolver` -- has something to
    /// actually resolve. `extract_source`'s fake path is not enough for these tests: they exist
    /// specifically to exercise the resolved (not just the lexically-anchored-unresolved) case.
    fn extract_at(dir: &std::path::Path, relative_path: &str, source: &str) -> FileExtraction {
        let path = dir.join(relative_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, source).unwrap();
        extract(source, &file(&path.to_string_lossy()), Globals::Disabled)
    }

    /// An import identity is one allocation per file, shared by every test that references it,
    /// and a test's import list carries no slack capacity. Both are what `--debug-memory`'s
    /// `imports` bucket measures: a `String` cloned into each test, or a vector left at its
    /// per-reference capacity after dedup, retained an order of magnitude more per test.
    #[test]
    fn import_identities_are_shared_across_a_files_tests_and_carry_no_slack() {
        let source = r#"
            import { test, expect } from 'vitest';
            import { compute } from './helpers';
            test('a', () => {
                expect(compute(1)).toBe(1);
                expect(compute(2)).toBe(2);
                expect(compute(3)).toBe(3);
            });
            test('b', () => { expect(compute(4)).toBe(4); });
        "#;
        let extraction = extract_source(source, "shared.test.ts", Globals::Disabled);
        let [first, second] = &extraction.tests[..] else {
            panic!("two tests extracted, got {}", extraction.tests.len());
        };
        let (a, b) = (&first.context.imports, &second.context.imports);
        assert_eq!(a.len(), 2, "two modules referenced: {a:?}");
        assert_eq!(a.len(), b.len(), "both tests reference the same modules");
        assert!(
            a.iter().zip(b).all(|(l, r)| Arc::ptr_eq(l, r)),
            "one allocation per identity, shared by both tests: {a:?} vs {b:?}"
        );
        assert_eq!(
            a.capacity(),
            a.len(),
            "six references, two modules, no slack"
        );
    }

    /// Two declarations of one module share one identity allocation: `a` and `b` arrive through
    /// separate `import` statements, and each test references only one of them.
    #[test]
    fn repeated_imports_of_one_module_share_one_identity_allocation() {
        let source = "import { test, expect } from 'vitest';\nimport { a } from './h';\nimport { b } from './h';\n\
            test('x', () => { expect(a).toBe(1); });\ntest('y', () => { expect(b).toBe(2); });\n";
        let extraction = extract_source(source, "twice.test.ts", Globals::Disabled);
        let [x, y] = &extraction.tests[..] else {
            panic!("two tests extracted, got {}", extraction.tests.len());
        };
        // `expect` also pulls in the `vitest` identity, so pick the module under test by name.
        let helper = |test: &TestCase| {
            let found = test.context.imports.iter().find(|i| i.ends_with(":h"));
            Arc::clone(found.expect("`./h` is referenced"))
        };
        let (from_x, from_y) = (helper(x), helper(y));
        assert!(
            Arc::ptr_eq(&from_x, &from_y),
            "one allocation across declarations: {from_x}"
        );
    }

    /// Assert exactly one extracted test and borrow the anchored imports retained in its
    /// context.
    fn only_test_imports(extraction: &FileExtraction) -> &[Arc<str>] {
        assert_eq!(extraction.tests.len(), 1, "expected exactly one test case");
        &extraction.tests[0].context.imports
    }

    /// The same relative specifier text (`./helpers`), written from two different directories,
    /// each with their own `helpers.ts` -- two genuinely different modules. Before this was fixed,
    /// `Context::imports` stored the raw specifier text, so these read as the same import; now
    /// each is anchored to its own resolved file and must differ.
    #[test]
    fn same_named_specifiers_resolving_to_different_files_produce_different_contexts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::create_dir_all(dir.path().join("b")).unwrap();
        std::fs::write(dir.path().join("a/helpers.ts"), "export const value = 1;").unwrap();
        std::fs::write(dir.path().join("b/helpers.ts"), "export const value = 2;").unwrap();
        let source = r#"
            import { test, expect } from 'vitest';
            import { value } from './helpers';
            test('uses value', () => { expect(value).toBeDefined(); });
        "#;
        let a = extract_at(dir.path(), "a/a.test.ts", source);
        let b = extract_at(dir.path(), "b/b.test.ts", source);
        let imports_a = only_test_imports(&a);
        let imports_b = only_test_imports(&b);
        assert!(
            !imports_a.is_empty(),
            "the import must be resolved and recorded"
        );
        assert_ne!(
            imports_a, imports_b,
            "the same specifier text naming two different files must produce different contexts"
        );
    }

    /// Two different spellings (`./helpers` vs `../shared/helpers`) that resolve to the exact
    /// same file on disk must anchor to the same identity, and therefore produce the same
    /// `Context::imports` -- real resolution, not lexical string matching, is what proves this.
    #[test]
    fn same_file_via_different_spellings_produces_the_same_context() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("shared")).unwrap();
        std::fs::create_dir_all(dir.path().join("nested")).unwrap();
        std::fs::write(
            dir.path().join("shared/helpers.ts"),
            "export const value = 1;",
        )
        .unwrap();
        let direct_source = r#"
            import { test, expect } from 'vitest';
            import { value } from './shared/helpers';
            test('uses value', () => { expect(value).toBeDefined(); });
        "#;
        let roundabout_source = r#"
            import { test, expect } from 'vitest';
            import { value } from '../shared/helpers';
            test('uses value', () => { expect(value).toBeDefined(); });
        "#;
        let direct = extract_at(dir.path(), "direct.test.ts", direct_source);
        let roundabout = extract_at(dir.path(), "nested/roundabout.test.ts", roundabout_source);
        assert_eq!(
            only_test_imports(&direct),
            only_test_imports(&roundabout),
            "two spellings naming the same file on disk must produce the same context"
        );
    }

    /// A specifier that cannot be resolved to a real file must stay distinguishable from every
    /// resolved specifier -- never silently degrading to a raw-text identity that could collide
    /// with one.
    #[test]
    fn unresolvable_specifier_stays_distinguishable_from_a_resolved_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("helpers.ts"), "export const value = 1;").unwrap();
        let resolved_source = r#"
            import { test, expect } from 'vitest';
            import { value } from './helpers';
            test('uses value', () => { expect(value).toBeDefined(); });
        "#;
        let unresolved_source = r#"
            import { test, expect } from 'vitest';
            import { value } from './does-not-exist';
            test('uses value', () => { expect(value).toBeDefined(); });
        "#;
        let resolved = extract_at(dir.path(), "resolved.test.ts", resolved_source);
        let unresolved = extract_at(dir.path(), "unresolved.test.ts", unresolved_source);
        let resolved_imports = only_test_imports(&resolved);
        let unresolved_imports = only_test_imports(&unresolved);
        assert_ne!(
            resolved_imports, unresolved_imports,
            "an unresolvable specifier must never coincide with a resolved one"
        );
        assert!(
            resolved_imports[0].starts_with("resolved:"),
            "a real file must anchor to a `resolved:` identity, got {resolved_imports:?}"
        );
        assert!(
            unresolved_imports[0].starts_with("unresolved:"),
            "a missing file must anchor to an `unresolved:` identity, got {unresolved_imports:?}"
        );
    }

    /// Imported-helper inlining's context half: when every reference to a module's symbol in the
    /// body is a call whose helper was inlined into the fingerprint, the module's identity drops
    /// out of `Context::imports` -- its content is compared directly now, and keeping the
    /// identity would block exactly the cross-module recognition inlining exists to make.
    /// `vitest` itself stays: `test(...)` is a framework call, never inlined.
    /// Writes the one helper module both imported-helper context tests extract against.
    fn write_check_helper(dir: &std::path::Path) {
        std::fs::write(
            dir.join("helpers.ts"),
            "import { expect } from 'vitest';\n\
             export function check(label: string) { expect(label.length).toBeGreaterThan(0); }",
        )
        .unwrap();
    }

    /// The context half of imported-helper inlining: every reference to the helper module in
    /// the body is a call the alpha pass inlined, so the module's identity drops out of
    /// `Context::imports` -- its content is compared directly, and keeping the identity would
    /// block exactly the cross-module recognition inlining exists to make.
    #[test]
    fn fully_inlined_helper_module_drops_out_of_context_imports() {
        let dir = tempfile::tempdir().unwrap();
        write_check_helper(dir.path());
        let source = r#"
            import { test } from 'vitest';
            import { check } from './helpers';
            test('accepts a plain label', () => { check('dashboard'); });
        "#;
        let extraction = extract_at(dir.path(), "twin.test.ts", source);
        assert_eq!(
            only_test_imports(&extraction),
            &[] as &[Arc<str>],
            "the inlined helper module must drop out of the compared imports entirely (the \
             `test(...)` call sits outside the body, so the framework import is not in this \
             body's list to begin with)"
        );
    }

    /// The other half of the drop rule: a module referenced as a *value* (`check` passed to
    /// `expect`, not called) keeps its identity, because that reference keeps the
    /// `free:import` identity in `alpha` -- dropping it would let the context say "equivalent"
    /// for a pair whose bodies still name different modules.
    #[test]
    fn helper_module_referenced_as_a_value_stays_in_context_imports() {
        let dir = tempfile::tempdir().unwrap();
        write_check_helper(dir.path());
        let source = r#"
            import { test, expect } from 'vitest';
            import { check } from './helpers';
            test('names its helper', () => { expect(check).toBeDefined(); });
        "#;
        let extraction = extract_at(dir.path(), "value.test.ts", source);
        let imports = only_test_imports(&extraction);
        assert!(
            imports.iter().any(|entry| entry.starts_with("resolved:")),
            "a module referenced outside an inlined call must keep its identity, got {imports:?}"
        );
    }

    /// Review-found false-merge shape: a helper call whose ONLY reference sits inside a class
    /// body. The serializer renders class bodies as opaque raw text -- the call neither inlines
    /// nor renders an identity -- while a reference-counting visitor still sees the call. When
    /// the drop was re-derived beside the renderer, such a module was dropped as "fully
    /// inlined", and two files calling same-named helpers from different modules inside class
    /// bodies could compare under equal contexts. The drop now comes from the alpha pass's own
    /// accounting, so a module the pass never rendered can never be dropped.
    #[test]
    fn a_module_referenced_only_inside_an_opaque_class_body_is_never_dropped() {
        let dir = tempfile::tempdir().unwrap();
        write_check_helper(dir.path());
        let source = r#"
            import { test, expect } from 'vitest';
            import { check } from './helpers';
            test('runs a step', () => {
                class Runner { step() { check('dashboard'); } }
                expect(new Runner().step()).toBeDefined();
            });
        "#;
        let extraction = extract_at(dir.path(), "class.test.ts", source);
        let imports = only_test_imports(&extraction);
        assert!(
            imports.iter().any(|entry| entry.contains("helpers")),
            "a module whose only reference lives where the serializer renders opaque text was \
             never actually inlined and must keep its identity, got {imports:?}"
        );
    }

    /// The `vi.mock` interplay, pinned (see `helpers.rs`'s module doc for the analysis): the
    /// mocked helper module participates in the context through `mocked_modules` whether the
    /// call inlines or not, so a mocked and an unmocked caller of the same helper never share a
    /// context -- inlining did not open a door here.
    #[test]
    fn a_mocked_helper_module_is_separated_by_the_context_not_by_inlining() {
        let dir = tempfile::tempdir().unwrap();
        write_check_helper(dir.path());
        let mocked = r#"
            import { test, expect, vi } from 'vitest';
            import { check } from './helpers';
            vi.mock('./helpers');
            test('accepts a plain label', () => { check('dashboard'); });
        "#;
        let unmocked = r#"
            import { test, expect } from 'vitest';
            import { check } from './helpers';
            test('accepts a plain label', () => { check('dashboard'); });
        "#;
        let mocked_context = extract_at(dir.path(), "mocked.test.ts", mocked)
            .tests
            .first()
            .expect("one test")
            .context
            .clone();
        let unmocked_context = extract_at(dir.path(), "plain.test.ts", unmocked)
            .tests
            .first()
            .expect("one test")
            .context
            .clone();
        assert_eq!(
            mocked_context.mocked_modules.len(),
            1,
            "the automocked helper module is recorded in the context"
        );
        assert!(
            unmocked_context.mocked_modules.is_empty(),
            "the unmocked file carries no mocked modules"
        );
        assert_ne!(
            mocked_context.mocked_modules, unmocked_context.mocked_modules,
            "a mocked and an unmocked caller of the same helper are separated by the context"
        );
    }

    /// Check declaration extraction: nested describe builds suite path outermost first.
    #[test]
    fn nested_describe_builds_suite_path_outermost_first() {
        let source = include_str!("../../fixtures/declarations/nested-describe.test.ts");
        let extraction = extract_source(source, "nested-describe.test.ts", Globals::Disabled);
        let deepest = extraction
            .tests
            .iter()
            .find(|t| t.title == Title::Static("arithmetic 1".to_owned()))
            .expect("deepest test present");
        assert_eq!(
            deepest.suite_path,
            vec![
                Title::Static("Utils".to_owned()),
                Title::Static("Math".to_owned()),
                Title::Static("Arithmetic".to_owned()),
            ]
        );
    }

    /// Check declaration extraction: nested hooks are ordered outer to inner then inner to
    /// outer.
    #[test]
    fn nested_hooks_are_ordered_outer_to_inner_then_inner_to_outer() {
        let source = include_str!("../../fixtures/hooks/hooks-nested-order.test.ts");
        let extraction = extract_source(source, "hooks-nested-order.test.ts", Globals::Disabled);
        let test = extraction
            .tests
            .iter()
            .find(|t| t.title == Title::Static("nested test".to_owned()))
            .expect("nested test present");
        let kinds: Vec<HookKind> = test.context.hooks.iter().map(|h| h.kind).collect();
        assert_eq!(
            kinds,
            vec![
                HookKind::BeforeAll,
                HookKind::BeforeEach,
                HookKind::BeforeEach,
                HookKind::AfterEach,
                HookKind::AfterEach,
                HookKind::AfterAll,
            ]
        );
    }

    /// Check declaration extraction: todo declarations have no body.
    #[test]
    fn todo_declarations_have_no_body() {
        let source = include_str!("../../fixtures/modifiers/modifier-todo.test.ts");
        let extraction = extract_source(source, "modifier-todo.test.ts", Globals::Disabled);
        assert!(extraction.tests.iter().all(|t| !t.has_body));
        assert!(extraction
            .tests
            .iter()
            .any(|t| t.modifiers.contains(&Modifier::Todo)));
        // A genuinely bodyless `.todo` declaration (no second argument at all) must not be
        // confused with C1's "opaque body" case (a body argument present but not recursed into):
        // guards against over-triggering the new unresolved logic.
        assert!(extraction.tests.iter().all(|t| !t.context.unresolved));
    }

    // A3: `vi.mock` with a non-literal specifier must not be silently dropped -- dropping it
    // would make a mocked test compare equal to an unmocked one with the same body.
    /// Check declaration extraction: a3 nonliteral vi mock specifier marks the file unresolved.
    #[test]
    fn a3_nonliteral_vi_mock_specifier_marks_the_file_unresolved() {
        let source = r#"
            import { test, vi } from "vitest";
            const SPEC = "./module-a";
            vi.mock(SPEC, () => ({ value: 1 }));
            test("uses module a", () => {});
        "#;
        let extraction = extract_source(source, "nonliteral-mock-spec.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert!(
            extraction.tests[0].context.mocked_modules.is_empty(),
            "a specifier this extractor cannot read must not be recorded as if it did not exist"
        );
        assert!(
            extraction.tests[0].context.unresolved,
            "a dropped vi.mock must never look like a clean, unmocked file"
        );
    }

    // A4: a factory this extractor cannot read (an imported/computed function, not an inline
    // arrow/function) must never be mapped onto `Automock` -- the two mean different behavior.
    /// Check declaration extraction: a4 non inline factory is unresolved not automock.
    #[test]
    fn a4_non_inline_factory_is_unresolved_not_automock() {
        let source = r#"
            import { test, vi } from "vitest";
            import { importedFactory } from "./factories";
            vi.mock("./m", importedFactory);
            test("uses m", () => {});
        "#;
        let extraction = extract_source(source, "imported-factory.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert_eq!(extraction.tests[0].context.mocked_modules.len(), 1);
        assert!(matches!(
            extraction.tests[0].context.mocked_modules[0].factory,
            MockFactory::Unresolved
        ));
        assert!(extraction.tests[0].context.unresolved);
    }

    // A4, other half: no second argument at all is a real automock and must stay comparable.
    /// Check declaration extraction: a4 missing factory argument is automock.
    #[test]
    fn a4_missing_factory_argument_is_automock() {
        let source = r#"
            import { test, vi } from "vitest";
            vi.mock("./m");
            test("uses m", () => {});
        "#;
        let extraction = extract_source(source, "automock.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert!(matches!(
            extraction.tests[0].context.mocked_modules[0].factory,
            MockFactory::Automock
        ));
        assert!(!extraction.tests[0].context.unresolved);
    }

    // F4: `vi.doMock` is not hoisted and is not file-wide like `vi.mock` -- it must never be
    // folded into the file-wide `mocked_modules` list (which would put one test's runtime-only
    // mock into every other test's `Context` and let tests with genuinely different module state
    // compare equal), and a file containing one must not look like a clean, fully-resolved file.
    /// Check declaration extraction: f4 do mock is not treated as file wide mock.
    #[test]
    fn f4_do_mock_is_not_treated_as_file_wide_mock() {
        let source = r#"
            import { test, vi } from "vitest";
            test("uses doMock", async () => {
                vi.doMock("./module-a", () => ({ value: 1 }));
                await import("./module-a");
            });
            test("does not use doMock", () => {});
        "#;
        let extraction = extract_source(source, "do-mock.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 2);
        assert!(
            extraction
                .tests
                .iter()
                .all(|t| t.context.mocked_modules.is_empty()),
            "a scoped `vi.doMock` must never be recorded as a file-wide `vi.mock`"
        );
        assert!(
            extraction.tests.iter().all(|t| t.context.unresolved),
            "a `vi.doMock` this scan cannot attribute to a single test must conservatively \
             taint the whole file, not just the test that called it"
        );
    }

    /// The `Context::mocked_modules` of the single test in `source`, as sortable strings, plus
    /// whether that test's context is unresolved. Every mock/unmock test below compares whole
    /// contexts rather than poking at one field, because the property that matters is "do these
    /// two files compare as the same context", not the shape of the intermediate value.
    fn mock_state(source: &str, name: &str) -> (Vec<String>, bool) {
        let extraction = extract_source(source, name, Globals::Disabled);
        assert_eq!(
            extraction.tests.len(),
            1,
            "these mock/unmock cases each declare exactly one test"
        );
        let test = &extraction.tests[0];
        let modules = test
            .context
            .mocked_modules
            .iter()
            .map(|module| format!("{}={:?}", module.specifier, module.factory))
            .collect();
        (modules, test.context.unresolved)
    }

    const MOCK_ONLY: &str = r#"
        import { test, vi } from "vitest";
        vi.mock("./m", () => ({ value: 1 }));
        test("reads the value", () => {});
    "#;

    // The defect this modelling exists to fix: `vi.mock(p, f); vi.unmock(p)` nets to "unmocked",
    // so it must not produce the same context as a file that leaves `p` mocked -- and it must not
    // be excluded from comparison either, which is what the previous conservative taint did.
    /// Check declaration extraction: mock then unmock is a different context from mock alone
    /// and stays comparable.
    #[test]
    fn mock_then_unmock_is_a_different_context_from_mock_alone_and_stays_comparable() {
        let source = r#"
            import { test, vi } from "vitest";
            vi.mock("./m", () => ({ value: 1 }));
            vi.unmock("./m");
            test("reads the value", () => {});
        "#;
        let (unmocked, unmocked_unresolved) = mock_state(source, "mock-then-unmock.test.ts");
        let (mocked, mocked_unresolved) = mock_state(MOCK_ONLY, "mock-then-unmock.test.ts");
        assert!(
            !unmocked_unresolved && !mocked_unresolved,
            "both files are fully statically readable; neither may be tainted out of comparison"
        );
        assert!(
            unmocked.is_empty(),
            "a specifier mocked and then unmocked is not in Vitest's registry, so it must not be \
             in the modelled mock set either: {unmocked:?}"
        );
        assert_ne!(
            unmocked, mocked,
            "mocking `./m` and mocking-then-unmocking it are different runtime contexts and must \
             not compare equal -- the false `duplicate-test` this models away"
        );
    }

    // The ordering, verified against a real Vitest 5 run rather than assumed: hoisted calls keep
    // their relative source order, so the LAST one for a specifier wins. `unmock` does not simply
    // dominate `mock`, which is the natural (and wrong) assumption.
    /// Check declaration extraction: unmock then mock is the same context as mock alone.
    #[test]
    fn unmock_then_mock_is_the_same_context_as_mock_alone() {
        let source = r#"
            import { test, vi } from "vitest";
            vi.unmock("./m");
            vi.mock("./m", () => ({ value: 1 }));
            test("reads the value", () => {});
        "#;
        let (reordered, reordered_unresolved) = mock_state(source, "unmock-then-mock.test.ts");
        let (mocked, mocked_unresolved) = mock_state(MOCK_ONLY, "unmock-then-mock.test.ts");
        assert!(!reordered_unresolved && !mocked_unresolved);
        assert_eq!(
            reordered, mocked,
            "the `vi.mock` comes last, so the module IS mocked -- a model in which `unmock` always \
             wins would wrongly report this file as unmocked"
        );
    }

    // An `unmock` of a specifier this file never mocked removes nothing, which is exactly what
    // Vitest does. Such a file must therefore be context-equivalent to one that does no mocking
    // at all -- not tainted, and not carrying a phantom entry.
    /// Check declaration extraction: unmock of a never mocked specifier matches a file with no
    /// mocking.
    #[test]
    fn unmock_of_a_never_mocked_specifier_matches_a_file_with_no_mocking() {
        let source = r#"
            import { test, vi } from "vitest";
            vi.unmock("./m");
            test("reads the value", () => {});
        "#;
        let plain = r#"
            import { test } from "vitest";
            test("reads the value", () => {});
        "#;
        let (unmock_only, unmock_unresolved) = mock_state(source, "unmock-only.test.ts");
        let (no_mocking, plain_unresolved) = mock_state(plain, "unmock-only.test.ts");
        assert!(
            !unmock_unresolved && !plain_unresolved,
            "a no-op `vi.unmock` resolves completely; tainting it would cost recall for nothing"
        );
        assert_eq!(unmock_only, no_mocking);
        assert!(unmock_only.is_empty());
    }

    // Two files that perform the same registrations in the same order really are the same
    // context, so the modelling must not make every unmocking file incomparable in a subtler way
    // (e.g. by recording the events rather than their net effect). Two *different* file names in
    // one directory, deliberately: the surviving specifier is anchored to the file it was written
    // in, so this also proves the anchoring still resolves both to the same module, and that the
    // fold's output order does not leak the hash map's iteration order.
    /// Check declaration extraction: two files mocking and unmocking alike share one context.
    #[test]
    fn two_files_mocking_and_unmocking_alike_share_one_context() {
        let preamble = r#"
            import { test, vi } from "vitest";
            vi.mock("./a", () => ({ value: 1 }));
            vi.mock("./b", () => ({ value: 2 }));
            vi.unmock("./a");
        "#;
        let (left, left_unresolved) = mock_state(
            &format!("{preamble}\ntest(\"reads the value\", () => {{}});"),
            "twin-a.test.ts",
        );
        let (right, right_unresolved) = mock_state(
            &format!("{preamble}\ntest(\"reads the value\", () => {{}});"),
            "twin-b.test.ts",
        );
        assert!(!left_unresolved && !right_unresolved);
        assert_eq!(
            left, right,
            "identical mock/unmock sequences in one directory are one context"
        );
        assert_eq!(
            left.len(),
            1,
            "only `./b` survives the fold; `./a` was unmocked after being mocked: {left:?}"
        );
    }

    // `vi.doUnmock` is the non-hoisted counterpart of `vi.unmock`, exactly as `vi.doMock` is of
    // `vi.mock`: it takes effect where it runs, for later dynamic imports only, so no file-wide
    // answer exists. It must keep tainting rather than being folded into the hoisted events.
    /// Check declaration extraction: do unmock still taints the file.
    #[test]
    fn do_unmock_still_taints_the_file() {
        let source = r#"
            import { test, vi } from "vitest";
            vi.mock("./m", () => ({ value: 1 }));
            test("reverts to the real module", async () => {
                vi.doUnmock("./m");
                await import("./m");
            });
        "#;
        let (modules, unresolved) = mock_state(source, "do-unmock.test.ts");
        assert!(
            unresolved,
            "a scoped, non-hoisted `vi.doUnmock` cannot be resolved file-wide and must exclude \
             the file from comparison"
        );
        assert_eq!(
            modules.len(),
            1,
            "the hoisted `vi.mock` is still read; `doUnmock` taints rather than cancelling it"
        );
    }

    // Hoisting and the last-wins fold are properties of module-top-level calls. Vitest 5 rejects
    // a hoisted member written anywhere else at transform time -- "1 call in <file> was defined
    // outside of the module's top level scope", verified by running this shape -- so there is no
    // runtime context here to model. The scan used to walk the whole program and treat anything
    // it found as file-wide regardless of nesting; this is the assertion that it no longer does.
    /// Check declaration extraction: a non top level unmock taints the file.
    #[test]
    fn a_non_top_level_unmock_taints_the_file() {
        let source = r#"
            import { test, beforeEach, vi } from "vitest";
            vi.mock("./m", () => ({ value: 1 }));
            beforeEach(() => {
                vi.unmock("./m");
            });
            test("reads the value", () => {});
        "#;
        let (modules, unresolved) = mock_state(source, "nested-unmock.test.ts");
        assert!(
            unresolved,
            "an unmocking call this scan cannot place in the hoisted order must taint the file, \
             not be silently folded in as if it were top level"
        );
        assert_eq!(
            modules.len(),
            1,
            "the top-level `vi.mock` is still read on its own terms"
        );
    }

    // The same restriction on the other member, and with the shape that looks most like an
    // exception: an `if` at top level. Vitest hoists the call straight out of the conditional, so
    // the branch is a lie about when it runs and Vitest 5 errors on it exactly as it does on a
    // call nested in a test. Asserted separately because a naive "is this statement in
    // `program.body`" check passes the `if` statement and would wrongly model the call inside it
    // -- as unconditionally mocked, which is what Vitest would do, but only on versions that
    // still run the file.
    /// Check declaration extraction: a conditional top level mock taints the file.
    #[test]
    fn a_conditional_top_level_mock_taints_the_file() {
        let source = r#"
            import { test, vi } from "vitest";
            if (process.env.CI) {
                vi.mock("./m", () => ({ value: 1 }));
            }
            test("reads the value", () => {});
        "#;
        let (modules, unresolved) = mock_state(source, "conditional-mock.test.ts");
        assert!(
            unresolved,
            "whether this file mocks `./m` depends on a runtime condition; no file-wide answer \
             exists and guessing one is how a mocked test compares equal to an unmocked one"
        );
        assert!(
            modules.is_empty(),
            "a conditional mock must not be recorded as if it were unconditional: {modules:?}"
        );
    }

    // A5: a fixture value this extractor cannot read (a call, an identifier, ...) must never
    // collapse into the same "plain literal" bucket as an actual literal -- `base.extend({ db:
    // makeRealDb() })` must not compare equal to `base.extend({ db: 42 })`.
    /// Check declaration extraction: a5 call expression fixture value is unresolved.
    #[test]
    fn a5_call_expression_fixture_value_is_unresolved() {
        let source = r#"
            import { test as baseTest } from "vitest";
            const test = baseTest.extend({ db: makeRealDb() });
            test("uses db", ({ db }) => {});
        "#;
        let extraction = extract_source(source, "fixture-call-value.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert!(matches!(
            extraction.tests[0].context.fixtures[0].body,
            crate::model::FixtureValue::Unresolved
        ));
        assert!(extraction.tests[0].context.unresolved);
    }

    /// Check declaration extraction: a5 literal and unresolved fixture values are distinct.
    #[test]
    fn a5_literal_and_unresolved_fixture_values_are_distinct() {
        let literal_source = r#"
            import { test as baseTest } from "vitest";
            const test = baseTest.extend({ db: 42 });
            test("uses db", ({ db }) => {});
        "#;
        let call_source = r#"
            import { test as baseTest } from "vitest";
            const test = baseTest.extend({ db: makeRealDb() });
            test("uses db", ({ db }) => {});
        "#;
        let literal = extract_source(literal_source, "fixture-literal.test.ts", Globals::Disabled);
        let call = extract_source(call_source, "fixture-call.test.ts", Globals::Disabled);
        assert!(matches!(
            literal.tests[0].context.fixtures[0].body,
            crate::model::FixtureValue::Literal(_)
        ));
        assert!(!literal.tests[0].context.unresolved);
        assert!(call.tests[0].context.unresolved);
        assert_ne!(
            literal.tests[0].context.fixtures[0].body,
            call.tests[0].context.fixtures[0].body
        );
    }

    // C1: a title whose runtime value is not statically known must not let the surrounding
    // context be trusted for comparison either.
    /// Check declaration extraction: c1 dynamic title marks context unresolved.
    #[test]
    fn c1_dynamic_title_marks_context_unresolved() {
        let source = r#"
            import { test } from "vitest";
            declare function something(): string;
            const name = something();
            test(name, () => {});
        "#;
        let extraction = extract_source(source, "dynamic-title.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert!(matches!(extraction.tests[0].title, Title::Dynamic(_)));
        assert!(extraction.tests[0].context.unresolved);
    }

    // C1: a body argument that is present but not an inline arrow/function this extractor can
    // recurse into (here, a reference to a named helper) is opaque, not absent -- unlike a
    // genuine `.todo` declaration, it must not silently compare as an empty, trivial body.
    /// Check declaration extraction: c1 opaque test body reference marks context unresolved.
    #[test]
    fn c1_opaque_test_body_reference_marks_context_unresolved() {
        let source = r#"
            import { test } from "vitest";
            function helper() {}
            test("uses a helper reference as its body", helper);
        "#;
        let extraction = extract_source(source, "opaque-body.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert!(!extraction.tests[0].has_body);
        assert!(extraction.tests[0].context.unresolved);
    }

    // C1, suite form: a `describe` callback that is neither a block nor a concise-body arrow
    // (here, a reference to a named helper) is a body this walker declines to recurse into --
    // any tests hidden inside it are invisible, so the suite's own record must at least flag
    // that its context could not be fully resolved.
    /// Check declaration extraction: c1 opaque describe body reference marks context
    /// unresolved.
    #[test]
    fn c1_opaque_describe_body_reference_marks_context_unresolved() {
        let source = r#"
            import { describe } from "vitest";
            function helper() {}
            describe("outer", helper);
        "#;
        let extraction = extract_source(source, "opaque-describe-body.test.ts", Globals::Disabled);
        let suite = extraction
            .tests
            .iter()
            .find(|t| t.kind == DeclarationKind::Suite)
            .expect("the describe itself is still recorded");
        assert!(suite.context.unresolved);
    }

    /// Check declaration extraction: each call of a call places title and body on the outer
    /// call.
    #[test]
    fn each_call_of_a_call_places_title_and_body_on_the_outer_call() {
        // Regression for the single easiest thing to get wrong about `.each` in the AST:
        // `test.each([...])("name", fn)` is a *nested* call, whose outer `CallExpression` has a
        // `CallExpression` (`test.each([...])`) as its callee. The title and the callback are
        // arguments of the OUTER call; reading them off the inner one finds the table instead and
        // leaves every row with no body.
        let source = include_str!("../../fixtures/each/each-array-of-arrays.test.ts");
        let extraction = extract_source(source, "each-array-of-arrays.test.ts", Globals::Disabled);
        assert!(extraction.tests.iter().all(|t| t.has_body));
    }

    /// Check declaration extraction: vi mock specifiers apply file wide regardless of source
    /// position.
    #[test]
    fn vi_mock_specifiers_apply_file_wide_regardless_of_source_position() {
        let source = include_str!("../../fixtures/edge/vi-mock-hoisting.test.ts");
        let extraction = extract_source(source, "vi-mock-hoisting.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert_eq!(extraction.tests[0].context.mocked_modules.len(), 1);
        // `"./module"` does not exist on disk under this synthetic test path, so resolution
        // fails and the specifier is anchored via the `unresolved:` fallback rather than left as
        // raw text -- see `MockedModule::specifier`'s doc.
        assert_eq!(
            &*extraction.tests[0].context.mocked_modules[0].specifier,
            "unresolved:module"
        );
        assert!(matches!(
            extraction.tests[0].context.mocked_modules[0].factory,
            MockFactory::Factory(_)
        ));
        assert!(!extraction.tests[0].context.unresolved);
    }

    /// Check declaration extraction: extraction never panics on malformed input.
    #[test]
    fn extraction_never_panics_on_malformed_input() {
        let extraction = extract_source(
            "this is not { valid javascript at all (((",
            "broken.test.ts",
            Globals::Disabled,
        );
        assert!(extraction.incomplete);
    }

    /// Check declaration extraction: extraction never panics on empty input.
    #[test]
    fn extraction_never_panics_on_empty_input() {
        let extraction = extract_source("", "empty.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 0);
    }

    // Robustness defect traced by the fingerprint agent: `oxc_parser` is recursive-descent and
    // aborts the whole process on deeply nested input (proven via forgotten destructors and a
    // binary search: succeeds at 500 levels, overflows at 1000 on a default-size thread). The
    // guard in `run()` must reject a file this deep *before* calling `Parser::parse` at all --
    // if it didn't, this test process itself would abort rather than fail.
    /// Check declaration extraction: deeply nested input is rejected without parsing and the
    /// process survives.
    #[test]
    fn deeply_nested_input_is_rejected_without_parsing_and_the_process_survives() {
        let depth = MAX_NESTING_DEPTH * 4;
        let source = format!("({}1{})", "(".repeat(depth), ")".repeat(depth));
        let extraction = extract_source(&source, "pathological.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 0);
        assert!(extraction.incomplete);
        assert!(
            extraction.diagnostics.iter().any(|d| d.contains("nesting")),
            "expected a diagnostic naming the nesting-depth guard, got {:?}",
            extraction.diagnostics
        );
    }

    // A real (if unusually nested) test file must still extract normally -- the guard's limit is
    // meant to be far beyond anything human-written, not a trap for legitimate deep nesting.
    /// Check declaration extraction: nesting just under the limit still extracts normally.
    #[test]
    fn nesting_just_under_the_limit_still_extracts_normally() {
        let depth = MAX_NESTING_DEPTH - 30;
        let nested = format!("{}1{}", "(".repeat(depth), ")".repeat(depth));
        let source = format!(
            "import {{ test, expect }} from 'vitest';\n\
             test('nested arithmetic', () => {{ expect({nested}).toBe(1); }});"
        );
        let extraction = extract_source(&source, "nested-ok.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert!(!extraction.incomplete);
    }

    /// Check declaration extraction: raw syntax budget has an exact independent boundary.
    #[test]
    fn raw_syntax_budget_has_an_exact_independent_boundary() {
        assert!(parse_refusal(&"!".repeat(MAX_RAW_SYNTAX_UNITS)).is_none());
        assert!(parse_refusal(&"void ".repeat(MAX_RAW_SYNTAX_UNITS)).is_none());
        for source in [
            "!".repeat(MAX_RAW_SYNTAX_UNITS + 1),
            "void ".repeat(MAX_RAW_SYNTAX_UNITS + 1),
            format!("'{}'", "!".repeat(MAX_RAW_SYNTAX_UNITS + 1)),
        ] {
            assert!(parse_refusal(&source)
                .unwrap()
                .contains("raw syntax budget"));
        }
        assert!(parse_refusal(&format!("[{}]", vec!["1"; 20_000].join(","))).is_none());
    }

    /// Check declaration extraction: max nesting depth measures bracket nesting conservatively.
    #[test]
    fn max_nesting_depth_measures_bracket_nesting_conservatively() {
        assert_eq!(max_nesting_depth(""), 0);
        assert_eq!(max_nesting_depth("()"), 1);
        assert_eq!(max_nesting_depth("(())"), 2);
        assert_eq!(max_nesting_depth("()()()"), 1);
        assert_eq!(max_nesting_depth("([{}])"), 3);
        // Defect 2 fixed: brackets inside a string, a comment, or a template literal's literal
        // text are masked entirely -- neither counted nor decremented -- rather than counted at
        // face value. Nothing in this string is real code, so the correct depth is 0, not 3.
        assert_eq!(max_nesting_depth(r#""(((""#), 0);
        assert_eq!(max_nesting_depth("// (((\n"), 0);
        assert_eq!(max_nesting_depth("/* ((( */"), 0);
        // A template literal's `${ ... }` hole is still real code and still counts (the hole's
        // own `{` plus the `(` inside it: depth 2); its literal text on either side does not.
        assert_eq!(max_nesting_depth("`(((${(1)})))`"), 2);
    }

    // Defect 2: the exploit this fix closes. A naive byte scan lets a string's *closing*
    // delimiters drag the running counter back down without ever having genuinely closed
    // anything, so a second batch of real nesting -- genuinely stacked on top of the first from
    // the parser's own perspective, since the string sits inside the still-open outer brackets,
    // not between two balanced groups -- reported a max depth far below the truth. Before this
    // fix, this returned 200 (the string's 200 fake closers reset the counter to 0 before the
    // second batch of opens, so neither batch's local peak ever reflected that both are really
    // nested together); the true depth a real parser recurses through here is 400.
    /// Check declaration extraction: defect2 closing delimiters inside a string no longer lower
    /// the counter.
    #[test]
    fn defect2_closing_delimiters_inside_a_string_no_longer_lower_the_counter() {
        let source = format!(
            "{opens}\"{fake_closes}\"{more_opens}",
            opens = "(".repeat(200),
            fake_closes = ")".repeat(200),
            more_opens = "(".repeat(200),
        );
        assert_eq!(max_nesting_depth(&source), 400);
    }

    // Concise-body arrow `describe` callbacks (`describe("x", () => test(...))`) are legal
    // Vitest. Missing them silently is the worst failure mode this tool has: a clean run that
    // quietly dropped a real test.
    /// Check declaration extraction: concise body describe callback is recursed into.
    #[test]
    fn concise_body_describe_callback_is_recursed_into() {
        let source = r#"
            import { describe, test, expect } from "vitest";
            describe("outer", () => test("inner", () => { expect(1).toBe(1); }));
        "#;
        let extraction = extract_source(source, "concise-describe.test.ts", Globals::Disabled);
        let suite = extraction
            .tests
            .iter()
            .find(|t| t.kind == DeclarationKind::Suite)
            .expect("the describe itself is still recorded as a Suite");
        assert_eq!(suite.title, Title::Static("outer".to_owned()));
        let inner = extraction
            .tests
            .iter()
            .find(|t| t.kind == DeclarationKind::Test)
            .expect("the nested test must not be silently dropped");
        assert_eq!(inner.title, Title::Static("inner".to_owned()));
        assert_eq!(inner.suite_path, vec![Title::Static("outer".to_owned())]);
    }

    // Defect 1 (CRITICAL): tests nested inside `if`/`for`/`try`/`switch` used to fall through the
    // statement walker's catch-all and vanish without a trace -- a clean run that quietly analyzed
    // almost nothing, indistinguishable from a genuinely clean one. This is the exact reproduction
    // that motivated the fix: before it, this extracted only the top-level test (1), with no
    // `incomplete` banner at all.
    //
    // The `for...of` loop is unrolled once per element of its literal array (2 elements), exactly
    // like `.each` row semantics, so it contributes 2 declarations, not 1 -- see
    // `literal_for_of_elements`. Total: 1 (`if`) + 2 (`for...of`) + 1 (top level) = 4.
    /// Check declaration extraction: defect1 tests inside if for are no longer silently
    /// dropped.
    #[test]
    fn defect1_tests_inside_if_for_are_no_longer_silently_dropped() {
        let source = r#"
            import { test, expect } from "vitest";
            if (enabled) { test('inside if', () => { expect(1).toBe(1) }) }
            for (const n of [1,2]) { test(`loop ${n}`, () => { expect(n).toBe(n) }) }
            test('top level', () => { expect(2).toBe(2) })
        "#;
        let extraction = extract_source(source, "defect1-repro.test.ts", Globals::Disabled);
        let titles: Vec<&str> = extraction.tests.iter().map(|t| t.title.display()).collect();
        assert_eq!(
            extraction.tests.len(),
            4,
            "expected 4 tests, got titles: {titles:?}"
        );
        assert!(titles.contains(&"inside if"));
        assert!(titles.contains(&"top level"));
        // The loop's two iterations render statically, since the array is a literal and the loop
        // variable's value is therefore knowable per iteration -- not `Title::Dynamic`.
        assert!(titles.contains(&"loop 1"));
        assert!(titles.contains(&"loop 2"));
        let loop_tests: Vec<&TestCase> = extraction
            .tests
            .iter()
            .filter(|t| t.title.display().starts_with("loop "))
            .collect();
        assert_eq!(loop_tests.len(), 2);
        for test in &loop_tests {
            assert!(
                !test.context.unresolved,
                "a literal for...of array's per-iteration value is fully known: {:?}",
                test.title
            );
        }
    }

    // Defect 1: a `for...of` over something this extractor cannot enumerate (here, a plain
    // identifier) must still walk the loop body exactly once, and a title that interpolates the
    // loop variable must degrade to `Title::Dynamic` -- unresolved, consistent with every other
    // dynamic title -- rather than being silently dropped or guessed at.
    /// Check declaration extraction: defect1 nonliteral for of walks body once with dynamic
    /// title.
    #[test]
    fn defect1_nonliteral_for_of_walks_body_once_with_dynamic_title() {
        let source = r#"
            import { test, expect } from "vitest";
            declare const items: number[];
            for (const n of items) { test(`item ${n}`, () => { expect(n).toBeDefined() }) }
        "#;
        let extraction = extract_source(
            source,
            "defect1-nonliteral-for-of.test.ts",
            Globals::Disabled,
        );
        assert_eq!(extraction.tests.len(), 1);
        assert!(matches!(extraction.tests[0].title, Title::Dynamic(_)));
        assert!(extraction.tests[0].context.unresolved);
    }

    // Defect 1: a statement form the walker deliberately does not recurse into (`with`, disallowed
    // in strict/ESM source and unsound to resolve identifiers inside) must still be visible as a
    // real limitation -- `incomplete` plus a diagnostic naming the construct -- never silent.
    /// Check declaration extraction: defect1 with statement is reported incomplete not silently
    /// skipped.
    #[test]
    fn defect1_with_statement_is_reported_incomplete_not_silently_skipped() {
        let source = r#"
            import { test } from "vitest";
            with (globalThis) {
                test('hidden inside a with block', () => {});
            }
        "#;
        let extraction = extract_source(source, "defect1-with.test.ts", Globals::Disabled);
        assert_eq!(
            extraction.tests.len(),
            0,
            "a `with` body is not analyzed at all, so nothing inside it is (falsely) extracted"
        );
        assert!(extraction.incomplete);
        assert!(
            extraction.diagnostics.iter().any(|d| d.contains("with")),
            "expected a diagnostic naming the `with` statement, got {:?}",
            extraction.diagnostics
        );
    }

    // Defect 1: a hook declared inside a plain `if` (not just the `import.meta.vitest` idiom) must
    // still be registered for the enclosing scope -- the same silent-drop failure mode as a
    // declaration, just for a hook instead of a test.
    /// Check declaration extraction: defect1 hook inside plain if is registered.
    #[test]
    fn defect1_hook_inside_plain_if_is_registered() {
        let source = r#"
            import { describe, test, beforeEach, expect } from "vitest";
            describe("suite", () => {
                if (true) {
                    beforeEach(() => {});
                }
                test("uses the hook", () => { expect(1).toBe(1) });
            });
        "#;
        let extraction = extract_source(source, "defect1-hook-in-if.test.ts", Globals::Disabled);
        let test = extraction
            .tests
            .iter()
            .find(|t| t.kind == DeclarationKind::Test)
            .expect("the test is still extracted");
        assert_eq!(test.context.hooks.len(), 1);
        assert_eq!(test.context.hooks[0].kind, HookKind::BeforeEach);
    }

    // Defect 3: a dynamic `describe` title must taint every test nested inside it, not just the
    // suite's own record -- otherwise two differently-behaving runtime suite instances that
    // normalize to the same title text let their nested tests be falsely compared.
    /// Check declaration extraction: defect3 dynamic describe title taints nested tests.
    #[test]
    fn defect3_dynamic_describe_title_taints_nested_tests() {
        let source = r#"
            import { describe, test, expect } from "vitest";
            declare function suiteName(): string;
            describe(suiteName(), () => {
                test("nested", () => { expect(1).toBe(1) });
            });
        "#;
        let extraction = extract_source(source, "defect3-dynamic-suite.test.ts", Globals::Disabled);
        let suite = extraction
            .tests
            .iter()
            .find(|t| t.kind == DeclarationKind::Suite)
            .expect("suite recorded");
        assert!(matches!(suite.title, Title::Dynamic(_)));
        assert!(suite.context.unresolved);
        let nested = extraction
            .tests
            .iter()
            .find(|t| t.kind == DeclarationKind::Test)
            .expect("nested test recorded, not dropped");
        assert!(
            nested.context.unresolved,
            "a test nested inside a dynamic-titled suite must inherit that suite's unresolved \
             status"
        );
    }

    // Defect 4: a `.each` table can be a literal array (so the table itself is fully enumerable)
    // while an individual row still carries a cell this extractor cannot read. That row's own
    // identity is unknown even though the table's shape is not, and must not be treated as
    // resolved just because the surrounding array literal is.
    /// Check declaration extraction: defect4 each row with unresolved cell is marked
    /// unresolved.
    #[test]
    fn defect4_each_row_with_unresolved_cell_is_marked_unresolved() {
        let source = r#"
            import { test } from "vitest";
            declare function computeFlag(): string;
            const flag = computeFlag();
            test.each([[flag, 1], ["known", 2]])("row %s", (value, n) => {});
        "#;
        let extraction = extract_source(
            source,
            "defect4-each-row-unresolved.test.ts",
            Globals::Disabled,
        );
        assert_eq!(extraction.tests.len(), 2);
        // The row referencing `flag` (an unresolved identifier) must be unresolved...
        assert!(
            extraction.tests.iter().any(|t| t.context.unresolved),
            "a row with an unresolved cell must mark its test unresolved"
        );
        // ...but the sibling row, whose only cell is a plain literal, must not be dragged down
        // with it -- resolution is per-row, not table-wide.
        assert!(
            extraction.tests.iter().any(|t| !t.context.unresolved),
            "a fully literal row must stay comparable even when a sibling row is not"
        );
    }

    // F2: `test.each([])` is a literal, known-empty table -- Vitest registers zero tests for it
    // at runtime. Synthesizing a representative test here (as the fallback used to, since it
    // could not tell "table is an empty literal" apart from "table could not be enumerated at
    // all") produced a synthetic test with no runtime counterpart, which could collide with a
    // real test at Error severity, inflate `testsAnalyzed`, and bypass the zero-extraction guard.
    /// Check declaration extraction: f2 test each with known empty table emits no test.
    #[test]
    fn f2_test_each_with_known_empty_table_emits_no_test() {
        let source = r#"
            import { test } from "vitest";
            test.each([])("case %s", () => {});
            test("real test", () => {});
        "#;
        let extraction = extract_source(source, "f2-empty-test-each.test.ts", Globals::Disabled);
        assert_eq!(
            extraction.tests.len(),
            1,
            "a known-empty `.each` table must contribute zero tests, not a synthetic \
             representative: {:?}",
            extraction.tests
        );
        assert_eq!(
            extraction.tests[0].title,
            Title::Static("real test".to_owned())
        );
    }

    // F2, suite form: `describe.each([])` instantiates zero suites at runtime, so neither the
    // suite's own record nor anything nested inside it is ever reached.
    /// Check declaration extraction: f2 describe each with known empty table emits nothing.
    #[test]
    fn f2_describe_each_with_known_empty_table_emits_nothing() {
        let source = r#"
            import { describe, test } from "vitest";
            describe.each([])("suite %s", () => {
                test("inner", () => {});
            });
            test("top-level", () => {});
        "#;
        let extraction =
            extract_source(source, "f2-empty-describe-each.test.ts", Globals::Disabled);
        assert_eq!(
            extraction.tests.len(),
            1,
            "a known-empty `describe.each` table must contribute neither a `Suite` record nor \
             any declaration nested inside it: {:?}",
            extraction.tests
        );
        assert_eq!(
            extraction.tests[0].title,
            Title::Static("top-level".to_owned())
        );
    }

    // F3: entering a suite used to push its title and hook scope but drop its own
    // `Each`/`SkipIf`/`RunIf` on the floor -- a nested test's own `modifiers` only ever held its
    // own chain, never anything inherited from an enclosing `describe`. `analysis.rs`'s
    // `ModifierKey` keys `duplicate-test` on exactly those chain steps, so a nested test must
    // carry its enclosing suite's copy too, not just its own (empty) one.
    /// Check declaration extraction: f3 suite each is inherited into nested test modifiers.
    #[test]
    fn f3_suite_each_is_inherited_into_nested_test_modifiers() {
        let source = r#"
            import { describe, test } from "vitest";
            describe.each([1])("suite", () => {
                test("inner", () => {});
            });
        "#;
        let extraction = extract_source(source, "f3-suite-each.test.ts", Globals::Disabled);
        let inner = extraction
            .tests
            .iter()
            .find(|t| t.kind == DeclarationKind::Test)
            .expect("nested test recorded");
        assert!(
            inner
                .modifiers
                .iter()
                .any(|m| matches!(m, Modifier::Each(_))),
            "a nested test must inherit its enclosing suite's own `.each`, not just its own \
             (empty) chain: {:?}",
            inner.modifiers
        );
    }

    // F3: the adversarial case the fix exists for -- two suites that differ *only* in an
    // enclosing `.skipIf` condition, with the same suite title and identical nested test bodies.
    // Before F3, both nested `inner` tests carried empty `modifiers`, so `ModifierKey` saw no
    // difference between them at all and they compared equal at Error severity.
    /// Check declaration extraction: f3 suite skip if is inherited and distinguishes nested
    /// tests across suites.
    #[test]
    fn f3_suite_skip_if_is_inherited_and_distinguishes_nested_tests_across_suites() {
        let source = r#"
            import { describe, test } from "vitest";
            declare const FEATURE_A: boolean;
            declare const FEATURE_B: boolean;
            describe.skipIf(FEATURE_A)("feature", () => {
                test("inner", () => {});
            });
            describe.skipIf(FEATURE_B)("feature", () => {
                test("inner", () => {});
            });
        "#;
        let extraction = extract_source(source, "f3-suite-skip-if.test.ts", Globals::Disabled);
        let nested: Vec<_> = extraction
            .tests
            .iter()
            .filter(|t| t.kind == DeclarationKind::Test)
            .collect();
        assert_eq!(nested.len(), 2);
        assert_ne!(
            nested[0].modifiers, nested[1].modifiers,
            "two nested tests declared under suites with different, resolvable `.skipIf` \
             conditions must not carry identical modifiers just because their own chain is empty"
        );
    }

    // `.each` row-value substitution in body fingerprints. A row's body was once fingerprinted
    // from the callback's *parameter identifiers*, never the row's concrete values, so a row of
    // `test.each([[2, 3, 6]])` and a hand-written test doing the same work with those literals
    // could never be linked -- exactly the consolidation this tool exists to surface, in reverse.
    // See `fingerprint.rs`'s module doc, "Row substitution", for the mechanism and its bail-outs.

    // The fixture this whole feature exists for (`fixtures/duplicates/each-expanded-vs-table.
    // test.ts`, moved there from `fixtures/near/` once this passed): a fully-known positional
    // `.each` row must fingerprint identically to a hand-written test duplicating that row's own
    // concrete values, so `duplicate-test` can link them.
    /// Check declaration extraction: p1 8 each row substitutes positional params and matches
    /// hand written duplicate.
    #[test]
    fn p1_8_each_row_substitutes_positional_params_and_matches_hand_written_duplicate() {
        let source = r#"
            import { test, expect } from "vitest";
            function area(w: number, h: number) {
                return w * h;
            }
            test.each([[2, 3, 6]])("area %i x %i", (w, h, expected) => {
                expect(area(w, h)).toBe(expected);
            });
            test("area of a 2x3 rectangle", () => {
                expect(area(2, 3)).toBe(6);
            });
        "#;
        let extraction = extract_source(source, "p1_8-row-substitution.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 2);
        assert_eq!(
            extraction.tests[0].body.alpha, extraction.tests[1].body.alpha,
            "a fully-known `.each` row must fingerprint identically (alpha) to a hand-written \
             test that duplicates that row's own concrete values"
        );
        assert_eq!(
            extraction.tests[0].body.structural, extraction.tests[1].body.structural,
            "structural must match too, since alpha already does"
        );
    }

    // Hard part 1 (scoping): a local that shadows a substituted parameter's *name* inside the
    // body must resolve to its own declaration, not the parameter's, and must therefore never be
    // substituted. This is the same "symbol identity, not identifier text" discipline the module
    // doc says has been gotten wrong and fixed four times already for free references; row
    // substitution must not become a fifth. Proven by matching a hand-written twin that uses the
    // exact same shadow: if substitution matched by name, it would incorrectly rewrite the
    // shadowed `w` too, and the two bodies would still match (vacuously) -- see the negative
    // control below for why that possibility is separately ruled out.
    /// Check declaration extraction: p1 8 row substitution respects lexical shadowing.
    #[test]
    fn p1_8_row_substitution_respects_lexical_shadowing() {
        let source = r#"
            import { test, expect } from "vitest";
            function add(a: number, b: number) {
                return a + b;
            }
            test.each([[2, 3, 6]])("row", (w, h, expected) => {
                { const w = 999; expect(w).toBe(999); }
                expect(add(w, h)).toBe(expected);
            });
            test("hand-written twin", () => {
                { const w = 999; expect(w).toBe(999); }
                expect(add(2, 3)).toBe(6);
            });
        "#;
        let extraction = extract_source(source, "p1_8-shadowing.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 2);
        assert_eq!(
            extraction.tests[0].body.alpha, extraction.tests[1].body.alpha,
            "the each row's OWN `w` parameter must substitute to the literal 2, but the \
             block-scoped `const w = 999` shadowing it must stay an ordinary local -- matching \
             the hand-written twin proves substitution is scoped by symbol identity, not text"
        );
    }

    // Negative control for the test above: a *differently*-valued shadow must still be visible in
    // the fingerprint. If shadowing were broken by matching on name instead of symbol (e.g. by
    // ignoring the shadowed `w` entirely, or worse, substituting it too), this would also collide
    // and the previous test would not actually be proving anything.
    /// Check declaration extraction: p1 8 row substitution does not erase a differently
    /// shadowed local.
    #[test]
    fn p1_8_row_substitution_does_not_erase_a_differently_shadowed_local() {
        let source = r#"
            import { test, expect } from "vitest";
            function add(a: number, b: number) {
                return a + b;
            }
            test.each([[2, 3, 6]])("row", (w, h, expected) => {
                { const w = 999; expect(w).toBe(999); }
                expect(add(w, h)).toBe(expected);
            });
            test("different shadow value", () => {
                { const w = 111; expect(w).toBe(111); }
                expect(add(2, 3)).toBe(6);
            });
        "#;
        let extraction = extract_source(source, "p1_8-shadow-negative.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 2);
        assert_ne!(
            extraction.tests[0].body.alpha, extraction.tests[1].body.alpha,
            "a differently-valued shadowing local must still be visible in the fingerprint"
        );
    }

    // Hard part 2 (destructured parameters): a row of objects bound to a single destructured
    // object parameter (`({a, b}) => ...`) must substitute by field name, and must therefore also
    // link to a hand-written duplicate of that row's own values.
    /// Check declaration extraction: p1 8 destructured object row substitutes named params and
    /// matches hand written duplicate.
    #[test]
    fn p1_8_destructured_object_row_substitutes_named_params_and_matches_hand_written_duplicate() {
        let source = r#"
            import { test, expect } from "vitest";
            function add(a: number, b: number) {
                return a + b;
            }
            test.each([{ a: 1, b: 2, expected: 3 }])(
                "add $a + $b = $expected",
                ({ a, b, expected }) => {
                    expect(add(a, b)).toBe(expected);
                }
            );
            test("add 1 + 2 = 3", () => {
                expect(add(1, 2)).toBe(3);
            });
        "#;
        let extraction = extract_source(source, "p1_8-destructured.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 2);
        assert_eq!(
            extraction.tests[0].body.alpha, extraction.tests[1].body.alpha,
            "a destructured object-row parameter must substitute by field name, matching a \
             hand-written duplicate of that row's own values"
        );
    }

    // Hard part 2, the other half: a `Named` (object) row bound to a single *non-destructured*
    // parameter (`(row) => ...`, read via `row.a`/`row.b`) would need member-expression
    // substitution to link -- deliberately not attempted (see `resolve_row_substitution`'s doc).
    // That row's body must fingerprint exactly as if no substitution had ever been offered to it,
    // not some incorrect guess.
    /// Check declaration extraction: p1 8 non destructured single row param is left
    /// unsubstituted.
    #[test]
    fn p1_8_non_destructured_single_row_param_is_left_unsubstituted() {
        // Both declarations live in one file/source (unlike this test's earlier draft, which
        // compared across two independently-parsed sources and could never pass regardless of
        // substitution: `add`'s `free:module` identity is deliberately file-scoped -- see the
        // module doc's discussion of within- vs cross-file free identity -- so the same function
        // declared in two different files never fingerprints as the same free reference). A
        // same-shape sibling test with a differently-named parameter is the control: canonical
        // local numbering already ignores the *name*, so if the `.each` row's `row` had been
        // substituted (incorrectly), its alpha would show a literal in place of `id:L0` and would
        // no longer match this sibling's own `id:L0`-based body.
        let source = r#"
            import { test, expect } from "vitest";
            function add(a: number, b: number) {
                return a + b;
            }
            test.each([{ a: 1, b: 2 }])("row", (row) => {
                expect(add(row.a, row.b)).toBe(3);
            });
            test("same shape, no each", (item) => {
                expect(add(item.a, item.b)).toBe(3);
            });
        "#;
        let extraction = extract_source(source, "p1_8-non-destructured.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 2);
        assert_eq!(
            extraction.tests[0].body.alpha, extraction.tests[1].body.alpha,
            "a plain, non-destructured single row parameter must not be substituted at all"
        );
    }

    // Hard part 3 (partial knowledge), unit-level: `resolve_row_substitution`'s caller-facing
    // gate, `row_substitution`, must refuse to build anything at all for a row with any
    // unresolved cell -- never substitute only the cells that happen to be readable.
    /// A printf argument whose renderings all agree, which is true of every plain number.
    fn arg(value: &str) -> TitleArg {
        TitleArg {
            coerced: value.to_owned(),
            inspected: value.to_owned(),
            json: Some(value.to_owned()),
            field: value.to_owned(),
            number: value
                .parse::<f64>()
                .ok()
                .map(crate::frontend::registry::TitleNumber::Number),
        }
    }

    /// Check declaration extraction: p1 8 row substitution bails out entirely when any cell is
    /// unresolved.
    #[test]
    fn p1_8_row_substitution_bails_out_entirely_when_any_cell_is_unresolved() {
        let row = Row {
            display: "[2, someHelper(), 6]".to_owned(),
            cells: RowCells::Positional(vec![
                Some(CellValue {
                    display: "2".to_owned(),
                    kind: "number",
                    raw: "2".to_owned(),
                }),
                None,
                Some(CellValue {
                    display: "6".to_owned(),
                    kind: "number",
                    raw: "6".to_owned(),
                }),
            ]),
            unresolved: true,
            title_args: vec![Some(arg("2")), None, Some(arg("6"))],
        };
        assert!(
            row_substitution(&row).is_none(),
            "a row with any unresolved cell must never be partially substituted"
        );
    }

    // Hard part 3, end to end: a row with one unreadable cell must extract with a body
    // fingerprint indistinguishable from the same callback declared with no `.each` (and
    // therefore no substitution) at all -- proving the partial cells that *were* readable were
    // never substituted either. A half-substituted body here could collide with something it does
    // not actually match.
    /// Check declaration extraction: p1 8 each row with one unreadable cell stays
    /// unsubstituted.
    #[test]
    fn p1_8_each_row_with_one_unreadable_cell_stays_unsubstituted() {
        // Same-file comparison against a same-shape sibling, for the same reason the test above
        // needs it: `add`'s free identity is file-scoped, so the comparison only means anything
        // when both declarations share one source. If this row had been half-substituted
        // (position 0 and 2 replaced by their literals, position 1 left as a canonical local
        // because `loadThird()` could not be read), its alpha would show `<number:2>`/`<number:6>`
        // where the sibling shows `id:L0`/`id:L2`, and the two would diverge.
        let source = r#"
            import { test, expect } from "vitest";
            declare function loadThird(): number;
            function add(a: number, b: number) {
                return a + b;
            }
            test.each([[2, loadThird(), 6]])("row", (a, b, expected) => {
                expect(add(a, b)).toBe(expected);
            });
            test("same shape, no each", (x, y, z) => {
                expect(add(x, y)).toBe(z);
            });
        "#;
        let extraction = extract_source(source, "p1_8-partial.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 2);
        assert!(
            extraction.tests[0].context.unresolved,
            "an unreadable cell already marks the row/context unresolved (pre-existing defect 4 \
             behavior -- unchanged by this feature)"
        );
        assert_eq!(
            extraction.tests[0].body.alpha, extraction.tests[1].body.alpha,
            "a row with one unreadable cell must fingerprint exactly as if no substitution \
             existed for it -- never half-substituted"
        );
    }

    // Hard part 4: rows of one `.each` declaration must still never be reported as duplicates of
    // each other. Substitution makes their bodies genuinely differ now (the first assertion
    // below, which was not true before this feature) -- the guard that stops them being compared
    // as a group at all (`analysis.rs`'s `dedup_by_declaration`) does not live in this module, but
    // it depends entirely on every row of one declaration continuing to share the declaration's
    // own call-site `Location` (the second assertion). That precondition is unrelated to body
    // fingerprinting and this feature does not touch it, but it is exactly the kind of thing a
    // change to how bodies compare could silently invalidate, so it is asserted directly here.
    /// Check declaration extraction: p1 8 distinct rows of one each declaration get distinct
    /// alpha but share location.
    #[test]
    fn p1_8_distinct_rows_of_one_each_declaration_get_distinct_alpha_but_share_location() {
        let source = r#"
            import { test, expect } from "vitest";
            function classify(a: number, b: number) {
                return a < b ? "less" : "not-less";
            }
            test.each([
                [2, 4, "less"],
                [3, 4, "less"],
                [3, 5, "less"],
            ])("classify(%i, %i) = '%s'", (a, b, expected) => {
                expect(classify(a, b)).toBe(expected);
            });
        "#;
        let extraction = extract_source(source, "p1_8-rows-distinct.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 3);
        let alphas: std::collections::BTreeSet<_> =
            extraction.tests.iter().map(|t| t.body.alpha).collect();
        assert_eq!(
            alphas.len(),
            3,
            "substitution must make each row's own body genuinely differ from its siblings"
        );
        let locations: std::collections::BTreeSet<_> = extraction
            .tests
            .iter()
            .map(|t| t.location.clone())
            .collect();
        assert_eq!(
            locations.len(),
            1,
            "every row of one `.each` declaration must still share the declaration's own \
             call-site Location -- the precondition `analysis.rs`'s `dedup_by_declaration` guard \
             relies on to keep rows of one declaration from ever being reported as duplicates of \
             each other, even though their bodies now genuinely differ"
        );
    }

    // `analysis.rs`'s `ModifierKey` treats `Modifier::Each` as behavior-significant precisely so
    // two *different* `.each` tables driving the same-shaped callback over different data are
    // never merged on body alone (the EMAILS/PHONES case its own doc describes). That guard is
    // exactly what would keep a hand-written duplicate of a substituted row from ever being
    // linked to it: the hand-written test never carries an `Each` modifier, so if the row kept
    // carrying one, `ModifierKey` would disagree between them regardless of how well the
    // (substituted) bodies match. A fully-substituted row's own `Each` must therefore be dropped
    // from *that row's* modifiers -- see `emit_test`'s comment at the each-row loop for why this
    // is still sound (the substituted body itself now carries the differentiation `Each` used to
    // provide).
    /// Check declaration extraction: p1 8 fully substituted row does not carry an each
    /// modifier.
    #[test]
    fn p1_8_fully_substituted_row_does_not_carry_an_each_modifier() {
        let source = r#"
            import { test, expect } from "vitest";
            test.each([[2, 3, 6]])("row", (a, b, expected) => {
                expect(a * b).toBe(expected);
            });
        "#;
        let extraction = extract_source(
            source,
            "p1_8-each-modifier-dropped.test.ts",
            Globals::Disabled,
        );
        assert_eq!(extraction.tests.len(), 1);
        assert!(
            !extraction.tests[0]
                .modifiers
                .iter()
                .any(|m| matches!(m, Modifier::Each(_))),
            "a fully-substituted `.each` row must not carry its own `Each` modifier: {:?}",
            extraction.tests[0].modifiers
        );
    }

    // Negative control: a row this extractor cannot fully substitute (defect 4's unreadable cell)
    // must keep carrying `Each`, unchanged from pre-substitution behavior -- there is no
    // substituted body
    // to lean on instead, so dropping it here would have no compensating differentiation. This
    // case never actually reaches `ModifierKey` in practice (the same unresolved cell already
    // marks `Context::unresolved`, excluding it from comparison entirely), but the modifier itself
    // must still reflect reality rather than being dropped unconditionally.
    /// Check declaration extraction: p1 8 partially unresolved row still carries its each
    /// modifier.
    #[test]
    fn p1_8_partially_unresolved_row_still_carries_its_each_modifier() {
        let source = r#"
            import { test, expect } from "vitest";
            declare function loadThird(): number;
            test.each([[2, loadThird(), 6]])("row", (a, b, expected) => {
                expect(a).toBeLessThan(expected);
            });
        "#;
        let extraction =
            extract_source(source, "p1_8-each-modifier-kept.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        assert!(
            extraction.tests[0]
                .modifiers
                .iter()
                .any(|m| matches!(m, Modifier::Each(_))),
            "a row this extractor could not fully substitute must keep its `Each` modifier: {:?}",
            extraction.tests[0].modifiers
        );
    }

    // A fully-substituted *inner* `test.each` row must drop only its own `Each` modifier, never
    // an *enclosing* `describe.each`'s -- substitution does not cover describe-level rows (see
    // `fingerprint::RowSubstitution`'s doc), so the suite's own `Each` still needs to differentiate
    // its instances exactly as before. This guards against the natural but wrong implementation
    // that filters `Modifier::Each` out of the whole (suite-prefixed) modifier list rather than
    // only the row's own contribution.
    /// Check declaration extraction: p1 8 dropping the rows own each modifier does not remove
    /// an enclosing suites.
    #[test]
    fn p1_8_dropping_the_rows_own_each_modifier_does_not_remove_an_enclosing_suites() {
        let source = r#"
            import { describe, test, expect } from "vitest";
            describe.each([["group-a"], ["group-b"]])("%s", (group) => {
                test.each([[2, 3, 6]])("row", (a, b, expected) => {
                    expect(a * b).toBe(expected);
                });
            });
        "#;
        let extraction = extract_source(source, "p1_8-nested-each.test.ts", Globals::Disabled);
        let inner_tests: Vec<_> = extraction
            .tests
            .iter()
            .filter(|t| t.kind == DeclarationKind::Test)
            .collect();
        assert_eq!(
            inner_tests.len(),
            2,
            "two describe.each instances, one test row each"
        );
        for test in inner_tests {
            let each_modifiers: Vec<_> = test
                .modifiers
                .iter()
                .filter(|m| matches!(m, Modifier::Each(_)))
                .collect();
            assert_eq!(
                each_modifiers.len(),
                1,
                "the inner test's own (substituted) `Each` must be dropped, but the enclosing \
                 `describe.each`'s own `Each` must survive: {:?}",
                test.modifiers
            );
        }
    }

    // The sixth instance of "unique in one scope, wrong in a wider one": `vi.mock`'s argument is
    // file-relative exactly like an `import` specifier is, but was stored as raw argument text
    // with no per-file anchoring at all, so the identical text in two different directories --
    // naming two different files on disk -- produced an identical `MockedModule` and therefore an
    // equivalent `Context`. See `MockedModule::specifier`'s doc and
    // `fingerprint::specifier_identity`.
    /// Check declaration extraction: cross file relative mock specifier is anchored to the
    /// declaring directory.
    #[test]
    fn cross_file_relative_mock_specifier_is_anchored_to_the_declaring_directory() {
        let dir = tempfile::tempdir().unwrap();
        let dir_a = dir.path().join("a");
        let dir_b = dir.path().join("b");
        // The two `./dep` targets are different files on disk; their content does not even need
        // to differ for this to matter -- two different real files are already two different
        // modules.
        let (extraction_a, extraction_b) =
            mock_extraction_pair(&dir_a, &dir_b, MOCK_DEP_SOURCE, true);
        let specifier_a = &extraction_a.tests[0].context.mocked_modules[0].specifier;
        let specifier_b = &extraction_b.tests[0].context.mocked_modules[0].specifier;

        assert!(
            specifier_a.starts_with("resolved:"),
            "a `./dep` that exists on disk must resolve: {specifier_a}"
        );
        assert!(
            specifier_b.starts_with("resolved:"),
            "a `./dep` that exists on disk must resolve: {specifier_b}"
        );
        assert_ne!(
            specifier_a, specifier_b,
            "identical `vi.mock('./dep')` text in two different directories names two \
             different modules and must not compare equal"
        );
    }

    /// A `vi.mock('./dep')` source whose specifier resolution depends on the file's directory.
    const MOCK_DEP_SOURCE: &str = r#"
            import { vi, test, expect } from "vitest";
            vi.mock("./dep", () => ({ value: 1 }));
            test("reads the mocked value", () => {
                expect(1).toBe(1);
            });
        "#;

    /// A `vi.mock('vitest')` source whose specifier is file-independent.
    const BARE_MOCK_SOURCE: &str = r#"
            import { vi, test } from "vitest";
            vi.mock("vitest");
            test("uses automock", () => {});
        "#;

    /// Creates `dir_a`/`dir_b` (when they differ), optionally writes a `dep.ts` into each,
    /// writes `source` into `x.test.ts` under `dir_a` and `y.test.ts` under `dir_b`, and
    /// extracts both -- the setup every cross-file mock-specifier test reduces to. The same
    /// directory twice is the "still matches" control.
    fn mock_extraction_pair(
        dir_a: &std::path::Path,
        dir_b: &std::path::Path,
        source: &str,
        write_dep: bool,
    ) -> (FileExtraction, FileExtraction) {
        std::fs::create_dir_all(dir_a).unwrap();
        std::fs::create_dir_all(dir_b).unwrap();
        if write_dep {
            std::fs::write(dir_a.join("dep.ts"), "export const value = 1;\n").unwrap();
            if dir_a != dir_b {
                std::fs::write(dir_b.join("dep.ts"), "export const value = 1;\n").unwrap();
            }
        }
        let file_a = dir_a.join("x.test.ts");
        let file_b = dir_b.join("y.test.ts");
        std::fs::write(&file_a, source).unwrap();
        std::fs::write(&file_b, source).unwrap();
        (
            extract(source, &file(file_a.to_str().unwrap()), Globals::Disabled),
            extract(source, &file(file_b.to_str().unwrap()), Globals::Disabled),
        )
    }

    // Positive control for the test above: the same relative specifier from two files in the
    // *same* directory names the same module and must still compare equal.
    /// Check declaration extraction: same directory relative mock specifier still matches
    /// across files.
    #[test]
    fn same_directory_relative_mock_specifier_still_matches_across_files() {
        let dir = tempfile::tempdir().unwrap();
        let (extraction_a, extraction_b) =
            mock_extraction_pair(dir.path(), dir.path(), MOCK_DEP_SOURCE, true);

        assert_eq!(
            extraction_a.tests[0].context.mocked_modules[0].specifier,
            extraction_b.tests[0].context.mocked_modules[0].specifier,
            "two files in the same directory mocking the same relative specifier must still \
             be recognized as the same module"
        );
    }

    // A bare/package specifier is already file-independent and must not be anchored per-file: two
    // files in different directories mocking the same package must still compare equal.
    /// Check declaration extraction: bare mock specifier still matches across files.
    #[test]
    fn bare_mock_specifier_still_matches_across_files() {
        let dir = tempfile::tempdir().unwrap();
        let dir_a = dir.path().join("a");
        let dir_b = dir.path().join("b");
        let (extraction_a, extraction_b) =
            mock_extraction_pair(&dir_a, &dir_b, BARE_MOCK_SOURCE, false);

        assert_eq!(
            extraction_a.tests[0].context.mocked_modules[0].specifier,
            extraction_b.tests[0].context.mocked_modules[0].specifier,
            "a bare/package specifier is file-independent and must match across files \
             regardless of which directory mocks it"
        );
    }

    // -----------------------------------------------------------------------------------------
    // Control-flow constructs a declaration or a hook can hide inside
    // -----------------------------------------------------------------------------------------
    //
    // `walk_statement` and `prepare_statement` each recurse through every construct that can
    // contain a `Statement`, and the wildcard arm is deliberately *not* allowed to mean "ignore
    // silently" for control flow -- a clean run that quietly analyzed less than it claimed to is
    // the worst failure mode this tool has. So each construct is one row, asserting that both the
    // declaration *and* the hook registered beside it survive the recursion, and that the file is
    // not reported incomplete.

    /// Check declaration extraction: a declaration and a hook are found inside every recursed
    /// construct.
    #[test]
    fn a_declaration_and_a_hook_are_found_inside_every_recursed_construct() {
        #[rustfmt::skip]
        let constructs: &[(&str, &str, &str)] = &[
            // (name, prefix, suffix) wrapped around the declaration and its hook
            ("bare block", "{", "}"),
            ("if consequent", "if (c) {", "}"),
            ("if alternate", "if (c) { } else {", "}"),
            ("if without a block", "if (c)", ""),
            ("for", "for (;;) {", "}"),
            ("for-in", "for (const k in o) {", "}"),
            ("for-of", "for (const k of o) {", "}"),
            ("while", "while (c) {", "}"),
            ("do-while", "do {", "} while (c);"),
            ("try block", "try {", "} catch (e) { }"),
            ("catch handler", "try { } catch (e) {", "}"),
            ("finally", "try { } finally {", "}"),
            ("switch case", "switch (c) { case 1:", "}"),
            ("labeled", "l: {", "}"),
            ("nested twice", "if (c) { while (d) {", "} }"),
        ];
        for (name, prefix, suffix) in constructs {
            // `if without a block` cannot hold two statements, so the hook goes in its own copy.
            let inner: &str = if *name == "if without a block" {
                "test('x', () => { f(); });"
            } else {
                "beforeEach(() => { g(); }); test('x', () => { f(); });"
            };
            let source = format!("{prefix} {inner} {suffix}");
            let extraction = extract_source(&source, "case.test.ts", Globals::Enabled);
            assert!(
                extraction.diagnostics.is_empty(),
                "{name}: unexpected diagnostics for `{source}`: {:?}",
                extraction.diagnostics
            );
            assert_eq!(
                extraction.tests.len(),
                1,
                "{name}: the declaration inside `{source}` must be found"
            );
            if *name != "if without a block" {
                assert_eq!(
                    extraction.tests[0].context.hooks.len(),
                    1,
                    "{name}: the hook beside it belongs to the same scope, not a new one"
                );
            }
        }
    }

    /// `with` is the one construct deliberately left unwalked, and it must say so rather than
    /// drop whatever is inside it silently -- the exact defect the diagnostic exists to close.
    #[test]
    fn a_with_statement_is_reported_rather_than_silently_skipped() {
        let extraction = extract_source(
            "with (o) { test('x', () => {}); }",
            "case.test.js",
            Globals::Enabled,
        );
        assert_eq!(
            extraction.tests.len(),
            0,
            "the body is genuinely not analyzed"
        );
        assert!(
            extraction
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("`with` statement")),
            "...and that has to be reported: {:?}",
            extraction.diagnostics
        );
    }

    // -----------------------------------------------------------------------------------------
    // Titles
    // -----------------------------------------------------------------------------------------

    /// Extract the source with ambient globals enabled and collect its declaration titles.
    fn titles_of(source: &str) -> Vec<Title> {
        extract_source(source, "case.test.ts", Globals::Enabled)
            .tests
            .into_iter()
            .map(|test| test.title)
            .collect()
    }

    /// Embed a hook body in an owned test fixture and return its extracted hook after checking
    /// the test count.
    fn extracted_hook(body: &str) -> Hook {
        let source = format!(
            "import {{ beforeEach, test, vi }} from 'vitest';\nlet x; let flag; let config;\n\
             beforeEach(() => {{ {body} }});\ntest('t', () => {{}});"
        );
        let mut extraction = extract_source(&source, "case.test.ts", Globals::Disabled);
        assert_eq!(extraction.tests.len(), 1);
        let mut hooks = extraction.tests.remove(0).context.hooks;
        assert_eq!(hooks.len(), 1);
        hooks.remove(0)
    }

    /// Refused guard shapes remain non-trivial hooks, without an idempotence claim.
    #[test]
    fn idempotent_guard_classifier_refuses_every_unrecognizable_if_shape() {
        for body in [
            "if (!x) x = 1; else x = 2;",
            "if (x) x = 1;",
            "if (!x) { x = 1; y = 2; }",
            "if (!x) x += 1;",
            "if (!x) y = 1;",
            "if (!getTarget()) x = 1;",
            "if (!x) send();",
            "if (typeof x) x = 1;",
            "if (!x) { var y = 1; }",
        ] {
            let hook = extracted_hook(body);
            assert!(!hook.body.trivial, "the refused hook remains observable");
            assert!(!hook.idempotent, "the shape is not an idempotent guard");
        }
        assert!(extracted_hook("if (!x) { x = 1; }").idempotent);
    }

    /// A reset name on a non-Vitest object must not create an idempotence claim.
    #[test]
    fn framework_reset_arms_are_all_reachable() {
        for body in ["db.vi.clearAllMocks();", "helpers.resetDb();"] {
            assert!(!extracted_hook(body).idempotent);
        }
    }

    /// A `for...of` whose iterable is not a literal array walks once, unrolled, without
    /// binding the loop variable (the loop-unroll classifier's refusal arm).
    #[test]
    fn a_for_of_over_a_non_literal_iterable_walks_once_without_binding() {
        let source = "import { test } from 'vitest';\nfor (const x of getRows()) {\ntest(`case ${x}`, () => {});\n}";
        let extraction = extract_source(source, "case.test.ts", Globals::Enabled);
        assert_eq!(
            extraction.tests.len(),
            1,
            "a dynamic iterable is walked once"
        );
        assert!(
            matches!(extraction.tests[0].title, Title::Dynamic(_)),
            "the unknown loop value must not become a static title"
        );
        assert!(extraction.tests[0].context.unresolved);
    }

    /// Check declaration extraction: helper parameter refusals do not invent static titles.
    #[test]
    fn helper_parameter_refusals_do_not_invent_static_titles() {
        for (parameters, arguments) in [
            ("label", "...args"),
            ("{ label }", "{ label: 'x' }"),
            ("label", ""),
            ("label", "getLabel()"),
            ("...labels", "'x'"),
        ] {
            let source = format!("import {{ test }} from 'vitest'; function declare({parameters}) {{ test(`case ${{label}}`, () => {{}}); }} declare({arguments});");
            let extraction = extract_source(&source, "case.test.ts", Globals::Disabled);
            assert_eq!(extraction.tests.len(), 1);
            assert!(matches!(extraction.tests[0].title, Title::Dynamic(_)));
            assert!(extraction.tests[0].context.unresolved);
        }
    }

    /// Check declaration extraction: non declaration for of targets are not unrolled.
    #[test]
    fn non_declaration_for_of_targets_are_not_unrolled() {
        let extraction = extract_source(
            "import { test } from 'vitest'; let x; for (x of [1, 2]) { test(`case ${x}`, () => {}); }",
            "case.test.ts", Globals::Disabled,
        );
        assert_eq!(extraction.tests.len(), 1);
        assert!(matches!(extraction.tests[0].title, Title::Dynamic(_)));
    }

    /// Concise-arrow hook bodies have no statement list, so the idempotence classifier
    /// refuses them (`function_like_statements`' `_ => None` arm) and the hook records as
    /// ordinary.
    #[test]
    fn a_concise_arrow_hook_body_refuses_idempotence_classification() {
        let extraction = extract_source(
            "import { beforeEach, test, vi } from 'vitest';\nbeforeEach(() => vi.clearAllMocks());\ntest('t', () => {});",
            "case.test.ts", Globals::Disabled,
        );
        assert_eq!(extraction.tests.len(), 1);
        assert_eq!(extraction.tests[0].context.hooks.len(), 1);
        assert!(!extraction.tests[0].context.hooks[0].idempotent);
    }

    /// A cell naming an UNDECLARED identifier binds as Free (globals-enabled): the free/local
    /// distinction in `binding_of`'s arms.
    #[test]
    fn an_undeclared_cell_identifier_binds_free() {
        let source =
            "import { test } from 'vitest';\ntest.each([[someUnknown]])('n %s', () => {});";
        let extraction = extract_source(source, "case.test.ts", Globals::Enabled);
        assert_eq!(extraction.tests.len(), 1);
        assert!(
            matches!(
                extraction.tests[0].title,
                crate::model::Title::Parameterized(_)
            ),
            "an undeclared identifier cannot fold; the title stays unrendered"
        );
    }

    /// C10-adjacent coverage round: `parse_numeric_prefix`'s branch table, directly -- the
    /// parseFloat/parseInt grammar corners a title string cannot reach.
    #[test]
    fn parse_numeric_prefix_covers_the_float_grammar_corners() {
        assert_eq!(
            parse_numeric_prefix("-Infinity", true),
            Some(f64::NEG_INFINITY)
        );
        assert_eq!(parse_numeric_prefix("+Infinity", true), Some(f64::INFINITY));
        // parseInt has no Infinity case: 'Infinity' has no decimal digits, so it is NaN.
        assert_eq!(parse_numeric_prefix("-Infinity", false), None);
        assert_eq!(parse_numeric_prefix("1e3", true), Some(1000.0));
        assert_eq!(parse_numeric_prefix("1E+3", true), Some(1000.0));
        assert_eq!(parse_numeric_prefix("1e-2", true), Some(0.01));
        assert_eq!(parse_numeric_prefix("1e", true), Some(1.0));
        assert_eq!(parse_numeric_prefix("1e-", true), Some(1.0));
        assert_eq!(parse_numeric_prefix(".5", true), Some(0.5));
        assert_eq!(parse_numeric_prefix("5.", true), Some(5.0));
        assert_eq!(parse_numeric_prefix(".e3", true), None);
        assert_eq!(parse_numeric_prefix("0x1G", false), Some(1.0));
        // parseInt parses leading DECIMAL digits and stops: '1e3' is 1 in JavaScript too.
        assert_eq!(parse_numeric_prefix("1e3", false), Some(1.0));
    }

    /// Hazard 6: a title renders only when every value is provable; unresolved values degrade
    /// rather than inventing a static title that unrelated tests could share.
    #[test]
    fn a_title_is_rendered_only_from_values_provable_in_source() {
        assert_eq!(
            titles_of("test('plain', () => {});"),
            vec![Title::Static("plain".to_owned())]
        );
        assert_eq!(
            titles_of("test(`plain`, () => {});"),
            vec![Title::Static("plain".to_owned())],
            "a no-substitution template is just a string"
        );
        assert_eq!(
            titles_of("test(`a ${1 + 1} b`, () => {});"),
            vec![Title::Static("a 2 b".to_owned())],
            "a template whose every substitution folds is fully known"
        );
        assert_eq!(
            titles_of("test(`a ${mk()} b`, () => {});"),
            vec![Title::Dynamic("a ${_} b".to_owned())],
            "one unfoldable substitution normalizes every substitution to a placeholder, \
             rather than mixing folded and unfolded into one shape"
        );
        assert_eq!(
            titles_of("test(1 + 1, () => {});"),
            vec![Title::Static("2".to_owned())],
            "a non-literal title argument still folds when it can"
        );
        // C12's real shape, missed by the first disproof: the DIRECT title path
        // (`extract_title` -> `fold_expr`) forwards `format_number`'s text verbatim, so a
        // non-finite fold titled as `inf` where JavaScript's `String(1/0)` is `Infinity`.
        // Measured against the binary; pinned here because the `.each` guard tests cannot
        // catch this path -- their values are reparsed before display.
        assert_eq!(
            titles_of("test(1 / 0, () => {});"),
            vec![Title::Static("Infinity".to_owned())],
            "a non-finite direct title renders the ECMAScript spelling"
        );
        assert_eq!(
            titles_of("test(`n ${1 / 0}`, () => {});"),
            vec![Title::Static("n Infinity".to_owned())],
            "and so does a template substitution carrying one"
        );
        assert_eq!(
            titles_of("test(-(1 / 0), () => {});"),
            vec![Title::Static("-Infinity".to_owned())],
            "both signs"
        );
        // The numeric-string grammar corners ride the %f directive (parseFloat), not template
        // substitution (which is ToString and renders the string itself).
        assert_eq!(
            titles_of("test.each([['-Infinity']])('n %f', () => {});"),
            vec![Title::Static("n -Infinity".to_owned())],
            "parseFloat reads the signed Infinity spelling"
        );
        assert_eq!(
            titles_of("test.each([['1e+3']])('n %f', () => {});"),
            vec![Title::Static("n 1000".to_owned())],
            "exponent with an explicit plus"
        );
        assert_eq!(
            titles_of("test.each([['1e-2']])('n %f', () => {});"),
            vec![Title::Static("n 0.01".to_owned())],
            "negative exponent"
        );
        assert_eq!(
            titles_of("test.each([['1e']])('n %f', () => {});"),
            vec![Title::Static("n 1".to_owned())],
            "an exponent with no digits stops the parse at the mantissa"
        );
        assert_eq!(
            titles_of("test.each([['.5']])('n %f', () => {});"),
            vec![Title::Static("n 0.5".to_owned())],
            "a fraction with no leading digits"
        );
        assert_eq!(
            titles_of("test(mk(), () => {});"),
            vec![Title::Dynamic("mk()".to_owned())],
            "...and keeps its source text when it cannot"
        );
        assert_eq!(
            titles_of("test();"),
            vec![Title::Dynamic(String::new())],
            "no title argument at all is dynamic, not an empty static title"
        );
    }

    /// Every `.each` title directive, one row each, plus the two shapes that must degrade.
    #[test]
    fn each_title_directives_render_from_the_rows_they_name() {
        #[rustfmt::skip]
        let cases: &[(&str, &str, &str)] = &[
            // (name, source, the single rendered title)
            ("%s consumes positional cells in order",
             "test.each([[1, 'a']])('case %s and %s', () => {});", "case 1 and a"),
            ("%d", "test.each([[1]])('n %d', () => {});", "n 1"),
            ("%i", "test.each([[1]])('n %i', () => {});", "n 1"),
            ("%f", "test.each([[1.5]])('n %f', () => {});", "n 1.5"),
            // The three directives disagree about one string, which is the whole reason a
            // printf argument carries three renderings. Measured on Vitest 5.0.1.
            ("%j is JSON.stringify", "test.each([['a']])('n %j', () => {});", "n \"a\""),
            ("%j uses JavaScript exponent notation",
             "test.each([[1e21]])('n %j', () => {});", "n 1e+21"),
            ("%j uses JavaScript's shortest decimal",
             "test.each([[201562347225087.63]])('n %j', () => {});", "n 201562347225087.62"),
            ("%j normalizes negative zero",
             "test.each([[-0]])('n %j', () => {});", "n 0"),
            ("%j turns non-finite numbers into null",
             "test.each([[1 / 0]])('n %j', () => {});", "n null"),
            ("%o is the inspect form", "test.each([['a']])('n %o', () => {});", "n 'a'"),
            ("%s is string coercion", "test.each([['a']])('n %s', () => {});", "n a"),
            // An exhausted argument list is not a failure for `%s`: Vitest prints `undefined`.
            ("%s past the end of the row",
             "test.each([[1]])('n %s %s', () => {});", "n 1 undefined"),
            ("an empty array row spreads to no arguments at all",
             "test.each([[]])('n %s', () => {});", "n undefined"),
            ("an object row is ONE argument, printed whole -- not one per field",
             "test.each([{ a: 1, b: 2 }])('n %s %s', () => {});", "n { a: 1, b: 2 } undefined"),
            ("a nested array under %s coerces through join, not inspect",
             "test.each([[[1, [2, 3]]]])('n %s', () => {});", "n 1,2,3"),
            ("null inside a join renders as empty",
             "test.each([[[null]]])('n %s', () => {});", "n "),
            ("%# is the zero-based index",
             "test.each([[1]])('row %#', () => {});", "row 0"),
            ("%$ is the one-based index",
             "test.each([[1]])('row %$', () => {});", "row 1"),

            // The numeric coercions that yield a real number render, measured on Vitest 5.0.3
            // (docs/vitest-surface.md §4): `%d` is `Number(value)`, so `null`/`[]`/`''` are 0
            // and `true` is 1; `%i`/`%f` parse the string form's prefix, so `'12abc'` and an
            // array's joined text both yield their leading number.
            ("%d of null is Number(null)", "test.each([[null]])('n %d', () => {});", "n 0"),
            ("%d of true is Number(true)", "test.each([[true]])('n %d', () => {});", "n 1"),
            ("%d of an empty-array cell is 0 through its empty string form",
             "test.each([[[]]])('n %d', () => {});", "n 0"),
            ("%d of an empty string is 0", "test.each([['']])('n %d', () => {});", "n 0"),
            ("%d of [null] is 0 through the joined string form",
             "test.each([[[null]]])('n %d', () => {});", "n 0"),
            ("%d of Infinity renders", "test.each([[1 / 0]])('n %d', () => {});", "n Infinity"),
            ("%f of Infinity renders", "test.each([[1 / 0]])('n %f', () => {});", "n Infinity"),
            // C12 guards: these pass because every `.each` display path reparses the fold
            // through ryu-js. They are NOT the whole story -- the direct-title path forwards
            // `format_number`'s text verbatim and was the real C12 defect (see
            // `a_title_is_rendered_only_from_values_provable_in_source`); these stay to pin the
            // reparse discipline that makes the two paths agree.
            ("%s of a non-finite fold renders Infinity",
             "test.each([[[1 / 0]]])('n %s', () => {});", "n Infinity"),
            ("%s of a concatenation with a non-finite fold",
             "test.each([['x' + (1 / 0)]])('n %s', () => {});", "n xInfinity"),
            // C12 companions: NaN spells the same in both languages, and the round-trip
            // through number_of_string is what makes 'Infinity' parse back to the value.
            ("%s of a NaN fold renders NaN",
             "test.each([[[0 / 0]]])('n %s', () => {});", "n NaN"),
            ("%d of an all-zero hex string is 0",
             "test.each([['0x000']])('n %d', () => {});", "n 0"),
            ("%d of -0 prints unsigned", "test.each([[-0]])('n %d', () => {});", "n 0"),
            ("%i of -0 prints unsigned", "test.each([[-0]])('n %i', () => {});", "n 0"),
            // The one signed-zero exception: Vitest's own formatTitle rewrites `%f` to `-%f`
            // for a negative-zero item, so `%f` alone of the three keeps the sign.
            ("%f of -0 keeps the sign", "test.each([[-0]])('n %f', () => {});", "n -0"),
            ("%i parses a numeric prefix", "test.each([['12abc']])('n %i', () => {});", "n 12"),
            ("%f parses a numeric prefix", "test.each([['12abc']])('n %f', () => {});", "n 12"),
            ("%i of a string truncates, not rounds",
             "test.each([['1.5']])('n %i', () => {});", "n 1"),
            ("%f of an array parses its joined string form",
             "test.each([[[2, 3]]])('n %f', () => {});", "n 2"),
            ("%i of a hex string reads the radix",
             "test.each([['0x1F']])('n %i', () => {});", "n 31"),
            ("%d of a hex string reads it too",
             "test.each([['0x1F']])('n %d', () => {});", "n 31"),
            // Copilot review, PR #28 round 6, verified in node: a string-valued binary is a
            // STRING, and Number reads it with the full string grammar -- the numeric path
            // parsed the folded text as Rust decimal and refused.
            ("%d of a string-valued binary reads the radix",
             "test.each([[('0x' + '10')]])('n %d', () => {});", "n 16"),
            // Copilot review, PR #28: the radix prefixes are case-insensitive and the sign
            // applies to them (round 2 corrects the prefix SET: parseInt reads hex only).
            ("%d of an uppercase hex string",
             "test.each([['0X1F']])('n %d', () => {});", "n 31"),
            ("%i of a signed hex string applies the sign",
             "test.each([['-0x1F']])('n %i', () => {});", "n -31"),
            // Copilot review, PR #28 round 2 (verified in V8): parseInt reads the HEX prefix
            // only, so a binary/octal-prefix string parses its leading 0 and renders 0 -- while
            // `%d` is `Number`, which DOES read those prefixes.
            ("%i of a binary-prefix string parses its leading 0",
             "test.each([['0b11']])('n %i', () => {});", "n 0"),
            ("%i of an octal-prefix string parses its leading 0",
             "test.each([['0o17']])('n %i', () => {});", "n 0"),
            ("%d of a binary-prefix string reads the radix",
             "test.each([['0b11']])('n %d', () => {});", "n 3"),
            ("%d of an octal-prefix string reads the radix",
             "test.each([['0o17']])('n %d', () => {});", "n 15"),
            ("%i of trailing garbage after a hex prefix stops at the digits",
             "test.each([['0x12g']])('n %i', () => {});", "n 18"),
            // Copilot review, PR #28 round 6, verified in node: a bare prefix commits the
            // radix -- `parseInt('0x')`, `parseInt('0xg')`, `parseInt('0x-1')` are all NaN,
            // NOT the prefix's leading 0, which the decimal fallback used to render.

            // Copilot review, PR #28 round 5, measured in V8: the radix converters are
            // unbounded, rounding once from the exact mathematical value like the spec says.
            // Thirty-two f's is 2^128 - 1 exactly; the old i128 path refused it.
            ("%i of a 32-digit hex string rounds like Number",
             "test.each([['0xffffffffffffffffffffffffffffffff']])('n %i', () => {});",
             "n 3.402823669209385e+38"),
            ("%d of a 32-digit hex string rounds like Number",
             "test.each([['0xffffffffffffffffffffffffffffffff']])('n %d', () => {});",
             "n 3.402823669209385e+38"),
            ("%d of a 200-digit binary string",
             "test.each([['0b11111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111']])('n %d', () => {});",
             "n 1.6069380442589903e+60"),
            ("%d past the double range is Infinity",
             "test.each([['0x1000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000']])('n %d', () => {});",
             "n Infinity"),
            ("%d of a mid-size hex string rounds from the exact value",
             "test.each([['0x123456789abcdef0123456789abcdef']])('n %d', () => {});",
             "n 1.512366075204171e+36"),

            ("%i of a padded number trims first",
             "test.each([['  12  ']])('n %i', () => {});", "n 12"),
            // Copilot review, PR #28 round 3, measured in V8: the Unicode Space_Separator
            // characters (U+1680, U+2000-U+200A, U+202F, U+205F, U+3000) are ECMAScript
            // whitespace -- the round-1 predicate omitted them and refused values Vitest
            // renders.
            ("%d of an em-space-padded number",
             "test.each([['\\u{2003}12']])('n %d', () => {});", "n 12"),
            // U+FEFF is ECMAScript whitespace BY NAME (the WhiteSpace production lists ZWNBSP)
            // though it is category Cf, which Rust's is_whitespace misses (round 5).
            ("%d of a BOM-padded number",
             "test.each([['\\u{FEFF}12']])('n %d', () => {});", "n 12"),
            ("%f of a BOM-padded number",
             "test.each([['\\u{FEFF}1.5']])('n %f', () => {});", "n 1.5"),
            ("%i of an em-space-padded number",
             "test.each([['\\u{2003}12']])('n %i', () => {});", "n 12"),
            ("%f of an ideographic-space-padded number",
             "test.each([['\\u{3000}1.5']])('n %f', () => {});", "n 1.5"),
            ("%i truncates a numeric cell, not just a string one",
             "test.each([[1.5]])('n %i', () => {});", "n 1"),
            // A BigInt splits the three directives (measured on 5.0.3): `%d`/`%i` keep the
            // literal's own `n` spelling, `%f` has no BigInt case and parses the string form.
            ("%d of a BigInt keeps its spelling",
             "test.each([[10n]])('n %d', () => {});", "n 10n"),
            ("%i of a BigInt keeps its spelling",
             "test.each([[10n]])('n %i', () => {});", "n 10n"),
            ("%f of a BigInt parses the string form",
             "test.each([[10n]])('n %f', () => {});", "n 10"),

            ("an unrecognized directive is copied verbatim",
             "test.each([[1]])('q %q %s', () => {});", "q %q 1"),
            ("$field names an object row's field",
             "test.each([{ a: 1, b: 2 }])('$a-$b', () => {});", "1-2"),
            ("$0 indexes positionally",
             "test.each([[7, 8]])('idx $0 $1', () => {});", "idx 7 8"),
            ("$field also indexes a tagged-template row's columns",
             "test.each`a | b\n${5} | ${6}`('$a/$b', () => {});", "5/6"),
            ("a bare $ with no field name is a literal",
             "test.each([[1]])('cost $ for %s', () => {});", "cost $ for 1"),
            ("a trailing % is a literal",
             "test.each([[1]])('%s%', () => {});", "1%"),
        ];
        for (name, source, expected) in cases {
            assert_eq!(
                titles_of(source),
                vec![Title::Static((*expected).to_owned())],
                "{name}: `{source}`"
            );
        }

        // A mixed table binds each row whole, but its TITLE formatter spreads an array row
        // anyway -- verified against Vitest 5.0.1, where the callback of the second row receives
        // `[[4, 5]]` (one argument) while the title prints `4` and `5`. Two rows, so this cannot
        // live in the single-title table above.
        assert_eq!(
            titles_of("test.each([1, [4, 5]])('n %s %s', () => {});"),
            vec![
                Title::Static("n 1 undefined".to_owned()),
                Title::Static("n 4 5".to_owned()),
            ],
            "an array row in a mixed table spreads for the title though it binds whole"
        );

        // One declaration per row, each with its own rendered title.
        assert_eq!(
            titles_of("test.each([[1, 'a'], [2, 'b']])('case %s and %s', () => {});"),
            vec![
                Title::Static("case 1 and a".to_owned()),
                Title::Static("case 2 and b".to_owned()),
            ]
        );

        #[rustfmt::skip]
        let degraded: &[(&str, &str)] = &[
            ("an unreadable table", "test.each(getCases())('case %s', () => {});"),
            ("an unreadable cell", "test.each([[mk()]])('case %s', () => {});"),
            ("a dotted field path", "test.each([{ a: { v: 1 } }])('$a.v', () => {});"),
            ("a field the row does not have", "test.each([{ a: 1 }])('$zz', () => {});"),
            ("a positional index past the row", "test.each([[1]])('$3', () => {});"),
            // `%%` is not modelled at all -- Vitest's own handling of it consumes an argument,
            // which is almost certainly a bug upstream. See the `'%'` arm of the renderer.
            ("a literal percent", "test.each([[1]])('100%% of %s', () => {});"),
            // The numeric coercions that yield `NaN` are refused rather than rendered: `NaN` is
            // a title that does not discriminate, the shape behind 18 of `duplicate-each-row`'s
            // 41 false positives on the 2026-09-27 scan. Measured on Vitest 5.0.3, each of
            // these REALLY prints `NaN` there -- the refusal is a choice, documented at
            // `numeric_directive` and in docs/vitest-surface.md §4.
            ("%d of a non-numeric string", "test.each([['a']])('n %d', () => {});"),
            ("%d of a prefixed string does not prefix-parse",
             "test.each([['12abc']])('n %d', () => {});"),
            ("%d of a two-element array", "test.each([[[2, 3]]])('n %d', () => {});"),
            ("%d of a plain object", "test.each([{ a: 1 }])('n %d', () => {});"),
            ("%i of null", "test.each([[null]])('n %i', () => {});"),
            ("%f of true", "test.each([[true]])('n %f', () => {});"),
            ("%d of a true inside an array", "test.each([[[true]]])('n %d', () => {});"),
            ("%d past the end of the row prints NaN in Vitest",
             "test.each([[1]])('n %d %d', () => {});"),
            // The refusal companions of the cases above (each exercises one refusal arm).
            ("%d of a NaN string refuses",
             "test.each([['NaN']])('n %d', () => {});"),
            ("%d of a Rust-spelled inf refuses",
             "test.each([['inf']])('n %d', () => {});"),
            ("%d of trailing garbage inside a hex string refuses",
             "test.each([['0x1g']])('n %d', () => {});"),
            ("%j of an unreadable value refuses",
             "test.each([{ a: mk() }])('n %j', () => {});"),
            ("%o of a too-deep array refuses",
             "test.each([[[[[[[[[[[1]]]]]]]]]]])('n %o', () => {});"),
            ("a spread table is unreadable, not empty",
             "test.each([...getCases()])('n %s', () => {});"),
            ("a named field over a positional row refuses",
             "test.each([[1]])('$a', () => {});"),
            ("a member cell whose object is a call result refuses",
             "test.each([[make().high]])('n %d', () => {});"),
            ("a __proto__ entry in an object cell refuses",
             "test.each([[{ __proto__: 3 }]])('n %o', () => {});"),
            // Copilot review, PR #28, narrowed in round 3: U+0085 is the ONE Unicode
            // whitespace character JavaScript does not trim (Rust's `is_whitespace` does), so a
            // NEL-padded string is `NaN` in Vitest and must refuse rather than trim to a
            // number. Every other Unicode space -- Zs and the line terminators -- IS trimmed.
            // The sign discovery the same round forced, measured in V8: Number's radix
            // grammar takes NO sign (`Number('-0x10')` is NaN), while the old from_str_radix
            // path accepted one inside the digits ('0x-10' rendered -16).
            ("%d of a signed hex string is NaN in Vitest",
             "test.each([['0x-10']])('n %d', () => {});"),
            // Copilot review, PR #28 round 6, verified in node: a bare prefix commits the
            // radix -- `parseInt('0x')`, `parseInt('0xg')`, `parseInt('0x-1')` are all NaN,
            // NOT the prefix's leading 0, which the decimal fallback used to render.
            ("%i of a bare hex prefix refuses",
             "test.each([['0x']])('n %i', () => {});"),
            ("%i of a prefix with no hex digits refuses",
             "test.each([['0xg']])('n %i', () => {});"),
            ("%i of a prefix with a sign and no digits refuses",
             "test.each([['0x-1']])('n %i', () => {});"),
            ("%d of a string around a NEL, which JavaScript does not trim",
             "test.each([['\\u0085 12']])('n %d', () => {});"),
            ("%i of a string around a NEL, ditto",
             "test.each([['\\u0085 12']])('n %i', () => {});"),
        ];
        for (name, source) in degraded {
            let titles = titles_of(source);
            assert!(
                matches!(titles.as_slice(), [Title::Parameterized(_)]),
                "{name}: `{source}` must degrade to the unrendered pattern, got {titles:?}"
            );
        }

        // Two cases that used to sit in the list above and do NOT belong there. Running out of
        // arguments is not a failure in Vitest: it prints `undefined` and carries on, for a
        // positional row and for an object row alike (measured on 5.0.1). Degrading here cost a
        // static title -- and, through `title_is_unresolved`, cost the whole row its
        // comparability -- over a title Vitest renders perfectly well.
        assert_eq!(
            titles_of("test.each([[1]])('%s %s', () => {});"),
            vec![Title::Static("1 undefined".to_owned())],
            "more directives than cells prints `undefined`, it does not degrade"
        );
        assert_eq!(
            titles_of("test.each([{ a: 1 }])('%s %s', () => {});"),
            vec![Title::Static("{ a: 1 } undefined".to_owned())],
            "an object row is one argument, and the second directive gets `undefined`"
        );
    }

    /// T1: a cell that NAMES a value -- an identifier bound to an immutable `const`, or a static
    /// member read off a never-written, never-escaped `const` object -- reads as the value it
    /// names, in every reading (title, identity, substitution). Before T1 each of these degraded
    /// to the unrendered pattern and excluded its row -- every test in it -- from comparison;
    /// identifiers and static members were the two largest readable causes on the census that
    /// sized that row (695 and 243 cells).
    #[test]
    fn const_bound_cells_read_as_the_value_they_name() {
        #[rustfmt::skip]
        let cases: &[(&str, &str, &str)] = &[
            ("a const string", "const MODE = 'fast'; test.each([[MODE]])('runs %s', () => {});", "runs fast"),
            ("a const number under %d", "const N = 3; test.each([[N]])('n %d', () => {});", "n 3"),
            ("a const in a $field cell", "const WHO = 'dev'; test.each([{ who: WHO }])('for $who', () => {});", "for dev"),
            ("a const inside an array row", "const K = 'x'; test.each([[K, 'y']])('%s %s', () => {});", "x y"),
            ("a const as a template substitution",
             "const P = 'v'; test.each([[`${P}-1`]])('%s', () => {});", "v-1"),
            ("a negated const", "const N = 5; test.each([[-N]])('%s', () => {});", "-5"),
            ("a chain of consts", "const A = 'x'; const B = A; test.each([[B]])('%s', () => {});", "x"),
            // Copilot review, PR #28: a const-bound BigInt is classified on its RESOLVED
            // expression, so it renders exactly as a direct `10n` cell does.
            ("a const BigInt under %d keeps its spelling",
             "const N = 10n; test.each([[N]])('n %d', () => {});", "n 10n"),
            // Copilot CLI review of PR #28: numeric_value dropped resolution, so a negated
            // const-bound zero missed the `%s` sign special case and rendered `0` where Vitest
            // prints `-0`. A bare `-0` cell is pinned beside it, guarding the case itself.
            ("a negated const zero keeps the %s sign",
             "const N = 0; test.each([[-N]])('n %s', () => {});", "n -0"),
            // The same read, one statement later, is legitimate: the declaration is
            // initialized before the table is built, and both rows render -- identical here,
            // which is exactly what duplicate-each-row is for.
            ("a multi-declarator statement reads only what is initialized before it",
             "const A = 1, B = A; test.each([[B]])('%s', () => {});", "1"),
            // Shorthand `{ __proto__ }` (no colon, key and value identical) is a real own
            // data property, unlike every colon form -- even `{ __proto__: V }` is the setter.
            ("a shorthand __proto__ data property reads",
             "const __proto__ = 3; const L = { __proto__ }; test.each([[L.__proto__]])('%d', () => {});", "3"),
            ("a bare negative-zero cell keeps the %s sign",
             "test.each([[-0]])('n %s', () => {});", "n -0"),
            ("a const BigInt under %i keeps its spelling",
             "const N = 10n; test.each([[N]])('n %i', () => {});", "n 10n"),
            ("a const concatenated into a string cell",
             "const P = 'a'; test.each([[P + 'b']])('%s', () => {});", "ab"),
            // Copilot review, PR #28 round 3: nested identifiers inside an initializer's
            // expression used to read as "not a literal" and refuse the whole peel -- and with
            // the peel gone the fold never saw the initializer at all, since it only receives
            // the cell's own expression. Both shapes below rendered as refusals before.
            ("a const built from other consts",
             "const A = 'x'; const B = A + 'b'; test.each([[B]])('%s', () => {});", "xb"),
            ("a member value built from other consts",
             "const A = 'x'; const T = { k: A + 'b' }; test.each([[T.k]])('%s', () => {});", "xb"),
            ("a const template built from other consts",
             "const A = 'x'; const G = `${A}!`; test.each([[G]])('%s', () => {});", "x!"),
            ("a static member of a const object",
             "const LEVELS = { high: 3 }; test.each([[LEVELS.high]])('n %d', () => {});", "n 3"),
            ("a method VALUE detached without a call keeps the object readable",
             "const LEVELS = { high: 3, mutate: function () { this.high = 4 } };\nconst m = LEVELS.mutate;\ntest.each([[LEVELS.high]])('n %d', () => {});", "n 3"),
            ("a member read is safe when a method runs on a DIFFERENT object",
             "const LEVELS = { high: 3 };\nconst other = { ping() { return 1 } };\nother.ping();\ntest.each([[LEVELS.high]])('n %d', () => {});", "n 3"),
            ("a member read passed as a call argument keeps the object readable",
             "const LEVELS = { high: 3 };\nreport(LEVELS.high);\ntest.each([[LEVELS.high]])('n %d', () => {});", "n 3"),
            ("a static member read more than once in the file",
             "const LEVELS = { high: 3, low: 1 };\nexpect(LEVELS.low).toBe(1);\ntest.each([[LEVELS.high]])('n %d', () => {});", "n 3"),
            ("a static member whose value is itself a const",
             "const HIGH = 3; const LEVELS = { high: HIGH }; test.each([[LEVELS.high]])('n %d', () => {});", "n 3"),
            ("a static string member through $field",
             "const NAMES = { admin: 'root' }; test.each([{ x: NAMES.admin }])('$x', () => {});", "root"),
        ];
        for (name, source, expected) in cases {
            assert_eq!(
                titles_of(source),
                vec![Title::Static((*expected).to_owned())],
                "{name}: `{source}`"
            );
        }

        // The same read, one statement later, is legitimate: the declaration is initialized
        // before the table is built, and both rows render -- identical here, which is exactly
        // what duplicate-each-row is for. The TDZ refusals above are the same shape with the
        // declaration moved after the table.
        assert_eq!(
            titles_of("const MODE = 'fast'; test.each([[MODE], ['fast']])('n %s', () => {});"),
            vec![
                Title::Static("n fast".to_owned()),
                Title::Static("n fast".to_owned()),
            ],
            "a declaration before the table backs the read; both rows render"
        );
    }

    /// T1's refusals, each one a way the named value could differ at runtime from what source
    /// alone says -- the difference between following a binding and guessing.
    #[test]
    fn const_bound_cells_refuse_everything_that_could_change() {
        #[rustfmt::skip]
        let degraded: &[(&str, &str)] = &[
            // Reassignment makes the initializer evidence, not a value.
            ("a let binding", "let MODE = 'fast'; test.each([[MODE]])('runs %s', () => {});"),
            ("a var binding (hoisting)", "var MODE = 'fast'; test.each([[MODE]])('runs %s', () => {});"),
            ("an un-reassigned let", "let MODE = 'fast'; test.each([[MODE]])('runs %s', () => {});"),
            // A `const` initializer that is itself a runtime value.
            ("a call initializer", "const MODE = mk(); test.each([[MODE]])('runs %s', () => {});"),
            // A composite initializer can be mutated after declaration (`const L = [1];
            // L.push(2)`), so the declared shape is not the value the table constructs from.
            ("a composite const as a whole cell", "const ROWS = [1, 2]; test.each([[ROWS]])('%o', () => {});"),
            ("a member off a composite-valued const chain",
             "const L = { n: 1 }; const M = L; test.each([[M.n]])('%d', () => {});"),
            // A const object that is written through anywhere in the file.
            ("a member write on the const object",
             "const LEVELS = { high: 3 }; LEVELS.high = 4; test.each([[LEVELS.high]])('n %d', () => {});"),
            // Copilot review, PR #28 round 5: a METHOD CALL on the object binds `this` to it,
            // so the method can mutate through references that are all in member-object
            // position -- the one shape the write-through and escape checks both missed.
            ("a method call on the const object",
             "const LEVELS = { high: 3, mutate: function () { this.high = 4 } };\nLEVELS.mutate();\ntest.each([[LEVELS.high]])('n %d', () => {});"),
            ("a method call on the const object, optional chain",
             "const LEVELS = { high: 3, mutate: function () { this.high = 4 } };\nLEVELS.mutate?.();\ntest.each([[LEVELS.high]])('n %d', () => {});"),
            // Copilot review, PR #28 round 6, verified in node: a TAGGED TEMPLATE binds `this`
            // exactly like a call -- `LEVELS.mutate`` ` `` moved high to 4.
            ("a method invoked as a tagged-template tag",
             "const LEVELS = { high: 3, mutate: function () { this.high = 4 } };\nLEVELS.mutate``;\ntest.each([[LEVELS.high]])('n %d', () => {});"),
            // A GETTER fires when its key is read: the plain member read `LEVELS.other` is
            // allowed by the escape rules, and it ran the getter, which moved high to 4.
            ("a getter on another key invalidates the object",
             "const LEVELS = { get other() { this.high = 4 }, high: 3 };\nLEVELS.other;\ntest.each([[LEVELS.high]])('n %d', () => {});"),
            // A non-shorthand __proto__ entry is the prototype SETTER: the runtime object has
            // no own keys, and L.__proto__ reads Object.prototype -- not 3.
            ("a __proto__ initializer is a prototype setter, not a property",
             "const L = { __proto__: 3 }; test.each([[L.__proto__]])('%d', () => {});"),
            ("a delete on the const object",
             "const LEVELS = { high: 3 }; delete LEVELS.high; test.each([[LEVELS.high]])('n %d', () => {});"),
            ("an update on the const object",
             "const LEVELS = { high: 3 }; LEVELS.high++; test.each([[LEVELS.high]])('n %d', () => {});"),
            // A const object that escapes to a call anywhere: this cannot see through the call,
            // and `Object.assign(LEVELS, ...)` would make the literal stale.
            ("a const object passed to a call",
             "const LEVELS = { high: 3 };\nconfigure(LEVELS);\ntest.each([[LEVELS.high]])('n %d', () => {});"),
            ("a const object aliased",
             "const LEVELS = { high: 3 }; const other = LEVELS; test.each([[LEVELS.high]])('n %d', () => {});"),
            // Destructured and imported bindings: the declaration is not a `const` declarator
            // with a plain identifier pattern.
            ("a destructured const", "const { a } = { a: 1 }; test.each([[a]])('%s', () => {});"),
            // Copilot review, PR #28 round 4, premise verified in node: the table evaluates
            // where it is written, so a cell read of a const declared LATER is a TDZ
            // ReferenceError -- Vitest registers no tests at all, and folding the row would
            // report a duplicate that never exists. Both arms refuse.
            ("a forward member read of a later const object",
             "test.each([[L.k]])('n %s', () => {}); const L = { k: 1 };"),
            ("a member read off a destructured binding",
             "const { a } = { foo: 1, a: 2 }; test.each([[a.foo]])('%d', () => {});"),
            ("an imported binding", "import { MODE } from './mode'; test.each([[MODE]])('runs %s', () => {});"),
            // The member name itself must be a static key on the object.
            ("a computed key could shadow the member",
             "const k = 'high'; const LEVELS = { high: 3, [k]: 4 }; test.each([[LEVELS.high]])('n %d', () => {});"),
        ];
        for (name, source) in degraded {
            let titles = titles_of(source);
            assert!(
                matches!(titles.as_slice(), [Title::Parameterized(_)]),
                "{name}: `{source}` must degrade to the unrendered pattern, got {titles:?}"
            );
        }

        // The forward read is PER-ROW, not whole-declaration: the MODE row refuses (its value
        // is a TDZ read -- the file throws here at runtime and registers no tests), while the
        // literal row beside it renders exactly as Vitest would.
        assert_eq!(
            titles_of("test.each([[MODE], ['fast']])('n %s', () => {}); const MODE = 'fast';"),
            vec![
                Title::Parameterized("n %s".to_owned()),
                Title::Static("n fast".to_owned()),
            ],
            "only the forward read degrades; the literal row is unaffected"
        );
    }

    /// The safety property the whole feature rests on: binding resolution is by symbol, never by
    /// name, so two sibling scopes that each declare their own same-named `const` resolve to
    /// their own values. A name-based implementation renders both rows identically and reports
    /// two genuinely different tests as one case.
    #[test]
    fn const_bound_cells_resolve_by_symbol_not_by_name() {
        assert_eq!(
            titles_of(
                "describe('a', () => { const MODE = 'x'; test.each([[MODE]])('runs %s', () => {}); });\n\
                 describe('b', () => { const MODE = 'y'; test.each([[MODE]])('runs %s', () => {}); });"
            ),
            vec![
                // `describe` records its own Suite title first; the tests follow their suites.
                Title::Static("a".to_owned()),
                Title::Static("runs x".to_owned()),
                Title::Static("b".to_owned()),
                Title::Static("runs y".to_owned()),
            ],
            "each row reads its own scope's binding, so the titles must differ"
        );

        // And the outer-scope case: an inner binding shadows an outer one, and the cell in the
        // inner scope must observe the inner value.
        assert_eq!(
            titles_of(
                "const MODE = 'outer';\n\
                 describe('inner', () => { const MODE = 'inner'; test.each([[MODE]])('runs %s', () => {}); });\n\
                 test.each([[MODE]])('runs %s', () => {});"
            ),
            vec![
                Title::Static("inner".to_owned()),
                Title::Static("runs inner".to_owned()),
                Title::Static("runs outer".to_owned()),
            ],
            "shadowing resolves to the innermost binding"
        );
    }

    // -----------------------------------------------------------------------------------------
    // Modifiers
    // -----------------------------------------------------------------------------------------

    /// Extract one declaration with ambient globals enabled and return its modifiers, asserting
    /// a single result.
    fn modifiers_of(source: &str) -> Vec<Modifier> {
        let extraction = extract_source(source, "case.test.ts", Globals::Enabled);
        assert_eq!(extraction.tests.len(), 1, "expected one declaration");
        extraction.tests.into_iter().next().expect("one").modifiers
    }

    /// Check declaration extraction: every member modifier and its options object twin are
    /// recognized.
    #[test]
    fn every_member_modifier_and_its_options_object_twin_are_recognized() {
        #[rustfmt::skip]
        let members: &[(&str, Modifier)] = &[
            ("skip", Modifier::Skip),
            ("only", Modifier::Only),
            ("todo", Modifier::Todo),
            ("fails", Modifier::Fails),
            ("concurrent", Modifier::Concurrent),
            // Removed in v5, but pre-v5 corpora still contain it: it must surface as a real
            // modifier rather than be swallowed into `Unknown`.
            ("sequential", Modifier::Sequential),
            ("shuffle", Modifier::Shuffle),
        ];
        for (member, expected) in members {
            assert!(
                modifiers_of(&format!("test.{member}('x', () => {{}});")).contains(expected),
                "`test.{member}` must surface as {expected:?}"
            );
        }
        assert_eq!(
            modifiers_of("test.frobnicate('x', () => {});"),
            vec![Modifier::Unknown("frobnicate".to_owned())],
            "an unrecognized member is recorded by name, not dropped"
        );

        // The options-object spelling of the same five, and the shapes it must ignore.
        #[rustfmt::skip]
        let options: &[(&str, Modifier)] = &[
            ("skip", Modifier::Skip),
            ("only", Modifier::Only),
            ("todo", Modifier::Todo),
            ("fails", Modifier::Fails),
            ("concurrent", Modifier::Concurrent),
        ];
        for (key, expected) in options {
            assert!(
                modifiers_of(&format!("test('x', {{ {key}: true }}, () => {{}});"))
                    .contains(expected),
                "`{{ {key}: true }}` must surface as {expected:?}"
            );
        }
        #[rustfmt::skip]
        let ignored: &[(&str, &str)] = &[
            ("a false value", "test('x', { skip: false }, () => {});"),
            ("a non-boolean value", "test('x', { skip: 1 }, () => {});"),
            ("an unrecognized key", "test('x', { retry: true }, () => {});"),
            ("a computed key", "test('x', { [k]: true }, () => {});"),
            ("a spread", "test('x', { ...opts }, () => {});"),
        ];
        for (name, source) in ignored {
            assert!(
                modifiers_of(source).is_empty(),
                "{name}: `{source}` declares no modifier"
            );
        }
        // The member and the options object agree rather than double-counting.
        assert_eq!(
            modifiers_of("test.skip('x', { skip: true }, () => {});"),
            vec![Modifier::Skip]
        );
    }

    // -----------------------------------------------------------------------------------------
    // The `function` callback form, and the module flavor
    // -----------------------------------------------------------------------------------------

    /// Check declaration extraction: a function callback is extracted like an arrow one.
    #[test]
    fn a_function_callback_is_extracted_like_an_arrow_one() {
        let arrow = extract_source(
            "describe('s', () => { beforeEach(() => { g(); }); test('x', () => { f(); }); });",
            "case.test.ts",
            Globals::Enabled,
        );
        let function = extract_source(
            "describe('s', function () { beforeEach(function () { g(); }); \
             test('x', function () { f(); }); });",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(function.tests.len(), arrow.tests.len(), "same declarations");
        assert_eq!(
            function.tests[1].body, arrow.tests[1].body,
            "a `function` test body fingerprints like its arrow twin"
        );
        assert_eq!(
            function.tests[1].context.hooks.len(),
            1,
            "and its hooks are still collected"
        );
        assert_eq!(
            function.tests[1].context.hooks[0].body, arrow.tests[1].context.hooks[0].body,
            "...with the same hook body fingerprint"
        );
    }

    /// Check declaration extraction: a forced commonjs file is still extracted.
    #[test]
    fn a_forced_commonjs_file_is_still_extracted() {
        // `.cjs`/`.cts` pin the module flavor, which changes the `SourceType` handed to the
        // parser. Declarations must still be found.
        for path in ["case.test.cjs", "case.test.cts"] {
            let extraction = extract_source("test('x', () => { f(); });", path, Globals::Enabled);
            assert_eq!(
                extraction.tests.len(),
                1,
                "`{path}` must still yield its declaration"
            );
            assert!(extraction.diagnostics.is_empty(), "for `{path}`");
        }
    }

    // -----------------------------------------------------------------------------------------
    // Degrading rather than dropping
    // -----------------------------------------------------------------------------------------
    //
    // Every path below reports *less* than a clean extraction while saying so, because the worst
    // failure mode this tool has is a clean run that quietly analyzed less than it claimed to.
    // Each row asserts the diagnostic or the `incomplete`/`unresolved` flag, not merely that the
    // call did not panic.

    /// Check declaration extraction: an unsupported extension is reported rather than extracted
    /// as nothing.
    #[test]
    fn an_unsupported_extension_is_reported_rather_than_extracted_as_nothing() {
        let extraction = extract(
            "test('x', () => {});",
            &SourceFile {
                path: PathBuf::from("case.test.txt"),
                language: SourceLanguage::TypeScript,
                module_flavor: ModuleFlavor::Ambiguous,
                config_scope: None,
                project_globals: None,
            },
            Globals::Enabled,
        );
        assert!(extraction.tests.is_empty());
        assert!(
            extraction.incomplete,
            "an unparsed file is not proven empty"
        );
        assert!(
            extraction
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("unsupported source type")),
            "got {:?}",
            extraction.diagnostics
        );
        assert!(!extraction.proven_empty);
    }

    /// Check declaration extraction: recoverable parse and semantic diagnostics are counted and
    /// reported.
    #[test]
    fn recoverable_parse_and_semantic_diagnostics_are_counted_and_reported() {
        // A top-level `return` is a recoverable parse error: the parser recovers and the
        // declaration beside it is still analyzed, but the diagnostic has to reach the report.
        let extraction = extract_source(
            "return 1; test('x', () => {});",
            "case.test.ts",
            Globals::Enabled,
        );
        assert!(
            extraction
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("parse diagnostic(s)")),
            "got {:?}",
            extraction.diagnostics
        );

        // A redeclared binding is a semantic diagnostic, not a parse one.
        let extraction = extract_source(
            "let a; let a; test('x', () => {});",
            "case.test.ts",
            Globals::Enabled,
        );
        assert!(
            extraction
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("semantic diagnostic(s)")),
            "got {:?}",
            extraction.diagnostics
        );
        assert_eq!(
            extraction.tests.len(),
            1,
            "the declaration is still extracted"
        );
    }

    /// The bracket-nesting pre-scan runs over every source, and it has to mirror what the real
    /// tokenizer does with strings, template literals, and their escapes -- over-counting real
    /// code brackets is safe, undercounting a pathological file is not.
    #[test]
    fn the_nesting_pre_scan_survives_strings_templates_and_escapes() {
        #[rustfmt::skip]
        let sources: &[(&str, &str)] = &[
            ("an escape inside template text", r"test('x', () => { const t = `a\`b`; });"),
            ("a template hole containing braces", "test('x', () => { const t = `a${ {b: 1} }c`; });"),
            ("nested template literals", "test('x', () => { const t = `a${ `b${1}c` }d`; });"),
            ("an escape inside a quoted string", r"test('x', () => { const s = 'a\'b'; });"),
            ("brackets inside a string", "test('x', () => { const s = '((((('; });"),
            ("brackets inside template text", "test('x', () => { const s = `((((( `; });"),
        ];
        for (name, source) in sources {
            let extraction = extract_source(source, "case.test.ts", Globals::Enabled);
            assert_eq!(
                extraction.tests.len(),
                1,
                "{name}: `{source}` must still be analyzed: {:?}",
                extraction.diagnostics
            );
        }

        // Brackets masked inside a string do not count toward the depth cap, so a file that is
        // only "deep" inside a string literal is still analyzed...
        let masked = format!(
            "test('x', () => {{ const s = '{}'; }});",
            "(".repeat(MAX_NESTING_DEPTH + 10)
        );
        let extraction = extract_source(&masked, "case.test.ts", Globals::Enabled);
        assert_eq!(
            extraction.tests.len(),
            1,
            "brackets inside a string literal are not real nesting"
        );
        // Template text masks brackets the same way, and the escape inside it is what keeps a
        // `\`` from being mistaken for the closing backtick -- without it the masking stops
        // early and everything after it is counted as real nesting.
        let in_template = format!(
            "test('x', () => {{ const t = `{}`; }});",
            "(".repeat(MAX_NESTING_DEPTH + 10)
        );
        assert_eq!(
            extract_source(&in_template, "case.test.ts", Globals::Enabled)
                .tests
                .len(),
            1,
            "brackets inside template text are not real nesting"
        );
        let after_escape = format!(
            "test('x', () => {{ const t = `a\\`{}`; }});",
            "(".repeat(MAX_NESTING_DEPTH + 10)
        );
        assert_eq!(
            extract_source(&after_escape, "case.test.ts", Globals::Enabled)
                .tests
                .len(),
            1,
            "an escaped backtick does not end the template, so what follows stays masked"
        );

        // ...while genuine nesting past the cap is refused, with a diagnostic.
        let deep = format!(
            "test('x', () => {{ const v = {}1{}; }});",
            "[".repeat(MAX_NESTING_DEPTH + 10),
            "]".repeat(MAX_NESTING_DEPTH + 10)
        );
        let extraction = extract_source(&deep, "case.test.ts", Globals::Enabled);
        assert!(extraction.tests.is_empty() && extraction.incomplete);
        assert!(
            !extraction.diagnostics.is_empty(),
            "a refused file must say so"
        );
    }

    // -----------------------------------------------------------------------------------------
    // `vi.mock` / `vi.unmock` bookkeeping
    // -----------------------------------------------------------------------------------------

    /// Extract one test and return its retained mock identities and unresolved-context flag.
    fn mock_context(source: &str) -> (Vec<MockedModule>, bool) {
        let extraction = extract_source(source, "case.test.ts", Globals::Enabled);
        assert_eq!(extraction.tests.len(), 1, "expected one declaration");
        let test = extraction.tests.into_iter().next().expect("one");
        (test.context.mocked_modules, test.context.unresolved)
    }

    /// Check declaration extraction: a mocking call this extractor cannot read taints the whole
    /// file.
    #[test]
    fn a_mocking_call_this_extractor_cannot_read_taints_the_whole_file() {
        // The control: a readable `vi.mock` is recorded, and nothing is tainted.
        let (modules, unresolved) =
            mock_context("import { vi } from 'vitest'; vi.mock('./m'); test('x', () => {});");
        assert_eq!(modules.len(), 1);
        assert!(!unresolved);

        #[rustfmt::skip]
        let tainting: &[(&str, &str)] = &[
            ("a mock target that is not a literal", "vi.mock(specifier);"),
            ("an unmock target that is not a literal", "vi.unmock(specifier);"),
            // The two non-hoisted members: they run in statement order, so folding them into the
            // hoisted event list would claim an ordering Vitest does not give them.
            ("doMock", "vi.doMock('./m');"),
            ("doUnmock", "vi.doUnmock('./m');"),
        ];
        for (name, call) in tainting {
            let (_, unresolved) = mock_context(&format!(
                "import {{ vi }} from 'vitest'; {call} test('x', () => {{}});"
            ));
            assert!(
                unresolved,
                "{name}: `{call}` cannot be read, so no mock in the file can be trusted"
            );
        }

        // A `vi.` call that is not a mocking member at all is simply not a mocking call.
        let (modules, unresolved) =
            mock_context("import { vi } from 'vitest'; vi.spyOn(o, 'm'); test('x', () => {});");
        assert!(modules.is_empty() && !unresolved);

        // Neither is a top-level expression statement that is not a call.
        let (modules, unresolved) =
            mock_context("import { vi } from 'vitest'; 1; test('x', () => {});");
        assert!(modules.is_empty() && !unresolved);

        // A side-effect-only import binds no local symbol for `Context::imports`.
        let (modules, _) = mock_context(
            "import './side';\nimport { vi } from 'vitest'; vi.mock('./m'); test('x', () => {});",
        );
        assert_eq!(modules.len(), 1, "and changes nothing about the mock list");
    }

    // -----------------------------------------------------------------------------------------
    // Bindings introduced outside a plain import
    // -----------------------------------------------------------------------------------------

    /// Check declaration extraction: an import meta vitest destructure binds only the names it
    /// can canonicalize.
    #[test]
    fn an_import_meta_vitest_destructure_binds_only_the_names_it_can_canonicalize() {
        let extraction = extract_source(
            "if (import.meta.vitest) { const { test: t, notAnExport: n, [k]: c } = \
             import.meta.vitest; t('x', () => {}); }",
            "case.test.ts",
            Globals::Disabled,
        );
        assert_eq!(
            extraction.tests.len(),
            1,
            "`t` is the canonicalized `test`: {:?}",
            extraction.diagnostics
        );

        // A name that is not a Vitest export, and a computed key, bind nothing -- so calling them
        // is not a declaration.
        let extraction = extract_source(
            "if (import.meta.vitest) { const { notAnExport: n } = import.meta.vitest; \
             n('x', () => {}); }",
            "case.test.ts",
            Globals::Disabled,
        );
        assert_eq!(extraction.tests.len(), 0);
    }

    /// Check declaration extraction: a local alias binds only when it resolves to test.
    #[test]
    fn a_local_alias_binds_only_when_it_resolves_to_test() {
        // `const myTest = test.extend({...})` resolves exactly like `test` did, fixtures included.
        let extraction = extract_source(
            "import { test } from 'vitest'; const myTest = test.extend({ db: 1 }); \
             myTest('x', () => {});",
            "case.test.ts",
            Globals::Disabled,
        );
        assert_eq!(extraction.tests.len(), 1);
        assert_eq!(
            extraction.tests[0].context.fixtures.len(),
            1,
            "the alias carries the fixtures it was extended with"
        );

        // A fixture-configuration call on an alias carries no title or body, so it is not a
        // declaration of its own -- and neither is a hook scoped to one.
        let extraction = extract_source(
            "import { test } from 'vitest'; const myTest = test.extend({ db: 1 }); \
             myTest.extend({ other: 2 }); myTest.scoped({ db: 3 }); \
             myTest.beforeEach(() => { g(); }); myTest('x', () => {});",
            "case.test.ts",
            Globals::Disabled,
        );
        assert_eq!(
            extraction.tests.len(),
            1,
            "only the `myTest('x', ...)` call is a declaration: {:?}",
            extraction
                .tests
                .iter()
                .map(|test| test.title.clone())
                .collect::<Vec<_>>()
        );

        // An initializer that resolves to something other than `test` binds nothing...
        let extraction = extract_source(
            "import { describe } from 'vitest'; const d = describe; d('x', () => {});",
            "case.test.ts",
            Globals::Disabled,
        );
        assert_eq!(
            extraction.tests.len(),
            0,
            "only a `test`-rooted alias is bound here"
        );
        // ...and neither does one that resolves to nothing at all.
        let extraction = extract_source(
            "const x = mk(); x('y', () => {});",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(extraction.tests.len(), 0);
    }

    /// Check declaration extraction: a hook with no callback registers nothing.
    #[test]
    fn a_hook_with_no_callback_registers_nothing() {
        let extraction = extract_source(
            "beforeEach(1); test('x', () => {});",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(extraction.tests.len(), 1);
        assert!(
            extraction.tests[0].context.hooks.is_empty(),
            "there is no body to fingerprint"
        );
    }

    // -----------------------------------------------------------------------------------------
    // Suite and declaration shapes
    // -----------------------------------------------------------------------------------------

    /// Check declaration extraction: a named function callback supplies its own title.
    #[test]
    fn a_named_function_callback_supplies_its_own_title() {
        // `test(function myTestName() { ... })`: the title is the function's name, and the
        // function is the body -- it is never invoked to produce a title.
        let extraction = extract_source(
            "test(function myTestName() { f(); });",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(extraction.tests.len(), 1);
        assert_eq!(
            extraction.tests[0].title,
            Title::Static("myTestName".to_owned())
        );
        // An *anonymous* function argument has no name to take, so it falls through to the
        // ordinary title/body split and has no title at all.
        let extraction = extract_source(
            "test(function () { f(); });",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(extraction.tests.len(), 1);
        assert!(
            matches!(extraction.tests[0].title, Title::Dynamic(_)),
            "got {:?}",
            extraction.tests[0].title
        );
    }

    /// Check declaration extraction: an empty suite body and a concise one are both handled.
    #[test]
    fn an_empty_suite_body_and_a_concise_one_are_both_handled() {
        // An empty `describe` body has nothing to recurse into, so only the suite is recorded.
        let extraction =
            extract_source("describe('s', () => {});", "case.test.ts", Globals::Enabled);
        assert_eq!(extraction.tests.len(), 1);
        assert_eq!(extraction.tests[0].kind, DeclarationKind::Suite);

        // A concise-body arrow callback holds exactly one expression, which is treated the same
        // way a call inside an ordinary block would be.
        let extraction = extract_source(
            "describe('s', () => test('x', () => { f(); }));",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(extraction.tests.len(), 2);
        assert_eq!(extraction.tests[1].kind, DeclarationKind::Test);
        assert_eq!(
            extraction.tests[1].suite_path,
            vec![Title::Static("s".to_owned())],
            "the nested test is still inside the suite"
        );
    }

    /// Check declaration extraction: a suite each table that cannot be read yields one
    /// representative suite.
    #[test]
    fn a_suite_each_table_that_cannot_be_read_yields_one_representative_suite() {
        // A known-empty table registers zero suites at runtime, so extracting one would be a
        // synthetic suite with no counterpart.
        let extraction = extract_source(
            "describe.each([])('s $a', () => { test('x', () => {}); });",
            "case.test.ts",
            Globals::Enabled,
        );
        assert!(
            extraction.tests.is_empty(),
            "a proven-empty table registers nothing: {:?}",
            extraction.tests
        );

        // An *unreadable* table's instance count is unknown, which is not the same as zero:
        // exactly one representative instance, marked unresolved.
        let extraction = extract_source(
            "describe.each(getCases())('s $a', () => { test('x', () => {}); });",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(extraction.tests.len(), 2, "the suite and its nested test");
        assert_eq!(
            extraction.tests[0].title,
            Title::Parameterized("s $a".to_owned()),
            "the pattern is kept unrendered rather than guessed at"
        );
        assert!(extraction.tests[0].context.unresolved);
    }

    /// A `for (const x of [literal, ...])` loop is unrolled, so a title built from the loop
    /// variable renders the value for *this* iteration instead of degrading to `Dynamic`. Every
    /// shape that cannot be enumerated falls back to walking the body once, unrolled.
    #[test]
    fn a_literal_for_of_loop_is_unrolled_for_title_rendering() {
        let extraction = extract_source(
            "for (const c of ['a', 'b']) { test(`case ${c}`, () => { f(); }); }",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(
            extraction
                .tests
                .iter()
                .map(|test| test.title.clone())
                .collect::<Vec<_>>(),
            vec![
                Title::Static("case a".to_owned()),
                Title::Static("case b".to_owned()),
            ],
            "each iteration renders its own title"
        );

        #[rustfmt::skip]
        let not_enumerable: &[(&str, &str)] = &[
            ("a destructuring loop variable", "for (const [c] of [[1]]) { test(`case ${c}`, () => {}); }"),
            ("an assignment target", "let c; for (c of [1, 2]) { test(`case ${c}`, () => {}); }"),
            ("a non-array iterable", "for (const c of cases) { test(`case ${c}`, () => {}); }"),
            ("a spread element", "for (const c of [...xs]) { test(`case ${c}`, () => {}); }"),
            ("an elided element", "for (const c of [1, , 2]) { test(`case ${c}`, () => {}); }"),
            ("an element that does not fold", "for (const c of [mk()]) { test(`case ${c}`, () => {}); }"),
            ("an empty array", "for (const c of []) { test(`case ${c}`, () => {}); }"),
        ];
        for (name, source) in not_enumerable {
            let extraction = extract_source(source, "case.test.ts", Globals::Enabled);
            assert_eq!(
                extraction.tests.len(),
                1,
                "{name}: the body is walked exactly once, unrolled"
            );
            assert!(
                matches!(extraction.tests[0].title, Title::Dynamic(_)),
                "{name}: the title cannot be rendered, so it must not claim to be: {:?}",
                extraction.tests[0].title
            );
        }
    }

    /// `.runIf` is `.skipIf`'s twin and carries the same condition identity; both must reach the
    /// modifier list, since `analysis.rs`'s `ModifierKey` treats them as behavior-significant.
    #[test]
    fn both_conditional_modifiers_carry_their_condition() {
        let skip_if = modifiers_of("test.skipIf(isCI)('x', () => {});");
        assert_eq!(
            skip_if,
            vec![Modifier::SkipIf("<global:isCI>".to_owned())],
            "`.skipIf`"
        );
        let run_if = modifiers_of("test.runIf(isCI)('x', () => {});");
        assert_eq!(
            run_if,
            vec![Modifier::RunIf("<global:isCI>".to_owned())],
            "`.runIf`"
        );
        assert_ne!(skip_if, run_if, "the two are not interchangeable");

        // A composite condition remains unknown in value but comparable by syntax and bindings.
        let extraction = extract_source(
            "test.skipIf(process.platform === 'win32')('x', () => {});",
            "case.test.ts",
            Globals::Enabled,
        );
        assert!(
            !extraction.tests[0].context.unresolved,
            "a stable expression identity must not taint the whole context"
        );
    }

    /// The `.each` title *pattern* is read from the title argument's own source before any row is
    /// rendered, so the pattern has to survive every spelling a title can take.
    #[test]
    fn an_each_title_pattern_is_read_from_every_title_spelling() {
        assert_eq!(
            titles_of("test.each([[1]])(`case %s`, () => {});"),
            vec![Title::Static("case 1".to_owned())],
            "a no-substitution template is a usable pattern"
        );
        // CHANGED by S7, deliberately: these three used to assert `Title::Static`. That was a
        // claim the extractor could not support -- `Title::is_static` means "the runtime value is
        // statically known", and the runtime value of `mk()` is exactly what this extractor did
        // not read. Calling it `Static` made the title half of `duplicate-each-row`'s two-part
        // claim ("identical RENDERED title and substituted body") unconditionally true for every
        // row of such a declaration, which is 18 of that rule's 41 false positives on the
        // 2026-09-27 scan. An unread pattern is `Parameterized`: kept verbatim, asserted of
        // nothing.
        //
        // What the test was really protecting is untouched and still checked below: the pattern
        // keeps that argument's OWN source text, so two different unreadable patterns stay apart
        // rather than collapsing into one title.
        assert_eq!(
            titles_of("test.each([[1]])(mk(), () => {});"),
            vec![Title::Parameterized("mk()".to_owned())],
            "an unreadable pattern keeps its source text, unrendered"
        );
        assert_ne!(
            titles_of("test.each([[1]])(mkA(), () => {});"),
            titles_of("test.each([[1]])(mkB(), () => {});"),
            "two different unreadable patterns must not collapse to one title"
        );
        // `undefined` is an identifier, not a literal, and nothing here proves it is the global
        // one rather than a shadowing binding -- so it is unread like any other identifier.
        assert_eq!(
            titles_of("test.each([[1]])(undefined, () => {});"),
            vec![Title::Parameterized("undefined".to_owned())],
        );
        // No title argument at all leaves an empty pattern, which renders as an empty title
        // rather than being mistaken for a missing row.
        assert_eq!(
            titles_of("test.each([[1]])();"),
            vec![Title::Static(String::new())],
        );
    }

    // ---------------------------------------------------------------------------------------------
    // Rows specified by the 2026-09-27 tests-implementation plan (S7/S9/S11 blocks)
    // ---------------------------------------------------------------------------------------------

    /// The behavior trace of one extracted test, for the S9 rows. `fingerprint.rs`'s `BehaviorCollector`
    /// is the producer; this module owns the public extraction path that assembles it into a
    /// `TestCase`, so the pin rides the same surface every rule consumes.
    fn behavior_of(source: &str) -> crate::model::BehaviorTrace {
        extract_source(source, "case.test.ts", Globals::Enabled)
            .tests
            .into_iter()
            .next()
            .expect("source declares exactly one test")
            .body
            .behavior
    }

    /// R7: the sibling of the `%%` arm in `try_render_each_title`. A `.each` title pattern carrying an
    /// unresolvable `${...}` substitution must degrade to `Title::Parameterized` -- every row of the
    /// declaration shares the unrendered pattern by construction, so rendering "succeeds" only by
    /// handing the reader a template nobody evaluated. Today the raw template text passes through as
    /// `Title::Static`; ignored until the S7 fix lands, then it guards the decline.
    #[test]
    fn try_render_each_title_declines_a_pattern_with_an_unresolved_substitution() {
        let titles = titles_of("test.each([[1]])(`ready ${mk()}`, (v) => {});");
        assert!(
            matches!(titles.as_slice(), [Title::Parameterized(_)]),
            "an unfoldable substitution must degrade to the unrendered pattern, got {titles:?}"
        );
    }

    /// R12 (S9): a chai-style property-chain assertion must reach the behavior trace, with its
    /// polarity. `expect(spy).to.not.have.been.calledOnce` and `expect(spy).to.have.been.calledTwice`
    /// are opposite assertions on the same spy; reporting them as "the same behavior events in the
    /// same order" (which is what equal traces produce) was the adjudicated false positive. A chain
    /// has no call node, so nothing but the member names distinguishes them, and nothing used to
    /// carry those names into the trace. `fingerprint::BehaviorCollector::property_chain_event`
    /// now emits one `assert:chain:` event per chain, recorded verbatim so `not` travels with it;
    /// this guards both halves from the public extraction path: recorded at all, and
    /// `not`-polarity distinct from plain.
    #[test]
    fn behavior_trace_records_a_property_chain_assertion_and_its_polarity() {
        let negated = behavior_of(
            "test('records two calls', () => { const spy = vi.fn(); spy(); spy(); \
         expect(spy).to.not.have.been.calledOnce; });",
        );
        let plain = behavior_of(
            "test('records the pair', () => { const spy = vi.fn(); spy(); spy(); \
         expect(spy).to.have.been.calledTwice; });",
        );
        assert_ne!(
            negated, plain,
            "opposite-polarity property-chain assertions must not compare as the same behavior"
        );
    }

    /// R23 (S11): the lint-rule-package shape -- tests declared inside a module-scope helper that IS
    /// called at module scope. `runWithConfig('default', {})` runs for every test in the file, so the
    /// `it.concurrent` inside its body is as much a declaration as a top-level one; the extractor's
    /// structural walk used to drop function bodies, so such a file analyzed to zero tests. Pins
    /// that the wrapped declaration -- and its `.concurrent` modifier -- are extracted, and that
    /// the call site's literal argument flows into the title.
    ///
    /// A faithful distillation, verified against the source: that package's `oxlint.test.ts` is this
    /// file almost line for line. What the S11 brief got wrong is the
    /// *scope* -- this accounts for 2 of that repository's 135 files, not all of them; see
    /// [`Walker::walk_helper_call`]'s "What this does not reach" for what the other 133 are.
    #[test]
    fn tests_declared_in_a_module_scope_helper_called_at_module_scope_are_extracted() {
        let extraction = extract_source(
        "import { it } from 'vitest';\nrunWithConfig('default', {});\nfunction runWithConfig(name, cfg) {\n  it.concurrent(name, () => { check(cfg); });\n}\n",
        "case.test.ts",
        Globals::Enabled,
    );
        let [test] = extraction.tests.as_slice() else {
            panic!(
                "the called helper's it.concurrent must be extracted, got {:?}",
                extraction.tests
            )
        };
        assert_eq!(
            test.title,
            crate::model::Title::Static("default".to_owned())
        );
        assert!(
            test.modifiers
                .iter()
                .any(|modifier| matches!(modifier, crate::model::Modifier::Concurrent)),
            "the .concurrent modifier travels with the wrapped declaration: {:?}",
            test.modifiers
        );
    }

    /// R24 (S11's counter-case): `it(` text inside a string literal is not a declaration. The
    /// `vitest-dev/eslint-plugin-vitest` corpus correctly extracts zero tests from 83 files because
    /// its `it(`/`describe(` text sits inside `ruleTester.run()` string literals; any S11 fix that
    /// starts scanning function bodies must not start scanning STRING contents. Passes today and
    /// guards the fix.
    #[test]
    fn it_text_inside_a_string_literal_is_not_a_declaration() {
        let extraction = extract_source(
        "const results = ruleTester.run('no-magic-numbers', {\n  valid: [\"it('is fine', () => { expect(1).toBe(1) })\"],\n  invalid: [],\n});\n",
        "case.test.ts",
        Globals::Enabled,
    );
        assert!(
            extraction.tests.is_empty(),
            "declaration-looking text inside a string literal must not extract: {:?}",
            extraction.tests
        );
    }

    /// R24's live half, for the shape S11 actually created. R24 above passes for a reason that
    /// survives any change to this module -- `ruleTester.run(...)` is a member expression, so no
    /// helper is ever expanded there and no code inside the quotes is reached by any route. That
    /// makes it a weak guard against *this* fix specifically. This one puts the same string
    /// inside the body that S11 now walks, one edit from firing: delete the two `"` characters
    /// and `it('inner', ...)` becomes a statement in an expanded helper body, which the
    /// assertion below proves extracts (the `control` half). Only "the walker does not read
    /// string contents" keeps the first assertion true.
    #[test]
    fn a_string_literal_inside_an_expanded_helper_body_is_not_a_declaration() {
        let quoted = extract_source(
            "import { it } from 'vitest';\nfunction run() {\n  const source = \"it('inner', () => { expect(1).toBe(1); })\";\n  lint(source);\n}\nrun();\n",
            "case.test.ts",
            Globals::Enabled,
        );
        assert!(
            quoted.tests.is_empty(),
            "declaration-looking text inside a string literal must not extract, even now that \
             the helper body around it is walked: {:?}",
            quoted.tests
        );

        let unquoted = extract_source(
            "import { it } from 'vitest';\nfunction run() {\n  const source = 1;\n  it('inner', () => { expect(1).toBe(1); });\n}\nrun();\n",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(
            unquoted.tests.len(),
            1,
            "control: the identical declaration *as code* in the same expanded body must extract \
             -- otherwise the assertion above passes for the wrong reason: {:?}",
            unquoted.tests
        );
    }

    /// R25: the boundary between R23 and R24 -- a helper that DECLARES tests but is never called
    /// contributes nothing, whether or not its body is walked. A `.skip`-gated or dead helper's
    /// declarations do not run, so extracting them would invent tests Vitest never executes. Passes
    /// today; after the S11 fix it keeps the call requirement honest.
    #[test]
    fn a_helper_declaring_tests_that_is_never_called_contributes_no_tests() {
        let extraction = extract_source(
        "import { it } from 'vitest';\nfunction declareLater(name) {\n  it(name, () => {});\n}\nit('runs now', () => {});\n",
        "case.test.ts",
        Globals::Enabled,
    );
        assert_eq!(
            extraction.tests.len(),
            1,
            "only the called declaration extracts"
        );
        assert_eq!(
            extraction.tests[0].title,
            crate::model::Title::Static("runs now".to_owned())
        );
    }

    /// S11's multiplicity decision, both halves. Vitest registers a helper's declarations once
    /// per call, so two calls really are two tests and `testsAnalyzed` must say so -- but both
    /// come from **one** `it` call, so they share one declaration `Location`, which is what
    /// `analysis::dedup_by_declaration` keys the "one declaration, many instances" collapse on.
    /// Emitting one test for two calls would fail the first assertion; giving each instance the
    /// call site's own location (so the two could pair with each other as a `duplicate-test`)
    /// would fail the second.
    #[test]
    fn a_helper_called_twice_declares_twice_from_one_declaration_site() {
        let extraction = extract_source(
            "import { it } from 'vitest';\nfunction run(name) {\n  it(name, () => { expect(1).toBe(1); });\n}\nrun('first');\nrun('second');\n",
            "case.test.ts",
            Globals::Enabled,
        );
        let titles: Vec<_> = extraction
            .tests
            .iter()
            .map(|test| test.title.clone())
            .collect();
        assert_eq!(
            titles,
            vec![
                crate::model::Title::Static("first".to_owned()),
                crate::model::Title::Static("second".to_owned()),
            ],
            "each call registers the helper's declarations again, with its own argument"
        );
        assert_eq!(
            extraction.tests[0].location, extraction.tests[1].location,
            "both instances come from the one `it` call inside the helper, so the rules' \
             per-declaration collapse must be able to see that"
        );
    }

    /// The helper parameter's flow into a title is all-or-nothing, matching how
    /// `render_template_title` already treats a substitution it cannot fold: a literal argument
    /// renders, an argument this extractor cannot prove declines to `Title::Dynamic` rather than
    /// rendering the parameter's *name* or some other value nobody evaluated. The second half is
    /// the one that can regress -- binding a parameter to its own source text would turn
    /// `Title::Dynamic("label")` into `Title::Static("label")` and quietly publish a title the
    /// run never produces.
    #[test]
    fn a_helper_parameter_renders_only_when_the_call_site_argument_is_provable() {
        let provable = extract_source(
            "import { it } from 'vitest';\nfunction run(label) {\n  it(label, () => { expect(1).toBe(1); });\n}\nrun('a' + 'b');\n",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(
            provable.tests[0].title,
            crate::model::Title::Static("ab".to_owned()),
            "a const-foldable argument flows into the title"
        );

        let unprovable = extract_source(
            "import { it } from 'vitest';\nfunction run(label) {\n  it(label, () => { expect(1).toBe(1); });\n}\nrun(computeLabel());\n",
            "case.test.ts",
            Globals::Enabled,
        );
        assert!(
            matches!(
                &unprovable.tests[0].title,
                crate::model::Title::Dynamic(text) if text == "label"
            ),
            "an argument this extractor cannot evaluate must decline to a dynamic title, not \
             render one: {:?}",
            unprovable.tests[0].title
        );
        assert!(
            !unprovable.tests[0].context.is_comparable(),
            "and the declaration it produced must be excluded from comparison"
        );
    }

    /// Why a helper-declared test stays **comparable** even though the body fingerprint never
    /// sees the argument the call site passed.
    ///
    /// A parameter is the one channel through which call-site data reaches an inlined body, so
    /// the obvious worry is that `f(value)` in two helpers, with two callers supplying different
    /// values, fingerprints identically and reads as one `duplicate-test` at error severity. It
    /// does not, and this pins why: a symbol declared outside the fingerprinted body is
    /// identified by its own **declaration** (`free:outer:<file>:<span>:<name>` -- see
    /// `fingerprint::free_identity`), never by its name, so two helpers' parameters can never
    /// collide. The only declarations that *do* share a parameter binding are the ones inside one
    /// helper, which genuinely do observe the same value on any given call.
    ///
    /// That is what makes marking these declarations unresolved unnecessary rather than merely
    /// cautious -- and marking them unresolved would be a silent recall floor over exactly the
    /// corpus S11 exists to read. If `free_identity` ever regresses to keying on the name, this
    /// fails and the conservatism has to come back.
    #[test]
    fn two_helpers_parameters_do_not_collide_so_their_declarations_stay_comparable() {
        let distinct_helpers = extract_source(
            "import { it } from 'vitest';\nfunction runA(value) { it('a', () => { expect(f(value)).toBe(1); }); }\nfunction runB(value) { it('b', () => { expect(f(value)).toBe(1); }); }\nrunA(1);\nrunB(2);\n",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(distinct_helpers.tests.len(), 2);
        assert_ne!(
            distinct_helpers.tests[0].body.alpha, distinct_helpers.tests[1].body.alpha,
            "two helpers' same-named parameters are different bindings, so the bodies reading \
             them are different bodies"
        );
        assert!(
            distinct_helpers
                .tests
                .iter()
                .all(|test| test.context.is_comparable()),
            "and nothing about being inlined makes them incomparable"
        );

        // The control: inside ONE helper, two declarations really do share the binding, and
        // really do observe the same value -- so they are equal here, and reporting them as
        // duplicates is a true positive rather than the false one above.
        let one_helper = extract_source(
            "import { it } from 'vitest';\nfunction run(value) {\n  it('a', () => { expect(f(value)).toBe(1); });\n  it('b', () => { expect(f(value)).toBe(1); });\n}\nrun(1);\n",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(one_helper.tests.len(), 2);
        assert_eq!(
            one_helper.tests[0].body.alpha, one_helper.tests[1].body.alpha,
            "control: one binding, one value, one body -- if this differs too, the assertion \
             above proves nothing about parameter identity"
        );
    }

    /// A hook written inside a called helper belongs to the scope that calls it, because a
    /// function body is not a Vitest scope. That is why S11 expands helpers in the *prepare* pass
    /// as well as the walk: Vitest applies a hook to every test in its scope regardless of
    /// textual order, so a helper called after the test it affects must still contribute its
    /// `beforeEach`. Removing `prepare_helper_call` leaves the test extracted but hookless, which
    /// this fails on.
    #[test]
    fn a_hook_declared_in_a_called_helper_reaches_the_tests_in_the_calling_scope() {
        let extraction = extract_source(
            "import { it, beforeEach } from 'vitest';\nit('runs first', () => { expect(1).toBe(1); });\nfunction install() {\n  beforeEach(() => { seed(); });\n}\ninstall();\n",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(extraction.tests.len(), 1);
        assert_eq!(
            extraction.tests[0]
                .context
                .hooks
                .iter()
                .map(|hook| hook.kind)
                .collect::<Vec<_>>(),
            vec![HookKind::BeforeEach],
            "the helper's hook applies to a test declared textually before the call"
        );
    }

    /// Recursion terminates, contributes each declaration exactly once, and says so. The cycle
    /// guard is what stops `run` from being expanded once per frame down to
    /// `MAX_HELPER_EXPANSION_DEPTH`; the diagnostic is what stops the smaller-than-runtime count
    /// from reading as a complete answer, since a recursive declaring helper really does register
    /// more tests than this extractor can count.
    #[test]
    fn a_self_calling_helper_terminates_contributes_once_and_reports_incomplete() {
        let extraction = extract_source(
            "import { it } from 'vitest';\nfunction run(n) {\n  it('step', () => { expect(n).toBe(1); });\n  if (n > 0) { run(n - 1); }\n}\nrun(3);\n",
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(
            extraction.tests.len(),
            1,
            "the cycle guard collapses the recursion to one expansion: {:?}",
            extraction.tests
        );
        assert!(
            extraction.incomplete,
            "and the run must not present that count as the whole truth"
        );
        assert!(
            extraction
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("calls itself")),
            "the diagnostic must name why the body was not fully expanded: {:?}",
            extraction.diagnostics
        );
    }

    /// The depth bound is real, sits exactly where the comparison puts it, and refuses out loud
    /// rather than stopping silently. Both halves probe the edge from either side -- a chain of
    /// exactly `MAX_HELPER_EXPANSION_DEPTH` helpers must still reach its declaration, one deeper
    /// must not -- so the `>=` in `begin_helper_expansion` cannot be softened to `>` (measured:
    /// that mutation fails this test), the guard cannot be deleted, and the refusal cannot become
    /// silent.
    ///
    /// It does **not** pin the constant's numeric value, and deliberately so: the chain is built
    /// from `MAX_HELPER_EXPANSION_DEPTH` itself, so retuning the bound moves both halves with it
    /// and this still passes. That value is a tuning parameter (how much work a hostile file may
    /// cost), not a property of the extractor; what is asserted here is that *some* bound exists,
    /// that it is enforced with the exact off-by-one the constant states, and that crossing it is
    /// visible.
    #[test]
    fn helper_nesting_one_past_the_depth_limit_is_refused_rather_than_silently_dropped() {
        /// Build a chain of local helper calls of the requested depth ending in one test
        /// registration.
        fn chain(levels: usize) -> String {
            let mut source = String::from("import { it } from 'vitest';\nh0();\n");
            for level in 0..levels {
                source.push_str(&format!("function h{level}() {{\n"));
                if level + 1 == levels {
                    source.push_str("  it('deepest', () => { expect(1).toBe(1); });\n");
                } else {
                    source.push_str(&format!("  h{}();\n", level + 1));
                }
                source.push_str("}\n");
            }
            source
        }

        let at_limit = extract_source(
            &chain(MAX_HELPER_EXPANSION_DEPTH),
            "case.test.ts",
            Globals::Enabled,
        );
        assert_eq!(
            at_limit.tests.len(),
            1,
            "a chain exactly as deep as the limit still reaches its declaration: {:?}",
            at_limit.diagnostics
        );
        assert!(!at_limit.incomplete, "{:?}", at_limit.diagnostics);

        let past_limit = extract_source(
            &chain(MAX_HELPER_EXPANSION_DEPTH + 1),
            "case.test.ts",
            Globals::Enabled,
        );
        assert!(
            past_limit.tests.is_empty(),
            "one level deeper, the declaration is out of reach: {:?}",
            past_limit.tests
        );
        assert!(
            past_limit
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("nested deeper")),
            "and refusing must be visible, never silent: {:?}",
            past_limit.diagnostics
        );
    }

    /// The depth bound alone is not a bound on *work*: eight levels of a body that calls two
    /// helpers is still only 256 expansions, but a machine-generated body calling a hundred is
    /// `10^16`. `MAX_HELPER_EXPANSIONS` is what makes the total linear, and this is the only test
    /// that reaches it -- a budget nothing exercises is a budget that can be silently broken.
    ///
    /// One call past the budget, from either side of it: `MAX_HELPER_EXPANSIONS` shallow calls
    /// are all expanded, and the declaration reached through the next one is not. Both passes
    /// draw on the one budget (the prepare scan of a scope runs interleaved with its walk, so
    /// there is no point at which "the passes" could be given separate allowances), which is why
    /// the spending calls below are counted as they are.
    #[test]
    fn helper_expansion_past_the_budget_is_refused_rather_than_silently_dropped() {
        // Each `spend()` call expands once in the prepare pass and once in the walk, and its body
        // declares nothing -- so `MAX_HELPER_EXPANSIONS / 2` of them consume the budget exactly.
        let spenders = MAX_HELPER_EXPANSIONS / 2;
        let prelude = "import { it } from 'vitest';\nfunction spend() { const x = 1; }\n\
                       function declare() { it('last', () => { expect(1).toBe(1); }); }\n";

        let within = format!("{prelude}{}declare();\n", "spend();\n".repeat(spenders - 1));
        let within = extract_source(&within, "case.test.ts", Globals::Enabled);
        assert_eq!(
            within.tests.len(),
            1,
            "inside the budget, the declaring helper still expands: {:?}",
            within.diagnostics
        );
        assert!(!within.incomplete, "{:?}", within.diagnostics);

        let past = format!("{prelude}{}declare();\n", "spend();\n".repeat(spenders));
        let past = extract_source(&past, "case.test.ts", Globals::Enabled);
        assert!(
            past.tests.is_empty(),
            "one call further, the budget is spent and the declaration is out of reach: {:?}",
            past.tests
        );
        assert!(
            past.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("budget is spent")),
            "and spending the budget must be visible, never silent: {:?}",
            past.diagnostics
        );
    }
}

#[cfg(test)]
#[path = "extract/tests/imported.rs"]
mod imported_tests;
