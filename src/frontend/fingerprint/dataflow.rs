use super::*;

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
pub(crate) enum DataflowSubst<'a, 'b> {
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
pub(crate) fn analyze_dataflow<'a, 'b>(
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
    /// `:` cannot appear in a JS identifier, and [`collect_pattern`](DestructuredNameScan::collect_pattern) retains identifier-written
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
/// A bounded [`Visit`] walk discovers reads in statement order without depending on the optional
/// semantic node store. This also supports synthetic serializer tests that build scoping only.
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

/// Whether an expression is provably inert data: dropping it cannot change what a body does.
///
/// Stricter than [`expression_has_effect`] on purpose. That oracle treats identifier and
/// member reads as pure for the statements it keeps, but an expression this serializer is
/// about to *erase* can still throw (a TDZ read) or run a getter. Only literal data --
/// scalars, pure templates, and arrays/objects of literal data -- may be dropped.
///
/// `coerced` marks a template substitution, which is converted to a string: that runs
/// `toString`/`valueOf` on an object, array or regular expression and can throw, so only
/// primitive data is inert there.
pub(super) fn pure_literal(expr: &Expression<'_>, depth: u32, coerced: bool) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    match expr {
        Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::StringLiteral(_) => true,
        Expression::RegExpLiteral(_) => !coerced,
        Expression::TemplateLiteral(template) => template
            .expressions
            .iter()
            .all(|inner| pure_literal(inner, depth + 1, true)),
        Expression::ArrayExpression(array) => {
            !coerced
                && array.elements.iter().all(|element| {
                    element
                        .as_expression()
                        .is_some_and(|inner| pure_literal(inner, depth + 1, false))
                })
        }
        Expression::ObjectExpression(object) => {
            !coerced
                && object.properties.iter().all(|property| {
                    let ObjectPropertyKind::ObjectProperty(property) = property else {
                        return false;
                    };
                    property.kind == PropertyKind::Init
                        && !property.computed
                        && !property.method
                        && !property.shorthand
                        && property_key_name(&property.key).is_some_and(|key| key != "__proto__")
                        && pure_literal(&property.value, depth + 1, false)
                })
        }
        Expression::ParenthesizedExpression(parenthesized) => {
            pure_literal(&parenthesized.expression, depth + 1, coerced)
        }
        Expression::TSAsExpression(as_expression) => {
            pure_literal(&as_expression.expression, depth + 1, coerced)
        }
        Expression::TSSatisfiesExpression(satisfies) => {
            pure_literal(&satisfies.expression, depth + 1, coerced)
        }
        Expression::TSNonNullExpression(non_null) => {
            pure_literal(&non_null.expression, depth + 1, coerced)
        }
        _ => false,
    }
}

/// See [`statement_has_effect`]'s doc; the expression-level half of the same allowlist.
pub(crate) fn expression_has_effect(expr: &Expression<'_>, depth: u32) -> bool {
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
