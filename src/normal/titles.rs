//! Bounded literal values for names, independent of callback normalization.

use super::{form, Row};
use crate::lang::{property_name, Declaration, Field, NodeId, Parsed, SymbolId, Tree};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// A title string whose value is proved by the source, or a named function used as a title.
pub(crate) fn title(parsed: &Parsed, call: NodeId) -> Option<String> {
    title_with_bindings(parsed, call, &HashMap::new())
}

/// A title under one proved invocation's immutable positional values.
pub(crate) fn title_with_bindings(
    parsed: &Parsed,
    call: NodeId,
    bindings: &HashMap<SymbolId, Value>,
) -> Option<String> {
    let tree = &parsed.tree;
    let node = tree.inner(tree.list(call, "arguments").first().copied()??);
    if tree.kind(node) == "FunctionExpression" {
        return tree
            .child(node, "id")
            .and_then(|id| tree.text(id, "name"))
            .map(str::to_owned);
    }
    match value_with_bindings(parsed, node, 0, &mut 256, bindings)? {
        Value::String(text) => Some(text),
        _ => None,
    }
}

/// Literal printf arguments and named fields. Array rows supply positional title arguments even
/// when the callback receives the row whole. Unknown cells remain distinct from missing ones.
pub(crate) fn row(
    parsed: &Parsed,
    row: &Row,
    whole_rows: bool,
) -> (Vec<Option<Value>>, Option<Map<String, Value>>) {
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
            // A whole-row table titles a null row with no items, not with `null`.
            if whole_rows && whole == Some(Value::Null) {
                return (Vec::new(), None);
            }
            let fields = match whole.as_ref().and_then(Value::as_object) {
                Some(fields) => Some(fields.clone()),
                None => readable_fields(parsed, node, &mut budget),
            };
            (vec![whole], fields)
        }
    }
}

/// The fields an object literal row spells with readable values, when some other value is not:
/// an unreadable one is left out, so naming it stays unknown, and the last duplicate key decides.
/// Any shape that may define or shadow a field another way refuses them all.
fn readable_fields(
    parsed: &Parsed,
    node: NodeId,
    budget: &mut usize,
) -> Option<Map<String, Value>> {
    let tree = &parsed.tree;
    if tree.kind(node) != "ObjectExpression" {
        return None;
    }
    let mut fields = Map::new();
    for (name, field) in plain_properties(tree, node)? {
        match value(parsed, field, 1, budget) {
            Some(value) => fields.insert(name.to_owned(), value),
            None => fields.remove(name),
        };
    }
    Some(fields)
}

/// Reads literal construction and immutable scalar consts only; arbitrary execution, object
/// aliases and custom coercion are unknown. Depth and total visits bound even cyclic initializers.
pub(super) fn value(
    parsed: &Parsed,
    node: NodeId,
    depth: usize,
    budget: &mut usize,
) -> Option<Value> {
    value_with_bindings(parsed, node, depth, budget, &HashMap::new())
}

/// Read a bounded literal value with caller bindings; nested lexical bindings keep their IDs.
pub(crate) fn value_with_bindings(
    parsed: &Parsed,
    node: NodeId,
    depth: usize,
    budget: &mut usize,
    bindings: &HashMap<SymbolId, Value>,
) -> Option<Value> {
    if depth >= 32 || *budget == 0 {
        return None;
    }
    *budget -= 1;
    let tree = &parsed.tree;
    let node = tree.inner(node);
    let mut child = |node| value_with_bindings(parsed, node, depth + 1, budget, bindings);
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
            if let Some(value) = parsed
                .scopes
                .symbol_at(tree.span(node).start)
                .and_then(|symbol| bindings.get(&symbol))
            {
                return Some(value.clone());
            }
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
            for (name, field) in plain_properties(tree, node)? {
                fields.insert(name.to_owned(), child(field)?);
            }
            Some(Value::Object(fields))
        }
        _ => None,
    }
}

/// An object literal's `key: value` properties, in order, or nothing when any property may define
/// or shadow a field another way (a spread, a computed key, a method or accessor, or a key that
/// changes the prototype or coercion).
fn plain_properties(tree: &Tree, node: NodeId) -> Option<Vec<(&str, NodeId)>> {
    let mut properties = Vec::new();
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
        properties.push((name, tree.child(property, "value")?));
    }
    Some(properties)
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
