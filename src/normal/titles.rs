//! Bounded literal values for names, independent of callback normalization.

use super::{form, Row};
use crate::lang::limits::{cooked, MAX_TEXT_BYTES};
use crate::lang::{property_name, Declaration, Field, NodeId, Parsed, SymbolId, Tree};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// What a read may assume beyond the source: invocation values, which suite callbacks run late,
/// and, while a `let`'s writes are replayed, the value it holds inside the expression being run.
#[derive(Clone, Copy)]
struct Reading<'a> {
    bindings: &'a HashMap<SymbolId, Value>,
    deferral: Option<&'a Deferral<'a>>,
    /// The replayed symbol, the expression it is read in at the replay point, and its value there.
    current: Option<(SymbolId, crate::lang::Span, &'a Value)>,
}

impl<'a> Reading<'a> {
    /// Invocation values only.
    fn of(bindings: &'a HashMap<SymbolId, Value>) -> Self {
        Self {
            bindings,
            deferral: None,
            current: None,
        }
    }
}

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
    title_deferred(parsed, call, bindings, None)
}

/// [`title_with_bindings`] for a registration inside suite callbacks, which may read a `let` the
/// body around them leaves.
pub(crate) fn title_deferred(
    parsed: &Parsed,
    call: NodeId,
    bindings: &HashMap<SymbolId, Value>,
    deferral: Option<&Deferral<'_>>,
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
    let reading = Reading {
        deferral,
        ..Reading::of(bindings)
    };
    match read(parsed, node, 0, &mut 256, reading, &mut Limit::title())? {
        Value::String(text) if text.len() <= MAX_TEXT_BYTES => Some(text),
        _ => None,
    }
}

/// A value read without the comparison limit, every string it copies counted against `budget`:
/// the value, and the bytes reading it copied. The runner planner reads a call's arguments and
/// its caller's direct titles this way, so it can charge their reading, not only their result,
/// to its work budget.
pub(crate) fn value_within(
    parsed: &Parsed,
    node: NodeId,
    bindings: &HashMap<SymbolId, Value>,
    budget: usize,
) -> Option<(Value, usize)> {
    let mut limit = Limit {
        copied: true,
        left: budget,
    };
    let value = read(parsed, node, 0, &mut 256, Reading::of(bindings), &mut limit)?;
    Some((value, budget - limit.left))
}

/// [`value_within`] for a title, which must be a string.
pub(crate) fn title_within(
    parsed: &Parsed,
    node: NodeId,
    budget: usize,
) -> Option<(String, usize)> {
    match value_within(parsed, node, &HashMap::new(), budget)? {
        (Value::String(text), spent) => Some((text, spent)),
        _ => None,
    }
}

/// How much string text one read may still copy; see [`MAX_TEXT_BYTES`]. One budget serves the
/// whole read, so naming a `const` many times spends it as many times.
#[derive(Debug)]
struct Limit {
    /// Whether what is read here is copied by every reader: a title, or what a `const` supplies.
    copied: bool,
    /// The copied text this read may still take.
    left: usize,
}

impl Limit {
    /// A title: every test, row and nested test copies it. Reading a template copies its text
    /// once more as it is assembled, so the read may copy twice the limit; the title itself is
    /// measured against [`MAX_TEXT_BYTES`] when it is returned.
    fn title() -> Self {
        Self {
            copied: true,
            left: 2 * MAX_TEXT_BYTES,
        }
    }

    /// A value written in place, which a `const` or binding it names may still supply.
    fn in_place() -> Self {
        Self {
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
        self.left = self.left.checked_sub(bytes)?;
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
        Reading::of(&HashMap::new()),
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
        Reading::of(bindings),
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
    reading: Reading<'_>,
    limit: &mut Limit,
) -> Option<Value> {
    if depth >= 32 || *budget == 0 {
        return None;
    }
    *budget -= 1;
    let tree = &parsed.tree;
    let node = tree.inner(node);
    let mut child = |node, limit: &mut Limit| read(parsed, node, depth + 1, budget, reading, limit);
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
            let symbol = parsed.scopes.symbol_at(tree.span(node).start);
            // A replayed `let` read in the expression being run has the value the replay reached;
            // read anywhere else, through another declaration, it is proved from its own position.
            let current = reading.current.filter(|&(replayed, within, _)| {
                symbol == Some(replayed) && within.contains(tree.span(node))
            });
            if let Some(value) = current
                .map(|(_, _, value)| value)
                .or_else(|| symbol.and_then(|symbol| reading.bindings.get(&symbol)))
            {
                limit.supplied(text_bytes(value))?;
                return Some(value.clone());
            }
            let id = parsed.scopes.symbol_at(tree.span(node).start)?;
            let symbol = parsed.scopes.symbol(id);
            let Declaration::Variable(declaration) = symbol.declaration else {
                return None;
            };
            let parent = tree.parent(declaration)?;
            if tree.text(parent, "kind") == Some("let")
                && tree.span(declaration).end <= tree.span(node).start
                && tree.kind(tree.child(declaration, "id")?) == "Identifier"
            {
                let mut replay = Replay {
                    parsed,
                    symbol: id,
                    depth,
                    budget,
                    reading,
                    current: None,
                    limit,
                    writes: Vec::new(),
                };
                return replay.at(declaration, node);
            }
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
                    // The substitution's text is copied into this template again, at every level
                    // of a nested template.
                    let part = string(&child((*expression)?, limit)?)?;
                    limit.spelled(part.len())?;
                    text.push_str(&part);
                }
            }
            Some(Value::String(crate::lang::text::canonical(text)))
        }
        "CallExpression" => {
            // Only `text.replaceAll(pattern, replacement)` over strings, with neither a `$`
            // substitution nor the empty pattern, which inserts between every code unit. A setup
            // file replacing the method is not seen, as for `.each` tables and the array iterator.
            let callee = tree.inner(tree.child(node, "callee")?);
            // The built-in method, unless the module may have replaced it.
            if parsed.scopes.may_replace_builtins()
                || tree.flag(node, "optional")
                || tree.kind(callee) != "MemberExpression"
                || tree.flag(callee, "computed")
                || tree.flag(callee, "optional")
                || tree
                    .child(callee, "property")
                    .and_then(|property| tree.text(property, "name"))
                    != Some("replaceAll")
            {
                return None;
            }
            let [Some(pattern), Some(replacement)] = tree.list(node, "arguments")[..] else {
                return None;
            };
            let mut text = |node, limit: &mut Limit| match child(node, limit)? {
                Value::String(text) => Some(text),
                _ => None,
            };
            let subject = text(tree.child(callee, "object")?, limit)?;
            let pattern = text(pattern, limit)?;
            let replacement = text(replacement, limit)?;
            // U+FFFD introduces an escaped code unit in the lossless text form; a match must not
            // start or end inside one.
            if pattern.is_empty()
                || replacement.contains('$')
                || [&subject, &pattern, &replacement]
                    .iter()
                    .any(|text| text.contains('\u{fffd}'))
            {
                return None;
            }
            let count = subject.matches(pattern.as_str()).count();
            let length = (subject.len() - count * pattern.len())
                .checked_add(count.checked_mul(replacement.len())?)?;
            if length > MAX_TEMPLATE_BYTES {
                return None;
            }
            limit.spelled(length)?;
            Some(Value::String(
                subject.replace(pattern.as_str(), &replacement),
            ))
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

/// The most references one `let` may have for a read to replay its writes: each read checks them
/// all.
const MAX_LET_REFERENCES: usize = 256;

/// Suite callbacks Vitest runs only after the body registering them returns, and the functions
/// that may return before their body ends. Both are read once per file.
pub(crate) struct Deferral<'a> {
    /// Inline suite callbacks, by the call registering each.
    pub suites: &'a HashMap<NodeId, NodeId>,
    /// Functions holding a `return` of their own.
    pub returning: &'a std::collections::HashSet<NodeId>,
}

/// What a `let` holds where it is read: its initializer, then each `=` its body runs before the
/// statement reading it. A read inside suite callbacks the body registers runs after the body
/// returns (measured: Vitest 5.1.0), so it takes the value the whole body leaves, which only a body
/// that cannot return early, await or yield is known to leave. Proved only when every write is
/// one of those assignments and every branch to one is decided by proved values.
struct Replay<'a, 'b> {
    parsed: &'a Parsed,
    symbol: SymbolId,
    depth: usize,
    budget: &'b mut usize,
    reading: Reading<'b>,
    /// The `let`'s value so far.
    current: Option<Value>,
    limit: &'b mut Limit,
    /// Where the symbol is assigned.
    writes: Vec<crate::lang::Span>,
}

impl Replay<'_, '_> {
    /// The value at `read` of the `let` that `declarator` declares.
    fn at(&mut self, declarator: NodeId, read: NodeId) -> Option<Value> {
        let (tree, scopes) = (&self.parsed.tree, &self.parsed.scopes);
        let references = &scopes.symbol(self.symbol).references;
        if scopes.reads_global_eval() || references.len() > MAX_LET_REFERENCES {
            return None;
        }
        let declaration = tree.parent(declarator)?;
        let body = tree.parent(declaration)?;
        if !matches!(tree.kind(body), "BlockStatement" | "Program") {
            return None;
        }
        // The body's statement holding the read, reached without entering code that may run at
        // another time, other than a deferred suite callback, or resolve names another way.
        let (mut statement, mut deferred) = (read, false);
        loop {
            let up = tree.parent(statement)?;
            if up == body {
                break;
            }
            if tree.is_function(up)
                && self
                    .reading
                    .deferral
                    .is_some_and(|deferral| deferral.suites.contains_key(&up))
            {
                deferred = true;
            } else if tree.is_function(up)
                || matches!(
                    tree.kind(up),
                    "ClassDeclaration" | "ClassExpression" | "WithStatement"
                )
            {
                return None;
            }
            statement = up;
        }
        let from = tree.span(declaration).end;
        let to = if deferred {
            let ends = tree.kind(body) == "Program"
                || tree.parent(body).is_some_and(|function| {
                    tree.is_function(function)
                        && !tree.flag(function, "async")
                        && !tree.flag(function, "generator")
                        && self
                            .reading
                            .deferral
                            .is_some_and(|deferral| !deferral.returning.contains(&function))
                });
            if !ends {
                return None;
            }
            tree.span(body).end
        } else {
            tree.span(statement).start
        };
        self.writes = references
            .iter()
            .filter(|reference| reference.write)
            .map(|reference| reference.span)
            .collect();
        if from > to
            || self
                .writes
                .iter()
                .any(|write| write.start < from || write.end > to)
        {
            return None;
        }
        // The body's statements holding a write, in order: found from each write, so a long body
        // costs no more than a short one.
        let mut statements: Vec<NodeId> = Vec::new();
        for reference in references.iter().filter(|reference| reference.write) {
            let mut statement = reference.node?;
            while tree.parent(statement)? != body {
                statement = tree.parent(statement)?;
            }
            if statements.last() != Some(&statement) {
                statements.push(statement);
            }
        }
        // Every read copies what the `let` supplies, as for a `const`; a refused value spends
        // nothing.
        let (spelled, left) = (
            std::mem::replace(&mut self.limit.copied, true),
            self.limit.left,
        );
        let value = self.replay(declarator, &statements);
        self.limit.copied = spelled;
        if value.is_none() {
            self.limit.left = left;
        }
        value
    }

    /// The initializer, then every statement of the body that writes the symbol.
    fn replay(&mut self, declarator: NodeId, statements: &[NodeId]) -> Option<Value> {
        self.current = Some(self.value(self.parsed.tree.child(declarator, "init")?)?);
        for &statement in statements {
            self.statement(statement)?;
        }
        self.current.take()
    }

    /// Runs one statement's writes; a statement holding none is skipped.
    fn statement(&mut self, node: NodeId) -> Option<()> {
        let tree = &self.parsed.tree;
        if !self.holds_write(node) {
            return Some(());
        }
        *self.budget = self.budget.checked_sub(1)?;
        match tree.kind(node) {
            "ExpressionStatement" => {
                let assignment = tree.inner(tree.child(node, "expression")?);
                let target = tree.inner(tree.child(assignment, "left")?);
                let right = tree.child(assignment, "right")?;
                if tree.kind(assignment) != "AssignmentExpression"
                    || tree.text(assignment, "operator") != Some("=")
                    || tree.kind(target) != "Identifier"
                    || self.parsed.scopes.symbol_at(tree.span(target).start) != Some(self.symbol)
                {
                    return None;
                }
                self.current = Some(self.value(right)?);
            }
            "BlockStatement" => {
                for statement in tree.list(node, "body") {
                    self.statement(statement?)?;
                }
            }
            "IfStatement" => {
                // A condition is read, never run, so it cannot assign the symbol.
                let test = tree.child(node, "test")?;
                let branch = if self.condition(test)? {
                    tree.child(node, "consequent")
                } else {
                    tree.child(node, "alternate")
                };
                if let Some(branch) = branch {
                    self.statement(branch)?;
                }
            }
            _ => return None,
        }
        Some(())
    }

    /// `===` or `!==` between proved primitives.
    fn condition(&mut self, test: NodeId) -> Option<bool> {
        let tree = &self.parsed.tree;
        let test = tree.inner(test);
        if tree.kind(test) != "BinaryExpression" {
            return None;
        }
        let equal = match tree.text(test, "operator")? {
            "===" => true,
            "!==" => false,
            _ => return None,
        };
        let left = self.value(tree.child(test, "left")?)?;
        let right = self.value(tree.child(test, "right")?)?;
        let same = match (&left, &right) {
            (Value::Number(left), Value::Number(right)) => left.as_f64()? == right.as_f64()?,
            (left, right) => left == right,
        };
        Some(same == equal)
    }

    /// A primitive read at the replay point; a composite may be mutated in place.
    fn value(&mut self, node: NodeId) -> Option<Value> {
        let current = self.current.clone();
        let reading = Reading {
            current: current
                .as_ref()
                .map(|value| (self.symbol, self.parsed.tree.span(node), value)),
            ..self.reading
        };
        read(
            self.parsed,
            node,
            self.depth + 1,
            self.budget,
            reading,
            self.limit,
        )
        .filter(|value| !value.is_object() && !value.is_array())
    }

    /// Whether the symbol is assigned inside `node`.
    fn holds_write(&self, node: NodeId) -> bool {
        let span = self.parsed.tree.span(node);
        self.writes
            .iter()
            .any(|write| span.start <= write.start && write.end <= span.end)
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
