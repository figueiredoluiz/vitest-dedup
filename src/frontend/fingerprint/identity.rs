use super::*;

// ---------------------------------------------------------------------------------------------
// Shared symbol-identity helpers, used by both the serializer and the behavior collector.
// ---------------------------------------------------------------------------------------------

/// Test whether the inner byte span lies entirely within the outer span.
pub(crate) fn within(inner: Span, outer: Span) -> bool {
    inner.start >= outer.start && inner.end <= outer.end
}

/// Check whether a symbol's declaration span lies inside the fingerprinted callable body.
pub(crate) fn is_local_symbol(
    semantic: &Semantic<'_>,
    body_span: Span,
    symbol_id: SymbolId,
) -> bool {
    within(semantic.scoping().symbol_span(symbol_id), body_span)
}

/// Identity for a symbol declared **outside** the body being fingerprinted. Classifies by what
/// the symbol *is*, not where it happens to sit -- and the two families below are deliberately
/// built differently, because they need opposite properties. See the module doc for the full
/// history of getting this wrong in both directions.
///
/// - An **import binding** names something that lives outside this file entirely, so its identity
///   must be content-derived: the specifier's real, resolved module (via [`resolver::resolve`](crate::frontend::resolver::resolve)) --
///   or, failing that, the specifier anchored to this file's directory (see
///   `super::imports::normalize_relative_specifier`), tagged so it can never collide with a real resolution --
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
pub(crate) fn canonical_local(
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
pub(crate) fn binding_identity(
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
pub(crate) fn reference_identity(
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
pub(crate) fn is_expect_identifier(
    semantic: &Semantic<'_>,
    body_span: Span,
    ident: &IdentifierReference<'_>,
) -> bool {
    ident.name.as_str() == "expect" && !is_local_reference(semantic, body_span, ident)
}
