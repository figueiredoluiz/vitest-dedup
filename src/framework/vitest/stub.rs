//! Narrow placeholder-body recognition for optional subsumption comparisons.
use crate::lang::{NodeId, Tree};

/// Whether the callback consists only of one recognized placeholder, or no statements.
pub(crate) fn callback(tree: &Tree, function: NodeId) -> bool {
    let Some(mut body) = tree.child(function, "body") else {
        return true;
    };
    if tree.kind(body) == "BlockStatement" {
        let statements = tree.list(body, "body");
        if statements.is_empty() {
            return true;
        }
        if statements.len() != 1 {
            return false;
        }
        let Some(statement) = statements[0] else {
            return false;
        };
        let field = match tree.kind(statement) {
            "ReturnStatement" => "argument",
            "ExpressionStatement" => "expression",
            _ => return false,
        };
        let Some(expression) = tree.child(statement, field) else {
            return true;
        };
        body = expression;
    }
    expression(tree, body)
}

/// Recognizes inert literals, conventional pending markers and the canonical true assertion.
fn expression(tree: &Tree, node: NodeId) -> bool {
    let mut node = tree.inner(node);
    let awaited = tree.kind(node) == "AwaitExpression";
    while tree.kind(node) == "AwaitExpression" {
        let Some(argument) = tree.child(node, "argument") else {
            return false;
        };
        node = tree.inner(argument);
    }
    match tree.kind(node) {
        "Literal" => tree.text(node, "raw") == Some("null"),
        "Identifier" => tree.text(node, "name") == Some("undefined"),
        "UnaryExpression" => {
            tree.text(node, "operator") == Some("void")
                && tree
                    .child(node, "argument")
                    .is_some_and(|n| tree.text(n, "raw") == Some("0"))
        }
        "CallExpression" => {
            let Some(callee) = tree.child(node, "callee") else {
                return false;
            };
            if tree.flag(callee, "computed") {
                return false;
            }
            let property = tree.child(callee, "property");
            let name = tree
                .text(callee, "name")
                .or_else(|| property.and_then(|n| tree.text(n, "name")));
            let normalized = name
                .unwrap_or("")
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase();
            if matches!(normalized.as_str(), "todo" | "pending" | "notimplemented") {
                return true;
            }
            let object = tree.child(callee, "object");
            if name == Some("resolve")
                && object.is_some_and(|n| tree.text(n, "name") == Some("Promise"))
            {
                return tree.list(node, "arguments").is_empty();
            }
            if awaited || name != Some("toBe") {
                return false;
            }
            let Some(inner) = object else {
                return false;
            };
            tree.kind(inner) == "CallExpression"
                && tree
                    .child(inner, "callee")
                    .is_some_and(|n| tree.text(n, "name") == Some("expect"))
                && first_true(tree, inner)
                && first_true(tree, node)
        }
        _ => false,
    }
}

/// A literal `true` as the first argument, without evaluating an expression.
fn first_true(tree: &Tree, call: NodeId) -> bool {
    tree.list(call, "arguments")
        .first()
        .copied()
        .flatten()
        .is_some_and(|n| tree.kind(n) == "Literal" && tree.flag(n, "value"))
}
