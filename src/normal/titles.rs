//! Bounded literal values for names, independent of callback normalization.

use super::{form, Row};
use crate::lang::{property_name, Declaration, Field, NodeId, Parsed};
use serde_json::{Map, Value};

/// A title string whose value is proved by the source, or a named function used as a title.
pub(crate) fn title(parsed: &Parsed, call: NodeId) -> Option<String> {
    let tree = &parsed.tree;
    let node = tree.inner(tree.list(call, "arguments").first().copied()??);
    if tree.kind(node) == "FunctionExpression" {
        return tree
            .child(node, "id")
            .and_then(|id| tree.text(id, "name"))
            .map(str::to_owned);
    }
    match value(parsed, node, 0, &mut 256)? {
        Value::String(text) => Some(text),
        _ => None,
    }
}

/// Literal printf arguments and named fields. Array rows supply positional title arguments even
/// when the callback receives the row whole. Unknown cells remain distinct from missing ones.
pub(crate) fn row(parsed: &Parsed, row: &Row) -> (Vec<Option<Value>>, Option<Map<String, Value>>) {
    let mut budget = 256;
    match row {
        Row::Spread(cells) => (
            cells
                .iter()
                .map(|&cell| value(parsed, cell, 0, &mut budget))
                .collect(),
            None,
        ),
        Row::Whole(node, _) => {
            let tree = &parsed.tree;
            let node = tree.inner(*node);
            if tree.kind(node) == "ArrayExpression" {
                return (
                    tree.list(node, "elements")
                        .into_iter()
                        .map(|cell| cell.and_then(|cell| value(parsed, cell, 0, &mut budget)))
                        .collect(),
                    None,
                );
            }
            let whole = value(parsed, node, 0, &mut budget);
            let fields = whole.as_ref().and_then(Value::as_object).cloned();
            (vec![whole], fields)
        }
    }
}

/// Reads literal construction and immutable scalar consts only; arbitrary execution, object
/// aliases and custom coercion are unknown. Depth and total visits bound even cyclic initializers.
pub(super) fn value(
    parsed: &Parsed,
    node: NodeId,
    depth: usize,
    budget: &mut usize,
) -> Option<Value> {
    if depth >= 32 || *budget == 0 {
        return None;
    }
    *budget -= 1;
    let tree = &parsed.tree;
    let node = tree.inner(node);
    let mut child = |node| value(parsed, node, depth + 1, budget);
    match tree.kind(node) {
        "Literal" => match tree.field_of(node, "value") {
            Field::Value(value @ (Value::String(_) | Value::Bool(_) | Value::Number(_))) => {
                Some(value.clone())
            }
            Field::Value(Value::Null) if tree.text(node, "raw") == Some("null") => {
                Some(Value::Null)
            }
            _ => None,
        },
        "Identifier" => {
            let symbol = parsed
                .scopes
                .symbol(parsed.scopes.symbol_at(tree.span(node).start)?);
            let Declaration::Variable(declaration) = symbol.declaration else {
                return None;
            };
            let parent = tree.parent(declaration)?;
            if tree.text(parent, "kind") != Some("const")
                || tree.span(declaration).end > tree.span(node).start
                || symbol.references.iter().any(|reference| reference.write)
                || tree.kind(tree.child(declaration, "id")?) != "Identifier"
            {
                return None;
            }
            let value = child(tree.child(declaration, "init")?)?;
            (!value.is_object() && !value.is_array()).then_some(value)
        }
        "UnaryExpression" => {
            let number = child(tree.child(node, "argument")?)?.as_f64()?;
            let result = match tree.text(node, "operator")? {
                "+" => number,
                "-" => -number,
                _ => return None,
            };
            serde_json::Number::from_f64(result).map(Value::Number)
        }
        "TemplateLiteral" => {
            let expressions = tree.list(node, "expressions");
            let mut text = String::new();
            for (index, quasi) in tree.list(node, "quasis").into_iter().enumerate() {
                let Field::Object(fields) = tree.field_of(quasi?, "value") else {
                    return None;
                };
                let cooked = fields
                    .iter()
                    .find_map(|(key, field)| match (&**key, field) {
                        ("cooked", Field::Value(Value::String(text))) => Some(text.as_str()),
                        _ => None,
                    })?;
                text.push_str(cooked);
                if let Some(expression) = expressions.get(index) {
                    text.push_str(&string(&child((*expression)?)?)?);
                }
                if text.len() > 65_536 {
                    return None;
                }
            }
            Some(Value::String(text))
        }
        "ArrayExpression" => tree
            .list(node, "elements")
            .into_iter()
            .map(|element| match element {
                Some(element) => child(element),
                None => Some(Value::Null),
            })
            .collect::<Option<Vec<_>>>()
            .map(Value::Array),
        "ObjectExpression" => {
            let mut fields = Map::new();
            for property in tree.list(node, "properties") {
                let property = property?;
                if tree.kind(property) != "Property"
                    || tree.flag(property, "computed")
                    || tree.flag(property, "method")
                    || tree.text(property, "kind") != Some("init")
                {
                    return None;
                }
                let name = property_name(tree, property)?;
                if matches!(name, "__proto__" | "toString" | "valueOf") {
                    return None;
                }
                fields.insert(name.to_owned(), child(tree.child(property, "value")?)?);
            }
            Some(Value::Object(fields))
        }
        _ => None,
    }
}

/// JavaScript string coercion for the literal values admitted above. Objects have no custom
/// coercion; array nulls and holes contribute empty fields rather than the word `null`.
fn string(value: &Value) -> Option<String> {
    Some(match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => form::js_number(number.as_f64()?),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".to_owned(),
        Value::Object(_) => "[object Object]".to_owned(),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                if value.is_null() {
                    Some(String::new())
                } else {
                    string(value)
                }
            })
            .collect::<Option<Vec<_>>>()?
            .join(","),
    })
}
