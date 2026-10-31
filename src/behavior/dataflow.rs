//! The dataflow normalizations only the trace reads.
//!
//! Three canonicalizations and one naming tag, each a bounded port of the current core's
//! `dataflow.rs` onto the owned tree. All stay out of the flat normal form on purpose:
//! `duplicate-test` reports at Error as an established claim, and each precondition here is
//! necessary but not sufficient for equivalence — which is why they serve only the
//! `similar-uncertain` rules the trace feeds.
//!
//! - **Single-use local inlining**: `const x = f(); g(x)` renders `x`'s one read as `f()` itself.
//!   Gated on a `const` with a single declarator, declared and read in the body's own top-level
//!   statements, read exactly once, and no effectful statement between the declaration and that
//!   read. A read under a branch, a loop, a `try` or a nested closure is never inlined, and an
//!   effectful initializer is not absorbed through another inlined declaration.
//! - **Destructuring to member access**: `const { a } = o; use(a)` renders `o.a`. Gated on `o`
//!   being a plain identifier resolving to a symbol outside the body (never a body-local, whose
//!   number depends on the traversal order this substitution feeds), a plain non-computed key
//!   with no default, and no effectful statement between the declaration and the last read.
//!   Multiple reads are allowed here: duplicating a property *read* has no evaluation-count
//!   hazard.
//! - **Destructured binding names**: every local the body declares through an object-destructuring
//!   pattern renders with a `:prop:` suffix carrying the whole property path, `:`-joined. A suffix
//!   can only tell two events apart, never make them equal, so it carries none of the
//!   preconditions above and is collected over the whole body, nested closures included.

use crate::lang::{NodeId, Parsed, Scopes, Span, SymbolId, Tree};
use std::collections::HashMap;

/// How deep the effect and read scans recurse. Past it a statement degrades toward "has an
/// effect" and a scan toward "unsafe", which blocks a canonicalization instead of claiming one.
const MAX_SCAN_DEPTH: u32 = 256;

/// The dataflow substitutions one body's trace reads, computed once per body.
#[derive(Debug, Default)]
pub(super) struct Dataflow {
    /// A local whose one read stands for its own initializer, by symbol.
    pub inline: HashMap<SymbolId, NodeId>,
    /// A destructured local whose reads stand for `object.property`, by symbol.
    pub member: HashMap<SymbolId, (NodeId, String)>,
    /// The `:prop:` suffix a destructured local's rendering carries, by symbol.
    pub names: HashMap<SymbolId, String>,
}

impl Dataflow {
    /// Whether the declarator of an `Identifier` binding records no event of its own because its
    /// one read renders the initializer: the read site owes the initializer's full accounting.
    pub fn inlined(&self, symbol: Option<SymbolId>) -> bool {
        symbol.is_some_and(|symbol| self.inline.contains_key(&symbol))
    }

    /// The initializer a read of `symbol` stands for, when inlined.
    pub fn init_of(&self, symbol: Option<SymbolId>) -> Option<NodeId> {
        symbol.and_then(|symbol| self.inline.get(&symbol)).copied()
    }
}

/// Computes one body's [`Dataflow`]. The two canonicalizations scan only the body's own top-level
/// statement list — an arrow's expression body or a bodyless function registers none — while the
/// name suffixes are collected over the whole body, because they carry no precondition.
pub(super) fn analyze(parsed: &Parsed, body: NodeId) -> Dataflow {
    let tree = &parsed.tree;
    let scopes = &parsed.scopes;
    let mut dataflow = Dataflow::default();
    let mut names: Vec<(SymbolId, String)> = Vec::new();
    for node in tree.descendants(body) {
        if tree.kind(node) == "VariableDeclarator" {
            if let Some(pattern) = tree.child(node, "id") {
                collect_pattern(tree, scopes, pattern, "", 0, &mut names);
            }
        }
    }
    let Some(body_list) = top_level_statements(tree, body) else {
        dataflow.names = names
            .into_iter()
            .map(|(symbol, path)| (symbol, format!(":prop:{path}")))
            .collect();
        return dataflow;
    };
    for (index, statement) in body_list.iter().enumerate() {
        if tree.kind(*statement) != "VariableDeclaration"
            || tree.text(*statement, "kind") != Some("const")
        {
            continue;
        }
        let declarations = tree.list(*statement, "declarations");
        let [Some(declarator)] = declarations.as_slice() else {
            continue;
        };
        let Some(init) = tree.child(*declarator, "init") else {
            continue;
        };
        let Some(id) = tree.child(*declarator, "id") else {
            continue;
        };
        match tree.kind(id) {
            "Identifier" => {
                try_register_inline(
                    &Scan {
                        tree,
                        scopes,
                        statements: &body_list,
                        root: tree.span(body),
                    },
                    index,
                    id,
                    init,
                    &mut dataflow,
                );
            }
            "ObjectPattern" => try_register_members(
                &Scan {
                    tree,
                    scopes,
                    statements: &body_list,
                    root: tree.span(body),
                },
                index,
                id,
                init,
                &mut dataflow,
            ),
            _ => {}
        }
    }
    // A symbol a canonicalization claimed keeps that claim; the name suffix fills only the rest.
    for (symbol, path) in names {
        dataflow
            .names
            .entry(symbol)
            .or_insert_with(|| format!(":prop:{path}"));
    }
    dataflow.names.retain(|symbol, _| {
        !dataflow.inline.contains_key(symbol) && !dataflow.member.contains_key(symbol)
    });
    dataflow
}

/// The body's own top-level statements, `None` for a concise arrow body or a bodyless function.
fn top_level_statements(tree: &Tree, body: NodeId) -> Option<Vec<NodeId>> {
    let block = tree.child(body, "body")?;
    (tree.kind(block) == "BlockStatement")
        .then(|| tree.list(block, "body").into_iter().flatten().collect())
}

/// Collects the `(symbol, property path)` pairs a binding pattern declares. `path` is the
/// `:`-joined route of property keys walked through to reach this sub-pattern; a binding reached
/// with an empty path was not destructured out of anything and contributes nothing. Only keys
/// written as identifiers are retained: a quoted key is author-controlled text that could spell
/// the event grammar's own punctuation.
fn collect_pattern(
    tree: &Tree,
    scopes: &Scopes,
    pattern: NodeId,
    path: &str,
    depth: u32,
    names: &mut Vec<(SymbolId, String)>,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    match tree.kind(pattern) {
        "Identifier" => {
            if path.is_empty() {
                return;
            }
            if let Some(symbol) = scopes.symbol_at(tree.span(pattern).start) {
                names.push((symbol, path.to_owned()));
            }
        }
        "ObjectPattern" => {
            for property in tree.list(pattern, "properties").into_iter().flatten() {
                if tree.kind(property) != "Property" || tree.flag(property, "computed") {
                    continue;
                }
                let Some(key) = tree.child(property, "key") else {
                    continue;
                };
                if tree.kind(key) != "Identifier" {
                    continue;
                }
                let Some(name) = tree.text(key, "name") else {
                    continue;
                };
                let child = if path.is_empty() {
                    name.to_owned()
                } else {
                    format!("{path}:{name}")
                };
                if let Some(value) = tree.child(property, "value") {
                    collect_pattern(tree, scopes, value, &child, depth + 1, names);
                }
            }
        }
        "ArrayPattern" => {
            // An element contributes position, not a name, so it inherits the enclosing path.
            for element in tree.list(pattern, "elements").into_iter().flatten() {
                collect_pattern(tree, scopes, element, path, depth + 1, names);
            }
        }
        "AssignmentPattern" => {
            if let Some(left) = tree.child(pattern, "left") {
                collect_pattern(tree, scopes, left, path, depth + 1, names);
            }
        }
        _ => {}
    }
}

/// What the top-level scans read: the tree, its scopes, the body's flat statement list, and the
/// body's root span.
struct Scan<'a> {
    tree: &'a Tree,
    scopes: &'a Scopes,
    statements: &'a [NodeId],
    root: Span,
}

/// Registers a single-use inlining candidate for one `const x = …` declarator, if its one
/// safely-reasoned-about read qualifies.
fn try_register_inline(
    scan: &Scan,
    decl_index: usize,
    id: NodeId,
    init: NodeId,
    dataflow: &mut Dataflow,
) {
    let tree = scan.tree;
    let Some(symbol) = scan.scopes.symbol_at(tree.span(id).start) else {
        return;
    };
    let Some(reads) = read_statement_indices(tree, scan.scopes, scan.statements, symbol) else {
        return; // a read under control flow or in a closure: never inline.
    };
    // A local read twice may exist precisely so an effectful initializer runs once; the trace
    // never assumes purity, so it never duplicates a twice-read initializer.
    let [use_index] = reads.as_slice() else {
        return;
    };
    if *use_index <= decl_index {
        return;
    }
    if scan.statements[decl_index + 1..*use_index]
        .iter()
        .any(|statement| statement_has_effect(tree, *statement, 0))
    {
        return;
    }
    dataflow.inline.insert(symbol, init);
}

/// Registers destructuring candidates for one `const { … } = o` declarator: the name tag for
/// every plain-identifier property, and the member-access rewrite for those that also qualify.
/// Only the last read's statement is checked for an intervening effect: a statement's effect,
/// once present before some read, is present before every later one too.
fn try_register_members(
    scan: &Scan,
    decl_index: usize,
    pattern: NodeId,
    init: NodeId,
    dataflow: &mut Dataflow,
) {
    let tree = scan.tree;
    let scopes = scan.scopes;
    let can_rewrite_object = tree.kind(init) == "Identifier"
        && scopes
            .symbol_at(tree.span(init).start)
            .is_some_and(|symbol| !scan.root.contains(scopes.symbol(symbol).span));
    for property in tree.list(pattern, "properties").into_iter().flatten() {
        if tree.kind(property) != "Property" || tree.flag(property, "computed") {
            continue;
        }
        let Some(key) = tree.child(property, "key") else {
            continue;
        };
        if tree.kind(key) != "Identifier" {
            continue;
        }
        let Some(name) = tree.text(key, "name") else {
            continue;
        };
        // A nested pattern or a default is not supported: the value must be the binding itself.
        let Some(value) = tree.child(property, "value") else {
            continue;
        };
        if tree.kind(value) != "Identifier" {
            continue;
        }
        let Some(symbol) = scopes.symbol_at(tree.span(value).start) else {
            continue;
        };
        if !can_rewrite_object {
            continue;
        }
        let Some(reads) = read_statement_indices(tree, scopes, scan.statements, symbol) else {
            continue;
        };
        let Some(&last_use) = reads.iter().max() else {
            continue; // never read: nothing to substitute.
        };
        if last_use <= decl_index {
            continue;
        }
        if scan.statements[decl_index + 1..last_use]
            .iter()
            .any(|statement| statement_has_effect(tree, *statement, 0))
        {
            continue;
        }
        dataflow.member.insert(symbol, (init, name.to_owned()));
    }
}

/// The top-level statement index of each read of `symbol` that is safe to reason about, or
/// `None` when any read sits under a branch, a loop, a `try`, a class body or a nested function:
/// there, execution past the declaration is conditional, repeated or deferred, and "declared
/// once, read exactly there" stops being a flat scan over sibling statements.
fn read_statement_indices(
    tree: &Tree,
    scopes: &Scopes,
    statements: &[NodeId],
    symbol: SymbolId,
) -> Option<Vec<usize>> {
    let mut indices = Vec::new();
    for (index, statement) in statements.iter().enumerate() {
        let span = tree.span(*statement);
        for reference in &scopes.symbol(symbol).references {
            if reference.write || !span.contains(reference.span) {
                continue;
            }
            if !directly_in(tree, reference.span, *statement) {
                return None;
            }
            indices.push(index);
        }
    }
    Some(indices)
}

/// Whether `reference`'s walk from its identifier up to `statement` crosses no branch, loop,
/// `try`, class body or nested function.
fn directly_in(tree: &Tree, reference: Span, statement: NodeId) -> bool {
    let Some(node) = tree.outermost(reference) else {
        return false;
    };
    let mut current = node;
    while current != statement {
        if is_unsafe_nesting(tree.kind(current)) {
            return false;
        }
        let Some(parent) = tree.parent(current) else {
            return false;
        };
        current = parent;
    }
    true
}

/// Whether this node kind sits between a read and its statement's direct execution.
fn is_unsafe_nesting(kind: &str) -> bool {
    matches!(
        kind,
        "IfStatement"
            | "ForStatement"
            | "ForInStatement"
            | "ForOfStatement"
            | "WhileStatement"
            | "DoWhileStatement"
            | "SwitchStatement"
            | "TryStatement"
            | "CatchClause"
            | "ClassBody"
            | "FunctionDeclaration"
            | "FunctionExpression"
            | "ArrowFunctionExpression"
    )
}

/// Whether a statement could observably change program state: a deliberately broad
/// over-approximation that fails closed — a statement not specifically cleared is effectful,
/// because an unverified precondition must never be reported as satisfied.
pub(super) fn statement_has_effect(tree: &Tree, statement: NodeId, depth: u32) -> bool {
    if depth > MAX_SCAN_DEPTH {
        return true;
    }
    match tree.kind(statement) {
        "EmptyStatement" => false,
        "BlockStatement" => tree
            .list(statement, "body")
            .into_iter()
            .flatten()
            .any(|inner| statement_has_effect(tree, inner, depth + 1)),
        "VariableDeclaration" => tree
            .list(statement, "declarations")
            .into_iter()
            .flatten()
            .any(|declarator| {
                tree.child(declarator, "init")
                    .is_some_and(|init| expression_has_effect(tree, init, depth + 1))
            }),
        "ExpressionStatement" => tree
            .child(statement, "expression")
            .is_some_and(|expression| expression_has_effect(tree, expression, depth + 1)),
        _ => true,
    }
}

/// Whether an expression is provably inert data. Calls, `new`, assignments, updates, `await`,
/// `yield`, spreads, chains, and anything else not specifically cleared are effectful.
pub(super) fn expression_has_effect(tree: &Tree, expression: NodeId, depth: u32) -> bool {
    if depth > MAX_SCAN_DEPTH {
        return true;
    }
    match tree.kind(expression) {
        "Literal" | "Identifier" | "ThisExpression" | "Super" => false,
        "TemplateLiteral" => tree
            .list(expression, "expressions")
            .into_iter()
            .flatten()
            .any(|inner| expression_has_effect(tree, inner, depth + 1)),
        "ArrayExpression" => tree
            .list(expression, "elements")
            .into_iter()
            .any(|element| match element {
                // A hole coerces through `undefined` only in trivial positions; the
                // over-approximation refuses it.
                None => true,
                Some(element) => match tree.kind(element) {
                    "SpreadElement" => tree
                        .child(element, "argument")
                        .is_some_and(|argument| expression_has_effect(tree, argument, depth + 1)),
                    _ => expression_has_effect(tree, element, depth + 1),
                },
            }),
        "ObjectExpression" => tree
            .list(expression, "properties")
            .into_iter()
            .flatten()
            .any(|property| match tree.kind(property) {
                "Property" => tree
                    .child(property, "value")
                    .is_some_and(|value| expression_has_effect(tree, value, depth + 1)),
                _ => true,
            }),
        "MemberExpression" => {
            let object = tree
                .child(expression, "object")
                .is_some_and(|object| expression_has_effect(tree, object, depth + 1));
            let key = tree.flag(expression, "computed")
                && tree
                    .child(expression, "property")
                    .is_some_and(|key| expression_has_effect(tree, key, depth + 1));
            object || key
        }
        "BinaryExpression" | "LogicalExpression" => ["left", "right"].into_iter().any(|key| {
            tree.child(expression, key)
                .is_some_and(|child| expression_has_effect(tree, child, depth + 1))
        }),
        "UnaryExpression" => tree
            .child(expression, "argument")
            .is_some_and(|argument| expression_has_effect(tree, argument, depth + 1)),
        "ConditionalExpression" => ["test", "consequent", "alternate"].into_iter().any(|key| {
            tree.child(expression, key)
                .is_some_and(|child| expression_has_effect(tree, child, depth + 1))
        }),
        "ParenthesizedExpression"
        | "TSAsExpression"
        | "TSSatisfiesExpression"
        | "TSNonNullExpression" => tree
            .child(expression, "expression")
            .is_some_and(|inner| expression_has_effect(tree, inner, depth + 1)),
        _ => true,
    }
}
