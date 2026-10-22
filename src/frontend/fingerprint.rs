//! Symbol-based body fingerprinting.
//!
//! This is the module CukeDedup could not have without a scope-and-symbol table. CukeDedup's
//! `typescript/handler.rs` renames identifiers by collecting declared names into a flat,
//! name-keyed map and substituting any token whose *text* matches (see
//! `collect_declared_locals`/`serialize_ast` there). That is sound for a single self-contained
//! handler body compared against exact fingerprint equality, but it is a name match, not an
//! identity match: a token that shares a name with a declared local but actually resolves to an
//! outer binding gets renamed anyway.
//!
//! VitestDedup cannot accept that gap, because a test body is compared alongside its hook chain,
//! fixtures, and imported helpers -- multiple scopes, sometimes multiple files -- and "local vs.
//! imported" is exactly the kind of pair the tool exists to tell apart (see
//! `fixtures/duplicates/not-duplicates-local-vs-imported-name.test.ts`). So every rename here goes
//! through `oxc_semantic`: for each identifier we ask which `SymbolId` it resolves to, not what
//! text it spells.
//!
//! ## The renaming rule
//!
//! - A symbol whose declaration span falls **inside** the body being fingerprinted (a parameter,
//!   a `const`/`let`/`var`, a `function`/`class` name, a `catch` binding, a nested closure's own
//!   locals) is a true local. Locals are renamed to `L0`, `L1`, ... in first-encounter order
//!   during a single deterministic left-to-right traversal. Two bodies with the same structure
//!   but different local spelling collapse to the same `alpha` -- this is what
//!   `fixtures/duplicates/duplicates-alpha-equivalent.test.ts` is checking.
//! - A symbol whose declaration span falls **outside** the body (an import, a module-level
//!   `const`, an outer closure variable) is a free reference. Getting its identity right took
//!   three attempts, because the within-file and cross-file directions pull in opposite ways and
//!   each fix this module shipped solved one while breaking the other:
//!     1. The original scheme used `free:name#{symbol_id}`. `SymbolId` is assigned in declaration
//!        order within one file's symbol table, so an unrelated import added above a test would
//!        renumber every later symbol and silently change that test's `alpha` (a fingerprint
//!        whose value depends on declaration order elsewhere in the file is not a fingerprint),
//!        and two byte-identical tests in different files that both import the same helper would
//!        get different, incomparable `SymbolId`s and could never be recognized as duplicates --
//!        exactly the case `duplicate-test` exists to catch.
//!     2. The fix made every free identity content-derived and scoped to the file, uniformly:
//!        `free:module:<file>:<name>` and `free:outer:<file>:<name>`. That closed the cross-file
//!        gap but broke the *within*-file case it wasn't looking at: sibling `describe` blocks
//!        each declaring their own `const commonParams = { ... }` with different values, and
//!        identical test bodies referencing `commonParams` in each -- distinct bindings, same
//!        name, same file, collapsed into one identity and reported as duplicates at Error
//!        severity.
//!     3. The two binding kinds need genuinely different treatment, because one names something
//!        outside this file and the other names something inside it:
//!     - An **import binding** (`scoping.symbol_flags(id).is_import()`) gets
//!       `free:import:resolved:<canonical path>:<name on the exporting side>` when the specifier
//!       actually resolves to a real file, and `free:import:unresolved:<anchored specifier>:<name>`
//!       when it does not. The lexical-anchoring scheme this replaced (string manipulation on the
//!       importer's own path, no filesystem access) stopped the cross-directory collision below
//!       but was explicitly not resolution: it could not follow tsconfig `paths`, package
//!       `exports` maps, or symlinked workspace packages, so two specifiers reaching the same
//!       module could still read as different (a missed finding) and, in principle, an alias
//!       could misread as matching an unrelated one. That was called out at the time as
//!       `oxc_resolver`'s job, "a later milestone" -- this module now calls
//!       [`resolver::resolve`](crate::frontend::resolver::resolve) (see `resolver.rs`) to answer "what file does this specifier
//!       actually name" for real, and keys a successful resolution's identity on the resolved,
//!       canonical path instead of the specifier text. `resolver.rs` lives in `src/frontend/`
//!       alongside this module; `oxc_resolver` is a separately-versioned (11.x, ordinary semver),
//!       standalone crate, not part of the pinned-at-0.149.0 `oxc_*` parser/AST/semantic family.
//!       But `AGENTS.md`'s parser-containment rule is scoped to the *directory*
//!       ("only `src/frontend/` may import an `oxc_*` crate"), not to that
//!       specific family, so this needs no separate exception.
//!
//!       Resolution touches the filesystem, which can fail for reasons that have nothing to do
//!       with whether two imports name the same thing: a dependency that is not installed, a
//!       specifier this resolver's algorithm does not cover, a malformed `tsconfig.json`. A
//!       failure must not silently fall back to anything that risks a false merge, so an
//!       unresolved import keeps the old lexically-anchored specifier text (bare/package
//!       specifiers used as written, directory-independent; relative specifiers anchored via
//!       `imports::normalize_relative_specifier`, unchanged from the scheme above) but the whole string
//!       is prefixed `unresolved:` -- structurally, not just by convention, so it can never equal
//!       any `resolved:<path>` identity regardless of what path string a resolution could ever
//!       produce. This was chosen over the one other sound option -- routing the failure
//!       through `Context::unresolved` instead -- because that flag is
//!       produced and consumed entirely outside this module (`extract.rs`/`model.rs`, owned
//!       elsewhere) and fingerprint identity is the only channel this module actually owns; the
//!       `unresolved:` tag gets the same soundness guarantee (a failed resolution can never
//!       collide with a successful one) without reaching outside this module's boundary. Within
//!       one file (or, for the unresolved fallback, within one directory), this still correctly
//!       separates different named imports from the same specifier
//!       (`free:import:resolved:<path>:a` != `free:import:resolved:<path>:b`). That is
//!       load-bearing, and not only here: [`crate::model::Context`]`::imports` records
//!       specifiers, not imported names, so two bodies referencing different named exports of one
//!       module carry the *same* `imports` entry. That coarseness is only harmless because this
//!       fingerprint makes the finer distinction one level down, which leaves `Context::imports`
//!       as a secondary gate rather than the thing that tells two named imports apart. The
//!       assumption is pinned by `src/analysis/tests.rs`'s
//!       `named_imports_from_the_same_module_are_assumed_to_get_different_alpha`, so a change
//!       here that collapsed the two identities would fail a test instead of silently widening
//!       every `Context` comparison.
//!
//!       One consumer of this identity deserves its own line: a call in *call position* whose
//!       callee resolves to an import binding may not render the identity at all -- when the
//!       import resolves to a readable file and the helper's parameters bind exactly,
//!       `try_inline_imported_helper` renders the helper's own body in place of the call (see
//!       `helpers.rs`; `alpha` only, never `structural`). The identity remains the rendering for
//!       every refusal, and for every non-call reference, which is what keeps refused and
//!       unresolvable imports from ever colliding with inlined bodies.
//!     - A **module-level declaration or outer closure variable** names something that lives
//!       *inside* this file, where sibling scopes routinely reuse a name for unrelated bindings
//!       (the `describe.each`-shaped case above). Content -- the name -- is not what identifies
//!       one of these; the declaration itself does. So these use the declaration's own span
//!       (`scoping.symbol_span`) plus the file:
//!       `free:module:<file>:<declaration span>:<name>` and
//!       `free:outer:<file>:<declaration span>:<name>`. A span is not vulnerable to (1)'s failure
//!       mode -- it is stable under edits anywhere else in the file, unlike a declaration-order
//!       index -- while still telling sibling same-named bindings apart, because they occupy
//!       different spans. This is also the right answer for a *future* `cross-file-duplicate`
//!       rule, not merely a safe one: two file-local bindings in two different files are
//!       different bindings regardless of name, so there is no cross-file case where collapsing
//!       them by name would ever have been correct.
//!       In every case the file scoping (`<file>`, from [`path_key`]) matters for the same reason:
//!       `duplicate-test` is file-scoped, but an identity scheme that is only safe because
//!       nothing compares across files is a trap for `cross-file-duplicate`, whose entire job is
//!       to lift exactly that restriction and group the same key across every file in the run.
//! - An identifier with **no** resolved symbol at all (a true global such as `expect` under
//!   `globals: true`, or anything `oxc_semantic` could not bind) keeps `global:name`. Using the
//!   name alone here is intentional and safe: unlike locals, these are never given positional
//!   names, so two bodies that each call a single, different global do not collapse onto the same
//!   placeholder.
//!
//! ## Traversal shape
//!
//! oxc's AST is a large family of distinct node types (unlike tree-sitter's single homogeneous
//! `Node`), so the "explicit stack of untyped nodes" trick CukeDedup's `handler.rs` uses does not
//! translate directly -- there is no one node type to push. Instead every serialization/behavior
//! function threads an explicit `depth: u32` counter and refuses to recurse past [`MAX_DEPTH`],
//! emitting a bounded opaque marker instead. `MAX_DEPTH` is small enough (a few hundred) that the
//! real call stack this uses is nowhere near overflow, so pathological input degrades the output
//! (two subtrees deeper than the cap that otherwise share a prefix could under-match past that
//! point) rather than crashing the process. This is the one place this module accepts recursion
//! instead of an explicit work stack, and it is bounded on every path.
//!
//! ## Deliberate under-matching
//!
//! Every place this module gives up and falls back to raw source text is a deliberate choice to
//! bias toward false negatives (two different things stay different) rather than risk a false
//! positive (two different things collapse). Each is called out at the match arm that does it:
//! parenthesization and TS type-only wrappers (`as`, `satisfies`, non-null `!`, type assertions)
//! are transparently unwrapped everywhere because they carry no runtime identity (the one
//! exception is a parenthesized optional chain used as a receiver, which ends the chain and is
//! marked `chain-end`), but destructuring
//! *assignment* targets (not declarations), decorators, class bodies, and most TS-only declaration
//! forms are rendered as opaque raw text. JSX is the one construct that outgrew that treatment and
//! is serialized structurally (`ser_jsx_element`) -- component tests are too large a share of real
//! suites for the whitespace sensitivity of a raw slice to be an acceptable under-match, and an
//! attribute literal hidden inside one is a value this module's `structural` view needs to see.
//!
//! Raw text only under-matches when it carries its own meaning. A slice keeps the original
//! spelling of every name, so identical text can resolve to different bindings: the fallback
//! therefore appends the identity of each binding declared outside the slice
//! (`Serializer::raw_fallback`), and a slice using `this`, `arguments` or `new.target` of a
//! function outside it makes the body depend on its callback form.
//!
//! ## Explicit type arguments
//!
//! "Carries no runtime identity" is not the same as "says nothing", and conflating the two cost
//! this module an Error-severity false positive: an explicit type-argument list
//! (`assertEqual<Record<string, string>, Expected>(true)`) was dropped from every fingerprint,
//! because it is erased before the program runs. For a type-level test suite the type argument
//! *is* the assertion -- `expectTypeOf<A>().toEqualTypeOf<B>()`, `assertType<T>(value)`, and the
//! hand-rolled `assertEqual<A, B>(true)` helpers that schema-validation suites use say nothing at
//! all -- so erasing it made every such test in a file a byte-identical `duplicate-test` of its
//! siblings. [`type_argument_fragment`] now renders the list into the serialization of every
//! construct that can carry one (`CallExpression`, `NewExpression`,
//! `TaggedTemplateExpression`, and `TSInstantiationExpression`) and into the call/assertion
//! behavior events, in all three modes -- see [`Serializer::ser_type_arguments`] for why
//! `structural` keeps them rather than collapsing them the way it collapses a literal's value,
//! and [`text::normalize_type_argument_text`] for how formatting is kept from mattering.
//!
//! Type *annotations* (a parameter's or variable's `: T`, an `as T` cast's target) remain erased.
//! They are a separate construct with a separate trade-off -- an annotation constrains a value
//! the fingerprint already renders, where a type argument is frequently the only content a
//! type-level test has -- and modeling them belongs to whoever measures that on a corpus.
//!
//! ## Row substitution
//!
//! A `test.each`/`test.for` row's body used to be fingerprinted purely from the callback's own
//! *parameter identifiers* -- `test.each([[2, 3, 6]])('...', (w, h, expected) => expect(area(w,
//! h)).toBe(expected))` fingerprinted identically for every row, differing only in the title, and
//! never matched a hand-written `test('...', () => expect(area(2, 3)).toBe(6))` even though a
//! literal row is that hand-written test in every way that matters. [`RowSubstitution`] closes
//! that gap: when `extract.rs` can prove every cell of a row's own value statically (see
//! `registry::Row::unresolved`), it hands this module a `SymbolId`-free description of which
//! parameter binds which cell, and [`resolve_row_substitution`] turns that into a
//! `SymbolId -> (kind, raw)` map scoped to this one body. Two places consult it, and both must
//! agree for a substituted row to actually match a hand-written duplicate:
//!
//! - `Serializer::reference_or_literal` renders a substituted parameter's *read* occurrences
//!   exactly as [`Serializer::literal`] would render the same value written directly in source --
//!   not as `id:<substituted>`, because the whole point is that a substituted reference and a real
//!   literal in the same position must serialize byte-for-byte identically, and every non-literal
//!   `Expression::Identifier` already goes through the `id:` prefix specifically to keep it
//!   visibly distinct from a literal.
//! - `Serializer::ser_params`/`ser_binding_pattern` omit a fully-substituted parameter's own
//!   *declaration* from the signature entirely (see `binding_fully_substituted`), rather than
//!   printing it as an unused canonical local. A hand-written duplicate of a substituted row never
//!   declares that parameter at all -- `(w, h, expected) => expect(area(w, h)).toBe(expected)`
//!   substituted for `[2, 3, 6]` must match `() => expect(area(2, 3)).toBe(6)`, which has zero
//!   parameters, not three unused locals sitting in the signature. Leaving the declaration behind
//!   would make every substituted row permanently unmatchable against the hand-written form this
//!   feature exists to link.
//!
//! This is symbol-scoped, never name-matched, for the same reason every other identity in this
//! module is (see the top of this doc): a row's cell binds to *that* row's own parameter
//! declaration, and a same-named local shadowing it inside the body must resolve to its own
//! declaration, not the parameter's, and therefore never gets substituted.
//!
//! Three guards keep this from ever producing a false merge, each all-or-nothing rather than
//! best-effort, because a half-substituted body is strictly more dangerous than an
//! unsubstituted one: substituting some parameters (to literals) while leaving one sibling as an
//! anonymous canonical local *removes* differentiation a fully unsubstituted body still had,
//! which can turn two genuinely different bodies into an accidental match. No guard tries to
//! substitute whatever it safely can and give up only on the rest:
//!
//! - **Cell readability** (`extract.rs`'s job): a row with *any* cell this extractor could not
//!   read at all (an identifier, a call, a spread, ...) never produces a [`RowSubstitution`] in
//!   the first place -- see `registry::Row::unresolved` and `extract::row_substitution`.
//! - **Parameter shape** ([`resolve_row_substitution`]'s job): a rest parameter, a default value,
//!   a positional cell binding to a destructuring pattern instead of a plain identifier, or a
//!   destructured object parameter with a rest or a nested pattern, all bail out to "substitute
//!   nothing for this row" rather than substituting the parameters that *do* have a simple shape.
//!   A parameter with no corresponding cell at all (a trailing `TestContext` beyond the row's own
//!   arity, or a destructured property the row's object simply does not have) is different in
//!   kind, not degree: there is no cell to have failed to read, so leaving that one parameter as
//!   an ordinary canonical local is exactly today's (safe) pre-substitution behavior for it, not a
//!   partial application of this feature.
//! - **Parameter writes** (also [`resolve_row_substitution`]'s job, mirroring helper inlining's
//!   written-parameter refusal in `helpers.rs`): a callback that writes a parameter the row would
//!   substitute -- `x = 5`, `x += 1`, `x++`, a destructuring write, a `for...of` target -- bails
//!   out to "substitute nothing" like the shape cases above. A written parameter no longer holds
//!   the cell it bound, so its later reads would render the cell where the body actually
//!   observes the write: `(x) => { x = 5; expect(x).toBe(6) }` bound from `[6]` would
//!   fingerprint `expect(6).toBe(6)` while Vitest evaluates `expect(5).toBe(6)` -- measured
//!   against vitest 5.0.3, and two tests on opposite sides of that gap merged at Error severity.
//!   Like the no-cell case, a written parameter with *no* cell is untouched by this guard: there
//!   is nothing to substitute for it, so its writes are its own to render.
//!
//! Substitution only ever changes a read *reference* to a bound parameter (`add(w, h)`). Write
//! positions (an assignment target, a `for...of` loop variable) are deliberately left alone --
//! substituting a write target with a literal would be nonsensical, not merely imprecise. With
//! the write guard above, a write position always belongs to a parameter substitution never
//! claimed.
//! `structural` and `alpha` (and, through `BehaviorCollector::shape`, `behavior`'s assertion
//! subject/value shapes) all substitute, because those are exactly the three views
//! `duplicate-test`/`parameterization-candidate`/a future `near-duplicate-test` key on.
//!
//! This says nothing about whether two rows of *one* `.each` table should ever be reported as
//! duplicates of each other -- they should not, and substitution does not change that: it makes
//! their bodies genuinely differ (by design), and the existing declaration-site guard in
//! `analysis.rs` (`dedup_by_declaration`) excludes same-`Location` members before alpha/structural
//! equality is even consulted, so it holds regardless of what the bodies' fingerprints say.

mod behavior;
mod behavior_calls;
mod classes;
mod context;
mod dataflow;
mod identity;
mod imports;
mod row_substitution;
mod serializer;
mod serializer_members;
mod text;
mod trivial;

use behavior::BehaviorCollector;
#[cfg(test)]
use context::body_run;
use context::FingerprintContext;
pub(crate) use context::Mode;
pub(super) use dataflow::analyze_dataflow;
pub(super) use dataflow::expression_has_effect;
use dataflow::pure_literal;
use dataflow::DataflowSubst;
pub(super) use identity::binding_identity;
pub(super) use identity::canonical_local;
pub(super) use identity::is_expect_identifier;
pub(super) use identity::is_local_symbol;
pub(super) use identity::reference_identity;
pub(super) use identity::within;
pub(super) use imports::file_import_origins;
#[cfg(test)]
pub(super) use imports::import_origin_parses;
#[cfg(test)]
pub(super) use imports::import_origins;
pub(super) use imports::import_origins_from_program;
pub(crate) use imports::module_export_name;
pub(super) use imports::path_key;
pub(super) use imports::specifier_identity;
pub(super) use imports::value_import_declarations;
pub(super) use imports::with_file_import_origins;
pub(super) use row_substitution::resolve_row_substitution;
pub(crate) use row_substitution::RowSubstitution;
use serializer::Serializer;
pub(super) use text::escape_literal_delimiters;
pub(super) use text::normalize_jsx_text;
#[cfg(test)]
pub(super) use text::normalize_type_argument_text;
pub(super) use text::type_argument_fragment;
use trivial::is_trivial;
#[cfg(test)]
mod tests;

use super::registry::property_key_name;
use crate::model::{BehaviorTrace, BodyDigest, CallbackForm, Fingerprints};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionBody, ArrowFunctionExpression, AssignmentTarget,
    BindingIdentifier, BindingPattern, CallExpression, CatchClause, ChainElement, ChainExpression,
    Class, ComputedMemberExpression, DoWhileStatement, Expression, ForInStatement, ForOfStatement,
    ForStatement, ForStatementInit, ForStatementLeft, FormalParameters, Function,
    IdentifierReference, IfStatement, ImportDeclarationSpecifier, JSXAttributeItem,
    JSXAttributeName, JSXAttributeValue, JSXChild, JSXElement, JSXElementName, JSXFragment,
    JSXMemberExpressionObject, ModuleExportName, NewExpression, ObjectPattern, ObjectPropertyKind,
    PrivateFieldExpression, PropertyKey, PropertyKind, SimpleAssignmentTarget, Statement,
    StaticMemberExpression, SwitchCase, SwitchStatement, TSInstantiationExpression,
    TSTypeParameterInstantiation, TaggedTemplateExpression, TemplateLiteral, TryStatement,
    VariableDeclaration, VariableDeclarationKind, VariableDeclarator, WhileStatement,
};
use oxc_ast::AstKind;
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::{AstNode, AstNodes, NodeId, Semantic};
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::operator::UnaryOperator;
use oxc_syntax::reference::ReferenceId;
use oxc_syntax::scope::ScopeFlags;
use oxc_syntax::symbol::SymbolId;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

/// Everything the compared fingerprints need to know about one import binding, keyed by local
/// binding name. `component`/`exported` are what `identity::free_identity` keys identity on (see the
/// module doc); `specifier`/`resolved` exist for imported-helper inlining (see `helpers.rs`),
/// which needs the raw specifier (to refuse the framework itself, on the same text the registry
/// keys that refusal on) and the resolved path (to go read the helper's body).
#[derive(Clone, Debug)]
pub(super) struct ImportOrigin {
    /// `resolved:<canonical path>` or `unresolved:<anchored specifier>`; see
    /// [`specifier_identity`].
    pub(super) component: String,
    /// The name on the exporting side (`import { a as b }` records `a`).
    pub(super) exported: String,
    /// The specifier exactly as written (`./helpers`, `vitest`, `@scope/pkg`).
    pub(super) specifier: String,
    /// The file the specifier resolved to, when resolution succeeded. `None` is exactly
    /// [`crate::frontend::resolver::Resolved::Unresolved`], and with it no helper call through this import
    /// can ever be inlined -- an unverifiable body must stay an identity, never a guess.
    pub(super) resolved: Option<PathBuf>,
}

/// A file's top-level imports, keyed by local binding name. See [`imports::import_origins`].
pub(super) type ImportOrigins = HashMap<String, ImportOrigin>;

/// Hard cap on serialization/behavior-collection recursion.
///
/// The previous value, 512, was justified by a claim -- "an order of magnitude below any
/// realistic stack overflow risk (each recursive frame here is a small `match` arm, not a large
/// stack allocation)" -- that nobody had measured, and it was wrong by an order of magnitude in
/// the other direction. `Serializer::ser_expr` is one `match` with dozens of arms, and an
/// unoptimized build gives every arm's locals their own slot in one shared frame: the measured
/// frame is **~3.3 KB**, so a full-depth recursion at 512 consumed **1,713,472 bytes** -- 82% of
/// the 2 MiB stack a `libtest` thread gets, leaving ~375 KiB. That is not headroom, it is a fuse:
/// adding three `String` locals to a single arm once blew it (mitigated at the time by hoisting
/// arms into their own methods -- see [`Serializer::ser_new`] -- which moves the cost rather than
/// removing it), and every later arm widening re-lit it.
///
/// 256 halves the requirement to a measured **861,504 bytes** -- 41% of that thread, leaving
/// ~1.2 MiB -- and it is the one
/// depth this frontend already commits to: `extract::MAX_NESTING_DEPTH` refuses to hand
/// `oxc_parser` any source whose bracket nesting exceeds 256, so no bracketed construct
/// (`(`/`[`/`{` -- objects, arrays, arguments, parenthesized groups, template interpolations) can
/// reach this cap at all. What still can is a bracket-free operator chain -- `!!!x`, `a + b + c`,
/// `a.b.c`, `a ? b : c ? d : e` -- which needs 256 links in one test body to get here, and which
/// degrades rather than crashes when it does.
///
/// Degrading is safe in the direction that matters: past the cap [`Serializer::degraded`] falls
/// back to the node's **raw source text**, which is byte-exact, so two subtrees deeper than the
/// cap can only fail to match (an alpha-rename below the cap stops being canonicalized) -- they
/// can never collide into a false duplicate. Lowering the cap therefore trades a little
/// under-matching on input no human writes for stack margin, never precision on real input.
///
/// The product of this constant and the frame size is now measured and asserted, rather than
/// assumed, by `tests::ser_expr_at_max_depth_stays_within_its_stack_budget` against
/// `MAX_DEPTH_STACK_BUDGET_BYTES`.
const MAX_DEPTH: u32 = 256;

/// Stack ceiling for a full-[`MAX_DEPTH`] [`Serializer::ser_expr`] recursion, asserted by
/// [`tests::ser_expr_at_max_depth_stays_within_its_stack_budget`].
///
/// Derived from the smallest stack this code is known to run on -- the 2 MiB a `libtest` thread
/// gets by default (confirmed by bisection with `RUST_MIN_STACK`: the pre-fix 512-deep recursion
/// survived 1,835,008 bytes and aborted at 1,700,000) -- halved, so the recursion may occupy at
/// most half of it and the other half stays available to callers, `oxc` internals, and the
/// harness. The measured cost at `MAX_DEPTH = 256` is 861,504 bytes, inside this with ~183 KiB
/// (about 728 bytes per frame) of room for the frame to grow before the test objects.
///
/// Raising this number is not a fix for a failure of that test. It is the budget; the frame or
/// the depth is what should move.
#[cfg(test)]
const MAX_DEPTH_STACK_BUDGET_BYTES: usize = 1024 * 1024;

#[cfg(test)]
thread_local! {
    /// Test-only stack low-water mark, in absolute addresses, for
    /// [`tests::ser_expr_at_max_depth_stays_within_its_stack_budget`].
    ///
    /// A depth cap only bounds stack use in combination with a per-frame cost, and nothing
    /// measured that product -- which is how this module ended up occupying 82% of a test
    /// thread's stack (see [`MAX_DEPTH`]'s doc). This makes it observable:
    /// [`record_stack_low_water_mark`] is called on entry to [`Serializer::ser_expr`] -- the
    /// deepest and widest of this module's recursions -- and keeps the lowest stack address any
    /// frame reached, so a test can subtract it from its own frame's address and get the exact
    /// byte cost of a full-depth serialization.
    static STACK_LOW_WATER_MARK: std::cell::Cell<usize> =
        const { std::cell::Cell::new(usize::MAX) };
}

/// Records the current frame's stack address into [`STACK_LOW_WATER_MARK`] if it is the lowest
/// seen. `#[inline(never)]` so its own frame is real and the measurement is conservative (it
/// includes one extra callee frame beyond `ser_expr`'s own) rather than optimistic.
#[cfg(test)]
#[inline(never)]
fn record_stack_low_water_mark() {
    let probe = 0u8;
    let address = std::ptr::addr_of!(probe) as usize;
    STACK_LOW_WATER_MARK.with(|cell| {
        if address < cell.get() {
            cell.set(address);
        }
    });
}

/// Bound on how many member hops an assertion chain walk will follow before giving up.
///
/// Both recognizers share it, and each declines differently past it:
/// [`BehaviorCollector::assertion_event`] (the `.not`/`.resolves`/`.rejects` hops before a
/// matcher call) falls back to treating the call as an ordinary, non-assertion call;
/// [`BehaviorCollector::property_chain_event`] (the chai-style `to.not.have.been` links, which
/// are unrestricted words rather than a fixed set) falls back to treating the member access as
/// an ordinary member access, contributing no event of its own.
///
/// Independent of [`MAX_DEPTH`]: this bounds a single linear unwrap loop, not tree recursion.
const MAX_ASSERTION_CHAIN: usize = 16;

/// The two shapes Vitest accepts as a test, hook, or suite callback.
pub(super) enum FunctionLike<'a, 'b> {
    /// `() => { ... }` or `() => expr`.
    Arrow(&'b ArrowFunctionExpression<'a>),
    /// `function () { ... }`, including `async function` and generators.
    Function(&'b Function<'a>),
}

impl<'a> FunctionLike<'a, '_> {
    /// The callable's own node in the semantic model.
    fn node_id(&self) -> NodeId {
        match self {
            Self::Arrow(arrow) => arrow.node_id.get(),
            Self::Function(function) => function.node_id.get(),
        }
    }

    /// The span covering the whole callable, params through body.
    ///
    /// This is the boundary the renamer uses to decide "local vs. free": any symbol declared
    /// inside this span is a local of the body being fingerprinted, and anything declared outside
    /// it (an import, an outer `const`, a closed-over variable) is a free reference. See the
    /// module doc.
    fn span(&self) -> Span {
        match self {
            Self::Arrow(arrow) => arrow.span,
            Self::Function(function) => function.span,
        }
    }

    /// Borrow formal parameters from either supported callable form.
    pub(super) fn params(&self) -> &FormalParameters<'a> {
        match self {
            Self::Arrow(arrow) => &arrow.params,
            Self::Function(function) => &function.params,
        }
    }

    /// Return the callable's async flag for arrow and ordinary function forms.
    fn is_async(&self) -> bool {
        match self {
            Self::Arrow(arrow) => arrow.r#async,
            Self::Function(function) => function.r#async,
        }
    }

    /// Report whether this callable is an ordinary generator function.
    fn is_generator(&self) -> bool {
        matches!(self, Self::Function(function) if function.generator)
    }
}

/// Computes all five [`Fingerprints`] for one test/hook body.
///
/// `semantic` must be built from the same `Program` that `body` is a node of -- callers own one
/// file's parse and `Semantic` and call this once per discovered callback. `source` is the whole
/// file's text (not just this body's), which is what makes [`imports::import_origins`] possible. `file` is
/// that same file's own path -- **this parameter was added to close a proved cross-file
/// false-positive** (see the module doc's discussion of `free:import`/`free:module`/`free:outer`)
/// and is not optional: without it, a relative import specifier cannot be anchored to the
/// importing file's directory, and a module-level or outer free reference cannot be scoped to its
/// own file, so two unrelated files can collide into a false `duplicate-test`. Callers should pass
/// the same path used for [`crate::model::Location::path`] so identities computed from `file`
/// line up with how findings are reported.
///
/// Never panics: unresolvable identifiers fall back to a name-derived identity, unhandled node
/// shapes fall back to raw source text, recursion is depth-capped, and a failure to re-parse
/// `source` for import info degrades to an empty map rather than propagating.
///
/// `row_substitution` is `Some` only when `body` is a `test.each`/`test.for` row callback whose
/// row `extract.rs` proved fully readable (see the module doc's "Row substitution" section);
/// every other caller (hooks, `test.extend` fixture bodies, a non-`.each` test, ...) passes `None`.
pub(super) fn fingerprint(
    body: FunctionLike<'_, '_>,
    semantic: &Semantic<'_>,
    source: &str,
    file: &Path,
    row_substitution: Option<&RowSubstitution>,
) -> BodyFingerprint {
    let trivial = is_trivial(&body);
    let imports = file_import_origins(source, file);
    let file_key = path_key(file);
    // Resolved once, shared by all four passes below: they all walk the identical
    // `body.params()`, so re-deriving this per pass would just repeat the same work (and, worse,
    // risk the four passes disagreeing about which parameters substitute if the logic ever drifted
    // between call sites).
    let substitution = row_substitution
        .and_then(|row_substitution| {
            resolve_row_substitution(body.params(), row_substitution, semantic)
        })
        .unwrap_or_default();
    // Dataflow canonicalization (see the section doc above `DataflowSubst`) is consulted ONLY by
    // the `behavior` pass, never by `alpha`/`structural` -- those two get a permanently empty
    // map, regardless of what `analyze_dataflow` found for this body. See the section doc for
    // why: an unverified dataflow equivalence must never silently upgrade into an
    // `alpha`/`structural` claim.
    let dataflow = analyze_dataflow(&body, semantic);
    let no_dataflow: HashMap<SymbolId, DataflowSubst<'_, '_>> = HashMap::new();
    // One construction of the shared pass context (with the empty dataflow map, which is what
    // `alpha`/`structural` must see); each pass then takes its own fresh copy so canonical
    // naming starts from zero, and the behavior pass alone swaps in the real dataflow map.
    let context = FingerprintContext::new(
        semantic,
        source,
        &imports,
        file_key,
        file.to_path_buf(),
        body.span(),
        body.node_id(),
        &substitution,
        &no_dataflow,
    );
    // The alpha pass is borrowed rather than consumed so its import accounting survives the run:
    // `inlined_imports` is what extract.rs drops from `Context::imports`, taken from the pass
    // that actually rendered the body rather than re-derived beside it.
    let mut alpha_pass = Serializer::new(context.fresh_for_pass(), Mode::Alpha);
    let (alpha, this_dependent) = alpha_pass.run_observing_this(&body);
    let inlined_imports = alpha_pass.ctx.fully_inlined_imports();
    let structural = Serializer::new(context.fresh_for_pass(), Mode::Structural).run(&body);
    let behavior_run = BehaviorCollector::new(context.fresh_for_pass_with(&dataflow)).run(&body);
    // `behavior_truncated` is the run's other half (see `BehaviorRun`): a trace the depth cap
    // cut short compares equal to a different body's equally-cut trace, so it must never reach
    // a rule as evidence. Like `class_unresolved` below, it is consumed by extract.rs and
    // deliberately absent from `digest()`.
    let behavior = behavior_run.events;
    // `this_dependent` comes from the alpha pass, which already decided it -- computing the same
    // thing twice is how two views of one body drift apart.
    let callback_form = if this_dependent {
        match &body {
            FunctionLike::Arrow(_) => CallbackForm::Arrow,
            FunctionLike::Function(_) => CallbackForm::Function,
        }
    } else {
        CallbackForm::Interchangeable
    };
    BodyFingerprint {
        alpha,
        structural,
        behavior,
        trivial,
        callback_form,
        inlined_imports,
        class_unresolved: context.class_uncertain.get(),
        behavior_truncated: behavior_run.truncated,
    }
}

/// A helper body and the owned class uncertainty discovered while rendering it.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct HelperRendering {
    pub text: String,
    pub class_unresolved: bool,
}

/// Serializes one imported helper's body for inlining at a call site -- the rendering half of
/// imported-helper inlining (`helpers.rs` owns locating the helper and the guards; this module
/// owns what its body renders to). `argument_fragments` are the call site's arguments, already
/// serialized by the *importing* file's pass; each parameter read in the helper's body renders
/// as its argument's fragment, exactly the terms [`Serializer::reference_or_literal`] substitutes
/// a `.each` row's cells.
///
/// The rendering is a fresh pass over the *helper file's* own context: its own imports (their
/// identities resolved from the helper file, so what the helper itself depends on participates in
/// the comparison), its own `path_key` scoping its module-level bindings, its own canonical
/// locals. Inlining is disabled inside it ([`FingerprintContext::inline_imported_helpers`]), so
/// a helper calling another imported helper renders that call as an identity rather than
/// recursing -- the one-level bound, chosen over a depth-capped recursion because each level of
/// helper-calls-helper is a body nobody has proved equivalent, and one level already carries the
/// claim this feature exists to make.
///
/// The wrapper carries the properties of the callable itself -- `async`, generator, arrow vs
/// `function` -- because they change what a call to it *is* (a returned promise, an iterator, a
/// different `this` binding) even when the statements are identical. Two helpers that differ
/// only in one of those are not equivalent and must not render alike.
///
/// # Why the fragments are spliced, not tokenized (a review suggestion, declined)
///
/// A parameter read renders its argument's fragment *verbatim at each read position*, rather
/// than the alternative of rendering arguments once and substituting positional placeholders at
/// reads. The alternative was declined twice, for two reasons. First, textual placeholders can
/// collide with literal text (the payload of any `<string:...>`), and the one mechanism this
/// module already has against that class -- [`escape_literal_delimiters`] -- exists because the
/// collision actually happened; fragments composed by the serializer's own grammar add no new
/// way to spell a collision, because the whole rendering is only ever compared for equality,
/// never parsed back. Second, a substituted read rendering exactly as its argument *is* the
/// point: `f(1)` against a helper reading its parameter and a helper returning `1` must compare
/// equal when the argument is `1` -- beta-reduction, the same equivalence `.each` row
/// substitution ships (`fixtures/recall/`'s `duplicate-test-imported-helper-*` cases pin the
/// twin directions: equal when the bodies agree, distinct when a literal differs).
///
/// Never panics; a helper whose parameters do not bind exactly (a guard `helpers.rs` enforces
/// before calling) degrades to unsubstituted canonical locals rather than failing.
/// Every parameter is a distinct piece of construction context with no natural grouping --
/// the same rationale `FingerprintContext::new`'s own `#[allow]` records.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_inlined_helper_body(
    semantic: &Semantic<'_>,
    source: &str,
    imports: &ImportOrigins,
    file: &Path,
    body: &FunctionLike<'_, '_>,
    argument_fragments: &[String],
    mode: Mode,
    depth: u32,
) -> HelperRendering {
    let mut arg_substitution = HashMap::with_capacity(argument_fragments.len());
    for (index, parameter) in body.params().items.iter().enumerate() {
        let BindingPattern::BindingIdentifier(binding) = &parameter.pattern else {
            continue;
        };
        let (Some(symbol), Some(fragment)) =
            (binding.symbol_id.get(), argument_fragments.get(index))
        else {
            continue;
        };
        arg_substitution.insert(symbol, fragment.clone());
    }
    let no_rows: HashMap<SymbolId, (&'static str, String)> = HashMap::new();
    let no_dataflow: HashMap<SymbolId, DataflowSubst<'_, '_>> = HashMap::new();
    let mut context = FingerprintContext::new(
        semantic,
        source,
        imports,
        path_key(file),
        file.to_path_buf(),
        body.span(),
        body.node_id(),
        &no_rows,
        &no_dataflow,
    );
    context.arg_substitution = arg_substitution;
    context.inline_imported_helpers = false;
    // Helper-local accounting dies with the pass and cannot claim an importing-file dependency.
    context.local_prefix = "H";
    let mut serializer = Serializer::new(context, mode);
    let asyncness = if body.is_async() { "async " } else { "" };
    let generator = if body.is_generator() { "* " } else { "" };
    let form = match body {
        FunctionLike::Arrow(_) => "arrow-fn",
        FunctionLike::Function(_) => "function-fn",
    };
    // The helper's statements serialize starting one level inside the wrapper, which itself
    // sits exactly where the CALL sat -- so the inner pass's recursion depth and the caller's
    // share one [`MAX_DEPTH`] budget and the in-use stack never exceeds a single pass's
    // measured bound (see `MAX_DEPTH`'s doc: the cap exists for stack, not semantics). Below
    // the cap the depth never enters the text, so a helper's rendering is position-independent
    // for every input a parser will hand this code; at the cap it degrades to `<max-depth:...>`
    // exactly like any other too-deep subtree.
    let statements = match body {
        FunctionLike::Arrow(arrow) => match &arrow.body {
            ArrowFunctionBody::FunctionBody(function_body) => {
                serializer.ser_stmts(&function_body.statements, depth + 1)
            }
            other => match other.as_expression() {
                Some(expression) => serializer.ser_expr(expression, depth + 1),
                None => serializer.opaque(other.span()),
            },
        },
        FunctionLike::Function(function) => match &function.body {
            Some(function_body) => serializer.ser_stmts(&function_body.statements, depth + 1),
            None => "(no-body)".to_owned(),
        },
    };
    // Every argument no parameter read consumed still renders, in an `(args ...)` tail: an
    // unread argument is invisible in the body's text yet observable at runtime through
    // `arguments[i]`, so two calls differing only in an unread argument must not render alike
    // (review-found false merge; `arguments` itself is not a parameter and never substitutes).
    let unread: Vec<String> = body
        .params()
        .items
        .iter()
        .enumerate()
        .filter(|(index, parameter)| {
            let BindingPattern::BindingIdentifier(binding) = &parameter.pattern else {
                return false;
            };
            binding
                .symbol_id
                .get()
                .is_some_and(|symbol| !serializer.ctx.consumed_args.contains(&symbol))
                && argument_fragments.get(*index).is_some()
        })
        .filter_map(|(index, _)| argument_fragments.get(index).cloned())
        .collect();
    let text = if unread.is_empty() {
        format!("(helper-inline {asyncness}{generator}{form} {statements})")
    } else {
        format!(
            "(helper-inline {asyncness}{generator}{form} {statements} (args {}))",
            unread.join(" ")
        )
    };
    HelperRendering {
        text,
        class_unresolved: serializer.ctx.class_uncertain.get(),
    }
}

/// The three normalized **texts** one body serializes to, which is what this module actually
/// produces.
///
/// The IR does not retain these: [`BodyFingerprint::digest`] reduces them to the fixed-width
/// [`Fingerprints`] `crate::model` stores, and the texts are dropped at the call site. Keeping
/// them apart is what makes that possible -- for a long time `fingerprint` returned
/// `Fingerprints` directly, so the only way to compare two bodies was to keep every byte of
/// every body's serialization alive for the whole run (~3.5 KB per view, 33k bodies, 220 MB).
///
/// Two consumers still want a text and are served from here rather than from the IR: the three
/// IR fields that store an alpha fingerprint *as text* because it is published inside
/// `analysis`'s `contextKey` ([`crate::model::Hook::alpha_source`],
/// [`crate::model::MockFactory::Factory`], [`crate::model::FixtureValue::Setup`] -- see
/// `alpha_source`'s doc for why), which take it through
/// [`BodyFingerprint::into_alpha_source`]; and this module's own tests, whose subject is exactly
/// which node shape serializes to which text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BodyFingerprint {
    /// Serialized AST with locals renamed by resolved symbol; literals preserved.
    pub alpha: String,
    /// `alpha` with literal values replaced by their kind.
    pub structural: String,
    /// Ordered behavior events.
    pub behavior: Vec<String>,
    /// Whether the body is empty or a recognized stub.
    pub trivial: bool,
    /// Whether the callback form changes this body's meaning. See [`CallbackForm`].
    pub callback_form: CallbackForm,
    /// Modules whose every reference in the `alpha` pass rendered as an inlined helper body
    /// (see [`FingerprintContext::fully_inlined_imports`]). Consumed by extract.rs to drop
    /// exactly these from `Context::imports`; ignored by [`BodyFingerprint::digest`], which
    /// feeds only the comparison views to the IR.
    pub inlined_imports: Vec<String>,
    /// Whether an owned class dependency cannot be compared safely.
    pub class_unresolved: bool,
    /// Whether the behavior trace was cut short by [`MAX_DEPTH`] -- the collector's
    /// event-dropping guards fired (see `behavior::BehaviorRun::truncated`). A truncated trace is
    /// missing everything past the cap, so two bodies differing only there collected *equal*
    /// events; `extract.rs` folds this into `Context::unresolved` for tests and hook scopes,
    /// which is what keeps that equality from being read as similarity by any behavior-keyed
    /// rule. Deliberately absent from [`BodyFingerprint::digest`], exactly like
    /// `class_unresolved`: it is a fact about whether the view may be *used*, not a view the IR
    /// compares.
    pub behavior_truncated: bool,
}

impl BodyFingerprint {
    /// Reduces these texts to the form the IR retains.
    pub fn digest(&self) -> Fingerprints {
        Fingerprints {
            alpha: BodyDigest::of(&self.alpha),
            structural: BodyDigest::of(&self.structural),
            behavior: BehaviorTrace::new(&self.behavior),
            trivial: self.trivial,
            callback_form: self.callback_form,
        }
    }

    /// The `alpha` text, for the three IR fields that retain it.
    ///
    /// Returned as an `Arc<str>` rather than the `String` it was built in, and the conversion is
    /// not a formality: this buffer was grown by repeated `push_str`, so it ends the serializing
    /// pass holding roughly twice the bytes it needs, and a retained hook or mock factory would
    /// hold that slack for the whole run (~34 MB of pure doubling overhead on a 24k-test corpus).
    /// `Arc<str>` allocates for the exact length, so the copy that shares the text also sheds the
    /// slack. Every other text this struct carries is dropped by its caller, so this is the only
    /// place either problem outlives the pass.
    pub fn into_alpha_source(self) -> std::sync::Arc<str> {
        std::sync::Arc::from(self.alpha.as_str())
    }
}

/// Unwinds a static member chain down to the call it hangs off, appending each property name to
/// `links` from the outside in, and returns that call.
///
/// The one traversal both assertion recognizers need, and the only place either of them decides
/// what a chain is *made of*: static member links, transparently unwrapping parentheses, ending
/// at a call. What the links are allowed to *say* is the callers' business and is where they
/// differ -- [`BehaviorCollector::assertion_event`] admits only `not`/`resolves`/`rejects`,
/// [`BehaviorCollector::property_chain_event`] admits any word. `None` when the chain bottoms out
/// in anything but a call (a bare identifier, a computed member, a literal) or when it exceeds
/// [`MAX_ASSERTION_CHAIN`] links, and in the latter case it declines outright rather than
/// returning a truncated prefix: a caller handed the longest chain it is willing to read would
/// report an assertion the source never wrote.
fn unwind_member_chain<'a, 'x>(
    start: &'x Expression<'a>,
    links: &mut Vec<&'x str>,
) -> Option<&'x CallExpression<'a>> {
    let mut current = start;
    loop {
        if links.len() > MAX_ASSERTION_CHAIN {
            return None;
        }
        match current {
            Expression::CallExpression(call) => return Some(call),
            Expression::ParenthesizedExpression(parenthesized) => {
                current = &parenthesized.expression
            }
            Expression::StaticMemberExpression(member) => {
                links.push(member.property.name.as_str());
                current = &member.object;
            }
            _ => return None,
        }
    }
}
