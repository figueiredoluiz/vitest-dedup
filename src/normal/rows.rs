//! Chain members, options and table rows: what of a registration besides its callback changes what
//! a test checks.

use super::{form, Normalizer};
use crate::framework::{Chain, TableRows};
use crate::lang::{property_name, NodeId, SymbolId};
use serde_json::Value;
use std::collections::HashMap;

/// One row of a table the file spells out.
#[derive(Clone, Debug)]
pub enum Row {
    /// Cells spread into the callback's parameters.
    Spread(Vec<NodeId>),
    /// One value passed whole, with its properties when it is an object literal that spells them
    /// all statically.
    Whole(NodeId, Option<Vec<(String, NodeId)>>),
}

/// What a registration's callee chain says beyond scheduling.
#[derive(Clone, Debug, Default)]
pub struct Modifiers {
    /// The chain with scheduling members dropped: root identity, members that change what is
    /// checked, and the normal form of their arguments (tables as `(rows)`).
    pub chain: String,
    /// [`Modifiers::chain`] without the table member and its rows: what a row whose every
    /// parameter is bound is, written out as its own test.
    pub untabled: String,
    /// Whether the outcome is inverted (an expected failure), kept apart so the options form can
    /// set it too.
    pub fails: bool,
    /// The rows of a table the chain carries, when the file spells it out.
    pub rows: Option<Vec<Row>>,
}

/// Whether a chain member changes only how a test is scheduled.
fn scheduling(meaning: Option<Chain>) -> bool {
    matches!(
        meaning,
        Some(
            Chain::Skip
                | Chain::Focus
                | Chain::Todo
                | Chain::Concurrent
                | Chain::Sequential
                | Chain::Shuffle
        )
    )
}

/// What `Object.prototype` holds, which a destructured key an object literal lacks reads.
const OBJECT_PROTOTYPE: [&str; 12] = [
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
    "__proto__",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
];

impl Normalizer<'_> {
    /// The modifiers of a registration whose callee is `callee`.
    pub fn modifiers(&mut self, callee: NodeId) -> Modifiers {
        let tree = self.tree;
        let dialect = self.setting.dialect;
        let mut modifiers = Modifiers::default();
        let mut steps = Vec::new();
        // Where the table's `(rows)` and member sit in `steps`, which is built innermost last.
        let mut table_at = None;
        let mut current = tree.inner(callee);
        loop {
            match tree.kind(current) {
                "MemberExpression" if !tree.flag(current, "computed") => {
                    let name = tree
                        .child(current, "property")
                        .and_then(|property| tree.text(property, "name"))
                        .unwrap_or_default();
                    match dialect.chain_member(name) {
                        Some(Chain::ExpectedFailure) => modifiers.fails = true,
                        meaning if scheduling(meaning) => {}
                        _ => steps.push(name.to_owned()),
                    }
                    let Some(object) = tree.child(current, "object") else {
                        break;
                    };
                    current = tree.inner(object);
                }
                "CallExpression" => {
                    let arguments = tree.list(current, "arguments");
                    let Some(callee) = tree.child(current, "callee") else {
                        break;
                    };
                    let callee = tree.inner(callee);
                    let table = match (tree.kind(callee), arguments.as_slice()) {
                        ("MemberExpression", [Some(table)])
                            if tree.kind(*table) == "ArrayExpression" =>
                        {
                            let member = tree
                                .child(callee, "property")
                                .and_then(|property| tree.text(property, "name"))
                                .and_then(|name| dialect.chain_member(name));
                            match member {
                                Some(Chain::Table(rows)) => self.table_rows(*table, rows),
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    match table {
                        Some(rows) => {
                            modifiers.rows = Some(rows);
                            steps.push("(rows)".to_owned());
                            table_at = Some(steps.len());
                        }
                        None => {
                            let arguments: Vec<String> = arguments
                                .into_iter()
                                .flatten()
                                .map(|argument| self.of(argument))
                                .collect();
                            steps.push(format!("({})", arguments.join(",")));
                        }
                    }
                    current = callee;
                }
                "TaggedTemplateExpression" => {
                    let quasi = tree.child(current, "quasi").map(|quasi| self.of(quasi));
                    steps.push(format!("`{}`", quasi.unwrap_or_default()));
                    let Some(tag) = tree.child(current, "tag") else {
                        break;
                    };
                    current = tree.inner(tag);
                }
                _ => {
                    let root = self.of(current);
                    // A global registration name is the API it names.
                    let api = root
                        .strip_prefix("g:")
                        .and_then(|name| dialect.registration(name))
                        .map(|registration| format!("api:{}", registration.canonical));
                    steps.push(api.unwrap_or(root));
                    break;
                }
            }
        }
        let untabled: Vec<&String> = steps
            .iter()
            .enumerate()
            .filter(|(index, _)| table_at.is_none_or(|at| *index + 1 != at && *index != at))
            .map(|(_, step)| step)
            .rev()
            .collect();
        modifiers.untabled = untabled
            .iter()
            .map(|step| step.as_str())
            .collect::<Vec<_>>()
            .join(".");
        steps.reverse();
        modifiers.chain = steps.join(".");
        modifiers
    }

    /// The options a registration passes besides its title and callback that change what it
    /// checks, and whether one of them inverts the outcome.
    pub fn options(&mut self, arguments: &[NodeId], body: NodeId) -> (Vec<String>, bool) {
        let tree = self.tree;
        let dialect = self.setting.dialect;
        let mut options = Vec::new();
        let mut fails = false;
        for &argument in arguments
            .iter()
            .skip(1)
            .filter(|&&argument| argument != body)
        {
            match tree.kind(argument) {
                // A timeout changes nothing a passing test checks.
                "Literal"
                    if matches!(
                        tree.field_of(argument, "value"),
                        crate::lang::Field::Value(Value::Number(_))
                    ) => {}
                "ObjectExpression" => {
                    for property in tree.list(argument, "properties").into_iter().flatten() {
                        let name = (tree.kind(property) == "Property")
                            .then(|| property_name(tree, property))
                            .flatten();
                        let meaning = name.and_then(|name| dialect.chain_member(name));
                        let enabled = tree.child(property, "value").is_some_and(|value| {
                            tree.kind(value) == "Literal"
                                && matches!(
                                    tree.field_of(value, "value"),
                                    crate::lang::Field::Value(Value::Bool(true))
                                )
                        });
                        let scheduled = scheduling(meaning)
                            || name.is_some_and(|name| dialect.scheduling_options.contains(&name));
                        if meaning == Some(Chain::ExpectedFailure) && enabled {
                            fails = true;
                        } else if !scheduled {
                            options.push(self.of(property));
                        }
                    }
                }
                _ => options.push(self.of(argument)),
            }
        }
        (options, fails)
    }

    /// The rows of a table literal: a spreading table spreads array rows when every row is one,
    /// a whole-row table never does. `None` when a row is a hole or a spread.
    fn table_rows(&self, table: NodeId, form: TableRows) -> Option<Vec<Row>> {
        let tree = self.tree;
        let rows: Vec<NodeId> = tree
            .list(table, "elements")
            .into_iter()
            .map(|row| row.filter(|row| tree.kind(*row) != "SpreadElement"))
            .collect::<Option<_>>()?;
        let spread = form == TableRows::Spread
            && rows
                .iter()
                .all(|row| tree.kind(tree.inner(*row)) == "ArrayExpression");
        rows.into_iter()
            .map(|row| {
                let inner = tree.inner(row);
                match tree.kind(inner) {
                    "ArrayExpression" if spread => tree
                        .list(inner, "elements")
                        .into_iter()
                        .map(|cell| cell.filter(|cell| tree.kind(*cell) != "SpreadElement"))
                        .collect::<Option<_>>()
                        .map(Row::Spread),
                    "ObjectExpression" => {
                        let properties = tree
                            .list(inner, "properties")
                            .into_iter()
                            .map(|property| {
                                let property = property?;
                                let plain = tree.kind(property) == "Property"
                                    && !tree.flag(property, "computed")
                                    && !tree.flag(property, "method")
                                    && tree.text(property, "kind") == Some("init");
                                // `__proto__:` sets the prototype a missing key is read from.
                                let name = property_name(tree, property)?;
                                if !plain || name == "__proto__" {
                                    return None;
                                }
                                Some((name.to_owned(), tree.child(property, "value")?))
                            })
                            .collect::<Option<_>>();
                        Some(Row::Whole(row, properties))
                    }
                    _ => Some(Row::Whole(row, None)),
                }
            })
            .collect()
    }

    /// Binds a callback's parameters to one row's cells, with whether every parameter is bound;
    /// `None` when a parameter's shape is not one this reads (a default, a rest element, a nested
    /// pattern) or a cell cannot be written in where it is read.
    pub fn bind_row(
        &mut self,
        parameters: &[NodeId],
        row: &Row,
    ) -> Option<(HashMap<SymbolId, String>, bool)> {
        let tree = self.tree;
        let undefined = || "g:undefined".to_owned();
        let mut bound = HashMap::new();
        match row {
            Row::Spread(cells) => {
                for (index, &parameter) in parameters.iter().enumerate() {
                    if tree.kind(parameter) != "Identifier" {
                        return None;
                    }
                    let symbol = self.scopes.symbol_at(tree.span(parameter).start)?;
                    let value = match cells.get(index) {
                        Some(&cell) => self.cell(symbol, cell)?,
                        None => undefined(),
                    };
                    bound.insert(symbol, value);
                }
            }
            Row::Whole(value, properties) => match parameters.first().copied() {
                None => {}
                Some(parameter) if tree.kind(parameter) == "Identifier" => {
                    let symbol = self.scopes.symbol_at(tree.span(parameter).start)?;
                    let cell = self.cell(symbol, *value)?;
                    bound.insert(symbol, cell);
                }
                Some(parameter) if tree.kind(parameter) == "ObjectPattern" => {
                    let properties = properties.as_ref()?;
                    for property in tree.list(parameter, "properties") {
                        let property = property?;
                        if tree.kind(property) != "Property" {
                            return None;
                        }
                        let binding = tree.child(property, "value")?;
                        if tree.kind(binding) != "Identifier" {
                            return None;
                        }
                        let key = property_name(tree, property)?.to_owned();
                        let symbol = self.scopes.symbol_at(tree.span(binding).start)?;
                        let value = match properties.iter().rev().find(|(name, _)| *name == key) {
                            Some((_, cell)) => self.cell(symbol, *cell)?,
                            // A key the object lacks is read from `Object.prototype`.
                            None if OBJECT_PROTOTYPE.contains(&key.as_str()) => return None,
                            None => undefined(),
                        };
                        bound.insert(symbol, value);
                    }
                }
                Some(_) => return None,
            },
        }
        let every = matches!(row, Row::Spread(_)) || parameters.len() <= 1;
        Some((bound, every))
    }

    /// A cell's normal form to write in for `symbol`: a cell that creates or allocates an object
    /// is evaluated once per row, so written in at every read it would be one object per read.
    ///
    /// A cell that reads a binding something reassigns is not written in either: the row holds the
    /// value it had when the test registered, which the binding may no longer hold when it runs.
    fn cell(&mut self, symbol: SymbolId, cell: NodeId) -> Option<String> {
        let tree = self.tree;
        let reassigned = std::iter::once(cell)
            .chain(tree.descendants(cell))
            .filter(|&node| tree.kind(node) == "Identifier")
            .filter_map(|node| self.scopes.symbol_at(tree.span(node).start))
            .any(|read| {
                self.scopes
                    .symbol(read)
                    .references
                    .iter()
                    .any(|reference| reference.write)
            });
        if reassigned {
            return None;
        }
        let reads = self.scopes.symbol(symbol).references.len();
        let fresh = form::creates(self, cell) || form::allocates(self.tree, cell);
        (!fresh || reads <= 1).then(|| self.of(cell))
    }
}
