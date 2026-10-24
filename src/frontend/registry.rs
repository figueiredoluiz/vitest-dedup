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
    /// Row substitution reads this too: it is the gate [`crate::frontend::extract`](mod@crate::frontend::extract)'s
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
///   `Serializer::literal` expects, for row substitution. This is
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
    /// A bare string at the top level and the inspect form otherwise -- see [`field_within_budget`].
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
    imported: &ImportedConstants<'_>,
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
                    imported,
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
                    imported,
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
                        imported,
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
    imported: &ImportedConstants<'_>,
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
            let rows = table_rows(
                arguments.first(),
                row_limit,
                semantic,
                imported,
                source,
                file,
            );
            resolved.steps.push(ChainStep::Each {
                is_for: false,
                rows,
            });
        }
        "for" => {
            let rows = table_rows(
                arguments.first(),
                row_limit,
                semantic,
                imported,
                source,
                file,
            );
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
    imported: &ImportedConstants<'_>,
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
    let scope = FoldScope {
        semantic,
        imported: Some(imported),
    };
    let rows = tagged_template_rows(quasi, row_limit, source, Some(scope));
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
    imported: &ImportedConstants<'_>,
    source: &str,
    file: &Path,
) -> Result<Vec<Row>, String> {
    match argument.and_then(Argument::as_expression) {
        Some(table) => literal_table_rows(table, row_limit, semantic, imported, source, file),
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
    imported: &ImportedConstants<'_>,
    source: &str,
    file: &Path,
) -> Result<Vec<Row>, String> {
    let scope = Some(FoldScope {
        semantic,
        imported: Some(imported),
    });
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
                    rows.push(analyze_whole_row(cell_expr, source, scope));
                } else {
                    rows.push(analyze_row(cell_expr, source, scope));
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
/// and, when `VD_CELL_CENSUS` is set, records why when it cannot.
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

/// The census histogram, printed once at the end of a run with `VD_CELL_CENSUS` set by the CLI.
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
            match resolve.and_then(|scope| resolved_binding(expr, scope, 0)) {
                Some(_) => "identifier (through a binding)".to_owned(),
                None => "identifier".to_owned(),
            }
        }
        Expression::StaticMemberExpression(_) => {
            match resolve.and_then(|scope| resolved_binding(expr, scope, 0)) {
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
pub(super) type Resolve<'s, 'a> = Option<FoldScope<'s, 'a>>;

/// What a fold resolves names against: the file's semantic, and the imported constants proved
/// for it ([`ImportedConstants`]; `None` where no file-level proof ran, such as an imported
/// helper's own frame).
#[derive(Clone, Copy)]
pub(super) struct FoldScope<'s, 'a> {
    semantic: &'s Semantic<'a>,
    imported: Option<&'s ImportedConstants<'a>>,
}

impl<'s, 'a> FoldScope<'s, 'a> {
    /// A scope that resolves only this file's own bindings.
    pub(super) fn local(semantic: &'s Semantic<'a>) -> Self {
        Self {
            semantic,
            imported: None,
        }
    }
}

/// Imported scalar constants proved for one file (see [`imported_constants`]): the importing
/// binding's symbol, mapped to the declaring module's literal cloned into this file's arena.
#[derive(Default)]
pub(super) struct ImportedConstants<'a> {
    values: HashMap<SymbolId, &'a Expression<'a>>,
}

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
/// their own values and never collide. An imported binding (whose declaration is an import
/// statement, not a `const`) resolves only when [`ImportedConstants`] proved it for this file.
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

/// Recognize immutable ECMAScript globals only after semantic resolution rules out lexical
/// bindings and dynamic `with` scopes. Missing semantic evidence never proves a global.
fn global_constant(expr: &Expression, resolve: Resolve<'_, '_>) -> Option<&'static str> {
    let Expression::Identifier(ident) = unwrap_transparent(expr) else {
        return None;
    };
    let name = match ident.name.as_str() {
        "undefined" => "undefined",
        "NaN" => "NaN",
        "Infinity" => "Infinity",
        _ => return None,
    };
    let scoping = resolve?.semantic.scoping();
    let reference = scoping.get_reference(ident.reference_id.get()?);
    if reference.symbol_id().is_some()
        || scoping
            .scope_ancestors(reference.scope_id())
            .any(|scope| scoping.scope_flags(scope).is_with())
    {
        return None;
    }
    Some(name)
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
    scope: FoldScope<'_, 'a>,
    hops: u32,
) -> Option<&'e Expression<'a>> {
    if hops >= MAX_VALUE_HOPS {
        return None;
    }
    let semantic = scope.semantic;
    match unwrap_transparent(expr) {
        Expression::Identifier(ident) => {
            let reference_id = ident.reference_id.get()?;
            let symbol_id = semantic.scoping().get_reference(reference_id).symbol_id()?;
            // An import proved to hold a scalar literal: already a closed value, cloned here.
            if let Some(value) = scope
                .imported
                .and_then(|imported| imported.values.get(&symbol_id))
            {
                return Some(value);
            }
            let declarator = const_declarator(semantic, symbol_id)?;
            if !declaration_precedes(declarator, ident.span()) {
                return None;
            }
            let init = declarator.init.as_ref()?;
            let resolved = resolved_binding(init, scope, hops + 1).unwrap_or(init);
            // An IMMUTABLE value only. A scalar cannot be changed out from under this read
            // (primitives are values, and `const` bars reassignment), which is what makes
            // following the binding a proof. A composite can: `const L = { n: 1 }; L.n = 2` is
            // legal, mutates the very object the initializer describes, and would leave this
            // reading a stale 1 where the runtime observes 2 -- so composite initializers stay
            // unread as whole values. (Member reads off them are the member arm below, with
            // their own discipline.)
            is_immutable_value(resolved, scope, hops + 1).then_some(resolved)
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
            let resolved = resolved_binding(value, scope, hops + 1).unwrap_or(value);
            is_immutable_value(resolved, scope, hops + 1).then_some(resolved)
        }
        _ => None,
    }
}

/// Whether `expr` is a value JavaScript cannot change out from under a reader: a scalar literal,
/// or a fold of scalars. Primitives are immutable and a `const` cannot be reassigned, so a
/// `const` whose initializer is THIS is stable for the life of the file; an object or array
/// never is, whatever the binding kind (`const L = [1]; L.push(2)`).
fn is_immutable_value<'a>(expr: &Expression<'a>, scope: FoldScope<'_, 'a>, hops: u32) -> bool {
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
        Expression::Identifier(_) => {
            global_constant(expr, Some(scope)).is_some()
                || resolved_binding(expr, scope, hops + 1).is_some()
        }
        Expression::TemplateLiteral(template) => template
            .expressions
            .iter()
            .all(|expression| is_immutable_value(expression, scope, hops)),
        Expression::UnaryExpression(unary) => {
            matches!(
                unary.operator,
                UnaryOperator::UnaryNegation | UnaryOperator::UnaryPlus
            ) && is_immutable_value(&unary.argument, scope, hops)
        }
        // Every arithmetic, comparison, and logical operator over scalars produces a scalar.
        Expression::BinaryExpression(binary) => {
            is_immutable_value(&binary.left, scope, hops)
                && is_immutable_value(&binary.right, scope, hops)
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
        .and_then(|scope| resolved_binding(expr, scope, 0))
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
    if global_constant(expr, resolve).is_some() {
        return const_fold_number(expr, resolve).filter(|value| !value.is_nan());
    }
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
/// This syntax-only helper refuses identifiers and calls. Hazard 6 warns that Vitest's own
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
/// `Serializer::literal` would produce for the same value written
/// directly as a literal in source -- including `null`, where this returns `("null", "null")`
/// rather than `const_fold_tagged`'s bare `<null>` (no raw part), because `Serializer::literal`
/// itself always renders `null` as `self.literal("null", "null")`. Accepts exactly the same
/// expression shapes as [`const_fold`], including semantically proven global primitives,
/// so the two never disagree
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
    if let Some(value) = global_constant(expr, resolve) {
        let kind = if value == "undefined" {
            "undefined"
        } else {
            "number"
        };
        return Some((kind, value.to_owned()));
    }
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
/// evaluator cannot establish. With semantic evidence, immutable global primitives and earlier
/// local const aliases also resolve; unknown bindings and calls remain refused.
pub(super) fn const_fold(expr: &Expression, resolve: Resolve<'_, '_>) -> Option<String> {
    let expr = peeled(expr, resolve);
    if let Some(value) = global_constant(expr, resolve) {
        return Some(value.to_owned());
    }
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
/// `Serializer::literal` as a `raw` and two different values that
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
                .map(|(name, value)| {
                    // The escaped key's byte length pins where the key ends, so key text can
                    // never impersonate the `name:value`, sibling `,` or bracket framing:
                    // `{a:[],b:1}` and `{'a:[],b':1}` rendered identically until 2026-10-18,
                    // collapsing two different tables' substituted bodies -- and two different
                    // `test.extend` fixture values -- into one `duplicate-test` at error
                    // severity. Scalar framing is covered by `escape_literal_delimiters`.
                    let escaped = escape_literal_delimiters(name);
                    format!("{}:{escaped}:{value}", escaped.len())
                })
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
    if is_string_valued(expr, resolve) {
        return None;
    }
    const_fold_number(expr, resolve)
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
pub(super) fn render_js_to_string(
    expr: &Expression,
    depth: u32,
    resolve: Resolve<'_, '_>,
) -> Option<String> {
    let expr = peeled_unwrapped(expr, resolve, depth)?;
    if let Some(value) = global_constant(expr, resolve) {
        return Some(value.to_owned());
    }
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
    let expr = peeled_unwrapped(expr, resolve, depth)?;
    if global_constant(expr, resolve) == Some("undefined") {
        return Some(String::new());
    }
    match expr {
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
/// `None` where `JSON.stringify` has no string to give: undefined yields no string, a BigInt
/// throws outright, and this
/// refuses rather than inventing a representation. Compact separators, matching the real thing
/// (`{"a":1}`, not `{ "a": 1 }`) -- measured: `%j` on `'a'` prints `"a"` while `%o` on the same
/// value prints `'a'`, which is exactly why the two cannot share one rendering.
fn render_json(expr: &Expression, depth: u32, resolve: Resolve<'_, '_>) -> Option<String> {
    let expr = peeled_unwrapped(expr, resolve, depth)?;
    if let Some(value) = global_constant(expr, resolve) {
        return (value != "undefined").then(|| "null".to_owned());
    }
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
                // Only a proven undefined value becomes null; an unknown element still refuses.
                parts.push(
                    if global_constant(peeled(element, resolve), resolve) == Some("undefined") {
                        "null".to_owned()
                    } else {
                        render_json(element, depth + 1, resolve)?
                    },
                );
            }
            Some(format!("[{}]", parts.join(",")))
        }
        Expression::ObjectExpression(_) => {
            let fields = object_fields(expr)?;
            let mut parts = Vec::with_capacity(fields.len());
            for (name, value) in fields {
                if global_constant(peeled(value, resolve), resolve) == Some("undefined") {
                    continue;
                }
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
    let expr = peeled(expr, resolve);
    match global_constant(expr, resolve) {
        Some("Infinity") => return Some(f64::INFINITY),
        Some("NaN" | "undefined") => return Some(f64::NAN),
        _ => {}
    }
    match unwrap_transparent(expr) {
        Expression::NumericLiteral(literal) => Some(literal.value),
        Expression::BinaryExpression(_) => const_fold(expr, resolve)?.parse().ok(),
        inner @ Expression::UnaryExpression(_) => fold_signed_number(inner, resolve),
        _ => None,
    }
}

/// Format numbers for static display, spelling NaN and positive/negative Infinity explicitly.
fn format_number(value: f64) -> String {
    // `-0` keeps its own spelling, deliberately NOT JavaScript's (`String(-0)` is `0`): the
    // distinction is observable in a test via `Object.is(value, -0)`, so two fixtures bound to
    // `0` and `-0` must not fold to one value. Every other number takes the ECMAScript
    // `Number::toString` spelling through ryu-js (non-finite folds included, the spelling C12
    // restored: `Infinity`, never Rust's `inf`). Rust's own `f64` Display was the previous
    // fallback and diverges exactly where ECMAScript switches notation: `1e21` printed as
    // `1000000000000000000000`, so an object key written `{1e21: 1}` (runtime key `"1e+21"`)
    // folded onto the unrelated string key `{'1000000000000000000000': 1}` while the runtime-
    // equal `{'1e+21': 1}` folded apart -- two wrong comparisons from one spelling. ryu-js is
    // the same formatter `render_js_to_string` already trusts, so display, keys and coercion
    // cannot drift apart again.
    if value == 0.0 && value.is_sign_negative() {
        return "-0".to_owned();
    }
    js_number_to_string(value)
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
/// `Serializer::ser_expr` already descends straight through every
/// one of these -- `TSAsExpression` serializes as its inner expression and nothing else -- so a
/// cell written `1 as const` and a cell written `1` are the *same* value to every comparison this
/// tool makes. Refusing to read the first was not caution; it excluded the test from comparison
/// altogether over a type annotation that the fingerprint erases anyway. Measured on a real
/// 24,147-test corpus: 207 unreadable cells, the largest non-composite cause.
pub(super) fn unwrap_transparent<'e, 'a>(expr: &'e Expression<'a>) -> &'e Expression<'a> {
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

pub(super) mod imported_constants;
#[cfg(test)]
mod tests;
