//! Bounded literal values for names, independent of callback normalization.

use super::{form, Row};
use crate::lang::limits::{cooked, MAX_TEXT_BYTES};
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
            .filter(|name| name.len() <= MAX_TEXT_BYTES)
            .map(str::to_owned);
    }
    // Numbers a template substitutes are not counted as they are read, so the title is measured
    // whole as well.
    match read(parsed, node, 0, &mut 256, bindings, &mut Limit::title())? {
        Value::String(text) if text.len() <= MAX_TEXT_BYTES => Some(text),
        _ => None,
    }
}

/// A runner call's argument: read whole, because the runner charges what it keeps to its own
/// invocation budget. A title or row that later reads it as a binding is limited there.
pub(crate) fn argument(
    parsed: &Parsed,
    node: NodeId,
    bindings: &HashMap<SymbolId, Value>,
) -> Option<Value> {
    read(parsed, node, 0, &mut 256, bindings, &mut Limit::charged())
}

/// A title read without the comparison limit, every string it copies counted against `budget`:
/// the title, and the bytes reading it copied. A runner caller's direct titles are read this way,
/// so the planner can charge their reading, not only their result, to its work budget.
pub(crate) fn title_within(
    parsed: &Parsed,
    node: NodeId,
    budget: usize,
) -> Option<(String, usize)> {
    let mut limit = Limit {
        charged: false,
        copied: true,
        left: budget,
    };
    match read(parsed, node, 0, &mut 256, &HashMap::new(), &mut limit)? {
        Value::String(text) => Some((text, budget - limit.left)),
        _ => None,
    }
}

/// How much string text one read may still copy; see [`MAX_TEXT_BYTES`]. One budget serves the
/// whole read, so naming a `const` many times spends it as many times.
#[derive(Debug)]
struct Limit {
    /// Read whole: the caller charges what it keeps to its own budget.
    charged: bool,
    /// Whether what is read here is copied by every reader: a title, or what a `const` supplies.
    copied: bool,
    /// The copied text this read may still take.
    left: usize,
}

impl Limit {
    /// A title: every test, row and nested test copies it.
    fn title() -> Self {
        Self {
            charged: false,
            copied: true,
            left: MAX_TEXT_BYTES,
        }
    }

    /// A value written in place, which a `const` or binding it names may still supply.
    fn in_place() -> Self {
        Self {
            charged: false,
            copied: false,
            left: MAX_TEXT_BYTES,
        }
    }

    /// A runner argument, which the runner charges to its invocation budget.
    fn charged() -> Self {
        Self {
            charged: true,
            copied: false,
            left: MAX_TEXT_BYTES,
        }
    }

    /// Spends text the source spells where it is read: only when that text is copied.
    fn spelled(&mut self, bytes: usize) -> Option<()> {
        if self.copied {
            self.supplied(bytes)?;
        }
        Some(())
    }

    /// Spends text a `const` or binding supplies, which every reader copies again.
    fn supplied(&mut self, bytes: usize) -> Option<()> {
        if !self.charged {
            self.left = self.left.checked_sub(bytes)?;
        }
        Some(())
    }
}

/// The string text a value holds, keys included: what copying it copies.
fn text_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len(),
        Value::Array(values) => values.iter().map(text_bytes).sum(),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| key.len() + text_bytes(value))
            .sum(),
        _ => 0,
    }
}

/// The values one read of a row may visit.
const ROW_VISITS: usize = 256;

/// The longest template built, however it is read. What a template copies for every reader is
/// counted against [`MAX_TEXT_BYTES`] as well.
const MAX_TEMPLATE_BYTES: usize = 65_536;

/// One read of a row: the values it may visit and the copied text it may take, shared by every
/// cell so that naming a `const` in many cells spends the row's one budget.
pub(crate) struct RowRead {
    /// Values left to visit.
    visits: usize,
    /// Copied text left to take.
    limit: Limit,
}

impl RowRead {
    /// A fresh read of one row.
    pub(crate) fn new() -> Self {
        Self {
            visits: ROW_VISITS,
            limit: Limit::in_place(),
        }
    }
}

/// Literal printf arguments and named fields. Array rows supply positional title arguments even
/// when the callback receives the row whole. Unknown cells remain distinct from missing ones.
pub(crate) fn row(
    parsed: &Parsed,
    row: &Row,
    whole_rows: bool,
) -> (Vec<Option<Value>>, Option<Map<String, Value>>) {
    let mut read = RowRead::new();
    match row {
        Row::Fields(properties) => {
            let fields = properties
                .iter()
                .filter_map(|(key, node)| {
                    value(parsed, *node, 0, &mut read).map(|v| (key.clone(), v))
                })
                .collect();
            (vec![None], Some(fields))
        }
        Row::Spread(cells) => (
            cells
                .iter()
                .map(|&cell| value(parsed, cell, 0, &mut read))
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
                        .map(|cell| cell.and_then(|cell| value(parsed, cell, 0, &mut read)))
                        .collect(),
                    None,
                );
            }
            let left = read.limit.left;
            let whole = value(parsed, node, 0, &mut read);
            // A whole-row table titles a null row with no items, not with `null`.
            if whole_rows && whole == Some(Value::Null) {
                return (Vec::new(), None);
            }
            let fields = match whole.as_ref().and_then(Value::as_object) {
                Some(fields) => Some(fields.clone()),
                // The field read replaces a whole read that failed, which may have spent its
                // visits and text: it starts from what the row had, not from what that read left.
                None => {
                    read.visits = ROW_VISITS;
                    read.limit.left = left;
                    readable_fields(parsed, node, &mut read)
                }
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
    read: &mut RowRead,
) -> Option<Map<String, Value>> {
    let tree = &parsed.tree;
    if tree.kind(node) != "ObjectExpression" {
        return None;
    }
    let mut fields = Map::new();
    for (name, field) in plain_properties(tree, node)? {
        match value(parsed, field, 1, read) {
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
    read: &mut RowRead,
) -> Option<Value> {
    self::read(
        parsed,
        node,
        depth,
        &mut read.visits,
        &HashMap::new(),
        &mut read.limit,
    )
}

/// Read a bounded literal value with caller bindings; nested lexical bindings keep their IDs.
#[cfg(test)]
pub(crate) fn value_with_bindings(
    parsed: &Parsed,
    node: NodeId,
    depth: usize,
    budget: &mut usize,
    bindings: &HashMap<SymbolId, Value>,
) -> Option<Value> {
    read(
        parsed,
        node,
        depth,
        budget,
        bindings,
        &mut Limit::in_place(),
    )
}

/// A bounded literal value with caller bindings under `limit`; nested lexical bindings keep their
/// IDs. Each string the limit counts is measured before it is copied.
fn read(
    parsed: &Parsed,
    node: NodeId,
    depth: usize,
    budget: &mut usize,
    bindings: &HashMap<SymbolId, Value>,
    limit: &mut Limit,
) -> Option<Value> {
    if depth >= 32 || *budget == 0 {
        return None;
    }
    *budget -= 1;
    let tree = &parsed.tree;
    let node = tree.inner(node);
    let mut child =
        |node, limit: &mut Limit| read(parsed, node, depth + 1, budget, bindings, limit);
    match tree.kind(node) {
        "Literal" => match tree.field_of(node, "value") {
            Field::Value(Value::String(text)) => {
                limit.spelled(text.len())?;
                Some(Value::String(text.clone()))
            }
            Field::Value(value @ (Value::Bool(_) | Value::Number(_))) => Some(value.clone()),
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
                limit.supplied(text_bytes(value))?;
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
            // Every read of a `const` copies what it supplies again, from the read's one budget.
            let init = tree.child(declaration, "init")?;
            let (spelled, left) = (std::mem::replace(&mut limit.copied, true), limit.left);
            let value = child(init, limit).filter(|value| !value.is_object() && !value.is_array());
            limit.copied = spelled;
            // A refused value keeps nothing, so it spends nothing.
            if value.is_none() {
                limit.left = left;
            }
            value
        }
        "UnaryExpression" => {
            let number = child(tree.child(node, "argument")?, limit)?.as_f64()?;
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
                let cooked = cooked(tree, quasi?)?;
                // A substitution is measured with the text that always follows it.
                if text.len() + cooked.len() > MAX_TEMPLATE_BYTES {
                    return None;
                }
                limit.spelled(cooked.len())?;
                text.push_str(cooked);
                if let Some(expression) = expressions.get(index) {
                    text.push_str(&string(&child((*expression)?, limit)?)?);
                }
            }
            Some(Value::String(text))
        }
        "ArrayExpression" => tree
            .list(node, "elements")
            .into_iter()
            .map(|element| match element {
                Some(element) => child(element, limit),
                None => Some(Value::Null),
            })
            .collect::<Option<Vec<_>>>()
            .map(Value::Array),
        "ObjectExpression" => {
            let mut fields = Map::new();
            for (name, field) in plain_properties(tree, node)? {
                limit.spelled(name.len())?;
                fields.insert(name.to_owned(), child(field, limit)?);
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
