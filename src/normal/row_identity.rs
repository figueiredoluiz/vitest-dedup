//! Proven callback input classes for rows of one directly read table.

use super::{titles, Row};
use crate::lang::{Declaration, NodeId, Parsed};
use serde_json::Value;

/// An exact identity of shallow literal row data. Unknown or effectful cells refuse the row,
/// including unused cells. Only fixed-parameter arrows cannot observe trailing spread cells.
pub(crate) fn identity(parsed: &Parsed, row: &Row, callback: NodeId) -> Option<String> {
    let tree = &parsed.tree;
    let parameters = tree.list(callback, "params");
    let fixed_arrow = tree.kind(callback) == "ArrowFunctionExpression"
        && parameters.iter().all(|parameter| {
            parameter.is_some_and(|parameter| tree.kind(parameter) == "Identifier")
        });
    let mut budget = 256;
    let mut scalar = |node| {
        let value = titles::value(parsed, node, 0, &mut budget)?;
        // The owned text encoding for lone surrogates is display text, not code-unit identity.
        let encoded_text = value.as_str().is_some_and(|text| text.contains('\u{FFFD}'));
        (!value.is_array() && !value.is_object() && !encoded_text).then_some(value)
    };
    let identity = match row {
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
                    let fields = properties
                        .as_ref()?
                        .iter()
                        .map(|(key, node)| {
                            if key.contains('\u{FFFD}')
                                || matches!(key.as_str(), "__proto__" | "toString" | "valueOf")
                            {
                                return None;
                            }
                            Some(Value::Array(vec![
                                Value::String(key.clone()),
                                scalar(*node)?,
                            ]))
                        })
                        .collect::<Option<Vec<_>>>()?;
                    Value::Array(fields)
                }
                _ => {
                    let value = scalar(node)?;
                    // Whole null has special title argument handling not yet modeled.
                    if value.is_null() {
                        return None;
                    }
                    value
                }
            }
        }
    };
    // Distinguish whole arrays, objects and scalars even when their serialized contents coincide.
    let shape = match row {
        Row::Spread(_) => "spread",
        Row::Whole(node, _) => match tree.kind(tree.inner(*node)) {
            "ArrayExpression" => "array",
            "ObjectExpression" => "object",
            _ => "primitive",
        },
    };
    Some(format!("{shape}:{identity}"))
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
