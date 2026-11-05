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
    /// An object row assembled from a proven tagged-table header and interpolation cells.
    Fields(Vec<(String, NodeId)>),
}

/// A present table element and its index in the written array, counting outer elisions.
#[derive(Clone, Debug)]
pub struct TableRow {
    /// The array index used by title formatting and row ordering.
    pub index: usize,
    /// The callback inputs represented by this element.
    pub value: Row,
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
    pub rows: Option<Vec<TableRow>>,
    /// Whether those rows reach the callback whole.
    pub whole_rows: bool,
    /// Whether the chain keeps the test from running (`skip`, `todo`), the way a report marks a
    /// participant inert.
    pub inert: bool,
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
pub(super) const OBJECT_PROTOTYPE: [&str; 12] = [
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
    /// The modifiers of a registration whose callee is `callee`, which registers through the
    /// test-function member `member` when the inventory recorded one ([`crate::inventory::Site::member`]).
    pub fn modifiers(&mut self, callee: NodeId, member: Option<NodeId>) -> Modifiers {
        let tree = self.tree;
        let dialect = self.setting.dialect;
        let mut modifiers = Modifiers::default();
        let mut steps = Vec::new();
        // Where the table's `(rows)` and member sit in `steps`, which is built innermost last.
        let mut table_at = None;
        let mut current = tree.inner(callee);
        loop {
            // The member is the API function it names; what it was read from does not apply.
            if member == Some(current) {
                let name = tree
                    .child(current, "property")
                    .and_then(|property| tree.text(property, "name"))
                    .unwrap_or_default();
                let canonical = dialect
                    .registration(name)
                    .map_or(name, |registration| registration.canonical);
                steps.push(format!("api:{canonical}"));
                break;
            }
            match tree.kind(current) {
                "MemberExpression" if !tree.flag(current, "computed") => {
                    let name = tree
                        .child(current, "property")
                        .and_then(|property| tree.text(property, "name"))
                        .unwrap_or_default();
                    match dialect.chain_member(name) {
                        Some(Chain::ExpectedFailure) => modifiers.fails = true,
                        // Recorded, not keyed: `skip`/`todo` change nothing the test checks, but
                        // a report marks a participant that does not run.
                        Some(Chain::Skip | Chain::Todo) => modifiers.inert = true,
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
                            if tree.kind(tree.inner(*table)) == "ArrayExpression" =>
                        {
                            let member = tree
                                .child(callee, "property")
                                .and_then(|property| tree.text(property, "name"))
                                .and_then(|name| dialect.chain_member(name));
                            match member {
                                Some(Chain::Table(form)) => self
                                    .table_rows(tree.inner(*table), form)
                                    .map(|rows| (rows, form == TableRows::Whole)),
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    match table {
                        Some((rows, whole_rows)) => {
                            modifiers.rows = Some(rows);
                            modifiers.whole_rows = whole_rows;
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
                    if let Some(rows) = self.tagged_rows(current) {
                        modifiers.rows = Some(rows);
                        modifiers.whole_rows = true;
                        steps.push("(rows)".to_owned());
                        table_at = Some(steps.len());
                        current = tree.inner(
                            tree.child(current, "tag")
                                .expect("a proved tag has a callee"),
                        );
                        continue;
                    }
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
                        let enabled = self.enabled(property);
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

    /// Whether option `property` is a literal `true`.
    fn enabled(&self, property: NodeId) -> bool {
        let tree = self.tree;
        tree.child(property, "value").is_some_and(|value| {
            tree.kind(value) == "Literal"
                && matches!(
                    tree.field_of(value, "value"),
                    crate::lang::Field::Value(Value::Bool(true))
                )
        })
    }

    /// Whether an options object among `arguments` keeps the test from running, the way
    /// [`Modifiers::inert`] records it for a chain (`{ skip: true }`, `{ todo: true }`).
    pub fn inert_options(&self, arguments: &[NodeId]) -> bool {
        let tree = self.tree;
        let dialect = self.setting.dialect;
        arguments
            .iter()
            .skip(1)
            .filter(|&&argument| tree.kind(argument) == "ObjectExpression")
            .flat_map(|&argument| tree.list(argument, "properties").into_iter().flatten())
            .any(|property| {
                let name = (tree.kind(property) == "Property")
                    .then(|| property_name(tree, property))
                    .flatten();
                matches!(
                    name.and_then(|name| dialect.chain_member(name)),
                    Some(Chain::Skip | Chain::Todo)
                ) && self.enabled(property)
            })
    }

    /// The rows of a table literal: a spreading table spreads array rows when every row is one,
    /// a whole-row table never does. Outer holes register nothing; spreads remain unreadable.
    fn table_rows(&self, table: NodeId, form: TableRows) -> Option<Vec<TableRow>> {
        let tree = self.tree;
        let rows: Vec<(usize, NodeId)> = tree
            .list(table, "elements")
            .into_iter()
            .enumerate()
            .filter_map(|(index, row)| row.map(|row| (index, row)))
            .map(|(index, row)| (tree.kind(row) != "SpreadElement").then_some((index, row)))
            .collect::<Option<_>>()?;
        let spread = form == TableRows::Spread
            && rows
                .iter()
                .all(|(_, row)| tree.kind(tree.inner(*row)) == "ArrayExpression");
        rows.into_iter()
            .map(|(index, row)| {
                let inner = tree.inner(row);
                let value = match tree.kind(inner) {
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
                }?;
                Some(TableRow { index, value })
            })
            .collect()
    }

    /// Reads only framework tagged tables whose rows are interpolation-only and rectangular.
    fn tagged_rows(&self, tagged: NodeId) -> Option<Vec<TableRow>> {
        let tree = self.tree;
        let tag = tree.inner(tree.child(tagged, "tag")?);
        if tree.kind(tag) != "MemberExpression" || tree.flag(tag, "computed") {
            return None;
        }
        let member = tree.text(tree.child(tag, "property")?, "name")?;
        if !matches!(
            self.setting.dialect.chain_member(member),
            Some(Chain::Table(_))
        ) {
            return None;
        }
        let quasi = tree.child(tagged, "quasi")?;
        let quasis = tree.list(quasi, "quasis");
        let expressions = tree.list(quasi, "expressions");
        let text = |node| {
            let crate::lang::Field::Object(fields) = tree.field_of(node, "value") else {
                return None;
            };
            fields
                .iter()
                .find_map(|(key, field)| match (&**key, field) {
                    ("cooked", crate::lang::Field::Value(Value::String(text))) => {
                        Some(text.as_str())
                    }
                    _ => None,
                })
        };
        let first = text(quasis.first().copied()??)?;
        if expressions.is_empty() {
            return (!first.contains('\u{fffd}') && first.len() <= 65_536).then(|| {
                vec![TableRow {
                    index: 0,
                    value: Row::Whole(quasi, None),
                }]
            });
        }
        // Vitest strips ASCII spaces only; other whitespace can be part of the actual key.
        if first
            .rsplit_once('\n')
            .is_none_or(|(_, cells)| !cells.trim().is_empty())
            || first
                .chars()
                .any(|c| c.is_whitespace() && !matches!(c, ' ' | '\n'))
        {
            return None;
        }
        let lines = first
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect::<Vec<_>>();
        if lines.len() != 1 {
            return None;
        }
        let headers = lines[0].split('|').map(str::trim).collect::<Vec<_>>();
        if headers.is_empty()
            || headers.iter().any(|h| {
                h.is_empty()
                    || !h
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
                    || matches!(*h, "__proto__" | "constructor" | "toString" | "valueOf")
            })
            || headers
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != headers.len()
            || !expressions.len().is_multiple_of(headers.len())
            || quasis.len() != expressions.len() + 1
        {
            return None;
        }
        let mut rows = Vec::new();
        for (index, cells) in expressions.chunks(headers.len()).enumerate() {
            let mut fields = Vec::new();
            for (column, cell) in cells.iter().enumerate() {
                let cell = (*cell)?;
                let separator = text(quasis[index * headers.len() + column + 1]?)?;
                let last = column + 1 == headers.len();
                if (!last && separator.trim() != "|") || (last && !separator.trim().is_empty()) {
                    return None;
                }
                if last
                    && index * headers.len() + column + 1 < expressions.len()
                    && !separator.contains('\n')
                {
                    return None;
                }
                fields.push((headers[column].to_owned(), cell));
            }
            rows.push(TableRow {
                index,
                value: Row::Fields(fields),
            });
        }
        Some(rows)
    }

    /// Binds a callback's parameters to one row's cells, with whether every parameter is bound;
    /// `None` when a parameter's shape is not one this reads (a default, a rest element, a nested
    /// pattern) or a cell cannot be written in where it is read.
    pub fn bind_row(
        &mut self,
        parameters: &[NodeId],
        row: &Row,
    ) -> Option<(HashMap<SymbolId, String>, bool)> {
        self.bind_row_mode(parameters, row, false)
    }

    /// The same binding proof as [`Self::bind_row`], with cells rendered by literal kind.
    pub fn bind_row_shape(
        &mut self,
        parameters: &[NodeId],
        row: &Row,
    ) -> Option<(HashMap<SymbolId, String>, bool)> {
        self.bind_row_mode(parameters, row, true)
    }

    /// Binds cells using one proof for both exact and structural forms.
    fn bind_row_mode(
        &mut self,
        parameters: &[NodeId],
        row: &Row,
        structural: bool,
    ) -> Option<(HashMap<SymbolId, String>, bool)> {
        if self.structure.direct_eval {
            return None;
        }
        let tree = self.tree;
        let undefined = || "g:undefined".to_owned();
        let mut bound = HashMap::new();
        match row {
            Row::Fields(properties) => {
                if properties
                    .iter()
                    .any(|(_, cell)| self.row_scalar(*cell).is_none())
                {
                    return None;
                }
                let Some(&parameter) = parameters.first() else {
                    return Some((bound, true));
                };
                if tree.kind(parameter) != "ObjectPattern" {
                    return None;
                }
                for property in tree.list(parameter, "properties").into_iter().flatten() {
                    if tree.kind(property) != "Property" || tree.flag(property, "computed") {
                        return None;
                    }
                    let binding = tree.child(property, "value")?;
                    if tree.kind(binding) != "Identifier" {
                        return None;
                    }
                    let key = property_name(tree, property)?;
                    let symbol = self.unwritten_parameter(binding)?;
                    let value = match properties.iter().find(|(name, _)| name == key) {
                        Some((_, node)) => self.cell(symbol, *node, structural)?,
                        None if OBJECT_PROTOTYPE.contains(&key) => return None,
                        None => undefined(),
                    };
                    bound.insert(symbol, value);
                }
            }
            Row::Spread(cells) => {
                for (index, &parameter) in parameters.iter().enumerate() {
                    if tree.kind(parameter) != "Identifier" {
                        return None;
                    }
                    let symbol = self.unwritten_parameter(parameter)?;
                    let value = match cells.get(index) {
                        Some(&cell) => self.cell(symbol, cell, structural)?,
                        None => undefined(),
                    };
                    bound.insert(symbol, value);
                }
            }
            Row::Whole(value, properties) => match parameters.first().copied() {
                None => {}
                Some(parameter) if tree.kind(parameter) == "Identifier" => {
                    let symbol = self.unwritten_parameter(parameter)?;
                    let cell = self.cell(symbol, *value, structural)?;
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
                        let symbol = self.unwritten_parameter(binding)?;
                        let value = match properties.iter().rev().find(|(name, _)| *name == key) {
                            Some((_, cell)) => self.cell(symbol, *cell, structural)?,
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

    /// The parameter's symbol when its invocation value remains stable for every read.
    fn unwritten_parameter(&self, parameter: NodeId) -> Option<SymbolId> {
        let symbol = self.scopes.symbol_at(self.tree.span(parameter).start)?;
        self.scopes
            .symbol(symbol)
            .references
            .iter()
            .all(|reference| !reference.write)
            .then_some(symbol)
    }

    /// A cell's normal form to write in for `symbol`: a cell that creates or allocates an object
    /// is evaluated once per row, so written in at every read it would be one object per read.
    ///
    /// A cell that reads a binding something reassigns is not written in either: the row holds the
    /// value it had when the test registered, which the binding may no longer hold when it runs.
    fn cell(&mut self, symbol: SymbolId, cell: NodeId, structural: bool) -> Option<String> {
        if self.row_callback_uses_receiver(symbol) {
            return None;
        }
        if let Some(value) = self.row_scalar(cell) {
            return self.scalar_form(&value, structural);
        }
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
        if fresh && reads > 1 && !self.readonly_row_data(symbol, cell) {
            return None;
        }
        if structural
            && matches!(
                tree.kind(tree.inner(cell)),
                "ObjectExpression" | "ArrayExpression"
            )
            && self.literal_row_data(cell, &HashMap::new())
        {
            return self.row_data_shape(cell, &HashMap::new());
        }
        Some(self.render(cell, HashMap::new(), false, structural))
    }
}
