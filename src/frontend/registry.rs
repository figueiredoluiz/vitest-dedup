//! Resolves whether a call expression refers to a Vitest test-declaration binding.
//!
//! Hazards 4 and 10 in `docs/vitest-surface.md` rule out name matching as a detection strategy:
//! Playwright, Jest, Cypress, `node:test`, and Mocha all expose `describe(string, fn)` and
//! `it(string, fn)` calls that are textually indistinguishable from Vitest's, and a local
//! variable can shadow a real `test` import inside an otherwise-legitimate Vitest file. The only
//! sound signal is *binding identity*: which declaration (an `import ... from "vitest"`
//! specifier, an `import.meta.vitest` destructure, or a `test.extend(...)` result) a symbol
//! traces back to, resolved through `oxc_semantic`'s scope/symbol tables rather than through the
//! text of an identifier.
//! The competing frameworks in that list are counterexamples, not supported input dialects:
//! declarations bound to `node:test`, for example, deliberately produce no Vitest test cases.
//!
//! This module owns exactly that resolution step. It does not build [`crate::model::TestCase`]
//! values itself -- `extract.rs` does that -- but it is the single place that decides, for one
//! callee expression, which Vitest export (if any) it ultimately refers to and what chain of
//! member accesses and calls sits between the binding and the call being examined.

use crate::model::{FixtureBinding, FixtureValue};
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, BinaryOperator, BindingPattern, ChainElement, Expression,
    IdentifierReference, ImportDeclaration, ImportDeclarationSpecifier, ModuleExportName,
    ObjectPropertyKind, Program, PropertyKey, PropertyKind, Statement, TemplateLiteral,
    UnaryOperator, VariableDeclarationKind, VariableDeclarator,
};
use oxc_ast::AstKind;
use oxc_ast_visit::{walk, Visit};
use oxc_semantic::{AstNodes, NodeId, ReferenceFlags, Scoping, Semantic, SymbolId};
use oxc_span::GetSpan;
use std::collections::{HashMap, HashSet};
use std::path::Path;
#[cfg(not(test))]
use std::sync::{Mutex, OnceLock};

use super::fingerprint::{escape_literal_delimiters, fingerprint, path_key, FunctionLike};

/// Whether bare, unimported `test`/`describe`/`it`/hook identifiers should be treated as
/// Vitest's ambient globals for this file.
///
/// Mirrors Vitest's own `test.globals` config flag (default `false`). Hazard 3 in
/// `docs/vitest-surface.md` establishes that this must be resolved *per project*, not once for a
/// whole repository -- `test.projects` entries can set `extends: false` and diverge. This
/// frontend has no access to `vitest.config.*` and must not guess: the caller (which does own
/// project/config resolution) passes the already-resolved value in for each file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Globals {
    /// `test: { globals: true }` is in effect for the project that owns this file.
    Enabled,
    /// The default: a bare `test`/`describe`/`it` is an ordinary, unrelated identifier.
    Disabled,
}

/// What a resolved local symbol stands for, in Vitest's own vocabulary.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Binding {
    /// A name bound -- directly via import, or transitively via `test.extend` -- to one of
    /// Vitest's exports. `canonical` is the post-alias export name (`it` and `suite` never
    /// appear here; they normalize to `"test"`/`"describe"` at import time), so every later
    /// match site only has to consider one spelling per concept.
    Local {
        canonical: &'static str,
        /// Fixtures visible through the `test.extend` chain that produced this binding. Empty for
        /// a plain import.
        fixtures: Vec<FixtureBinding>,
        /// Set when some fixture in the chain could not be statically named or fingerprinted
        /// (e.g. a computed key, or `.extend(getFixtures())`), so callers know the fixture list
        /// is incomplete rather than exhaustive.
        fixtures_unresolved: bool,
    },
    /// `import * as x from "vitest"`: `x` alone is not callable, but `x.<member>` is exactly as
    /// good as a named import of `<member>`.
    Namespace,
}

/// Per-file table from resolved symbol to the Vitest export it represents.
///
/// Built once per file from top-level `import ... from "vitest"` declarations, then extended
/// live during the main extraction walk as `import.meta.vitest` destructures and
/// `test.extend(...)` results are encountered -- both can introduce new locally-scoped bindings
/// that later statements in the same file call through.
#[derive(Debug, Default)]
pub(super) struct Registry {
    bindings: HashMap<SymbolId, Binding>,
    globals_enabled: bool,
    /// Most rows a `.each`/`.for` table may have before it is refused unread: the file's
    /// remaining declaration budget, set by the walker before each resolution.
    table_row_limit: std::cell::Cell<usize>,
}

/// The [`ChainStep::Each`] table identity of a table longer than [`Registry::limit_table_rows`].
pub(super) const OVER_LIMIT_TABLE: &str = "<over-limit-table>";

#[cfg(test)]
thread_local! {
    /// Test-only count of literal table rows this thread read.
    pub(super) static TABLE_ROWS_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Registry {
    /// Seeds a registry from every top-level `import ... from "vitest"` declaration in the
    /// program. Imports are the one binding source that is guaranteed to be syntactically
    /// top-level in a module, so this never needs to recurse.
    pub(super) fn new(program: &Program, globals: Globals) -> Self {
        let mut registry = Registry {
            bindings: HashMap::new(),
            globals_enabled: matches!(globals, Globals::Enabled),
            table_row_limit: std::cell::Cell::new(usize::MAX),
        };
        for statement in &program.body {
            if let Statement::ImportDeclaration(import) = statement {
                registry.register_import(import);
            }
        }
        registry
    }

    /// Sets how many rows a table may have before resolution refuses it without reading a row.
    pub(super) fn limit_table_rows(&self, limit: usize) {
        self.table_row_limit.set(limit);
    }

    /// Registers runtime Vitest import symbols and skips type-only declarations/specifiers.
    fn register_import(&mut self, import: &ImportDeclaration) {
        if matches!(import.import_kind, oxc_ast::ast::ImportOrExportKind::Type)
            || import.source.value.as_str() != "vitest"
        {
            return;
        }
        let Some(specifiers) = &import.specifiers else {
            return;
        };
        for specifier in specifiers {
            match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                    if matches!(
                        specifier.import_kind,
                        oxc_ast::ast::ImportOrExportKind::Type
                    ) {
                        continue;
                    }
                    let imported_name = match &specifier.imported {
                        ModuleExportName::IdentifierName(id) => id.name.as_str(),
                        ModuleExportName::IdentifierReference(id) => id.name.as_str(),
                        ModuleExportName::StringLiteral(literal) => literal.value.as_str(),
                    };
                    if let (Some(canonical), Some(symbol_id)) =
                        (canonicalize(imported_name), specifier.local.symbol_id.get())
                    {
                        self.bindings.insert(
                            symbol_id,
                            Binding::Local {
                                canonical,
                                fixtures: Vec::new(),
                                fixtures_unresolved: false,
                            },
                        );
                    }
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
                    if let Some(symbol_id) = specifier.local.symbol_id.get() {
                        self.bindings.insert(symbol_id, Binding::Namespace);
                    }
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(_) => {
                    // Vitest has no meaningful default export for our purposes.
                }
            }
        }
    }

    /// Binds a local symbol to a Vitest export outside of an `import` statement: either an
    /// `import.meta.vitest` destructure (hazard 9) or a `test.extend(...)` result, both of which
    /// introduce a binding that later statements call through exactly like a named import.
    pub(super) fn bind_local(
        &mut self,
        symbol_id: SymbolId,
        canonical: &'static str,
        fixtures: Vec<FixtureBinding>,
        fixtures_unresolved: bool,
    ) {
        self.bindings.insert(
            symbol_id,
            Binding::Local {
                canonical,
                fixtures,
                fixtures_unresolved,
            },
        );
    }
}

/// Maps an imported/exported name to its canonical, alias-free form.
///
/// `it` and `suite` are full aliases of `test` and `describe` respectively (per
/// `docs/vitest-surface.md` section 1) -- collapsing them here means every later match only has
/// to consider one spelling. Names with no Vitest meaning return `None`, which is itself
/// important: an import of, say, `assert` from `"vitest"` should never be mistaken for a
/// declaration-relevant binding.
fn canonicalize(name: &str) -> Option<&'static str> {
    match name {
        "test" | "it" => Some("test"),
        "describe" | "suite" => Some("describe"),
        "bench" => Some("bench"),
        "vi" => Some("vi"),
        "beforeAll" => Some("beforeAll"),
        "beforeEach" => Some("beforeEach"),
        "afterEach" => Some("afterEach"),
        "afterAll" => Some("afterAll"),
        "expect" => Some("expect"),
        _ => None,
    }
}

/// Exposes [`canonicalize`] to `extract.rs` for the `import.meta.vitest` destructure case, where
/// the "imported" names are property-pattern keys rather than `ImportSpecifier`s.
pub(super) fn canonicalize_export_name(name: &str) -> Option<&'static str> {
    canonicalize(name)
}

/// One step in a resolved callee chain, in source order (closest to the root first).
#[derive(Debug, Clone)]
pub(super) enum ChainStep {
    /// A plain `.name` property access carrying no special meaning of its own -- a boolean
    /// modifier (`skip`, `only`, ...) or, if unrecognized, surfaced later as
    /// [`crate::model::Modifier::Unknown`].
    Member(String),
    /// `.each(table)` or `.for(table)`, with the table's rows if it was a literal the analyzer
    /// could enumerate, or an identity for the table expression when it was not (an identifier,
    /// call, or spread) -- hazard 5 requires this stays distinct from "zero rows", and two
    /// different unreadable tables must stay distinct from each other. See
    /// [`crate::model::EachRows`].
    Each {
        is_for: bool,
        rows: Result<Vec<Row>, String>,
    },
    /// `.skipIf(condition)`. `condition` is the fingerprint [`condition_fingerprint`] computed
    /// (a symbol-scoped identity for an identifier, a kind-tagged value for a literal), not raw
    /// source text -- see that function's doc for why. `unresolved` is set only when the call has
    /// no expression argument to identify.
    SkipIf { condition: String, unresolved: bool },
    /// `.runIf(condition)`, on the same terms as `SkipIf`.
    RunIf { condition: String, unresolved: bool },
}

/// The result of resolving a callee expression back to a Vitest export.
#[derive(Debug, Clone)]
pub(super) struct ResolvedCallee {
    /// The canonical export name at the root of the chain (see [`canonicalize`]).
    pub root: &'static str,
    /// Fixtures visible through the `test.extend` chain that produced the root binding, plus any
    /// introduced by an `.extend(...)` link in this very chain (`.extend().extend()` unions both).
    pub fixtures: Vec<FixtureBinding>,
    /// Set when some fixture along the way could not be statically resolved.
    pub fixtures_unresolved: bool,
    /// Member accesses and special calls between the root and the call under examination.
    pub steps: Vec<ChainStep>,
}

/// One statically enumerated `.each`/`.for` table row.
#[derive(Debug, Clone)]
pub(super) struct Row {
    /// Exact source text of the row, used for [`crate::model::EachTable::rows`]. This is a
    /// verbatim snippet rather than an evaluated value -- hazard 6 warns that title/value
    /// rendering is itself Vitest-version-dependent, so the extractor does not pretend to
    /// reproduce runtime formatting.
    pub display: String,
    /// The row's cells, resolved to literal text where possible, for title interpolation.
    pub cells: RowCells,
    /// What Vitest passes as printf arguments to the `%s` family, already rendered.
    ///
    /// Not derivable from [`Row::cells`], for two reasons measured against Vitest 5.0.1. An
    /// object row binds WHOLE, so it is one argument printed as `{ a: 1 }` -- not one argument
    /// per field, which is what reading `cells` would suggest and what this extractor used to do
    /// (`it.each([1, { a: 1 }])('t %s')` reported `t 1` where Vitest reports `t { a: 1 }`). And
    /// an array row in a MIXED table binds whole but its *title* spreads anyway: the callback
    /// receives `[[1, 2]]` while `%s`/`%s` prints `1` and `2`. Binding and title formatting
    /// genuinely disagree inside Vitest, so they are modelled separately here.
    pub title_args: Vec<Option<TitleArg>>,
    /// Set when at least one cell in this row is not a value this extractor can read (an
    /// identifier, a call, a spread, a nested object/array it does not evaluate, ...).
    ///
    /// Defect 4: the table as a whole being a literal array (so [`ChainStep::Each`]'s `rows` is
    /// `Some`) only proves the *row count* is known, not that every row's *values* are. Two rows
    /// in different scopes whose unresolved cell happens to display as the same text (a shared
    /// helper's return value, referenced by an identically-named local in each scope) must never
    /// be allowed to compare equal on that text alone -- exactly the same defect class
    /// `const_fold_tagged` and `test.extend` fixture bodies already close.
    ///
    /// Row substitution reads this too: it is the gate [`crate::frontend::extract`]'s
    /// `row_substitution` uses to
    /// decide whether this row's body may be substituted at all -- a row with *any* unresolved
    /// cell never reaches substitution, full stop, rather than substituting only the cells that
    /// happened to be readable. See `fingerprint.rs`'s row-substitution doc for why a
    /// half-substituted body is unsafe in a way an entirely unsubstituted one is not.
    pub unresolved: bool,
}

impl Row {
    /// Canonical, unambiguous identity of the resolved values Vitest binds for this row.
    ///
    /// Uses kind and raw value rather than display text, so numeric `1` cannot collide with string
    /// `'1'`. Length prefixes keep field names and values from manufacturing separator collisions.
    pub fn identity(&self) -> Option<String> {
        if self.unresolved {
            return None;
        }
        /// Append a length-prefixed component to the row identity buffer.
        fn push_part(output: &mut String, part: &str) {
            output.push_str(&format!("{}:{part}", part.len()));
        }
        /// Encode a resolved cell's kind/raw text or a known elision distinctly in the row
        /// identity.
        fn push_value(output: &mut String, value: &Option<CellValue>) {
            match value {
                Some(value) => {
                    output.push('V');
                    push_part(output, value.kind);
                    push_part(output, &value.raw);
                }
                // In a resolved row, `None` is a statically known array elision (`undefined`).
                None => output.push('E'),
            }
        }

        let mut identity = String::new();
        match &self.cells {
            RowCells::Positional(values) => {
                identity.push('P');
                identity.push_str(&values.len().to_string());
                identity.push(':');
                for value in values {
                    push_value(&mut identity, value);
                }
            }
            RowCells::Named(values) => {
                identity.push('N');
                identity.push_str(&values.len().to_string());
                identity.push(':');
                for (name, value) in values {
                    push_part(&mut identity, name);
                    push_value(&mut identity, value);
                }
            }
        }
        Some(identity)
    }
}

/// A single row cell's value in the two forms its two consumers need, computed from the same
/// expression so they can never drift apart:
///
/// - `display`: the untagged text `.each` title rendering interpolates (from [`const_fold`]),
///   matching Vitest's own runtime formatting -- unchanged from before this struct existed.
/// - `kind`/`raw`: that same value's kind and raw text, in exactly the shape
///   [`crate::frontend::fingerprint::Serializer::literal`] expects, for row substitution. This is
///   deliberately *not* [`const_fold_tagged`]'s combined `<kind:raw>` string: that format special-
///   cases `null` as the bare `<null>` (no raw part at all, fine for the fixture/`skipIf` identity
///   comparisons `const_fold_tagged` itself serves), which cannot be fed back through `literal`
///   without re-parsing it. Keeping `kind` and `raw` apart lets the substitution consumer render
///   both `alpha` (`<kind:raw>`) and `structural` (`<kind>`) forms correctly by calling the very
///   function that already renders a real literal that way, rather than a second, independently
///   -- and possibly divergently -- formatted copy of the same rule. This is the fifth place this
///   codebase has had to be careful not to erase a literal's kind; see `const_fold_tagged`'s own
///   doc for the first four.
#[derive(Debug, Clone)]
pub(super) struct CellValue {
    /// What Vitest prints where this cell is referenced by name (`$field`).
    ///
    /// A bare string at the top level and the inspect form otherwise -- see [`render_field`].
    /// The `%s` family does NOT read this: printf coercion and `$field` interpolation are
    /// different renderings of the same value in Vitest itself (an array is `1,2` under `%s` and
    /// `[ 1, 2 ]` under `$field`), so the positional arguments live on [`Row::title_args`].
    pub display: String,
    pub kind: &'static str,
    pub raw: String,
}

/// One printf argument, in each of the renderings Vitest's directives ask for.
///
/// Three, because the directives genuinely disagree about the same value: `%s` on `'a'` prints
/// `a`, `%o` prints `'a'`, and `%j` prints `"a"`. All measured against Vitest 5.0.1.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TitleArg {
    /// `%s`, and the source the numeric directives (`%i`/`%f`) parse as a string prefix.
    pub coerced: String,
    /// `%o`.
    pub inspected: String,
    /// `%j`. `None` where `JSON.stringify` has no answer (a BigInt throws).
    pub json: Option<String>,
    /// A positional `$0`/`$1` reference, which follows `$field` rules rather than printf ones:
    /// a bare string, the inspect form otherwise.
    pub field: String,
    /// The numeric coercions: `%d` for [`TitleNumber::Number`], and the BigInt spelling shared
    /// by `%d`/`%i` for [`TitleNumber::BigInt`].
    ///
    /// `Number` is computed from the VALUE rather than parsed out of `coerced`, because the two
    /// disagree on exactly the values this exists for -- `Number(null)` is `0` while `null` as
    /// text is not a number, and `Number([null])` is `0` through the array's empty string form.
    /// A BigInt is carried as its own spelling because the directives split on it (all measured,
    /// Vitest 5.0.3): `%d` and `%i` print `10n`, while `%f` is `parseFloat(String(10n))` and
    /// prints `10`.
    ///
    /// `None` is both "not statically determinable" and "determinably `NaN`": a `NaN` renders as
    /// the title `NaN` in Vitest, a title that does not discriminate between rows, so the title
    /// renderer refuses it and keeps the unrendered pattern.
    pub number: Option<TitleNumber>,
}

/// A printf argument's numeric identity, for the `%d`/`%i` directives.
#[derive(PartialEq, Debug, Clone)]
pub(super) enum TitleNumber {
    /// `Number(value)`.
    Number(f64),
    /// A BigInt cell's own spelling, `n` suffix included.
    BigInt(String),
}

/// How a row's values are addressed from a title pattern.
#[derive(Debug, Clone)]
pub(super) enum RowCells {
    /// Array-of-arrays (or a single non-array/object value): addressed positionally (`%s`, `$0`).
    Positional(Vec<Option<CellValue>>),
    /// Array-of-objects or a tagged-template row: addressed by field name (`$field`) as well as
    /// position.
    ///
    /// Each name appears at most once, in first-insertion order, holding the *last* value written
    /// under it -- an object literal with a repeated static key (`{ a: 1, a: 2 }`) resolves `a` to
    /// `2` at runtime, and `analyze_row` normalizes to that here so every consumer that looks a
    /// name up (with `.find`, which returns the first match) observes the value Vitest itself
    /// would produce, without needing to know duplicates were ever possible.
    Named(Vec<(String, Option<CellValue>)>),
}

/// Resolves a callee expression to the Vitest export (if any) it refers to.
///
/// This is the core of the module: it walks identifiers, static member accesses, nested calls
/// (`test.each(table)` is a call whose callee is itself a call, so the `.each` member access has
/// to be resolved off the *inner* call), and tagged templates (`test.each\`...\``), converting
/// each into an owned, arena-independent
/// [`ResolvedCallee`]. Returns `None` for anything that does not trace back to a recognized
/// Vitest export -- including a shape this function does not model, which is the conservative
/// choice: a call this function cannot explain is never reported as a Vitest declaration.
pub(super) fn resolve_callee(
    expr: &Expression,
    registry: &Registry,
    semantic: &Semantic<'_>,
    source: &str,
    file: &Path,
) -> Option<ResolvedCallee> {
    // Unwind a member/call chain explicitly. Even a shallow-bracket input can contain thousands
    // of links; recursive resolution previously consumed one native frame for each link.
    let mut pending = Vec::new();
    let mut current = expr;
    let mut resolved = loop {
        let object_and_property = match current {
            Expression::Identifier(ident) => {
                break resolve_identifier(ident, registry, semantic.scoping())?;
            }
            Expression::StaticMemberExpression(member) => {
                Some((&member.object, member.property.name.as_str()))
            }
            Expression::CallExpression(call) => {
                pending.push(current);
                current = &call.callee;
                continue;
            }
            Expression::TaggedTemplateExpression(tagged) => {
                pending.push(current);
                current = &tagged.tag;
                continue;
            }
            Expression::ParenthesizedExpression(paren) => {
                current = &paren.expression;
                continue;
            }
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => {
                    pending.push(current);
                    current = &call.callee;
                    continue;
                }
                ChainElement::StaticMemberExpression(member) => {
                    Some((&member.object, member.property.name.as_str()))
                }
                _ => return None,
            },
            _ => return None,
        };
        let (object, property) = object_and_property?;
        if is_namespace(object, registry, semantic.scoping()) {
            break ResolvedCallee {
                root: canonicalize(property)?,
                fixtures: Vec::new(),
                fixtures_unresolved: false,
                steps: Vec::new(),
            };
        }
        pending.push(current);
        current = object;
    };
    for part in pending.into_iter().rev() {
        match part {
            Expression::StaticMemberExpression(member) => {
                resolved
                    .steps
                    .push(ChainStep::Member(member.property.name.to_string()));
            }
            Expression::CallExpression(call) => {
                resolved = finish_interior_call(
                    resolved,
                    &call.arguments,
                    registry.table_row_limit.get(),
                    semantic,
                    source,
                    file,
                )?;
            }
            Expression::TaggedTemplateExpression(tagged) => {
                resolved = finish_tagged_call(
                    resolved,
                    &tagged.quasi,
                    registry.table_row_limit.get(),
                    semantic,
                    source,
                )?;
            }
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => {
                    resolved = finish_interior_call(
                        resolved,
                        &call.arguments,
                        registry.table_row_limit.get(),
                        semantic,
                        source,
                        file,
                    )?;
                }
                ChainElement::StaticMemberExpression(member) => {
                    resolved
                        .steps
                        .push(ChainStep::Member(member.property.name.to_string()));
                }
                _ => unreachable!("only supported chain elements are queued"),
            },
            _ => unreachable!("only member/call/tagged expressions are queued"),
        }
    }
    Some(resolved)
}

/// Resolve a reference through registered Vitest bindings or permitted ambient globals,
/// declining unrelated bindings.
fn resolve_identifier(
    ident: &IdentifierReference,
    registry: &Registry,
    scoping: &Scoping,
) -> Option<ResolvedCallee> {
    let reference_id = ident.reference_id.get()?;
    let reference = scoping.get_reference(reference_id);
    match reference.symbol_id() {
        // Resolved to a real binding: it is Vitest only if that binding traces to a "vitest"
        // import (or an equivalent). A local variable, or an import from a competing framework,
        // resolves to a symbol that simply is not in `registry.bindings` -- hazards 4 and 10.
        Some(symbol_id) => match registry.bindings.get(&symbol_id)? {
            Binding::Local {
                canonical,
                fixtures,
                fixtures_unresolved,
            } => Some(ResolvedCallee {
                root: canonical,
                fixtures: fixtures.clone(),
                fixtures_unresolved: *fixtures_unresolved,
                steps: Vec::new(),
            }),
            Binding::Namespace => None,
        },
        // Unresolved: only a global if the caller told us this file's project enables them
        // (hazard 3). Ambient Mocha/Cypress-style globals never reach this branch as Vitest
        // because their names, even when they collide (`describe`, `it`), are only promoted to
        // Vitest bindings when `Globals::Enabled` was explicitly passed in for this file.
        None => {
            if registry.globals_enabled {
                let canonical = canonicalize(ident.name.as_str())?;
                Some(ResolvedCallee {
                    root: canonical,
                    fixtures: Vec::new(),
                    fixtures_unresolved: false,
                    steps: Vec::new(),
                })
            } else {
                None
            }
        }
    }
}

/// Recognize an identifier whose resolved symbol is a registered framework namespace.
fn is_namespace(expr: &Expression, registry: &Registry, scoping: &Scoping) -> bool {
    let Expression::Identifier(ident) = expr else {
        return false;
    };
    let Some(reference_id) = ident.reference_id.get() else {
        return false;
    };
    let Some(symbol_id) = scoping.get_reference(reference_id).symbol_id() else {
        return false;
    };
    matches!(registry.bindings.get(&symbol_id), Some(Binding::Namespace))
}

/// Converts an interior call (`test.each(table)`, `test.skipIf(cond)`, `baseTest.extend({..})`)
/// into the appropriate [`ChainStep`], replacing the bare [`ChainStep::Member`] that named it.
///
/// Vitest's own docs describe `.each`/`.for` as always the last link before the final call, but
/// this makes no such assumption: it converts whatever member the call closes over, wherever it
/// falls in the chain, so `test.skipIf(x).each(table)` and `test.each(table)` are both handled by
/// the same code path.
fn finish_interior_call(
    mut resolved: ResolvedCallee,
    arguments: &[Argument],
    row_limit: usize,
    semantic: &Semantic<'_>,
    source: &str,
    file: &Path,
) -> Option<ResolvedCallee> {
    let ChainStep::Member(name) = resolved.steps.pop()? else {
        // The thing being called was not a bare member access (e.g. calling an already-called
        // `.each(table)` result again) -- not a shape this extractor models.
        return None;
    };
    match name.as_str() {
        "each" => {
            let rows = table_rows(arguments.first(), row_limit, semantic, source, file);
            resolved.steps.push(ChainStep::Each {
                is_for: false,
                rows,
            });
        }
        "for" => {
            let rows = table_rows(arguments.first(), row_limit, semantic, source, file);
            resolved.steps.push(ChainStep::Each { is_for: true, rows });
        }
        "skipIf" => {
            let (condition, unresolved) = condition_fingerprint(arguments, semantic, source, file);
            resolved.steps.push(ChainStep::SkipIf {
                condition,
                unresolved,
            });
        }
        "runIf" => {
            let (condition, unresolved) = condition_fingerprint(arguments, semantic, source, file);
            resolved.steps.push(ChainStep::RunIf {
                condition,
                unresolved,
            });
        }
        "extend" => {
            // `.extend().extend()` unions fixtures from every link in the chain, not just the
            // last one -- `resolved.fixtures` already carries whatever the object side inherited.
            let (fixtures, unresolved) = extend_fixture_bindings(arguments, semantic, source, file);
            resolved.fixtures.extend(fixtures);
            resolved.fixtures_unresolved |= unresolved;
        }
        _ => return None,
    }
    Some(resolved)
}

/// Extracts fixture names and setup-function fingerprints from a `test.extend(...)` call's
/// arguments, in both its object-syntax and builder-syntax forms.
///
/// Per `crate::model::FixtureValue`'s contract: a fixture's body is `Setup(alpha)` when its value
/// is a setup function, `Literal(fingerprint)` when it is a statically readable plain value, and
/// `Unresolved` when it is neither (an identifier, a call, a member expression, ...) -- a value
/// this frontend cannot vouch for must never compare equal to another by coincidence. The
/// returned `bool` is set whenever *any* fixture in this call ends up `Unresolved`, or could not
/// be statically named at all (a computed key, a spread, or a table this extractor does not
/// model), which the caller unions into [`ResolvedCallee::fixtures_unresolved`].
fn extend_fixture_bindings(
    arguments: &[Argument],
    semantic: &Semantic<'_>,
    source: &str,
    file: &Path,
) -> (Vec<FixtureBinding>, bool) {
    match arguments.first() {
        Some(Argument::ObjectExpression(object)) => {
            let mut fixtures = Vec::new();
            let mut unresolved = false;
            for property in &object.properties {
                match property {
                    ObjectPropertyKind::ObjectProperty(property) => {
                        match property_key_name(&property.key) {
                            Some(name) => {
                                let body = fixture_body_fingerprint(
                                    &property.value,
                                    semantic,
                                    source,
                                    file,
                                );
                                if matches!(body, FixtureValue::Unresolved) {
                                    unresolved = true;
                                }
                                fixtures.push(FixtureBinding { name, body });
                            }
                            // A computed key (`[expr]: fn`) defeats static naming.
                            None => unresolved = true,
                        }
                    }
                    ObjectPropertyKind::SpreadProperty(_) => unresolved = true,
                }
            }
            (fixtures, unresolved)
        }
        // Builder-syntax (v4.1+): `.extend('name', async (deps, { onCleanup }) => value)`.
        Some(Argument::StringLiteral(literal)) => {
            let name = literal.value.as_str().to_owned();
            let body = arguments
                .get(1)
                .and_then(Argument::as_expression)
                .map(|value| fixture_body_fingerprint(value, semantic, source, file))
                .unwrap_or(FixtureValue::Unresolved);
            let unresolved = matches!(body, FixtureValue::Unresolved);
            (vec![FixtureBinding { name, body }], unresolved)
        }
        // No arguments, or a shape this extractor does not model (e.g. `.extend(getFixtures())`):
        // nothing is statically knowable, which must not be mistaken for "no fixtures".
        _ => (Vec::new(), true),
    }
}

/// Convert supported fixture values into setup/literal evidence, retaining an unresolved marker
/// for unsupported forms.
fn fixture_body_fingerprint(
    value: &Expression,
    semantic: &Semantic<'_>,
    source: &str,
    file: &Path,
) -> FixtureValue {
    match value {
        // `None`: a fixture setup body is never a `.each` row callback, so there is nothing to
        // substitute here -- see `fingerprint.rs`'s row-substitution doc.
        Expression::ArrowFunctionExpression(arrow) => {
            let body = fingerprint(FunctionLike::Arrow(arrow), semantic, source, file, None);
            if body.class_unresolved {
                FixtureValue::Unresolved
            } else {
                FixtureValue::Setup(body.alpha)
            }
        }
        Expression::FunctionExpression(function) => {
            let body = fingerprint(
                FunctionLike::Function(function),
                semantic,
                source,
                file,
                None,
            );
            if body.class_unresolved {
                FixtureValue::Unresolved
            } else {
                FixtureValue::Setup(body.alpha)
            }
        }
        // Anything else: a statically readable value (string, number, boolean, null, bigint, a
        // substitution-free template, a signed number, an object or array of those, or simple
        // arithmetic on one) is a meaningful, comparable state -- `Literal(fingerprint)`, not
        // "unresolved". Anything `const_fold` cannot prove (an identifier, a call such as
        // `makeRealDb()`, a member expression, a spread, a computed key, or a composite holding
        // any of those) genuinely is unresolved and must be reported as such (A5).
        other => match const_fold_tagged(other) {
            Some(literal) => FixtureValue::Literal(literal),
            None => FixtureValue::Unresolved,
        },
    }
}

/// The tagged-template sibling of [`finish_interior_call`], for `` test.each`a|b\n${1}|${2}` ``.
fn finish_tagged_call(
    mut resolved: ResolvedCallee,
    quasi: &TemplateLiteral,
    row_limit: usize,
    semantic: &Semantic<'_>,
    source: &str,
) -> Option<ResolvedCallee> {
    let ChainStep::Member(name) = resolved.steps.pop()? else {
        return None;
    };
    let is_for = match name.as_str() {
        "each" => false,
        "for" => true,
        _ => return None,
    };
    let rows = tagged_template_rows(quasi, row_limit, source, Some(semantic));
    resolved.steps.push(ChainStep::Each { is_for, rows });
    Some(resolved)
}

/// Identity for a tagged-template `.each`/`.for` table whose header this extractor could not
/// parse into columns (per [`tagged_template_rows`]) -- its own raw source text, the only thing
/// left that is honestly knowable about it.
fn tagged_template_identity(quasi: &TemplateLiteral, source: &str) -> String {
    format!("<unresolved:{}>", source_text(source, quasi.span()))
}

/// Fingerprints a `.skipIf`/`.runIf` condition so that two conditions sharing only *source text*
/// -- not runtime identity -- can never compare equal.
///
/// Defect 5, the same defect class as `const_fold_tagged` and `test.extend` fixture bodies (see
/// that function's doc): a raw-text condition made `test.skipIf(enabled)` in one `describe`
/// fingerprint identically to `test.skipIf(enabled)` in a sibling `describe` declaring its own,
/// differently-valued `const enabled` -- two opposite runtime conditions, one string, a false
/// `duplicate-test` at error severity.
///
/// - A **literal** condition (`skipIf(true)`, `skipIf('ci')`, ...) is tagged by kind and value via
///   [`const_fold_tagged`], so two literal conditions still compare equal when they should.
/// - A bare **identifier** is tagged by the *symbol* its reference resolves to -- its declaration
///   span, not its name, exactly like `fingerprint.rs`'s own free-variable identity scheme, so
///   sibling scopes that each declare their own same-named binding never collide. The span alone
///   is only unique *within one file*: two different files can declare a same-named local binding
///   at byte-for-byte identical offsets (identical boilerplate up to that point is a common way
///   for this to happen) with opposite values, and `CrossFileKey` (`analysis.rs`) deliberately
///   drops the path when grouping for `cross-file-duplicate`, so a span-only identity would
///   collide those two files' conditions into a false match. `file`, rendered through
///   [`path_key`], is folded into the identity for exactly this reason -- the same treatment
///   `fingerprint.rs`'s own `free:module`/`free:outer` identities already get, reusing that
///   module's own path-rendering helper rather than a second spelling of it. An identifier with no
///   resolved symbol at all (a genuine global, e.g. `skipIf(isCI)` with no local declaration in
///   this file) keeps its bare name and no file component: unlike a shadowable local, two
///   references to the same undeclared global really do mean the same thing, in this file or
///   another, so this is the one case where cross-file matching on a condition is still correct.
/// - Anything else -- a call, comparison, logical combination, member expression, ... -- keeps
///   its exact source spelling plus the symbol-scoped identity of every identifier reference
///   inside it. This does not claim to know the condition's value. It only lets two occurrences
///   compare when both their syntax and every referenced binding agree. Raw source alone is not
///   enough: two sibling scopes can both spell `enabled && isCI` while their `enabled` identifiers
///   resolve to different declarations.
fn condition_fingerprint(
    arguments: &[Argument],
    semantic: &Semantic<'_>,
    source: &str,
    file: &Path,
) -> (String, bool) {
    let Some(expr) = arguments.first().and_then(Argument::as_expression) else {
        return (String::new(), true);
    };
    let expr = unwrap_parens(expr);
    if let Some(literal) = const_fold_tagged(expr) {
        return (literal, false);
    }
    if let Expression::Identifier(ident) = expr {
        return (identifier_identity(ident, semantic, file), false);
    }
    (expression_identity(expr, semantic, source, file), false)
}

/// Exact-source identity for an expression, qualified by every identifier binding it references.
///
/// Length prefixes make the representation unambiguous even when source text, paths, or names
/// contain the separators used here. Local identifiers inherit [`identifier_identity`]'s file and
/// declaration span; unresolved globals retain their names. The result proves identity of the
/// expression spelling and bindings, not its runtime value.
fn expression_identity(
    expr: &Expression,
    semantic: &Semantic<'_>,
    source: &str,
    file: &Path,
) -> String {
    struct References<'s, 'p> {
        semantic: &'s Semantic<'s>,
        file: &'p Path,
        identities: Vec<String>,
    }

    impl<'a> Visit<'a> for References<'_, '_> {
        /// Collect each referenced identifier's anchored identity while walking the fixture
        /// expression.
        fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
            self.identities
                .push(identifier_identity(ident, self.semantic, self.file));
            walk::walk_identifier_reference(self, ident);
        }
    }

    let raw = source_text(source, expr.span());
    let mut references = References {
        semantic,
        file,
        identities: Vec::new(),
    };
    references.visit_expression(expr);

    let mut identity = format!("<expr:{}:{raw}", raw.len());
    for reference in references.identities {
        identity.push_str(&format!(":{}:{reference}", reference.len()));
    }
    identity.push('>');
    identity
}

/// Identity for a bare identifier reference this extractor could not otherwise read a value
/// from: the *symbol* its reference resolves to -- keyed on the declaration's own span, not its
/// name, so two sibling scopes that each declare their own same-named binding are never conflated
/// -- or, when it has no resolved symbol at all (a genuine global), its bare name. `file`,
/// rendered through [`path_key`], is folded into the resolved case for the same reason
/// [`condition_fingerprint`]'s doc gives: a span alone is only unique within one file. Shared by
/// [`condition_fingerprint`] and [`table_identity`] rather than fingerprinting a reference to a
/// bare name two different ways.
fn identifier_identity(
    ident: &IdentifierReference,
    semantic: &Semantic<'_>,
    file: &Path,
) -> String {
    let resolved_symbol = ident
        .reference_id
        .get()
        .and_then(|reference_id| semantic.scoping().get_reference(reference_id).symbol_id());
    match resolved_symbol {
        Some(symbol_id) => {
            let span = semantic.scoping().symbol_span(symbol_id);
            format!(
                "<local:{}:{}-{}:{}>",
                path_key(file),
                span.start,
                span.end,
                ident.name.as_str()
            )
        }
        None => format!("<global:{}>", ident.name.as_str()),
    }
}

/// Identity for a `.each`/`.for` table expression [`literal_table_rows`] could not read into
/// rows -- an identifier, a call, a spread, or any other shape it does not model. It never
/// claims to know the table's *values*. A literal folds to its own kind-tagged value via
/// [`const_fold_tagged`]; a bare identifier resolves to the symbol it names, via
/// [`identifier_identity`] -- the same scheme [`condition_fingerprint`] already uses for a bare
/// condition, reused rather than reinvented; anything else -- a call, a member expression, a
/// spread, ... -- falls back to [`expression_identity`], so `.each(getCasesA())` and
/// `.each(getCasesB())` are told apart by what they call, while same-spelled expressions referring
/// to different local bindings also remain distinct. Two genuinely different unreadable tables
/// are told apart by this; two occurrences of the exact same expression over the same bindings
/// are, correctly, not.
fn table_identity(expr: &Expression, semantic: &Semantic<'_>, source: &str, file: &Path) -> String {
    let expr = unwrap_parens(expr);
    if let Some(literal) = const_fold_tagged(expr) {
        return literal;
    }
    if let Expression::Identifier(ident) = expr {
        return identifier_identity(ident, semantic, file);
    }
    expression_identity(expr, semantic, source, file)
}

/// The `.each(table)`/`.for(table)` argument, resolved to its rows or -- so that two tables
/// nobody could read never compare equal -- an identity for the table expression when its rows
/// could not be read. `argument` is `None` when the call
/// carries no argument at all (`.each()`), which is as unreadable as any other shape.
fn table_rows(
    argument: Option<&Argument>,
    row_limit: usize,
    semantic: &Semantic<'_>,
    source: &str,
    file: &Path,
) -> Result<Vec<Row>, String> {
    match argument.and_then(Argument::as_expression) {
        Some(table) => literal_table_rows(table, row_limit, semantic, source, file),
        None => Err("<unresolved:no-table-argument>".to_owned()),
    }
}

/// Read static identifier/string property names, declining computed or unsupported keys.
pub(super) fn property_key_name(key: &PropertyKey) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.as_str().to_owned()),
        PropertyKey::StringLiteral(literal) => Some(literal.value.as_str().to_owned()),
        // A computed key (`[expr]: fn`) defeats static naming -- report it as unresolved rather
        // than guess, per `docs/vitest-surface.md`'s "Fixtures (`test.extend`)" section.
        _ => None,
    }
}

/// Read a literal array table using homogeneous-array spreading rules, retaining unresolved
/// cells and refusing unreadable tables. A table longer than `row_limit` is refused as
/// [`OVER_LIMIT_TABLE`] before any row is read.
fn literal_table_rows(
    expr: &Expression,
    row_limit: usize,
    semantic: &Semantic<'_>,
    source: &str,
    file: &Path,
) -> Result<Vec<Row>, String> {
    let Expression::ArrayExpression(array) = unwrap_parens(expr) else {
        // Not a literal array at all (an identifier, a call like `getCases()`, ...): the row
        // count is not statically knowable. Hazard 5: this must stay unreadable, not an empty
        // `Ok(vec![])`. The `Err` carries the table expression's own identity rather than erasing
        // it to a bare marker, so two tables nobody could read do not collapse to one value.
        return Err(table_identity(expr, semantic, source, file));
    };
    if array.elements.len() > row_limit {
        return Err(OVER_LIMIT_TABLE.to_owned());
    }
    #[cfg(test)]
    TABLE_ROWS_READ.with(|read| read.set(read.get() + array.elements.len()));
    // Vitest spreads a row into multiple arguments only when EVERY row is an array; a mixed
    // table (`it.each([1, 'a', [1, 2]])`) binds each row whole, so an array row inside one is a
    // single VALUE, not a positional spread. Verified against real Vitest 5 behavior, and it is
    // load-bearing: treating that array row as a spread made its first cell collide with the
    // scalar row (`[1, 2]` reading as `1` to a single-parameter callback), which reported the
    // two rows as the same case.
    let uniform_arrays = array.elements.iter().all(|element| {
        element
            .as_expression()
            .is_some_and(|expr| matches!(unwrap_parens(expr), Expression::ArrayExpression(_)))
    });
    let mut rows = Vec::with_capacity(array.elements.len());
    for element in &array.elements {
        match element {
            // A spread means the final element count depends on a runtime value. The table
            // expression is still known even though the row count is not -- identify the whole
            // array (its shape, including whatever precedes the spread), not just this element.
            ArrayExpressionElement::SpreadElement(_) => {
                return Err(table_identity(expr, semantic, source, file));
            }
            ArrayExpressionElement::Elision(_) => {
                // A hole is genuinely, statically known to be `undefined` -- not a value this
                // extractor failed to read, so it does not make the row `unresolved`.
                rows.push(Row {
                    display: String::new(),
                    cells: RowCells::Positional(Vec::new()),
                    unresolved: false,
                    // The hole's value is `undefined`, and that is what `%s` prints for it.
                    title_args: vec![Some(TitleArg {
                        coerced: "undefined".to_owned(),
                        inspected: "undefined".to_owned(),
                        // `JSON.stringify(undefined)` is `undefined`, not a JSON document.
                        json: None,
                        field: "undefined".to_owned(),
                        // `Number(undefined)` is `NaN`, which refuses.
                        number: None,
                    })],
                });
            }
            _ => {
                let Some(cell_expr) = element.as_expression() else {
                    return Err(table_identity(expr, semantic, source, file));
                };
                if !uniform_arrays
                    && matches!(unwrap_parens(cell_expr), Expression::ArrayExpression(_))
                {
                    rows.push(analyze_whole_row(cell_expr, source, Some(semantic)));
                } else {
                    rows.push(analyze_row(cell_expr, source, Some(semantic)));
                }
            }
        }
    }
    Ok(rows)
}

/// A row of a mixed table, bound whole: exactly one cell, folding the entire row expression.
///
/// The whole row folds to ONE cell holding the whole array, under the `array` kind. That kind tag
/// is what keeps this row from colliding with a scalar row beside it: `[1]` is `<array:...>` and
/// `1` is `<number:1>`, so the two can never compare equal -- which is the collision that made
/// this whole binding rule necessary, now prevented by the tag rather than by refusing to read
/// the array at all.
///
/// Measured against Vitest 5.0.1: a mixed table really does bind each row whole (a rest-parameter
/// callback receives `[[1, 2]]`, one argument), while Vitest's *title* formatter spreads the same
/// array. Binding is what comparison depends on, so binding is what this follows; the title is
/// rendered from the array's elements rather than from the bound value (see [`Row::title_args`]).
fn analyze_whole_row(cell_expr: &Expression, source: &str, resolve: Resolve<'_, '_>) -> Row {
    // Folded exactly once (Copilot review, PR #28): a second fold of the same cell double-counts
    // it in the `VD_CELL_CENSUS` histogram and halves nothing else -- both fields below derive
    // from this one value.
    let value = fold_cell(cell_expr, resolve);
    let unresolved = value.is_none();
    Row {
        display: source_text(source, cell_expr.span()),
        cells: RowCells::Positional(vec![value]),
        unresolved,
        // Spread, even though the binding above is whole: see `Row::title_args`.
        title_args: title_args_for(cell_expr, resolve),
    }
}

/// Defect 4: a table this extractor could enumerate the *shape* of (an array literal, so
/// [`literal_table_rows`] returns `Some`) does not mean every row's *values* are known -- a cell
/// can be an identifier, a call, or anything else [`const_fold`] refuses to guess at. Two rows in
/// different scopes whose unresolved cell happens to render the same display text (e.g. a shared
/// local name bound to different runtime data in each scope) must not be allowed to compare equal
/// on that text alone, so each row now also reports whether any of its own cells were unresolved.
/// Folds one row cell expression to a [`CellValue`], when [`const_fold`] can read it at all.
///
/// `const_fold` and [`const_fold_kind`] are deliberately called on the very same expression here,
/// not derived from one another, so a future edit to either cannot silently desynchronize
/// `display` from `kind`/`raw` -- both already accept exactly the same expression shapes (see
/// `const_fold_kind`'s doc), so this never observes one returning `Some` while the other returns
/// `None`.
/// Folds one row cell expression to a [`CellValue`], when [`const_fold`] can read it at all --
/// and, under `VD_CELL_CENSUS=1`, records why when it cannot.
///
/// The census is T1's re-measurement instrument (backlog): the 2,462-cell figure that sized that
/// row came from a branch that no longer exists, so the instrumentation lives in the tree now and
/// caches the env setting once when the variable is unset. A re-measure answers "which
/// refusal causes remain, and how large" -- the number that decides whether `call` and `function`
/// cells (deliberately unread: they are runtime values) are the whole story or not.
fn fold_cell(expr: &Expression, resolve: Resolve<'_, '_>) -> Option<CellValue> {
    // `display` is the `$field` title rendering -- the only consumer is title assembly, so the
    // budget mirror applies. The fingerprint-facing `raw` is untouched: a non-injective
    // rendering there would be an Error-severity false merge.
    let value = const_fold_kind(expr, resolve).and_then(|(kind, raw)| {
        Some(CellValue {
            display: field_within_budget(expr, || inspect_within_budget(expr, resolve), resolve)?,
            kind,
            raw,
        })
    });
    if value.is_none() {
        census_refusal(expr, resolve);
    }
    value
}

/// The census histogram, printed once at the end of a `VD_CELL_CENSUS=1` run by the CLI.
pub fn take_cell_census() -> Vec<(String, u64)> {
    with_cell_census(|census| {
        let mut counts: Vec<_> = census.counts.drain().collect();
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        counts
    })
}

#[derive(Clone, Default)]
pub(super) struct CellCensus {
    counts: HashMap<String, u64>,
    // Spans are unique within one file, even when the Walker rebuilds its table.
    counted: HashSet<(u32, u32)>,
    #[cfg(test)]
    enabled: bool,
}

/// Run an operation against the shared census under its mutex, or the thread-local census in
/// tests.
#[cfg(not(test))]
fn with_cell_census<T>(operation: impl FnOnce(&mut CellCensus) -> T) -> T {
    static CENSUS: OnceLock<Mutex<CellCensus>> = OnceLock::new();
    operation(
        &mut CENSUS
            .get_or_init(Mutex::default)
            .lock()
            .expect("cell census mutex"),
    )
}

// Unit tests extract files concurrently. Give each test thread its own census rather than
// changing the process environment or sharing a file-scope dedup set with unrelated tests.
#[cfg(test)]
std::thread_local! {
    static TEST_CELL_CENSUS: std::cell::RefCell<CellCensus> = Default::default();
}

/// Run an operation against the shared census under its mutex, or the thread-local census in
/// tests.
#[cfg(test)]
fn with_cell_census<T>(operation: impl FnOnce(&mut CellCensus) -> T) -> T {
    TEST_CELL_CENSUS.with(|census| operation(&mut census.borrow_mut()))
}

/// Clone the current test-thread census so a fixture can restore it afterward.
#[cfg(test)]
pub(super) fn test_cell_census_snapshot() -> CellCensus {
    with_cell_census(|census| census.clone())
}

/// Replace the test-thread census with a previously captured snapshot.
#[cfg(test)]
pub(super) fn restore_test_cell_census(snapshot: CellCensus) {
    with_cell_census(|census| *census = snapshot);
}

/// Run a test body with a fresh enabled census and restore the previous state when the guard
/// drops.
#[cfg(test)]
fn with_test_census<T>(body: impl FnOnce() -> T) -> T {
    struct Restore(CellCensus);
    impl Drop for Restore {
        /// Restore the saved thread-local census when the test guard leaves scope.
        fn drop(&mut self) {
            TEST_CELL_CENSUS.with(|census| census.replace(std::mem::take(&mut self.0)));
        }
    }
    let previous = TEST_CELL_CENSUS.with(|census| {
        census.replace(CellCensus {
            enabled: true,
            ..CellCensus::default()
        })
    });
    let _restore = Restore(previous);
    body()
}

/// Starts a new file's dedup scope. The CLI extracts one file at a time.
pub(super) fn reset_cell_census_file_scope() {
    with_cell_census(|census| census.counted.clear());
}

/// The opt-in is fixed for a CLI run; tests enable their own thread's census explicitly.
fn census_enabled() -> bool {
    #[cfg(test)]
    {
        with_cell_census(|census| census.enabled)
    }
    #[cfg(not(test))]
    {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| std::env::var_os("VD_CELL_CENSUS").is_some())
    }
}

/// Record a refused cell once per span when census collection is enabled.
fn census_refusal(expr: &Expression, resolve: Resolve<'_, '_>) {
    if !census_enabled() {
        return;
    }
    let span = expr.span();
    if !mark_census_cell(span) {
        return;
    }
    record_census_refusal(cell_refusal_cause(expr, resolve));
}

/// Marks one cell span as counted in the current file scope; `false` when already counted.
fn mark_census_cell(span: oxc_span::Span) -> bool {
    with_cell_census(|census| census.counted.insert((span.start, span.end)))
}

/// Increment the census count for the supplied refusal cause.
fn record_census_refusal(cause: String) {
    with_cell_census(|census| *census.counts.entry(cause).or_insert(0) += 1);
}

/// Why one refused cell could not be read, as a census key.
///
/// Composites are dug into so the histogram names the inner cause (`object: call`, not
/// `object`). A refusal whose resolution *survived* the binding chain -- possible only when the
/// fold refused for a reason resolution does not share (the depth bounds differ) -- is
/// annotated (`identifier (through a binding)`), keeping the histogram honest about which of
/// the two refusers fired.
fn cell_refusal_cause(expr: &Expression, resolve: Resolve<'_, '_>) -> String {
    match unwrap_transparent(expr) {
        Expression::Identifier(_) => {
            match resolve.and_then(|semantic| resolved_binding(expr, semantic, 0)) {
                Some(_) => "identifier (through a binding)".to_owned(),
                None => "identifier".to_owned(),
            }
        }
        Expression::StaticMemberExpression(_) => {
            match resolve.and_then(|semantic| resolved_binding(expr, semantic, 0)) {
                Some(_) => "static member (through a binding)".to_owned(),
                None => "static member".to_owned(),
            }
        }
        Expression::CallExpression(_) => "call".to_owned(),
        Expression::NewExpression(_) => "new".to_owned(),
        Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_) => {
            "function".to_owned()
        }
        Expression::TemplateLiteral(template) if !template.expressions.is_empty() => {
            for substitution in &template.expressions {
                if const_fold_kind(substitution, resolve).is_none() {
                    return format!("template: {}", cell_refusal_cause(substitution, resolve));
                }
            }
            "template".to_owned()
        }
        Expression::ArrayExpression(_) => match array_elements(expr) {
            None => "array: spread or hole".to_owned(),
            Some(elements) => {
                for element in elements {
                    if const_fold_kind(element, resolve).is_none() {
                        return format!("array: {}", cell_refusal_cause(element, resolve));
                    }
                }
                "array".to_owned()
            }
        },
        Expression::ObjectExpression(_) => match object_fields(expr) {
            None => "object: computed key, spread, shorthand, or method".to_owned(),
            Some(fields) => {
                for (_, value) in fields {
                    if const_fold_kind(value, resolve).is_none() {
                        return format!("object: {}", cell_refusal_cause(value, resolve));
                    }
                }
                "object".to_owned()
            }
        },
        _ => "other".to_owned(),
    }
}

/// The file's semantic, when a fold is running somewhere that has one.
///
/// `None` everywhere a fold runs without a file behind it (unit tests, and -- deliberately --
/// [`const_fold_tagged`]'s fixture values, whose contract treats an identifier as `Unresolved`
/// on purpose: a fixture's *value* is compared, and pretending a same-named local in two scopes
/// is one value is the exact defect `const_fold_tagged` exists to prevent).
pub(super) type Resolve<'s, 'a> = Option<&'s Semantic<'a>>;

/// How many binding hops a cell value may follow while resolving (see [`resolved_binding`]). A
/// bound, not a judgement: `const` cycles are impossible in valid JavaScript (TDZ), so this
/// guards against pathological chains and nothing real.
const MAX_VALUE_HOPS: u32 = 8;

/// Follows an identifier or static member expression to the expression it is statically bound
/// to in this file, or `None`.
///
/// T1 (backlog): a `.each` cell that *names* a value -- `test.each([[MODE]])` with
/// `const MODE = 'fast'` above it -- was unreadable as a class: 695 identifier cells and 243
/// static members on the census that sized that row, plus the object and array rows whose
/// *contents* they poisoned, and every one of them excluded its whole row -- and every test in
/// that row -- from comparison. A `const` binding's initializer is statically THE value: `const`
/// cannot be reassigned, and a reference that executes before the initializer is a TDZ crash
/// rather than a silently different value, so following it is a proof, not a guess. `let`/`var`
/// bindings are refused: `var` is hoisted (a reference above the declaration legitimately
/// observes `undefined`), and both admit reassignment, so an initializer is evidence there, not
/// a value.
///
/// Resolution is by symbol identity, never by name -- the same discipline [`resolve_identifier`]
/// applies to callees -- so two sibling scopes that each declare their own `MODE` resolve to
/// their own values and never collide, and an imported binding (whose declaration is an import
/// statement, not a `const`) is refused, keeping cross-file value resolution in C7's territory.
/// The one `const` declarator that declares `symbol_id`, when the binding is a plain
/// `const name = ...` with a single declaration.
///
/// Exactly one declaration: `const` cannot be redeclared in a scope, so two means a merged
/// TypeScript shape (a `const` plus an `interface` of the same name) -- refuse rather than guess
/// which one owns the value. oxc records the declarator's own node as the declaration for
/// lexical bindings, but this also accepts the declaration node itself, so an oxc version that
/// shifts the anchor by one parent cannot silently turn every read into a refusal.
fn const_declarator<'a>(
    semantic: &Semantic<'a>,
    symbol_id: SymbolId,
) -> Option<&'a VariableDeclarator<'a>> {
    let nodes = semantic.nodes();
    let declarations: Vec<NodeId> = semantic.scoping().symbol_declarations(symbol_id).collect();
    if declarations.len() != 1 {
        return None;
    }
    let is_const = |kind: VariableDeclarationKind| kind == VariableDeclarationKind::Const;
    // The declarator's pattern must be the plain identifier that declares THIS symbol (Copilot
    // review, PR #28 round 2): oxc anchors a destructured binding to the same declarator node,
    // and `const { a } = { foo: 1, a: 2 }` would otherwise let a member read off `a` treat the
    // whole initializer as `a`'s value -- rendering `a.foo` as `1` where runtime `a` is `2` and
    // has no such property.
    let declares_symbol = |declarator: &VariableDeclarator| {
        matches!(
            &declarator.id,
            BindingPattern::BindingIdentifier(binding)
                if binding.symbol_id.get() == Some(symbol_id)
        )
    };
    match nodes.kind(declarations[0]) {
        AstKind::VariableDeclarator(declarator) => {
            if !declares_symbol(declarator) {
                return None;
            }
            match nodes.parent_kind(declarations[0]) {
                AstKind::VariableDeclaration(parent) if is_const(parent.kind) => Some(declarator),
                _ => None,
            }
        }
        AstKind::VariableDeclaration(declaration) if is_const(declaration.kind) => declaration
            .declarations
            .iter()
            .find(|declarator| declares_symbol(declarator)),
        _ => None,
    }
}

/// TDZ guard (Copilot review, PR #28 round 4): a `const` declarator only backs a static read
/// when it is initialized before the read executes.
///
/// Module and block bodies run in source order, so a declarator textually AFTER the reference
/// means the read executes while the binding is still in its temporal dead zone --
/// `test.each([[MODE], ['fast']])` written above `const MODE = 'fast'` throws a
/// `ReferenceError` while the table is being built and registers no tests at all. Folding the
/// row anyway would report a duplicate Vitest never creates, so the read refuses instead. The
/// recall cost is confined to reads that execute later than their own text (a closure called
/// after initialization), which table cells never are: a cell is a value expression, evaluated
/// eagerly where it is written. `use_span` is the reference's own span.
fn declaration_precedes(declarator: &VariableDeclarator, use_span: oxc_span::Span) -> bool {
    declarator.span().start <= use_span.start
}

/// Follow supported earlier const bindings within the value-hop limit, declining unavailable or
/// unsafe references.
fn resolved_binding<'e, 'a: 'e>(
    expr: &'e Expression<'a>,
    semantic: &Semantic<'a>,
    hops: u32,
) -> Option<&'e Expression<'a>> {
    if hops >= MAX_VALUE_HOPS {
        return None;
    }
    match unwrap_transparent(expr) {
        Expression::Identifier(ident) => {
            let reference_id = ident.reference_id.get()?;
            let symbol_id = semantic.scoping().get_reference(reference_id).symbol_id()?;
            let declarator = const_declarator(semantic, symbol_id)?;
            if !declaration_precedes(declarator, ident.span()) {
                return None;
            }
            let init = declarator.init.as_ref()?;
            let resolved = resolved_binding(init, semantic, hops + 1).unwrap_or(init);
            // An IMMUTABLE value only. A scalar cannot be changed out from under this read
            // (primitives are values, and `const` bars reassignment), which is what makes
            // following the binding a proof. A composite can: `const L = { n: 1 }; L.n = 2` is
            // legal, mutates the very object the initializer describes, and would leave this
            // reading a stale 1 where the runtime observes 2 -- so composite initializers stay
            // unread as whole values. (Member reads off them are the member arm below, with
            // their own discipline.)
            is_immutable_value(resolved, semantic, hops + 1).then_some(resolved)
        }
        Expression::StaticMemberExpression(member) => {
            // The object must be a `const` in this file whose initializer is an object literal:
            // `LEVELS.high` where `const LEVELS = { high: 3 }`. An imported namespace, a call
            // result, or anything else refuses.
            let Expression::Identifier(object_ident) = unwrap_transparent(&member.object) else {
                return None;
            };
            let reference_id = object_ident.reference_id.get()?;
            let symbol_id = semantic.scoping().get_reference(reference_id).symbol_id()?;
            let declarator = const_declarator(semantic, symbol_id)?;
            if !declaration_precedes(declarator, object_ident.span()) {
                return None;
            }
            let Expression::ObjectExpression(object) =
                unwrap_transparent(declarator.init.as_ref()?)
            else {
                return None;
            };
            // The object must never be written through or allowed to escape anywhere in this
            // file: `LEVELS.high = 4`, `delete LEVELS.high`, `++LEVELS.high` all make the
            // initializer stale, and so does passing `LEVELS` to anything that might mutate it
            // (`Object.assign(LEVELS, ...)`), which this cannot see through. See
            // [`object_is_only_member_read`] for the exact rule.
            if !object_is_only_member_read(semantic, symbol_id) {
                return None;
            }
            // Last write wins under a repeated static key, the same normalization
            // [`object_fields`] applies. Computed keys and spreads refuse: either could define
            // or shadow the name being read, and a miss is safe while a wrong hit is not.
            let mut value = None;
            for property in &object.properties {
                let ObjectPropertyKind::ObjectProperty(property) = property else {
                    return None;
                };
                if property.computed || property.method {
                    return None;
                }
                // An ACCESSOR fires when its key is read: the file may read that key as a
                // plain member -- which the escape rules below allow, since it IS a read --
                // and the getter can mutate anything on the object before the table is built
                // (verified: a getter on `other` moved `high` to 4). Refuse the whole object
                // rather than model which keys are read when (Copilot review, PR #28 round 6).
                if property.kind != PropertyKind::Init {
                    return None;
                }
                // A non-shorthand `__proto__` entry is the prototype SETTER, not an own data
                // property: `{ __proto__: 3 }` leaves the object with no own keys, and a later
                // `L.__proto__` reads `Object.prototype` -- not 3 (verified in node). Shorthand
                // `{ __proto__ }` is a real data property and stays readable.
                if !property.shorthand
                    && property_key_name(&property.key).as_deref() == Some("__proto__")
                {
                    return None;
                }
                if property_key_name(&property.key).as_deref()
                    == Some(member.property.name.as_str())
                {
                    value = Some(&property.value);
                }
            }
            let value = value?;
            let resolved = resolved_binding(value, semantic, hops + 1).unwrap_or(value);
            is_immutable_value(resolved, semantic, hops + 1).then_some(resolved)
        }
        _ => None,
    }
}

/// Whether `expr` is a value JavaScript cannot change out from under a reader: a scalar literal,
/// or a fold of scalars. Primitives are immutable and a `const` cannot be reassigned, so a
/// `const` whose initializer is THIS is stable for the life of the file; an object or array
/// never is, whatever the binding kind (`const L = [1]; L.push(2)`).
fn is_immutable_value<'a>(expr: &Expression<'a>, semantic: &Semantic<'a>, hops: u32) -> bool {
    if hops >= MAX_VALUE_HOPS {
        return false;
    }
    match unwrap_transparent(expr) {
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_) => true,
        // A nested identifier is immutable exactly when the value it names is -- which is
        // precisely what [`resolved_binding`] proves (a `const` chain ending in a scalar), so
        // this defers to it rather than restating a weaker version. Copilot review, PR #28
        // round 3: `const A = 'x'; const B = A + 'b'` refused outright because the identifier
        // inside the initializer's binary expression read as "not a literal", blocking the
        // peel of B -- and with the peel gone, the fold never sees the binary at all, since it
        // only receives the cell's own (unpeeled) expression.
        Expression::Identifier(_) => resolved_binding(expr, semantic, hops + 1).is_some(),
        Expression::TemplateLiteral(template) => template
            .expressions
            .iter()
            .all(|expression| is_immutable_value(expression, semantic, hops)),
        Expression::UnaryExpression(unary) => {
            matches!(
                unary.operator,
                UnaryOperator::UnaryNegation | UnaryOperator::UnaryPlus
            ) && is_immutable_value(&unary.argument, semantic, hops)
        }
        // Every arithmetic, comparison, and logical operator over scalars produces a scalar.
        Expression::BinaryExpression(binary) => {
            is_immutable_value(&binary.left, semantic, hops)
                && is_immutable_value(&binary.right, semantic, hops)
        }
        _ => false,
    }
}

/// Whether every reference to a const object binding in this file reads it ONLY as the object of
/// a member access, in a position nothing writes through -- the discipline
/// [`resolved_binding`]'s member arm requires before reading a property off the declared
/// literal.
///
/// Two refusals, each naming a way the declared literal could go stale before the `.each` table
/// is constructed:
///
/// * **Written through**: oxc marks the object identifier of any member expression in a write
///   context (`LEVELS.high = 4`, `LEVELS[k] += 1`, `++LEVELS.high`, `delete LEVELS.high`,
///   `for (LEVELS.k in xs)`) with [`ReferenceFlags::MemberWriteTarget`], at any chain depth --
///   read straight off the reference, no AST walking of our own.
/// * **Allowed to escape**: every remaining reference must sit in object position of a member
///   access. A reference passed to a call (`Object.assign(LEVELS, ...)`), aliased
///   (`const other = LEVELS`), spread, or used as a whole value could be mutated by code this
///   cannot see, so one such reference anywhere refuses member reads off the object for the
///   whole file. A missed read costs recall; a stale value would cost precision.
fn object_is_only_member_read(semantic: &Semantic<'_>, symbol_id: SymbolId) -> bool {
    let scoping = semantic.scoping();
    let nodes = semantic.nodes();
    scoping
        .get_resolved_reference_ids(symbol_id)
        .iter()
        .all(|reference_id| {
            let reference = scoping.get_reference(*reference_id);
            if reference
                .flags()
                .contains(ReferenceFlags::MemberWriteTarget)
            {
                return false;
            }
            let span = nodes.kind(reference.node_id()).span();
            let parent_id = nodes.parent_id(reference.node_id());
            let in_member_object = match nodes.kind(parent_id) {
                AstKind::StaticMemberExpression(member) => member.object.span() == span,
                AstKind::ComputedMemberExpression(member) => member.object.span() == span,
                _ => return false,
            };
            in_member_object && !member_chain_is_invoked(nodes, parent_id)
        })
}

/// Whether the member chain whose innermost link is `chain_id` is INVOKED somewhere up its
/// length (Copilot review, PR #28 round 5): `LEVELS.mutate()` binds `this` to `LEVELS`, so a
/// method on the object can mutate it through references that all sit in member-object
/// position -- the one shape the write-through and escape checks below both miss. The chain
/// climbs through further member links (and parentheses, which oxc preserves) and refuses
/// only when the chain's top is a call/new CALLEE; a chain passed as a value
/// (`foo(LEVELS.high)`) or detached (`const m = LEVELS.mutate`, which loses `this`) cannot
/// mutate the object and stays readable.
fn member_chain_is_invoked(nodes: &AstNodes<'_>, mut chain_id: oxc_semantic::NodeId) -> bool {
    loop {
        let parent_id = nodes.parent_id(chain_id);
        let chain_span = nodes.kind(chain_id).span();
        match nodes.kind(parent_id) {
            AstKind::StaticMemberExpression(member) if member.object.span() == chain_span => {
                chain_id = parent_id;
            }
            AstKind::ComputedMemberExpression(member) if member.object.span() == chain_span => {
                chain_id = parent_id;
            }
            AstKind::ParenthesizedExpression(paren) if paren.expression.span() == chain_span => {
                chain_id = parent_id;
            }
            AstKind::CallExpression(call) => return call.callee.span() == chain_span,
            AstKind::NewExpression(new) => return new.callee.span() == chain_span,
            // A member used as a TAGGED-TEMPLATE tag is invoked with the object as `this`,
            // exactly like a call: `LEVELS.mutate`` ` `` ran and moved `high` to 4 (verified).
            // (Copilot review, PR #28 round 6.)
            AstKind::TaggedTemplateExpression(tag) => return tag.tag.span() == chain_span,
            _ => return false,
        }
    }
}

/// [`Resolve`] applied to one expression: what it resolves through, or itself unchanged.
fn peeled<'e, 'a: 'e>(expr: &'e Expression<'a>, resolve: Resolve<'_, 'a>) -> &'e Expression<'a> {
    resolve
        .and_then(|semantic| resolved_binding(expr, semantic, 0))
        .unwrap_or(expr)
}

/// Extract positional/named row cells and title arguments, retaining unresolved evidence for
/// unsupported values.
fn analyze_row(cell_expr: &Expression, source: &str, resolve: Resolve<'_, '_>) -> Row {
    let display = source_text(source, cell_expr.span());
    let (cells, unresolved) = match unwrap_parens(cell_expr) {
        Expression::ArrayExpression(inner) => {
            let mut unresolved = false;
            let values = inner
                .elements
                .iter()
                .map(|element| match element {
                    // An elision is a known `undefined`, not an unresolved value.
                    ArrayExpressionElement::Elision(_) => None,
                    ArrayExpressionElement::SpreadElement(_) => {
                        unresolved = true;
                        None
                    }
                    _ => {
                        let value = element
                            .as_expression()
                            .and_then(|element| fold_cell(element, resolve));
                        if value.is_none() {
                            unresolved = true;
                        }
                        value
                    }
                })
                .collect();
            (RowCells::Positional(values), unresolved)
        }
        Expression::ObjectExpression(object) => {
            // Defect (Copilot review #8): a JS object literal resolves a repeated static key to
            // its *last* value -- `{ amount: 'GBP', amount: 'USD' }` binds `amount` to `'USD'` at
            // runtime, never `'GBP'`. Every downstream consumer (the title renderer and
            // `fingerprint.rs`'s row-substitution) looks a name up with `.find`, which returns the
            // *first* match, so an unnormalized `Named` row let them observe a value Vitest itself
            // never produces. `fields_by_name` makes this a non-issue for those consumers: by the
            // time a `Named` row leaves this function, each name appears at most once, so `.find`
            // trivially returns "the" match -- there is nothing left for those call sites to get
            // wrong, and they do not need to change.
            //
            // Position still follows JS's own semantics too: a repeated key keeps its *first*
            // insertion index (`Object.keys({ a: 1, a: 2 })` is `["a"]`, and inserting `b` between
            // two `a`s gives `["a", "b"]`, not `["b", "a"]`) -- only the *value* moves to the last
            // occurrence. `fields_by_name` tracks each name's index in `fields` so a repeat
            // overwrites in place rather than appending.
            let mut unresolved = false;
            let mut fields: Vec<(String, Option<CellValue>)> = Vec::new();
            let mut fields_by_name: HashMap<String, usize> = HashMap::new();
            for property in &object.properties {
                match property {
                    ObjectPropertyKind::ObjectProperty(property) => {
                        // A computed key here would drop the field *silently*, leaving a row that
                        // looks completely known but is missing a member -- so an unnamed key is
                        // handled the same way the spread arm below handles its own.
                        let Some(name) = property_key_name(&property.key) else {
                            unresolved = true;
                            continue;
                        };
                        let value = fold_cell(&property.value, resolve);
                        // Last-wins: a value this extractor cannot read still wins over an
                        // earlier, readable one for the same key, because that is exactly what
                        // happens at runtime -- the earlier value is simply never observed. This
                        // can make an otherwise-fully-known row unresolved (the row's *only*
                        // surviving value is the unknown one), or -- the case this fix must not
                        // get backwards -- it can leave a row resolved when only a *discarded*
                        // earlier occurrence was unreadable, since nothing at runtime ever sees
                        // that earlier value either.
                        match fields_by_name.get(&name) {
                            Some(&index) => fields[index].1 = value,
                            None => {
                                fields_by_name.insert(name.clone(), fields.len());
                                fields.push((name, value));
                            }
                        }
                    }
                    // A spread's contribution to this row is not statically enumerable, and (like
                    // a computed key above) it could itself be shadowing or be shadowed by any
                    // other key here -- both leave the row unresolved rather than silently
                    // under- or mis-representing its fields.
                    ObjectPropertyKind::SpreadProperty(_) => {
                        unresolved = true;
                    }
                }
            }
            // Exactly one value per name survives above, so this only has to ask whether *that*
            // surviving value is readable -- an earlier, overwritten occurrence's unreadability
            // never reaches here.
            if fields.iter().any(|(_, value)| value.is_none()) {
                unresolved = true;
            }
            (RowCells::Named(fields), unresolved)
        }
        other => {
            let value = fold_cell(other, resolve);
            let unresolved = value.is_none();
            (RowCells::Positional(vec![value]), unresolved)
        }
    };
    let title_args = title_args_for(cell_expr, resolve);
    Row {
        display,
        cells,
        unresolved,
        title_args,
    }
}

/// The printf arguments Vitest would format this row's title with.
///
/// An array row spreads into one argument per element; anything else is a single argument. That
/// holds for a uniform table (where the binding spreads too) and for an array row inside a mixed
/// table (where the binding does NOT) -- the title formatter does not consult the uniformity rule
/// that governs binding, which is why this is computed from the row expression alone.
fn title_args_for(cell_expr: &Expression, resolve: Resolve<'_, '_>) -> Vec<Option<TitleArg>> {
    match unwrap_transparent(cell_expr) {
        Expression::ArrayExpression(array) => array
            .elements
            .iter()
            .map(|element| {
                element
                    .as_expression()
                    .and_then(|element| title_arg(element, resolve))
            })
            .collect(),
        other => vec![title_arg(other, resolve)],
    }
}

/// One argument in all three renderings. `%s` is the one that must succeed: without it there is
/// no argument at all, and the title falls back to its pattern.
fn title_arg(expr: &Expression, resolve: Resolve<'_, '_>) -> Option<TitleArg> {
    // `$field` of a non-string IS the inspect rendering (Vitest's `formatAttribute` calls the
    // same `inspect`), so the budget pipeline runs once and serves both.
    let inspected = inspect_within_budget(expr, resolve)?;
    Some(TitleArg {
        coerced: render_coerce(expr, 0, resolve)?,
        field: field_within_budget(expr, || Some(inspected.clone()), resolve)?,
        inspected,
        json: render_json(expr, 0, resolve),
        // Classified on the RESOLVED expression (Copilot review, PR #28): `const N = 10n` followed
        // by `[[N]]` must render exactly as `[[10n]]` does, and without this the `%i` arm would
        // prefix-parse `10n` into a confidently wrong `10`.
        number: match unwrap_transparent(peeled(expr, resolve)) {
            Expression::BigIntLiteral(literal) => {
                Some(TitleNumber::BigInt(format!("{}n", literal.value.as_str())))
            }
            _ => js_number_value(expr, 0, resolve).map(TitleNumber::Number),
        },
    })
}

/// `Number(value)` over a statically known cell, for the `%d` directive.
///
/// Measured on Vitest 5.0.3 (`docs/vitest-surface.md` §4): `%d` is `Number(value).toString()`,
/// which is a different function on a different input than the `parseInt`/`parseFloat` that
/// `%i`/`%f` apply to the value's string form -- `null` is `0` and `true` is `1` here, and both
/// are `NaN` there. An array reaches this through its string form (`ToString`, i.e. `join`), so
/// `[]` and `[null]` are both `''` are both `0`, while `[true]` is `'true'` and refuses.
///
/// `None` whenever the result would be `NaN` or cannot be determined: `NaN` renders as the
/// non-discriminating title `NaN` (18 of `duplicate-each-row`'s 41 false positives on the
/// 2026-09-27 scan were exactly that shape), so the renderer refuses it rather than trades a
/// recall gap for a precision one.
fn js_number_value(expr: &Expression, depth: u32, resolve: Resolve<'_, '_>) -> Option<f64> {
    if depth > MAX_CELL_DEPTH {
        return None;
    }
    let expr = peeled(expr, resolve);
    match unwrap_transparent(expr) {
        Expression::NullLiteral(_) => Some(0.0),
        Expression::BooleanLiteral(literal) => Some(if literal.value { 1.0 } else { 0.0 }),
        Expression::NumericLiteral(literal) => Some(literal.value),
        // Deliberately NOT `Number(10n)` = `10`: Vitest's `%d` prints a BigInt cell as `10n`,
        // and a correct model of the wrong function is a wrong answer (see `TitleArg::number`).
        Expression::BigIntLiteral(_) => None,
        Expression::UnaryExpression(_) => fold_signed_number(unwrap_transparent(expr), resolve),
        // String-valued foldables go through `Number(string)`: `'1' + '2'` is `'12'` and then 12.
        // Numeric arithmetic takes the numeric path instead -- a `1 / 0` cell folds to `Infinity`
        // as a NUMBER, while its display text carries a Rust `inf` spelling that `Number(string)`
        // would (correctly) refuse.
        inner @ (Expression::StringLiteral(_) | Expression::TemplateLiteral(_)) => {
            number_of_string(&const_fold(inner, resolve)?)
        }
        Expression::BinaryExpression(_) => {
            // A string-valued binary (`'0x' + '10'`) IS a string, and `Number` reads it with
            // the full string grammar -- radix prefixes included (`Number('0x' + '10')` is
            // 16, verified) -- exactly as the direct string arms above do. The numeric path
            // would parse the folded text as Rust decimal and refuse (Copilot review, PR #28
            // round 6).
            if is_string_valued(unwrap_transparent(expr), resolve) {
                return number_of_string(&const_fold(unwrap_transparent(expr), resolve)?);
            }
            let value = const_fold_number(unwrap_transparent(expr), resolve)?;
            (!value.is_nan()).then_some(value)
        }
        // `Number(array)` is `Number(ToString(array))` -- the join, not the inspect form -- so
        // this reuses the ToString renderer rather than guessing elementwise.
        Expression::ArrayExpression(_) => {
            number_of_string(&render_js_to_string(expr, depth, resolve)?)
        }
        // A plain object is `NaN` (`[object Object]` is not numeric); an object with a
        // `valueOf`/`toString` could be anything. Both refuse.
        _ => None,
    }
}

/// The `0x`/`0b`/`0o` radix prefixes, either capitalization, as `Number` and `parseInt` both
/// read them (and `parseFloat` does not). Returns the radix and the text after the prefix.
pub(super) fn split_radix_prefix(text: &str) -> Option<(u32, &str)> {
    for (prefix, radix) in [
        ("0x", 16u32),
        ("0X", 16),
        ("0b", 2),
        ("0B", 2),
        ("0o", 8),
        ("0O", 8),
    ] {
        if let Some(digits) = text.strip_prefix(prefix) {
            return Some((radix, digits));
        }
    }
    None
}

/// The Number JavaScript computes for `digits` in a power-of-two `radix` (16, 8, or 2),
/// rounded to nearest-even from the EXACT mathematical value, as the spec mandates.
///
/// The bounded `i128::from_str_radix` this replaces refused 32+ hex digits (Copilot review,
/// PR #28 round 5) where JavaScript renders the rounded Number -- both `parseInt` and
/// `Number` give `3.402823669209385e+38` for thirty-two `f`s -- and the same bound hid binary
/// and octal tables on the `%d` path (`Number('0b' + '1'.repeat(200))` is
/// `1.6069380442589903e+60`). Decimal strings never had the problem: `f64::from_str` is
/// correctly rounded from the decimal form at any length and overflows to `Infinity`, which
/// is the JavaScript answer there too.
///
/// A power-of-two radix makes the exact value a bit string, so correct rounding needs only
/// the top 53 bits, the bit after them, and whether anything further is nonzero; the exponent
/// is exact because it is a power of two, so the constructed f64 IS the correctly rounded
/// value, boundary cases included (values within half an ULP below 2^1024 stay finite, at or
/// above it become `Infinity`). A sign is REFUSED here: `Number('-0x10')` is `NaN`, the radix
/// grammar takes no sign -- `parseInt` accepts one, and its caller applies it before calling.
pub(super) fn js_radix_digits_value(digits: &str, radix: u32) -> Option<f64> {
    debug_assert!(matches!(radix, 2 | 8 | 16));
    let bits_per_digit: u64 = match radix {
        16 => 4,
        8 => 3,
        _ => 1,
    };
    let digit_value = |byte: u8| -> Option<u32> {
        let digit = match byte {
            b'0'..=b'9' => (byte - b'0') as u32,
            b'a'..=b'f' => (byte - b'a') as u32 + 10,
            b'A'..=b'F' => (byte - b'A') as u32 + 10,
            _ => return None,
        };
        (digit < radix).then_some(digit)
    };
    if digits.is_empty() || digits.starts_with('-') || digits.starts_with('+') {
        // `Number('0x')` and `Number('0x-1')` are both NaN; the `parseInt` caller applies its
        // own sign before handing digits over, so neither shape is a value here.
        return None;
    }
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some(0.0); // `Number('0x000')` is 0.
    }
    let n = digits.len() as u64;
    let bytes = digits.as_bytes();
    let total_bits = n * bits_per_digit;
    // Fits exactly: the integer-to-float cast rounds to nearest, ties to even.
    if total_bits <= 127 {
        let mut value: u128 = 0;
        for byte in bytes {
            value = (value << bits_per_digit) | digit_value(*byte)? as u128;
        }
        return Some(value as f64);
    }
    // Long path. The value's bit length discounts the top digit's leading zero bits; the
    // rounding window (53 bits + round bit + sticky) must clear them, so the buffer holds
    // enough digits that at least 54 REAL bits are covered.
    let top_digit = digit_value(bytes[0])?;
    let mut n_bits = (n - 1) * bits_per_digit + top_digit.ilog2() as u64 + 1;
    let leading_zeros = bits_per_digit - (top_digit.ilog2() as u64 + 1);
    let k = (((53 + bits_per_digit) / bits_per_digit) + 1) as usize;
    let mut buf: u128 = 0;
    for byte in bytes[..k].iter() {
        buf = (buf << bits_per_digit) | digit_value(*byte)? as u128;
    }
    let buf_bits = k as u64 * bits_per_digit;
    let shift = buf_bits - 53 - leading_zeros;
    let mut mantissa = ((buf >> shift) & ((1u128 << 53) - 1)) as u64;
    let round_bit = (buf >> (shift - 1)) & 1 == 1;
    let mut sticky = (buf & ((1u128 << (shift - 1)) - 1)) != 0;
    for byte in &bytes[k..] {
        if digit_value(*byte)? != 0 {
            sticky = true;
        }
    }
    if round_bit && (sticky || mantissa & 1 == 1) {
        mantissa += 1;
    }
    if mantissa == 1u64 << 53 {
        mantissa = 1 << 52;
        n_bits += 1;
    }
    if n_bits >= 1025 {
        return Some(f64::INFINITY);
    }
    Some((mantissa as f64) * 2f64.powi((n_bits - 53) as i32))
}

/// ECMAScript's exact `TrimString` whitespace set -- what `Number(string)`, `parseInt`, and
/// `parseFloat` all remove, and nothing else.
///
/// The round-1 spelling of this predicate listed the set out by hand and got it wrong in the
/// recall direction (Copilot review, PR #28 round 3): ECMAScript's `WhiteSpace` production is
/// the Unicode **Space_Separator (Zs)** category plus TAB/VT/FF/NBSP/ZWNBSP, unioned with the
/// line terminators -- and `Zs` includes U+1680, U+2000-U+200A, U+202F, U+205F, U+3000, which
/// the hand-written list omitted, so a NEL-free string padded with one of those refused where
/// V8 measures a number for `Number`, `parseInt`, and `parseFloat` alike. The whole truth is
/// simpler: Rust's `char::is_whitespace` is exactly the Unicode `White_Space` property, which
/// agrees with ECMAScript's set on every code point except **U+0085** (NEL, category Cc, which
/// is `NaN` in JavaScript). Measured in V8, both directions. Shared with `parse_numeric_prefix`.
pub(super) fn is_ecmascript_trim_whitespace(char: char) -> bool {
    // U+FEFF (ZWNBSP) is ECMAScript whitespace by name -- the `WhiteSpace` production lists
    // it explicitly -- but it is category Cf, not White_Space, so Rust's `is_whitespace`
    // misses it (Copilot review, PR #28 round 5: the round-3 simplification dropped it along
    // with the hand list). Measured in V8: `Number('\u{FEFF}12')` is `12`.
    char != '\u{0085}' && (char.is_whitespace() || char == '\u{feff}')
}

/// `Number(string)`, for a statically known string.
///
/// ECMAScript trims a fixed whitespace set from both ends, reads `''` as `0`, accepts the
/// `Infinity` spelling and the `0x`/`0b`/`0o` radix prefixes, and is `NaN` for anything else.
/// Rust's `f64::from_str` agrees on the decimal and exponent forms (and accepts both
/// capitalizations of infinity) but silently accepts `NaN`, which JavaScript only reaches from
/// the literal text `NaN` -- and that must refuse, so it is filtered after parsing rather than
/// trusted from the parser.
fn number_of_string(raw: &str) -> Option<f64> {
    let trimmed = raw.trim_matches(is_ecmascript_trim_whitespace);
    if trimmed.is_empty() {
        return Some(0.0);
    }
    // `Number` reads the radix prefixes case-insensitively (Copilot review, PR #28: `'0X1F'`
    // was refused where Vitest renders `31`), rejects any trailing garbage, and takes no
    // sign -- `Number('-0x10')` is NaN, which the converter refuses on this path's behalf.
    // The converter also removes the i128 bound: thirty-two `f`s rendered where they used to
    // refuse (round 5).
    if let Some((radix, digits)) = split_radix_prefix(trimmed) {
        return js_radix_digits_value(digits, radix);
    }
    let value: f64 = trimmed.parse().ok()?;
    if value.is_nan() {
        return None;
    }
    // Rust accepts `inf` and `infinity` in any capitalization; ECMAScript's `Number` accepts
    // only the exact `Infinity` spelling and is `NaN` for every other. Since C12,
    // `format_number` spells non-finite folds `Infinity`, so a folded `1 / 0` arrives here as
    // the value it denotes instead of being refused -- and Rust-only spellings (`inf`, from
    // arithmetic this guard no longer serves on the numeric path) still refuse, which matches
    // JavaScript: `Number('inf')` is `NaN` there too.
    if !value.is_finite() && !trimmed.ends_with("Infinity") {
        return None;
    }
    Some(value)
}

/// Read a tagged table's header and substitutions into rows, refusing unsupported or
/// inconsistent forms.
fn tagged_template_rows(
    quasi: &TemplateLiteral,
    row_limit: usize,
    source: &str,
    resolve: Resolve<'_, '_>,
) -> Result<Vec<Row>, String> {
    let Some(header_text) = quasi
        .quasis
        .first()
        .and_then(|quasi| quasi.value.cooked.as_ref())
        .map(|cooked| cooked.as_str())
    else {
        return Err(tagged_template_identity(quasi, source));
    };
    let columns: Vec<String> = header_text
        .split('|')
        .map(|column| column.trim().to_owned())
        .filter(|column| !column.is_empty())
        .collect();
    if columns.is_empty() {
        return Err(tagged_template_identity(quasi, source));
    }
    let column_count = columns.len();
    if quasi.expressions.is_empty() || !quasi.expressions.len().is_multiple_of(column_count) {
        return Err(tagged_template_identity(quasi, source));
    }
    if quasi.expressions.len() / column_count > row_limit {
        return Err(OVER_LIMIT_TABLE.to_owned());
    }
    let mut rows = Vec::new();
    for chunk in quasi.expressions.chunks(column_count) {
        let mut named: Vec<(String, Option<CellValue>)> = Vec::with_capacity(column_count);
        let mut cell_texts = Vec::with_capacity(column_count);
        let mut unresolved = false;
        for (column, cell_expr) in columns.iter().zip(chunk) {
            cell_texts.push(source_text(source, cell_expr.span()));
            let value = fold_cell(cell_expr, resolve);
            if value.is_none() {
                unresolved = true;
            }
            named.push((column.clone(), value));
        }
        // A tagged-template row binds one object, so `%s` gets one argument: that object's
        // inspect form, assembled from the columns in declaration order. The assembled object
        // is ONE interpolated value, so it runs through the same budget mirror as any other
        // `%o`/`$field` value -- the halving loop re-renders the columns at the halved depth,
        // and the elision search counts COLUMNS.
        let title_args = vec![assembled_object_within_budget(
            columns
                .iter()
                .zip(chunk)
                .map(|(column, expr)| (column.as_str(), expr))
                .collect::<Vec<_>>()
                .as_slice(),
            resolve,
        )
        .map(|rendered| TitleArg {
            // An object has no own `toString`, so `%s` inspects it -- the two agree here.
            coerced: rendered.clone(),
            inspected: rendered.clone(),
            // Reconstructing JSON from already-inspected columns is not something this
            // can do honestly, so `%j` over a tagged-template row keeps the pattern.
            json: None,
            field: rendered,
            // The row is an object to `%d`, and `Number(object)` is `NaN`, which refuses.
            number: None,
        })];
        rows.push(Row {
            display: cell_texts.join(" | "),
            cells: RowCells::Named(named),
            unresolved,
            title_args,
        });
    }
    Ok(rows)
}

/// [`inspect_within_budget`] for the tagged-template row's assembled object: the value is one
/// object whose fields are the template's columns, so the halving loop re-renders the columns
/// at the halved depth and the elision search counts columns. Same algorithm, different
/// rendering source -- the object has no single `Expression` to re-render, only parts.
fn assembled_object_within_budget(
    columns: &[(&str, &Expression)],
    resolve: Resolve<'_, '_>,
) -> Option<String> {
    let render_column = |(name, expr): &(&str, &Expression), limit: Option<u32>| {
        render_inspect(expr, 1, limit, resolve).map(|rendered| format!("{name}: {rendered}"))
    };
    // A fn item rather than a closure: it is used from two closures, and passing a closure by
    // reference to `map` is a lint under current stable clippy.
    /// Join rendered object-property parts into one object display fragment.
    fn assemble(parts: Vec<String>) -> String {
        format!("{{ {} }}", parts.join(", "))
    }
    let render_all = |limit: Option<u32>| {
        columns
            .iter()
            .map(|column| render_column(column, limit))
            .collect::<Option<Vec<_>>>()
            .map(assemble)
    };
    if let Ok(within) = halve_to_budget(render_all(None)?, |max_depth| render_all(Some(max_depth)))?
    {
        return Some(within);
    }
    search_first_n(columns.len(), |keep| {
        let mut parts: Vec<String> = Vec::with_capacity(keep);
        for column in columns.iter().take(keep) {
            parts.push(render_column(column, Some(1))?);
        }
        let omitted = columns.len() - keep;
        if omitted > 0 {
            parts.push(format!("…({omitted})"));
        }
        Some(assemble(parts))
    })
}

/// Evaluates the small subset of expressions whose value is knowable without running the file:
/// literals, and arithmetic on literals (`1 + 1`). Used both for `.each`/`.for` row cells and for
/// const-foldable template-literal titles (`` `title with ${1 + 1}` ``, per
/// `docs/vitest-surface.md` fixture #7).
///
/// Anything else -- an identifier, a function call, string concatenation with a variable --
/// returns `None`. This is deliberately conservative: hazard 6 warns that Vitest's own
/// title-rendering changed across majors, so this extractor never pretends to reproduce runtime
/// formatting beyond values it can prove from source alone.
/// Kind-tagged constant folding, for values that are COMPARED rather than displayed.
///
/// [`const_fold`] returns a literal's raw text because `.each` title rendering interpolates it
/// into a human-readable title, where `<number:1000>` would be nonsense. That raw form is unsafe
/// to compare: a numeric `1000` and a string `'1000'` both fold to `"1000"`, so a fixture bound to
/// one compares equal to a fixture bound to the other, and two tests running under genuinely
/// different fixtures are reported as duplicates at error severity.
///
/// This is the fourth place in this codebase where a value's kind was erased and content collapsed
/// -- after `vi.mock` factories, `test.extend` setup bodies, and `Serializer::literal`. The
/// tagging here mirrors `Serializer::literal`'s `<kind:raw>` form on purpose, so the two agree.
pub(super) fn const_fold_tagged(expr: &Expression) -> Option<String> {
    match unwrap_transparent(expr) {
        Expression::StringLiteral(literal) => Some(format!("<string:{}>", literal.value.as_str())),
        Expression::NumericLiteral(literal) => {
            Some(format!("<number:{}>", format_number(literal.value)))
        }
        Expression::BooleanLiteral(literal) => Some(format!("<boolean:{}>", literal.value)),
        Expression::NullLiteral(_) => Some("<null>".to_owned()),
        Expression::BigIntLiteral(literal) => Some(format!("<bigint:{}>", literal.value.as_str())),
        Expression::TemplateLiteral(template) => {
            template_without_substitutions(template, None).map(|value| format!("<string:{value}>"))
        }
        // Arithmetic and a leading sign carry the numeric tag; `+` over a string operand
        // concatenates and carries the string tag. See `const_fold_kind`.
        inner @ (Expression::BinaryExpression(_) | Expression::UnaryExpression(_)) => {
            let kind = if is_string_valued(inner, None) {
                "string"
            } else {
                "number"
            };
            const_fold(expr, None).map(|value| format!("<{kind}:{value}>"))
        }
        inner @ Expression::ArrayExpression(_) => {
            fold_composite(inner, 0, None).map(|raw| format!("<array:{raw}>"))
        }
        inner @ Expression::ObjectExpression(_) => {
            fold_composite(inner, 0, None).map(|raw| format!("<object:{raw}>"))
        }
        _ => None,
    }
}

/// [`const_fold_tagged`]'s kind and raw text, uncombined, for [`fold_cell`]/row substitution.
///
/// Matches exactly the `(kind, raw)` pair
/// [`crate::frontend::fingerprint::Serializer::literal`] would produce for the same value written
/// directly as a literal in source -- including `null`, where this returns `("null", "null")`
/// rather than `const_fold_tagged`'s bare `<null>` (no raw part), because `Serializer::literal`
/// itself always renders `null` as `self.literal("null", "null")`. Accepts exactly the same
/// expression shapes as [`const_fold`] (both match `StringLiteral`/`NumericLiteral`/
/// `BooleanLiteral`/`NullLiteral`/`BinaryExpression` and nothing else), so the two never disagree
/// about whether a given cell is readable at all -- only about the shape of what they return for
/// one that is.
pub(super) fn const_fold_kind(
    expr: &Expression,
    resolve: Resolve<'_, '_>,
) -> Option<(&'static str, String)> {
    let expr = peeled(expr, resolve);
    if let Some(scalar) = const_fold_kind_scalar(expr, resolve) {
        return Some(scalar);
    }
    // A composite's `raw` is the canonical rendering of its contents, so `alpha` keeps two
    // different objects apart while `structural` -- which drops every raw and keeps only the kind
    // -- collapses them to `<object>`, exactly the "same shape, different values" step that makes
    // a parameterization candidate. Neither kind existed before, so neither can collide with the
    // scalar tags.
    match unwrap_transparent(expr) {
        Expression::ArrayExpression(_) => {
            fold_composite(expr, 0, resolve).map(|raw| ("array", raw))
        }
        Expression::ObjectExpression(_) => {
            fold_composite(expr, 0, resolve).map(|raw| ("object", raw))
        }
        _ => None,
    }
}

/// Read a supported scalar's runtime kind and raw fingerprint value after bounded binding
/// resolution.
fn const_fold_kind_scalar(
    expr: &Expression,
    resolve: Resolve<'_, '_>,
) -> Option<(&'static str, String)> {
    let expr = peeled(expr, resolve);
    match unwrap_transparent(expr) {
        Expression::StringLiteral(literal) => Some(("string", literal.value.as_str().to_owned())),
        Expression::NumericLiteral(literal) => Some(("number", format_number(literal.value))),
        Expression::BooleanLiteral(literal) => Some(("boolean", literal.value.to_string())),
        Expression::NullLiteral(_) => Some(("null", "null".to_owned())),
        // `bigint` is the serializer's own kind for `100n`, deliberately not folded into
        // `number`: they are different runtime types, and sharing a tag would let `100n` and
        // `100` collide the way `true` and `'true'` once did.
        Expression::BigIntLiteral(literal) => Some(("bigint", literal.value.as_str().to_owned())),
        Expression::TemplateLiteral(template) => {
            template_without_substitutions(template, resolve).map(|value| ("string", value))
        }
        // Arithmetic and a leading sign fold to a number -- but `+` over a string operand
        // concatenates, and the result is a STRING. Tagging that `number` would let `'1' + '2'`
        // collide with the number 12, the exact shape of the `true`/`'true'` collision the kind
        // tags exist to prevent.
        inner @ (Expression::BinaryExpression(_) | Expression::UnaryExpression(_)) => {
            let kind = if is_string_valued(inner, resolve) {
                "string"
            } else {
                "number"
            };
            const_fold(expr, resolve).map(|value| (kind, value))
        }
        _ => None,
    }
}

/// Render supported constant expressions as display text, returning None for forms the
/// evaluator cannot establish.
pub(super) fn const_fold(expr: &Expression, resolve: Resolve<'_, '_>) -> Option<String> {
    let expr = peeled(expr, resolve);
    match unwrap_transparent(expr) {
        Expression::StringLiteral(literal) => Some(literal.value.as_str().to_owned()),
        Expression::NumericLiteral(literal) => Some(format_number(literal.value)),
        Expression::BooleanLiteral(literal) => Some(literal.value.to_string()),
        Expression::NullLiteral(_) => Some("null".to_owned()),
        // The `n` suffix belongs in the *title* and not in the fingerprint's raw text: Vitest
        // renders a BigInt row through `util.format`, which prints `100n`, while
        // `Serializer::literal` tags it `<bigint:100>` from the same `value` this reads. Display
        // and raw are computed by separate functions precisely so they can differ here without
        // either drifting from what it is responsible for.
        Expression::BigIntLiteral(literal) => Some(format!("{}n", literal.value.as_str())),
        Expression::TemplateLiteral(template) => template_without_substitutions(template, resolve),
        inner @ Expression::UnaryExpression(_) => {
            fold_signed_number(inner, resolve).map(format_number)
        }
        // Composites resolve, but their `display` is this canonical form rather than a guess at
        // how Vitest would print them -- `render_field` and `render_coerce` own that.
        inner @ (Expression::ArrayExpression(_) | Expression::ObjectExpression(_)) => {
            fold_composite(inner, 0, resolve)
        }
        Expression::BinaryExpression(binary) => {
            // `+` concatenates when either operand is a string: `'a' + 'b'` is `ab`, and
            // `'v' + 1` is `v1`. Checked before the numeric path, because that path would
            // refuse a string operand and lose a cell whose value is perfectly well known.
            if binary.operator == BinaryOperator::Addition
                && (is_string_valued(&binary.left, resolve)
                    || is_string_valued(&binary.right, resolve))
            {
                // ToString, NOT the `%s` renderer: `'x' + { a: 1 }` is `x[object Object]`.
                // Using the Vitest renderer here made it fold to `x{ a: 1 }`, byte-identical to
                // the unrelated literal `'x{ a: 1 }'` -- a `duplicate-test` false positive at
                // error severity.
                let left = render_js_to_string(&binary.left, 1, resolve)?;
                let right = render_js_to_string(&binary.right, 1, resolve)?;
                return Some(format!("{left}{right}"));
            }
            let left = const_fold_number(&binary.left, resolve)?;
            let right = const_fold_number(&binary.right, resolve)?;
            let result = match binary.operator {
                BinaryOperator::Addition => left + right,
                BinaryOperator::Subtraction => left - right,
                BinaryOperator::Multiplication => left * right,
                BinaryOperator::Division => left / right,
                _ => return None,
            };
            Some(format_number(result))
        }
        _ => None,
    }
}

/// How deep a composite cell may nest before this refuses to read it.
///
/// A bound, not a judgement about real tables: the recursion below is driven by repository
/// content, and unbounded recursion over attacker-shaped input is how a parser gets a stack
/// overflow instead of a diagnostic. Refusing past this depth costs a `not-compared` test, which
/// is the same answer this function gave for every composite before it existed.
const MAX_CELL_DEPTH: u32 = 8;

/// Renders one cell to a canonical, injective string, or refuses.
///
/// This is the composite half of [`const_fold_kind`]. Objects and arrays are 62% of the cells
/// this extractor could not read on a real corpus (2,182 and 837 of 4,886), and every one of them
/// excluded its whole row -- and therefore every test in that row -- from comparison.
///
/// Injectivity is the whole requirement, because the result reaches
/// [`crate::frontend::fingerprint::Serializer::literal`] as a `raw` and two different values that
/// render alike would become a `duplicate-test` false positive at error severity. Each scalar
/// carries its kind tag (`<number:1>` is not `<string:1>`), and each raw goes through
/// [`escape_literal_delimiters`] -- the serializer's own function, shared rather than copied, so
/// the two escaping rules are identical by construction and not by inspection -- meaning a string
/// containing `>` cannot close a delimiter it does not own. That forgery was a real defect in this
/// codebase once already.
///
/// Two deliberate refusals to normalize, both costing a missed match rather than risking a false
/// one:
///
/// * **Key order is source order, never sorted.** `{a: 1, b: 2}` and `{b: 2, a: 1}` bind the same
///   names to the same values, so sorting would catch a real duplicate pair -- but `Object.keys`
///   observes insertion order, and a test that iterates its fixture would behave differently
///   across the two. Unequal is the answer that cannot be wrong.
/// * **Anything computed stays unread**: a spread, a computed key, a shorthand, a getter, a
///   method, an array hole. Each is a value this cannot prove from source alone.
fn fold_composite(expr: &Expression, depth: u32, resolve: Resolve<'_, '_>) -> Option<String> {
    if depth > MAX_CELL_DEPTH {
        return None;
    }
    match unwrap_transparent(expr) {
        Expression::ArrayExpression(array) => {
            let mut parts = Vec::with_capacity(array.elements.len());
            for element in &array.elements {
                // A hole is `undefined`, which this has no literal kind for, and a spread is a
                // value only the runtime knows.
                let inner = element.as_expression()?;
                parts.push(fold_cell_canonical(inner, depth + 1, resolve)?);
            }
            Some(format!("[{}]", parts.join(",")))
        }
        Expression::ObjectExpression(object) => {
            // Last value wins under a repeated key, holding its first insertion position -- the
            // same normalization `analyze_row` applies to a row-level object, and for the same
            // reason: `{a: 1, a: 2}` binds `a` to `2` at runtime and nothing downstream should
            // have to know the first one was ever written.
            let mut fields: Vec<(String, String)> = Vec::new();
            let mut index_by_name: HashMap<String, usize> = HashMap::new();
            for property in &object.properties {
                let ObjectPropertyKind::ObjectProperty(property) = property else {
                    // A spread. Its contents are not knowable from this literal.
                    return None;
                };
                if property.computed || property.method || property.shorthand {
                    return None;
                }
                // A non-shorthand `__proto__` entry is the prototype SETTER (same rule as the
                // member-read arm): the object the runtime sees has no such own property, so
                // the canonical form must not invent one.
                if property_key_name(&property.key).as_deref() == Some("__proto__") {
                    return None;
                }
                let name = match &property.key {
                    PropertyKey::StaticIdentifier(identifier) => {
                        identifier.name.as_str().to_owned()
                    }
                    PropertyKey::StringLiteral(literal) => literal.value.as_str().to_owned(),
                    PropertyKey::NumericLiteral(literal) => format_number(literal.value),
                    _ => return None,
                };
                let value = fold_cell_canonical(&property.value, depth + 1, resolve)?;
                match index_by_name.get(&name) {
                    Some(&position) => fields[position].1 = value,
                    None => {
                        index_by_name.insert(name.clone(), fields.len());
                        fields.push((name, value));
                    }
                }
            }
            let rendered = fields
                .iter()
                .map(|(name, value)| format!("{}:{value}", escape_literal_delimiters(name)))
                .collect::<Vec<_>>()
                .join(",");
            Some(format!("{{{rendered}}}"))
        }
        _ => None,
    }
}

/// One cell as `<kind:raw>` or a nested composite, for [`fold_composite`]'s canonical form.
fn fold_cell_canonical(expr: &Expression, depth: u32, resolve: Resolve<'_, '_>) -> Option<String> {
    let expr = peeled(expr, resolve);
    if let Some((kind, raw)) = const_fold_kind_scalar(expr, resolve) {
        return Some(format!("<{kind}:{}>", escape_literal_delimiters(&raw)));
    }
    fold_composite(expr, depth, resolve)
}

/// Renders a cell the way Vitest's `%s` directive would.
///
/// Vitest builds an `.each` title by printf-formatting the row's arguments, so `%s` follows
/// JavaScript's string coercion -- **not** its inspect form. The distinction is real and
/// load-bearing: an array coerces through `Array.prototype.join` (`[1, 2]` prints `1,2`) while the
/// same array under a `$field` reference prints `[ 1, 2 ]`. A plain object has no own `toString`,
/// so `%s` falls back to inspecting it, which is why the two renderings agree there and nowhere
/// else.
///
/// Every rule below was measured against Vitest 5.0.1 rather than inferred, and
/// `each_titles_match_vitest` pins the observations.
/// The shared prologue of the render family: refuse past the depth bound, resolve through any
/// binding, and unwrap the wrappers that carry no value.
fn peeled_unwrapped<'e, 'a: 'e>(
    expr: &'e Expression<'a>,
    resolve: Resolve<'_, 'a>,
    depth: u32,
) -> Option<&'e Expression<'a>> {
    (depth <= MAX_CELL_DEPTH).then(|| unwrap_transparent(peeled(expr, resolve)))
}

/// Render supported top-level values for string-format coercion, preserving inspected objects
/// and BigInt suffixes.
fn render_coerce(expr: &Expression, depth: u32, resolve: Resolve<'_, '_>) -> Option<String> {
    let inner = peeled_unwrapped(expr, resolve, depth)?;
    match inner {
        // A plain object has no own `toString`, so the formatter inspects it instead of
        // coercing: `%s` of `{ a: 1 }` is `{ a: 1 }`, where ToString would give
        // `[object Object]`. This is the whole difference between the two renderings at the top
        // level, and it is why they cannot share one function. The inspect is the budgeted one:
        // Vitest's `%s` arm calls the same `inspect(value, { truncate })` as `%o`, measured on
        // 5.0.3 (`docs/vitest-surface.md` §4, the `%s`-of-object row). Only the top level can
        // reach this arm (composites never fall through from `render_inspect`).
        Expression::ObjectExpression(_) => inspect_within_budget(expr, resolve),
        // `%s` of `10n` keeps the suffix; ToString does not. Measured both ways: the row `10n`
        // titles as `10n`, while the row `[10n]` titles as `10`, because the array goes through
        // `join` and `join` uses ToString.
        Expression::BigIntLiteral(literal) => Some(format!("{}n", literal.value.as_str())),
        _ => {
            // `%s` of `-0` prints the sign; `String(-0)` is `"0"`. Same split as BigInt: inside
            // an array the join erases it, and `[-0]` really does title as `0`.
            if let Some(value) = numeric_value(inner, resolve) {
                if value == 0.0 && value.is_sign_negative() {
                    return Some("-0".to_owned());
                }
            }
            render_js_to_string(inner, depth, resolve)
        }
    }
}

/// The numeric value of a cell, when it has one. `None` for everything else.
///
/// Resolution is threaded (Copilot CLI review of PR #28): hardcoding `None` here while the
/// caller `render_coerce` holds a real resolve made `const N = 0; [[-N]]` miss the `-0`
/// special case below -- the unary operand refused to resolve, control fell through to
/// `render_js_to_string`, and ECMAScript's `ToString(-0)` printed `0` where Vitest's `%s`
/// keeps the sign. A wrong accepted value, not a refusal.
fn numeric_value(expr: &Expression, resolve: Resolve<'_, '_>) -> Option<f64> {
    match unwrap_transparent(expr) {
        Expression::NumericLiteral(literal) => Some(literal.value),
        inner @ Expression::UnaryExpression(_) => fold_signed_number(inner, resolve),
        _ => None,
    }
}

/// ECMAScript `ToString`, which is what `+` and a template substitution use.
///
/// **Not** the same as [`render_coerce`], and conflating them produced a `duplicate-test` false
/// positive at error severity: `'x' + { a: 1 }` was folded with the `%s` renderer to
/// `x{ a: 1 }`, which made it byte-identical to the unrelated literal `'x{ a: 1 }'`. The real
/// value is `x[object Object]`. Three rules differ, all verified against Node:
///
/// | value | `ToString` (here) | Vitest `%s` |
/// | --- | --- | --- |
/// | `{ a: 1 }` | `[object Object]` | `{ a: 1 }` |
/// | `10n` | `10` | `10n` |
/// | `-0` | `0` | `-0` |
///
/// Arrays agree, because `%s` on an array *is* `ToString` -- `Array.prototype.join` calling
/// `ToString` on each element is the only path either renderer takes.
fn render_js_to_string(expr: &Expression, depth: u32, resolve: Resolve<'_, '_>) -> Option<String> {
    let expr = peeled_unwrapped(expr, resolve, depth)?;
    match expr {
        // `Object.prototype.toString` -- but ONLY when that is really the method being called.
        Expression::ObjectExpression(_) => {
            inherits_default_string_coercion(expr).then(|| "[object Object]".to_owned())
        }
        Expression::ArrayExpression(_) => {
            let elements = array_elements(expr)?;
            let mut parts = Vec::with_capacity(elements.len());
            for element in elements {
                parts.push(render_js_join_element(element, depth + 1, resolve)?);
            }
            Some(parts.join(","))
        }
        Expression::StringLiteral(literal) => Some(literal.value.as_str().to_owned()),
        Expression::TemplateLiteral(template) => fold_template(template, depth + 1, resolve),
        Expression::BooleanLiteral(literal) => Some(literal.value.to_string()),
        Expression::NullLiteral(_) => Some("null".to_owned()),
        // `String(10n)` is `"10"`: a BigInt drops the literal's `n`, which is syntax and not
        // part of the value.
        Expression::BigIntLiteral(literal) => Some(literal.value.as_str().to_owned()),
        Expression::NumericLiteral(literal) => Some(js_number_to_string(literal.value)),
        inner @ Expression::UnaryExpression(_) => {
            fold_signed_number(inner, resolve).map(js_number_to_string)
        }
        inner @ Expression::BinaryExpression(_) => {
            let folded = const_fold(expr, resolve)?;
            if is_string_valued(inner, resolve) {
                return Some(folded);
            }
            folded.parse::<f64>().ok().map(js_number_to_string)
        }
        _ => None,
    }
}

/// Whether an object literal provably stringifies through `Object.prototype.toString`.
///
/// `[object Object]` is what a *plain* object coerces to, and an object literal is only plain if
/// nothing in it can intercept the coercion. `'x' + { toString: () => 'y' }` is `xy`, not
/// `x[object Object]`, and folding it to the latter collided with the unrelated literal
/// `'x[object Object]'` -- a second `duplicate-test` false positive at error severity, from the
/// same review, in the branch that fixed the first one.
///
/// Refused, each for a reason JavaScript can demonstrate:
///
/// * `toString` and `valueOf` -- either is consulted by `ToPrimitive` before the default, and
///   `{ valueOf: () => 5 }` concatenates as `x5`.
/// * `__proto__` -- rewrites the prototype, so the inherited `toString` is no longer
///   `Object.prototype`'s: `'x' + { __proto__: { toString: () => 'z' } }` is `xz`.
/// * a computed key -- the name is not knowable, so it could be any of the above, including
///   `[Symbol.toPrimitive]`, which outranks both.
/// * a spread -- brings in properties this cannot see at all.
///
/// Everything else is safe regardless of its value: a getter or a method on an ordinary key does
/// not participate in coercion, and `'x' + { get a() { return 1 } }` really is `x[object Object]`.
/// That is checked, not assumed -- see the folding contract.
fn inherits_default_string_coercion(expr: &Expression) -> bool {
    let Expression::ObjectExpression(object) = unwrap_transparent(expr) else {
        return false;
    };
    object.properties.iter().all(|property| {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            return false;
        };
        if property.computed {
            return false;
        }
        let name = match &property.key {
            PropertyKey::StaticIdentifier(identifier) => identifier.name.as_str(),
            PropertyKey::StringLiteral(literal) => literal.value.as_str(),
            // A numeric key cannot spell any of the names below.
            PropertyKey::NumericLiteral(_) => return true,
            _ => return false,
        };
        !matches!(name, "toString" | "valueOf" | "__proto__")
    })
}

/// One element inside `Array.prototype.join`.
///
/// Identical to [`render_js_to_string`] except that `null` and `undefined` become empty strings
/// -- `[null]` really does stringify to nothing, and `[null, undefined]` to a bare `,`. That is
/// a rule of `join`, not of `ToString`.
fn render_js_join_element(
    expr: &Expression,
    depth: u32,
    resolve: Resolve<'_, '_>,
) -> Option<String> {
    match unwrap_transparent(expr) {
        Expression::NullLiteral(_) => Some(String::new()),
        other => render_js_to_string(other, depth, resolve),
    }
}

/// JavaScript's `Number::toString`, including its shortest-decimal and exponent rules.
///
/// Rust's `f64::to_string` is not interchangeable here. For the same binary float it can choose
/// the other equally short decimal: Rust prints `201562347225087.63`, while JavaScript prints
/// `201562347225087.62`. Using the Rust spelling made `'x' + 201562347225087.63` collide with the
/// unrelated literal `'x201562347225087.63'` and produced a `duplicate-test` false positive at
/// error severity. `ryu-js` implements the ECMAScript spelling directly, including `-0`,
/// exponent notation, infinities, and NaN.
/// JavaScript's `Number::toString`, including its shortest-decimal and exponent rules.
///
/// Rust's `f64::to_string` is not interchangeable here. For the same binary float it can choose
/// the other equally short decimal: Rust prints `201562347225087.63`, while JavaScript prints
/// `201562347225087.62`. Using the Rust spelling made `'x' + 201562347225087.63` collide with
/// the unrelated literal `'x201562347225087.63'` and produced a `duplicate-test` false positive
/// at error severity. `ryu-js` implements the ECMAScript spelling directly, including `-0`,
/// exponent notation, infinities, and NaN.
pub(super) fn js_number_to_string(value: f64) -> String {
    let mut buffer = ryu_js::Buffer::new();
    buffer.format(value).to_owned()
}

/// Renders a cell the way Vitest prints a `$field` reference, within the title budget.
///
/// A bare string at the top level (`$name` of `'x'` is `x`, not `'x'`), and the inspect form for
/// everything else -- including a nested string, which *is* quoted. Every string is bare here,
/// however it was written -- a literal, a template, or a concatenation. Asking `is_string_valued`
/// rather than matching two syntactic shapes is what keeps this from drifting as more expressions
/// become foldable to strings.
///
/// The two branches are Vitest's `formatAttribute` (5.0.3): a string goes through
/// `truncateString(value, 40)` -- unchanged at 40 units, cut to 39 plus `…` above -- and
/// anything else through the same `inspect` call `%o` uses. `inspected` supplies that
/// rendering so a caller that already has it ([`title_arg`]) does not run the budget twice.
fn field_within_budget(
    expr: &Expression,
    inspected: impl FnOnce() -> Option<String>,
    resolve: Resolve<'_, '_>,
) -> Option<String> {
    let inner = unwrap_transparent(peeled(expr, resolve));
    if is_string_valued(inner, resolve) {
        return const_fold(inner, resolve).map(|value| truncate_bare_string(&value));
    }
    inspected()
}

/// `truncateString(value, 40)`: a bare `$field` string at or under the budget is unchanged;
/// over it, the first 39 code units then `…`. Measured (Vitest 5.0.3): `'a'×40` unchanged,
/// `'a'×41` and `'a'×45` both `'a'×39` then `…`. Distinct from the quoted `%o` cut, where the
/// quotes count against the budget and the kept prefix is 38.
fn truncate_bare_string(value: &str) -> String {
    if title_utf16_len(value) <= TITLE_VALUE_BUDGET {
        value.to_owned()
    } else {
        cut_utf16(value, TITLE_VALUE_BUDGET - 1)
    }
}

/// The step-3 string cut under `%o`: reached only when the quoted rendering is over the budget,
/// so the content is over 38 units, and Vitest keeps `formatted.slice(1, 39)` -- 38 units of
/// content -- then `…` inside the quotes, 41 units in all; the algorithm does not re-verify
/// after the cut. Measured: `'a'×38` is returned whole (40 units quoted), `'a'×39` is already
/// cut to 38.
fn truncate_quoted_string(value: &str) -> String {
    format!("'{}'", cut_utf16(value, TITLE_VALUE_BUDGET - 2))
}

/// The first `keep` UTF-16 code units of `value`, then `…`. A character that would straddle the
/// cut is dropped whole -- Vitest's `isHighSurrogate(string[end - 1])` back-off, which is why
/// `'😀'×21` under `$field` keeps 19 (38 units), not 19 and a half.
fn cut_utf16(value: &str, keep: usize) -> String {
    let mut units = 0;
    let mut cut: String = value
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= keep
        })
        .collect();
    cut.push('…');
    cut
}

/// The budget is measured in UTF-16 code units -- JavaScript's `string.length`, which is what
/// every comparison in Vitest's `inspect`/`truncateString` reads -- so an astral character
/// (`😀`) counts twice. Measured: `'😀'×20` under `$field` (40 units) is unchanged; `'😀'×21`
/// is cut. A `chars().count()` here read the same two titles as 20 and 21.
fn title_utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// The per-value title budget: Vitest 5's `taskTitleValueFormatTruncate` default, measured and
/// recorded in `docs/vitest-surface.md` §4. One budget per interpolated value, not per title.
const TITLE_VALUE_BUDGET: usize = 40;

/// Renders an inspected value within Vitest's 40-character title budget, mirroring the
/// algorithm §4 records for `taskTitleValueFormatTruncate`:
///
/// 1. the full rendering (our [`MAX_CELL_DEPTH`] refusal applies -- a value nested past it is
///    *refused*, not partially rendered; see §4's "a refusal, not a rendering difference");
/// 2. while the rendering is at or over the budget, re-render with `maxDepth` halved
///    (5 → 2 → 1), composites at the limit printing `[Array]`/`[Object]`;
/// 3. still over: a string value is cut ([`truncate_quoted_string`]); an array or object
///    re-renders at `maxDepth: 1` showing the largest item count that fits, the cut marked
///    `…(k)`; anything else renders at `maxDepth: 0` (for a scalar, the same text).
///
/// Every length is in UTF-16 code units ([`title_utf16_len`]), as Vitest's are.
///
/// The mirror is what makes a rendered `.each` title a claim about the runtime string: two
/// distinct values that Vitest itself titles identically (the same `…(k)` elision) now render
/// alike HERE too, which is the point -- and is safe for precision because the body and context
/// fingerprints still distinguish the tests; only the title text aligns with runtime. The
/// item-count search reproduces §4's measured rows byte-for-byte (`…(10)`, `…(3)`, `…(1)` --
/// including width 0, where even one item cannot fit).
fn inspect_within_budget(expr: &Expression, resolve: Resolve<'_, '_>) -> Option<String> {
    let full = render_inspect(expr, 0, None, resolve)?;
    let over = match halve_to_budget(full, |max_depth| {
        render_inspect(expr, 0, Some(max_depth), resolve)
    })? {
        Ok(within) => return Some(within),
        Err(over) => over,
    };
    let top = unwrap_transparent(peeled(expr, resolve));
    match top {
        Expression::StringLiteral(_) | Expression::TemplateLiteral(_) => {
            const_fold(top, resolve).map(|value| truncate_quoted_string(&value))
        }
        inner @ Expression::BinaryExpression(_) if is_string_valued(inner, resolve) => {
            const_fold(top, resolve).map(|value| truncate_quoted_string(&value))
        }
        Expression::ArrayExpression(_) => {
            let total = array_elements(top)?.len();
            search_first_n(total, |keep| render_array_items(top, keep, total, resolve))
        }
        Expression::ObjectExpression(_) => {
            let total = object_fields(top)?.len();
            search_first_n(total, |keep| render_object_items(top, keep, total, resolve))
        }
        // A scalar over the budget renders at `maxDepth: 0`, which for a scalar is the same
        // text as every other depth -- numbers cannot reach 40 units, and a BigInt past the
        // budget stays long, exactly as pretty-format's `maxDepth: 0` re-render leaves it.
        _ => Some(over),
    }
}

/// Steps 1 and 2 of the budget algorithm, shared by [`inspect_within_budget`] and
/// [`assembled_object_within_budget`] so a threshold lives in one place: `full` is returned
/// when under the budget; otherwise `render_at` is asked for `maxDepth` 5, 2 and 1 in turn,
/// the first rendering under the budget wins, and the `maxDepth: 1` rendering is accepted at
/// exactly the budget. `Err` carries that last rendering when it is still over, for the
/// caller's step 3; `None` means a limited render refused.
///
/// The thresholds differ on purpose: Vitest halves while `length >= 40` and then accepts
/// `length <= 40`, so a rendering of exactly 40 units is halved but returned untouched if the
/// halving does not shorten it.
fn halve_to_budget(
    full: String,
    render_at: impl Fn(u32) -> Option<String>,
) -> Option<Result<String, String>> {
    if title_utf16_len(&full) < TITLE_VALUE_BUDGET {
        return Some(Ok(full));
    }
    let mut last = full;
    for max_depth in [5u32, 2, 1] {
        last = render_at(max_depth)?;
        if title_utf16_len(&last) < TITLE_VALUE_BUDGET {
            return Some(Ok(last));
        }
    }
    Some(if title_utf16_len(&last) <= TITLE_VALUE_BUDGET {
        Ok(last)
    } else {
        Err(last)
    })
}

/// Step 3's elision search: the largest item count whose rendering fits the budget, the cut
/// marked `…(k)`. A count search, not a per-item width cut -- §4's `{ <46-char key>: 1 }` row
/// renders `{ …(1) }`, the whole item omitted, which is what "the search can find width 0"
/// means here.
fn search_first_n(
    total: usize,
    render_first_n: impl Fn(usize) -> Option<String>,
) -> Option<String> {
    // Invariant: `keep == 0` always fits (`[ …(total) ]` is short), so the search terminates
    // with a fit. Binary-searched because item text is unbounded (a long-string item), and a
    // linear scan would render every prefix of a hostile 10,000-item array. The caller reaches
    // this only after the `maxDepth: 1` rendering -- which IS `keep == total` -- exceeded the
    // budget, so `total` itself is never a candidate.
    let fitting = |keep: usize| {
        render_first_n(keep).filter(|text| title_utf16_len(text) <= TITLE_VALUE_BUDGET)
    };
    let mut best = fitting(0)?;
    let mut low = 0usize;
    let mut high = total.saturating_sub(1);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        match fitting(mid) {
            Some(text) => {
                best = text;
                low = mid;
            }
            None => high = mid - 1,
        }
    }
    Some(best)
}

/// The array half of the elision rendering: the first `keep` items at `maxDepth: 1`, then
/// `…(k)` for the omitted tail.
fn render_array_items(
    expr: &Expression,
    keep: usize,
    total: usize,
    resolve: Resolve<'_, '_>,
) -> Option<String> {
    let elements = array_elements(expr)?;
    let mut parts: Vec<String> = elements
        .iter()
        .take(keep)
        .map(|element| render_inspect(element, 1, Some(1), resolve))
        .collect::<Option<Vec<_>>>()?;
    let omitted = total - keep;
    if omitted > 0 {
        parts.push(format!("…({omitted})"));
    }
    Some(format!("[ {} ]", parts.join(", ")))
}

/// The object half: `key: value` items, the same bare-key rule as the full renderer.
fn render_object_items(
    expr: &Expression,
    keep: usize,
    total: usize,
    resolve: Resolve<'_, '_>,
) -> Option<String> {
    let fields = object_fields(expr)?;
    let mut parts: Vec<String> = fields
        .iter()
        .take(keep)
        .map(|(name, value)| {
            render_inspect(value, 1, Some(1), resolve)
                .map(|rendered| format!("{}: {rendered}", inspect_key(name)))
        })
        .collect::<Option<Vec<_>>>()?;
    let omitted = total - keep;
    if omitted > 0 {
        parts.push(format!("…({omitted})"));
    }
    Some(format!("{{ {} }}", parts.join(", ")))
}

/// Vitest's inspect form: `{ a: 1 }`, `[ 1, 2 ]`, `{}`, `[]`.
///
/// Deliberately NOT Node's `util.inspect`, which this was checked against and differs from.
/// Measured on Vitest 5.0.1: strings are always single-quoted and **not** escaped (`{ a: "it's" }`
/// prints `{ a: 'it's' }`, where Node would switch to double quotes), and a key is left bare only
/// when it matches `[A-Za-z_][A-Za-z0-9_]*` -- `$x` is quoted, `_y` is not.
///
/// `limit` is the pretty-format `maxDepth` of a budget render ([`inspect_within_budget`] drives
/// the halving loop): a composite whose nesting depth has reached the limit prints the
/// `[Array]`/`[Object]` marker instead of recursing, exactly as pretty-format prints a composite
/// past `maxDepth`. `None` renders unbounded (still refused past [`MAX_CELL_DEPTH`]). Strings are
/// leaves and render at any depth; only composites check the limit.
fn render_inspect(
    expr: &Expression,
    depth: u32,
    limit: Option<u32>,
    resolve: Resolve<'_, '_>,
) -> Option<String> {
    let expr = peeled_unwrapped(expr, resolve, depth)?;
    match expr {
        Expression::StringLiteral(literal) => Some(format!("'{}'", literal.value.as_str())),
        Expression::TemplateLiteral(template) => {
            template_without_substitutions(template, resolve).map(|value| format!("'{value}'"))
        }
        inner @ Expression::BinaryExpression(_) if is_string_valued(inner, resolve) => {
            const_fold(expr, resolve).map(|value| format!("'{value}'"))
        }
        Expression::ArrayExpression(_) => {
            if limit.is_some_and(|max_depth| depth >= max_depth) {
                return Some("[Array]".to_owned());
            }
            let elements = array_elements(expr)?;
            if elements.is_empty() {
                return Some("[]".to_owned());
            }
            let mut parts = Vec::with_capacity(elements.len());
            for element in elements {
                parts.push(render_inspect(element, depth + 1, limit, resolve)?);
            }
            Some(format!("[ {} ]", parts.join(", ")))
        }
        Expression::ObjectExpression(_) => {
            if limit.is_some_and(|max_depth| depth >= max_depth) {
                return Some("[Object]".to_owned());
            }
            let fields = object_fields(expr)?;
            if fields.is_empty() {
                return Some("{}".to_owned());
            }
            let mut parts = Vec::with_capacity(fields.len());
            for (name, value) in fields {
                parts.push(format!(
                    "{}: {}",
                    inspect_key(&name),
                    render_inspect(value, depth + 1, limit, resolve)?
                ));
            }
            Some(format!("{{ {} }}", parts.join(", ")))
        }
        // Every non-composite prints the same inside an inspect as it does under `%s` -- the
        // BigInt suffix and the signed zero included -- so this defers rather than restating it.
        // Composites are handled above, so this can never recurse back into an object.
        other => render_coerce(other, depth, resolve),
    }
}

/// An object key as the inspect form prints it: bare when it matches
/// `[A-Za-z_][A-Za-z0-9_]*`, single-quoted otherwise. Shared by the full renderer and the
/// budget's item search so the two cannot drift.
fn inspect_key(name: &str) -> String {
    if is_bare_inspect_key(name) {
        name.to_owned()
    } else {
        format!("'{name}'")
    }
}

/// Whether an object key prints without quotes. `$` is NOT part of the bare set -- measured.
fn is_bare_inspect_key(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `JSON.stringify` of a cell, for the `%j` directive.
///
/// `None` where `JSON.stringify` has no string to give: a BigInt throws outright, and this
/// refuses rather than inventing a representation. Compact separators, matching the real thing
/// (`{"a":1}`, not `{ "a": 1 }`) -- measured: `%j` on `'a'` prints `"a"` while `%o` on the same
/// value prints `'a'`, which is exactly why the two cannot share one rendering.
fn render_json(expr: &Expression, depth: u32, resolve: Resolve<'_, '_>) -> Option<String> {
    let expr = peeled_unwrapped(expr, resolve, depth)?;
    match expr {
        Expression::StringLiteral(literal) => Some(json_string(literal.value.as_str())),
        Expression::TemplateLiteral(template) => {
            template_without_substitutions(template, resolve).map(|value| json_string(&value))
        }
        Expression::BooleanLiteral(literal) => Some(literal.value.to_string()),
        Expression::NullLiteral(_) => Some("null".to_owned()),
        // `JSON.stringify(1n)` throws a TypeError; there is no answer to give.
        Expression::BigIntLiteral(_) => None,
        Expression::NumericLiteral(literal) => Some(render_json_number(literal.value)),
        inner @ Expression::UnaryExpression(_) => {
            fold_signed_number(inner, resolve).map(render_json_number)
        }
        inner @ Expression::BinaryExpression(_) => {
            let folded = const_fold(expr, resolve)?;
            Some(if is_string_valued(inner, resolve) {
                json_string(&folded)
            } else {
                render_json_number(folded.parse().ok()?)
            })
        }
        Expression::ArrayExpression(_) => {
            let elements = array_elements(expr)?;
            let mut parts = Vec::with_capacity(elements.len());
            for element in elements {
                // `JSON.stringify([undefined])` is `[null]`; anything this cannot render at all
                // refuses the whole value rather than guessing which case it is.
                parts.push(render_json(element, depth + 1, resolve)?);
            }
            Some(format!("[{}]", parts.join(",")))
        }
        Expression::ObjectExpression(_) => {
            let fields = object_fields(expr)?;
            let mut parts = Vec::with_capacity(fields.len());
            for (name, value) in fields {
                parts.push(format!(
                    "{}:{}",
                    json_string(&name),
                    render_json(value, depth + 1, resolve)?
                ));
            }
            Some(format!("{{{}}}", parts.join(",")))
        }
        _ => None,
    }
}

/// `JSON.stringify`'s spelling for a JavaScript number.
///
/// JSON turns non-finite values into `null`, erases the sign of zero, and otherwise uses the
/// same shortest-decimal representation as `Number::toString`.
fn render_json_number(value: f64) -> String {
    if !value.is_finite() {
        "null".to_owned()
    } else if value == 0.0 {
        "0".to_owned()
    } else {
        js_number_to_string(value)
    }
}

/// A JSON string literal, with the escapes `JSON.stringify` emits.
fn json_string(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The elements of an array literal, refusing a spread or a hole (neither has a knowable value).
fn array_elements<'e, 'a>(expr: &'e Expression<'a>) -> Option<Vec<&'e Expression<'a>>> {
    let Expression::ArrayExpression(array) = unwrap_transparent(expr) else {
        return None;
    };
    array
        .elements
        .iter()
        .map(|element| element.as_expression())
        .collect()
}

/// An object literal's fields in source order, last value winning under a repeated key.
fn object_fields<'e, 'a>(expr: &'e Expression<'a>) -> Option<Vec<(String, &'e Expression<'a>)>> {
    let Expression::ObjectExpression(object) = unwrap_transparent(expr) else {
        return None;
    };
    let mut fields: Vec<(String, &Expression)> = Vec::new();
    let mut index_by_name: HashMap<String, usize> = HashMap::new();
    for property in &object.properties {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            return None;
        };
        if property.computed || property.method || property.shorthand {
            return None;
        }
        // Same __proto__ rule as the member-read arm (see there): the runtime object has no
        // such own property, so neither the inspect nor the JSON form may invent one.
        if property_key_name(&property.key).as_deref() == Some("__proto__") {
            return None;
        }
        let name = match &property.key {
            PropertyKey::StaticIdentifier(identifier) => identifier.name.as_str().to_owned(),
            PropertyKey::StringLiteral(literal) => literal.value.as_str().to_owned(),
            PropertyKey::NumericLiteral(literal) => format_number(literal.value),
            _ => return None,
        };
        match index_by_name.get(&name) {
            Some(&position) => fields[position].1 = &property.value,
            None => {
                index_by_name.insert(name.clone(), fields.len());
                fields.push((name, &property.value));
            }
        }
    }
    Some(fields)
}

/// Whether an expression is a statically known *string*, which makes `+` concatenate.
fn is_string_valued(expr: &Expression, resolve: Resolve<'_, '_>) -> bool {
    match unwrap_transparent(peeled(expr, resolve)) {
        Expression::StringLiteral(_) => true,
        Expression::TemplateLiteral(template) => fold_template(template, 0, resolve).is_some(),
        Expression::BinaryExpression(binary) => {
            binary.operator == BinaryOperator::Addition
                && (is_string_valued(&binary.left, resolve)
                    || is_string_valued(&binary.right, resolve))
        }
        _ => false,
    }
}

/// Read a numeric literal or supported folded numeric expression as f64.
fn const_fold_number(expr: &Expression, resolve: Resolve<'_, '_>) -> Option<f64> {
    match unwrap_transparent(peeled(expr, resolve)) {
        Expression::NumericLiteral(literal) => Some(literal.value),
        Expression::BinaryExpression(_) => const_fold(expr, resolve)?.parse().ok(),
        inner @ Expression::UnaryExpression(_) => fold_signed_number(inner, resolve),
        _ => None,
    }
}

/// Format numbers for static display, spelling NaN and positive/negative Infinity explicitly.
fn format_number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-Infinity" } else { "Infinity" }.to_owned();
    }
    // Non-finite folds take the ECMAScript spelling (C12, corrected on PR #30 round 3): a first
    // "disproof" checked only the `.each` display paths -- which reparse the fold through
    // ryu-js -- and missed that DIRECT titles forward this text verbatim: `test(1 / 0)` and
    // `` test(`n ${1 / 0}`) `` render through `extract_title` -> `fold_expr` -> `const_fold`
    // and titled as `inf` where JavaScript's `String(1/0)` is `Infinity`. Measured against the
    // binary before this restoration. The spelling is also what `number_of_string` expects:
    // `Number('Infinity')` is the value it denotes, where `Number('inf')` is `NaN` -- so folds
    // now round-trip to the value they came from instead of refusing.
    if value.fract() == 0.0 && value.abs() < 1e15 {
        // `-0.0 as i64` is `0`, so the sign is lost and `-0` becomes indistinguishable from `0`.
        // That distinction is observable in a test via `Object.is(value, -0)`, so two fixtures
        // bound to `0` and `-0` would otherwise produce an equivalent context and let genuinely
        // different tests group as duplicates.
        if value == 0.0 && value.is_sign_negative() {
            return "-0".to_owned();
        }
        (value as i64).to_string()
    } else {
        value.to_string()
    }
}

/// Borrow the innermost expression after removing parenthesized wrappers.
fn unwrap_parens<'e, 'a>(expr: &'e Expression<'a>) -> &'e Expression<'a> {
    let mut current = expr;
    while let Expression::ParenthesizedExpression(inner) = current {
        current = &inner.expression;
    }
    current
}

/// Strips the wrappers that carry no runtime value: parentheses, and the TypeScript casts
/// (`as T`, `as const`, `satisfies T`, `<T>x`, `x!`).
///
/// [`crate::frontend::fingerprint::Serializer::ser_expr`] already descends straight through every
/// one of these -- `TSAsExpression` serializes as its inner expression and nothing else -- so a
/// cell written `1 as const` and a cell written `1` are the *same* value to every comparison this
/// tool makes. Refusing to read the first was not caution; it excluded the test from comparison
/// altogether over a type annotation that the fingerprint erases anyway. Measured on a real
/// 24,147-test corpus: 207 unreadable cells, the largest non-composite cause.
fn unwrap_transparent<'e, 'a>(expr: &'e Expression<'a>) -> &'e Expression<'a> {
    let mut current = expr;
    loop {
        current = match current {
            Expression::ParenthesizedExpression(inner) => &inner.expression,
            Expression::TSAsExpression(inner) => &inner.expression,
            Expression::TSSatisfiesExpression(inner) => &inner.expression,
            Expression::TSNonNullExpression(inner) => &inner.expression,
            Expression::TSTypeAssertion(inner) => &inner.expression,
            other => return other,
        };
    }
}

/// A template literal with no `${}` in it is exactly the string it spells.
///
/// `ser_template` keeps templates distinct from string literals in a *body*, because a template
/// with substitutions is a computation. One with none is not: `` `foo` `` and `'foo'` are the
/// same value, and reading it as a string is what lets a row carrying one participate at all.
fn template_without_substitutions(
    template: &TemplateLiteral,
    resolve: Resolve<'_, '_>,
) -> Option<String> {
    fold_template(template, 0, resolve)
}

/// A template literal whose every substitution is itself statically known.
///
/// `` `a` `` is the string `a`, and so is `` `${'a'}` ``; `` `${1}-${2}` `` is `1-2`. A
/// substitution is interpolated with JavaScript's string coercion -- the same rule `%s` follows,
/// so `` `${[1, 2]}` `` is `1,2` -- which is what [`render_coerce`] already implements. A
/// substitution referencing anything runtime refuses the whole template, so this never guesses.
fn fold_template(
    template: &TemplateLiteral,
    depth: u32,
    resolve: Resolve<'_, '_>,
) -> Option<String> {
    if depth > MAX_CELL_DEPTH {
        return None;
    }
    // A well-formed template always has exactly one more quasi than it has expressions.
    if template.quasis.len() != template.expressions.len() + 1 {
        return None;
    }
    let mut out = String::new();
    for (index, quasi) in template.quasis.iter().enumerate() {
        // `cooked` is `None` only for an invalid escape sequence, where there is no string value
        // to speak of -- exactly the case to refuse rather than guess at.
        out.push_str(quasi.value.cooked.as_ref()?.as_str());
        if let Some(expression) = template.expressions.get(index) {
            // A template substitution is ToString too, so `` `${1n}` `` is `1` and not `1n`.
            out.push_str(&render_js_to_string(expression, depth + 1, resolve)?);
        }
    }
    Some(out)
}

/// Folds `-1`/`+1` to the number they denote.
///
/// Unlike the casts above this is *not* what the serializer does with a directly written `-1`
/// (it emits `(unary - <number:1>)`, since a unary operator on an arbitrary expression is a
/// computation). The mismatch is deliberate and one-directional: a row cell is a *value* Vitest
/// has already evaluated by the time the test body runs, so `-1` in a table is the number -1, and
/// reading it as one makes rows comparable to each other. It can only cost a match against a
/// body that writes `-1` inline -- a false negative, never a false positive.
fn fold_signed_number(expr: &Expression, resolve: Resolve<'_, '_>) -> Option<f64> {
    let Expression::UnaryExpression(unary) = expr else {
        return None;
    };
    let inner = const_fold_number(&unary.argument, resolve)?;
    match unary.operator {
        UnaryOperator::UnaryNegation => Some(-inner),
        UnaryOperator::UnaryPlus => Some(inner),
        _ => None,
    }
}

/// Return a trimmed source-span slice, falling back to empty text for an invalid range.
pub(super) fn source_text(source: &str, span: oxc_span::Span) -> String {
    source
        .get(span.start as usize..span.end as usize)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {

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
            let titles =
                extracted_titles("const N = mk();\ntest.each([[N], [2]])('t %s', () => {});");
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
            &(format!(
                "{decl}LEVELS.wrap.mutate();\ntest.each([[LEVELS.high]])('n %d', () => {{}});"
            ))
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
            &(format!(
                "{decl}new (LEVELS.make)();\ntest.each([[LEVELS.high]])('n %d', () => {{}});"
            ))
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

    // Namespace imports: `vitest.test(...)` resolves the same as a named import.
    #[test]
    fn namespace_import_resolves_member_access() {
        let resolved = resolve_first_call(
            r#"import * as vitest from "vitest"; vitest.describe("x", () => {});"#,
            Globals::Disabled,
        )
        .expect("vitest.describe(...) should resolve");
        assert_eq!(resolved.root, "describe");
    }

    // HAZARD 3: globals are opt-in per call site, matching the caller's resolved per-project flag.
    #[test]
    fn bare_identifier_only_resolves_when_globals_enabled() {
        let source = r#"describe("x", () => { test("y", () => {}); });"#;
        assert!(resolve_first_call(source, Globals::Disabled).is_none());
        let resolved = resolve_first_call(source, Globals::Enabled).expect("globals enabled");
        assert_eq!(resolved.root, "describe");
    }

    // HAZARD 4: an import from a competing framework must never resolve, regardless of globals.
    #[test]
    fn import_from_other_framework_does_not_resolve() {
        let source =
            r#"import { test } from "@playwright/test"; test("x", async ({ page }) => {});"#;
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

    // `test.each(table)` is a call whose callee is itself a call: the `.each` member must resolve
    // off the *inner* call, and the table rows must be enumerable.
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

    // HAZARD 5: a non-literal `.each` table must degrade to "unresolved", not zero rows.
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

    // Two different unreadable `.each` tables must be told apart by what is knowable about each
    // expression, not collapsed to the same "unresolved" marker.
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

    // The same unreadable table, resolved twice, must report the same identity both times --
    // otherwise a rule that groups on it could never match a table against itself.
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

    #[test]
    fn canonicalizes_aliases() {
        assert_eq!(canonicalize("it"), Some("test"));
        assert_eq!(canonicalize("suite"), Some("describe"));
        assert_eq!(canonicalize("bogus"), None);
    }

    // HAZARD 6 / fixture #7: a const-foldable template substitution resolves statically, but this
    // extractor never claims to reproduce Vitest's own (version-dependent) runtime rendering
    // beyond values it can prove from source.
    #[test]
    fn const_fold_handles_simple_arithmetic() {
        assert_eq!(
            with_parsed_expression!("1 + 1", |expr| const_fold(&expr, None)),
            Some("2".to_owned())
        );
    }

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

    #[test]
    fn tagged_folding_still_matches_identical_literals() {
        // Positive control: the fix must not make every fixture context unequal, which would
        // silence the rule rather than correct it.
        assert_eq!(fold_tagged("1000"), fold_tagged("1000"));
        assert_eq!(fold_tagged("'a'"), fold_tagged("'a'"));
    }

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

    // A composite condition carries exact syntax plus binding identities. Its truth value remains
    // unknown, but equality is comparable and must not taint the whole test context.
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

    // Defect 5: a literal condition is a genuinely known, comparable value and must keep comparing
    // equal to another occurrence of the same literal -- the fix must not make every condition
    // incomparable, only the ones that deserve it.
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

    // Copilot review #4, defect 1: a local `skipIf`/`runIf` condition's identity was scoped to
    // `<start>-<end>:name>`, unique only *within* one file. Two different files can declare a
    // same-named local binding at byte-for-byte identical offsets (identical boilerplate up to
    // that point) with opposite values -- `CrossFileKey` (`analysis.rs`) deliberately drops the
    // path when grouping for `cross-file-duplicate`, so a span-only identity collided these into a
    // false match. `const enabled = ` is the same length in both sources below, so `enabled`'s own
    // declaration span is byte-identical in each -- the only thing that can now tell them apart is
    // the file.
    #[test]
    fn local_condition_identity_is_scoped_by_file() {
        let source_a = r#"import { test } from "vitest"; const enabled = true; test.skipIf(enabled && isCI)("x", () => {});"#;
        let source_b = r#"import { test } from "vitest"; const enabled = false; test.skipIf(enabled && isCI)("x", () => {});"#;
        let a =
            resolve_first_call_in_file(source_a, Globals::Disabled, Path::new("a/file.test.ts"))
                .expect("should resolve as a skipIf declaration");
        let b =
            resolve_first_call_in_file(source_b, Globals::Disabled, Path::new("b/file.test.ts"))
                .expect("should resolve as a skipIf declaration");
        assert_ne!(
            skip_if_condition(&a),
            skip_if_condition(&b),
            "same-span local bindings in different files must not fingerprint identically"
        );
    }

    // Positive control for the fix above: only the *local* identity branch needed a file
    // component. A condition that resolves to a true global (no local declaration in either
    // file) must still fingerprint identically across files -- two references to the same
    // undeclared global really do mean the same thing, and the fix must not overreach into
    // breaking that legitimate cross-file match.
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

    // Defect 4: a `.each` table can be a literal array (so `literal_table_rows` returns `Some`)
    // while a specific row's cell is an identifier or call this extractor cannot read. That row
    // must be marked unresolved, independent of the table's own shape being fully known.
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

    // Copilot review #8: a JS object literal resolves a repeated static key to its LAST value
    // (`{ a: 1, a: 2 }` binds `a` to `2` at runtime), but `RowCells::Named` used to preserve every
    // occurrence, letting a `.find`-based lookup observe the FIRST one instead -- a value Vitest
    // itself never produces. This is the core of the fix: the duplicate must collapse to one
    // field holding the last-written value.
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

    // The case this fix is most likely to get backwards: when the LAST occurrence of a duplicate
    // key is itself not statically readable, that unreadable value still wins -- it is what
    // Vitest would actually bind -- so the row must be unresolved even though an earlier
    // occurrence under the same key was perfectly readable.
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

    // The mirror image, which the fix must also get right: when only an EARLIER, discarded
    // occurrence of a duplicate key was unreadable, and the LAST occurrence is a plain literal,
    // the row must stay resolved -- nothing at runtime ever observes that earlier value, so its
    // unreadability must not taint the row.
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

    // Field order pins JS's own semantics: a repeated key keeps its FIRST-insertion position
    // (`Object.keys({ a: 1, b: 2, a: 3 })` is `["a", "b"]`, not `["b", "a"]`) -- only the value
    // moves to the last occurrence, never the position.
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
            resolve_first_call("test.frobnicate`a\n${1}`('x', () => {});", Globals::Enabled)
                .is_none(),
            "an unknown tagged member must not resolve"
        );
    }

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
            ("{ a: 1 }", "{ a: 1 }", "object", "{a:<number:1>}"),
            ("{}", "{}", "object", "{}"),
            ("{ 'a-b': 1 }", "{ 'a-b': 1 }", "object", "{a-b:<number:1>}"),
            // A repeated key binds its last value at runtime, at its first position.
            ("{ a: 1, a: 2 }", "{ a: 2 }", "object", "{a:<number:2>}"),
            // Nesting, and a cast inside one.
            ("[[1], { a: [2] }]", "[ [ 1 ], { a: [ 2 ] } ]", "array", "[[<number:1>],{a:[<number:2>]}]"),
            ("[1 as const]", "[ 1 ]", "array", "[<number:1>]"),
            // A string that spells a delimiter cannot forge one: it is escaped, so this stays
            // distinguishable from a nested number.
            (r#"['<number:1\u003e']"#, r"[ '<number:1>' ]", "array", r"[<string:\<number:1\>>]"),
        ];
        for (literal, display, kind, raw) in readable {
            let value = cell(literal)
                .unwrap_or_else(|| panic!("`{literal}` should fold to a readable value"));
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
            ("{ a: 1 }", "{a:<number:1>}"),
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
            ("{ a: 1 }", "<object:{a:<number:1>}>"),
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
        let (different, _) = fixtures_of(
            "test.extend({ a: () => { const p = other(); return p; } })('t', () => {});",
        );
        assert_ne!(renamed[0].body, different[0].body);
    }

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
        let (fixtures, unresolved) =
            fixtures_of("test.extend('db', async () => 1)('t', () => {});");
        assert!(!unresolved);
        assert_eq!(fixtures.len(), 1);
        assert_eq!(fixtures[0].name, "db");
        assert!(matches!(fixtures[0].body, FixtureValue::Setup(_)));

        // `.extend().extend()` unions every link in the chain rather than keeping the last.
        let (fixtures, _) = fixtures_of("test.extend({ a: 1 }).extend({ b: 2 })('t', () => {});");
        let names: Vec<&str> = fixtures.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
        // ...and one unreadable link taints the whole chain.
        let (_, unresolved) =
            fixtures_of("test.extend({ a: 1 }).extend({ b: mk() })('t', () => {});");
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
        let condition =
            |expr: &str| condition_of(&format!("test.skipIf({expr})('t', () => {{}});")).0;

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

    #[test]
    fn import_forms_that_bind_nothing_useful_bind_nothing() {
        // A side-effect-only import of vitest declares no names at all.
        assert!(
            resolve_first_call("import 'vitest'; test('x', () => {});", Globals::Disabled)
                .is_none(),
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

        let resolved =
            resolve_outermost_call("(test?.each([[1]]))('x', () => {});", Globals::Enabled)
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
}
