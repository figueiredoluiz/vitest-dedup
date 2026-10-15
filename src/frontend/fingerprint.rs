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
//!       [`resolver::resolve`] (see `resolver.rs`) to answer "what file does this specifier
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
//!       [`normalize_relative_specifier`], unchanged from the scheme above) but the whole string
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
//! are transparently unwrapped everywhere because they carry no runtime identity, but destructuring
//! *assignment* targets (not declarations), decorators, class bodies, and most TS-only declaration
//! forms are rendered as opaque raw text. JSX is the one construct that outgrew that treatment and
//! is serialized structurally (`ser_jsx_element`) -- component tests are too large a share of real
//! suites for the whitespace sensitivity of a raw slice to be an acceptable under-match, and an
//! attribute literal hidden inside one is a value this module's `structural` view needs to see.
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
//! and [`normalize_type_argument_text`] for how formatting is kept from mattering.
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
    PrivateFieldExpression, PropertyKey, SimpleAssignmentTarget, Statement, StaticMemberExpression,
    SwitchCase, SwitchStatement, TSInstantiationExpression, TSTypeParameterInstantiation,
    TaggedTemplateExpression, TemplateLiteral, TryStatement, VariableDeclaration,
    VariableDeclarationKind, VariableDeclarator, WhileStatement,
};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::Semantic;
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::operator::UnaryOperator;
use oxc_syntax::reference::ReferenceId;
use oxc_syntax::scope::ScopeFlags;
use oxc_syntax::symbol::SymbolId;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

/// Everything the compared fingerprints need to know about one import binding, keyed by local
/// binding name. `component`/`exported` are what [`free_identity`] keys identity on (see the
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
    /// [`super::resolver::Resolved::Unresolved`], and with it no helper call through this import
    /// can ever be inlined -- an unverifiable body must stay an identity, never a guess.
    pub(super) resolved: Option<PathBuf>,
}

/// A file's top-level imports, keyed by local binding name. See [`import_origins`].
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
/// assumed, by [`tests::ser_expr_at_max_depth_stays_within_its_stack_budget`] against
/// [`MAX_DEPTH_STACK_BUDGET_BYTES`].
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
/// file's text (not just this body's), which is what makes [`import_origins`] possible. `file` is
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
    let behavior = BehaviorCollector::new(context.fresh_for_pass_with(&dataflow)).run(&body);
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
    }
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
) -> String {
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
    let context = FingerprintContext {
        semantic,
        source,
        imports,
        file_key: path_key(file),
        file: file.to_path_buf(),
        body_span: body.span(),
        row_substitution: &no_rows,
        dataflow: &no_dataflow,
        arg_substitution,
        consumed_args: HashSet::new(),
        inline_imported_helpers: false,
        // The helper's own rendering records nothing: its accounting dies with the pass, and no
        // module of the *importing* file is inlined-or-identity-rendered by the helper's body.
        inlined_imports: HashSet::new(),
        identity_imports: HashSet::new(),
        local_prefix: "H",
        canonical: HashMap::new(),
        next_local: 0,
    };
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
    if unread.is_empty() {
        format!("(helper-inline {asyncness}{generator}{form} {statements})")
    } else {
        format!(
            "(helper-inline {asyncness}{generator}{form} {statements} (args {}))",
            unread.join(" ")
        )
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

/// A `.each`/`.for` row's cell values, described purely in terms of *which parameter they bind
/// to* -- never a `SymbolId`, since `extract.rs` (where this is built) does not itself walk the
/// callback body and has no occasion to resolve one. [`resolve_row_substitution`] is what turns
/// this into the `SymbolId`-keyed map [`Serializer`]/[`BehaviorCollector`] actually consult. See
/// the module doc's "Row substitution" section for the full contract.
pub(super) enum RowSubstitution {
    /// Cells bind positionally to the callback's own top-level parameters, left-to-right
    /// (`test.each([[2, 3]])('...', (a, b) => ...)`). `None` at an index means this row supplies
    /// no cell for that parameter -- a trailing `TestContext` parameter beyond the row's own
    /// arity, or an elided array element (`[1, , 3]`, statically known to be `undefined` but not
    /// modeled as a substitutable value) -- and that parameter is left as an ordinary local,
    /// exactly as if no substitution were in effect for it.
    Positional(Vec<Option<(&'static str, String)>>),
    /// Cells bind by field name into the callback's single destructured object parameter
    /// (`test.each([{a: 1, b: 2}])('...', ({a, b}) => ...)`). A destructured property with no
    /// entry here is left as an ordinary local, the same as a `Positional` gap.
    Named(Vec<(String, (&'static str, String))>),
}

/// Matches [`RowSubstitution`]'s row cells against `params`' own parameter shape, producing the
/// `SymbolId -> (kind, raw)` map [`Serializer`]/[`BehaviorCollector`] substitute from -- or `None`
/// ("substitute nothing at all for this row") for any shape this cannot vouch for with full
/// confidence.
///
/// Deliberately all-or-nothing, not best-effort: a rest parameter or a parameter default makes the
/// row-cell-to-parameter arity mapping ambiguous; a positional cell whose parameter is itself a
/// destructuring pattern (`([a, b]) => ...` bound from a `Positional` row) cannot be substituted
/// into safely (substitution only ever replaces a whole `BindingIdentifier`'s read occurrences,
/// never rewrites a pattern's own sub-bindings); a `Named` row's callback that is not a single
/// plain-property object-destructure (a plain, non-destructured single parameter would need
/// member-expression substitution -- `row.a` -- which this module does not attempt; a rest or
/// nested property is the same "cannot vouch for this shape" case as the positional one). Any of
/// these returns `None` outright rather than substituting whichever parameters *do* have a simple
/// shape, because a parameter left as an ordinary local *right next to* others already replaced by
/// literals loses exactly the differentiation an entirely unsubstituted body still has -- see the
/// module doc's "Row substitution" section for why that is a soundness risk, not merely a missed
/// opportunity.
///
/// A parameter with no corresponding cell at all is not one of these bail-out cases: it is simply
/// left unresolved in the returned map, precisely mirroring today's pre-substitution behavior for
/// it (see [`RowSubstitution`]'s own doc on `None`/gap entries).
///
/// Neither is a parameter the body *writes* -- but a written parameter that has a cell is. A
/// written parameter no longer holds the cell it bound, so substituting its later reads would
/// render the cell where the body actually observes the write: `(x) => { x = 5; expect(x)
/// .toBe(6) }` bound from `[6]` would fingerprint `expect(6).toBe(6)` while Vitest evaluates
/// `expect(5).toBe(6)`. That is the false-merge class helper inlining declines with its
/// written-parameter refusal (`helpers.rs`), so the refusal is mirrored here, all-or-nothing
/// like the shape cases above. The check covers exactly the parameters that would enter the map:
/// a written parameter with no cell stays an ordinary local, which is all it ever was.
fn resolve_row_substitution(
    params: &FormalParameters<'_>,
    substitution: &RowSubstitution,
    semantic: &Semantic<'_>,
) -> Option<HashMap<SymbolId, (&'static str, String)>> {
    if params.rest.is_some() || params.items.iter().any(|item| item.initializer.is_some()) {
        return None;
    }
    // The one place a parameter enters the map: its symbol must exist, and a symbol the body
    // writes is refused for the whole row (see the doc above), so both arms share the guard.
    /// Bind an unmutated parameter symbol to its row-cell kind and raw value; decline missing
    /// or written symbols.
    fn bind(
        resolved: &mut HashMap<SymbolId, (&'static str, String)>,
        semantic: &Semantic<'_>,
        binding: &BindingIdentifier<'_>,
        cell: (&'static str, &str),
    ) -> Option<()> {
        let symbol_id = binding.symbol_id.get()?;
        if semantic.scoping().symbol_is_mutated(symbol_id) {
            return None;
        }
        resolved.insert(symbol_id, (cell.0, cell.1.to_owned()));
        Some(())
    }
    let mut resolved = HashMap::new();
    match substitution {
        RowSubstitution::Positional(cells) => {
            for (item, cell) in params.items.iter().zip(cells.iter()) {
                let Some((kind, raw)) = cell else {
                    continue;
                };
                let BindingPattern::BindingIdentifier(binding) = &item.pattern else {
                    return None;
                };
                bind(&mut resolved, semantic, binding, (*kind, raw))?;
            }
        }
        RowSubstitution::Named(fields) => {
            let [item] = params.items.as_slice() else {
                return None;
            };
            let BindingPattern::ObjectPattern(object) = &item.pattern else {
                return None;
            };
            if object.rest.is_some() {
                return None;
            }
            for property in &object.properties {
                let name = property_key_name(&property.key)?;
                let BindingPattern::BindingIdentifier(binding) = &property.value else {
                    return None;
                };
                let Some((_, (kind, raw))) =
                    fields.iter().find(|(field_name, _)| *field_name == name)
                else {
                    // The row's own object has no field by this name -- nothing to substitute
                    // for this destructured property, not a bail; see the module doc.
                    continue;
                };
                bind(&mut resolved, semantic, binding, (*kind, raw))?;
            }
        }
    }
    Some(resolved)
}

// ---------------------------------------------------------------------------------------------
// Dataflow canonicalization
// ---------------------------------------------------------------------------------------------
//
// A canonical dataflow view of a body, consulted only by [`BehaviorCollector::shape`] when
// rendering an assertion's subject/expected-value shape for a `behavior` event -- never by the
// top-level `alpha`/`structural` passes (see [`fingerprint`], which hands those an empty map).
// This is deliberate, not an oversight: `duplicate-test` and `parameterization-candidate`
// key on `alpha`/`structural` equality and report at `Error`/`Warning` severity as an established
// claim, while `near-duplicate-test` (the only rule that reads `behavior`) already reports at
// `similar-uncertain` -- a suppressible, human-reviewed prompt, and it reports that verdict
// precisely because each transformation's precondition is *necessary but not sufficient* for true
// equivalence: the rule detects that two bodies do the same work differently, it does not
// establish that they are interchangeable. Feeding this same canonicalization into
// `alpha`/`structural` would let an unverified dataflow equivalence silently upgrade into a
// claimed, Error-severity one, which is the mistake that verdict exists to avoid.
//
// Two transformations are implemented, each gated on a syntactic, cheap-to-check precondition.
// That is a hard constraint, not a convenience: a normalisation whose precondition cannot be
// checked this cheaply is not performed at all, because an unchecked precondition means the
// equivalence is assumed rather than verified.
//
// - **Single-use local inlining**: `const x = f(); use(x)` renders `x`'s one read as `f()`
//   itself. Gated on the local being `const`, declared and read at this body's own top level (no
//   branch/loop/closure nesting -- dataflow across control flow is a different, harder problem
//   this does not attempt), read **exactly once**, and no side-effecting statement
//   ([`statement_has_effect`]) between the declaration and that read. The read-count-one gate is
//   deliberate, not incidental: a local read twice may exist precisely so a side-effecting
//   initializer is evaluated once instead of twice
//   (`fixtures/duplicates/near-extracted-local-evaluation-twin.test.ts`), and this module has no
//   cheap way to tell a pure initializer from an impure one, so it never assumes purity -- it
//   sidesteps the question entirely by only ever duplicating an initializer that is read once to
//   begin with.
//
//   The declaration site emits nothing for such a local (`BehaviorCollector::walk_stmt` skips it),
//   so the read site owes the initializer's full accounting -- both halves of it. The rendered
//   half ([`Serializer::reference_or_literal`], reached via [`BehaviorCollector::shape`]) covers a
//   read standing in an assertion's subject or expected value; the *event* half
//   ([`BehaviorCollector::walk_inlined_read`]) covers a read standing anywhere else. Only the
//   first existed at first, which silently erased the initializer's `call:` events from every
//   non-assertion read position and produced a proved `near-duplicate-test` false positive; see
//   `walk_inlined_read`'s doc for why the missing half belongs at the read and not back at the
//   declaration.
// - **Destructuring to member access**: `const { a } = o; use(a)` renders `a`'s read as `o.a`.
//   Gated on `o` being a plain identifier that resolves to a symbol **outside** this body (an
//   import, a module-level or outer free variable, or a true global -- never a body-local, since a
//   local's canonical `L`-number depends on first-encounter order during the very traversal this
//   substitution feeds into, and getting that interleaving right is not a cheap check); the
//   destructured property using a plain, non-computed key with no default
//   (`BindingPattern::BindingIdentifier`, never `AssignmentPattern`); and no side-effecting
//   statement between the declaration and a given read (checked per read, but see
//   `try_register_members`'s doc for why checking only the *last* read's range suffices). Multiple
//   reads are allowed here, unlike single-use inlining: duplicating a property *read* (not a call)
//   has no evaluation-count hazard of the kind the other transformation guards against.
//
// A third equivalence-preserving transformation -- `await f()` vs. `return f().then(cb)` --
// is not a value substitution at all (it restructures which statements count as
// directly-executed behavior) and is implemented separately, in
// [`BehaviorCollector::then_continuation`] and its call sites in `walk_stmt`/`run`.
//
// The same map carries one entry kind that is **not** a canonicalization, because it is keyed and
// consulted in exactly the same place and threading a second map to the same call sites would buy
// nothing:
//
// - **Destructured binding names** ([`DataflowSubst::DestructuredProperty`]): a read of a local
//   bound by an object-destructuring *declaration* renders as `id:L0:prop:isEveryTag` rather than
//   a bare `id:L0` -- carrying the whole property path, `:`-joined, for a nested pattern
//   (`const { user: { id } } = t()` records `user:id`). Registered by
//   [`collect_destructured_names`] for every such binding this body declares, and only where
//   neither transformation above already claimed the symbol.
//
//   This exists because the property name was otherwise erased from `behavior` entirely.
//   `const { isEveryTag } = useThing()` and `const { isSameAsInitial } = useThing()` differ in
//   `alpha` and in `structural` (both serialize the binding pattern, which names the property)
//   but produced the byte-identical behavior multiset
//   `["call:...useThing", "assert:expect.toBe(subject=(member id:L0 .value); ...)"]`, because the
//   binding is declared inside the body and alpha-renaming collapses it to a positional
//   `L`-number. That is a `near-duplicate-test`/`subsumed-test` false positive on two tests that
//   read different things. "Destructuring to member access" does not cover it: that transform
//   requires the destructured object to be a plain identifier resolving outside the body, and
//   `useThing()` is a call, so the gate declines and the name is dropped rather than relocated
//   into an `object.property` rendering.
//
//   The precedence is what keeps the member-access equivalence intact: a symbol the
//   `Member` transform did claim never gets a `Name` entry, so `const { a } = o; use(a)` still
//   renders `o.a` and still matches a body that wrote `use(o.a)`
//   (`fixtures/duplicates/near-destructure-vs-member-positive.test.ts`).
//
//   Unlike the two transformations, this one is collected over the **whole body**, not just its
//   top-level statement list, and carries no read-safety, effect, or read-count precondition.
//   Those preconditions all exist to establish that one expression may stand in for another;
//   nothing stands in for anything here. Adding a suffix to a local's own rendering is
//   monotone -- it can only tell two previously-equal event strings apart, never make two
//   different ones equal (an undecorated rendering is `id:L{n}`, `id:global:x` or `id:free:...`,
//   none of which a decorated `id:L{n}:prop:{key}` can spell) -- so its failure mode is a missed
//   near-duplicate, never a claimed one. Rejected alternative: emitting a separate
//   `destructure:(a,b)` event at the declaration. It reads more naturally but changes the event
//   multiset's *arity*, which moves bodies across
//   `MIN_BEHAVIOR_EVENTS_FOR_NEAR_DUPLICATE`'s substance floor and changes `subsumed-test`'s
//   subsequence lengths -- both directions, including *new* findings. A rendering change cannot
//   do either.

/// A single-body dataflow canonicalization, computed once per body by [`analyze_dataflow`] and
/// consulted by [`Serializer::reference_or_literal`] (only when the `Serializer` was constructed
/// with a non-empty map -- see the section doc above). `'b` is the lifetime of the borrow of the
/// body being fingerprinted (matching [`FunctionLike`]'s own second parameter); `'a` is the arena
/// lifetime.
#[derive(Clone)]
enum DataflowSubst<'a, 'b> {
    /// Substitute a local's one read with its own initializer expression. See the section doc's
    /// "Single-use local inlining".
    Inline {
        init: &'b Expression<'a>,
        declaration_index: usize,
        read_index: usize,
    },
    /// Substitute a destructured local's read with `object.property`. See the section doc's
    /// "Destructuring to member access".
    Member {
        object: &'b Expression<'a>,
        property: String,
    },
    /// Tag a destructured local's read with the property key it was bound from. See the section
    /// doc's "Destructured binding names". Unlike the two above this is not a substitution at
    /// all -- nothing is rewritten into anything else, the local keeps its own canonical
    /// `L`-number and only gains a suffix -- so it carries none of their preconditions.
    DestructuredProperty { property: String },
}

/// Computes this body's [`DataflowSubst`] map. The two *canonicalizations* are scoped entirely to
/// the body's own top-level statement list -- an arrow's expression body (no statement list at
/// all) or a function with no body registers none of them, and nothing here ever descends into a
/// branch, loop, try, switch, or nested closure for them. That scope is deliberate: every
/// precondition below is a flat, linear scan over sibling statements, which stops being a cheap,
/// obviously-correct check the moment control flow enters the picture.
///
/// [`DataflowSubst::DestructuredProperty`] entries are collected separately and over the whole
/// body, because they substitute nothing and so share none of those preconditions -- see the
/// section doc's "Destructured binding names". They are registered last, and only into vacant
/// slots, so a symbol either canonicalization claimed keeps that claim.
fn analyze_dataflow<'a, 'b>(
    body: &FunctionLike<'a, 'b>,
    semantic: &Semantic<'a>,
) -> HashMap<SymbolId, DataflowSubst<'a, 'b>> {
    let mut result = HashMap::new();
    let body_span = body.span();
    let destructured_names = collect_destructured_names(body);
    // `match *body` (rather than `match body`) deliberately avoids match ergonomics binding
    // `arrow`/`function` as a double reference (`&'_ &'b ...`): the variant's own field is already
    // `&'b ...`, a `Copy` type, so matching the dereferenced enum copies that reference out
    // directly at its own natural `'b`, which is what every helper below needs to name in its
    // signature.
    // An expression-bodied arrow or a bodyless function has no statement list for either
    // canonicalization to scan; it yields the empty slice rather than an early return, because
    // the `Name` entries below are collected over the whole body and still apply (a destructuring
    // declaration can sit inside a nested closure of an expression-bodied arrow).
    let statements: &'b [Statement<'a>] = match *body {
        FunctionLike::Arrow(arrow) => match &arrow.body {
            ArrowFunctionBody::FunctionBody(function_body) => &function_body.statements,
            _ => &[],
        },
        FunctionLike::Function(function) => match &function.body {
            Some(function_body) => &function_body.statements,
            None => &[],
        },
    };
    for (index, statement) in statements.iter().enumerate() {
        let Statement::VariableDeclaration(declaration) = statement else {
            continue;
        };
        if declaration.kind != VariableDeclarationKind::Const {
            continue;
        }
        let [declarator] = declaration.declarations.as_slice() else {
            continue;
        };
        let Some(init) = &declarator.init else {
            continue;
        };
        match &declarator.id {
            BindingPattern::BindingIdentifier(binding) => {
                try_register_inline(&mut result, semantic, statements, index, binding, init);
            }
            BindingPattern::ObjectPattern(object) => {
                try_register_members(
                    &mut result,
                    semantic,
                    body_span,
                    statements,
                    index,
                    object,
                    init,
                );
            }
            _ => {}
        }
    }
    // Do not recursively absorb an effectful inlined declaration through another. Keeping the
    // inner initializer as its own behavior event lets the near-duplicate multiset compose extraction
    // with setup reordering: `const a = f(); effect(); expect(a.x)` and
    // `effect(); const a = f(); const x = a.x; expect(x)` both retain `call:f` plus the same
    // assertion shape. Fully chaining the second body folds `f()` into the assertion while the
    // first body's effect correctly blocks that movement, so a normalization meant to bridge
    // extraction defeats the independent reordering normalization.
    //
    // This is exact, not a hash heuristic: each candidate already has exactly one resolved read
    // in one flat statement, and that statement either is another inline candidate's declaration
    // or it is not. Pure initializers still chain, preserving value-shape normalization such as
    // `const cart = [10, 20]; const total = compute(cart); expect(total)`.
    let inline_declaration_indices: Vec<usize> = result
        .values()
        .filter_map(|substitution| match substitution {
            DataflowSubst::Inline {
                declaration_index, ..
            } => Some(*declaration_index),
            DataflowSubst::Member { .. } | DataflowSubst::DestructuredProperty { .. } => None,
        })
        .collect();
    let nested_inline_symbols: Vec<SymbolId> = result
        .iter()
        .filter_map(|(&symbol_id, substitution)| {
            let DataflowSubst::Inline {
                init,
                declaration_index,
                read_index,
            } = substitution
            else {
                return None;
            };
            (expression_has_effect(init, 0)
                && inline_declaration_indices.contains(read_index)
                && statements[..*declaration_index]
                    .iter()
                    .any(statement_has_effect))
            .then_some(symbol_id)
        })
        .collect();
    for symbol_id in nested_inline_symbols {
        result.remove(&symbol_id);
    }
    for (symbol_id, property) in destructured_names {
        // `or_insert`, never an overwrite: a symbol "destructuring to member access" already
        // claimed renders as `object.property` and must keep doing so -- that rendering is what
        // makes `const { a } = o; use(a)` match `use(o.a)`.
        result
            .entry(symbol_id)
            .or_insert(DataflowSubst::DestructuredProperty { property });
    }
    result
}

/// Every local this body declares through an object-destructuring pattern, paired with the
/// property key it is bound from -- the input to [`DataflowSubst::DestructuredProperty`] (see the
/// section doc's "Destructured binding names" for why these are retained at all).
///
/// Walks the whole body, nested closures and control flow included, because a name suffix has no
/// precondition to establish (unlike the two canonicalizations, which are confined to the
/// top-level statement list). Deliberate omissions, each because there is no property key to
/// retain rather than because retaining one would be unsafe:
///
/// * **Parameters.** Only [`VariableDeclarator`] patterns are collected, so a destructured
///   callback/context parameter (`test('x', ({ expect }) => ...)`) is untouched. A `test.each`
///   row binds its cells into exactly such a parameter, and [`RowSubstitution`] already renders
///   those reads as the row's own literal value; leaving parameters alone keeps this change out
///   of that machinery entirely.
/// * **Array patterns** contribute position, not a name -- `const [a] = pair` and
///   `const [b] = pair` already render identically under `alpha` too. An element still inherits
///   the object key it sits under, and a nested object pattern inside one is still collected.
/// * **Rest elements** (`const { ...rest } = o`) name no property, and **computed keys**
///   (`const { [k]: v } = o`) name none statically ([`property_key_name`] returns `None`).
/// * **Quoted keys** (`const { 'is-empty': x } = o`) do name one, and are skipped anyway -- for a
///   reason about the event grammar rather than about naming. See the guard's own comment in
///   [`DestructuredNameScan::collect_pattern`].
fn collect_destructured_names(body: &FunctionLike<'_, '_>) -> Vec<(SymbolId, String)> {
    let mut scan = DestructuredNameScan { names: Vec::new() };
    match *body {
        FunctionLike::Arrow(arrow) => match &arrow.body {
            ArrowFunctionBody::FunctionBody(function_body) => {
                for statement in &function_body.statements {
                    scan.visit_statement(statement);
                }
            }
            other => {
                if let Some(expression) = other.as_expression() {
                    scan.visit_expression(expression);
                }
            }
        },
        FunctionLike::Function(function) => {
            if let Some(function_body) = &function.body {
                for statement in &function_body.statements {
                    scan.visit_statement(statement);
                }
            }
        }
    }
    scan.names
}

/// The [`Visit`] implementation behind [`collect_destructured_names`]. Driven over the body's
/// statements only (never the function node itself), so a destructured *parameter* of the body is
/// never reached; `visit_variable_declarator` is the single override, and the generated walk
/// finds declarators wherever they sit, including inside a nested closure or a `for`-`of` head.
struct DestructuredNameScan {
    names: Vec<(SymbolId, String)>,
}

impl<'a> Visit<'a> for DestructuredNameScan {
    /// Collect destructured local names and continue into the variable declarator.
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        self.collect_pattern(&it.id, "", 0);
        walk::walk_variable_declarator(self, it);
    }
}

impl DestructuredNameScan {
    /// Collects the `(symbol, property path)` pairs a binding pattern declares. `path` is the
    /// `:`-joined sequence of property keys walked through to reach this sub-pattern, empty at the
    /// declarator's own root; a binding reached with an empty path was not destructured out of
    /// anything and contributes nothing.
    ///
    /// The path is the WHOLE route, not just the innermost key: `const { user: { id } } = t()`
    /// and `const { post: { id } } = t()` both bind one local whose own key is `id`, so keeping
    /// only that key left the two bodies with the identical behavior event -- the same false
    /// positive one nesting level down. They now record `user:id` and `post:id`.
    ///
    /// `:` joins the segments rather than `.` because the result is spliced into a call path,
    /// where `.` is the member separator: with a dotted join, `const { api: { get } } = t(); get()`
    /// would render `call:L0:prop:api.get`, which is exactly what
    /// `const { api } = t(); api.get()` renders, and two different bodies would share an event.
    /// `:` cannot appear in a JS identifier, and [`collect_pattern`] retains identifier-written
    /// keys only, so the joined path stays unambiguously decomposable.
    ///
    /// `depth` bounds the recursion the same way [`MAX_DEPTH`] bounds every other recursive walk
    /// in this module: a pathologically nested pattern stops contributing names rather than
    /// overflowing the stack, which costs discrimination and nothing else.
    fn collect_pattern(&mut self, pattern: &BindingPattern<'_>, path: &str, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        match pattern {
            BindingPattern::BindingIdentifier(binding) => {
                if path.is_empty() {
                    return; // `const x = o()`, or an element of `const [x] = o()`: no key.
                }
                if let Some(symbol_id) = binding.symbol_id.get() {
                    self.names.push((symbol_id, path.to_owned()));
                }
            }
            BindingPattern::ObjectPattern(object) => {
                for property in &object.properties {
                    if property.computed {
                        continue;
                    }
                    // Only a key WRITTEN as an identifier is retained, never a quoted one, even
                    // though `property_key_name` resolves both. The retained path is appended to
                    // an event raw -- `id:L0:prop:{path}`, `call:L0:prop:{path}(...)` -- and a
                    // quoted key is arbitrary author-controlled text, so it can spell the event
                    // grammar's own punctuation and make one event imitate another. Constructed
                    // and confirmed before this guard existed: `const { x } = o(); x(1)` and
                    // `const { 'x(<number:1>)': f } = o(); f()` both emitted
                    // `["call:...o", "call:L0:prop:x(<number:1>)"]` -- two different bodies with
                    // one behavior list. A JS identifier cannot contain `(`, `)`, `;`, `<`, `>`,
                    // `,`, `.`, `:` or a space, and no escape smuggles one in (those bytes are
                    // not `ID_Continue`, so such an identifier does not parse), which is what
                    // makes this a cheap syntactic check rather than an escaping scheme.
                    // Escaping was rejected: `escape_literal_delimiters` covers `\`, `<` and `>`
                    // only, so it would not have closed the case above, and widening it changes
                    // how every literal in every fingerprint renders.
                    //
                    // The cost is discrimination on a quoted key (`const { 'is-empty': x } = o`),
                    // which keeps exactly the pre-existing behavior -- a missed near-duplicate
                    // split, the safe direction. The sibling hazard is NOT introduced here and is
                    // deliberately left alone: `DataflowSubst::Member` renders the same
                    // `property_key_name` result into `(member ... .{property})` with no such
                    // guard, which predates this entry kind.
                    if !matches!(property.key, PropertyKey::StaticIdentifier(_)) {
                        continue;
                    }
                    let Some(name) = property_key_name(&property.key) else {
                        continue;
                    };
                    let child = if path.is_empty() {
                        name
                    } else {
                        format!("{path}:{name}")
                    };
                    // `property.value` covers the default form too (`const { a = 1 } = o` is an
                    // `AssignmentPattern` here), which the arm below unwraps under this same key.
                    self.collect_pattern(&property.value, &child, depth + 1);
                }
            }
            BindingPattern::ArrayPattern(array) => {
                // An element contributes position, not a name, so it inherits the enclosing path
                // unchanged: in `const { pair: [first] } = o`, `first` came from `pair` and that
                // is the whole of what is knowable by name.
                for element in array.elements.iter().flatten() {
                    self.collect_pattern(element, path, depth + 1);
                }
            }
            BindingPattern::AssignmentPattern(assignment) => {
                self.collect_pattern(&assignment.left, path, depth + 1);
            }
        }
    }
}

/// Registers a "single-use local inlining" candidate (see the section doc) for one `const x = ...`
/// declarator, if its one precondition-checkable read qualifies.
fn try_register_inline<'a, 'b>(
    result: &mut HashMap<SymbolId, DataflowSubst<'a, 'b>>,
    semantic: &Semantic<'a>,
    statements: &'b [Statement<'a>],
    decl_index: usize,
    binding: &'b BindingIdentifier<'a>,
    init: &'b Expression<'a>,
) {
    let Some(symbol_id) = binding.symbol_id.get() else {
        return;
    };
    let Some(reads) = read_statement_indices(semantic, statements, symbol_id) else {
        return; // a read outside this flat statement list (a branch, a loop, a closure): bail.
    };
    // Bound-once-read-twice must NOT inline -- see the section doc.
    let [use_index] = reads.as_slice() else {
        return;
    };
    if *use_index <= decl_index {
        return; // should not happen for a `const` (TDZ forbids a read before declaration), but a
                // defensive floor here costs nothing.
    }
    if statements[decl_index + 1..*use_index]
        .iter()
        .any(statement_has_effect)
    {
        return;
    }
    result.insert(
        symbol_id,
        DataflowSubst::Inline {
            init,
            declaration_index: decl_index,
            read_index: *use_index,
        },
    );
}

/// Registers a "destructuring to member access" candidate (see the section doc) for one
/// `const { ... } = o` declarator, for each of its plain-identifier properties that qualifies.
///
/// Checks only the *last* read's range for an intervening effect, not every read's own range
/// individually: a statement's effect, once present between the declaration and some read, is
/// also present between the declaration and every *later* read (the range only grows), so the
/// widest range (declaration to the last read) is the only one that can newly fail the check.
fn try_register_members<'a, 'b>(
    result: &mut HashMap<SymbolId, DataflowSubst<'a, 'b>>,
    semantic: &Semantic<'a>,
    body_span: Span,
    statements: &'b [Statement<'a>],
    decl_index: usize,
    object_pattern: &'b ObjectPattern<'a>,
    init: &'b Expression<'a>,
) {
    let can_rewrite_object = match init {
        Expression::Identifier(object_ident) => object_ident
            .reference_id
            .get()
            .and_then(|reference_id| semantic.scoping().get_reference(reference_id).symbol_id())
            .is_some_and(|symbol_id| !is_local_symbol(semantic, body_span, symbol_id)),
        _ => false,
    };
    for property in &object_pattern.properties {
        if property.computed || !matches!(property.key, PropertyKey::StaticIdentifier(_)) {
            continue;
        }
        let BindingPattern::BindingIdentifier(binding) = &property.value else {
            continue; // a nested pattern or a default (`AssignmentPattern`): not supported.
        };
        let Some(name) = property_key_name(&property.key) else {
            continue;
        };
        let Some(symbol_id) = binding.symbol_id.get() else {
            continue;
        };
        // The fallback is identity-preserving rather than equivalence-producing. Keep it even
        // when a read occurs in control flow or after a mutation: neither fact can change which
        // property the binding names, and retaining that name can only prevent a false match.
        result.insert(
            symbol_id,
            DataflowSubst::DestructuredProperty {
                property: name.clone(),
            },
        );
        if !can_rewrite_object {
            continue;
        }
        let Some(reads) = read_statement_indices(semantic, statements, symbol_id) else {
            continue;
        };
        let Some(&last_use) = reads.iter().max() else {
            continue; // never read: nothing to substitute.
        };
        if last_use <= decl_index {
            continue;
        }
        if statements[decl_index + 1..last_use]
            .iter()
            .any(statement_has_effect)
        {
            continue;
        }
        result.insert(
            symbol_id,
            DataflowSubst::Member {
                object: init,
                property: name,
            },
        );
    }
}

/// The flat top-level statement index containing each safely-reasoned-about **read** of
/// `symbol_id`, or `None` if the symbol is read anywhere this analysis does not consider safe to
/// reason about: inside a branch, loop, switch, try, class body, or nested closure -- any of
/// which make "declared once, read exactly there" unsound, since execution past that point is
/// conditional, repeated, or deferred relative to the declaration's own straight-line statement
/// list. Never inspects declaration sites, only read (`IdentifierReference` with
/// `Reference::is_read()`) uses.
///
/// Deliberately does NOT use `Semantic::reference_span` (which requires the `AstNodes` store built
/// via `SemanticBuilder::with_build_nodes(true)`): `extract.rs` builds this crate's `Semantic`
/// without it (`SemanticBuilder::new().build(...)`), so that store is always empty here, and
/// calling `reference_span` against an empty store panics rather than returning a useless answer.
/// A bounded [`Visit`] walk over `statements` -- which this function already owns, unlike the node
/// store -- finds read positions without needing it: every read is discovered in the same
/// statement-by-statement order this function iterates in, so there is no span comparison to do at
/// all.
fn read_statement_indices<'a>(
    semantic: &Semantic<'a>,
    statements: &[Statement<'a>],
    symbol_id: SymbolId,
) -> Option<Vec<usize>> {
    let mut indices = Vec::new();
    for (index, statement) in statements.iter().enumerate() {
        let mut scan = SymbolReadScan {
            semantic,
            target: symbol_id,
            unsafe_depth: 0,
            direct_reads: 0,
            unsafe_reads: false,
        };
        scan.visit_statement(statement);
        if scan.unsafe_reads {
            return None;
        }
        for _ in 0..scan.direct_reads {
            indices.push(index);
        }
    }
    Some(indices)
}

/// The [`Visit`] implementation behind [`read_statement_indices`]. `unsafe_depth` counts how many
/// branch/loop/switch/try/class/closure boundaries the walk is currently inside; a read of
/// `target` found while it is nonzero sets `unsafe_reads` instead of counting toward
/// `direct_reads`. Relies on `Visit`'s own generated traversal for every node kind this does not
/// explicitly override -- unlike a hand-matched traversal (the style [`BehaviorCollector`] and
/// [`Serializer`] use elsewhere in this module for building fingerprint *content*, where every
/// node kind's rendering is meaningful and worth naming), there is nothing this scan needs to
/// treat specially about, say, a template literal's interpolations or a tagged template's tag: an
/// identifier reference is an identifier reference regardless of what expression shape contains
/// it, so leaning on the generated exhaustive walk is the right tool for "is `target` read
/// anywhere in here, and is that reachable unconditionally" specifically.
struct SymbolReadScan<'s, 'a> {
    semantic: &'s Semantic<'a>,
    target: SymbolId,
    unsafe_depth: u32,
    direct_reads: usize,
    unsafe_reads: bool,
}

impl<'a> Visit<'a> for SymbolReadScan<'_, 'a> {
    /// Count reads of the target symbol, distinguishing direct reads from reads under unsafe
    /// nesting.
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        let is_target = it.reference_id.get().is_some_and(|reference_id| {
            let reference = self.semantic.scoping().get_reference(reference_id);
            reference.is_read() && reference.symbol_id() == Some(self.target)
        });
        if is_target {
            if self.unsafe_depth > 0 {
                self.unsafe_reads = true;
            } else {
                self.direct_reads += 1;
            }
        }
    }

    /// Treat symbol reads inside if statement nesting as unsafe for single-use inlining.
    fn visit_if_statement(&mut self, it: &IfStatement<'a>) {
        self.unsafe_depth += 1;
        walk::walk_if_statement(self, it);
        self.unsafe_depth -= 1;
    }

    /// Treat symbol reads inside for statement nesting as unsafe for single-use inlining.
    fn visit_for_statement(&mut self, it: &ForStatement<'a>) {
        self.unsafe_depth += 1;
        walk::walk_for_statement(self, it);
        self.unsafe_depth -= 1;
    }

    /// Treat symbol reads inside for in statement nesting as unsafe for single-use inlining.
    fn visit_for_in_statement(&mut self, it: &ForInStatement<'a>) {
        self.unsafe_depth += 1;
        walk::walk_for_in_statement(self, it);
        self.unsafe_depth -= 1;
    }

    /// Treat symbol reads inside for of statement nesting as unsafe for single-use inlining.
    fn visit_for_of_statement(&mut self, it: &ForOfStatement<'a>) {
        self.unsafe_depth += 1;
        walk::walk_for_of_statement(self, it);
        self.unsafe_depth -= 1;
    }

    /// Treat symbol reads inside while statement nesting as unsafe for single-use inlining.
    fn visit_while_statement(&mut self, it: &WhileStatement<'a>) {
        self.unsafe_depth += 1;
        walk::walk_while_statement(self, it);
        self.unsafe_depth -= 1;
    }

    /// Treat symbol reads inside do while statement nesting as unsafe for single-use inlining.
    fn visit_do_while_statement(&mut self, it: &DoWhileStatement<'a>) {
        self.unsafe_depth += 1;
        walk::walk_do_while_statement(self, it);
        self.unsafe_depth -= 1;
    }

    /// Treat symbol reads inside switch statement nesting as unsafe for single-use inlining.
    fn visit_switch_statement(&mut self, it: &SwitchStatement<'a>) {
        self.unsafe_depth += 1;
        walk::walk_switch_statement(self, it);
        self.unsafe_depth -= 1;
    }

    /// Treat symbol reads inside try statement nesting as unsafe for single-use inlining.
    fn visit_try_statement(&mut self, it: &TryStatement<'a>) {
        self.unsafe_depth += 1;
        walk::walk_try_statement(self, it);
        self.unsafe_depth -= 1;
    }

    /// Treat symbol reads inside class nesting as unsafe for single-use inlining.
    fn visit_class(&mut self, it: &Class<'a>) {
        self.unsafe_depth += 1;
        walk::walk_class(self, it);
        self.unsafe_depth -= 1;
    }

    /// Treat symbol reads inside function nesting as unsafe for single-use inlining.
    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        self.unsafe_depth += 1;
        walk::walk_function(self, it, flags);
        self.unsafe_depth -= 1;
    }

    /// Treat symbol reads inside arrow function expression nesting as unsafe for single-use
    /// inlining.
    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        self.unsafe_depth += 1;
        walk::walk_arrow_function_expression(self, it);
        self.unsafe_depth -= 1;
    }
}

/// Whether a statement could observably change program state -- a cheap, deliberately broad
/// over-approximation (an allowlist of known-safe shapes, not a precise effect analysis), used as
/// the "no intervening effect" precondition both dataflow transformations share. It fails closed
/// by construction: a statement this does not specifically clear is treated as effectful, never
/// the reverse, because an unverified precondition must never be reported as satisfied.
fn statement_has_effect(statement: &Statement<'_>) -> bool {
    statement_has_effect_at(statement, 0)
}

/// Conservatively detect statement effects, treating depth exhaustion as effectful.
fn statement_has_effect_at(statement: &Statement<'_>, depth: u32) -> bool {
    if depth > MAX_DEPTH {
        return true; // degrade toward "has an effect" (blocks canonicalization) rather than loop.
    }
    match statement {
        Statement::EmptyStatement(_) => false,
        Statement::BlockStatement(block) => block
            .body
            .iter()
            .any(|inner| statement_has_effect_at(inner, depth + 1)),
        Statement::VariableDeclaration(declaration) => declaration.declarations.iter().any(|d| {
            d.init
                .as_ref()
                .is_some_and(|init| expression_has_effect(init, depth + 1))
        }),
        Statement::ExpressionStatement(expression_statement) => {
            expression_has_effect(&expression_statement.expression, depth + 1)
        }
        // Control flow, `return`/`throw`, and any declaration form not specifically cleared above
        // are conservatively treated as a possible effect: only the shapes explicitly recognized
        // above are known safe.
        _ => true,
    }
}

/// See [`statement_has_effect`]'s doc; the expression-level half of the same allowlist.
fn expression_has_effect(expr: &Expression<'_>, depth: u32) -> bool {
    if depth > MAX_DEPTH {
        return true;
    }
    match expr {
        Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::Identifier(_)
        | Expression::ThisExpression(_)
        | Expression::Super(_) => false,
        Expression::TemplateLiteral(template) => template
            .expressions
            .iter()
            .any(|inner| expression_has_effect(inner, depth + 1)),
        Expression::ArrayExpression(array) => array.elements.iter().any(|element| {
            element
                .as_expression()
                .is_none_or(|inner| expression_has_effect(inner, depth + 1))
        }),
        Expression::ObjectExpression(object) => {
            object.properties.iter().any(|property| match property {
                ObjectPropertyKind::ObjectProperty(object_property) => {
                    expression_has_effect(&object_property.value, depth + 1)
                }
                ObjectPropertyKind::SpreadProperty(_) => true,
            })
        }
        Expression::StaticMemberExpression(member) => {
            expression_has_effect(&member.object, depth + 1)
        }
        Expression::ComputedMemberExpression(member) => {
            expression_has_effect(&member.object, depth + 1)
                || expression_has_effect(&member.expression, depth + 1)
        }
        Expression::PrivateFieldExpression(member) => {
            expression_has_effect(&member.object, depth + 1)
        }
        Expression::BinaryExpression(binary) => {
            expression_has_effect(&binary.left, depth + 1)
                || expression_has_effect(&binary.right, depth + 1)
        }
        Expression::LogicalExpression(logical) => {
            expression_has_effect(&logical.left, depth + 1)
                || expression_has_effect(&logical.right, depth + 1)
        }
        Expression::UnaryExpression(unary) => expression_has_effect(&unary.argument, depth + 1),
        Expression::ConditionalExpression(conditional) => {
            expression_has_effect(&conditional.test, depth + 1)
                || expression_has_effect(&conditional.consequent, depth + 1)
                || expression_has_effect(&conditional.alternate, depth + 1)
        }
        Expression::ParenthesizedExpression(parenthesized) => {
            expression_has_effect(&parenthesized.expression, depth + 1)
        }
        Expression::TSAsExpression(as_expression) => {
            expression_has_effect(&as_expression.expression, depth + 1)
        }
        Expression::TSSatisfiesExpression(satisfies) => {
            expression_has_effect(&satisfies.expression, depth + 1)
        }
        Expression::TSNonNullExpression(non_null) => {
            expression_has_effect(&non_null.expression, depth + 1)
        }
        // Calls, `new`, assignments, updates, `await`, `yield`, sequences, chains, and anything
        // else not specifically cleared above are conservatively treated as effectful.
        _ => true,
    }
}

/// Extracts, from a file's full source text, each top-level import's local binding name mapped to
/// `(identity component, name on the exporting side)`. `file` is the path of the file being
/// fingerprinted -- passed to [`resolver::resolve`] so a relative or aliased specifier resolves
/// against the file that actually wrote it, and used (via its parent directory) to anchor the
/// unresolved fallback -- see [`normalize_relative_specifier`] and the module doc's discussion of
/// why an unanchored fallback is a proved cross-file false-positive.
///
/// The identity component is `resolved:<canonical path>` when [`resolver::resolve`] finds a real
/// file, or `unresolved:<anchored specifier>` when it does not; see the module doc for why the two
/// are prefixed distinctly and can never collide.
///
/// `Semantic` does not retain this once binding is resolved -- only the AST does, via
/// `ImportDeclaration::source` -- and `fingerprint`'s signature is never handed the `Program`,
/// only `body`/`semantic`/`source`/`file`. So this re-parses `source` with its own throwaway
/// `Allocator` to read straight off the top-level `ImportDeclaration`s, which needs no semantic
/// analysis of its own (imports are always top-level, never nested, so a shallow scan of
/// `program.body` is sufficient). Extraction does not use this re-parse: it reads the map off its
/// own real-type parse ([`with_file_import_origins`]), shared by every body fingerprinted from that
/// file. Helper files and callers outside an extraction scope still compute it here.
///
/// `SourceType::tsx()` is used unconditionally as the most permissive superset available (this
/// module is never told the real file extension). A file using the legacy `<T>expr` TypeScript
/// cast syntax -- ambiguous with JSX and therefore rejected in `.tsx` mode -- degrades to an empty
/// map, same as any other parse failure; that only costs import-specifier precision; see
/// `free_identity`'s fallback.
pub(super) fn import_origins(source: &str, file: &Path) -> ImportOrigins {
    if super::extract::parse_refusal(source).is_some() {
        return HashMap::new();
    }
    #[cfg(test)]
    IMPORT_ORIGIN_PARSES.with(|count| count.set(count.get() + 1));
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::tsx()).parse();
    if parsed.fatal_error {
        return HashMap::new();
    }
    import_origins_from_program(&parsed.program, file)
}

/// `program`'s top-level import declarations that bind values: `import type` declarations are
/// erased by TypeScript and never reach runtime, so every import table skips them.
pub(super) fn value_import_declarations<'p, 'a>(
    program: &'p oxc_ast::ast::Program<'a>,
) -> impl Iterator<Item = &'p oxc_ast::ast::ImportDeclaration<'a>> + 'p {
    program.body.iter().filter_map(|statement| match statement {
        Statement::ImportDeclaration(import)
            if !matches!(import.import_kind, oxc_ast::ast::ImportOrExportKind::Type) =>
        {
            Some(&**import)
        }
        _ => None,
    })
}

/// The import map of an already-parsed `program` written at `file`: each top-level, non-type
/// import's local binding name mapped to its origin (see [`import_origins`] for the identity
/// scheme). Needs no semantic analysis, because imports are always top-level.
pub(super) fn import_origins_from_program(
    program: &oxc_ast::ast::Program<'_>,
    file: &Path,
) -> ImportOrigins {
    let mut origins = HashMap::new();
    for import in value_import_declarations(program) {
        let Some(specifiers) = &import.specifiers else {
            continue;
        };
        let module = import.source.value.as_str();
        let origin = specifier_origin(file, module);
        for specifier in specifiers {
            let (local, exported) = match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                    if matches!(
                        specifier.import_kind,
                        oxc_ast::ast::ImportOrExportKind::Type
                    ) {
                        continue;
                    }
                    (&specifier.local, module_export_name(&specifier.imported))
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
                    (&specifier.local, "default".to_owned())
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
                    (&specifier.local, "*".to_owned())
                }
            };
            origins.insert(
                local.name.as_str().to_owned(),
                ImportOrigin {
                    component: origin.component.clone(),
                    exported,
                    specifier: module.to_owned(),
                    resolved: origin.resolved.clone(),
                },
            );
        }
    }
    origins
}

/// Identifies one file's text for [`FILE_IMPORTS`]: the borrowed source's address and length plus
/// the path. The scope that installs an entry borrows that source for its whole lifetime, so the
/// address cannot be reused by other text while the entry is visible.
#[derive(PartialEq, Eq)]
struct FileKey {
    source: usize,
    len: usize,
    path: PathBuf,
}

impl FileKey {
    /// Builds the key for `source` as written at `file`.
    fn of(source: &str, file: &Path) -> Self {
        Self {
            source: source.as_ptr() as usize,
            len: source.len(),
            path: file.to_path_buf(),
        }
    }
}

thread_local! {
    /// The import origins of the file being extracted on this thread, computed once for it by
    /// [`with_file_import_origins`]. Without this, every fingerprinted body re-parsed the whole file
    /// and re-resolved its imports, which made a file's cost quadratic in its test count.
    static FILE_IMPORTS: std::cell::RefCell<Option<(FileKey, std::rc::Rc<ImportOrigins>)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    /// Test-only count of [`import_origins`] parses on this thread.
    static IMPORT_ORIGIN_PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Returns the number of [`import_origins`] parses this thread has performed.
#[cfg(test)]
pub(super) fn import_origin_parses() -> usize {
    IMPORT_ORIGIN_PARSES.with(std::cell::Cell::get)
}

/// Runs `walk` with `file`'s import origins, read once from `program` (the caller's parse of
/// `source`), shared by every body fingerprinted from that same `source` during the walk; the
/// previous state is restored afterwards, including on unwind.
pub(super) fn with_file_import_origins<R>(
    source: &str,
    file: &Path,
    program: &oxc_ast::ast::Program<'_>,
    walk: impl FnOnce() -> R,
) -> R {
    /// Puts back the entry that was visible before this scope.
    struct Restore(Option<(FileKey, std::rc::Rc<ImportOrigins>)>);
    impl Drop for Restore {
        /// Reinstalls the saved entry when the scope ends, normally or by unwinding.
        fn drop(&mut self) {
            let previous = self.0.take();
            FILE_IMPORTS.with(|slot| *slot.borrow_mut() = previous);
        }
    }
    // Read off the caller's own parse, made with the file's real source type: a second TSX parse
    // would reject valid `.ts` syntax (a legacy `<T>value` cast, a generic arrow) and silently
    // empty the map.
    let entry = (
        FileKey::of(source, file),
        std::rc::Rc::new(import_origins_from_program(program, file)),
    );
    let _restore = Restore(FILE_IMPORTS.with(|slot| slot.replace(Some(entry))));
    walk()
}

/// Returns `file`'s import origins: the shared copy when `source` is the file being walked, else a
/// fresh computation (helper files, direct callers outside an extraction scope).
fn file_import_origins(source: &str, file: &Path) -> std::rc::Rc<ImportOrigins> {
    let key = FileKey::of(source, file);
    FILE_IMPORTS
        .with(|slot| {
            slot.borrow()
                .as_ref()
                .filter(|(installed, _)| *installed == key)
                .map(|(_, origins)| std::rc::Rc::clone(origins))
        })
        .unwrap_or_else(|| std::rc::Rc::new(import_origins(source, file)))
}

/// Resolves `specifier`, as written in `file`, to a fingerprint-safe identity component:
/// `resolved:<canonical path>` when [`resolver::resolve`] finds a real file on disk, or
/// `unresolved:<anchored specifier>` when it does not -- the two are prefixed distinctly so a
/// failed resolution can never collide with a successful one, regardless of what path string a
/// resolution could ever produce.
///
/// This is the one place a file-relative specifier is turned into something safe to compare
/// *across* files, and every caller that puts repository text from a `vi.mock`/`import` argument
/// into a cross-file-reaching identity must go through it rather than storing the argument's raw
/// text -- see [`import_origins`] (free-import identity, the original caller) and
/// `extract::collect_mocked_modules` (`MockedModule::specifier`, the same shape of bug found a
/// sixth time: `vi.mock('./dep')` is file-relative exactly like an `import` specifier is, but was
/// being stored as the raw argument text with no anchoring at all, so the same relative text in
/// two different directories named two different modules yet compared equal).
///
/// A bare/package specifier (`vitest`, `@scope/pkg`) that fails to resolve (no `node_modules`, no
/// matching tsconfig `paths` alias) falls back to the same directory-independent text it always
/// used: two files that both fail to resolve the same package name are not proven to differ, so
/// there is nothing to anchor. An absolute-style specifier (a bare leading `/`) is left as-is too,
/// for the same reason. A successful resolution of a bare specifier is naturally file-independent
/// already (it names the same real file -- typically the same installed package -- regardless of
/// which file in the project imports it), so this function needs no special case for "bare" versus
/// "relative" on the `resolved:` path at all; only the `unresolved:` fallback needs to tell them
/// apart, via [`normalize_relative_specifier`].
pub(super) fn specifier_identity(file: &Path, specifier: &str) -> String {
    specifier_origin(file, specifier).component
}

/// [`specifier_identity`], keeping the resolution's by-products the identity string itself
/// discards: the raw specifier and the resolved path, which [`import_origins`] stores on every
/// binding so the inlining path (`try_inline_imported_helper`) can reach the helper's file and
/// refuse the framework by its raw specifier.
fn specifier_origin(file: &Path, specifier: &str) -> ImportOrigin {
    match super::resolver::resolve(file, specifier) {
        super::resolver::Resolved::Module(resolved_path) => ImportOrigin {
            component: format!("resolved:{}", path_key(&resolved_path)),
            exported: String::new(),
            specifier: specifier.to_owned(),
            resolved: Some(resolved_path),
        },
        super::resolver::Resolved::Unresolved => {
            let anchored = if specifier.starts_with('.') {
                let importer_dir = file.parent().unwrap_or_else(|| Path::new(""));
                normalize_relative_specifier(importer_dir, specifier)
            } else {
                specifier.to_owned()
            };
            ImportOrigin {
                component: format!("unresolved:{anchored}"),
                exported: String::new(),
                specifier: specifier.to_owned(),
                resolved: None,
            }
        }
    }
}

/// The text of a `ModuleExportName` -- the `foo` of `import { foo as bar }` in whichever of its
/// three syntactic shapes it was written -- as the exporting side spelled it. Shared with
/// `helpers.rs`, whose export lookup keys on exactly these names.
pub(super) fn module_export_name(name: &ModuleExportName<'_>) -> String {
    match name {
        ModuleExportName::IdentifierName(id) => id.name.as_str().to_owned(),
        ModuleExportName::IdentifierReference(id) => id.name.as_str().to_owned(),
        ModuleExportName::StringLiteral(literal) => literal.value.as_str().to_owned(),
    }
}

/// Renders a filesystem path as a stable, `/`-separated string, for use as an identity component
/// (`free:module:<file>:name`, `free:outer:<file>:name`) and as the starting point for
/// [`normalize_relative_specifier`]. `/`-joining rather than using the path's native separator
/// keeps the result identical regardless of the host platform, which matters because this string
/// is baked into a fingerprint that two files (or two runs) are compared by.
///
/// `RootDir`/`CurDir`/`ParentDir` components contribute no segment of their own: a leading
/// `RootDir` would only add a spurious empty segment (this function does not itself prefix a
/// leading `/`, since only relative composition in `normalize_relative_specifier` needs the
/// segments as a plain stack), and `CurDir`/`ParentDir` should not appear in an already-absolute
/// path in the first place. `Prefix` (a Windows drive letter or UNC root) is kept, so two files on
/// different drives never collide.
fn path_segments(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Prefix(prefix) => Some(prefix.as_os_str().to_string_lossy().into_owned()),
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::RootDir | Component::CurDir | Component::ParentDir => None,
        })
        .collect()
}

/// Join normalized path segments into the file identity used by fingerprint resolution.
pub(super) fn path_key(path: &Path) -> String {
    path_segments(path).join("/")
}

/// Resolves a relative module specifier (`./x`, `../x`) against the importing file's directory
/// into an anchored, `/`-separated path -- lexically, by manipulating path segments as text, with
/// no filesystem access and no attempt at extension or index-file resolution.
///
/// This is no longer the primary way a relative specifier's identity is computed -- that is now
/// [`resolver::resolve`], which does real filesystem resolution. This function survives
/// as the *unresolved* fallback (see [`import_origins`]): when `resolver::resolve` cannot find a
/// real file, this still stops two files in different directories that both write the same
/// relative specifier (e.g. `import { helper } from './helpers'`) from being treated as the same
/// import, which an adversarial review proved happens without it, and which resolution failing is
/// not a reason to stop guarding against.
///
/// `..` never pops past an empty stack: an import climbing above the importer's own path is not
/// valid input this function needs to resolve correctly, only safely (never panic, never
/// underflow). `importer_dir` is expected to already be absolute (callers pass a `SourceFile`'s
/// own path), so in practice this never triggers, but a defensive floor costs nothing here.
fn normalize_relative_specifier(importer_dir: &Path, specifier: &str) -> String {
    let mut segments = path_segments(importer_dir);
    for segment in specifier.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other.to_owned()),
        }
    }
    segments.join("/")
}

// ---------------------------------------------------------------------------------------------
// Shared symbol-identity helpers, used by both the serializer and the behavior collector.
// ---------------------------------------------------------------------------------------------

/// Test whether the inner byte span lies entirely within the outer span.
fn within(inner: Span, outer: Span) -> bool {
    inner.start >= outer.start && inner.end <= outer.end
}

/// Check whether a symbol's declaration span lies inside the fingerprinted callable body.
fn is_local_symbol(semantic: &Semantic<'_>, body_span: Span, symbol_id: SymbolId) -> bool {
    within(semantic.scoping().symbol_span(symbol_id), body_span)
}

/// Identity for a symbol declared **outside** the body being fingerprinted. Classifies by what
/// the symbol *is*, not where it happens to sit -- and the two families below are deliberately
/// built differently, because they need opposite properties. See the module doc for the full
/// history of getting this wrong in both directions.
///
/// - An **import binding** names something that lives outside this file entirely, so its identity
///   must be content-derived: the specifier's real, resolved module (via [`resolver::resolve`]) --
///   or, failing that, the specifier anchored to this file's directory (see
///   [`normalize_relative_specifier`]), tagged so it can never collide with a real resolution --
///   plus the name on the exporting side. Two files importing the same thing must agree.
/// - A **module-level declaration or outer closure variable** names something that lives *inside*
///   this file, and sibling scopes routinely reuse a name (`describe.each` blocks each declaring
///   their own `const config = ...` is the case that proved this matters) for genuinely different
///   bindings. Content (the name) is not enough to tell them apart -- only the binding's own
///   declaration identifies it, so this uses the declaration's span (stable under edits anywhere
///   else in the file, unlike a declaration-order index such as `SymbolId`) plus the file, scoped
///   as `module` or `outer` so a same-named import can never collide with either.
fn free_identity(
    semantic: &Semantic<'_>,
    imports: &ImportOrigins,
    file_key: &str,
    name: &str,
    symbol_id: SymbolId,
) -> String {
    let scoping = semantic.scoping();
    if scoping.symbol_flags(symbol_id).is_import() {
        return match imports.get(name) {
            Some(origin) => format!("free:import:{}:{}", origin.component, origin.exported),
            // The re-parse in `import_origins` failed, or otherwise disagreed with `semantic`
            // about this being an import (should not happen, but must not panic if it does).
            // Falling back to the bare name *alone* would reintroduce the exact cross-file
            // collision this function exists to close, so this scopes by file too, same as the
            // `free:module` case below -- a precision loss (this specific import can no longer
            // match its counterpart in another file), not a soundness one. Tagged `unresolved`
            // for the same reason every other resolution failure is: it must never be able to
            // collide with a `free:import:resolved:...` identity.
            None => format!("free:import:unresolved:{file_key}:{name}"),
        };
    }
    let scope_id = semantic.symbol_scope(symbol_id);
    let declaration = scoping.symbol_span(symbol_id);
    if scoping.scope_flags(scope_id).is_top() {
        format!(
            "free:module:{file_key}:{}-{}:{name}",
            declaration.start, declaration.end
        )
    } else {
        format!(
            "free:outer:{file_key}:{}-{}:{name}",
            declaration.start, declaration.end
        )
    }
}

/// Gets-or-assigns the positional canonical name for a local symbol, in first-encounter order.
fn canonical_local(
    canonical: &mut HashMap<SymbolId, String>,
    next: &mut u32,
    prefix: &str,
    symbol_id: SymbolId,
) -> String {
    if let Some(existing) = canonical.get(&symbol_id) {
        return existing.clone();
    }
    let name = format!("{prefix}{next}");
    *next += 1;
    canonical.insert(symbol_id, name.clone());
    name
}

/// Renames a `BindingIdentifier` (a declaration site).
///
/// Every `BindingIdentifier` reachable while walking the body's own subtree is, by construction,
/// declared inside the body's span -- an identifier cannot be part of the subtree it declares a
/// symbol outside of. So this is unconditionally a local; the only defensive fallback is a symbol
/// that failed to bind at all, which `SemanticBuilder` should never produce but which must not
/// panic if it somehow does.
fn binding_identity(
    canonical: &mut HashMap<SymbolId, String>,
    next: &mut u32,
    prefix: &str,
    symbol_id: Option<SymbolId>,
    name: &str,
) -> String {
    match symbol_id {
        Some(symbol_id) => canonical_local(canonical, next, prefix, symbol_id),
        None => format!("unbound:{name}"),
    }
}

/// Renames an `IdentifierReference` (a use site): local, free, or global. See the module doc.
/// Every parameter is a distinct piece of resolution context with no natural grouping; bundling
/// them into a struct purely to satisfy the lint would add a type that means nothing.
#[allow(clippy::too_many_arguments)]
fn reference_identity(
    semantic: &Semantic<'_>,
    body_span: Span,
    imports: &ImportOrigins,
    file_key: &str,
    canonical: &mut HashMap<SymbolId, String>,
    next: &mut u32,
    prefix: &str,
    name: &str,
    reference_id: Option<ReferenceId>,
) -> String {
    let symbol_id = reference_id
        .and_then(|reference_id| semantic.scoping().get_reference(reference_id).symbol_id());
    match symbol_id {
        None => format!("global:{name}"),
        Some(symbol_id) if is_local_symbol(semantic, body_span, symbol_id) => {
            canonical_local(canonical, next, prefix, symbol_id)
        }
        Some(symbol_id) => free_identity(semantic, imports, file_key, name, symbol_id),
    }
}

/// Whether an `IdentifierReference` resolves to a symbol declared inside `body_span`.
///
/// Used by assertion/stub detection to recognize `expect`/`todo`/etc. by name while still
/// refusing to be fooled by a local variable that happens to share that name (e.g. a test that
/// shadows `expect`, however unlikely).
fn is_local_reference(
    semantic: &Semantic<'_>,
    body_span: Span,
    ident: &IdentifierReference<'_>,
) -> bool {
    ident
        .reference_id
        .get()
        .and_then(|reference_id| semantic.scoping().get_reference(reference_id).symbol_id())
        .is_some_and(|symbol_id| is_local_symbol(semantic, body_span, symbol_id))
}

/// An `expect`/`expect.soft`/`expect.poll` bare identifier, resolved so it is not fooled by a
/// same-named local (see [`is_local_reference`]).
fn is_expect_identifier(
    semantic: &Semantic<'_>,
    body_span: Span,
    ident: &IdentifierReference<'_>,
) -> bool {
    ident.name.as_str() == "expect" && !is_local_reference(semantic, body_span, ident)
}

/// Escapes `\`, `<`, and `>` in a literal's raw text so it can never be mistaken for the
/// `<kind:raw>` framing [`Serializer::literal`] wraps it in.
///
/// `\` must be escaped too, not just the two delimiters: without it, a payload ending in a raw
/// `\` immediately followed by an escaped `\<` (from a *different*, adjacent piece of text) would
/// let the backslash pair up with the wrong following character when read back informally, the
/// same ambiguity classic C-string/regex escaping schemes exist to avoid. Escaping all three with
/// the same backslash prefix keeps the scheme unambiguous: after escaping, the only unescaped `<`
/// or `>` bytes in the output are the ones this module inserts itself as real structural
/// delimiters, so no literal's contents can ever imitate the boundary between two serialized
/// nodes. This function does not need to be reversible (nothing ever parses a fingerprint back
/// into a tree -- fingerprints are only ever compared for equality), only injective, which
/// escaping the three special bytes guarantees.
pub(super) fn escape_literal_delimiters(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '<' => escaped.push_str("\\<"),
            '>' => escaped.push_str("\\>"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Normalizes a JSX text child to the string a renderer actually places in the DOM.
///
/// JSX source text is laid out for readability, not for content: an element written across lines
/// carries `\n  Save\n` as its text child, and the transform strips the whitespace-only lines and
/// the indentation before any of it renders -- which is why this exists. Without it, the *same*
/// element written on one line and across lines serialized differently, and a repository-wide
/// formatter run broke every match between two bodies that render identically (the
/// `duplicate-test-jsx-formatting-only` recall case). The rule is the one Babel's
/// `cleanJSXElementLiteralChild` applies, because that is what decides the rendered text:
///
/// - split on line breaks (`\r\n`, `\r`, `\n` all normalized to `\n` first);
/// - tabs become spaces, then a line's leading spaces are stripped unless it is the first line,
///   and its trailing spaces are stripped unless it is both the last line and the last non-empty
///   line;
/// - surviving non-empty lines are joined with a single space appended to all but the last
///   non-empty one;
/// - a result with no non-empty lines is empty, and an empty text child is not a child at all.
///
/// Known under-match, recorded rather than fixed: HTML entities (`&amp;`) are NOT decoded -- the
/// parser hands JSX text over verbatim, so `&amp;` and `&` stay different texts and two elements
/// using different spellings compare unequal. The safe direction, and vanishingly rare in test
/// bodies.
fn normalize_jsx_text(raw: &str) -> String {
    let normalized_breaks = raw.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized_breaks.split('\n').collect();
    // The last line holding anything other than spaces/tabs; none means the child is whitespace
    // only, which renders nothing.
    let Some(last_non_empty) = lines
        .iter()
        .rposition(|line| line.chars().any(|ch| ch != ' ' && ch != '\t'))
    else {
        return String::new();
    };
    let mut rendered = String::new();
    for (index, line) in lines.iter().enumerate() {
        let is_first = index == 0;
        let is_last = index + 1 == lines.len();
        let is_last_non_empty = index == last_non_empty;
        let mut trimmed = line.replace('\t', " ");
        if !is_first {
            trimmed = trimmed.trim_start_matches(' ').to_owned();
        }
        if !is_last || !is_last_non_empty {
            trimmed = trimmed.trim_end_matches(' ').to_owned();
        }
        if !trimmed.is_empty() {
            rendered.push_str(&trimmed);
            if !is_last_non_empty {
                rendered.push(' ');
            }
        }
    }
    rendered
}

/// Whether `ch` can appear in a TypeScript identifier, for [`normalize_type_argument_text`]'s
/// "keep a separator only where removing it would weld two words together" rule.
fn is_type_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$'
}

/// Canonicalizes a type-argument list's raw source text so that formatting alone never changes a
/// fingerprint.
///
/// The type-level AST is not modeled by this module (nothing above `src/frontend/` consumes a
/// type), so type arguments are compared as source text -- the same "opaque raw text" bias every
/// other unmodeled construct gets (see the module doc's "Deliberate under-matching"). Raw text
/// alone would be wrong in the *other* direction though: `Array<string>` and `Array< string >`
/// name the same type, and reporting two tests as different because a formatter put a space
/// somewhere is a defect of its own. So whitespace is normalized away, with exactly two carve-outs
/// that keep the normalization from over-matching:
///
/// - A whitespace run is deleted unless it separates two identifier characters, in which case it
///   collapses to a single space. `Record< string , number >` and `Record<string,number>` become
///   the same text, while `keyof T` keeps its space and can never collide with an identifier
///   spelled `keyofT`.
/// - Text inside a string literal type (`'a b'`, `"a b"`, and template literal types) is copied
///   verbatim, escapes included. `Record<'a b', 1>` and `Record<'ab', 1>` are genuinely different
///   types and the whitespace inside the quotes is content, not formatting.
///
/// Comments inside a type argument are deliberately *not* stripped: they are vanishingly rare
/// there, and keeping them only ever makes two type arguments compare as different, which is the
/// safe direction.
fn normalize_type_argument_text(raw: &str) -> String {
    let mut normalized = String::with_capacity(raw.len());
    let mut characters = raw.chars().peekable();
    let mut quote: Option<char> = None;
    let mut pending_separator = false;
    while let Some(ch) = characters.next() {
        if let Some(delimiter) = quote {
            normalized.push(ch);
            if ch == '\\' {
                if let Some(escaped) = characters.next() {
                    normalized.push(escaped);
                }
            } else if ch == delimiter {
                quote = None;
            }
            continue;
        }
        if ch.is_whitespace() {
            pending_separator = true;
            continue;
        }
        if pending_separator {
            pending_separator = false;
            let previous_is_word = normalized
                .chars()
                .next_back()
                .is_some_and(is_type_word_char);
            if previous_is_word && is_type_word_char(ch) {
                normalized.push(' ');
            }
        }
        normalized.push(ch);
        if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        }
    }
    normalized
}

/// Renders an explicit type-argument list as a fingerprint fragment, or the empty string when the
/// construct carries none.
///
/// Returned without a leading separator so that a call, `new`, or tagged template with no type
/// arguments serializes exactly as it did before type arguments were modeled -- the fragment is
/// concatenated straight onto the callee/tag/path, the same way `ser_call` already concatenates
/// its `?.` marker onto `call`.
///
/// The payload goes through [`escape_literal_delimiters`] for the same reason a literal's does:
/// it is arbitrary source text being framed by `<...>`, and unescaped type text could otherwise
/// imitate the delimiter grammar this serializer uses between sibling nodes.
fn type_argument_fragment(
    source: &str,
    type_arguments: Option<&TSTypeParameterInstantiation<'_>>,
) -> String {
    let Some(type_arguments) = type_arguments else {
        return String::new();
    };
    let raw = source
        .get(type_arguments.span.start as usize..type_arguments.span.end as usize)
        .unwrap_or_default();
    format!(
        "<targs:{}>",
        escape_literal_delimiters(&normalize_type_argument_text(raw))
    )
}

// ---------------------------------------------------------------------------------------------
// alpha / structural serialization
// ---------------------------------------------------------------------------------------------

/// Which of the two textual fingerprints is being produced.
#[derive(Clone, Copy, PartialEq, Eq)]
/// Which of the two exact-equality views a serialization pass produces. `pub(super)` only so
/// `helpers.rs` can thread it through to [`render_inlined_helper_body`]; no other module matches
/// on it.
pub(super) enum Mode {
    /// Locals renamed by symbol; literals preserved.
    Alpha,
    /// Locals renamed by symbol; literals collapsed to their kind.
    Structural,
}

/// Produces a deterministic S-expression-shaped serialization of a callback body.
///
/// One `Serializer` is used for exactly one of `alpha`/`structural`; its `canonical` map
/// is local to that one pass. Because the traversal order is a pure function of the AST plus the
/// (shared, read-only) `Semantic`, the `alpha` and `structural` passes independently assign the
/// same canonical names to the same symbols without needing to share state.
/// The file-level construction context and canonicalization state that [`Serializer`] and
/// [`BehaviorCollector`] share verbatim.
///
/// The two traversals answer different questions over the same body -- one renders a whole
/// callable as text, the other collects ordered behavior events -- but everything they know
/// about the *file* is identical, and so is the canonical-naming state both must advance for
/// `behavior`'s shapes and `alpha`'s text to agree on what a symbol is called. This struct exists
/// because the two used to repeat ten fields and two methods, which is one construct handled
/// twice, in two places that can drift apart.
struct FingerprintContext<'a, 'b, 's, 'm> {
    semantic: &'b Semantic<'a>,
    source: &'s str,
    imports: &'m ImportOrigins,
    /// The fingerprinted file's own path, rendered by [`path_key`]. Owned rather than borrowed:
    /// it is cheap (one small string per body), and avoids yet another lifetime parameter for a
    /// value every caller already has to compute once per `fingerprint` call.
    file_key: String,
    /// The fingerprinted file's own path, unrendered. `file_key` is the *display* form; this is
    /// what [`specifier_identity`] needs to resolve a specifier relative to the importer, and a
    /// dynamic `import()` is the one place inside a body that carries one.
    file: PathBuf,
    body_span: Span,
    /// This row's substitution, already resolved to `SymbolId`s by [`resolve_row_substitution`] --
    /// empty (never populated) when `fingerprint` was not given a `.each` row substitution, or
    /// when this body's parameter shape did not qualify for one. See the module doc's "Row
    /// substitution" section; consulted only by [`Serializer::reference_or_literal`].
    row_substitution: &'m HashMap<SymbolId, (&'static str, String)>,
    /// This body's dataflow canonicalization, computed once by [`analyze_dataflow`]. Empty for
    /// every `Serializer` `fingerprint` constructs directly (`alpha`/`structural`); only
    /// [`BehaviorCollector::shape`]'s temporary `Serializer` is ever given a populated map. See
    /// the "Dataflow canonicalization" section doc above [`DataflowSubst`]; consulted only by
    /// [`Serializer::reference_or_literal`].
    dataflow: &'m HashMap<SymbolId, DataflowSubst<'a, 'b>>,
    /// Parameter-to-argument-fragment substitution for an inlined imported helper's body (see
    /// `helpers.rs`): the helper's parameter symbols, each mapped to the already-rendered text of
    /// the argument the call site passed. Empty for every ordinary pass -- only the `Serializer`
    /// [`render_inlined_helper_body`] constructs is ever given a populated map, and its keys are
    /// that helper file's own `SymbolId`s, never the fingerprinted body's. Consulted only by
    /// [`Serializer::reference_or_literal`], before every other substitution, on exactly the
    /// terms `row_substitution` substitutes a row's cells: a substituted read is the argument's
    /// rendering, not an `id:`-prefixed reference.
    arg_substitution: HashMap<SymbolId, String>,
    /// The parameter symbols whose substitution [`Serializer::reference_or_literal`] has
    /// actually consumed -- reads that rendered. An inlined helper's wrapper renders every
    /// UNCONSUMED argument in an `(args ...)` tail, because an argument no read consumed is
    /// invisible to the body's text and yet still observable at runtime (`arguments[i]` indexes
    /// it): two calls differing only in an unread argument must not render alike.
    consumed_args: HashSet<SymbolId>,
    /// Whether this pass may inline imported helper bodies at call sites (`helpers.rs`). True for
    /// the passes over a fingerprinted body; false inside an inlined helper's own rendering, so
    /// inlining is exactly one level deep -- a helper calling another imported helper renders
    /// that inner call as an ordinary `free:import` identity, which under-matches (equivalent
    /// inner helpers stay distinct) rather than recursing without a bound.
    inline_imported_helpers: bool,
    /// Imported-helper inlining's **accounting**, recorded by the pass itself so that what
    /// extract.rs drops from `Context::imports` is what this pass *actually rendered*, never a
    /// re-derivation of the decision (an opaque-rendered region -- a class body -- hides a call
    /// from the serializer while still showing it to a reference-counting visitor, and the two
    /// disagreeing was a review-found false-merge). `inlined` collects components whose calls
    /// rendered as bodies; `identity` collects components that rendered at least once as a
    /// `free:import` identity (a refused call, a value reference, an argument read). A module is
    /// fully inlined for this body iff it is in `inlined` and not in `identity` -- see
    /// [`Self::fully_inlined_imports`].
    ///
    /// Degraded or opaque regions inside a rendered body do **not** un-report the module (a
    /// review suggestion, declined): `<opaque:...>`/`<max-depth:...>` carry byte-exact raw
    /// source, which is comparison at its strictest -- two bodies render equal through them only
    /// when the raw text is equal -- so "the content was compared directly" remains true, just
    /// by raw text instead of structure. Un-reporting would trade a real comparison for a
    /// coarser identity without removing any merge.
    inlined_imports: HashSet<String>,
    identity_imports: HashSet<String>,
    /// The canonical-local namespace of this pass: `L` for a pass over a fingerprinted body,
    /// `H` for an inlined helper's own rendering. Separate namespaces by construction, so a
    /// helper's own locals can never render the same name as a caller local appearing in a
    /// substituted argument fragment -- the two are numbered by different passes, and equal
    /// spellings from different allocations must not be able to make different bodies compare
    /// equal (review-hardened; the structural argument already held, the prefix removes the
    /// question).
    local_prefix: &'static str,
    canonical: HashMap<SymbolId, String>,
    next_local: u32,
}

impl<'a, 'b, 's, 'm> FingerprintContext<'a, 'b, 's, 'm> {
    // Every parameter is a distinct piece of construction context with no natural grouping --
    // same rationale as `reference_identity`'s own `#[allow]` above.
    /// The file-level construction context from `fingerprint`'s inputs, with empty
    /// per-pass naming state.
    #[allow(clippy::too_many_arguments)]
    fn new(
        semantic: &'b Semantic<'a>,
        source: &'s str,
        imports: &'m ImportOrigins,
        file_key: String,
        file: PathBuf,
        body_span: Span,
        row_substitution: &'m HashMap<SymbolId, (&'static str, String)>,
        dataflow: &'m HashMap<SymbolId, DataflowSubst<'a, 'b>>,
    ) -> Self {
        Self {
            semantic,
            source,
            imports,
            file_key,
            file,
            body_span,
            row_substitution,
            dataflow,
            arg_substitution: HashMap::new(),
            consumed_args: HashSet::new(),
            inline_imported_helpers: true,
            inlined_imports: HashSet::new(),
            identity_imports: HashSet::new(),
            local_prefix: "L",
            canonical: HashMap::new(),
            next_local: 0,
        }
    }

    /// A fresh pass over the same file: identical borrowed inputs, re-owned path strings, and
    /// empty canonical-naming state. Every pass must start canonical naming from zero (see
    /// [`Serializer`]'s doc for why the passes deliberately never share it), so this -- not a
    /// repeat of the construction argument list -- is how one `fingerprint` call yields three
    /// independent passes.
    fn fresh_for_pass(&self) -> Self {
        Self {
            semantic: self.semantic,
            source: self.source,
            imports: self.imports,
            file_key: self.file_key.clone(),
            file: self.file.clone(),
            body_span: self.body_span,
            row_substitution: self.row_substitution,
            dataflow: self.dataflow,
            // Both inlining-related fields are per-pass constants in truth: an ordinary body's
            // passes never substitute helper arguments, and they all inline; an inlined helper's
            // own rendering pass is never built through here.
            arg_substitution: HashMap::new(),
            consumed_args: HashSet::new(),
            inline_imported_helpers: self.inline_imported_helpers,
            // Accounting starts empty for every pass: only the alpha pass's numbers are ever
            // reported, and only after it finishes.
            inlined_imports: HashSet::new(),
            identity_imports: HashSet::new(),
            local_prefix: "L",
            canonical: HashMap::new(),
            next_local: 0,
        }
    }

    /// [`Self::fresh_for_pass`] with `dataflow` swapped -- the one input that differs between
    /// passes, since only the behavior pass consults dataflow canonicalization (see `fingerprint`).
    fn fresh_for_pass_with(&self, dataflow: &'m HashMap<SymbolId, DataflowSubst<'a, 'b>>) -> Self {
        let mut fresh = self.fresh_for_pass();
        fresh.dataflow = dataflow;
        fresh
    }

    /// The source text under `span` (empty when the span is out of range).
    fn slice(&self, span: Span) -> &'s str {
        self.source
            .get(span.start as usize..span.end as usize)
            .unwrap_or_default()
    }

    /// Renders one identifier reference: canonical local names, free/import identities, or a
    /// global name -- the single choke point every identifier rendering passes through, which is
    /// why the imported-helper inlining accounting records here (a reference that renders an
    /// import *identity* marks the module as not fully inlined).
    fn reference(&mut self, ident: &IdentifierReference<'_>) -> String {
        // Accounting for imported-helper inlining (see the `inlined_imports`/`identity_imports`
        // doc): this is the one choke point every identifier rendering passes through, so a
        // reference to an import binding that renders as an identity -- a refused call's callee,
        // a value reference, an argument read -- is recorded here, exactly when it happens.
        if let Some(origin) = self.import_origin_of(ident) {
            self.identity_imports.insert(origin.component.clone());
        }
        reference_identity(
            self.semantic,
            self.body_span,
            self.imports,
            &self.file_key,
            &mut self.canonical,
            &mut self.next_local,
            self.local_prefix,
            ident.name.as_str(),
            ident.reference_id.get(),
        )
    }

    /// The import binding `ident` resolves to, when it resolves to one -- by symbol identity,
    /// the same dispatch [`free_identity`] performs, so a local shadowing an import's name is
    /// never recorded as a reference to that import.
    fn import_origin_of(&self, ident: &IdentifierReference<'_>) -> Option<&ImportOrigin> {
        let symbol_id = ident.reference_id.get().and_then(|reference_id| {
            self.semantic
                .scoping()
                .get_reference(reference_id)
                .symbol_id()
        })?;
        self.semantic
            .scoping()
            .symbol_flags(symbol_id)
            .is_import()
            .then(|| self.imports.get(ident.name.as_str()))
            .flatten()
    }

    /// The modules whose every reference in this pass rendered as an inlined helper body: the
    /// report `fingerprint()` hands extract.rs, which drops exactly these from
    /// `Context::imports`. Sorted, so the report is a pure function of the body.
    fn fully_inlined_imports(&self) -> Vec<String> {
        let mut fully: Vec<String> = self
            .inlined_imports
            .difference(&self.identity_imports)
            .cloned()
            .collect();
        fully.sort();
        fully
    }
}

/// Produces a deterministic S-expression-shaped serialization of a callback body.
///
/// One `Serializer` is used for exactly one of `alpha`/`structural`; its `canonical` map
/// is local to that one pass. Because the traversal order is a pure function of the AST plus the
/// (shared, read-only) `Semantic`, the `alpha` and `structural` passes independently assign the
/// same canonical names to the same symbols without needing to share state.
struct Serializer<'a, 'b, 's, 'm> {
    /// Everything both traversals know about the file, plus the shared canonical-naming state;
    /// see [`FingerprintContext`].
    ctx: FingerprintContext<'a, 'b, 's, 'm>,
    mode: Mode,
    /// Whether the callable currently being serialized reads `this` or `arguments`.
    ///
    /// Arrow and `function` callbacks are deliberately collapsed to one serialization -- Vitest
    /// accepts either, and rewriting one as the other is the single most common cosmetic
    /// difference between two copies of the same test (`near-arrow-vs-function-equivalence`
    /// pins that collapse). The collapse is only sound while the body uses neither `this` nor
    /// `arguments`: those are precisely the constructs an arrow does *not* bind itself, so two
    /// otherwise byte-identical bodies mean different things across the two forms. Collapsing
    /// them anyway reported a `duplicate-test` at Error severity on correct code.
    ///
    /// Tracked as state rather than a pre-pass because `this` binds to the nearest enclosing
    /// *non-arrow* function: a `this` inside a nested arrow belongs to the function around it,
    /// so the flag propagates outward through arrows and stops at a `function`. See
    /// [`Serializer::ser_callable`].
    this_or_arguments: bool,
    /// Whether the OUTERMOST callable this serializer ran over was `this`/`arguments`-dependent.
    ///
    /// Not the same as [`Serializer::this_or_arguments`] after the run: `ser_callable` *absorbs*
    /// the flag for a `function` (it binds its own `this`), so the post-state reads `false` for
    /// exactly the body that is most dependent on it. This records the decision at the moment it
    /// is made, for the one callable that matters -- the test body itself.
    outermost_this_dependent: bool,
}

impl<'a, 'b, 's, 'm> Serializer<'a, 'b, 's, 'm> {
    /// One serializer for exactly one pass over one body.
    fn new(ctx: FingerprintContext<'a, 'b, 's, 'm>, mode: Mode) -> Self {
        Self {
            ctx,
            mode,
            this_or_arguments: false,
            outermost_this_dependent: false,
        }
    }

    /// The whole callable as one text: signature, then statements.
    fn run(&mut self, body: &FunctionLike<'a, '_>) -> String {
        self.ser_callable(body, 0)
    }

    /// [`Serializer::run`], also reporting whether the body binds meaning to `this`/`arguments`.
    ///
    /// `behavior` needs the same answer `alpha` computes, and computing it twice is how two
    /// views of one body drift apart. Taking it from the alpha pass costs nothing: the body is
    /// serialized exactly as often as before. Borrowed (`&mut self`, unlike the by-value shape
    /// these methods once had) so the caller can also read the pass's import accounting when it
    /// finishes -- see `fingerprint`'s use of [`FingerprintContext::fully_inlined_imports`].
    fn run_observing_this(&mut self, body: &FunctionLike<'a, '_>) -> (String, bool) {
        let text = self.ser_callable(body, 0);
        (text, self.outermost_this_dependent)
    }

    /// Raw-text fallback for anything not explicitly modeled. Deliberately under-matching: two
    /// different opaque constructs never accidentally collapse (they compare by literal text,
    /// idents and all), but two constructs whose only difference lives inside an opaque region
    /// stay indistinguishable to this fingerprint. See the module doc.
    fn opaque(&self, span: Span) -> String {
        format!("<opaque:{}>", self.ctx.slice(span))
    }

    /// Retain a depth-limit marker and the original source slice rather than claiming a
    /// complete rendering.
    fn degraded(&self, span: Span) -> String {
        format!("<max-depth:{}>", self.ctx.slice(span))
    }

    /// An explicit type-argument list, rendered identically in all three modes.
    ///
    /// `Mode::Structural` collapses a literal's *value* to its kind so that
    /// `parameterization-candidate` can propose a `test.each` table whose rows supply the values
    /// that differ. A type argument is not a value: `assertType<string>` and `assertType<number>`
    /// cannot be consolidated into two rows of a table, because a table cell is a runtime value
    /// and a type argument is erased before any value exists. Collapsing type arguments in
    /// `structural` would therefore not enable a real consolidation, it would only manufacture a
    /// `parameterization-candidate` finding whose suggested fix is impossible to write -- and it
    /// would do the same to `behavior`'s assertion *subject* shape, which is rendered in
    /// `structural` too ([`BehaviorCollector::assertion_event`]), re-opening the very erasure
    /// this models its way out of one rule lower down. So every mode keeps the type arguments.
    fn ser_type_arguments(
        &self,
        type_arguments: Option<&TSTypeParameterInstantiation<'a>>,
    ) -> String {
        type_argument_fragment(self.ctx.source, type_arguments)
    }

    /// Renders an `IdentifierReference` in *value* (read) position: a row-substituted parameter
    /// renders exactly as [`Serializer::literal`] would render the same value written directly as
    /// a literal in source, and a dataflow-substituted local (see [`DataflowSubst`]) renders
    /// exactly as its substituted expression would render if written directly at this position --
    /// both deliberately *not* wrapped in the `id:` prefix every other identifier reference gets,
    /// because that prefix exists precisely to keep an identifier visibly distinct from what it
    /// stands in for, and a substituted reference must be indistinguishable from the real thing.
    /// Only a resolved local symbol can have a substitution at all -- nothing this module
    /// resolves to `free`/`global` was ever a parameter of this body or a dataflow-canonicalized
    /// local (`analyze_dataflow` only ever registers a body-local symbol).
    fn reference_or_literal(&mut self, ident: &IdentifierReference<'_>, depth: u32) -> String {
        // `arguments` is the other construct an arrow does not bind (see `this_or_arguments`).
        // It reaches here as an ordinary identifier reference; a *resolved* symbol by that name
        // is some user's own binding and carries none of the meaning, so only the free form --
        // the real `arguments` object -- counts.
        if ident.name.as_str() == "arguments"
            && ident
                .reference_id
                .get()
                .and_then(|reference_id| {
                    self.ctx
                        .semantic
                        .scoping()
                        .get_reference(reference_id)
                        .symbol_id()
                })
                .is_none()
        {
            self.this_or_arguments = true;
        }
        let symbol_id = ident.reference_id.get().and_then(|reference_id| {
            self.ctx
                .semantic
                .scoping()
                .get_reference(reference_id)
                .symbol_id()
        });
        if let Some(symbol_id) = symbol_id {
            if let Some(fragment) = self.ctx.arg_substitution.get(&symbol_id).cloned() {
                self.ctx.consumed_args.insert(symbol_id);
                return fragment;
            }
            if let Some((kind, raw)) = self.ctx.row_substitution.get(&symbol_id).cloned() {
                return self.literal(kind, &raw);
            }
            let dataflow = self.ctx.dataflow.get(&symbol_id).cloned();
            if let Some(dataflow) = dataflow {
                return match dataflow {
                    DataflowSubst::Inline { init, .. } => self.ser_expr(init, depth + 1),
                    DataflowSubst::Member { object, property } => {
                        let object_repr = self.ser_expr(object, depth + 1);
                        format!("(member {object_repr} .{property})")
                    }
                    DataflowSubst::DestructuredProperty { property } => {
                        format!("id:{}:prop:{property}", self.ctx.reference(ident))
                    }
                };
            }
        }
        format!("id:{}", self.ctx.reference(ident))
    }

    /// Allocate or reuse a canonical binding identity in this serialization context.
    fn binding(&mut self, name: &str, symbol_id: Option<SymbolId>) -> String {
        binding_identity(
            &mut self.ctx.canonical,
            &mut self.ctx.next_local,
            self.ctx.local_prefix,
            symbol_id,
            name,
        )
    }

    /// Renders a literal, always including its `kind` so two literals of different types that
    /// happen to render the same raw text (`true` the boolean vs. `'true'` the string; `1000` the
    /// number vs. `'1000'` the string) can never collide, and so an empty-content literal (`''`)
    /// can never vanish entirely and become indistinguishable from *no* literal at all -- both
    /// were real, adjudicator-found `duplicate-test` false positives (`expect(parse(true))` vs.
    /// `expect(parse('true'))`; `expect(parse(['']))` vs. `expect(parse([]))`, since `''` used to
    /// serialize to nothing, making a one-element array of it byte-identical to an empty array).
    /// This also keeps `alpha` from drifting toward `structural`: two bodies that differ only in
    /// a literal's *value* should be indistinguishable under `alpha` only when they also share
    /// the literal's *kind* -- that is what makes `structural`'s further collapse (which does
    /// erase kind, deliberately) the meaningful step between `duplicate-test` and
    /// `parameterization-candidate`, rather than `alpha` already doing `structural`'s job.
    ///
    /// `raw` is escaped before being framed by `<kind:...>` (see [`escape_literal_delimiters`]).
    /// Without escaping, `raw` is attacker/author-controlled text (a string literal's own
    /// contents) that can itself contain the `<`/`>`/space grammar this serializer uses to
    /// separate sibling nodes, so one literal whose *value* spells out that grammar can render
    /// byte-identical to a sequence of several sibling literals -- e.g. an array holding the
    /// single string `"x> <string:y"` and an array holding the two strings `"x"` and `"y"` both
    /// serialize their elements to `<string:x> <string:y>` without escaping, joined identically
    /// by `ser_array_element`'s `" "`.join, producing the same `alpha` for genuinely different
    /// code (this was a real, self-inflicted `duplicate-test` false positive at Error severity,
    /// introduced by the kind-tagging fix above). Escaping -- rather than length-prefixing -- is
    /// chosen because it is a strictly local, one-line change at the single call site that
    /// produces every literal's raw text, it keeps the serialized form readable for debugging
    /// (a length prefix would require counting bytes by eye to find the payload boundary), and it
    /// composes for free with every other literal-producing call site (`ser_template`'s quasis
    /// are deliberately left unescaped raw text by contrast -- see that function's own comment --
    /// because they are never compared as a bare kind-delimited unit the way `literal` values
    /// are).
    fn literal(&self, kind: &'static str, raw: &str) -> String {
        match self.mode {
            Mode::Alpha => {
                format!("<{kind}:{}>", escape_literal_delimiters(raw))
            }
            Mode::Structural => format!("<{kind}>"),
        }
    }

    /// Serialize a callable's flags, parameters and body while scoping this/arguments
    /// observations to its binding behavior.
    fn ser_callable(&mut self, body: &FunctionLike<'a, '_>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(body.span());
        }
        let asyncness = if body.is_async() { "async " } else { "" };
        let generator = if body.is_generator() { "*" } else { "" };
        let params = self.ser_params(body.params(), depth + 1);
        // Scope the `this`/`arguments` observation to this callable. An arrow does not bind
        // either, so whatever it saw still belongs to the enclosing function and is propagated
        // back below; a `function` does bind both, so what it saw stops here and must not make
        // its *parent* look `this`-dependent.
        let enclosing_saw = self.this_or_arguments;
        self.this_or_arguments = false;
        let body_text = match body {
            FunctionLike::Arrow(arrow) => match &arrow.body {
                ArrowFunctionBody::FunctionBody(function_body) => {
                    self.ser_stmts(&function_body.statements, depth + 1)
                }
                other => match other.as_expression() {
                    Some(expression) => self.ser_expr(expression, depth + 1),
                    None => self.opaque(other.span()),
                },
            },
            FunctionLike::Function(function) => match &function.body {
                Some(function_body) => self.ser_stmts(&function_body.statements, depth + 1),
                None => "(no-body)".to_owned(),
            },
        };
        let this_dependent = self.this_or_arguments;
        if depth == 0 {
            self.outermost_this_dependent = this_dependent;
        }
        self.this_or_arguments = match body {
            // Propagate: the arrow did not bind what it saw, its enclosing function did.
            FunctionLike::Arrow(_) => enclosing_saw || this_dependent,
            // Absorb: a `function` binds its own `this`/`arguments`.
            FunctionLike::Function(_) => enclosing_saw,
        };
        // `fn` when the two forms really are interchangeable -- which keeps every existing
        // fingerprint byte-identical -- and the specific form only when it changes meaning.
        let form = if !this_dependent {
            "fn"
        } else {
            match body {
                FunctionLike::Arrow(_) => "arrow-fn",
                FunctionLike::Function(_) => "function-fn",
            }
        };
        format!("({asyncness}{generator}{form} ({params}) {body_text})")
    }

    /// Whether every identifier `pattern` declares is a row-substituted parameter -- meaning
    /// `pattern` should be omitted from the *declaration* side of the fingerprint entirely
    /// (`ser_params`, and a destructured property inside `ser_binding_pattern`'s `ObjectPattern`
    /// arm), not merely have its own read occurrences replaced.
    ///
    /// This matters because a substituted parameter is no longer meaningfully "a declared
    /// parameter" for comparison purposes: its value is now fixed by the row, exactly like a
    /// hand-written literal is fixed by the source that wrote it, and a hand-written duplicate of
    /// a substituted row never declares that parameter at all (`(w, h, expected) => expect(area(w,
    /// h)).toBe(expected)` substituted for row `[2, 3, 6]` must match `() =>
    /// expect(area(2, 3)).toBe(6)`, which has zero parameters, not three canonical locals sitting
    /// unused in the signature). Leaving a fully-substituted parameter in the printed signature
    /// would make every substituted row permanently un-matchable against the hand-written form
    /// row substitution exists to link, defeating the whole feature.
    ///
    /// Only `BindingIdentifier` (always: substituted or not, by direct lookup) and `ObjectPattern`
    /// (recursively: every property, only when there is no rest) can ever be "fully substituted",
    /// because those are the only two shapes [`resolve_row_substitution`] ever populates the map
    /// from; anything else (`AssignmentPattern`, `ArrayPattern`) is never substituted at all and
    /// this always returns `false` for it, which is the correct, safe default -- it can only cause
    /// a parameter to stay visible in the signature (a false negative on matching), never the
    /// unsound direction of erasing a parameter this module did not actually substitute.
    fn binding_fully_substituted(&self, pattern: &BindingPattern<'a>) -> bool {
        match pattern {
            BindingPattern::BindingIdentifier(id) => id
                .symbol_id
                .get()
                .is_some_and(|symbol_id| self.ctx.row_substitution.contains_key(&symbol_id)),
            BindingPattern::ObjectPattern(object) => {
                object.rest.is_none()
                    && !object.properties.is_empty()
                    && object
                        .properties
                        .iter()
                        .all(|property| self.binding_fully_substituted(&property.value))
            }
            BindingPattern::AssignmentPattern(_) | BindingPattern::ArrayPattern(_) => false,
        }
    }

    /// Render formal parameters and defaults, omitting patterns fully replaced by row
    /// substitutions.
    fn ser_params(&mut self, params: &FormalParameters<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(params.span);
        }
        let mut parts: Vec<String> = Vec::with_capacity(params.items.len());
        for item in &params.items {
            // See `binding_fully_substituted`'s doc: a fully-substituted parameter is omitted
            // from the signature entirely, not printed with its substituted value inline --
            // `reference_or_literal` already renders every *use* of it that way; the declaration
            // site itself must vanish, matching a hand-written duplicate that never declared it.
            if self.binding_fully_substituted(&item.pattern) {
                continue;
            }
            let part = match &item.initializer {
                Some(initializer) => format!(
                    "(default {} {})",
                    self.ser_binding_pattern(&item.pattern, depth + 1),
                    self.ser_expr(initializer, depth + 1)
                ),
                None => self.ser_binding_pattern(&item.pattern, depth + 1),
            };
            parts.push(part);
        }
        if let Some(rest) = &params.rest {
            parts.push(format!(
                "(rest {})",
                self.ser_binding_pattern(&rest.rest.argument, depth + 1)
            ));
        }
        parts.join(" ")
    }

    /// Serialize binding identifiers and destructuring/default patterns within the depth limit.
    fn ser_binding_pattern(&mut self, pattern: &BindingPattern<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(pattern.span());
        }
        match pattern {
            BindingPattern::BindingIdentifier(id) => {
                self.binding(id.name.as_str(), id.symbol_id.get())
            }
            BindingPattern::AssignmentPattern(assignment) => format!(
                "(default {} {})",
                self.ser_binding_pattern(&assignment.left, depth + 1),
                self.ser_expr(&assignment.right, depth + 1)
            ),
            BindingPattern::ObjectPattern(object) => {
                let mut parts: Vec<String> = Vec::with_capacity(object.properties.len());
                for property in &object.properties {
                    // See `binding_fully_substituted`'s doc: a substituted destructured property
                    // is omitted from the printed pattern entirely, matching a hand-written
                    // duplicate that would only destructure the fields it still needs.
                    if self.binding_fully_substituted(&property.value) {
                        continue;
                    }
                    parts.push(format!(
                        "(prop {} {})",
                        self.ser_property_key(&property.key, depth + 1),
                        self.ser_binding_pattern(&property.value, depth + 1)
                    ));
                }
                if let Some(rest) = &object.rest {
                    parts.push(format!(
                        "(rest {})",
                        self.ser_binding_pattern(&rest.argument, depth + 1)
                    ));
                }
                format!("(object-pattern {})", parts.join(" "))
            }
            BindingPattern::ArrayPattern(array) => {
                let mut parts: Vec<String> = array
                    .elements
                    .iter()
                    .map(|element| match element {
                        Some(element) => self.ser_binding_pattern(element, depth + 1),
                        None => "<hole>".to_owned(),
                    })
                    .collect();
                if let Some(rest) = &array.rest {
                    parts.push(format!(
                        "(rest {})",
                        self.ser_binding_pattern(&rest.argument, depth + 1)
                    ));
                }
                format!("(array-pattern {})", parts.join(" "))
            }
        }
    }

    /// Render static/private property names or expression-valued keys, retaining opaque
    /// unsupported syntax.
    fn ser_property_key(&mut self, key: &PropertyKey<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(key.span());
        }
        if let Some(expression) = key.as_expression() {
            return format!("[{}]", self.ser_expr(expression, depth + 1));
        }
        match key {
            PropertyKey::StaticIdentifier(id) => format!("prop:{}", id.name.as_str()),
            PropertyKey::PrivateIdentifier(id) => format!("prop:#{}", id.name.as_str()),
            _ => self.opaque(key.span()),
        }
    }

    /// Serialize statements in order into a block fragment, retaining a degradation marker at
    /// the depth limit.
    fn ser_stmts(&mut self, statements: &[Statement<'a>], depth: u32) -> String {
        if depth > MAX_DEPTH {
            let span = statements
                .first()
                .map(GetSpan::span)
                .unwrap_or(Span::new(0, 0));
            return self.degraded(span);
        }
        let parts: Vec<String> = statements
            .iter()
            .map(|statement| self.ser_stmt(statement, depth + 1))
            .collect();
        format!("(block {})", parts.join(" "))
    }

    /// Serialize one statement while preserving its control flow and resolved local bindings.
    /// Function declarations emit their canonical identity before parameters, so references
    /// retain their target even when hoisting places a call before the declaration.
    fn ser_stmt(&mut self, statement: &Statement<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(statement.span());
        }
        match statement {
            Statement::BlockStatement(block) => self.ser_stmts(&block.body, depth + 1),
            Statement::ExpressionStatement(expression_statement) => {
                format!(
                    "(stmt {})",
                    self.ser_expr(&expression_statement.expression, depth + 1)
                )
            }
            Statement::EmptyStatement(_) => "(empty)".to_owned(),
            Statement::DebuggerStatement(_) => "(debugger)".to_owned(),
            Statement::VariableDeclaration(declaration) => {
                self.ser_variable_declaration(declaration, depth)
            }
            Statement::FunctionDeclaration(function) => {
                // Keep the declaration-to-reference link, including when a hoisted call was
                // encountered first. Registering without emitting the identity would still
                // merge calls to different declarations before those declarations appear.
                let name = function.id.as_ref().map_or_else(
                    || "<anonymous>".to_owned(),
                    |id| self.binding(id.name.as_str(), id.symbol_id.get()),
                );
                format!(
                    "(function-decl {name} {})",
                    self.ser_callable(&FunctionLike::Function(function), depth + 1)
                )
            }
            Statement::IfStatement(if_statement) => {
                let test = self.ser_expr(&if_statement.test, depth + 1);
                let consequent = self.ser_stmt(&if_statement.consequent, depth + 1);
                match &if_statement.alternate {
                    Some(alternate) => {
                        format!(
                            "(if {test} {consequent} {})",
                            self.ser_stmt(alternate, depth + 1)
                        )
                    }
                    None => format!("(if {test} {consequent})"),
                }
            }
            Statement::ForStatement(for_statement) => {
                let init = match &for_statement.init {
                    Some(ForStatementInit::VariableDeclaration(declaration)) => {
                        self.ser_variable_declaration(declaration, depth)
                    }
                    Some(other) => match other.as_expression() {
                        Some(expression) => self.ser_expr(expression, depth + 1),
                        None => self.opaque(other.span()),
                    },
                    None => "()".to_owned(),
                };
                let test = for_statement
                    .test
                    .as_ref()
                    .map_or_else(|| "()".to_owned(), |test| self.ser_expr(test, depth + 1));
                let update = for_statement.update.as_ref().map_or_else(
                    || "()".to_owned(),
                    |update| self.ser_expr(update, depth + 1),
                );
                let body = self.ser_stmt(&for_statement.body, depth + 1);
                format!("(for {init} {test} {update} {body})")
            }
            Statement::ForInStatement(for_in) => {
                let left = self.ser_for_left(&for_in.left, depth + 1);
                let right = self.ser_expr(&for_in.right, depth + 1);
                let body = self.ser_stmt(&for_in.body, depth + 1);
                format!("(for-in {left} {right} {body})")
            }
            Statement::ForOfStatement(for_of) => {
                let left = self.ser_for_left(&for_of.left, depth + 1);
                let right = self.ser_expr(&for_of.right, depth + 1);
                let body = self.ser_stmt(&for_of.body, depth + 1);
                format!("(for-of {left} {right} {body})")
            }
            Statement::WhileStatement(while_statement) => {
                format!(
                    "(while {} {})",
                    self.ser_expr(&while_statement.test, depth + 1),
                    self.ser_stmt(&while_statement.body, depth + 1)
                )
            }
            Statement::DoWhileStatement(do_while) => {
                format!(
                    "(do-while {} {})",
                    self.ser_stmt(&do_while.body, depth + 1),
                    self.ser_expr(&do_while.test, depth + 1)
                )
            }
            Statement::ReturnStatement(return_statement) => match &return_statement.argument {
                Some(argument) => format!("(return {})", self.ser_expr(argument, depth + 1)),
                None => "(return)".to_owned(),
            },
            Statement::ThrowStatement(throw_statement) => {
                format!(
                    "(throw {})",
                    self.ser_expr(&throw_statement.argument, depth + 1)
                )
            }
            Statement::TryStatement(try_statement) => {
                let block = self.ser_stmts(&try_statement.block.body, depth + 1);
                let handler = match &try_statement.handler {
                    Some(handler) => self.ser_catch(handler, depth + 1),
                    None => "()".to_owned(),
                };
                let finalizer = match &try_statement.finalizer {
                    Some(finalizer) => self.ser_stmts(&finalizer.body, depth + 1),
                    None => "()".to_owned(),
                };
                format!("(try {block} {handler} {finalizer})")
            }
            Statement::SwitchStatement(switch_statement) => {
                let discriminant = self.ser_expr(&switch_statement.discriminant, depth + 1);
                let cases: Vec<String> = switch_statement
                    .cases
                    .iter()
                    .map(|case| self.ser_switch_case(case, depth + 1))
                    .collect();
                format!("(switch {discriminant} {})", cases.join(" "))
            }
            Statement::BreakStatement(break_statement) => match &break_statement.label {
                Some(label) => format!("(break {})", label.name.as_str()),
                None => "(break)".to_owned(),
            },
            Statement::ContinueStatement(continue_statement) => match &continue_statement.label {
                Some(label) => format!("(continue {})", label.name.as_str()),
                None => "(continue)".to_owned(),
            },
            Statement::LabeledStatement(labeled) => {
                format!(
                    "(label {} {})",
                    labeled.label.name.as_str(),
                    self.ser_stmt(&labeled.body, depth + 1)
                )
            }
            // Class declarations, TS-only declaration forms, and module declarations are rare
            // inside a Vitest callback body; rendered as raw text rather than modeled in depth.
            _ => self.opaque(statement.span()),
        }
    }

    /// A `var`/`let`/`const`/`using` declaration, as a statement or as a `for` initializer --
    /// the same construct in both positions, so one serializer for both.
    fn ser_variable_declaration(
        &mut self,
        declaration: &VariableDeclaration<'a>,
        depth: u32,
    ) -> String {
        let kind = match declaration.kind {
            VariableDeclarationKind::Var => "var",
            VariableDeclarationKind::Let => "let",
            VariableDeclarationKind::Const => "const",
            VariableDeclarationKind::Using => "using",
            VariableDeclarationKind::AwaitUsing => "await-using",
        };
        let declarators: Vec<String> = declaration
            .declarations
            .iter()
            .map(|declarator| self.ser_declarator(declarator, depth + 1))
            .collect();
        format!("({kind} {})", declarators.join(" "))
    }

    /// Serialize supported loop-left declarations and assignment targets; keep unsupported
    /// target forms opaque and emit a degradation marker beyond the depth limit.
    fn ser_for_left(&mut self, left: &ForStatementLeft<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(left.span());
        }
        match left {
            ForStatementLeft::VariableDeclaration(declaration) => {
                let declarators: Vec<String> = declaration
                    .declarations
                    .iter()
                    .map(|declarator| self.ser_declarator(declarator, depth + 1))
                    .collect();
                format!("(decl {})", declarators.join(" "))
            }
            ForStatementLeft::AssignmentTargetIdentifier(ident) => {
                format!("id:{}", self.ctx.reference(ident))
            }
            ForStatementLeft::StaticMemberExpression(member) => {
                self.ser_static_member(member, depth + 1)
            }
            ForStatementLeft::ComputedMemberExpression(member) => {
                self.ser_computed_member(member, depth + 1)
            }
            ForStatementLeft::PrivateFieldExpression(member) => {
                self.ser_private_field(member, depth + 1)
            }
            // Destructuring into an existing binding (`for ([a, b] of pairs)` reassigning, as
            // opposed to `for (const [a, b] of pairs)` declaring) is rare in test bodies; raw
            // text is the safe fallback.
            _ => self.opaque(left.span()),
        }
    }

    /// Render a declared binding pattern and any initializer expression.
    fn ser_declarator(&mut self, declarator: &VariableDeclarator<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(declarator.span);
        }
        let pattern = self.ser_binding_pattern(&declarator.id, depth + 1);
        match &declarator.init {
            Some(init) => format!("(decl {pattern} {})", self.ser_expr(init, depth + 1)),
            None => format!("(decl {pattern})"),
        }
    }

    /// Render the optional catch binding and ordered handler statements.
    fn ser_catch(&mut self, clause: &CatchClause<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(clause.span);
        }
        let param = match &clause.param {
            Some(param) => self.ser_binding_pattern(&param.pattern, depth + 1),
            None => "()".to_owned(),
        };
        format!(
            "(catch {param} {})",
            self.ser_stmts(&clause.body.body, depth + 1)
        )
    }

    /// Render a switch label or default marker followed by its consequent statements.
    fn ser_switch_case(&mut self, case: &SwitchCase<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(case.span);
        }
        let test = case.test.as_ref().map_or_else(
            || "default".to_owned(),
            |test| self.ser_expr(test, depth + 1),
        );
        let consequent: Vec<String> = case
            .consequent
            .iter()
            .map(|statement| self.ser_stmt(statement, depth + 1))
            .collect();
        format!("(case {test} {})", consequent.join(" "))
    }

    /// Render template quasis and embedded expressions, preserving quasi text even in
    /// structural mode.
    fn ser_template(&mut self, template: &TemplateLiteral<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(template.span);
        }
        // Quasi text is left intact even in structural mode: templates are string composition,
        // not a single literal value, and collapsing them risks merging templates with
        // meaningfully different static text. Documented under-match.
        let mut parts = Vec::new();
        for (index, quasi) in template.quasis.iter().enumerate() {
            parts.push(format!("`{}`", quasi.value.raw.as_str()));
            if let Some(expression) = template.expressions.get(index) {
                parts.push(self.ser_expr(expression, depth + 1));
            }
        }
        format!("(template {})", parts.join(" "))
    }

    /// Serialize supported expression structure using the current mode, with conservative
    /// fallback for depth or unsupported syntax.
    fn ser_expr(&mut self, expr: &Expression<'a>, depth: u32) -> String {
        #[cfg(test)]
        record_stack_low_water_mark();
        if depth > MAX_DEPTH {
            return self.degraded(expr.span());
        }
        match expr {
            Expression::BooleanLiteral(literal) => {
                self.literal("boolean", if literal.value { "true" } else { "false" })
            }
            Expression::NullLiteral(_) => self.literal("null", "null"),
            Expression::NumericLiteral(literal) => self.literal(
                "number",
                literal
                    .raw
                    .as_ref()
                    .map(|raw| raw.as_str())
                    .unwrap_or_default(),
            ),
            Expression::BigIntLiteral(literal) => {
                // A distinct `bigint` kind, not folded into `number`: an audit prompted by the
                // kind-erasure bug above found this module already had the same collision
                // one level down -- `100n` and `100` share digit text, and sharing the `number`
                // kind tag too would let them collide the same way `true`/`'true'` did. BigInt
                // and Number are different runtime types with different arithmetic, so even
                // `structural` keeps them apart rather than folding into the original five
                // literal kinds; that is strictly more precision, never less.
                self.literal("bigint", literal.value.as_str())
            }
            Expression::RegExpLiteral(literal) => self.literal(
                "regex",
                literal
                    .raw
                    .as_ref()
                    .map(|raw| raw.as_str())
                    .unwrap_or_default(),
            ),
            Expression::StringLiteral(literal) => self.literal("string", literal.value.as_str()),
            Expression::TemplateLiteral(template) => self.ser_template(template, depth + 1),
            Expression::Identifier(ident) => self.reference_or_literal(ident, depth),
            Expression::Super(_) => "super".to_owned(),
            Expression::ThisExpression(_) => {
                self.this_or_arguments = true;
                "this".to_owned()
            }
            Expression::ArrayExpression(array) => {
                let elements: Vec<String> = array
                    .elements
                    .iter()
                    .map(|element| self.ser_array_element(element, depth + 1))
                    .collect();
                format!("(array {})", elements.join(" "))
            }
            Expression::ObjectExpression(object) => {
                let properties: Vec<String> = object
                    .properties
                    .iter()
                    .map(|property| self.ser_object_property(property, depth + 1))
                    .collect();
                format!("(object {})", properties.join(" "))
            }
            Expression::ArrowFunctionExpression(arrow) => {
                self.ser_callable(&FunctionLike::Arrow(arrow), depth + 1)
            }
            Expression::FunctionExpression(function) => {
                self.ser_callable(&FunctionLike::Function(function), depth + 1)
            }
            Expression::CallExpression(call) => self.ser_call(call, depth + 1),
            Expression::NewExpression(new_expression) => self.ser_new(new_expression, depth + 1),
            Expression::AssignmentExpression(assignment) => format!(
                "(assign {} {} {})",
                assignment.operator.as_str(),
                self.ser_assignment_target(&assignment.left, depth + 1),
                self.ser_expr(&assignment.right, depth + 1)
            ),
            Expression::BinaryExpression(binary) => format!(
                "(bin {} {} {})",
                binary.operator.as_str(),
                self.ser_expr(&binary.left, depth + 1),
                self.ser_expr(&binary.right, depth + 1)
            ),
            Expression::LogicalExpression(logical) => format!(
                "(logic {} {} {})",
                logical.operator.as_str(),
                self.ser_expr(&logical.left, depth + 1),
                self.ser_expr(&logical.right, depth + 1)
            ),
            Expression::UnaryExpression(unary) => {
                format!(
                    "(unary {} {})",
                    unary.operator.as_str(),
                    self.ser_expr(&unary.argument, depth + 1)
                )
            }
            Expression::UpdateExpression(update) => {
                let target = self.ser_simple_assignment_target(&update.argument, depth + 1);
                format!(
                    "(update {} {} {target})",
                    update.operator.as_str(),
                    update.prefix
                )
            }
            Expression::ConditionalExpression(conditional) => format!(
                "(cond {} {} {})",
                self.ser_expr(&conditional.test, depth + 1),
                self.ser_expr(&conditional.consequent, depth + 1),
                self.ser_expr(&conditional.alternate, depth + 1)
            ),
            Expression::SequenceExpression(sequence) => {
                let parts: Vec<String> = sequence
                    .expressions
                    .iter()
                    .map(|expression| self.ser_expr(expression, depth + 1))
                    .collect();
                format!("(seq {})", parts.join(" "))
            }
            Expression::AwaitExpression(await_expression) => {
                format!(
                    "(await {})",
                    self.ser_expr(&await_expression.argument, depth + 1)
                )
            }
            Expression::YieldExpression(yield_expression) => match &yield_expression.argument {
                Some(argument) => format!(
                    "(yield{} {})",
                    if yield_expression.delegate { "*" } else { "" },
                    self.ser_expr(argument, depth + 1)
                ),
                None => format!(
                    "(yield{})",
                    if yield_expression.delegate { "*" } else { "" }
                ),
            },
            Expression::ChainExpression(chain) => self.ser_chain(chain, depth + 1),
            Expression::ParenthesizedExpression(parenthesized) => {
                // Parens carry no runtime identity; unwrapping avoids spurious mismatches between
                // `x` and `(x)`.
                self.ser_expr(&parenthesized.expression, depth + 1)
            }
            Expression::TSAsExpression(as_expression) => {
                self.ser_expr(&as_expression.expression, depth + 1)
            }
            Expression::TSSatisfiesExpression(satisfies) => {
                self.ser_expr(&satisfies.expression, depth + 1)
            }
            Expression::TSNonNullExpression(non_null) => {
                self.ser_expr(&non_null.expression, depth + 1)
            }
            Expression::TSTypeAssertion(assertion) => {
                self.ser_expr(&assertion.expression, depth + 1)
            }
            Expression::TSInstantiationExpression(instantiation) => {
                self.ser_instantiation(instantiation, depth + 1)
            }
            Expression::TaggedTemplateExpression(tagged) => {
                self.ser_tagged_template(tagged, depth + 1)
            }
            Expression::StaticMemberExpression(member) => self.ser_static_member(member, depth + 1),
            Expression::ComputedMemberExpression(member) => {
                self.ser_computed_member(member, depth + 1)
            }
            Expression::PrivateFieldExpression(member) => self.ser_private_field(member, depth + 1),
            // A dynamic `import()` names a module, and a module identity is exactly what this
            // scheme anchors everywhere else. Falling through to `opaque` rendered the verbatim
            // source slice instead, so two files each doing `await import('./recipes')` compared
            // EQUAL however differently their `./recipes` resolved -- the same defect
            // `fixtures/recall/cross-file-duplicate-same-specifier-different-module/` pins for
            // the STATIC form, which `import_origins` closed and this path never saw. Measured:
            // 380 of the corpus's 13,781 test files contain `await import(`.
            //
            // Only a static string specifier can be resolved; `import(name)` keeps the
            // conservative fallback, because the module it names is not knowable here.
            Expression::ImportExpression(import) => match &import.source {
                Expression::StringLiteral(specifier) => format!(
                    "(import {})",
                    specifier_identity(&self.ctx.file, specifier.value.as_str())
                ),
                _ => self.opaque(expr.span()),
            },
            // JSX is serialized structurally (`ser_jsx_element`/`ser_jsx_fragment`): the verbatim
            // slice this arm used to emit made the comparison textual, so a formatter reindenting
            // one element broke a match between two bodies that render identically, and a literal
            // inside an attribute was not a literal to the structural digest at all.
            Expression::JSXElement(element) => self.ser_jsx_element(element, depth + 1),
            Expression::JSXFragment(fragment) => self.ser_jsx_fragment(fragment, depth + 1),
            // Decorators, `import.meta`/`new.target`, private `in`, V8 intrinsics, and any other
            // exotic expression form: raw text. `opaque` emits the verbatim source slice, so
            // textually different forms DO compare unequal; what it loses is alpha-renaming and
            // import anchoring inside the slice, and it is whitespace-sensitive.
            _ => self.opaque(expr.span()),
        }
    }

    /// Render a named member access with the receiver and optional-chain distinction.
    fn ser_static_member(&mut self, member: &StaticMemberExpression<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(member.span);
        }
        let object = self.ser_expr(&member.object, depth + 1);
        let optional = if member.optional { "?." } else { "." };
        format!(
            "(member {object} {optional}{})",
            member.property.name.as_str()
        )
    }

    /// Render an indexed access, preserving both its key expression and optional-chain flag.
    fn ser_computed_member(&mut self, member: &ComputedMemberExpression<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(member.span);
        }
        let object = self.ser_expr(&member.object, depth + 1);
        let property = self.ser_expr(&member.expression, depth + 1);
        // `?.[k]` is not `[k]`: the first yields `undefined` on a nullish receiver, the second
        // throws. Two tests differing only in that guard are different tests, and without this
        // they serialized identically and were reported as `duplicate-test` at ERROR severity.
        //
        // Both siblings already recorded it -- `ser_static_member` emits `.b` against `?.b`, and
        // `ser_call` emits `(call` against `(call?.` -- so this was the one member form that did
        // not, which is the "fixed at one site, not its siblings" shape this repository keeps
        // producing rather than a decision anyone made.
        let optional = if member.optional { "?" } else { "" };
        format!("(index{optional} {object} {property})")
    }

    /// Render a private-field access with its receiver and private member name.
    fn ser_private_field(&mut self, member: &PrivateFieldExpression<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(member.span);
        }
        let object = self.ser_expr(&member.object, depth + 1);
        format!("(member {object} .#{})", member.field.name.as_str())
    }

    /// A JSX element, structurally: name, attributes in source order, children in source order.
    ///
    /// Why structural and not the verbatim slice [`Serializer::opaque`] used to emit here: JSX in a
    /// test body is an argument to `render(...)`/`mount(...)`, and what the test observes is the
    /// RENDERED element -- which a formatter's reindenting does not change, and which an attribute
    /// literal participates in as a *value*. The slice made the comparison textual (reindent broke
    /// every match) and hid attribute literals from `structural`'s kind-collapse (the
    /// `parameterization-candidate` family could never see them). Every sub-part reuses the
    /// serializer that already exists for it: component names go through the same
    /// reference-identity machinery plain identifiers do (so two different components imported
    /// from one module stay distinct, and the same component resolved from two files can match),
    /// attribute literals through [`Serializer::literal`] (so alpha keeps the value and structural
    /// keeps the kind), and expression children/attribute values through [`Serializer::ser_expr`].
    ///
    /// `self_closing` is deliberately NOT recorded: `<Button />` and `<Button></Button>` render
    /// identically, which is the relation this serializer claims.
    ///
    /// Kept out of [`Serializer::ser_expr`]'s match for the stack-headroom reason
    /// [`Serializer::ser_new`]'s doc records, like every other wide arm.
    fn ser_jsx_element(&mut self, element: &JSXElement<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(element.span);
        }
        let name = self.ser_jsx_name(&element.opening_element.name, depth + 1);
        let type_arguments =
            self.ser_type_arguments(element.opening_element.type_arguments.as_deref());
        let attributes: Vec<String> = element
            .opening_element
            .attributes
            .iter()
            .map(|attribute| self.ser_jsx_attribute_item(attribute, depth + 1))
            .collect();
        let children = self.ser_jsx_children(&element.children, depth + 1);
        let mut parts = Vec::with_capacity(2 + attributes.len());
        parts.push(format!("{name}{type_arguments}"));
        parts.extend(attributes);
        parts.push(children);
        format!("(jsx-elem {})", parts.join(" "))
    }

    /// A JSX fragment: children only, since fragments carry no name or attributes.
    fn ser_jsx_fragment(&mut self, fragment: &JSXFragment<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(fragment.span);
        }
        format!(
            "(jsx-frag {})",
            self.ser_jsx_children(&fragment.children, depth + 1)
        )
    }

    /// An element name. The two identifier variants are disjoint by JSX's own grammar (a lowercase
    /// tag is an intrinsic element, a capitalized one a component reference), and are kept distinct
    /// by prefix so `<div>` can never collide with a local binding spelled the same way.
    fn ser_jsx_name(&mut self, name: &JSXElementName<'a>, depth: u32) -> String {
        match name {
            JSXElementName::Identifier(identifier) => {
                format!("intrinsic:{}", identifier.name.as_str())
            }
            JSXElementName::IdentifierReference(ident) => {
                format!("component:{}", self.ctx.reference(ident))
            }
            JSXElementName::NamespacedName(namespaced) => format!(
                "jsx-ns:{}:{}",
                namespaced.namespace.name.as_str(),
                namespaced.name.name.as_str()
            ),
            JSXElementName::MemberExpression(member) => {
                let object = self.ser_jsx_member_object(&member.object, depth + 1);
                format!("(jsx-member {object} .{})", member.property.name.as_str())
            }
            JSXElementName::ThisExpression(_) => {
                // `<this.foo />` reads the enclosing function's `this`, exactly as `this.foo`
                // outside JSX would -- see [`Serializer::this_or_arguments`].
                self.this_or_arguments = true;
                "this".to_owned()
            }
        }
    }

    /// Render nested JSX member objects using component reference identities.
    fn ser_jsx_member_object(
        &mut self,
        object: &JSXMemberExpressionObject<'a>,
        depth: u32,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(object.span());
        }
        match object {
            // `component:` for the same reason [`Serializer::ser_jsx_name`]'s identifier-reference
            // arm carries it: this identifier is a component reference, and the prefix keeps it
            // from colliding with the same name read as an intrinsic.
            JSXMemberExpressionObject::IdentifierReference(ident) => {
                format!("component:{}", self.ctx.reference(ident))
            }
            JSXMemberExpressionObject::MemberExpression(member) => {
                let object = self.ser_jsx_member_object(&member.object, depth + 1);
                format!("(jsx-member {object} .{})", member.property.name.as_str())
            }
            JSXMemberExpressionObject::ThisExpression(_) => {
                self.this_or_arguments = true;
                "this".to_owned()
            }
        }
    }

    /// One attribute: `(jsx-attr <name> <value>)`, or `(jsx-attr <name>)` for the valueless
    /// boolean form (`<button disabled>`), or a spread. Attribute names are JSX identifiers --
    /// identifier characters plus `-` -- so a name can never contain the spaces and parentheses
    /// of the value's serialization and the two cannot run together.
    fn ser_jsx_attribute_item(&mut self, attribute: &JSXAttributeItem<'a>, depth: u32) -> String {
        match attribute {
            JSXAttributeItem::Attribute(attribute) => {
                let name = match &attribute.name {
                    JSXAttributeName::Identifier(identifier) => identifier.name.as_str().to_owned(),
                    JSXAttributeName::NamespacedName(namespaced) => format!(
                        "{}:{}",
                        namespaced.namespace.name.as_str(),
                        namespaced.name.name.as_str()
                    ),
                };
                match &attribute.value {
                    None => format!("(jsx-attr {name})"),
                    Some(value) => format!(
                        "(jsx-attr {name} {})",
                        self.ser_jsx_attribute_value(value, depth + 1)
                    ),
                }
            }
            JSXAttributeItem::SpreadAttribute(spread) => format!(
                "(jsx-spread-attr {})",
                self.ser_expr(&spread.argument, depth + 1)
            ),
        }
    }

    /// Serialize JSX string, expression, element and fragment attribute values in the current
    /// fingerprint mode.
    fn ser_jsx_attribute_value(&mut self, value: &JSXAttributeValue<'a>, depth: u32) -> String {
        match value {
            // A plain string attribute is where the `parameterization-candidate` family was
            // blind: routed through `literal`, so `alpha` keeps the value (`variant="primary"` vs
            // `variant="secondary"` differ) while `structural` collapses to the kind.
            JSXAttributeValue::StringLiteral(literal) => {
                self.literal("string", literal.value.as_str())
            }
            JSXAttributeValue::ExpressionContainer(container) => {
                match container.expression.as_expression() {
                    Some(expression) => self.ser_expr(expression, depth + 1),
                    // `attr={}` is not a value a renderer can observe differently from no value.
                    None => "(jsx-empty)".to_owned(),
                }
            }
            JSXAttributeValue::Element(element) => self.ser_jsx_element(element, depth + 1),
            JSXAttributeValue::Fragment(fragment) => self.ser_jsx_fragment(fragment, depth + 1),
        }
    }

    /// Children, space-joined. Whitespace-only text children contribute NOTHING rather than a
    /// normalized-empty string, because JSX itself drops them: the whitespace between a tag and
    /// its first nested element is not a child any renderer observes.
    fn ser_jsx_children(&mut self, children: &[JSXChild<'a>], depth: u32) -> String {
        let parts: Vec<String> = children
            .iter()
            .filter_map(|child| self.ser_jsx_child(child, depth + 1))
            .collect();
        parts.join(" ")
    }

    /// Render a meaningful JSX child, declining empty text and empty expression containers.
    fn ser_jsx_child(&mut self, child: &JSXChild<'a>, depth: u32) -> Option<String> {
        match child {
            JSXChild::Text(text) => {
                let normalized = normalize_jsx_text(text.value.as_str());
                (!normalized.is_empty())
                    .then(|| format!("(jsx-text {})", self.literal("string", &normalized)))
            }
            JSXChild::Element(element) => Some(self.ser_jsx_element(element, depth + 1)),
            JSXChild::Fragment(fragment) => Some(self.ser_jsx_fragment(fragment, depth + 1)),
            // An empty expression container `{}` renders nothing, so it is not a child.
            JSXChild::ExpressionContainer(container) => container
                .expression
                .as_expression()
                .map(|expression| format!("(jsx-expr {})", self.ser_expr(expression, depth + 1))),
            JSXChild::Spread(spread) => Some(format!(
                "(jsx-spread-children {})",
                self.ser_expr(&spread.expression, depth + 1)
            )),
        }
    }

    /// An optional-chain fragment: the `?.`-linked element list of `a?.b.c()`.
    fn ser_chain(&mut self, chain: &ChainExpression<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(chain.span);
        }
        match &chain.expression {
            ChainElement::CallExpression(call) => self.ser_call(call, depth + 1),
            ChainElement::TSNonNullExpression(non_null) => {
                self.ser_expr(&non_null.expression, depth + 1)
            }
            ChainElement::StaticMemberExpression(member) => {
                self.ser_static_member(member, depth + 1)
            }
            ChainElement::ComputedMemberExpression(member) => {
                self.ser_computed_member(member, depth + 1)
            }
            ChainElement::PrivateFieldExpression(member) => {
                self.ser_private_field(member, depth + 1)
            }
        }
    }

    /// `(call callee args...)`, with an imported-helper call site rendered as the helper's own
    /// body instead when `try_inline_imported_helper` admits it (`alpha` only).
    fn ser_call(&mut self, call: &CallExpression<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(call.span);
        }
        if let Some(inlined) = self.try_inline_imported_helper(call, depth) {
            return inlined;
        }
        let callee = self.ser_expr(&call.callee, depth + 1);
        let type_arguments = self.ser_type_arguments(call.type_arguments.as_deref());
        let arguments: Vec<String> = call
            .arguments
            .iter()
            .map(|argument| self.ser_argument(argument, depth + 1))
            .collect();
        let optional = if call.optional { "?." } else { "" };
        format!(
            "(call{optional} {callee}{type_arguments} {})",
            arguments.join(" ")
        )
    }

    /// Imported-helper inlining, the `alpha`-only half (see `helpers.rs` for the other half:
    /// which file and declaration an import names, and the guards). A call whose callee is a
    /// bare identifier resolving to an import binding renders the *helper's own body* in place of
    /// the call, with each parameter read rendered as the argument text this pass already
    /// serialized for it -- so two tests differing only by calling equivalent helpers compare by
    /// what the calls do rather than by which exports they name.
    ///
    /// **`alpha` only, deliberately.** `structural` is `parameterization-candidate`'s key, and
    /// its remedy -- a `test.each` table in the *test file* -- can only parameterize literals
    /// written in the test bodies. Inlined into `structural`, a pair whose differing literals
    /// live inside two helper modules (measured: this exact fixture,
    /// `duplicate-test-imported-helper-differs`) would collapse to one table proposal whose
    /// suggested fix is impossible to write, which is the same reason type arguments survive in
    /// `structural` (see [`Serializer::ser_type_arguments`]). `structural` therefore keeps the
    /// pre-inlining rendering byte-for-byte, and the rule's population is untouched by this
    /// feature.
    ///
    /// Every refusal is silent and lands on today's exact rendering (an `id:free:import:...`
    /// call), because a refusal is an under-match, not a defect: the framework itself
    /// (`specifier == "vitest"`, the same text the registry keys its own framework check on), an
    /// unresolvable import, spread arguments, an arity the helper's parameters cannot bind
    /// exactly, or a helper file/export shape `helpers.rs` does not model. See that module for
    /// why each guard exists.
    ///
    /// Arguments render *before* the helper is asked for its body, so a refused inline wastes
    /// serialization but never changes the outcome: canonical naming is assigned on first
    /// encounter and re-encounter reuses it, so rendering the arguments twice is idempotent.
    fn try_inline_imported_helper(
        &mut self,
        call: &CallExpression<'a>,
        depth: u32,
    ) -> Option<String> {
        if !self.ctx.inline_imported_helpers || matches!(self.mode, Mode::Structural) {
            return None;
        }
        let Expression::Identifier(ident) = &call.callee else {
            return None;
        };
        // Resolve the callee to *the import binding* by symbol identity, not by name -- the same
        // resolution `free_identity` performs for the identity rendering, so a local shadowing
        // an import's name can never inline through the import's origin.
        let symbol_id = ident.reference_id.get().and_then(|reference_id| {
            self.ctx
                .semantic
                .scoping()
                .get_reference(reference_id)
                .symbol_id()
        })?;
        if !self
            .ctx
            .semantic
            .scoping()
            .symbol_flags(symbol_id)
            .is_import()
        {
            return None;
        }
        let origin = self.ctx.imports.get(ident.name.as_str())?;
        let has_spread = call
            .arguments
            .iter()
            .any(|argument| matches!(argument, Argument::SpreadElement(_)));
        if !super::helpers::would_inline(origin, call.arguments.len(), has_spread) {
            return None;
        }
        let arguments: Vec<String> = call
            .arguments
            .iter()
            .map(|argument| self.ser_argument(argument, depth + 1))
            .collect();
        let rendered = super::helpers::render_inline(origin, &arguments, self.mode, depth);
        // Accounting: only a render that actually happened counts as inlined (see
        // `FingerprintContext::inlined_imports`). A refusal here leaves the module unrecorded,
        // and the callee then renders as an identity through `reference` below, which records
        // the other half.
        if rendered.is_some() {
            self.ctx.inlined_imports.insert(origin.component.clone());
        }
        rendered
    }

    /// `new Foo<T>(...)`.
    ///
    /// Lives in its own method rather than inline in [`Serializer::ser_expr`]'s match, like
    /// [`Serializer::ser_call`] and the member-expression arms already do. That is load-bearing,
    /// not tidiness: `ser_expr` recurses to [`MAX_DEPTH`], an unoptimized build gives every arm's
    /// locals their own stack slot in one shared frame, and the frame was already close enough to
    /// a default test thread's stack that adding a couple of `String` locals to three arms
    /// overflowed it (caught by `ser_expr_depth_guard_bounds_an_ast_deeper_than_the_parser_could_produce`).
    /// Keeping wide arms out of `ser_expr` keeps that headroom.
    fn ser_new(&mut self, new_expression: &NewExpression<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(new_expression.span);
        }
        let callee = self.ser_expr(&new_expression.callee, depth + 1);
        let type_arguments = self.ser_type_arguments(new_expression.type_arguments.as_deref());
        let arguments: Vec<String> = new_expression
            .arguments
            .iter()
            .map(|argument| self.ser_argument(argument, depth + 1))
            .collect();
        format!("(new {callee}{type_arguments} {})", arguments.join(" "))
    }

    /// `` tag<T>`...` `` -- see [`Serializer::ser_new`] on why this is not inline.
    fn ser_tagged_template(&mut self, tagged: &TaggedTemplateExpression<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(tagged.span);
        }
        let tag = self.ser_expr(&tagged.tag, depth + 1);
        let type_arguments = self.ser_type_arguments(tagged.type_arguments.as_deref());
        let quasi = self.ser_template(&tagged.quasi, depth + 1);
        format!("(tagged {tag}{type_arguments} {quasi})")
    }

    /// `foo<string>` with no call: an instantiation expression is the one construct here whose
    /// *entire* content is a type-argument list, so unwrapping it to its inner expression (as the
    /// other TS-only wrappers are unwrapped, correctly, because they carry no identity at all)
    /// erased the only thing it says. It is kept as its own node rather than folded into the inner
    /// expression's serialization so that `foo` and `foo<string>` -- genuinely different
    /// expressions -- can never render alike.
    fn ser_instantiation(
        &mut self,
        instantiation: &TSInstantiationExpression<'a>,
        depth: u32,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(instantiation.span);
        }
        let expression = self.ser_expr(&instantiation.expression, depth + 1);
        let type_arguments = self.ser_type_arguments(Some(&instantiation.type_arguments));
        format!("(instantiate {expression}{type_arguments})")
    }

    /// Render an expression or spread call argument, retaining opaque unsupported forms.
    fn ser_argument(&mut self, argument: &Argument<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(argument.span());
        }
        match argument.as_expression() {
            Some(expression) => self.ser_expr(expression, depth + 1),
            None => match argument {
                Argument::SpreadElement(spread) => {
                    format!("(spread {})", self.ser_expr(&spread.argument, depth + 1))
                }
                _ => self.opaque(argument.span()),
            },
        }
    }

    /// Render array values, spreads and holes without conflating their shapes.
    fn ser_array_element(&mut self, element: &ArrayExpressionElement<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(element.span());
        }
        match element.as_expression() {
            Some(expression) => self.ser_expr(expression, depth + 1),
            None => match element {
                ArrayExpressionElement::SpreadElement(spread) => {
                    format!("(spread {})", self.ser_expr(&spread.argument, depth + 1))
                }
                ArrayExpressionElement::Elision(_) => "<hole>".to_owned(),
                _ => self.opaque(element.span()),
            },
        }
    }

    /// Render object key/value pairs and spread properties in source order.
    fn ser_object_property(&mut self, property: &ObjectPropertyKind<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(property.span());
        }
        match property {
            ObjectPropertyKind::ObjectProperty(object_property) => {
                let key = self.ser_property_key(&object_property.key, depth + 1);
                let value = self.ser_expr(&object_property.value, depth + 1);
                format!("(prop {key} {value})")
            }
            ObjectPropertyKind::SpreadProperty(spread) => {
                format!("(spread {})", self.ser_expr(&spread.argument, depth + 1))
            }
        }
    }

    /// Serializes the write-target of an assignment.
    ///
    /// Only identifiers and member expressions are modeled explicitly; destructuring assignment
    /// targets (`[a, b] = arr`, `({a} = obj)`, as opposed to a `const`/`let` destructuring
    /// *declaration*, which goes through `ser_binding_pattern` instead) fall back to raw text.
    /// This is a narrow, rare construct in test bodies, and raw text keeps the fallback safe.
    fn ser_assignment_target(&mut self, target: &AssignmentTarget<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(target.span());
        }
        // `AssignmentTarget` carries every `SimpleAssignmentTarget` variant inline, so the two
        // families were serialized by two matches listing the same eight arms. Adding the four TS
        // wrappers to both is what made that a clone large enough to take production Rust to its
        // duplication ceiling -- and it is the same defect shape those wrappers were added to fix:
        // one construct handled twice, in two places that can drift apart. Delegating means there
        // is one list to get right.
        match target.as_simple_assignment_target() {
            Some(simple) => self.ser_simple_assignment_target(simple, depth),
            // What remains is the destructuring family (array and object patterns), which this
            // module does not model.
            None => self.opaque(target.span()),
        }
    }

    /// `UpdateExpression::argument` is a `SimpleAssignmentTarget`, a strict subset of
    /// `AssignmentTarget`'s variants; reuse the same handling for the variants they share.
    fn ser_simple_assignment_target(
        &mut self,
        target: &SimpleAssignmentTarget<'a>,
        depth: u32,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(target.span());
        }
        match target {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) => {
                format!("id:{}", self.ctx.reference(ident))
            }
            SimpleAssignmentTarget::StaticMemberExpression(member) => {
                self.ser_static_member(member, depth + 1)
            }
            SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                self.ser_computed_member(member, depth + 1)
            }
            SimpleAssignmentTarget::PrivateFieldExpression(member) => {
                self.ser_private_field(member, depth + 1)
            }
            // The same four TS wrappers `ser_expr` already unwraps in READ position. Without
            // these, `(o.b as any)++` rendered `<opaque:o.b as any>` while a bare `o.b` in the
            // same file rendered its member path -- one construct transparent when read and
            // opaque when written, so a type assertion on the left-hand side silently stopped
            // two otherwise-identical bodies from matching. Type syntax carries no runtime
            // identity, which is the whole reason the read side unwraps it.
            SimpleAssignmentTarget::TSAsExpression(as_expression) => {
                self.ser_expr(&as_expression.expression, depth + 1)
            }
            SimpleAssignmentTarget::TSSatisfiesExpression(satisfies) => {
                self.ser_expr(&satisfies.expression, depth + 1)
            }
            SimpleAssignmentTarget::TSNonNullExpression(non_null) => {
                self.ser_expr(&non_null.expression, depth + 1)
            }
            SimpleAssignmentTarget::TSTypeAssertion(assertion) => {
                self.ser_expr(&assertion.expression, depth + 1)
            } // No catch-all: with the four TS wrappers above, `SimpleAssignmentTarget` is matched
              // exhaustively, and clippy says so (`unreachable pattern`). Leaving a dead
              // `_ => opaque` here would be a fallback that can never run pretending to be a
              // safety net -- and would silently swallow a variant a future oxc adds, which is
              // exactly what an exhaustive match is for.
        }
    }
}

// ---------------------------------------------------------------------------------------------
// behavior events
// ---------------------------------------------------------------------------------------------

/// Collects the ordered behavior signature: canonical callee paths, assertions, awaits, and
/// control-flow markers, in source order.
struct BehaviorCollector<'a, 'b, 's, 'm> {
    /// Everything both traversals know about the file, plus the shared canonical-naming state;
    /// see [`FingerprintContext`].
    ctx: FingerprintContext<'a, 'b, 's, 'm>,
    events: Vec<String>,
}

impl<'a, 'b, 's, 'm> BehaviorCollector<'a, 'b, 's, 'm> {
    /// Initialize a behavior collector with the supplied fingerprint context and no events.
    fn new(ctx: FingerprintContext<'a, 'b, 's, 'm>) -> Self {
        Self {
            ctx,
            events: Vec::new(),
        }
    }

    /// Collect ordered behavior events from a callable body or concise arrow expression.
    fn run(mut self, body: &FunctionLike<'a, '_>) -> Vec<String> {
        match body {
            FunctionLike::Arrow(arrow) => match &arrow.body {
                ArrowFunctionBody::FunctionBody(function_body) => {
                    for statement in &function_body.statements {
                        self.walk_stmt(statement, false, 0);
                    }
                }
                other => {
                    if let Some(expression) = other.as_expression() {
                        // `() => <call>().then(callback)` -- the expression-bodied-arrow twin of
                        // the `return <call>().then(callback)` case `walk_stmt` handles; see
                        // `then_continuation`'s doc.
                        if let Expression::CallExpression(call) = expression {
                            if let Some((awaited, continuation)) = Self::then_continuation(call) {
                                self.push(false, "await".to_owned());
                                self.walk_expr(awaited, false, 1);
                                for statement in continuation {
                                    self.walk_stmt(statement, false, 1);
                                }
                                return self.events;
                            }
                        }
                        self.walk_expr(expression, false, 0);
                    }
                }
            },
            FunctionLike::Function(function) => {
                if let Some(function_body) = &function.body {
                    for statement in &function_body.statements {
                        self.walk_stmt(statement, false, 0);
                    }
                }
            }
        }
        self.events
    }

    /// Whether a `const` declarator's own binding is fully absorbed by
    /// [`DataflowSubst::Inline`] -- only ever true for a plain `BindingIdentifier` (destructuring
    /// never registers an `Inline` entry; see `analyze_dataflow`), so this deliberately does not
    /// recurse into `ObjectPattern`/`ArrayPattern` the way `Serializer::binding_fully_substituted`
    /// does for row substitution: a destructured property's own read is canonicalized at its read
    /// site, but its *declaration* statement (`const { a } = o`) contributes no event either way
    /// (see the module doc's "Traversal shape" -- a bare identifier read, which is all `o`
    /// contributes here, was never an event to begin with), so there is nothing to suppress.
    fn declarator_fully_inlined(&self, id: &BindingPattern<'a>) -> bool {
        matches!(id, BindingPattern::BindingIdentifier(binding)
        if binding.symbol_id.get().is_some_and(|symbol_id| {
            matches!(
                self.ctx.dataflow.get(&symbol_id),
                Some(DataflowSubst::Inline { .. })
            )
        }))
    }

    /// Recognizes `<call>().then(callback)`: a single awaited call chained straight into its own
    /// continuation, expressed as a promise chain instead of `await`. Returns the awaited call
    /// expression and the callback's own statement list when the shape qualifies for the
    /// `await`/`.then` canonicalization; `None` for anything else -- every arm below is a
    /// precondition, not a parsing convenience. The
    /// transformation's stated precondition is "a single awaited call, with no interleaving", and
    /// each arm is one half of checking it:
    ///
    /// - `call.arguments` must be a single argument: a second argument is a rejection handler
    ///   (`.then(onFulfilled, onRejected)`), and that shape observes a DIFFERENT code path than a
    ///   bare `await` ever can (an `await` that rejects throws, it does not call a second
    ///   function), so it is never equivalent -- the one precondition this transformation was
    ///   required to verify rather than assume.
    /// - the object being `.then`-ed must itself be a plain `CallExpression`, not another `.then`
    ///   chain (`x().then(a).then(b)`) -- chaining means the continuation depends on what `a`
    ///   returns, which is not "a single awaited call".
    /// - the callback must be a zero-parameter arrow or `function` expression with a block body:
    ///   a parameter would receive the resolved value, which this module does not attempt to
    ///   correlate with anything (a missed finding, not a soundness risk); an expression-bodied
    ///   arrow (`.then(() => x)`) has no statement list to splice, so it is left alone too.
    fn then_continuation<'x>(
        call: &'x CallExpression<'a>,
    ) -> Option<(&'x Expression<'a>, &'x [Statement<'a>])> {
        let Expression::StaticMemberExpression(member) = &call.callee else {
            return None;
        };
        if member.property.name.as_str() != "then" {
            return None;
        }
        let Expression::CallExpression(inner) = &member.object else {
            return None;
        };
        if let Expression::StaticMemberExpression(inner_member) = &inner.callee {
            if inner_member.property.name.as_str() == "then" {
                return None; // a chained `.then().then()`: not "a single awaited call".
            }
        }
        let [argument] = call.arguments.as_slice() else {
            return None; // zero, or two-or-more (a rejection handler): never equivalent.
        };
        let callback = argument.as_expression()?;
        let statements: &'x [Statement<'a>] = match callback {
            Expression::ArrowFunctionExpression(arrow)
                if arrow.params.items.is_empty() && arrow.params.rest.is_none() =>
            {
                match &arrow.body {
                    ArrowFunctionBody::FunctionBody(function_body) => &function_body.statements,
                    _ => return None,
                }
            }
            Expression::FunctionExpression(function)
                if function.params.items.is_empty() && function.params.rest.is_none() =>
            {
                function.body.as_ref()?.statements.as_slice()
            }
            _ => return None,
        };
        Some((&member.object, statements))
    }

    /// Append a behavior event, prefixing it when its execution is deferred.
    fn push(&mut self, deferred: bool, event: String) {
        self.events.push(if deferred {
            format!("deferred-{event}")
        } else {
            event
        });
    }

    /// Collect statement behavior within the depth bound, carrying deferred-execution state
    /// into nested expressions.
    fn walk_stmt(&mut self, statement: &Statement<'a>, deferred: bool, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        match statement {
            Statement::BlockStatement(block) => {
                for inner in &block.body {
                    self.walk_stmt(inner, deferred, depth + 1);
                }
            }
            Statement::ExpressionStatement(expression_statement) => {
                self.walk_expr(&expression_statement.expression, deferred, depth + 1);
            }
            Statement::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    if let Some(init) = &declarator.init {
                        // A local fully absorbed into "single-use local inlining" (see
                        // `DataflowSubst::Inline`) contributes no event of its own here -- its one
                        // read accounts for this same initializer expression instead, at that read
                        // site: rendered into the assertion's shape when the read is an assertion
                        // subject/value, and re-walked for its own events by `walk_inlined_read`
                        // everywhere else. Walking it here too would double-count it (e.g. its own
                        // `call:` event, once here and once at the read), which is exactly the
                        // mismatch that would stop the inlined and the written-out-twice bodies
                        // from converging.
                        if self.declarator_fully_inlined(&declarator.id) {
                            continue;
                        }
                        self.walk_expr(init, deferred, depth + 1);
                    }
                }
            }
            Statement::IfStatement(if_statement) => {
                self.push(deferred, "if".to_owned());
                self.walk_expr(&if_statement.test, deferred, depth + 1);
                self.walk_stmt(&if_statement.consequent, deferred, depth + 1);
                if let Some(alternate) = &if_statement.alternate {
                    self.walk_stmt(alternate, deferred, depth + 1);
                }
            }
            Statement::ForStatement(for_statement) => {
                self.push(deferred, "loop".to_owned());
                if let Some(test) = &for_statement.test {
                    self.walk_expr(test, deferred, depth + 1);
                }
                if let Some(update) = &for_statement.update {
                    self.walk_expr(update, deferred, depth + 1);
                }
                self.walk_stmt(&for_statement.body, deferred, depth + 1);
            }
            Statement::ForInStatement(for_in) => {
                self.push(deferred, "loop".to_owned());
                self.walk_expr(&for_in.right, deferred, depth + 1);
                self.walk_stmt(&for_in.body, deferred, depth + 1);
            }
            Statement::ForOfStatement(for_of) => {
                self.push(deferred, "loop".to_owned());
                self.walk_expr(&for_of.right, deferred, depth + 1);
                self.walk_stmt(&for_of.body, deferred, depth + 1);
            }
            Statement::WhileStatement(while_statement) => {
                self.push(deferred, "loop".to_owned());
                self.walk_expr(&while_statement.test, deferred, depth + 1);
                self.walk_stmt(&while_statement.body, deferred, depth + 1);
            }
            Statement::DoWhileStatement(do_while) => {
                self.push(deferred, "loop".to_owned());
                self.walk_stmt(&do_while.body, deferred, depth + 1);
                self.walk_expr(&do_while.test, deferred, depth + 1);
            }
            Statement::TryStatement(try_statement) => {
                self.push(deferred, "try".to_owned());
                for inner in &try_statement.block.body {
                    self.walk_stmt(inner, deferred, depth + 1);
                }
                if let Some(handler) = &try_statement.handler {
                    for inner in &handler.body.body {
                        self.walk_stmt(inner, deferred, depth + 1);
                    }
                }
                if let Some(finalizer) = &try_statement.finalizer {
                    for inner in &finalizer.body {
                        self.walk_stmt(inner, deferred, depth + 1);
                    }
                }
            }
            Statement::SwitchStatement(switch_statement) => {
                self.walk_expr(&switch_statement.discriminant, deferred, depth + 1);
                for case in &switch_statement.cases {
                    if let Some(test) = &case.test {
                        self.walk_expr(test, deferred, depth + 1);
                    }
                    for inner in &case.consequent {
                        self.walk_stmt(inner, deferred, depth + 1);
                    }
                }
            }
            Statement::ReturnStatement(return_statement) => {
                // `return <call>().then(callback)` -- see `then_continuation`'s doc: a resolved
                // promise chain returned from a test body is what Vitest itself awaits, so this
                // canonicalizes to the same event shape a bare `await <call>(); <callback body>`
                // would produce, rather than the ordinary `return` + `call:...then` + a `deferred-`
                // callback body this would otherwise collect.
                if let Some(Expression::CallExpression(call)) = &return_statement.argument {
                    if let Some((awaited, continuation)) = Self::then_continuation(call) {
                        self.push(deferred, "await".to_owned());
                        self.walk_expr(awaited, deferred, depth + 1);
                        for inner in continuation {
                            self.walk_stmt(inner, deferred, depth + 1);
                        }
                        return;
                    }
                }
                self.push(deferred, "return".to_owned());
                if let Some(argument) = &return_statement.argument {
                    self.walk_expr(argument, deferred, depth + 1);
                }
            }
            Statement::ThrowStatement(throw_statement) => {
                self.push(deferred, "throw".to_owned());
                self.walk_expr(&throw_statement.argument, deferred, depth + 1);
            }
            Statement::LabeledStatement(labeled) => {
                self.walk_stmt(&labeled.body, deferred, depth + 1)
            }
            Statement::FunctionDeclaration(function) => {
                if let Some(function_body) = &function.body {
                    for inner in &function_body.statements {
                        self.walk_stmt(inner, true, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }

    /// Collect expression behavior while distinguishing enclosing execution from deferred
    /// nested callables.
    fn walk_expr(&mut self, expr: &Expression<'a>, deferred: bool, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        match expr {
            Expression::CallExpression(call) => self.walk_call(call, deferred, depth + 1),
            Expression::AwaitExpression(await_expression) => {
                self.push(deferred, "await".to_owned());
                self.walk_expr(&await_expression.argument, deferred, depth + 1);
            }
            // A nested function/arrow used as a value (a callback passed elsewhere, a helper
            // assigned to a variable) is retained as syntactic evidence but not promoted to
            // directly-executed behavior: this body may never run, or may run many times, so its
            // calls and assertions are marked `deferred-` rather than treated as equivalent to
            // the enclosing test's own top-level behavior. This module treats every nested
            // function-like uniformly this way, unlike CukeDedup's narrower "only if passed as a
            // call argument" heuristic -- a deliberate simplification given how rarely a bare,
            // never-invoked nested function declaration appears in a test body.
            Expression::ArrowFunctionExpression(arrow) => match &arrow.body {
                ArrowFunctionBody::FunctionBody(function_body) => {
                    for statement in &function_body.statements {
                        self.walk_stmt(statement, true, depth + 1);
                    }
                }
                other => {
                    if let Some(inner) = other.as_expression() {
                        self.walk_expr(inner, true, depth + 1);
                    }
                }
            },
            Expression::FunctionExpression(function) => {
                if let Some(function_body) = &function.body {
                    for statement in &function_body.statements {
                        self.walk_stmt(statement, true, depth + 1);
                    }
                }
            }
            Expression::StaticMemberExpression(member) => {
                self.walk_member_value(member, deferred, depth + 1)
            }
            Expression::ComputedMemberExpression(member) => {
                self.walk_expr(&member.object, deferred, depth + 1);
                self.walk_expr(&member.expression, deferred, depth + 1);
            }
            Expression::PrivateFieldExpression(member) => {
                self.walk_expr(&member.object, deferred, depth + 1)
            }
            Expression::BinaryExpression(binary) => {
                self.walk_expr(&binary.left, deferred, depth + 1);
                self.walk_expr(&binary.right, deferred, depth + 1);
            }
            Expression::LogicalExpression(logical) => {
                self.walk_expr(&logical.left, deferred, depth + 1);
                self.walk_expr(&logical.right, deferred, depth + 1);
            }
            Expression::UnaryExpression(unary) => {
                self.walk_expr(&unary.argument, deferred, depth + 1)
            }
            Expression::ConditionalExpression(conditional) => {
                self.walk_expr(&conditional.test, deferred, depth + 1);
                self.walk_expr(&conditional.consequent, deferred, depth + 1);
                self.walk_expr(&conditional.alternate, deferred, depth + 1);
            }
            Expression::AssignmentExpression(assignment) => {
                let target = self.assignment_target_shape(&assignment.left, depth + 1);
                let value = self.shape(&assignment.right, Mode::Alpha, depth + 1);
                self.push(
                    deferred,
                    format!(
                        "assign:{} target={target} value={value}",
                        assignment.operator.as_str()
                    ),
                );
            }
            Expression::UpdateExpression(update) => {
                let target = self.simple_assignment_target_shape(&update.argument, depth + 1);
                self.push(
                    deferred,
                    format!(
                        "update:{}{} target={target}",
                        if update.prefix { "prefix" } else { "postfix" },
                        update.operator.as_str()
                    ),
                );
            }
            Expression::SequenceExpression(sequence) => {
                for inner in &sequence.expressions {
                    self.walk_expr(inner, deferred, depth + 1);
                }
            }
            Expression::ArrayExpression(array) => {
                for element in &array.elements {
                    if let Some(inner) = element.as_expression() {
                        self.walk_expr(inner, deferred, depth + 1);
                    }
                }
            }
            Expression::ObjectExpression(object) => {
                for property in &object.properties {
                    if let ObjectPropertyKind::ObjectProperty(object_property) = property {
                        self.walk_expr(&object_property.value, deferred, depth + 1);
                    }
                }
            }
            Expression::ParenthesizedExpression(parenthesized) => {
                self.walk_expr(&parenthesized.expression, deferred, depth + 1)
            }
            Expression::TSAsExpression(as_expression) => {
                self.walk_expr(&as_expression.expression, deferred, depth + 1)
            }
            Expression::TSSatisfiesExpression(satisfies) => {
                self.walk_expr(&satisfies.expression, deferred, depth + 1)
            }
            Expression::TSNonNullExpression(non_null) => {
                self.walk_expr(&non_null.expression, deferred, depth + 1)
            }
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => self.walk_call(call, deferred, depth + 1),
                ChainElement::TSNonNullExpression(non_null) => {
                    self.walk_expr(&non_null.expression, deferred, depth + 1)
                }
                ChainElement::StaticMemberExpression(member) => {
                    self.walk_member_value(member, deferred, depth + 1)
                }
                ChainElement::ComputedMemberExpression(member) => {
                    self.walk_expr(&member.object, deferred, depth + 1);
                    self.walk_expr(&member.expression, deferred, depth + 1);
                }
                ChainElement::PrivateFieldExpression(member) => {
                    self.walk_expr(&member.object, deferred, depth + 1)
                }
            },
            // A bare identifier is not itself behavior -- except when it is the single read of a
            // local whose declaration `walk_stmt` skipped as fully inlined, in which case the
            // initializer's own events belong *here*. See `walk_inlined_read`.
            Expression::Identifier(ident) => self.walk_inlined_read(ident, deferred, depth),
            _ => {}
        }
    }

    /// Walks a static member expression standing in **value** position -- a statement of its own,
    /// an operand, an argument, an initializer: anywhere the member access is the finished
    /// expression rather than the thing about to be called.
    ///
    /// A member access is not behavior, so ordinarily this is just a descent into the object. The
    /// exception is the chai-style property assertion (see [`Self::property_chain_event`]), whose
    /// entire meaning is the member names this descent would otherwise walk straight past. When
    /// the chain is recognized the event is emitted and the walk resumes at the base
    /// `expect(...)` call, **skipping the member links in between**: descending into
    /// `member.object` instead would re-enter here on `expect(spy).to.not.have.been` and emit a
    /// shortened chain event for every remaining link, so one assertion would contribute as many
    /// events as it has properties.
    ///
    /// Value position is the whole condition. A member chain in *callee* position reaches
    /// [`Self::walk_under_members`] instead, which descends past the links without consulting
    /// this: an `expect(x).to.have.property('a')` already emits
    /// `call:global:expect.to.have.property` carrying those same names, and emitting a chain
    /// event for its `expect(x).to.have` prefix would count one assertion twice.
    ///
    /// A *declined* chain descends the same way, and for a sharper reason: re-entering here on
    /// the object would offer this recognizer every proper prefix of a chain it has already
    /// refused, and a prefix can qualify where the whole does not. `expect(x).to['be'].ok` would
    /// have reported `assert:chain:expect.to`, and a chain one link past
    /// [`MAX_ASSERTION_CHAIN`] would have reported the longest accepted prefix of itself -- a
    /// bound that truncates instead of declining. A chain is recognized whole or not at all.
    fn walk_member_value(
        &mut self,
        member: &StaticMemberExpression<'a>,
        deferred: bool,
        depth: u32,
    ) {
        if let Some((event, base_call)) = self.property_chain_event(member, depth) {
            self.push(deferred, event);
            self.walk_call(base_call, deferred, depth + 1);
            return;
        }
        self.walk_under_members(&member.object, deferred, depth);
    }

    /// Walks whatever sits *underneath* a member chain, emitting nothing for the links
    /// themselves. Reached from the two positions where a chain's member names must not be
    /// offered to [`Self::property_chain_event`]: the callee of a call, whose own `call:` event
    /// already names them, and the object of a member access this recognizer declined.
    ///
    /// This is what `walk_expr` did by construction before [`Self::walk_member_value`] existed:
    /// every member arm it could reach only ever recursed into the object, so a chain was
    /// already descended without emitting anything of its own. Spelling that descent out here
    /// keeps it that way now that a static member access in value position can emit an event,
    /// and keeps the [`MAX_DEPTH`] accounting identical to the recursion it replaces -- one level
    /// per link, degrading at the same depth.
    fn walk_under_members(&mut self, callee: &Expression<'a>, deferred: bool, depth: u32) {
        let mut current = callee;
        let mut depth = depth;
        loop {
            if depth > MAX_DEPTH {
                return;
            }
            current = match current {
                Expression::StaticMemberExpression(member) => &member.object,
                Expression::PrivateFieldExpression(member) => &member.object,
                Expression::ComputedMemberExpression(member) => {
                    self.walk_expr(&member.expression, deferred, depth + 1);
                    &member.object
                }
                Expression::ParenthesizedExpression(parenthesized) => &parenthesized.expression,
                Expression::TSNonNullExpression(non_null) => &non_null.expression,
                Expression::TSAsExpression(as_expression) => &as_expression.expression,
                Expression::TSSatisfiesExpression(satisfies) => &satisfies.expression,
                other => return self.walk_expr(other, deferred, depth),
            };
            depth += 1;
        }
    }

    /// Re-emits a single-use local's inlined initializer at its one **read** site.
    ///
    /// `walk_stmt` skips a fully-inlined declarator's initializer (see `declarator_fully_inlined`)
    /// on the premise that the read renders it instead. That premise held for exactly one read
    /// position -- an assertion's subject or expected value, which `shape` renders through a
    /// `Serializer` that consults [`DataflowSubst`]. Every other read position (a call argument, a
    /// fluent receiver, a `return` operand) reaches this collector's own traversal, where a bare
    /// `Expression::Identifier` emits nothing at all, so the initializer's `call:` events were
    /// dropped outright and two tests building genuinely different subjects collided on an equal
    /// `behavior` multiset -- a real `near-duplicate-test` false positive, distilled in
    /// `fixtures/observed/observed-single-use-local-erases-behavior.test.ts`.
    ///
    /// Re-emitting **at the read site**, rather than un-skipping the declaration, is what keeps
    /// the two forms this canonicalization exists to equate actually equal. The whole point of
    /// single-use inlining is that `const x = f(); g(x)` must look like `g(f())`, and `g(f())`
    /// emits `call:g` *then* `call:f` -- the enclosing call first, its argument second. Walking
    /// the initializer back at the declaration would emit `call:f` first, restoring the multiset
    /// but inverting the sequence; `near-duplicate-test` keys on a sorted multiset and would not
    /// notice, but `subsumed-test` keys on an ordered subsequence and would stop matching the pair
    /// it matches today. It would also over-correct in the assertion-subject position, where the
    /// inlined body emits *no* event for the initializer at all (`walk_call` treats a whole
    /// assertion as one atomic event and never descends into its subject), so an extra `call:f`
    /// from the declaration would split `fixtures/duplicates/near-extracted-local-positive.test.ts`
    /// apart. Emitting at the read site gets both right for free: a read inside an assertion
    /// subject is never reached (the assertion already returned), and a read anywhere else lands
    /// in exactly the position the inlined expression would have occupied.
    ///
    /// Only [`DataflowSubst::Inline`] is re-emitted. A [`DataflowSubst::Member`] read stands for
    /// `object.property`, and a member read of a free object contributes no event in either form;
    /// a [`DataflowSubst::DestructuredProperty`] read stands for the local itself, which was never
    /// an event either -- that entry only decorates how the local *renders* where it already
    /// appears.
    fn walk_inlined_read(&mut self, ident: &IdentifierReference<'a>, deferred: bool, depth: u32) {
        // Copy the map reference out of `self` first: it is a `'m` borrow independent of this
        // `&mut self`, so holding an entry from it across the `walk_expr` call below is fine.
        let dataflow = self.ctx.dataflow;
        let Some(symbol_id) = ident.reference_id.get().and_then(|reference_id| {
            self.ctx
                .semantic
                .scoping()
                .get_reference(reference_id)
                .symbol_id()
        }) else {
            return;
        };
        if let Some(DataflowSubst::Inline { init, .. }) = dataflow.get(&symbol_id) {
            self.walk_expr(init, deferred, depth + 1);
        }
    }

    /// Render an alpha assignment target and retain the serializer's canonical-binding updates.
    fn assignment_target_shape(&mut self, target: &AssignmentTarget<'a>, depth: u32) -> String {
        let mut serializer = self.shape_serializer(Mode::Alpha);
        let result = serializer.ser_assignment_target(target, depth);
        self.ctx.canonical = serializer.ctx.canonical;
        self.ctx.next_local = serializer.ctx.next_local;
        result
    }

    /// Render a simple alpha assignment target and preserve the updated canonical-binding
    /// state.
    fn simple_assignment_target_shape(
        &mut self,
        target: &SimpleAssignmentTarget<'a>,
        depth: u32,
    ) -> String {
        let mut serializer = self.shape_serializer(Mode::Alpha);
        let result = serializer.ser_simple_assignment_target(target, depth);
        self.ctx.canonical = serializer.ctx.canonical;
        self.ctx.next_local = serializer.ctx.next_local;
        result
    }

    /// Collect one atomic assertion event or ordinary call behavior, respecting nesting and
    /// deferred execution.
    fn walk_call(&mut self, call: &CallExpression<'a>, deferred: bool, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        if let Some(event) = self.assertion_event(call, depth) {
            self.push(deferred, event);
            // The whole assertion is treated as one atomic behavior event, matching CukeDedup's
            // `assertion_behavior_event`: its subject and expected value are already captured in
            // the event string, so descending further would count the subject's own calls (e.g.
            // `page.locator(...)` inside `expect(page.locator(...)).toBeVisible()`) a second time.
            return;
        }
        let path = self.call_path(&call.callee, depth + 1);
        // `near-duplicate-test` and `subsumed-test` key on these events, not on `alpha`, so the
        // type arguments have to reach the event text too: without them
        // `assertType<string>(value)` and `assertType<number>(value)` emit the identical
        // `call:global:assertType` and the two rules cannot tell one type-level assertion from
        // another even though `alpha` now can.
        let type_arguments =
            type_argument_fragment(self.ctx.source, call.type_arguments.as_deref());
        let arguments = self.call_argument_fragment(call, depth);
        self.push(deferred, format!("call:{path}{type_arguments}{arguments}"));
        self.walk_under_members(&call.callee, deferred, depth + 1);
        for argument in &call.arguments {
            if let Some(argument_expr) = argument.as_expression() {
                self.walk_expr(argument_expr, deferred, depth + 1);
            } else if let Argument::SpreadElement(spread) = argument {
                self.walk_expr(&spread.argument, deferred, depth + 1);
            }
        }
    }

    /// The data arguments of a call, rendered into its behavior event.
    ///
    /// Without these, `wrapper.find('.name-text')` and `wrapper.find('.unit-value')` emit the
    /// identical `call:global:wrapper.find`, and two tests querying different things carry the
    /// same behavior multiset -- a `near-duplicate-test` false positive on tests that do
    /// different work. Measured on a 24,147-test codebase, that erasure was the largest single
    /// cause of the rule's 27% false-positive rate.
    ///
    /// This is the same argument [`type_argument_fragment`] already makes one level up: the two
    /// rules that key on `behavior` cannot tell apart what the event text does not mention.
    ///
    /// Three decisions, each costing discrimination rather than risking a wrong match:
    ///
    /// * Rendered through [`BehaviorCollector::shape`] in [`Mode::Alpha`], so the dataflow
    ///   canonicalization applies. That is what keeps single-use local inlining working:
    ///   `const sel = '.a'; find(sel)` renders its argument as `'.a'`, exactly like `find('.a')`,
    ///   and the two stay near-duplicates as the rule intends. `Alpha` rather than `Structural`
    ///   because the *value* is the discriminating part -- `Structural` would render both
    ///   selectors as `<string>` and change nothing.
    /// * A function argument renders as its normalized callable shape. Mock/setup helpers often
    ///   take a callback whose returned object is the branch being tested; if every callback is
    ///   just `fn`, `registerEndpoint('/x', () => ({type: 'a'}))` and the same endpoint returning
    ///   `{type: 'b'}` collapse before any later assertion can tell them apart.
    /// * A spread renders as its argument shape with an explicit `...` prefix. The value is still
    ///   available here, and erasing it would make `render(...adminProps)` and
    ///   `render(...guestProps)` indistinguishable.
    ///
    /// JSON supplies an unambiguous boundary between serialized arguments. A plain join would let
    /// an argument containing the separator forge another argument list, which could manufacture
    /// equality in the two rules that trust these event strings.
    fn call_argument_fragment(&mut self, call: &CallExpression<'a>, depth: u32) -> String {
        if call.arguments.is_empty() {
            return String::new();
        }
        let shapes: Vec<String> = call
            .arguments
            .iter()
            .map(|argument| match argument {
                Argument::SpreadElement(spread) => {
                    format!(
                        "...{}",
                        self.shape(&spread.argument, Mode::Alpha, depth + 1)
                    )
                }
                _ => match argument.as_expression() {
                    // A nested call renders in full, deliberately. Collapsing it to a
                    // placeholder was tried and loses the case that matters most: a
                    // project-owned helper whose argument is a *built subject*
                    // (`expectMethodMatch(z.string())` against
                    // `expectMethodMatch(z.string().min(5))`) then emits the identical event for
                    // both, and every refinement is reported as subsuming the primitive it
                    // refines. The duplication with the argument's own event is the price of
                    // telling those two apart, and `shape` renders an inlined single-use local
                    // exactly as it renders the call written inline, so the extraction
                    // equivalence survives it.
                    Some(expression) => self.shape(expression, Mode::Alpha, depth + 1),
                    None => "?".to_owned(),
                },
            })
            .collect();
        format!(
            " args={}",
            serde_json::to_string(&shapes).expect("argument shapes serialize infallibly")
        )
    }

    /// Flattens a member/call chain into a canonical dotted path.
    ///
    /// A fluent call such as `page.locator(x).click()` produces the event
    /// `call:global:page.locator.click` for the outer call -- unlike CukeDedup's
    /// `static_call_receiver`, which deliberately drops an intermediate call's own method name
    /// from the receiver chain (relying on that inner call getting its own separate event when
    /// the traversal reaches it), this keeps the full fluent path on the outer event, since oxc's
    /// typed `CallExpression` makes recursing through `callee` uniform regardless of nesting.
    /// `walk_call`'s subsequent walk into `call.callee` still visits the inner `CallExpression`
    /// separately, so `page.locator(x)` also gets its own `call:global:page.locator` event -- the
    /// two are additive, not a replacement for each other. Either way the outer event is
    /// "recognizably about `.click`", which is what callers need to spot a fluent action.
    /// The root identifier of a call path, carrying a destructured binding's property key when it
    /// has one. This is the callee-position half of [`DataflowSubst::DestructuredProperty`]:
    /// without it
    /// `const { fetchUser } = useApi(); await fetchUser(1)` and
    /// `const { deleteUser } = useApi(); await deleteUser(1)` both emit `call:L0(<number:1>)`,
    /// the same erasure the read position had, one event kind over. It reaches receivers too --
    /// `const { api } = useThing(); api.get()` renders `call:L0:prop:api.get` -- because
    /// `call_path` recurses through a member expression's object into this same arm.
    fn path_identity(&mut self, ident: &IdentifierReference<'_>) -> String {
        let substitution = ident
            .reference_id
            .get()
            .and_then(|reference_id| {
                self.ctx
                    .semantic
                    .scoping()
                    .get_reference(reference_id)
                    .symbol_id()
            })
            .and_then(|symbol_id| self.ctx.dataflow.get(&symbol_id).cloned());
        match substitution {
            Some(DataflowSubst::Member { object, property }) => {
                format!("{}.{}", self.call_path(object, 0), property)
            }
            Some(DataflowSubst::DestructuredProperty { property }) => {
                format!("{}:prop:{property}", self.ctx.reference(ident))
            }
            Some(DataflowSubst::Inline { .. }) | None => self.ctx.reference(ident),
        }
    }

    /// Render an anchored callee path, retaining raw source for unsupported callee forms or
    /// when the depth limit is exceeded.
    fn call_path(&mut self, callee: &Expression<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.ctx.slice(callee.span()).to_owned();
        }
        match callee {
            Expression::Identifier(ident) => self.path_identity(ident),
            Expression::ThisExpression(_) => "this".to_owned(),
            Expression::Super(_) => "super".to_owned(),
            Expression::ParenthesizedExpression(parenthesized) => {
                self.call_path(&parenthesized.expression, depth + 1)
            }
            Expression::TSAsExpression(as_expression) => {
                self.call_path(&as_expression.expression, depth + 1)
            }
            Expression::TSNonNullExpression(non_null) => {
                self.call_path(&non_null.expression, depth + 1)
            }
            Expression::StaticMemberExpression(member) => {
                format!(
                    "{}.{}",
                    self.call_path(&member.object, depth + 1),
                    member.property.name.as_str()
                )
            }
            Expression::ComputedMemberExpression(member) => {
                format!("{}[]", self.call_path(&member.object, depth + 1))
            }
            Expression::PrivateFieldExpression(member) => {
                format!(
                    "{}.#{}",
                    self.call_path(&member.object, depth + 1),
                    member.field.name.as_str()
                )
            }
            // An inner call in a fluent chain contributes its own type arguments to the outer
            // path, so `expectTypeOf<A>().toEqualTypeOf<B>()` and `expectTypeOf<C>().toEqualTypeOf<B>()`
            // produce different outer events rather than relying solely on the separate event
            // `walk_call` emits for the inner call.
            Expression::CallExpression(call) => format!(
                "{}{}",
                self.call_path(&call.callee, depth + 1),
                type_argument_fragment(self.ctx.source, call.type_arguments.as_deref())
            ),
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => format!(
                    "{}{}",
                    self.call_path(&call.callee, depth + 1),
                    type_argument_fragment(self.ctx.source, call.type_arguments.as_deref())
                ),
                ChainElement::TSNonNullExpression(non_null) => {
                    self.call_path(&non_null.expression, depth + 1)
                }
                ChainElement::StaticMemberExpression(member) => {
                    format!(
                        "{}.{}",
                        self.call_path(&member.object, depth + 1),
                        member.property.name.as_str()
                    )
                }
                ChainElement::ComputedMemberExpression(member) => {
                    format!("{}[]", self.call_path(&member.object, depth + 1))
                }
                ChainElement::PrivateFieldExpression(member) => {
                    format!(
                        "{}.#{}",
                        self.call_path(&member.object, depth + 1),
                        member.field.name.as_str()
                    )
                }
            },
            _ => format!("<{}>", self.ctx.slice(callee.span())),
        }
    }

    /// Recognizes the `expect(...)` call an assertion chain hangs off, and reports how it was
    /// spelled: `""` for a bare `expect(subject)`, `"soft"` or `"poll"` for
    /// `expect.soft(subject)` / `expect.poll(subject)`. `None` for any other call, including a
    /// local shadowing the name (see [`is_expect_identifier`]) and `expect.<anything else>(...)`.
    ///
    /// Shared by the two chain recognizers -- [`Self::assertion_event`] for the call-terminated
    /// form and [`Self::property_chain_event`] for the chai property form -- so that a new
    /// spelling of the base is taught to both at once. The base is the half the two forms agree
    /// on; what follows it is the half they differ on, and each keeps its own loop for that.
    fn expect_base(&self, call: &CallExpression<'a>) -> Option<&'static str> {
        match &call.callee {
            Expression::Identifier(ident) => {
                is_expect_identifier(self.ctx.semantic, self.ctx.body_span, ident).then_some("")
            }
            Expression::StaticMemberExpression(member) => {
                let spelling = match member.property.name.as_str() {
                    "soft" => "soft",
                    "poll" => "poll",
                    _ => return None,
                };
                match &member.object {
                    Expression::Identifier(ident)
                        if is_expect_identifier(self.ctx.semantic, self.ctx.body_span, ident) =>
                    {
                        Some(spelling)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Recognizes a chai-style property-chain assertion -- `expect(spy).to.not.have.been.calledOnce`
    /// -- and returns its event text together with the `expect(...)` call the chain hangs off.
    ///
    /// The whole meaning of such an assertion lives in member names: there is no call node after
    /// the `expect(...)`, so [`Self::walk_call`] never sees it and [`Self::assertion_event`] never
    /// runs. Before this existed the chain contributed *nothing* to `behavior`, and
    /// `expect(spy).to.not.have.been.calledOnce` against `expect(spy).to.have.been.calledTwice` --
    /// opposite polarity and a different assertion -- compared as the same behavior events in the
    /// same order, because all that reached the trace was the shared `vi.fn(); spy(); spy()`
    /// setup. That was a `near-duplicate-test` false positive adjudicated on `vitest`'s own
    /// `test/unit/test/chai-style-assertions.test.ts` (:111 against :419), distilled in
    /// `fixtures/duplicates/near-chai-property-chain-differs.test.ts`.
    ///
    /// The event is **additive**: the caller still walks the base `expect(...)` call afterwards,
    /// so every event a chai-style body produced before is produced now too and only the chain
    /// event is new. That is deliberate. Making the chain atomic the way `assertion_event` is
    /// would have dropped the `call:` event carrying the subject, and the subject is rendered in
    /// `Mode::Alpha` there -- literals and all -- where an atomic assertion event renders its
    /// subject in `Mode::Structural`. Two chains that differ only in a subject literal
    /// (`expect(build(1)).to.be.ok` against `expect(build(2)).to.be.ok`) would then have
    /// collapsed, trading a fixed false positive for a new one. For the same reason the event
    /// carries no `subject=` of its own: the `call:` event emitted immediately after it already
    /// carries the subject, more precisely than this event could.
    ///
    /// The chain is recorded **verbatim**, chai's no-op language links (`to`, `be`, `been`,
    /// `have`, ...) included. Normalizing them away would equate `to.be.true` with `is.true` and
    /// win a handful of duplicate reports, but it needs a hardcoded vocabulary of one assertion
    /// library's filler words, and any word missing from that list would silently become
    /// load-bearing. Keeping every link costs recall on chains spelled differently and cannot
    /// cost precision, which is the side of that trade this defect is on. `not` needs no special
    /// handling under this rule: it is a link like any other, so polarity travels for free.
    ///
    /// `None` for anything that is not rooted in an `expect(...)` call, and for a chain longer
    /// than [`MAX_ASSERTION_CHAIN`] links. The root must be the call *syntactically*: a chain
    /// hung off a local (`const e = expect(x); e.to.be.ok`) is declined even when
    /// [`DataflowSubst::Inline`] would substitute that local elsewhere, because this walk reads
    /// the member chain directly rather than through a [`Serializer`]. That is the ordinary
    /// under-matching this module biases toward -- the chain contributes no event, exactly as
    /// before -- not a wrong one.
    fn property_chain_event<'x>(
        &self,
        member: &'x StaticMemberExpression<'a>,
        depth: u32,
    ) -> Option<(String, &'x CallExpression<'a>)> {
        if depth > MAX_DEPTH {
            return None;
        }
        let mut links = vec![member.property.name.as_str()];
        let base_call = unwind_member_chain(&member.object, &mut links)?;
        let qualifier = match self.expect_base(base_call)? {
            "" => "expect".to_owned(),
            spelling => format!("expect.{spelling}"),
        };
        links.reverse();
        Some((
            format!("assert:chain:{qualifier}.{}", links.join(".")),
            base_call,
        ))
    }

    /// Recognizes `expect(subject)[.not][.resolves|.rejects].matcher(...expected)`, including
    /// `expect.soft`/`expect.poll`. Returns `None` for anything else, in which case the call is
    /// treated as an ordinary call event instead.
    fn assertion_event(&mut self, call: &CallExpression<'a>, depth: u32) -> Option<String> {
        let member = match &call.callee {
            Expression::StaticMemberExpression(member) => member,
            _ => return None,
        };
        let matcher = member.property.name.as_str();
        let mut links = Vec::new();
        let base_call = unwind_member_chain(&member.object, &mut links)?;
        // Unlike a chai property chain, the links here are a closed set: anything else between
        // `expect(...)` and the matcher means this is not an assertion at all, and the call falls
        // through to an ordinary call event. Validated after the unwind rather than during it,
        // which changes nothing -- either way an unrecognized link declines the whole chain.
        let mut modifiers: Vec<&'static str> = Vec::with_capacity(links.len());
        for name in links.iter().rev() {
            modifiers.push(match *name {
                "not" => "not",
                "resolves" => "resolves",
                "rejects" => "rejects",
                _ => return None,
            });
        }
        match self.expect_base(base_call)? {
            "" => {}
            spelling => modifiers.insert(0, spelling),
        }
        let subject = base_call
            .arguments
            .first()
            .and_then(|argument| argument.as_expression());
        let subject_shape = subject.map_or_else(
            || "()".to_owned(),
            |expression| self.shape(expression, Mode::Structural, depth + 1),
        );
        let value_parts: Vec<String> = call
            .arguments
            .iter()
            .filter_map(|argument| argument.as_expression())
            .map(|expression| self.shape(expression, Mode::Alpha, depth + 1))
            .collect();
        let qualifier = if modifiers.is_empty() {
            "expect".to_owned()
        } else {
            format!("expect.{}", modifiers.join("."))
        };
        // Both halves of the chain can carry explicit type arguments (`expect<T>(x)` and the
        // matcher's own `.toEqualTypeOf<U>()`), and each is attached to the half that wrote it.
        let subject_type_arguments =
            type_argument_fragment(self.ctx.source, base_call.type_arguments.as_deref());
        let matcher_type_arguments =
            type_argument_fragment(self.ctx.source, call.type_arguments.as_deref());
        Some(format!(
            "assert:{qualifier}{subject_type_arguments}.{matcher}{matcher_type_arguments}\
             (subject={subject_shape}; value={})",
            value_parts.join(", ")
        ))
    }

    /// Renders an assertion's subject or expected value using the same identifier/literal
    /// handling as the main serializer, without threading a second `Serializer` instance through
    /// the whole traversal. The temporarily-built `Serializer` borrows this collector's own
    /// canonical map and counter and hands them back afterward, so canonical names assigned while
    /// rendering an assertion's shape are visible to later assertions in the same body too.
    fn shape_serializer(&mut self, mode: Mode) -> Serializer<'a, 'b, 's, 'm> {
        let mut ctx = self.ctx.fresh_for_pass();
        // The canonical names this collector assigned while rendering an earlier event's shape
        // must stay visible to later ones, so the temporary carries this collector's own state
        // and `shape` below is the return leg.
        ctx.canonical = std::mem::take(&mut self.ctx.canonical);
        ctx.next_local = self.ctx.next_local;
        Serializer {
            ctx,
            mode,
            // This serializer renders one behavior event's shape, not a whole callable. Any `this`
            // it meets is attributed by the `ser_callable` frame that owns this body, so the flag
            // starts clean here and is discarded with the temporary.
            this_or_arguments: false,
            outermost_this_dependent: false,
        }
    }

    /// Render an expression in the requested mode and merge canonical-binding state back into
    /// the collector.
    fn shape(&mut self, expr: &Expression<'a>, mode: Mode, depth: u32) -> String {
        let mut serializer = self.shape_serializer(mode);
        let result = serializer.ser_expr(expr, depth);
        self.ctx.canonical = serializer.ctx.canonical;
        self.ctx.next_local = serializer.ctx.next_local;
        result
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

/// Recognize a literal true, the subject and expected value of the expect(true).toBe(true)
/// stub assertion.
fn is_stub_boolean_true(expr: &Expression<'_>) -> bool {
    matches!(expr, Expression::BooleanLiteral(literal) if literal.value)
}

/// Recognizes `expect(true).toBe(true)`, the canonical no-op assertion used as a placeholder.
fn is_stub_assertion(expr: &Expression<'_>) -> bool {
    let Expression::CallExpression(call) = expr else {
        return false;
    };
    let Expression::StaticMemberExpression(member) = &call.callee else {
        return false;
    };
    if member.property.name.as_str() != "toBe" {
        return false;
    }
    let Expression::CallExpression(inner) = &member.object else {
        return false;
    };
    let Expression::Identifier(ident) = &inner.callee else {
        return false;
    };
    if ident.name.as_str() != "expect" {
        return false;
    }
    let subject_is_true = inner
        .arguments
        .first()
        .and_then(|argument| argument.as_expression())
        .is_some_and(is_stub_boolean_true);
    let expected_is_true = call
        .arguments
        .first()
        .and_then(|argument| argument.as_expression())
        .is_some_and(is_stub_boolean_true);
    subject_is_true && expected_is_true
}

/// Recognizes a stub expression: `null`, `undefined`, `void 0`, or a call to a
/// pending/not-implemented marker (`todo()`, `pending()`, `Promise.resolve()` with no arguments).
/// Unwraps `await`/parens first.
fn is_stub_expression(expr: &Expression<'_>) -> bool {
    let mut current = expr;
    loop {
        match current {
            Expression::AwaitExpression(await_expression) => current = &await_expression.argument,
            Expression::ParenthesizedExpression(parenthesized) => {
                current = &parenthesized.expression
            }
            _ => break,
        }
    }
    match current {
        Expression::NullLiteral(_) => true,
        Expression::Identifier(ident) => ident.name.as_str() == "undefined",
        Expression::UnaryExpression(unary) => {
            unary.operator == UnaryOperator::Void
                && matches!(&unary.argument, Expression::NumericLiteral(literal) if literal.value == 0.0)
        }
        Expression::CallExpression(call) => {
            let name = match &call.callee {
                Expression::Identifier(ident) => Some(ident.name.as_str()),
                Expression::StaticMemberExpression(member) => Some(member.property.name.as_str()),
                _ => None,
            };
            let normalized = name.map(|name| {
                name.chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>()
                    .to_lowercase()
            });
            if matches!(
                normalized.as_deref(),
                Some("pending" | "todo" | "notimplemented")
            ) {
                return true;
            }
            matches!(&call.callee, Expression::StaticMemberExpression(member)
                if matches!(&member.object, Expression::Identifier(id) if id.name.as_str() == "Promise")
                    && member.property.name.as_str() == "resolve")
                && call.arguments.is_empty()
        }
        _ => false,
    }
}

/// True when a body is empty or a recognized no-op stub. Trivial bodies are excluded from
/// comparison upstream, so this is purely a false-positive guard: getting it wrong in the
/// direction of "not trivial" is safe (a real body just gets compared normally), so every branch
/// here requires a specific, narrow shape rather than guessing.
fn is_trivial(body: &FunctionLike<'_, '_>) -> bool {
    let statements: &[Statement<'_>] = match body {
        FunctionLike::Arrow(arrow) => match &arrow.body {
            ArrowFunctionBody::FunctionBody(function_body) => &function_body.statements,
            other => {
                return other.as_expression().is_some_and(|expression| {
                    is_stub_expression(expression) || is_stub_assertion(expression)
                });
            }
        },
        FunctionLike::Function(function) => match &function.body {
            Some(function_body) => &function_body.statements,
            // No body at all (an overload signature or `declare function`): nothing to compare.
            None => return true,
        },
    };
    match statements {
        [] => true,
        [Statement::ReturnStatement(return_statement)] => match &return_statement.argument {
            Some(argument) => is_stub_expression(argument) || is_stub_assertion(argument),
            None => true,
        },
        [Statement::ExpressionStatement(expression_statement)] => {
            is_stub_expression(&expression_statement.expression)
                || is_stub_assertion(&expression_statement.expression)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
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
        let semantic_ret = SemanticBuilder::new().build(program);
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
    // JSX: structured serialization
    // -----------------------------------------------------------------------------------------
    //
    // The recall corpus pins the two end-to-end losses (a `duplicate-test` and a
    // `parameterization-candidate`); these pin the serializer directly, including the shapes the
    // corpus has no case for (fragments, spread attributes, boolean attributes, expression
    // children, intrinsics vs components) and the text normalization the formatting case rests on.

    /// Two small arms with no coverage, both reachable from ordinary test bodies.
    ///
    /// A declaration with no initializer (`let captured;`) is the shape every "assign it in a hook,
    /// read it in the test" file opens with, and the dataflow walk skips it -- there is nothing to
    /// inline. A `..` segment in a dynamic-import specifier is how half of a monorepo spells its
    /// imports, and it is normalized away so two spellings of one file agree.
    #[test]
    fn an_uninitialized_declaration_and_a_parent_path_segment_are_handled() {
        // No initializer: skipped, and its presence does not change what the rest of the body says.
        assert_eq!(
            alpha_of("let captured; captured = compute(); expect(captured).toBe(1);"),
            alpha_of("let captured; captured = compute(); expect(captured).toBe(1);"),
        );
        assert_ne!(
            alpha_of("let captured; captured = compute(); expect(captured).toBe(1);"),
            alpha_of("let captured; captured = other(); expect(captured).toBe(1);"),
            "the declaration is skipped, not the assignment that follows it"
        );

        // `..` is resolved away, so these two specifiers name one module and compare equal.
        assert_eq!(
            alpha_of("const m = await import('../helpers'); expect(m).toBeDefined();"),
            alpha_of("const m = await import('./sub/../../helpers'); expect(m).toBeDefined();"),
            "two spellings of one path must not read as two different modules"
        );
        assert_ne!(
            alpha_of("const m = await import('../helpers'); expect(m).toBeDefined();"),
            alpha_of("const m = await import('../other'); expect(m).toBeDefined();"),
            "and genuinely different modules still differ"
        );
    }

    /// The attribute forms beyond `name="string"`, each of which had no coverage.
    ///
    /// `ser_jsx_attribute_value` handles four value shapes and the name handles two; only a quoted
    /// string and a braced expression were exercised. The rest are ordinary in real markup --
    /// `xlink:href` on an `<svg>`, an element handed straight to a prop -- and each is a place the
    /// serializer could silently emit nothing, which would make two different elements compare equal.
    #[test]
    fn jsx_attribute_forms_are_each_serialized() {
        // A namespaced attribute name, which SVG markup uses constantly.
        assert_ne!(
            alpha_of_tsx(r#"const el = render(<svg xlink:href="a" />);"#),
            alpha_of_tsx(r#"const el = render(<svg xlink:href="b" />);"#),
            "a namespaced attribute's value still distinguishes two elements"
        );
        assert_ne!(
            alpha_of_tsx(r#"const el = render(<svg xlink:href="a" />);"#),
            alpha_of_tsx(r#"const el = render(<svg href="a" />);"#),
            "and the namespace is part of the name"
        );
        // An element, and a fragment, handed to an attribute directly rather than through braces.
        assert_ne!(
            alpha_of_tsx("const el = render(<Panel header=<Title /> />);"),
            alpha_of_tsx("const el = render(<Panel header=<Caption /> />);"),
            "an element-valued attribute is serialized, not dropped"
        );
        assert_ne!(
            alpha_of_tsx("const el = render(<Panel header=<Title /> />);"),
            alpha_of_tsx("const el = render(<Panel header=<></> />);"),
            "a fragment-valued attribute is distinguishable from an element-valued one"
        );
        // An empty container is not observable, so it reads as the valueless form.
        assert_eq!(
            alpha_of_tsx("const el = render(<Panel header={} />);"),
            alpha_of_tsx("const el = render(<Panel header={} />);"),
            "`attr='{{}}'` is stable, which is the only claim to make about a value a renderer \
             cannot see"
        );
    }

    /// The element-name forms JSX's grammar allows beyond a bare identifier.
    ///
    /// `ser_jsx_name` and `ser_jsx_member_object` handle four shapes apiece and only the plain
    /// identifier had a test: the member, nested-member and `this` arms were reachable from any
    /// component library that namespaces its parts (`<Menu.Item />` is the common spelling) and had
    /// no coverage at all, which is most of what held `fingerprint.rs` under its floor.
    ///
    /// Each pair asserts the shape is DISTINGUISHED, which is the only thing a serializer can be
    /// wrong about here: collapsing `<Menu.Item />` and `<Menu.Divider />` would make two different
    /// components compare equal, and that is a false positive in a component suite.
    #[test]
    fn jsx_element_name_forms_are_each_distinguished() {
        // A dotted component name, and a sibling under the same namespace.
        assert_ne!(
            alpha_of_tsx("const el = render(<Menu.Item>Open</Menu.Item>);"),
            alpha_of_tsx("const el = render(<Menu.Divider>Open</Menu.Divider>);"),
            "two parts of one namespaced component render different things"
        );
        // Nesting one level deeper exercises the recursive arm.
        assert_ne!(
            alpha_of_tsx("const el = render(<Menu.Item.Label>Open</Menu.Item.Label>);"),
            alpha_of_tsx("const el = render(<Menu.Item>Open</Menu.Item>);"),
            "`Menu.Item.Label` is not `Menu.Item`"
        );
        // `<this.X />` reads the enclosing `this`, the same fact the non-JSX path tracks.
        assert_ne!(
            alpha_of_tsx("const el = render(<this.Panel>Open</this.Panel>);"),
            alpha_of_tsx("const el = render(<Panel>Open</Panel>);"),
            "a component read off `this` is not a free component of the same name"
        );
        // An intrinsic (lowercase) and a component (capitalized) are different by JSX's grammar.
        assert_ne!(
            alpha_of_tsx("const el = render(<button>Save</button>);"),
            alpha_of_tsx("const el = render(<Button>Save</Button>);"),
            "`<button>` is a DOM tag and `<Button>` is a component reference"
        );
        // ...and the same shape twice is still equal, so the inequalities above mean something.
        assert_eq!(
            alpha_of_tsx("const el = render(<Menu.Item>Open</Menu.Item>);"),
            alpha_of_tsx("const el = render(<Menu.Item>Open</Menu.Item>);"),
            "identical dotted names compare equal, or every assertion here is vacuous"
        );
    }

    /// The case that named the defect: the same element, written across several lines instead of
    /// one. Formatting is not a behavioral difference, and the raw slice this arm used to emit
    /// made it one.
    #[test]
    fn jsx_formatting_does_not_change_alpha() {
        assert_eq!(
            alpha_of_tsx("const el = render(<Button variant=\"primary\" size=\"large\">Save</Button>);"),
            alpha_of_tsx(
                "const el = render(\n  <Button variant=\"primary\" size=\"large\">\n    Save\n  </Button>,\n);"
            ),
            "an element reindented by a formatter renders identically and must fingerprint \
             identically in `alpha`"
        );
    }

    /// The other face of the same defect: an attribute literal is a VALUE, so `alpha` keeps it and
    /// `structural` collapses it to the kind -- the exact relation that makes
    /// `parameterization-candidate` possible outside JSX, which the raw slice hid.
    #[test]
    fn jsx_attribute_literal_is_a_literal_to_both_views() {
        let primary = "const el = render(<Button variant=\"primary\">Save</Button>);";
        let secondary = "const el = render(<Button variant=\"secondary\">Save</Button>);";
        assert_ne!(
            alpha_of_tsx(primary),
            alpha_of_tsx(secondary),
            "`alpha` preserves the attribute's value"
        );
        assert_eq!(
            structural_of_tsx(primary),
            structural_of_tsx(secondary),
            "`structural` collapses it to its kind, exactly as a literal outside JSX would"
        );
    }

    /// The precision counterweight: two different components taking the same props and children
    /// must stay distinct, and so must an intrinsic and a component of the same name-shape.
    #[test]
    fn jsx_element_names_do_not_collapse() {
        assert_ne!(
            alpha_of_tsx("const el = render(<Button variant=\"primary\" />);"),
            alpha_of_tsx("const el = render(<Link variant=\"primary\" />);"),
            "two different component references are two different elements"
        );
        assert_ne!(
            alpha_of_tsx("const el = render(<button variant=\"primary\" />);"),
            alpha_of_tsx("const el = render(<Button variant=\"primary\" />);"),
            "an intrinsic `button` and a component `Button` are different elements, and the \
             intrinsic:/component: prefixes keep the two identifier variants from colliding"
        );
    }

    /// One element exercising every sub-form the serializer claims to model, with the exact
    /// fragment each produces -- the JSX counterpart of `SERIALIZED_FRAGMENTS`' per-arm matrix.
    #[test]
    fn jsx_sub_forms_serialize_to_their_named_fragments() {
        let alpha = alpha_of_tsx(
            "const el = render(<div className=\"x\" disabled {...rest}>{count}</div>);",
        );
        assert!(alpha.contains("(jsx-elem intrinsic:div "), "name: {alpha}");
        assert!(
            alpha.contains("(jsx-attr className <string:x>) "),
            "value: {alpha}"
        );
        assert!(
            alpha.contains("(jsx-attr disabled) "),
            "boolean attribute: {alpha}"
        );
        assert!(
            alpha.contains("(jsx-spread-attr id:global:rest) "),
            "spread attribute: {alpha}"
        );
        assert!(
            alpha.contains("(jsx-expr id:global:count)"),
            "expression child: {alpha}"
        );

        let fragment = alpha_of_tsx("const all = <>{'a'}{items}</>;");
        assert!(
            fragment.contains("(jsx-frag (jsx-expr <string:a>) (jsx-expr id:global:items))"),
            "fragment with a literal-expression child and an identifier child: {fragment}"
        );

        let nested = alpha_of_tsx("const row = <Row label=<Icon /> />;");
        assert!(
            nested.contains("(jsx-attr label (jsx-elem "),
            "an element as an attribute value nests: {nested}"
        );

        let namespaced = alpha_of_tsx("const x = <f:bar />;");
        assert!(
            namespaced.contains("jsx-ns:f:bar"),
            "namespaced name: {namespaced}"
        );

        let member = alpha_of_tsx("const x = <Mods.Widget />;");
        assert!(
            member.contains("(jsx-member component:global:Mods .Widget)"),
            "member-expression name resolves its object like any reference: {member}"
        );

        let spread_children = alpha_of_tsx("const x = <div>{...items}</div>;");
        assert!(
            spread_children.contains("(jsx-spread-children id:global:items)"),
            "spread children: {spread_children}"
        );
    }

    /// `normalize_jsx_text` on the shapes that decide whether two layouts render the same text:
    /// each row is (raw source text, what a renderer places in the DOM).
    #[test]
    fn jsx_text_normalizes_the_way_a_transform_renders_it() {
        // The corpus case: a single text child, reindented.
        assert_eq!(normalize_jsx_text("\n  Save\n"), "Save");
        assert_eq!(normalize_jsx_text("Save"), "Save");
        // Words on separate lines join with one space, interior indentation vanishes.
        assert_eq!(normalize_jsx_text("\n  Hello\n  World\n"), "Hello World");
        assert_eq!(normalize_jsx_text("Hello World"), "Hello World");
        // Blank interior lines and whitespace-only children render nothing.
        assert_eq!(normalize_jsx_text("\n\n    \n"), "");
        assert_eq!(normalize_jsx_text("   "), "");
        assert_eq!(normalize_jsx_text(""), "");
        // Tabs are whitespace like any other.
        assert_eq!(normalize_jsx_text("\n\tSave\n"), "Save");
        // CRLF and lone CR are line breaks like LF.
        assert_eq!(normalize_jsx_text("\r\n  Save\r\n"), "Save");
        assert_eq!(normalize_jsx_text("\r  Save\r"), "Save");
        // Leading text on the FIRST line keeps its leading spaces; a run of them is content.
        assert_eq!(normalize_jsx_text("  Save  More"), "  Save  More");
        // Text around a nested element on one line: the spaces next to the braces are content.
        assert_eq!(normalize_jsx_text("Hello "), "Hello ");
    }

    /// F1 regression: adversarial collision for the unescaped `<kind:raw>` literal encoding.
    ///
    /// Before `escape_literal_delimiters` existed, a single string literal whose *value* spelled
    /// out this serializer's own delimiter syntax could render byte-identical to a sequence of
    /// several genuinely separate sibling literals. Concretely: `['x> <string:y']` (one array,
    /// one string element containing `x> <string:y`) and `['x', 'y']` (one array, two ordinary
    /// string elements) both serialized their elements to `<string:x> <string:y>` -- the first
    /// literal's own raw text supplied the closing `>`, a space, and the *next* literal's
    /// complete `<string:y>` shell, and `ser_array_element`'s `" "`.join could not tell that
    /// forged boundary apart from a real one between two elements. That made two structurally
    /// (and behaviorally) different test bodies produce the same `alpha`, which is exactly what
    /// `duplicate-test` keys on at Error severity -- an adjudicator-provable false duplicate
    /// build failure, self-inflicted by the kind-tagging fix `literal`'s doc comment describes.
    /// This asserts the two `alpha` values now differ.
    #[test]
    fn literal_payload_cannot_forge_the_delimiter_grammar() {
        let source = "test('a', () => { const x = ['x> <string:y']; });\n\
                       test('b', () => { const x = ['x', 'y']; });";
        let prints = fingerprint_all(source);
        assert_eq!(prints.len(), 2, "expected exactly two test bodies");
        assert_ne!(
            prints[0].alpha, prints[1].alpha,
            "a one-element array whose string literal *contains* this serializer's own \
             `<kind:...>` delimiter syntax must not collide with a genuinely different \
             two-element array of separate string literals"
        );
    }

    /// This is the regression test for the bug an earlier draft shipped: identifying a free
    /// reference by `SymbolId` instead of by what it resolves to. `SymbolId` is a per-file,
    /// declaration-order index, so two *separately parsed* files (own `Allocator`, own
    /// `Semantic`, and here deliberately different import orderings so `render`'s `SymbolId`
    /// differs between them) that both import the same name from the same specifier would get
    /// different, incomparable identities under that scheme -- silently defeating cross-file
    /// `duplicate-test` for the single most common case (a shared imported helper). The fix
    /// keys a free import's identity off the module specifier and exported name instead, which
    /// this asserts directly.
    #[test]
    fn free_import_identity_is_stable_across_separately_parsed_files() {
        let source_a = "import { render } from './render';\ntest('a', () => { render(1); });";
        let source_b = "import { other } from './unrelated';\n\
                         import { render } from './render';\n\
                         test('b', () => { render(1); });";
        let a = fingerprint_all(source_a);
        let b = fingerprint_all(source_b);
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(
            a[0].alpha, b[0].alpha,
            "the same name imported from the same specifier in two independently parsed files \
             must produce the same alpha, regardless of what else either file imports first \
             (which would shift `render`'s SymbolId if identity were keyed on it)"
        );
    }

    /// A6 regression: an adversarial review proved that keying a relative import's identity on
    /// the raw specifier text alone (no directory anchor) makes `pkgA/a.test.ts` and
    /// `pkgB/b.test.ts`, each independently writing `import { helper } from './helpers'` (two
    /// different files on disk, in two different directories), collapse to the same `alpha` --
    /// a false `duplicate-test`. Two separate `Allocator`/`Semantic` instances, two different
    /// `file` paths, identical bodies: the fix must tell them apart.
    #[test]
    fn free_import_identity_is_anchored_to_the_importing_files_directory() {
        let source = "import { helper } from './helpers';\ntest('x', () => { helper(1); });";
        let a = fingerprint_all_at(source, Path::new("/repo/pkgA/a.test.ts"));
        let b = fingerprint_all_at(source, Path::new("/repo/pkgB/b.test.ts"));
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_ne!(
            a[0].alpha, b[0].alpha,
            "`./helpers` imported from two different directories names two different files and \
             must not produce the same alpha, even though the specifier text is identical"
        );

        // The positive case must still hold: two files in the *same* directory importing the
        // same relative specifier really do name the same file, and must still match.
        let c = fingerprint_all_at(source, Path::new("/repo/pkgA/c.test.ts"));
        assert_eq!(
            a[0].alpha, c[0].alpha,
            "`./helpers` imported from two files in the *same* directory does name the same \
             file and must still produce the same alpha"
        );
    }

    /// With a real file on disk to resolve against, two *differently spelled* relative
    /// specifiers that name the same file must produce the same `alpha` -- lexical anchoring could
    /// never do this (`./helpers` and `./sub/../helpers` anchor to different literal strings), but
    /// real resolution must, because they are the same import.
    #[test]
    fn free_import_identity_matches_when_different_specifiers_resolve_to_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("helpers.ts"), "export const helper = 1;").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();

        let direct_source = "import { helper } from './helpers';\ntest('x', () => { helper(1); });";
        let roundabout_source =
            "import { helper } from './sub/../helpers';\ntest('x', () => { helper(1); });";

        let direct = fingerprint_all_at(direct_source, &dir.path().join("a.test.ts"));
        let roundabout = fingerprint_all_at(roundabout_source, &dir.path().join("b.test.ts"));
        assert_eq!(direct.len(), 1);
        assert_eq!(roundabout.len(), 1);
        assert_eq!(
            direct[0].alpha, roundabout[0].alpha,
            "two specifiers that resolve to the same real file must produce the same alpha, even \
             written from different directories and spelled differently"
        );
    }

    /// Resolution regression, real-resolution flavor of the lexical-anchoring test above: two
    /// directories that each
    /// contain their *own*, different `helpers.ts` and both write the textually identical
    /// `import { helper } from './helpers'` must resolve to two different files and must not
    /// collapse to the same `alpha` -- the exact case real resolution must not regress relative to
    /// lexical anchoring.
    #[test]
    fn free_import_identity_differs_when_the_same_specifier_text_resolves_to_different_files() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_a = dir.path().join("pkgA");
        let pkg_b = dir.path().join("pkgB");
        std::fs::create_dir(&pkg_a).unwrap();
        std::fs::create_dir(&pkg_b).unwrap();
        std::fs::write(pkg_a.join("helpers.ts"), "export const helper = 1;").unwrap();
        std::fs::write(pkg_b.join("helpers.ts"), "export const helper = 2;").unwrap();

        let source = "import { helper } from './helpers';\ntest('x', () => { helper(1); });";
        let a = fingerprint_all_at(source, &pkg_a.join("a.test.ts"));
        let b = fingerprint_all_at(source, &pkg_b.join("b.test.ts"));
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_ne!(
            a[0].alpha, b[0].alpha,
            "`./helpers` resolved from two different directories, each containing its own real \
             `helpers.ts`, must name two different files and must not produce the same alpha"
        );
    }

    /// An import specifier that cannot be resolved (no matching file on disk) must never
    /// produce an identity that collides with a real, resolved import -- even when the unresolved
    /// specifier's own anchored text would otherwise print identically to part of a resolved
    /// identity. This is the direct test of the `resolved:`/`unresolved:` tag split.
    #[test]
    fn unresolved_import_identity_never_collides_with_a_resolved_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("helpers.ts"), "export const helper = 1;").unwrap();

        let resolved_source =
            "import { helper } from './helpers';\ntest('x', () => { helper(1); });";
        let unresolved_source =
            "import { helper } from './missing';\ntest('x', () => { helper(1); });";

        let resolved = fingerprint_all_at(resolved_source, &dir.path().join("a.test.ts"));
        let unresolved = fingerprint_all_at(unresolved_source, &dir.path().join("b.test.ts"));
        assert_eq!(resolved.len(), 1);
        assert_eq!(unresolved.len(), 1);
        assert_ne!(
            resolved[0].alpha, unresolved[0].alpha,
            "a resolved import and an unresolvable one must never produce the same alpha, \
             regardless of how similar their specifier text looks"
        );
    }

    // ---------------------------------------------------------------------------------------
    // Imported-helper inlining: an imported helper's body participates in `alpha`, so two tests
    // differing only by calling equivalent helpers compare by what the calls do (see
    // `helpers.rs`).
    // ---------------------------------------------------------------------------------------

    /// Writes one helper file into `dir`, the shared setup half of every test below.
    fn write_helper(dir: &std::path::Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Fingerprints a one-test source that calls `import_specifier`'s export `export_name`
    /// with `argument`, and returns that test's `alpha`. The shared exercise half: every test
    /// below asks the same question (what did the call render to) and differs only in what is on
    /// disk. The import is aliased to one local name so the call site is byte-identical across
    /// every test, whatever the export is called.
    fn alpha_of_helper_call(
        dir: &std::path::Path,
        import_specifier: &str,
        export_name: &str,
        argument: &str,
    ) -> String {
        let source = format!(
            "import {{ {export_name} as callme }} from '{import_specifier}';\n             test('x', () => {{ callme({argument}); }});"
        );
        let prints = fingerprint_all_at(&source, &dir.join("a.test.ts"));
        assert_eq!(
            prints.len(),
            1,
            "expected exactly one test body for {source:?}"
        );
        prints[0].alpha.clone()
    }

    /// Depth threading: a call site near [`MAX_DEPTH`] leaves the inlined helper no second
    /// budget to spend, so its body degrades (`<max-depth:...>`) instead of rendering fully
    /// from depth zero -- keeping the in-use stack within one pass's measured bound.
    #[test]
    fn a_helper_called_near_the_depth_budget_degrades_instead_of_double_spending_it() {
        let dir = tempfile::tempdir().unwrap();
        write_helper(
            dir.path(),
            "helpers.ts",
            "export function check(label) { return label.length > 0; }",
        );

        // 125 array levels put the call site near [`MAX_DEPTH`]: still reachable (the wrapper
        // renders), but with the caller's depth threaded into the helper's serialization there is
        // no second full budget to spend, so the helper body degrades -- its `<number:0>` literal
        // is gone. 120 levels leave enough budget to render it. Without threading, the helper's
        // statements restarted at depth 0 and rendered fully from any call site.
        for (levels, expect_full_body) in [(120, true), (125, false)] {
            let source = format!(
            "import {{ check }} from './helpers';\ntest('x', () => {{ const deep = {}check('dashboard'){}; }});",
            "[".repeat(levels),
            "]".repeat(levels),
        );
            let prints = fingerprint_all_at(&source, &dir.path().join("a.test.ts"));
            assert_eq!(prints.len(), 1);
            let alpha = &prints[0].alpha;
            assert!(
                alpha.contains("helper-inline"),
                "at {levels} levels the call is still reachable and must inline, got {alpha}"
            );
            assert_eq!(
                alpha.contains("<number:0>"),
                expect_full_body,
                "at {levels} levels the helper body's literal {} render",
                if expect_full_body { "must" } else { "must not" }
            );
        }
    }

    /// The core claim: two helpers, in two different modules, spelled with entirely different
    /// names, whose bodies are alpha-equivalent -- two tests calling them with the same argument
    /// must produce the same `alpha`. Before inlining, the call sites rendered as two different
    /// `free:import` identities and the pair could never match.
    #[test]
    fn equivalent_imported_helpers_inline_to_the_same_alpha() {
        let dir = tempfile::tempdir().unwrap();
        write_helper(
            dir.path(),
            "helpers-a.ts",
            "import { expect } from 'vitest';\n\
             export function checkActive(label: string) {\n\
                 const trimmed = label.trim();\n\
                 expect(trimmed.length).toBeGreaterThan(0);\n\
             }",
        );
        write_helper(
            dir.path(),
            "helpers-b.ts",
            "import { expect } from 'vitest';\n\
             export function assertNonEmpty(text: string) {\n\
                 const cleaned = text.trim();\n\
                 expect(cleaned.length).toBeGreaterThan(0);\n\
             }",
        );

        let a = alpha_of_helper_call(dir.path(), "./helpers-a", "checkActive", "'dashboard'");
        let b = alpha_of_helper_call(dir.path(), "./helpers-b", "assertNonEmpty", "'dashboard'");
        assert!(
            a.contains("helper-inline"),
            "the call must render the helper's body, got {a}"
        );
        assert_eq!(
            a, b,
            "two tests differing only by calling equivalent imported helpers must produce the \
             same alpha"
        );
    }

    /// The one-edit negative twin: helper B's body differs by one literal, and that difference
    /// must survive inlining -- the alphas differ, which is what keeps the equivalent case's
    /// equality meaningful.
    #[test]
    fn inlined_helper_bodies_keep_their_distinguishing_literals() {
        let dir = tempfile::tempdir().unwrap();
        write_helper(
            dir.path(),
            "helpers-a.ts",
            "export function check(label) { return label.length > 0; }",
        );
        write_helper(
            dir.path(),
            "helpers-b.ts",
            "export function assertNonEmpty(text) { return text.length > 1; }",
        );

        let a = alpha_of_helper_call(dir.path(), "./helpers-a", "check", "'dashboard'");
        let b = alpha_of_helper_call(dir.path(), "./helpers-b", "assertNonEmpty", "'dashboard'");
        // Prove the pair actually inlined before trusting the inequality: if a refusal ever
        // regressed both sides back to identity rendering, they would still differ (two
        // different exports) and the assertion below would pass vacuously -- review-found shape.
        assert!(
            a.contains("helper-inline") && b.contains("helper-inline"),
            "both sides must render the helper's body for the inequality to mean anything, got \
             {a} / {b}"
        );
        assert_ne!(
            a, b,
            "a literal difference inside an inlined helper body must stay visible in alpha"
        );
    }

    /// A written parameter (`x = 5`, `x += 1`, `x++`, a destructuring write) makes
    /// substitution unsound: the write renders as a canonical local while later reads still
    /// render the original argument, so `f(x) { x = 5; return x; }` called `f(1)` and
    /// `g(x) { x = 5; return 1; }` called `g(1)` would render identically while returning 5
    /// and 1 (review-found false merge at Error severity). The whole export is refused and
    /// both calls keep their identity rendering.
    #[test]
    fn helpers_that_write_a_parameter_refuse_inlining_and_stay_distinct() {
        let dir = tempfile::tempdir().unwrap();
        write_helper(
            dir.path(),
            "writes-then-reads.ts",
            "export function f(x) { x = 5; return x; }",
        );
        write_helper(
            dir.path(),
            "writes-then-literal.ts",
            "export function g(x) { x = 5; return 1; }",
        );

        let f = alpha_of_helper_call(dir.path(), "./writes-then-reads", "f", "1");
        let g = alpha_of_helper_call(dir.path(), "./writes-then-literal", "g", "1");
        assert!(
            f.contains("free:import") && !f.contains("helper-inline"),
            "a helper that writes its parameter keeps the identity rendering, got {f}"
        );
        assert!(
            g.contains("free:import") && !g.contains("helper-inline"),
            "got {g}"
        );
        assert_ne!(
            f, g,
            "the review pair must stay distinct: they return 5 and 1 for the same argument"
        );
    }

    /// An argument no parameter read consumes is still observable at runtime -- directly
    /// ignored here, but indexed through `arguments` there -- so it must stay visible in the
    /// rendered wrapper. Two calls to the same helper differing only in an UNREAD argument
    /// would otherwise render identically and falsely merge (review-found).
    #[test]
    fn unread_arguments_stay_visible_in_the_inlined_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        write_helper(
            dir.path(),
            "ignores-second.ts",
            "export function check(label, unused) { return label.length > 0; }",
        );
        write_helper(
            dir.path(),
            "indexes-arguments.ts",
            "export function check(label, second) { return arguments[1].length > 0; }",
        );

        // The ignored-argument shape: same helper, differing only in the argument no read
        // consumes -- distinct, and the difference is visible as an `(args ...)` tail.
        let a = alpha_of_helper_call(dir.path(), "./ignores-second", "check", "'a', 'x'");
        let b = alpha_of_helper_call(dir.path(), "./ignores-second", "check", "'a', 'y'");
        assert!(
            a.contains("(args"),
            "the unread argument must render in an args tail, got {a}"
        );
        assert_ne!(
            a, b,
            "two calls differing only in an unread argument must not render alike"
        );

        // The `arguments` channel: the body reads neither parameter by name, so without the
        // tail both calls would render identically while returning different values.
        let indexed_a =
            alpha_of_helper_call(dir.path(), "./indexes-arguments", "check", "'a', 'x'");
        let indexed_b =
            alpha_of_helper_call(dir.path(), "./indexes-arguments", "check", "'a', 'y'");
        assert_ne!(
            indexed_a, indexed_b,
            "a difference observable only through `arguments` must still separate the calls"
        );

        // And the equal twin: identical arguments, both shapes, stay comparable to themselves.
        assert_eq!(
            a,
            alpha_of_helper_call(dir.path(), "./ignores-second", "check", "'a', 'x'")
        );
    }

    /// `structural` never inlines: `parameterization-candidate`'s remedy is a table in the *test
    /// file*, which cannot parameterize literals living inside helper modules. Pinned here so
    /// the alpha-only scope is a tested contract, not a comment.
    #[test]
    fn structural_keeps_the_identity_rendering_for_imported_helpers() {
        let dir = tempfile::tempdir().unwrap();
        write_helper(
            dir.path(),
            "helpers.ts",
            "export function check(label) { return label; }",
        );
        let source =
            "import { check } from './helpers';\ntest('x', () => { check('dashboard'); });";
        let prints = fingerprint_all_at(source, &dir.path().join("a.test.ts"));
        assert_eq!(prints.len(), 1);
        assert!(
            prints[0].alpha.contains("helper-inline"),
            "alpha inlines the helper body"
        );
        assert!(
            prints[0].structural.contains("free:import"),
            "structural keeps the export-identity rendering, got {}",
            prints[0].structural
        );
        assert!(
            !prints[0].structural.contains("helper-inline"),
            "structural must never inline an imported helper"
        );
    }

    /// The framework itself is never inlined, even when `vitest` genuinely resolves to a real,
    /// inlinable-looking export on disk: a call to `expect` is not "the test containing the
    /// framework's assertions", and the identity must not depend on whatever build is installed.
    #[test]
    fn vitest_specifier_is_never_inlined() {
        let dir = tempfile::tempdir().unwrap();
        write_helper(
            dir.path(),
            "node_modules/vitest/package.json",
            r#"{"name":"vitest","main":"index.js"}"#,
        );
        write_helper(
            dir.path(),
            "node_modules/vitest/index.js",
            "export function expect(actual) { return { toBe(expected) { return actual === expected; } }; }",
        );

        let source = "import { expect as check } from 'vitest';\ntest('x', () => { check(1); });";
        let prints = fingerprint_all_at(source, &dir.path().join("a.test.ts"));
        assert_eq!(prints.len(), 1);
        assert!(
            !prints[0].alpha.contains("helper-inline"),
            "a call through a 'vitest' import must keep its identity rendering even when the \
             module resolves to a real, callable export, got {}",
            prints[0].alpha
        );
        assert!(
            prints[0].alpha.contains("free:import"),
            "the framework call renders as a free-import identity"
        );
    }

    /// Exact arity is an admission requirement: a call whose arguments cannot bind
    /// parameter-for-parameter keeps the identity rendering, because there is no argument whose
    /// rendering could stand in for the unbound parameter.
    #[test]
    fn arity_mismatch_is_not_inlined() {
        let dir = tempfile::tempdir().unwrap();
        write_helper(
            dir.path(),
            "helpers.ts",
            "export function check(label, minimum) { return label.length > minimum; }",
        );
        let alpha = alpha_of_helper_call(dir.path(), "./helpers", "check", "'dashboard'");
        assert!(
            alpha.contains("free:import"),
            "an arity-mismatched call keeps the identity rendering, got {alpha}"
        );
    }

    /// `export const name = <arrow>` is an admitted shape, and the wrapper names the callable
    /// form (`arrow-fn`) because `this` binding differs from a `function` even with identical
    /// statements.
    #[test]
    fn const_arrow_helpers_inline_with_their_form_named() {
        let dir = tempfile::tempdir().unwrap();
        write_helper(
            dir.path(),
            "helpers.ts",
            "export const check = (label) => label.length > 0;",
        );
        let alpha = alpha_of_helper_call(dir.path(), "./helpers", "check", "'dashboard'");
        assert!(
            alpha.contains("(helper-inline arrow-fn"),
            "an inlined const-arrow names its form, got {alpha}"
        );
    }

    /// A named re-export hop (`export { x } from './inner'`) resolves to the inner declaration:
    /// the helper's body participates even when the public module only re-exports it -- the
    /// barrel shape real repositories use for shared runners.
    #[test]
    fn reexported_helpers_inline_through_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        write_helper(
            dir.path(),
            "helpers-a.ts",
            "export { check } from './inner-a';",
        );
        write_helper(
            dir.path(),
            "inner-a.ts",
            "export function check(label) { return label.trim().length > 0; }",
        );
        write_helper(
            dir.path(),
            "helpers-b.ts",
            "export { assertNonEmpty } from './inner-b';",
        );
        write_helper(
            dir.path(),
            "inner-b.ts",
            "export function assertNonEmpty(text) { return text.trim().length > 0; }",
        );

        let a = alpha_of_helper_call(dir.path(), "./helpers-a", "check", "'dashboard'");
        let b = alpha_of_helper_call(dir.path(), "./helpers-b", "assertNonEmpty", "'dashboard'");
        assert!(
            a.contains("helper-inline"),
            "a re-exported helper's body must still be reached, got {a}"
        );
        assert_eq!(
            a, b,
            "equivalent helpers behind equivalent re-export chains must produce the same alpha"
        );
    }

    /// The reviewer's own probe, pinned: `return x + y` against `return y + y`, called with
    /// two distinct caller locals. The fragments differ (`id:L0`+`id:L1` vs `id:L1` twice),
    /// so the alphas differ; the namespacing below makes this true by construction rather
    /// than by allocation order.
    #[test]
    fn param_pairing_survives_substitution_in_both_orders() {
        let dir = tempfile::tempdir().unwrap();
        write_helper(
            dir.path(),
            "sums.ts",
            "export function p(x, y) { return x + y; }\nexport function q(x, y) { return y + y; }",
        );
        let p = "import { p as callme } from './sums';\ntest('x', () => { const a = 1; const b = 2; callme(a, b); });";
        let q = "import { q as callme } from './sums';\ntest('x', () => { const a = 1; const b = 2; callme(a, b); });";
        let p_prints = fingerprint_all_at(p, &dir.path().join("p.test.ts"));
        let q_prints = fingerprint_all_at(q, &dir.path().join("q.test.ts"));
        assert_eq!(p_prints.len(), 1);
        assert_eq!(q_prints.len(), 1);
        assert!(
            p_prints[0].alpha.contains("helper-inline")
                && q_prints[0].alpha.contains("helper-inline"),
            "both sides must inline for the comparison to mean anything"
        );
        assert_ne!(
            p_prints[0].alpha, q_prints[0].alpha,
            "x + y and y + y over distinct caller locals must not render alike"
        );
    }

    /// Helper-local canonical names and caller-local names in substituted arguments must not
    /// collide: the helper's own locals and the caller's locals are numbered by *separate*
    /// passes, and a false equality here would merge two different tests. One side reads a
    /// parameter substituted with a caller local (`id:L0` from the caller's pass); the other
    /// declares and reads its own local (`id:L0` from the helper's own pass, plus the
    /// declaration the caller's side cannot have). They must stay distinct.
    #[test]
    fn helper_locals_and_caller_locals_render_without_colliding() {
        let dir = tempfile::tempdir().unwrap();
        write_helper(
            dir.path(),
            "reads-param.ts",
            "export function f(x) { return x; }",
        );
        write_helper(
            dir.path(),
            "reads-local.ts",
            "export function g() { const x = 1; return x; }",
        );

        let with_caller_local = r#"import { f } from './reads-param';
test('x', () => { const u = 1; f(u); });"#;
        let with_helper_local = r#"import { g } from './reads-local';
test('x', () => { g(); });"#;
        let a = fingerprint_all_at(with_caller_local, &dir.path().join("a.test.ts"));
        let b = fingerprint_all_at(with_helper_local, &dir.path().join("b.test.ts"));
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_ne!(
            a[0].alpha, b[0].alpha,
            "a parameter substituted with a caller local and a helper's own local must not \
             render alike just because both canonical names spell `id:L0`"
        );
        // The mechanism, made observable: the helper's own locals render in their own
        // namespace, distinct by construction from every caller-local name in the fragments.
        assert!(
            b[0].alpha.contains("id:H0"),
            "a helper-declared local renders in the helper namespace, got {}",
            b[0].alpha
        );

        // The equal twin must stay equal: same helper, differently named caller local -- both
        // arguments canonicalize to the same caller-side name, so the alphas agree.
        let renamed = r#"import { f } from './reads-param';
test('x', () => { const w = 1; f(w); });"#;
        let c = fingerprint_all_at(renamed, &dir.path().join("c.test.ts"));
        assert_eq!(c.len(), 1);
        assert_eq!(
            a[0].alpha, c[0].alpha,
            "renaming the caller local passed as an argument must not change the alpha"
        );
    }

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
    /// The substrate both depth-guard tests serialize their synthetic ASTs on: a trivial
    /// one-test parse, empty maps, a fresh Alpha serializer. A macro rather than a function
    /// because the serializer borrows four locals that must outlive it, and the two tests
    /// differ in what they measure around the `ser_expr` call itself.
    macro_rules! with_trivial_serializer {
        (| $serializer:ident | $body:expr) => {{
            let trivial_allocator = Allocator::default();
            let trivial_source = "test('x', () => {});";
            let trivial_type =
                SourceType::from_path(std::path::Path::new("trivial.test.ts")).unwrap();
            let trivial_parsed =
                Parser::new(&trivial_allocator, trivial_source, trivial_type).parse();
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
                    &row_substitution,
                    &dataflow,
                ),
                Mode::Alpha,
            );
            $body
        }};
    }

    #[test]
    fn ser_expr_depth_guard_bounds_an_ast_deeper_than_the_parser_could_produce() {
        let allocator = Allocator::default();
        let builder = AstBuilder::new(&allocator);
        let mut expr = Expression::new_boolean_literal(SPAN, true, &builder);
        let synthetic_depth = MAX_DEPTH as usize * 8;
        for _ in 0..synthetic_depth {
            expr =
                Expression::new_unary_expression(SPAN, UnaryOperator::LogicalNot, expr, &builder);
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
            expr =
                Expression::new_unary_expression(SPAN, UnaryOperator::LogicalNot, expr, &builder);
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
    // Dataflow canonicalization: the three `near-duplicate-test` transformations added on top of
    // reordered-setup multiset equality. Each has a positive-convergence test, a
    // precondition-violating case that must NOT converge, and (once, covering all three) a
    // determinism check. See `fixtures/duplicates/near-*-positive.test.ts` and their twins for the
    // fixture-level versions of the same claims.
    // -----------------------------------------------------------------------------------------

    #[test]
    fn dataflow_inlines_single_use_local_to_match_inlined_call() {
        let prints = fingerprint_all(
            "import { computeTotal } from './cart';\n\
             test('via local', () => { const cart = [10, 20]; const total = computeTotal(cart); \
                 expect(total).toBe(30); });\n\
             test('inline', () => { const cart = [10, 20]; expect(computeTotal(cart)).toBe(30); });",
        );
        let (a, b) = sorted_behavior_pair(&prints);
        assert_eq!(
            a, b,
            "a local read exactly once must inline to match the equivalent inlined call: {a:?} \
             vs {b:?}"
        );
    }

    /// The read-site half of single-use inlining, pinned in all three directions -- the boundary
    /// `fixtures/observed/observed-single-use-local-erases-behavior.test.ts` distills. Before
    /// `walk_inlined_read` existed, only the assertion-subject direction worked and the other two
    /// silently dropped the initializer's `call:` events.
    #[test]
    fn dataflow_inlined_read_outside_an_assertion_keeps_initializer_events() {
        let prints = fingerprint_all(
            "import { buildEmpty, buildFrom } from './builders';\n\
             test('from', () => { const subject = buildFrom(1, 2, 3); const result = \
                 subject.run(); expect(result).toBe(1); expect(result).toEqual(2); });\n\
             test('empty', () => { const subject = buildEmpty(); const result = subject.run(); \
                 expect(result).toBe(1); expect(result).toEqual(2); });",
        );
        assert_eq!(prints.len(), 2, "expected exactly two test bodies");
        for (index, expected) in [(0usize, "buildFrom"), (1usize, "buildEmpty")] {
            assert!(
                prints[index]
                    .behavior
                    .iter()
                    .any(|event| event.starts_with("call:") && event.contains(expected)),
                "a single-use local read outside an assertion must still account for its \
                 initializer's call event ({expected}): {:?}",
                prints[index].behavior
            );
        }
        let mut a = prints[0].behavior.clone();
        let mut b = prints[1].behavior.clone();
        a.sort();
        b.sort();
        assert_ne!(
            a, b,
            "two tests building genuinely different subjects into a single-use local must not \
             produce equal behavior multisets -- that collision is the `near-duplicate-test` \
             false positive this re-emission exists to close"
        );
    }

    /// The same shape with the local read TWICE: the read-once gate declines to inline, so the
    /// declaration's own event survives and the two tests still differ. This is the "inlining is
    /// the trigger, not the bodies" half of the boundary.
    #[test]
    fn dataflow_local_read_twice_outside_an_assertion_stays_distinct() {
        let prints = fingerprint_all(
            "import { buildEmpty, buildFrom } from './builders';\n\
             test('from', () => { const subject = buildFrom(1, 2, 3); const first = \
                 subject.run(); const second = subject.run(); expect(first).toBe(second); });\n\
             test('empty', () => { const subject = buildEmpty(); const first = subject.run(); \
                 const second = subject.run(); expect(first).toBe(second); });",
        );
        assert_eq!(prints.len(), 2, "expected exactly two test bodies");
        let mut a = prints[0].behavior.clone();
        let mut b = prints[1].behavior.clone();
        a.sort();
        b.sort();
        assert_ne!(a, b, "{a:?} vs {b:?}");
    }

    /// The assertion-SUBJECT direction, which `shape` already covered: it must keep working, and
    /// it must not acquire a duplicate `call:` event now that the read site also emits. The
    /// inlined twin emits no event for `buildFrom` at all (an assertion is one atomic event), so
    /// an extra event here would be the over-correction that splits
    /// `near-extracted-local-positive` apart.
    #[test]
    fn dataflow_inlined_read_as_assertion_subject_is_not_double_counted() {
        let prints = fingerprint_all(
            "import { buildFrom } from './builders';\n\
             test('extracted', () => { const subject = buildFrom(1, 2, 3); \
                 expect(subject).toBe(1); });\n\
             test('inlined', () => { expect(buildFrom(1, 2, 3)).toBe(1); });",
        );
        assert_eq!(prints.len(), 2, "expected exactly two test bodies");
        assert_eq!(
            prints[0].behavior, prints[1].behavior,
            "an extracted single-use local read as an assertion subject must still match the \
             inlined form exactly, with no extra call event"
        );
    }

    /// Order, not just multiset: the re-emitted events must land in the same *sequence* the
    /// inlined body produces, because `subsumed-test` keys on an ordered subsequence while
    /// `near-duplicate-test` keys on a sorted multiset. Walking the initializer back at its
    /// declaration would satisfy the multiset and fail this.
    #[test]
    fn dataflow_inlined_read_preserves_the_inlined_body_event_order() {
        let prints = fingerprint_all(
            "import { build, run } from './builders';\n\
             test('extracted', () => { const subject = build(1); run(subject); });\n\
             test('inlined', () => { run(build(1)); });",
        );
        assert_eq!(prints.len(), 2, "expected exactly two test bodies");
        assert_eq!(
            prints[0].behavior, prints[1].behavior,
            "the extracted form must emit the initializer's events in the same order the \
             inlined form does (enclosing call first, argument second)"
        );
        let run_position = prints[1]
            .behavior
            .iter()
            .position(|event| event.starts_with("call:") && event.contains(":run args="))
            .expect("run call event");
        let build_position = prints[1]
            .behavior
            .iter()
            .position(|event| event.starts_with("call:") && event.contains(":build args="))
            .expect("build call event");
        assert!(
            run_position < build_position,
            "guard on the premise of this test -- the inlined form emits the enclosing call \
             before its argument: {:?}",
            prints[1].behavior
        );
    }

    #[test]
    fn single_use_inlining_composes_with_reordered_setup() {
        let prints = fingerprint_all(
            "import { grantAccess, loadProfile } from './accounts';\n\
             test('direct', () => { const user = loadProfile(1); grantAccess(1); \
                 expect(user.email).toContain('@'); });\n\
             test('reordered', () => { grantAccess(1); const record = loadProfile(1); \
                 const email = record.email; expect(email).toContain('@'); });",
        );
        assert_eq!(prints.len(), 2);
        let mut direct = prints[0].behavior.clone();
        let mut reordered = prints[1].behavior.clone();
        direct.sort();
        reordered.sort();
        assert_eq!(
            direct, reordered,
            "single-use extraction must not defeat the behavior multiset's setup reordering: \
             {direct:?} vs {reordered:?}"
        );
    }

    #[test]
    fn effectful_two_step_extraction_still_matches_the_inline_form() {
        let prints = fingerprint_all(
            "import { fetchRow } from './rows';\n\
             test('extracted', () => { const raw = fetchRow(1); const name = raw.name; \
                 expect(name).toBe('x'); });\n\
             test('inline', () => { expect(fetchRow(1).name).toBe('x'); });",
        );
        assert_eq!(prints.len(), 2);
        let mut extracted = prints[0].behavior.clone();
        let mut inline = prints[1].behavior.clone();
        extracted.sort();
        inline.sort();
        assert_eq!(
            extracted, inline,
            "the composed-reordering guard must not disable ordinary effectful extraction"
        );
    }

    #[test]
    fn dataflow_does_not_inline_local_read_twice() {
        // Mirrors `fixtures/duplicates/near-extracted-local-evaluation-twin.test.ts`: a local read
        // twice must not be inlined, because a side-effecting initializer (here `nextId`) would
        // then appear evaluated once instead of twice.
        let prints = fingerprint_all(
            "import { nextId } from './ids';\n\
             test('x', () => { const id = nextId(); expect(id).toBe(1); expect(id).toBe(1); });",
        );
        let behavior = &prints[0].behavior;
        assert!(
            behavior
                .iter()
                .any(|event| event.starts_with("call:") && event.contains("nextId")),
            "a local read twice must not be inlined -- the declaration's own call event must \
             survive standalone: {behavior:?}"
        );
    }

    #[test]
    fn dataflow_inline_blocked_by_intervening_effect() {
        let prints = fingerprint_all(
            "import { compute, track } from './m';\n\
             test('x', () => { const total = compute(); track(); expect(total).toBe(1); });",
        );
        let behavior = &prints[0].behavior;
        assert!(
            behavior
                .iter()
                .any(|event| event.starts_with("call:") && event.contains("compute")),
            "a side-effecting statement between declaration and read must block single-use \
             inlining, so the declaration's own call event must survive standalone: {behavior:?}"
        );
    }

    #[test]
    fn dataflow_destructure_renders_as_member_access() {
        let prints = fingerprint_all(
            "import { config } from './config';\n\
             test('destructure', () => { const { retries } = config; expect(retries).toBe(3); });\n\
             test('member', () => { expect(config.retries).toBe(3); });",
        );
        let (a, b) = sorted_behavior_pair(&prints);
        assert_eq!(
            a, b,
            "a destructured property read must render as the equivalent member access: {a:?} \
             vs {b:?}"
        );
    }

    /// The bug [`DataflowSubst::DestructuredProperty`] exists for, in its measured shape: the destructured
    /// object is a *call*, so "destructuring to member access" declines and the property name had
    /// nowhere left to go. `alpha` and `structural` both keep it (they serialize the binding
    /// pattern), so only `behavior` -- `near-duplicate-test` and `subsumed-test` -- was exposed.
    #[test]
    fn a_destructured_binding_name_survives_into_the_behavior_event() {
        let prints = fingerprint_all(
            "import { useThing } from './thing';\n\
             test('a', () => { const { isEveryTag } = useThing(); expect(isEveryTag.value).toBe(true); });\n\
             test('b', () => { const { isSameAsInitial } = useThing(); expect(isSameAsInitial.value).toBe(true); });",
        );
        assert_eq!(prints.len(), 2, "expected exactly two test bodies");
        assert_ne!(
            prints[0].alpha, prints[1].alpha,
            "precondition: the two bodies were never alpha-equal, so `duplicate-test` was never \
             the rule at risk here"
        );
        let mut a = prints[0].behavior.clone();
        let mut b = prints[1].behavior.clone();
        a.sort();
        b.sort();
        assert_ne!(
            a, b,
            "two tests reading different destructured properties of the same factory call must \
             not share a behavior multiset: {a:?} vs {b:?}"
        );
        assert!(
            a.iter().any(|event| event.contains(":prop:isEveryTag")),
            "the property key is what distinguishes them, so it must appear in the event: {a:?}"
        );
    }

    /// The callee-position half of the same erasure: the destructured binding is *called*, not
    /// read into an assertion, so the name has to reach `call_path`, not just `shape`.
    #[test]
    fn a_destructured_callee_name_survives_into_the_behavior_event() {
        let prints = fingerprint_all(
            "import { useApi } from './api';\n\
             test('a', async () => { const { fetchUser } = useApi(); await fetchUser(1); });\n\
             test('b', async () => { const { deleteUser } = useApi(); await deleteUser(1); });",
        );
        let (a, b) = sorted_behavior_pair(&prints);
        assert_ne!(
            a, b,
            "two tests calling different methods destructured off the same factory must not \
             share a behavior multiset: {a:?} vs {b:?}"
        );
    }

    /// The decoration must stay inside `behavior`. `alpha` and `structural` are handed a
    /// permanently empty dataflow map (see `fingerprint`), which is what keeps `duplicate-test`
    /// at Error severity and `parameterization-candidate` unmoved by this entry kind; this pins
    /// that wiring rather than trusting it.
    #[test]
    fn a_destructured_binding_name_never_reaches_alpha_or_structural() {
        let prints = fingerprint_all(
            "import { useThing } from './thing';\n\
             test('a', () => { const { isEveryTag } = useThing(); expect(isEveryTag.value).toBe(true); });",
        );
        assert!(
            !prints[0].alpha.contains("L0:prop:") && !prints[0].structural.contains("L0:prop:"),
            "the behavior-only decoration leaked into a claimed identity: alpha={} structural={}",
            prints[0].alpha,
            prints[0].structural
        );
    }

    /// The same erasure one nesting level down, found by review after the flat case was fixed:
    /// both bodies bind one local whose own key is `id`, so retaining only the innermost key left
    /// them with the identical behavior event. Probes the nested shape rather than restating the
    /// flat one.
    #[test]
    fn a_nested_destructured_binding_keeps_the_whole_property_path() {
        let prints = fingerprint_all(
            "import { useThing } from './thing';\n\
             test('a', () => { const { user: { id } } = useThing(); expect(id).toBe(1); });\n\
             test('b', () => { const { post: { id } } = useThing(); expect(id).toBe(1); });",
        );
        assert_eq!(prints.len(), 2, "expected exactly two test bodies");
        assert_ne!(
            prints[0].behavior, prints[1].behavior,
            "a nested destructured binding must carry the key it was nested under, not only its \
             own: {:?} vs {:?}",
            prints[0].behavior, prints[1].behavior
        );
    }

    /// The path is `:`-joined, not `.`-joined, because it is spliced into a call path where `.`
    /// is the member separator. With a dotted join these two bodies -- one calling a doubly
    /// destructured `get`, the other calling `.get` on a singly destructured `api` -- both render
    /// `call:L0:prop:api.get`. Probes the collision, not a comfortable variant of it.
    #[test]
    fn a_nested_path_cannot_imitate_a_member_call_path() {
        let prints = fingerprint_all(
            "import { useThing } from './thing';\n\
             test('a', () => { const { api: { get } } = useThing(); get(); });\n\
             test('b', () => { const { api } = useThing(); api.get(); });",
        );
        assert_eq!(prints.len(), 2, "expected exactly two test bodies");
        assert_ne!(
            prints[0].behavior, prints[1].behavior,
            "a nested property path must not render the same as the member access it resembles: \
             {:?} vs {:?}",
            prints[0].behavior, prints[1].behavior
        );
    }

    /// A retained key is appended to an event raw, so a key able to spell the event grammar's own
    /// punctuation lets one body's event imitate another's. Both bodies here are real, parseable
    /// TypeScript, and before `collect_pattern` restricted itself to keys written as identifiers
    /// they emitted the identical `["call:...o", "call:L0:prop:x(<number:1>)"]` -- a
    /// self-inflicted `near-duplicate-test` false positive of exactly the kind
    /// `escape_literal_delimiters` exists to prevent for literals. Probes the shortest forgery
    /// found, not a comfortable variant of it.
    #[test]
    fn a_top_level_quoted_key_cannot_register_a_destructured_property_event() {
        let prints = fingerprint_all(
            "import { o } from './o';\n\
             test('identifier', () => { const { x } = o; x(1); });\n\
             test('quoted', () => { const { 'x(<number:1>)': f } = o; f(); });",
        );
        assert_eq!(prints.len(), 2, "expected exactly two test bodies");
        assert_ne!(
            prints[0].behavior, prints[1].behavior,
            "the top-level destructuring-to-member scan must apply the same quoted-key guard as \
             collect_pattern; otherwise its inserted DestructuredProperty entry bypasses the guard"
        );
        assert!(
            !prints[1]
                .behavior
                .iter()
                .any(|event| event.contains(":prop:")),
            "quoted keys must not be spliced into behavior grammar: {:?}",
            prints[1].behavior
        );
    }

    #[test]
    fn a_quoted_key_cannot_forge_another_body_s_event() {
        let prints = fingerprint_all(
            "import { o } from './o';\n\
             test('a', () => { const { x } = o(); x(1); });\n\
             test('b', () => { const { 'x(<number:1>)': f } = o(); f(); });",
        );
        assert_eq!(prints.len(), 2, "expected exactly two test bodies");
        assert_ne!(
            prints[0].behavior, prints[1].behavior,
            "a quoted destructuring key spelled to imitate a call's argument fragment must not \
             produce the same behavior events as the call it imitates: {:?} vs {:?}",
            prints[0].behavior, prints[1].behavior
        );
    }

    /// The hazard the `or_insert` precedence in `analyze_dataflow` guards: a symbol
    /// "destructuring to member access" already claimed must keep rendering as `object.property`,
    /// or `const { a } = o; use(a)` stops matching `use(o.a)`. `dataflow_destructure_renders_as_
    /// member_access` pins the equivalence itself; this pins the *reason* it survives, so an
    /// overwrite-instead-of-`or_insert` regression is named by the failure rather than guessed at.
    #[test]
    fn a_member_access_substitution_outranks_the_binding_name() {
        let prints = fingerprint_all(
            "import { config } from './config';\n\
             test('destructure', () => { const { retries } = config; expect(retries).toBe(3); });",
        );
        assert!(
            !prints[0]
                .behavior
                .iter()
                .any(|event| event.contains(":prop:")),
            "a substituted destructured property must render as the member access alone, with no \
             binding-name decoration to break the equivalence: {:?}",
            prints[0].behavior
        );
    }

    #[test]
    fn dataflow_destructure_blocked_by_intervening_mutation() {
        // Mirrors `fixtures/duplicates/near-destructure-mutation-twin.test.ts`.
        let prints = fingerprint_all(
            "test('x', () => { \
                 const state = { count: 1 }; \
                 const { count } = state; \
                 state.count = 2; \
                 expect(count).toBe(1); \
             });",
        );
        let behavior = &prints[0].behavior;
        assert!(
            behavior.iter().any(|event| event.contains(":prop:count")),
            "a mutation between the destructuring declaration and its read must block \
             member-access canonicalization while retaining property identity: {behavior:?}"
        );
    }

    #[test]
    fn dataflow_await_then_matches_bare_await() {
        let prints = fingerprint_all(
            "import { setup, state } from './pipeline';\n\
             test('await', async () => { await setup(); expect(state()).toBe('ready'); });\n\
             test('then', () => { return setup().then(() => { expect(state()).toBe('ready'); }); });",
        );
        let (a, b) = sorted_behavior_pair(&prints);
        assert_eq!(
            a, b,
            "a bare `await` and an equivalent single-argument `.then` chain must produce the \
             same behavior multiset: {a:?} vs {b:?}"
        );
    }

    #[test]
    fn dataflow_await_then_blocked_by_rejection_handler() {
        // Not covered by any `fixtures/` twin: the module doc and this transformation's own
        // precondition (a single awaited call, no interleaving and no rejection handler) both call
        // out a `.then(onFulfilled, onRejected)` as NOT equivalent to a bare `await`.
        let prints = fingerprint_all(
            "import { setup, state } from './pipeline';\n\
             test('await', async () => { await setup(); expect(state()).toBe('ready'); });\n\
             test('then', () => { return setup().then(() => { expect(state()).toBe('ready'); }, \
                 () => {}); });",
        );
        let (a, b) = sorted_behavior_pair(&prints);
        assert_ne!(
            a, b,
            "a `.then` with a rejection handler must NOT canonicalize to the same behavior \
             multiset as a bare await: {a:?} vs {b:?}"
        );
        assert!(
            prints[1].behavior.iter().any(|event| event == "return"),
            "a `.then` with a rejection handler must fall back to ordinary `return` event \
             collection, not the await canonicalization: {:?}",
            prints[1].behavior
        );
    }

    #[test]
    fn dataflow_canonicalization_is_deterministic() {
        let source = "import { computeTotal } from './cart';\n\
             test('via local', () => { const cart = [10, 20]; const total = computeTotal(cart); \
                 expect(total).toBe(30); });";
        let first = fingerprint_all(source);
        let second = fingerprint_all(source);
        assert_eq!(
            first[0].behavior, second[0].behavior,
            "dataflow canonicalization must be deterministic across separate fingerprinting runs \
             of the same source"
        );
    }

    // ---------------------------------------------------------------------------------------
    // Explicit type arguments
    // ---------------------------------------------------------------------------------------

    /// The Error-severity false positive this modeling exists for: two type-level assertions whose
    /// only difference is the type they assert about. Erasing the type arguments made these one
    /// `duplicate-test` at Error severity on correct code -- the same "a value's kind or content
    /// reduced to nothing" class as the literal-kind bug above, and the exact shape that made a
    /// type-level suite unusable.
    #[test]
    fn type_arguments_distinguish_otherwise_identical_calls() {
        let prints = fingerprint_all(
            "import { assertEqual } from './helpers';\n\
             type Expected = Record<string, unknown>;\n\
             test('string mapping', () => { assertEqual<Record<string, string>, Expected>(true); });\n\
             test('number mapping', () => { assertEqual<Record<string, number>, Expected>(true); });",
        );
        assert_eq!(prints.len(), 2, "expected exactly two test bodies");
        assert_ne!(
            prints[0].alpha, prints[1].alpha,
            "two type-level assertions differing only in their type arguments must not share an \
             `alpha`: that is a `duplicate-test` at Error severity on correct code"
        );
        assert_ne!(
            prints[0].structural, prints[1].structural,
            "a type argument cannot be lifted into a `test.each` row, so `structural` must keep \
             it too rather than proposing an impossible parameterization"
        );
        assert_ne!(
            prints[0].behavior, prints[1].behavior,
            "`near-duplicate-test`/`subsumed-test` key on behavior events, not `alpha` -- the \
             type arguments have to reach the events as well"
        );
    }

    /// The other direction of the same fix: modeling type arguments must not be a licence to make
    /// everything distinct. Two genuinely identical calls, type arguments included, still match.
    #[test]
    fn identical_type_arguments_still_compare_equal() {
        let prints = fingerprint_all(
            "import { assertEqual } from './helpers';\n\
             type Expected = Record<string, unknown>;\n\
             test('first', () => { assertEqual<Record<string, string>, Expected>(true); });\n\
             test('second', () => { assertEqual<Record<string, string>, Expected>(true); });",
        );
        assert_eq!(prints.len(), 2, "expected exactly two test bodies");
        assert_eq!(
            prints[0].alpha, prints[1].alpha,
            "identical type-level assertions must still be recognized as duplicates"
        );
        assert_eq!(prints[0].behavior, prints[1].behavior);
    }

    /// Type arguments are compared as source text, so formatting must be normalized away first:
    /// reporting two tests as different because a formatter inserted a space would be a defect of
    /// its own. Line breaks, indentation, and spacing around `<`, `,` and `>` all normalize out,
    /// while a space that separates two words (`keyof T`) is preserved.
    #[test]
    fn whitespace_only_differences_in_type_arguments_still_compare_equal() {
        let prints = fingerprint_all(
            "import { assertEqual } from './helpers';\n\
             type Expected = { a: 1 };\n\
             test('tight', () => { assertEqual<Record<string,keyof Expected>>(true); });\n\
             test('loose', () => { assertEqual< Record< string , keyof Expected >\n>(true); });",
        );
        assert_eq!(prints.len(), 2, "expected exactly two test bodies");
        assert_eq!(
            prints[0].alpha, prints[1].alpha,
            "type arguments differing only in whitespace name the same type and must fingerprint \
             identically"
        );
    }

    /// The normalization must not weld two words together: dropping *every* space would make
    /// `keyof T` collide with an identifier spelled `keyofT`, trading a false positive for a
    /// quieter one.
    #[test]
    fn type_argument_normalization_keeps_word_separators() {
        assert_eq!(
            normalize_type_argument_text("< Record< string , number > >"),
            "<Record<string,number>>"
        );
        assert_eq!(normalize_type_argument_text("<keyof  T>"), "<keyof T>");
        assert_ne!(
            normalize_type_argument_text("<keyof T>"),
            normalize_type_argument_text("<keyofT>")
        );
        assert_eq!(
            normalize_type_argument_text("<'a  b'>"),
            "<'a  b'>",
            "whitespace inside a string literal type is content, not formatting"
        );
        assert_ne!(
            normalize_type_argument_text("<'a b'>"),
            normalize_type_argument_text("<'ab'>")
        );
    }

    /// `CallExpression` is not the only construct that carries type arguments, and missing a
    /// sibling is this repository's most repeated defect. `new Foo<T>()`, a tagged template's
    /// `` tag<T>`...` ``, and a bare `TSInstantiationExpression` (`foo<T>` with no call) all get
    /// the same treatment.
    #[test]
    fn type_arguments_are_modeled_on_every_construct_that_carries_them() {
        let new_expression = fingerprint_all(
            "import { Box } from './box';\n\
             test('string box', () => { const box = new Box<string>(); return box; });\n\
             test('number box', () => { const box = new Box<number>(); return box; });",
        );
        assert_eq!(new_expression.len(), 2);
        assert_ne!(
            new_expression[0].alpha, new_expression[1].alpha,
            "`new Box<string>()` and `new Box<number>()` must not share an `alpha`"
        );

        let tagged = fingerprint_all(
            "import { sql } from './sql';\n\
             test('string rows', () => { const rows = sql<string>`select 1`; return rows; });\n\
             test('number rows', () => { const rows = sql<number>`select 1`; return rows; });",
        );
        assert_eq!(tagged.len(), 2);
        assert_ne!(
            tagged[0].alpha, tagged[1].alpha,
            "a tagged template's type arguments must reach the fingerprint"
        );

        let instantiation = fingerprint_all(
            "import { identity } from './identity';\n\
             test('string identity', () => { const pick = identity<string>; return pick; });\n\
             test('number identity', () => { const pick = identity<number>; return pick; });",
        );
        assert_eq!(instantiation.len(), 2);
        assert!(
            instantiation[0].alpha.contains("(instantiate "),
            "expected a TSInstantiationExpression, not some other parse: {}",
            instantiation[0].alpha
        );
        assert_ne!(
            instantiation[0].alpha, instantiation[1].alpha,
            "`identity<string>` and `identity<number>` are different expressions"
        );
    }

    /// A fluent type-level chain (`expectTypeOf<A>().toEqualTypeOf<B>()`) puts type arguments on
    /// both halves. Behavior events, which `near-duplicate-test` and `subsumed-test` key on, must
    /// distinguish a difference in either one.
    #[test]
    fn type_arguments_reach_behavior_events_on_both_halves_of_a_fluent_chain() {
        let inner = fingerprint_all(
            "import { expectTypeOf } from 'vitest';\n\
             type Expected = string;\n\
             test('a', () => { expectTypeOf<string>().toEqualTypeOf<Expected>(); });\n\
             test('b', () => { expectTypeOf<number>().toEqualTypeOf<Expected>(); });",
        );
        assert_ne!(
            inner[0].behavior, inner[1].behavior,
            "a difference in the subject's type argument must show up in the events: {:?} vs {:?}",
            inner[0].behavior, inner[1].behavior
        );

        let outer = fingerprint_all(
            "import { expectTypeOf } from 'vitest';\n\
             type Subject = string;\n\
             test('a', () => { expectTypeOf<Subject>().toEqualTypeOf<string>(); });\n\
             test('b', () => { expectTypeOf<Subject>().toEqualTypeOf<number>(); });",
        );
        assert_ne!(
            outer[0].behavior, outer[1].behavior,
            "a difference in the matcher's type argument must show up in the events: {:?} vs {:?}",
            outer[0].behavior, outer[1].behavior
        );
    }

    /// The `expect(...)` assertion path builds its own event string instead of going through
    /// `walk_call`, so it needs the same treatment or type-level matchers hung off `expect` stay
    /// indistinguishable.
    #[test]
    fn type_arguments_reach_the_assertion_event() {
        let prints = fingerprint_all(
            "import { expect } from 'vitest';\n\
             import { value } from './value';\n\
             test('a', () => { expect(value).toEqual<string>('x'); });\n\
             test('b', () => { expect(value).toEqual<number>('x'); });",
        );
        assert_eq!(prints.len(), 2);
        assert!(
            prints[0]
                .behavior
                .iter()
                .any(|event| event.starts_with("assert:")),
            "expected the assertion path, not an ordinary call event: {:?}",
            prints[0].behavior
        );
        assert_ne!(
            prints[0].behavior, prints[1].behavior,
            "a matcher's own type arguments must reach the assertion event: {:?} vs {:?}",
            prints[0].behavior, prints[1].behavior
        );
    }

    // -----------------------------------------------------------------------------------------
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

    // -----------------------------------------------------------------------------------------
    // Node-kind matrices: what each modeled `match` arm is *for*
    // -----------------------------------------------------------------------------------------
    //
    // 135 of this module's surviving mutants are `match`-arm deletions in `ser_expr`/`ser_stmt`/
    // `walk_expr`/`walk_stmt` and their helpers. A deleted arm does not crash: it routes the node
    // kind through the `_ =>` fallback, which is `opaque`/`degraded` raw source text for the
    // serializers and silence for the behavior walk. That is precisely how two tests differing
    // *only* in that node kind stop being distinguishable -- or, in the other direction, how two
    // alpha-equivalent bodies stop matching because their raw text differs.
    //
    // The tests below pin, per node kind, the exact fragment the arm emits. An assertion that
    // some fingerprint merely *differs* from another would not do it: `opaque` embeds verbatim
    // source, so two different nodes routed through the fallback still differ from each other.
    // Only the emitted content says the arm ran.

    /// A TypeScript wrapper around an ASSIGNMENT TARGET is transparent, the same as one around a
    /// value.
    ///
    /// `ser_simple_assignment_target`'s four TS arms unwrap to the inner expression, and none had
    /// coverage: every assignment in these tests writes to a bare identifier. They are the
    /// assignment-side twin of a fix review already made on the value side -- the arms exist
    /// precisely so `count! = 1` and `count = 1` are not two different behaviours -- and a wrapper
    /// that stopped unwrapping would make type annotations look like behavioural differences, which
    /// is a false negative on every duplicate pair where one half is annotated.
    #[test]
    fn a_typescript_wrapper_on_an_assignment_target_is_transparent() {
        let bare = alpha_of("let count = 0; count = 5; expect(count).toBe(5);");
        for wrapped in [
            "let count = 0; count! = 5; expect(count).toBe(5);",
            "let count = 0; (count as number) = 5; expect(count).toBe(5);",
            "let count = 0; (count satisfies number) = 5; expect(count).toBe(5);",
            "let count = 0; (<number>count) = 5; expect(count).toBe(5);",
        ] {
            assert_eq!(
                alpha_of(wrapped),
                bare,
                "a type wrapper is not a behavioural difference: {wrapped}"
            );
        }
    }

    /// `alpha` for one test callback body.
    fn alpha_of(body: &str) -> String {
        alpha_in(Path::new("case.test.ts"), body)
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

    /// `behavior` for one `async` test callback body.
    fn async_behavior_of(body: &str) -> Vec<String> {
        let prints = fingerprint_all(&format!("test('t', async () => {{ {body} }});"));
        assert_eq!(prints.len(), 1, "expected one test body for {body:?}");
        prints.into_iter().next().expect("one body").behavior
    }

    /// Every modeled node kind, with the exact fragment its `Serializer` arm emits.
    ///
    /// Each row fails if its arm is deleted (the node falls through to `opaque`, whose text is
    /// the raw source and never this fragment), and most fail on any edit to the fragment itself.
    #[rustfmt::skip]
    const SERIALIZED_FRAGMENTS: &[(&str, &str, &str)] = &[
        // (case name, test-callback body, fragment that must appear in `alpha`)
        // --- literals: the kind tag is what keeps `true` apart from `'true'` ---
        ("boolean", "const v = true;", "<boolean:true>"),
        ("null", "const v = null;", "<null:null>"),
        ("number", "const v = 1;", "<number:1>"),
        ("bigint", "const v = 1n;", "<bigint:1>"),
        ("regex", "const v = /ab/g;", "<regex:/ab/g>"),
        ("string", "const v = 's';", "<string:s>"),
        ("template", "const v = `q${1}`;", "(template `q` <number:1> ``)"),
        // --- `this` is both serialized and recorded as a form-changing observation ---
        ("this-value", "f(this);", " this)"),
        ("this-changes-the-callable-form", "f(this);", "(arrow-fn () "),
        ("arguments-changes-the-callable-form", "g(arguments);", "(arrow-fn () "),
        // --- composites ---
        ("array", "const v = [1];", "(array <number:1>)"),
        ("array-hole", "const v = [1,,2];", "<hole>"),
        ("array-spread", "const v = [...a];", "(array (spread "),
        ("argument-spread", "f(...a);", "(call id:global:f (spread "),
        ("object", "const v = {a:1};", "(object (prop prop:a <number:1>))"),
        ("object-computed-key", "const v = {[k]:1};", "(prop [id:global:k] <number:1>)"),
        ("object-spread", "const v = {...a};", "(object (spread "),
        ("arrow", "const v = () => 1;", "(fn () <number:1>)"),
        ("function-expression", "const v = function(){};", "(fn () (block ))"),
        ("generator-expression", "const v = async function*(){};", "(async *fn () (block ))"),
        ("call", "const v = f(1);", "(call id:global:f <number:1>)"),
        ("optional-call", "const v = f?.(1);", "(call?. id:global:f <number:1>)"),
        ("new", "const v = new F(1);", "(new id:global:F <number:1>)"),
        ("assign", "let x; x = 1;", "(assign = id:"),
        ("assign-to-member", "o.b = 1;", "(assign = (member id:global:o .b) <number:1>)"),
        ("assign-to-index", "o[0] = 1;", "(assign = (index id:global:o <number:0>) <number:1>)"),
        ("assign-destructuring-falls-back", "[a] = o;", "<opaque:[a]>"),
        ("binary", "const v = 1+2;", "(bin + <number:1> <number:2>)"),
        ("logical", "const v = 1||2;", "(logic || <number:1> <number:2>)"),
        ("unary", "const v = !1;", "(unary ! <number:1>)"),
        ("update", "let x; x++;", "(update ++ false id:"),
        ("update-member", "o.b++;", "(update ++ false (member id:global:o .b))"),
        ("update-index", "o[0]++;", "(update ++ false (index id:global:o <number:0>))"),
        ("conditional", "const v = 1?2:3;", "(cond <number:1> <number:2> <number:3>)"),
        ("sequence", "const v = (1,2);", "(seq <number:1> <number:2>)"),
        ("yield", "const v = function*(){ yield 1; };", "(yield <number:1>)"),
        ("yield-delegate", "const v = function*(){ yield* g(); };", "(yield* "),
        ("yield-bare", "const v = function*(){ yield; };", "(yield)"),
        ("static-member", "const v = o.b;", "(member id:global:o .b)"),
        ("optional-static-member", "const v = o?.b;", "(member id:global:o ?.b)"),
        ("computed-member", "const v = o[0];", "(index id:global:o <number:0>)"),
        ("optional-computed-member", "const v = o?.[0];", "(index? id:global:o <number:0>)"),
        ("tagged-template", "const v = t`q`;", "(tagged id:global:t (template `q`))"),
        ("instantiation", "const v = f<number>;", "(instantiate id:global:f<targs:"),
        // --- statements ---
        ("block", "{ 1; }", "(block (stmt <number:1>))"),
        ("empty", ";", "(empty)"),
        ("debugger", "debugger;", "(debugger)"),
        ("var", "var a = 1;", "(var (decl "),
        ("let", "let a = 1;", "(let (decl "),
        ("const", "const a = 1;", "(const (decl "),
        ("declarator-without-init", "let a;", "(let (decl L0))"),
        ("function-declaration", "function g(){}", "(block (function-decl L0 (fn () (block ))))"),
        ("if", "if(1)2;", "(if <number:1> (stmt <number:2>))"),
        ("if-else", "if(1)2;else 3;", "(if <number:1> (stmt <number:2>) (stmt <number:3>))"),
        ("for", "for(;;)1;", "(for () () () (stmt <number:1>))"),
        ("for-with-parts", "for(let i=0;i<1;i++)2;", "(for (let (decl "),
        ("for-with-expression-init", "for(f();;)1;", "(for (call id:global:f ) () () "),
        ("for-in-declaration", "for(const k in o)1;", "(for-in (decl "),
        ("for-in-identifier", "let k; for(k in o)1;", "(for-in id:"),
        ("for-in-member", "for(o.b in o)1;", "(for-in (member id:global:o .b) "),
        ("for-in-index", "for(o[0] in o)1;", "(for-in (index id:global:o <number:0>) "),
        ("for-of", "for(const k of o)1;", "(for-of (decl "),
        ("while", "while(1)2;", "(while <number:1> (stmt <number:2>))"),
        ("do-while", "do 1; while(2);", "(do-while (stmt <number:1>) <number:2>)"),
        ("return-bare", "return;", "(return)"),
        ("return-value", "return 1;", "(return <number:1>)"),
        ("throw", "throw 1;", "(throw <number:1>)"),
        ("try-catch-finally", "try{1;}catch(e){2;}finally{3;}", "(try (block (stmt <number:1>)) (catch "),
        ("try-without-handler", "try{1;}finally{2;}", "(try (block (stmt <number:1>)) () (block"),
        ("catch-without-param", "try{1;}catch{2;}", "(catch () (block (stmt <number:2>)))"),
        ("switch", "switch(1){case 2: 3;}", "(switch <number:1> (case <number:2> (stmt <number:3>)))"),
        ("switch-default", "switch(1){default: 3;}", "(case default (stmt <number:3>))"),
        ("break", "while(1){break;}", "(break)"),
        ("break-label", "l: while(1){break l;}", "(break l)"),
        ("continue", "while(1){continue;}", "(continue)"),
        ("continue-label", "l: while(1){continue l;}", "(continue l)"),
        ("label", "l: 1;", "(label l (stmt <number:1>))"),
        ("class-falls-back-to-raw-text", "class C {}", "<opaque:class C {}>"),
        // --- parameters and binding patterns ---
        ("param-identifier", "const v = (a) => a;", "(fn (L1) id:L1)"),
        ("param-object-pattern", "const v = ({a}) => a;", "(object-pattern (prop prop:a "),
        ("param-nested-object-pattern", "const v = ({a:{b}}) => b;", "(prop prop:a (object-pattern "),
        ("param-array-pattern", "const v = ([a]) => a;", "(array-pattern L1)"),
        ("param-array-pattern-hole", "const v = ([a,,b]) => a;", "(array-pattern L1 <hole> L2)"),
        ("param-default", "const v = (a=1) => a;", "(default L1 <number:1>)"),
        ("param-rest", "const v = (...a) => a;", "(rest L1)"),
        ("param-object-rest", "const v = ({...r}) => r;", "(object-pattern (rest "),
        ("param-array-rest", "const v = ([...r]) => r;", "(array-pattern (rest "),
    ];

    /// Rows whose whole point is that the wrapper *is* unwrapped: a deleted arm replaces the
    /// inner expression with the wrapper's verbatim source, which shows up as `<opaque:`.
    #[rustfmt::skip]
    const UNWRAPPED_EXPRESSIONS: &[(&str, &str, &str)] = &[
        ("parenthesized", "const v = (1);", "<number:1>"),
        ("ts-as", "const v = 1 as any;", "<number:1>"),
        ("ts-satisfies", "const v = 1 satisfies number;", "<number:1>"),
        ("ts-non-null", "const v = o!;", "global:o"),
        ("ts-type-assertion", "const v = <number>o;", "global:o"),
    ];

    /// A nullish guard on a computed member is part of the body's identity.
    ///
    /// `o[0]` throws when `o` is nullish; `o?.[0]` yields `undefined`. Two tests differing only in
    /// that guard are different tests -- and they serialized identically until 2026-09-25, so
    /// `duplicate-test` reported them at ERROR severity on other people's CI.
    ///
    /// Both siblings already recorded optionality (`.b` against `?.b`, `(call` against
    /// `(call?.`), so the gap was the one member form that did not: the "fixed at one site, not
    /// its siblings" shape rather than a decision. The whole family is asserted here together so
    /// the next form added is compared against all of them.
    #[test]
    fn optionality_is_part_of_every_member_and_call_form() {
        let pairs = [
            ("computed member", "const v = o[0];", "const v = o?.[0];"),
            ("static member", "const v = o.b;", "const v = o?.b;"),
            ("call", "f(1);", "f?.(1);"),
            // It propagates: a collapsed link contaminates everything built on top of it.
            (
                "computed member in a chain",
                "const v = o[0].k;",
                "const v = o?.[0].k;",
            ),
            ("computed member as an argument", "f(o[0]);", "f(o?.[0]);"),
        ];
        for (name, plain, optional) in pairs {
            assert_ne!(
                alpha_of(plain),
                alpha_of(optional),
                "{name}: a nullish guard must change `alpha`, or two different tests collapse \
                 into one `duplicate-test` finding at error severity"
            );
        }

        // The control. Optionality is the ONLY difference being claimed -- without this, the
        // assertions above would hold just as well if every body serialized uniquely.
        assert_eq!(
            alpha_of("const v = o[0];"),
            alpha_of("const v = o[0];"),
            "the same body must still serialize identically"
        );
    }

    #[test]
    fn every_modeled_node_kind_emits_its_own_serialized_fragment() {
        let mut problems: Vec<String> = Vec::new();
        for (name, body, fragment) in SERIALIZED_FRAGMENTS {
            let alpha = alpha_of(body);
            if !alpha.contains(fragment) {
                problems.push(format!(
                    "{name}: expected {fragment:?} in the `alpha` of `{body}`, got {alpha}"
                ));
            }
        }
        assert!(
            problems.is_empty(),
            "{} of {} rows did not emit their fragment:\n{}",
            problems.len(),
            SERIALIZED_FRAGMENTS.len(),
            problems.join("\n")
        );
        for (name, body, fragment) in UNWRAPPED_EXPRESSIONS {
            let alpha = alpha_of(body);
            assert!(
                alpha.contains(fragment),
                "{name}: expected {fragment:?} in the `alpha` of `{body}`, got {alpha}"
            );
            assert!(
                !alpha.contains("<opaque:"),
                "{name}: `{body}` must be unwrapped to its inner expression, not rendered as \
                 raw text: {alpha}"
            );
        }
        // `await` needs its own async wrapper.
        let alpha = async_alpha_of("const v = await 1;");
        assert!(
            alpha.contains("(await <number:1>)"),
            "await must be modeled, got {alpha}"
        );
    }

    /// Pairs of bodies that differ in nothing but one operator (or one operator-shaped choice:
    /// a position, an optional link, a declaration keyword) and must never share a fingerprint.
    ///
    /// [`SERIALIZED_FRAGMENTS`] proves each arm exists and spells its own shape, but every row
    /// there carries exactly *one* operator per kind. An arm that hard-coded its operator, or
    /// dropped `operator.as_str()` from its format string altogether, satisfies every one of
    /// those rows while collapsing `a + b` into `a - b`. This is the other half of that guard,
    /// and each row here is a `duplicate-test` at error severity on correct code if it stops
    /// holding -- the two bodies are different programs.
    ///
    /// `o[0]` against `o?.[0]` was once absent from this table on purpose: it was the one member
    /// form whose optional link did not reach `alpha`, so the row would have failed and pinning
    /// the collision would have cemented it. [`Serializer::ser_computed_member`] now reads
    /// `member.optional`, matching [`Serializer::ser_static_member`] (`.b` against `?.b`) and
    /// [`Serializer::ser_call`] (`(call` against `(call?.`), so the pair is asserted below with
    /// the rest.
    #[rustfmt::skip]
    const OPERATOR_PAIRS: &[(&str, &str, &str)] = &[
        // (case name, one body, the body it must not fingerprint as)
        ("arithmetic",            "const v = a + b;",  "const v = a - b;"),
        ("arithmetic non-additive", "const v = a * b;", "const v = a / b;"),
        ("remainder vs division", "const v = a % b;",  "const v = a / b;"),
        ("exponent vs product",   "const v = a ** b;", "const v = a * b;"),
        ("comparison direction",  "const v = a < b;",  "const v = a > b;"),
        ("comparison strictness", "const v = a < b;",  "const v = a <= b;"),
        ("loose vs strict equality",   "const v = a == b;", "const v = a === b;"),
        ("loose vs strict inequality", "const v = a != b;", "const v = a !== b;"),
        ("shift direction",       "const v = a << b;", "const v = a >> b;"),
        ("shift fill",            "const v = a >> b;", "const v = a >>> b;"),
        ("bitwise",               "const v = a & b;",  "const v = a | b;"),
        ("membership",            "const v = a in b;", "const v = a instanceof b;"),
        ("logical or vs and",     "const v = a || b;", "const v = a && b;"),
        ("nullish vs or",         "const v = a ?? b;", "const v = a || b;"),
        ("unary not vs negate",   "const v = !a;",     "const v = -a;"),
        ("unary sign",            "const v = +a;",     "const v = -a;"),
        ("unary typeof vs void",  "const v = typeof a;", "const v = void a;"),
        ("unary complement",      "const v = ~a;",     "const v = !a;"),
        // Order matters as much as the operator: `++x` and `x++` evaluate to different values.
        ("update direction",      "let x; x++;",       "let x; x--;"),
        ("update position",       "let x; x++;",       "let x; ++x;"),
        ("assignment compound",   "let x; x += 1;",    "let x; x -= 1;"),
        ("assignment plain vs compound", "let x; x = 1;", "let x; x += 1;"),
        ("assignment logical",    "let x; x ||= 1;",   "let x; x &&= 1;"),
        // An optional link is a guard against a nullish receiver, so it is part of what the
        // body does -- exactly as the `.b`/`?.b` and `f(...)`/`f?.(...)` spellings record.
        ("optional member",       "const v = o.b;",    "const v = o?.b;"),
        ("optional call",         "f(1);",             "f?.(1);"),
        ("optional computed member", "const v = o[0];", "const v = o?.[0];"),
        ("static vs computed member", "const v = o.b;", "const v = o[b];"),
        ("call vs construction",  "const v = f(1);",   "const v = new f(1);"),
        // Statement-level choices, on the same terms.
        ("for-in vs for-of",      "for(const k in o)f();", "for(const k of o)f();"),
        ("while vs do-while",     "while(a)f();",      "do f(); while(a);"),
        ("else branch present",   "if(a)f();",         "if(a)f();else f();"),
        ("break vs continue",     "while(a){break;}",  "while(a){continue;}"),
        ("labelled break",        "l: while(a){break;}", "l: while(a){break l;}"),
        ("throw vs return",       "throw a;",          "return a;"),
        ("declaration kind var/let",   "var a = 1;",   "let a = 1;"),
        ("declaration kind let/const", "let a = 1;",   "const a = 1;"),
        ("yield delegation", "const v = function*(){ yield g(); };",
                             "const v = function*(){ yield* g(); };"),
        ("switch case vs default", "switch(a){case 1: f();}", "switch(a){default: f();}"),
        ("array hole",            "const v = [1,,2];", "const v = [1,2];"),
        ("argument spread",       "f(a);",             "f(...a);"),
    ];

    /// An operator is part of a body's identity wherever one appears.
    ///
    /// If a row here fails, two tests that compute genuinely different things have become
    /// `alpha`-equal, which is the input `duplicate-test` reports at error severity.
    #[test]
    fn an_operator_is_part_of_the_identity_of_every_construct_that_carries_one() {
        for (name, left, right) in OPERATOR_PAIRS {
            assert_ne!(
                alpha_of(left),
                alpha_of(right),
                "{name}: `{left}` / `{right}`"
            );
        }
    }

    /// `structural` erases a literal's *value*, and nothing else. An operator survives it, so
    /// the same pairs stay distinct there too -- otherwise `parameterization-candidate` would
    /// propose a `test.each` table over two bodies that compute different things, which is the
    /// same defect one severity down.
    #[test]
    fn an_operator_survives_the_structural_collapse_of_literal_values() {
        for (name, left, right) in OPERATOR_PAIRS {
            assert_ne!(
                structural_of(left),
                structural_of(right),
                "{name}: `{left}` / `{right}`"
            );
        }
    }

    /// `structural` for one test callback body.
    fn structural_of(body: &str) -> String {
        structural_in(Path::new("case.test.ts"), body)
    }

    /// The update *position* reaches the behavior event, not only `alpha`.
    ///
    /// `updates_are_part_of_behavior_identity` pins the postfix spelling; a renderer that
    /// printed one word for both positions would still satisfy it. `++i` and `i++` differ in
    /// the value the surrounding expression observes, so the two must never share an event.
    #[test]
    fn an_update_records_which_side_of_its_target_it_sits_on() {
        let prefix = behavior_of("let x; ++x; expect(x).toBe(1);");
        let postfix = behavior_of("let x; x++; expect(x).toBe(1);");
        assert!(
            prefix.iter().any(|event| event.contains("update:prefix++")),
            "a prefix update must name its position: {prefix:?}"
        );
        assert!(
            postfix
                .iter()
                .any(|event| event.contains("update:postfix++")),
            "...and so must a postfix one: {postfix:?}"
        );
        assert_ne!(prefix, postfix, "the two positions are different behavior");
    }

    /// `structural` collapses a literal's *value* to its kind, which is what lets
    /// `parameterization-candidate` propose a `test.each` table. That collapse lives in the same
    /// `ser_expr` arms as the `alpha` rendering, so it needs its own assertion: two bodies
    /// differing only in a literal's value must be structurally equal, and two bodies differing
    /// in the literal's *kind* must not.
    #[test]
    fn structural_collapses_literal_values_but_not_literal_kinds() {
        #[rustfmt::skip]
        let same_kind: &[(&str, &str, &str)] = &[
            ("boolean", "const v = true;", "const v = false;"),
            ("number", "const v = 1;", "const v = 22;"),
            ("bigint", "const v = 1n;", "const v = 22n;"),
            ("regex", "const v = /a/;", "const v = /bb/;"),
            ("string", "const v = 'a';", "const v = 'bb';"),
        ];
        for (name, left, right) in same_kind {
            let source = format!("test('a', () => {{ {left} }});\ntest('b', () => {{ {right} }});");
            let prints = fingerprint_all(&source);
            assert_eq!(prints.len(), 2);
            assert_eq!(
                prints[0].structural, prints[1].structural,
                "{name}: two values of one literal kind must share a `structural` shape"
            );
            assert_ne!(
                prints[0].alpha, prints[1].alpha,
                "{name}: ...while `alpha` keeps the values apart"
            );
        }

        // Cross-kind: the kind tag survives the value collapse. `1000` the number and `'1000'`
        // the string were a real adjudicated false duplicate before the tag existed.
        let prints = fingerprint_all(
            "test('a', () => { const v = 1000; });\ntest('b', () => { const v = '1000'; });",
        );
        assert_ne!(
            prints[0].structural, prints[1].structural,
            "a number and a string must never collapse to the same structural shape"
        );
    }

    /// The `walk_stmt`/`walk_expr` half of the same problem: an arm's job there is to *descend*,
    /// so a deleted arm silently drops every event underneath it. Each row states the exact
    /// event list, because a subset assertion would still pass with a missing event.
    #[rustfmt::skip]
    const BEHAVIOR_EVENTS: &[(&str, &str, &[&str])] = &[
        // (case name, body, the exact `behavior` event list)
        ("expression-statement", "f();", &["call:global:f"]),
        ("block", "{ f(); }", &["call:global:f"]),
        ("variable-declaration", "const v = f();", &["call:global:f"]),
        ("if", "if(f())g();", &["if", "call:global:f", "call:global:g"]),
        ("if-else", "if(1)f();else g();", &["if", "call:global:f", "call:global:g"]),
        ("for", "for(;f();g())h();", &["loop", "call:global:f", "call:global:g", "call:global:h"]),
        ("for-in", "for(const k in f())g();", &["loop", "call:global:f", "call:global:g"]),
        ("for-of", "for(const k of f())g();", &["loop", "call:global:f", "call:global:g"]),
        ("while", "while(f())g();", &["loop", "call:global:f", "call:global:g"]),
        ("do-while", "do g(); while(f());", &["loop", "call:global:g", "call:global:f"]),
        ("try", "try{f();}catch(e){g();}finally{h();}", &["try", "call:global:f", "call:global:g", "call:global:h"]),
        ("switch", "switch(f()){case g(): h();}", &["call:global:f", "call:global:g", "call:global:h"]),
        ("return", "return f();", &["return", "call:global:f"]),
        ("throw", "throw f();", &["throw", "call:global:f"]),
        ("labeled", "l: f();", &["call:global:f"]),
        ("function-declaration-is-deferred", "function g(){ f(); }", &["deferred-call:global:f"]),
        ("arrow-value-is-deferred", "const v = () => { f(); };", &["deferred-call:global:f"]),
        ("arrow-expression-value-is-deferred", "const v = () => f();", &["deferred-call:global:f"]),
        ("function-value-is-deferred", "const v = function(){ f(); };", &["deferred-call:global:f"]),
        ("static-member", "f().b;", &["call:global:f"]),
        ("computed-member", "f()[g()];", &["call:global:f", "call:global:g"]),
        ("binary", "f()+g();", &["call:global:f", "call:global:g"]),
        ("logical", "f()||g();", &["call:global:f", "call:global:g"]),
        ("unary", "!f();", &["call:global:f"]),
        ("conditional", "f()?g():h();", &["call:global:f", "call:global:g", "call:global:h"]),
        ("assignment", "o.b = f();", &["assign:= target=(member id:global:o .b) value=(call id:global:f )"]),
        ("sequence", "(f(),g());", &["call:global:f", "call:global:g"]),
        ("array", "[f()];", &["call:global:f"]),
        ("object", "const v = {a:f()};", &["call:global:f"]),
        ("parenthesized", "(f());", &["call:global:f"]),
        ("ts-as", "f() as unknown;", &["call:global:f"]),
        ("ts-satisfies", "f() satisfies unknown;", &["call:global:f"]),
        ("ts-non-null", "f()!;", &["call:global:f"]),
        ("chain-call", "o?.b(f());", &[r#"call:global:o.b args=["(call id:global:f )"]"#, "call:global:f"]),
        ("chain-static-member", "o?.b.c;", &[]),
        ("chain-computed-member", "o?.[f()];", &["call:global:f"]),
        ("call-arguments-and-callee", "f(g())(h());", &[r#"call:global:f args=["(call id:global:h )"]"#, r#"call:global:f args=["(call id:global:g )"]"#, "call:global:g", "call:global:h"]),
        ("spread-argument", "f(...g());", &[r#"call:global:f args=["...(call id:global:g )"]"#, "call:global:g"]),
    ];

    #[test]
    fn every_modeled_node_kind_is_descended_into_by_the_behavior_walk() {
        for (name, body, expected) in BEHAVIOR_EVENTS {
            let events = behavior_of(body);
            let expected: Vec<String> = expected.iter().map(|e| (*e).to_owned()).collect();
            assert_eq!(
                events, expected,
                "{name}: unexpected `behavior` for `{body}`"
            );
        }
        assert_eq!(
            async_behavior_of("await f();"),
            vec!["await".to_owned(), "call:global:f".to_owned()],
            "an `await` must be recorded and then descended into"
        );
    }

    /// `call_path` flattens a callee into a dotted path. Every arm it models exists so that one
    /// spelling of a callee produces the *same* event as another; a deleted arm falls through to
    /// `<raw source>`, which breaks exactly that equivalence.
    #[test]
    fn call_path_flattens_every_modeled_callee_shape() {
        #[rustfmt::skip]
        let exact: &[(&str, &str, &str)] = &[
            ("identifier", "f();", "call:global:f"),
            ("this", "this.m();", "call:this.m"),
            ("static-member", "o.b.c();", "call:global:o.b.c"),
            ("computed-member", "o[0]();", "call:global:o[]"),
            ("inner-call", "o.b().c();", "call:global:o.b.c"),
            ("chain-static-member", "o?.b();", "call:global:o.b"),
            ("chain-computed-member", "o?.[0]();", "call:global:o[]"),
        ];
        for (name, body, expected) in exact {
            let events = behavior_of(body);
            assert!(
                events.iter().any(|event| event == expected),
                "{name}: expected {expected:?} among {events:?} for `{body}`"
            );
        }

        // The unwrapping arms (`(f)()`, `(f as any)()`, `f!()`) exist so a decorated callee
        // produces the identical event to the bare one. Comparing against the bare spelling is
        // what makes a deleted arm fail: `<(f)>` is still a perfectly distinct path.
        let bare = behavior_of("f();");
        for (name, body) in [
            ("parenthesized", "(f)();"),
            ("ts-as", "(f as any)();"),
            ("ts-non-null", "f!();"),
        ] {
            assert_eq!(
                behavior_of(body),
                bare,
                "{name}: `{body}` must flatten to the same call path as `f()`"
            );
        }

        // And a callee shape with no arm at all is deliberately kept as raw text rather than
        // guessed at.
        assert!(
            behavior_of("(o ? f : g)();")
                .iter()
                .any(|event| event.starts_with("call:<")),
            "an unmodeled callee must be recorded as verbatim source"
        );
    }

    /// `assertion_event` recognizes `expect(...)[.modifier].matcher(...)`. Each modifier arm
    /// deleted turns the whole thing back into an ordinary `call:` event, which is the difference
    /// between `near-duplicate-test` seeing an assertion and seeing a method call.
    #[test]
    fn assertion_event_recognizes_every_modifier_spelling() {
        #[rustfmt::skip]
        let cases: &[(&str, &str, &str)] = &[
            ("plain", "expect(1).toBe(2);", "assert:expect.toBe(subject=<number>; value=<number:2>)"),
            ("not", "expect(1).not.toBe(2);", "assert:expect.not.toBe("),
            ("resolves", "expect(p).resolves.toBe(2);", "assert:expect.resolves.toBe("),
            ("rejects", "expect(p).rejects.toBe(2);", "assert:expect.rejects.toBe("),
            ("chained-modifiers", "expect(p).resolves.not.toBe(2);", "assert:expect.resolves.not.toBe("),
            ("soft", "expect.soft(1).toBe(2);", "assert:expect.soft.toBe("),
            ("poll", "expect.poll(f).toBe(2);", "assert:expect.poll.toBe("),
            ("parenthesized-base", "(expect(1)).toBe(2);", "assert:expect.toBe("),
            ("no-subject", "expect().toBe(2);", "assert:expect.toBe(subject=(); "),
        ];
        for (name, body, fragment) in cases {
            let events = behavior_of(body);
            assert!(
                events.iter().any(|event| event.starts_with(fragment)),
                "{name}: expected an event starting {fragment:?} among {events:?}"
            );
        }

        // The negative side of the same match: an unrecognized modifier is NOT an assertion, so
        // it must stay an ordinary call event rather than being silently accepted.
        for (name, body) in [
            ("unknown-modifier", "expect(1).maybe.toBe(2);"),
            ("unknown-expect-namespace", "expect.eventually(1).toBe(2);"),
            ("not-expect-at-all", "assertThat(1).toBe(2);"),
        ] {
            let events = behavior_of(body);
            assert!(
                !events.iter().any(|event| event.starts_with("assert:")),
                "{name}: `{body}` is not an `expect` assertion: {events:?}"
            );
        }

        // A local named `expect` is somebody's own binding and carries none of the meaning.
        let events = behavior_of("const expect = mk(); expect(1).toBe(2);");
        assert!(
            !events.iter().any(|event| event.starts_with("assert:")),
            "a body-local `expect` must not be treated as Vitest's: {events:?}"
        );
    }

    // -----------------------------------------------------------------------------------------
    // The purity precondition behind dataflow canonicalization
    // -----------------------------------------------------------------------------------------
    //
    // `expression_has_effect` and `statement_has_effect_at` are the allowlist that decides
    // whether a single-use `const` may be inlined into its one read (and whether a destructured
    // property may be rewritten to a member access). Both functions, and 13 of
    // `expression_has_effect`'s arms, survived mutation -- including replacing the whole function
    // with `true`, which disables the canonicalization wholesale. Nothing failed, because no test
    // asserted what an inlined body's `behavior` actually looks like.
    //
    // These do, through the assertion subject: inlined, it is the initializer's own shape;
    // blocked, it is the bare local.

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

    /// [`assertion_subject`] for a whole source file rather than a bare body: the two
    /// destructuring-canonicalization tests need imports above the test, which `behavior_of`'s
    /// own wrapping cannot carry.
    fn assertion_subject_of_source(source: &str) -> String {
        let events = fingerprint_all(source).swap_remove(0).behavior;
        let event = events
            .iter()
            .find(|event| event.starts_with("assert:"))
            .unwrap_or_else(|| panic!("no assertion event: {events:?}"))
            .clone();
        let start = event.find("subject=").expect("subject") + "subject=".len();
        let end = event.find("; value=").expect("value");
        event[start..end].to_owned()
    }

    /// The statement whose purity is under test, wrapped between a single-use `const` and its
    /// one read. `@` is the intervening statement.
    fn inlining_probe(intervening: &str) -> String {
        format!("let y; const subject = build(1); {intervening} expect(subject).toBe(3);")
    }

    /// Every shape `expression_has_effect`/`statement_has_effect_at` clears as pure, all in one
    /// body, so that any one of them wrongly reporting an effect blocks the inlining this asserts.
    ///
    /// What makes this fail: replace either function with `true`; delete any of the arms below
    /// (the `_ =>` fallback is "has an effect"); or break the `Inline` substitution itself.
    #[test]
    fn a_pure_intervening_statement_does_not_block_single_use_local_inlining() {
        let pure = concat!(
            ";",                               // EmptyStatement
            " { 1; }",                         // BlockStatement, recursing
            " var w;",                         // a declarator with no initializer
            " 1;",                             // ExpressionStatement
            " const p1 = 1 + 2;",              // BinaryExpression, both operands
            " const p2 = 1 || 2;",             // LogicalExpression
            " const p3 = !1;",                 // UnaryExpression
            " const p4 = 1 ? 2 : 3;",          // ConditionalExpression
            " const p5 = (1);",                // ParenthesizedExpression
            " const p6 = 1 as any;",           // TSAsExpression
            " const p7 = 1 satisfies number;", // TSSatisfiesExpression
            " const p8 = o!;",                 // TSNonNullExpression
            " const p9 = `x${1}`;",            // TemplateLiteral
            " const pa = [1];",                // ArrayExpression
            " const pb = {a:1};",              // ObjectExpression
            " const pc = o.b;",                // StaticMemberExpression
            " const pd = o[0];",               // ComputedMemberExpression
            " const pe = null;",               // the literal/identifier/this/super arm
        );
        assert_eq!(
            assertion_subject(&inlining_probe(pure)),
            "(call id:global:build <number>)",
            "with only allowlisted-pure statements in between, the single-use local must inline \
             into its one read"
        );
    }

    /// The other direction, one statement shape per row. Each row's intervening statement is
    /// *identical* to the pure control above but for one sub-expression, and every row emits no
    /// behavior event of its own, so the only thing that moves is whether inlining was declined.
    ///
    /// An assignment is the vehicle throughout because it is effectful (`_ =>` in
    /// `expression_has_effect`) while contributing nothing to `behavior` -- so a row that wrongly
    /// inlines is visible in the subject and nowhere else.
    #[test]
    fn an_effectful_intervening_statement_blocks_single_use_local_inlining() {
        #[rustfmt::skip]
        let rows: &[(&str, &str)] = &[
            ("binary-right", "const b = 1 + (y = 1);"),
            ("binary-left", "const b = (y = 1) + 1;"),
            ("logical-right", "const b = 1 || (y = 1);"),
            ("logical-left", "const b = (y = 1) || 1;"),
            ("conditional-test", "const b = (y = 1) ? 1 : 2;"),
            ("conditional-consequent", "const b = 1 ? (y = 1) : 2;"),
            ("conditional-alternate", "const b = 1 ? 2 : (y = 1);"),
            ("computed-member-property", "const b = o[(y = 1)];"),
            ("computed-member-object", "const b = (y = 1)[0];"),
            ("static-member-object", "const b = (y = 1).c;"),
            ("unary", "const b = !(y = 1);"),
            ("array-element", "const b = [(y = 1)];"),
            ("object-property", "const b = {a:(y = 1)};"),
            ("object-spread", "const b = {...o};"),
            ("template-interpolation", "const b = `x${y = 1}`;"),
            ("parenthesized", "const b = ((y = 1));"),
            ("ts-as", "const b = (y = 1) as any;"),
            ("ts-satisfies", "const b = (y = 1) satisfies unknown;"),
            ("ts-non-null", "const b = (y = 1)!;"),
            ("inside-a-block", "{ (y = 1); }"),
            ("bare-expression-statement", "(y = 1);"),
            ("control-flow-is-never-cleared", "if (1) { }"),
        ];
        for (name, intervening) in rows {
            let subject = assertion_subject(&inlining_probe(intervening));
            assert!(
                subject.starts_with("id:L"),
                "{name}: `{intervening}` has an effect, so `subject` must stay the un-inlined \
                 local, got {subject:?}"
            );
        }
    }

    /// The same precondition guards the destructuring-to-member-access rewrite, which reads the
    /// widest declaration-to-last-read range rather than a single read.
    #[test]
    fn the_purity_precondition_also_guards_destructuring_to_member_access() {
        // The object has to be a *resolved* free symbol -- an unresolved true global carries no
        // identity safe to key the rewrite on -- so it is imported rather than assumed.
        let subject_for = |intervening: &str| {
            assertion_subject_of_source(&format!(
                "import {{ o }} from './o';\n\
                 test('t', () => {{ let y; const {{ a }} = o; {intervening} \
                 expect(a).toBe(3); }});"
            ))
        };
        let subject = subject_for("const p = 1 + 2;");
        assert!(
            subject.starts_with("(member ") && subject.ends_with(" .a)"),
            "a destructured property of a resolved free object must canonicalize to the member \
             access it stands for, got {subject:?}"
        );
        let subject = subject_for("const p = 1 + (y = 1);");
        assert!(
            subject.starts_with("id:L") && subject.ends_with(":prop:a"),
            "an intervening effect must block member-access equivalence while retaining the \
             property's identity, got {subject:?}"
        );
    }

    /// `SymbolReadScan` refuses to reason about a read that sits inside a branch, loop, switch,
    /// try, class body, or nested closure: execution past the declaration is then conditional,
    /// repeated, or deferred, so "declared once, read exactly there" is unsound.
    ///
    /// The observable is event *order*. With the read ruled unsafe, the declaration keeps its own
    /// `call:global:f` event where it stands; with the read wrongly accepted, `walk_stmt` skips
    /// the declaration and the event re-appears later (at the read) or, inside a class body,
    /// vanishes entirely. Every `unsafe_depth` bracket in that visitor is one row here, plus a
    /// positive control so the test cannot pass by declaring everything unsafe.
    #[test]
    fn a_read_inside_control_flow_or_a_closure_is_never_inlined() {
        #[rustfmt::skip]
        let unsafe_reads: &[(&str, &str)] = &[
            ("if", "if (1) { g(x); }"),
            ("for", "for (;;) { g(x); }"),
            ("for-in", "for (const k in o) { g(x); }"),
            ("for-of", "for (const k of o) { g(x); }"),
            ("while", "while (1) { g(x); }"),
            ("do-while", "do { g(x); } while (0);"),
            ("switch", "switch (1) { case 1: g(x); }"),
            ("try", "try { g(x); } catch (e) { }"),
            ("class-body", "class C { p = x; }"),
            ("function-declaration", "function h() { g(x); }"),
            ("arrow", "const h = () => { g(x); };"),
        ];
        for (name, reader) in unsafe_reads {
            let events = behavior_of(&format!("const x = f(); {reader}"));
            assert_eq!(
                events.first().map(String::as_str),
                Some("call:global:f"),
                "{name}: the read in `{reader}` is not safe to inline, so the declaration must \
                 keep its own event in place: {events:?}"
            );
        }

        // The control: a read in the same flat statement list IS inlined, so the declaration's
        // event moves to the read site. Without this the test above would pass just as well if
        // every read were ruled unsafe.
        let events = behavior_of("const x = f(); g(x);");
        assert!(events[0].starts_with("call:global:g args="), "{events:?}");
        assert_eq!(events[1], "call:global:f");
    }

    // -----------------------------------------------------------------------------------------
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
    // Type-argument text normalization
    // -----------------------------------------------------------------------------------------

    /// `normalize_type_argument_text` canonicalizes a type-argument list's raw source so that
    /// formatting alone never changes a fingerprint -- while keeping anything inside a string
    /// literal type byte-exact, and keeping the one space that separates two words.
    #[test]
    fn type_argument_text_normalizes_formatting_but_not_meaning() {
        let behavior_for =
            |type_arguments: &str| behavior_of(&format!("expect(v).toEqual<{type_arguments}>(1);"));

        // Formatting only: equal. The whitespace after a quoted string type is the part that
        // fails if the quote-tracking loop mishandles its escape or its closing delimiter.
        assert_eq!(
            behavior_for("{ a: 'p q', b: 1 }"),
            behavior_for("{a:'p q',b:1}"),
            "whitespace outside a string literal type must normalize away"
        );

        // Inside quotes: preserved, so two different literal types stay different.
        assert_ne!(
            behavior_for("'p q'"),
            behavior_for("'pq'"),
            "whitespace *inside* a string literal type is part of the type"
        );

        // A word boundary carries meaning and the single separating space survives.
        assert_ne!(
            behavior_for("keyof T"),
            behavior_for("keyofT"),
            "`keyof T` and `keyofT` are different types; the separating space must survive"
        );
        assert_eq!(
            behavior_for("keyof   T"),
            behavior_for("keyof T"),
            "...collapsed to exactly one space, however it was written"
        );

        // A non-word boundary needs no space at all.
        assert_eq!(
            behavior_for("A | B"),
            behavior_for("A|B"),
            "no space is needed either side of a punctuation boundary"
        );
    }

    /// `arguments` is the other construct an arrow does not bind, and only the *free* form -- the
    /// real `arguments` object -- carries that meaning: a resolved symbol by that name is
    /// somebody's own binding.
    #[test]
    fn only_a_free_arguments_reference_marks_a_body_as_this_dependent() {
        // A `function` body absorbs its own `arguments`, so the form it prints changes.
        let free = alpha_of("const h = function(){ g(arguments); };");
        assert!(
            free.contains("(function-fn () "),
            "a free `arguments` inside a `function` must print the specific form: {free}"
        );

        // A local of that name is not the `arguments` object, and an ordinary free identifier is
        // not either -- both must leave the interchangeable `fn` form in place.
        for (name, body) in [
            ("shadowed", "const h = function(args){ g(args); };"),
            (
                "other-free-identifier",
                "const h = function(){ g(other); };",
            ),
        ] {
            let alpha = alpha_of(body);
            assert!(
                !alpha.contains("function-fn"),
                "{name}: `{body}` observes no `this`/`arguments`, so the two callable forms stay \
                 interchangeable: {alpha}"
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
        let function =
            fingerprint_function_bodies("test('t', function () { const x = f(); g(x); });");
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
    // Constructs only reachable inside a class body
    // -----------------------------------------------------------------------------------------
    //
    // `ser_expr` renders a class as opaque raw text, so a private field or `super` can only be
    // reached by a test registered *inside* a class method -- lexically within the class body,
    // where those constructs are legal, but in a callback this module does fingerprint. Real
    // suites do this (a test helper class, a describe-per-class layout), and the whole
    // `PrivateFieldExpression`/`Super` family of arms is otherwise unreachable.

    #[test]
    fn a_private_field_reference_is_modeled_everywhere_it_can_appear() {
        let in_class = |body: &str| {
            let source = format!(
                "class C {{ #p = 1; #m() {{}} run() {{ test('t', () => {{ {body} }}); }} }}"
            );
            let prints = fingerprint_all(&source);
            assert_eq!(prints.len(), 1, "expected one test body for {body:?}");
            prints.into_iter().next().expect("one body")
        };

        // `ser_expr` -> `ser_private_field`
        assert!(
            in_class("const v = this.#p;")
                .alpha
                .contains("(member this .#p)"),
            "a private field read must be modeled, not rendered as raw text"
        );
        // `ser_assignment_target` and `ser_simple_assignment_target`
        assert!(
            in_class("this.#p = 1;")
                .alpha
                .contains("(assign = (member this .#p) <number:1>)"),
            "a private field write target must be modeled"
        );
        assert!(
            in_class("this.#p++;")
                .alpha
                .contains("(update ++ false (member this .#p))"),
            "a private field update target must be modeled"
        );
        // `ser_chain` -> `ser_private_field`
        assert!(
            in_class("const v = this?.#p;")
                .alpha
                .contains("(member this .#p)"),
            "an optional private field read must be modeled"
        );
        // `ser_for_left` -> `ser_private_field`
        assert!(
            in_class("for (this.#p in o) 1;")
                .alpha
                .contains("(for-in (member this .#p) "),
            "a private field as a for-in target must be modeled"
        );
        // `walk_expr` and `call_path`
        assert!(
            in_class("this.#m(f());")
                .behavior
                .iter()
                .any(|event| event.starts_with("call:this.#m args=")),
            "a private method call must flatten into the event's path"
        );
        assert!(
            in_class("g(this.#m(f()));")
                .behavior
                .contains(&"call:global:f".to_owned()),
            "the walk must descend through a private field's object"
        );
        assert!(
            in_class("this?.#m();")
                .behavior
                .contains(&"call:this.#m".to_owned()),
            "...including through an optional chain"
        );
        // `ser_private_field`'s own depth guard, landed on exactly: a private field cannot be
        // written outside a class body, so it has no row in `LANDED_GUARDS`. `ser_expr` reaches
        // the construct at depth `4 + padding` and hands it down one frame.
        // pinned from both sides, since a `>=` guard here would otherwise be invisible.
        // `n` unary links put the private field's own `ser_expr` at depth `4 + n`,
        // `ser_private_field` at `5 + n` and its `this` object at `6 + n`. At `n = MAX_DEPTH - 6`
        // the deepest of those three lands on `MAX_DEPTH` exactly, so nothing may degrade.
        let inside = (MAX_DEPTH - 6) as usize;
        let serialized = in_class(&format!("const v = {}this.#p;", "!".repeat(inside))).alpha;
        assert!(
            serialized.contains("(member this .#p)") && !serialized.contains("<max-depth:"),
            "at depth MAX_DEPTH exactly the private field must still be serialized whole. \
             Tail: {}",
            &serialized[serialized.len().saturating_sub(80)..]
        );
        // One deeper, `ser_private_field` itself sits on `MAX_DEPTH`: it must still run -- and
        // its `this` object, now one past the cap, is what degrades. This is the single point
        // where a `>=` guard here is observable, so it is asserted on content rather than on the
        // presence of a marker.
        let boundary = in_class(&format!("const v = {}this.#p;", "!".repeat(inside + 1))).alpha;
        assert!(
            boundary.contains("(member <max-depth:this> .#p)"),
            "on MAX_DEPTH exactly the private field is serialized and only its object degrades. \
             Tail: {}",
            &boundary[boundary.len().saturating_sub(80)..]
        );
        // One deeper again, the private field is itself past the cap.
        let degraded = in_class(&format!("const v = {}this.#p;", "!".repeat(inside + 2))).alpha;
        assert!(
            degraded.contains("<max-depth:this.#p>"),
            "past the cap it degrades to its own verbatim source. Tail: {}",
            &degraded[degraded.len().saturating_sub(80)..]
        );

        // `expression_has_effect`'s own private-field arm: a pure private read must not block
        // the inlining of a single-use local.
        let inlined = in_class("const s = build(1); const p = this.#p; expect(s).toBe(3);");
        let event = inlined
            .behavior
            .iter()
            .find(|event| event.starts_with("assert:"))
            .expect("assertion event");
        assert!(
            event.contains("subject=(call id:global:build <number>)"),
            "reading a private field has no effect, so it must not block inlining: {event}"
        );
    }

    #[test]
    fn super_is_modeled_in_both_serialization_and_call_paths() {
        let in_subclass = |body: &str| {
            let source = format!(
                "class B {{}} class C extends B {{ run() {{ test('t', () => {{ {body} }}); }} }}"
            );
            let prints = fingerprint_all(&source);
            assert_eq!(prints.len(), 1);
            prints.into_iter().next().expect("one body")
        };
        assert!(
            in_subclass("const v = super.x;")
                .alpha
                .contains("(member super .x)"),
            "`super` must serialize as itself, not as raw text"
        );
        assert!(
            in_subclass("super.x();")
                .behavior
                .contains(&"call:super.x".to_owned()),
            "`super` must flatten into a call path"
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

        let deep_expression_nesting =
            format!("const p = 1{};", "+1".repeat(MAX_DEPTH as usize + 2));
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
    // Declaration forms, import forms, and the remaining fallbacks
    // -----------------------------------------------------------------------------------------

    #[test]
    fn every_variable_declaration_kind_is_named_in_the_fingerprint() {
        for (kind, body) in [
            ("var", "var a = 1;"),
            ("let", "let a = 1;"),
            ("const", "const a = 1;"),
            ("using", "using a = r;"),
        ] {
            let alpha = alpha_of(body);
            assert!(
                alpha.contains(&format!("({kind} (decl ")),
                "`{body}` must be serialized as a `{kind}` declaration: {alpha}"
            );
        }
        let alpha = async_alpha_of("await using a = r;");
        assert!(
            alpha.contains("(await-using (decl "),
            "`await using` is its own declaration kind: {alpha}"
        );

        // The same five kinds are re-derived for a `for` statement's own init clause.
        for (kind, body) in [
            ("var", "for (var i = 1;;) 1;"),
            ("let", "for (let i = 1;;) 1;"),
            ("const", "for (const i = 1;;) 1;"),
            ("using", "for (using i = r;;) 1;"),
        ] {
            let alpha = alpha_of(body);
            assert!(
                alpha.contains(&format!("(for ({kind} (decl ")),
                "`{body}`'s init must be serialized as a `{kind}` declaration: {alpha}"
            );
        }
        let alpha = async_alpha_of("for (await using i = r;;) 1;");
        assert!(
            alpha.contains("(for (await-using (decl "),
            "`for (await using ...)` is its own kind: {alpha}"
        );
    }

    #[test]
    fn a_destructuring_default_and_an_unmodeled_for_target_keep_their_own_shapes() {
        assert!(
            alpha_of("const [a = 1] = arr;").contains("(default "),
            "a defaulted element of a destructuring declaration is modeled"
        );
        assert!(
            alpha_of("const {a = 1} = o;").contains("(default "),
            "...and so is a defaulted destructured property"
        );
        // A destructuring for-of target (reassigning existing bindings, not declaring) is rare
        // in test bodies; raw text is the deliberate fallback.
        assert!(
            alpha_of("let a; for ([a] of o) 1;").contains("<opaque:[a]>"),
            "an unmodeled for-of target falls back to verbatim source"
        );
    }

    #[test]
    fn every_import_form_yields_a_stable_free_identity() {
        // A default import's exported name is `default`, not the local binding's name, so two
        // files importing the same module under different local names still agree.
        let prints = fingerprint_all(
            "import d from './m';\ntest('a', () => { g(d); });\n\
             import x from './m';\ntest('b', () => { g(x); });",
        );
        assert_eq!(prints.len(), 2);
        assert_eq!(
            prints[0].alpha, prints[1].alpha,
            "two default imports of one module must share an identity whatever they are named"
        );

        // A string export name (`import { 'a-b' as ab }`) is a name no identifier can spell.
        let prints = fingerprint_all(
            "import { 'a-b' as ab } from './m';\ntest('a', () => { g(ab); });\n\
             import { 'c-d' as cd } from './m';\ntest('b', () => { g(cd); });",
        );
        assert_ne!(
            prints[0].alpha, prints[1].alpha,
            "two different string export names from one module are different identities"
        );

        // A namespace import is not a named export and carries no exported-name identity.
        let prints = fingerprint_all(
            "import * as ns from './m';\ntest('a', () => { g(ns); });\n\
             import * as other from './m';\ntest('b', () => { g(other); });",
        );
        assert_eq!(
            prints[0].alpha, prints[1].alpha,
            "two namespace imports of one module are the same free reference"
        );
    }

    #[test]
    fn a_literal_containing_a_backslash_is_escaped_rather_than_ambiguous() {
        // `escape_literal_delimiters` escapes `\` as well as `<` and `>`: without it a payload
        // ending in a raw backslash could pair with a following escaped delimiter.
        let plain = alpha_of(r"const v = 'a\\b';");
        assert!(
            plain.contains(r"<string:a\\b>"),
            "a backslash in a literal's value must be escaped: {plain}"
        );
        // And the escaping is injective: a literal that spells out an escaped delimiter cannot
        // collide with one that contains the delimiter itself.
        let prints = fingerprint_all(
            "test('a', () => { const v = 'x\\\\<y'; });\n\
             test('b', () => { const v = 'x\\\\\\\\<y'; });",
        );
        assert_ne!(
            prints[0].alpha, prints[1].alpha,
            "two distinct backslash payloads must not collide"
        );
    }

    #[test]
    fn a_quoted_escape_inside_a_type_argument_is_carried_through_verbatim() {
        // The `\` branch of `normalize_type_argument_text`'s quote scanner: the escaped
        // character is consumed with its backslash, so an escaped quote does not end the string
        // and the whitespace after it is still normalized.
        let with_escape = behavior_of(r"expect(v).toEqual<{ a: 'p\'q', b: 1 }>(1);");
        let compact = behavior_of(r"expect(v).toEqual<{a:'p\'q',b:1}>(1);");
        assert_eq!(
            with_escape, compact,
            "an escaped quote must not derail the normalizer's quote tracking"
        );
        let different = behavior_of(r"expect(v).toEqual<{a:'pq',b:1}>(1);");
        assert_ne!(
            with_escape, different,
            "...while the escaped content itself is still part of the type"
        );
    }

    // -----------------------------------------------------------------------------------------
    // Dataflow candidate selection
    // -----------------------------------------------------------------------------------------

    /// `analyze_dataflow` only ever considers a single-declarator `const` whose one read is in
    /// the same flat statement list. Every other shape is skipped, and each skip is one row.
    ///
    /// Un-inlined is the safe answer, so these rows assert the *absence* of a canonicalization
    /// this module must not claim -- with the positive control in
    /// `a_read_inside_control_flow_or_a_closure_is_never_inlined` proving the mechanism works at
    /// all, so "skip everything" cannot pass both.
    #[test]
    fn only_a_single_declarator_const_read_once_is_ever_inlined() {
        #[rustfmt::skip]
        let never_inlined: &[(&str, &str)] = &[
            ("let-not-const", "let x = build(1); expect(x).toBe(3);"),
            ("var-not-const", "var x = build(1); expect(x).toBe(3);"),
            ("two-declarators", "const x = build(1), y = 2; expect(x).toBe(3);"),
            ("read-twice", "const x = build(1); g(x); expect(x).toBe(3);"),
            ("never-read", "const x = build(1); expect(build(2)).toBe(3);"),
        ];
        for (name, body) in never_inlined {
            let events = behavior_of(body);
            assert!(
                events
                    .iter()
                    .any(|event| event.starts_with("call:global:build")),
                "{name}: `{body}` must keep the initializer's own event where it stands: \
                 {events:?}"
            );
        }
        // A declarator with no initializer at all is skipped before any of that.
        let events = behavior_of("let x; x = build(1); expect(2).toBe(3);");
        assert!(
            events.iter().any(
                |event| event.starts_with("assign:=") && event.contains("call id:global:build")
            ),
            "a declaration with no initializer is not an inlining candidate: {events:?}"
        );
        // An array-pattern declaration is neither an `Inline` nor a `Member` candidate.
        assert!(
            behavior_of("const [x] = build(1); expect(x).toBe(3);")
                .first()
                .is_some_and(|event| event.starts_with("call:global:build")),
            "an array-pattern declaration is not a dataflow candidate"
        );
    }

    /// `try_register_members` only produces `object.property` equivalence for shapes it can
    /// vouch for. Declined shapes still retain the destructured property's identity.
    #[test]
    fn destructuring_to_member_access_declines_every_shape_it_cannot_vouch_for() {
        let subject_for = |source: String| assertion_subject_of_source(&source);
        let imported =
            |body: &str| format!("import {{ o, k }} from './m';\ntest('t', () => {{ {body} }});");

        // The control: a plain destructured property of a resolved free object IS rewritten.
        let subject = subject_for(imported("const { a } = o; expect(a).toBe(3);"));
        assert!(
            subject.starts_with("(member ") && subject.ends_with(" .a)"),
            "the control must be rewritten, got {subject:?}"
        );

        #[rustfmt::skip]
        let unnamed: &[(&str, &str)] = &[
            ("computed-key", "const { [k]: a } = o; expect(a).toBe(3);"),
            ("nested-pattern", "const { a: { b } } = o; expect(b).toBe(3);"),
            ("property-default", "const { a = 1 } = o; expect(a).toBe(3);"),
            ("object-pattern-rest", "const { ...a } = o; expect(a).toBe(3);"),
        ];
        for (name, body) in unnamed {
            let subject = subject_for(imported(body));
            assert!(
                subject.starts_with("id:L"),
                "{name}: `{body}` has no plain property identity to retain, got {subject:?}"
            );
        }

        #[rustfmt::skip]
        let identity_only: &[(&str, &str)] = &[
            ("object-is-not-an-identifier", "const { a } = mk(); expect(a).toBe(3);"),
            ("object-is-a-body-local", "const o2 = mk(); const { a } = o2; expect(a).toBe(3);"),
        ];
        for (name, body) in identity_only {
            let subject = subject_for(imported(body));
            assert!(
                subject.starts_with("id:L") && subject.ends_with(":prop:a"),
                "{name}: `{body}` is not a shape this rewrite can vouch for, so the read must \
                 retain only its property identity, got {subject:?}"
            );
        }

        // An unresolved true global has no identity safe to key the rewrite on, even though the
        // pattern itself is the supported shape.
        let subject = subject_for(
            "test('t', () => { const { a } = someGlobal; expect(a).toBe(3); });".to_owned(),
        );
        assert!(
            subject.starts_with("id:L") && subject.ends_with(":prop:a"),
            "an unresolved global object carries no safe object identity, but the property name \
             must survive, got {subject:?}"
        );

        // A destructured property that is never read has nothing to substitute.
        let events =
            fingerprint_all(&imported("const { a } = o; expect(1).toBe(3);")).swap_remove(0);
        assert_eq!(
            events.behavior.len(),
            1,
            "an unread destructured property contributes no event: {:?}",
            events.behavior
        );
    }

    // -----------------------------------------------------------------------------------------
    // Assertion recognition edges
    // -----------------------------------------------------------------------------------------

    #[test]
    fn an_absurd_modifier_chain_and_an_unmodeled_base_are_not_assertions() {
        // `MAX_ASSERTION_CHAIN` bounds the unwrap loop: past it the call is treated as an
        // ordinary call rather than walked forever.
        let chain = "not.".repeat(MAX_ASSERTION_CHAIN + 2);
        let events = behavior_of(&format!("expect(1).{chain}toBe(2);"));
        assert!(
            !events.iter().any(|event| event.starts_with("assert:")),
            "a modifier chain longer than {MAX_ASSERTION_CHAIN} is not an assertion: {events:?}"
        );
        // ...and one link inside the bound still is.
        let chain = "not.".repeat(MAX_ASSERTION_CHAIN);
        let events = behavior_of(&format!("expect(1).{chain}toBe(2);"));
        assert!(
            events.iter().any(|event| event.starts_with("assert:")),
            "a chain of exactly {MAX_ASSERTION_CHAIN} modifiers is still an assertion: {events:?}"
        );

        #[rustfmt::skip]
        let not_assertions: &[(&str, &str)] = &[
            ("base-callee-is-a-call", "f()(1).toBe(2);"),
            ("soft-on-something-else", "o.expect.soft(1).toBe(2);"),
            ("modifier-on-a-non-member", "(1).toBe(2);"),
            ("matcher-on-a-bare-identifier", "o.toBe(2);"),
        ];
        for (name, body) in not_assertions {
            let events = behavior_of(body);
            assert!(
                !events.iter().any(|event| event.starts_with("assert:")),
                "{name}: `{body}` is not an `expect` assertion: {events:?}"
            );
        }
    }

    /// Every `assert:chain:` event a body emits, in order.
    fn chain_events(body: &str) -> Vec<String> {
        behavior_of(body)
            .into_iter()
            .filter(|event| event.starts_with("assert:chain:"))
            .collect()
    }

    /// The defect `property_chain_event` exists for: a chai-style property assertion carries no
    /// call node, so before it the chain contributed *nothing* and
    /// `expect(spy).to.not.have.been.calledOnce` compared equal to
    /// `expect(spy).to.have.been.calledTwice` -- opposite polarity and a different assertion.
    /// Distilled from `vitest`'s own `test/unit/test/chai-style-assertions.test.ts` (:111/:419);
    /// the extractor-level twin is `extract.rs`'s
    /// `behavior_trace_records_a_property_chain_assertion_and_its_polarity`.
    #[test]
    fn a_property_chain_assertion_reaches_behavior_with_its_polarity() {
        let setup = "const spy = vi.fn(); spy(); spy();";
        let negated = behavior_of(&format!("{setup} expect(spy).to.not.have.been.calledOnce;"));
        let plain = behavior_of(&format!("{setup} expect(spy).to.have.been.calledTwice;"));
        assert_ne!(
            negated, plain,
            "opposite-polarity property chains over identical setup must not compare equal"
        );
        assert!(
            negated.contains(&"assert:chain:expect.to.not.have.been.calledOnce".to_owned()),
            "the chain must reach the trace verbatim, `not` included: {negated:?}"
        );
        // Polarity alone, with the rest of the chain held fixed, is enough to separate them --
        // the narrowest form of the same claim.
        assert_ne!(
            chain_events("expect(spy).to.have.been.calledOnce;"),
            chain_events("expect(spy).to.not.have.been.calledOnce;"),
            "`not` must be load-bearing on its own"
        );
        // Additive, not atomic: the `expect(...)` call event that carries the subject in
        // `Mode::Alpha` still follows, which is what keeps two chains differing only in a
        // subject literal apart.
        assert_ne!(
            behavior_of("expect(build(1)).to.be.ok;"),
            behavior_of("expect(build(2)).to.be.ok;"),
            "the subject must still discriminate under a property chain"
        );
    }

    /// One assertion, one event. The chain is walked by skipping straight to the base
    /// `expect(...)` call; descending link by link instead would re-enter `walk_member_value` on
    /// every prefix and emit `expect.to.not.have.been`, `expect.to.not.have`, ... as well.
    #[test]
    fn a_property_chain_emits_exactly_one_event_however_long_it_is() {
        assert_eq!(
            chain_events("expect(spy).to.not.have.been.calledOnce;"),
            vec!["assert:chain:expect.to.not.have.been.calledOnce".to_owned()],
        );
        // `expect.soft`/`expect.poll` are the base spellings `expect_base` also accepts, and the
        // qualifier has to say which one was written.
        assert_eq!(
            chain_events("expect.soft(x).to.be.ok;"),
            vec!["assert:chain:expect.soft.to.be.ok".to_owned()],
        );
        assert_eq!(
            chain_events("expect.poll(x).to.be.ok;"),
            vec!["assert:chain:expect.poll.to.be.ok".to_owned()],
        );
    }

    /// A chai *method* assertion (`expect(x).to.have.property('a')`) is a call, so its member
    /// names already reach the trace through the ordinary `call:` path's own dotted callee path.
    /// A chain event for its `expect(x).to.have` prefix would count one assertion twice, which is
    /// why callees are walked by `walk_under_members` rather than `walk_member_value`. The second half
    /// pins that skipping the links costs nothing: everything inside the callee is still walked.
    #[test]
    fn a_chain_in_callee_position_is_not_a_property_chain_assertion() {
        let events = behavior_of("expect(x).to.have.property('a');");
        assert!(
            chain_events("expect(x).to.have.property('a');").is_empty(),
            "a called chai assertion must not also emit a chain event: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| event.starts_with("call:global:expect.to.have.property")),
            "the call path already names the chain: {events:?}"
        );
        // The callee descent still reaches the calls nested inside it.
        let fluent = behavior_of("page.locator(s).click();");
        for expected in ["call:global:page.locator.click", "call:global:page.locator"] {
            assert!(
                fluent.iter().any(|event| event.starts_with(expected)),
                "`{expected}` must survive walking the callee: {fluent:?}"
            );
        }
        let computed = behavior_of("registry[key()].run();");
        assert!(
            computed
                .iter()
                .any(|event| event.starts_with("call:global:key")),
            "a computed callee segment's own call must still be walked: {computed:?}"
        );
    }

    /// The negative side. Each of these is one edit away from the recognized shape, so a
    /// recognizer that stopped checking its premise would fail here rather than quietly widen.
    #[test]
    fn a_member_chain_that_is_not_rooted_in_expect_emits_no_chain_event() {
        #[rustfmt::skip]
        let declined: &[(&str, &str)] = &[
            ("root-is-not-a-call", "o.to.be.ok;"),
            ("root-is-a-local-named-expect", "const expect = mk(); expect(x).to.be.ok;"),
            ("root-is-another-call", "assertThat(x).to.be.ok;"),
            ("root-is-an-unmodeled-expect-member", "expect.unknown(x).to.be.ok;"),
            ("computed-link", "expect(x).to['be'].ok;"),
        ];
        for (name, body) in declined {
            let events = chain_events(body);
            assert!(
                events.is_empty(),
                "{name}: `{body}` is not a chai-style property assertion: {events:?}"
            );
        }
        // The control: the same bodies with the one edit undone are recognized.
        assert_eq!(chain_events("expect(x).to.be.ok;").len(), 1);
    }

    /// `MAX_ASSERTION_CHAIN` bounds the property-chain unwrap loop exactly as it bounds
    /// `assertion_event`'s. Probed one link either side of the edge, not comfortably inside it.
    #[test]
    fn a_property_chain_longer_than_the_bound_is_not_an_assertion() {
        let at_bound = "a.".repeat(MAX_ASSERTION_CHAIN - 1);
        let events = chain_events(&format!("expect(x).{at_bound}last;"));
        assert_eq!(
            events.len(),
            1,
            "a chain of exactly {MAX_ASSERTION_CHAIN} links is still an assertion: {events:?}"
        );
        let past_bound = "a.".repeat(MAX_ASSERTION_CHAIN);
        let events = chain_events(&format!("expect(x).{past_bound}last;"));
        assert!(
            events.is_empty(),
            "a chain of {} links is past the bound: {events:?}",
            MAX_ASSERTION_CHAIN + 1
        );
    }

    // -----------------------------------------------------------------------------------------
    // Row substitution
    // -----------------------------------------------------------------------------------------

    /// Fingerprints the first arrow callback in `source` as if it were a `.each` row body with
    /// `substitution` supplied -- the state `extract.rs` puts this module in for a row whose
    /// table it proved fully readable, and the only way to reach `resolve_row_substitution`.
    struct RowBodyCollector<'a, 'b> {
        semantic: &'b Semantic<'a>,
        source: &'b str,
        substitution: &'b RowSubstitution,
        result: Option<BodyFingerprint>,
    }

    impl<'a> Visit<'a> for RowBodyCollector<'a, '_> {
        /// Fingerprint the first readable arrow callback with the fixture's row substitutions.
        fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
            if self.result.is_none() {
                if let Some(Argument::ArrowFunctionExpression(arrow)) = it.arguments.get(1) {
                    self.result = Some(fingerprint(
                        FunctionLike::Arrow(arrow),
                        self.semantic,
                        self.source,
                        Path::new("case.test.ts"),
                        Some(self.substitution),
                    ));
                }
            }
            walk::walk_call_expression(self, it);
        }
    }

    /// Fingerprint the fixture's first readable arrow callback with the supplied row
    /// substitutions.
    fn fingerprint_row(source: &str, substitution: &RowSubstitution) -> BodyFingerprint {
        let allocator = Allocator::default();
        let source_type = SourceType::from_path(std::path::Path::new("case.test.ts")).unwrap();
        let parsed = Parser::new(&allocator, source, source_type).parse();
        assert!(!parsed.fatal_error, "fixture failed to parse: {source}");
        let program: &Program = &parsed.program;
        let semantic = SemanticBuilder::new().build(program).semantic;
        let mut collector = RowBodyCollector {
            semantic: &semantic,
            source,
            substitution,
            result: None,
        };
        collector.visit_program(program);
        collector.result.expect("an arrow callback")
    }

    /// Wrap a row-cell kind/raw-text pair in an owned optional substitution value.
    fn cell(kind: &'static str, raw: &str) -> Option<(&'static str, String)> {
        Some((kind, raw.to_owned()))
    }

    /// A fully-readable row's cells are substituted into the body *and* erased from the printed
    /// signature, so a substituted row matches the hand-written duplicate it is equivalent to --
    /// which declares no parameters at all.
    #[test]
    fn a_readable_row_substitutes_its_cells_and_drops_the_parameters_they_bound() {
        let substituted = fingerprint_row(
            "test('t', (a, b) => { expect(area(a, b)).toBe(6); });",
            &RowSubstitution::Positional(vec![cell("number", "2"), cell("number", "3")]),
        );
        let handwritten = fingerprint_all("test('t', () => { expect(area(2, 3)).toBe(6); });");
        assert_eq!(
            substituted.alpha, handwritten[0].alpha,
            "a substituted row must be byte-identical to the hand-written form it duplicates"
        );
        assert_eq!(
            substituted.behavior, handwritten[0].behavior,
            "...including its behavior events"
        );

        // Named rows bind by field name into a single destructured parameter, and the whole
        // pattern disappears when every property it declares was substituted.
        let substituted = fingerprint_row(
            "test('t', ({ a, b }) => { expect(area(a, b)).toBe(6); });",
            &RowSubstitution::Named(vec![
                ("a".to_owned(), ("number", "2".to_owned())),
                ("b".to_owned(), ("number", "3".to_owned())),
            ]),
        );
        assert_eq!(
            substituted.alpha, handwritten[0].alpha,
            "a named row must substitute by field name"
        );

        // A property the row supplies no field for stays an ordinary local, and the pattern
        // therefore stays in the signature.
        let partial = fingerprint_row(
            "test('t', ({ a, c }) => { expect(area(a, c)).toBe(6); });",
            &RowSubstitution::Named(vec![("a".to_owned(), ("number", "2".to_owned()))]),
        );
        assert!(
            partial.alpha.contains("(object-pattern (prop prop:c "),
            "an unsupplied property is left declared: {}",
            partial.alpha
        );
        assert!(
            partial.alpha.contains("<number:2>"),
            "...while the supplied one is still substituted: {}",
            partial.alpha
        );

        // A positional gap (a trailing `TestContext` parameter, or an elided cell) is the same
        // case: that parameter is left exactly as it would be with no substitution at all.
        let gap = fingerprint_row(
            "test('t', (a, ctx) => { expect(area(a, ctx)).toBe(6); });",
            &RowSubstitution::Positional(vec![cell("number", "2"), None]),
        );
        assert!(
            gap.alpha.contains("<number:2>") && gap.alpha.contains("(fn (L0) "),
            "the substituted parameter vanishes and the gap parameter stays: {}",
            gap.alpha
        );
    }

    /// `resolve_row_substitution` is deliberately all-or-nothing: any shape whose cell-to-
    /// parameter mapping it cannot vouch for substitutes *nothing*, because a body with some
    /// parameters replaced by literals and others left as locals has lost exactly the
    /// differentiation an entirely unsubstituted body still has.
    #[test]
    fn a_row_whose_parameter_shape_cannot_be_vouched_for_substitutes_nothing() {
        let positional = RowSubstitution::Positional(vec![cell("number", "2")]);
        let named = RowSubstitution::Named(vec![("a".to_owned(), ("number", "2".to_owned()))]);

        #[rustfmt::skip]
        let declined: &[(&str, &str, &RowSubstitution)] = &[
            ("rest-parameter", "test('t', (...a) => { g(a); });", &positional),
            ("parameter-default", "test('t', (a = 1) => { g(a); });", &positional),
            ("positional-into-a-pattern", "test('t', ([a]) => { g(a); });", &positional),
            ("named-into-a-plain-parameter", "test('t', (a) => { g(a); });", &named),
            ("named-with-two-parameters", "test('t', ({ a }, b) => { g(a); });", &named),
            ("named-into-a-rest-pattern", "test('t', ({ a, ...r }) => { g(a); });", &named),
            ("named-into-a-nested-property", "test('t', ({ a: { b } }) => { g(b); });", &named),
            ("named-into-a-computed-key", "test('t', ({ [k]: v, a }) => { g(v); });", &named),
        ];
        for (name, source, substitution) in declined {
            let with_row = fingerprint_row(source, substitution);
            let without_row = fingerprint_all(source);
            assert_eq!(
                with_row.alpha, without_row[0].alpha,
                "{name}: `{source}` must fingerprint exactly as it does with no substitution \
                 at all"
            );
        }
    }

    /// A written parameter no longer holds the cell it bound, so substituting its later reads
    /// would render the cell where the body actually observes the write -- the false-merge class
    /// helper inlining declines (`helpers.rs`'s written-parameter refusal). The refusal mirrors
    /// the shape guard: all-or-nothing, and covering exactly the parameters that would have
    /// entered the map.
    #[test]
    fn a_row_whose_parameter_is_written_substitutes_nothing() {
        let positional = RowSubstitution::Positional(vec![cell("number", "2")]);

        #[rustfmt::skip]
        let written: &[(&str, &str)] = &[
            ("plain-assignment", "test('t', (a) => { a = 5; g(a); });"),
            ("compound-assignment", "test('t', (a) => { a += 5; g(a); });"),
            ("update", "test('t', (a) => { a++; g(a); });"),
            ("destructuring-write", "test('t', (a) => { [a] = arr; g(a); });"),
            ("for-of-target", "test('t', (a) => { for (a of arr) { g(a); } });"),
        ];
        for (name, source) in written {
            let with_row = fingerprint_row(source, &positional);
            let without_row = fingerprint_all(source);
            assert_eq!(
                with_row.alpha, without_row[0].alpha,
                "{name}: a written parameter must leave the body unsubstituted: `{source}`"
            );
        }

        let named = RowSubstitution::Named(vec![("a".to_owned(), ("number", "2".to_owned()))]);
        let source = "test('t', ({ a }) => { a = 5; g(a); });";
        assert_eq!(
            fingerprint_row(source, &named).alpha,
            fingerprint_all(source)[0].alpha,
            "a written destructured property must leave the body unsubstituted"
        );

        // All-or-nothing includes the sibling: one written parameter declines the whole row.
        let source = "test('t', (a, b) => { a = 5; g(a, b); });";
        let both = RowSubstitution::Positional(vec![cell("number", "2"), cell("number", "3")]);
        assert_eq!(
            fingerprint_row(source, &both).alpha,
            fingerprint_all(source)[0].alpha,
            "one written parameter must substitute nothing for the row"
        );

        // The guard covers exactly the cell-bound parameters: a written parameter with no cell
        // stays the ordinary local it always was, and does not decline its sibling.
        let source = "test('t', (a, ctx) => { ctx = 5; g(a, ctx); });";
        let gap = RowSubstitution::Positional(vec![cell("number", "2"), None]);
        let substituted = fingerprint_row(source, &gap);
        assert!(
            substituted.alpha.contains("<number:2>"),
            "the cell-bound sibling still substitutes: {}",
            substituted.alpha
        );
    }

    /// An optional-chain expression used as a *callee* is the one place `call_path`,
    /// `ser_chain` and `walk_expr` each reach their own `ChainElement` match -- `o?.b()` puts the
    /// call inside the chain, so the callee it flattens is a plain member; `(o?.b)()` puts the
    /// chain inside the callee. Every element kind is one row, and each is asserted to produce
    /// the same path as its non-optional twin, which is the equivalence the arm exists for.
    #[test]
    fn an_optional_chain_in_callee_position_flattens_like_its_plain_twin() {
        for (name, optional, plain) in [
            ("static-member", "(o?.b)();", "o.b();"),
            ("call", "(o?.b())();", "o.b()();"),
            ("computed-member", "(o?.[0])();", "o[0]();"),
            ("non-null", "(o?.b!)();", "o.b();"),
        ] {
            assert_eq!(
                behavior_of(optional),
                behavior_of(plain),
                "{name}: `{optional}` must flatten to the same call path as `{plain}`"
            );
        }

        // The same for the class-only element kind.
        let source = "class C { #m() {} run() { test('t', () => { (this?.#m)(); }); } }";
        let events = fingerprint_all(source).swap_remove(0).behavior;
        assert!(
            events.contains(&"call:this.#m".to_owned()),
            "a private method behind an optional chain must still flatten: {events:?}"
        );

        // `ser_chain` and `walk_expr` reach their own `TSNonNullExpression` element only when the
        // assertion is the chain's outermost link.
        assert_eq!(
            alpha_of("const v = o?.b!;"),
            alpha_of("const v = o?.b;"),
            "a non-null assertion inside an optional chain carries no identity of its own"
        );
        assert_eq!(
            behavior_of("g(o?.b!);"),
            behavior_of("g(o?.b);"),
            "...and the behavior walk descends through it identically"
        );
    }

    /// The remaining deliberate raw-text fallbacks, each asserted to *be* the fallback rather
    /// than a guess: a destructuring update target, and a side-effect-only import that declares no
    /// local binding for `import_origins` to record.
    #[test]
    fn unmodeled_shapes_keep_their_verbatim_source() {
        // CHANGED: this used to assert `(o.b as any)++` falls back to `<opaque:o.b as any>`. It
        // was pinning an asymmetry, not a decision -- `ser_expr` unwraps the same four TS
        // wrappers in READ position, so one construct was transparent when read and opaque when
        // written, and a type assertion on a left-hand side silently stopped two otherwise
        // identical bodies from matching. Type syntax carries no runtime identity; the write path
        // now unwraps it too, and the pair below is what says so.
        assert_eq!(
            alpha_of("(o.b as any)++;"),
            alpha_of("o.b++;"),
            "a type assertion on an update target carries no runtime identity"
        );
        assert_eq!(
            alpha_of("(o.b as any) = 1;"),
            alpha_of("o.b = 1;"),
            "...and the same holds for a plain assignment target"
        );
        // Still a genuine fallback: a destructuring target is not a shape this models.
        assert!(
            alpha_of("[a.b] = pair;").contains("<opaque:"),
            "a destructuring assignment target this module does not model falls back to raw text"
        );

        // A bare `import './m';` has no specifiers at all. It must not disturb the identities of
        // the imports around it.
        let with_side_effect = fingerprint_all(
            "import './side';
import { r } from './m';
test('t', () => { g(r); });",
        );
        let without = fingerprint_all(
            "import { r } from './m';
test('t', () => { g(r); });",
        );
        assert_eq!(
            with_side_effect[0].alpha, without[0].alpha,
            "a side-effect-only import contributes no binding and changes no identity"
        );
    }

    /// The destructuring rewrite's read-safety guard, which is `try_register_members`' own copy
    /// of the scan `a_read_inside_control_flow_or_a_closure_is_never_inlined` covers for the
    /// single-use-local path.
    #[test]
    fn a_destructured_read_inside_control_flow_is_never_rewritten() {
        let source = "import { o } from './m';\n\
                      test('t', () => { const { a } = o; if (1) { expect(a).toBe(3); } });";
        let events = fingerprint_all(source).swap_remove(0).behavior;
        let event = events
            .iter()
            .find(|event| event.starts_with("assert:"))
            .unwrap_or_else(|| panic!("no assertion event: {events:?}"));
        assert!(
            event.contains("subject=id:L0:prop:a"),
            "a read inside a branch is not safe to rewrite as member access, but its property \
             identity must survive: {event}"
        );
    }
}
