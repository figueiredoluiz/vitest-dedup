//! ESTree shapes several layers read: specifier names, property keys, pattern bindings.

use super::{NodeId, Scopes, SymbolId, Tree};

/// The name an import or export specifier's `imported`, `local` or `exported` field spells.
pub fn export_name(tree: &Tree, name: NodeId) -> String {
    tree.text(name, "name")
        .or_else(|| tree.text(name, "value"))
        .unwrap_or_default()
        .to_owned()
}

/// The static name of an object pattern or literal property's key.
pub fn property_name(tree: &Tree, property: NodeId) -> Option<&str> {
    let key = tree.child(property, "key")?;
    match tree.kind(key) {
        "Identifier" if !tree.flag(property, "computed") => tree.text(key, "name"),
        "Literal" => tree.text(key, "value"),
        _ => None,
    }
}

/// Every symbol a binding pattern declares, in source order.
pub fn bound_symbols(tree: &Tree, scopes: &Scopes, pattern: NodeId) -> Vec<SymbolId> {
    let mut symbols = Vec::new();
    let mut pending = vec![pattern];
    while let Some(node) = pending.pop() {
        match tree.kind(node) {
            "Identifier" => symbols.extend(scopes.symbol_at(tree.span(node).start)),
            "ObjectPattern" => {
                pending.extend(tree.list(node, "properties").into_iter().flatten().rev())
            }
            "ArrayPattern" => {
                pending.extend(tree.list(node, "elements").into_iter().flatten().rev())
            }
            "Property" => pending.extend(tree.child(node, "value")),
            "RestElement" => pending.extend(tree.child(node, "argument")),
            "AssignmentPattern" => pending.extend(tree.child(node, "left")),
            "TSParameterProperty" => pending.extend(tree.child(node, "parameter")),
            _ => {}
        }
    }
    symbols
}
