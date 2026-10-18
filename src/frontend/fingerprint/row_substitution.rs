use super::*;

/// A `.each`/`.for` row's cell values, described purely in terms of *which parameter they bind
/// to* -- never a `SymbolId`, since `extract.rs` (where this is built) does not itself walk the
/// callback body and has no occasion to resolve one. [`resolve_row_substitution`] is what turns
/// this into the `SymbolId`-keyed map [`Serializer`]/[`BehaviorCollector`] actually consult. See
/// the module doc's "Row substitution" section for the full contract.
pub(crate) enum RowSubstitution {
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
pub(crate) fn resolve_row_substitution(
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
