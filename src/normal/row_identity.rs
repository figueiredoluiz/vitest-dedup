//! Proven callback input classes for rows of one directly read table.

use super::{Normalizer, Row};
use crate::lang::{Declaration, NodeId, Parsed};
use serde_json::Value;

/// An exact identity of shallow literal row data. Unknown or effectful cells refuse the row,
/// including unused cells. Only fixed-parameter arrows cannot observe trailing spread cells.
pub(crate) fn identity(
    normalizer: &mut Normalizer<'_>,
    row: &Row,
    callback: NodeId,
) -> Option<String> {
    let tree = normalizer.tree;
    let parameters = tree.list(callback, "params");
    let fixed_arrow = tree.kind(callback) == "ArrowFunctionExpression"
        && parameters.iter().all(|parameter| {
            parameter.is_some_and(|parameter| tree.kind(parameter) == "Identifier")
        });
    let mut scalar = |node| {
        normalizer
            .row_scalar(node)
            .map(|value| Value::String(value.identity()))
    };
    let identity = match row {
        Row::Fields(properties) => field_identity(properties, &mut scalar)?,
        Row::Spread(cells) => {
            let mut values = cells
                .iter()
                .map(|&cell| scalar(cell))
                .collect::<Option<Vec<_>>>()?;
            if fixed_arrow {
                values.truncate(parameters.len());
            }
            Value::Array(values)
        }
        Row::Whole(node, properties) => {
            let node = tree.inner(*node);
            match tree.kind(node) {
                "ArrayExpression" => Value::Array(
                    tree.list(node, "elements")
                        .into_iter()
                        .map(|cell| scalar(cell?))
                        .collect::<Option<Vec<_>>>()?,
                ),
                "ObjectExpression" => {
                    // Preserve insertion order: Object.keys and JSON.stringify observe it.
                    field_identity(properties.as_ref()?, &mut scalar)?
                }
                _ => {
                    let value = scalar(node)?;
                    // Whole null has special title argument handling not yet modeled.
                    if value.as_str() == Some("null:null") {
                        return None;
                    }
                    value
                }
            }
        }
    };
    // Distinguish whole arrays, objects and scalars even when their serialized contents coincide.
    let shape = match row {
        Row::Fields(_) => "object",
        Row::Spread(_) => "spread",
        Row::Whole(node, _) => match tree.kind(tree.inner(*node)) {
            "ArrayExpression" => "array",
            "ObjectExpression" => "object",
            _ => "primitive",
        },
    };
    Some(format!("{shape}:{identity}"))
}

/// Preserve ordered own field identities for both tagged and ordinary object rows.
fn field_identity(
    properties: &[(String, NodeId)],
    scalar: &mut impl FnMut(NodeId) -> Option<Value>,
) -> Option<Value> {
    properties
        .iter()
        .map(|(key, node)| {
            if matches!(key.as_str(), "__proto__" | "toString" | "valueOf") {
                return None;
            }
            Some(Value::Array(vec![
                Value::String(key.clone()),
                scalar(*node)?,
            ]))
        })
        .collect::<Option<Vec<_>>>()
        .map(Value::Array)
}

/// Whether a suite registration's name prefix is proven for row and suite findings: the inventory
/// recorded its callee as reaching the API through an import (re-exports included) or the
/// project's globals, and the callee names no local binding. A local binding may hold a suite
/// table, which the site does not record, so its prefix stays unknown; so does a wrapper's.
pub(crate) fn suite_prefix_known(
    parsed: &Parsed,
    call: NodeId,
    provenance: crate::inventory::Provenance,
) -> bool {
    use crate::inventory::Provenance;
    let tree = &parsed.tree;
    let Some(callee) = tree.child(call, "callee") else {
        return false;
    };
    matches!(provenance, Provenance::Direct | Provenance::Global)
        && !std::iter::once(callee)
            .chain(tree.descendants(callee))
            .any(|node| {
                tree.kind(node) == "Identifier"
                    && parsed
                        .scopes
                        .symbol_at(tree.span(node).start)
                        .is_some_and(|symbol| {
                            !matches!(
                                parsed.scopes.symbol(symbol).declaration,
                                Declaration::Import { .. }
                            )
                        })
            })
}
