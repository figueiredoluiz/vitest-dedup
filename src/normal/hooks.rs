//! A bounded proof that repeating a synchronous hook adds no effect.

use super::Normalizer;
use crate::lang::{Declaration, Field, NodeId, SymbolId};
use serde_json::Value;

impl Normalizer<'_> {
    /// Canonical state transformations, or no proof. Lexical writes retain exact binding identity.
    pub fn hook_effects(&mut self, callback: NodeId) -> Option<String> {
        let tree = self.tree;
        let dynamic = *self.hook_eval.get_or_insert_with(|| {
            tree.nodes().any(|node| {
                tree.kind(node) == "Identifier"
                    && tree.text(node, "name") == Some("eval")
                    && self.scopes.is_global_at(tree.span(node).start)
            })
        });
        if dynamic
            || tree.flag(callback, "async")
            || tree.flag(callback, "generator")
            || !tree.list(callback, "params").is_empty()
        {
            return None;
        }
        let body = tree.child(callback, "body")?;
        let statements = if tree.kind(body) == "BlockStatement" {
            tree.list(body, "body")
                .into_iter()
                .collect::<Option<Vec<_>>>()?
        } else {
            vec![body]
        };
        let mut effects = Vec::new();
        for statement in statements {
            let expression = if tree.kind(statement) == "ExpressionStatement" {
                tree.child(statement, "expression")?
            } else {
                statement
            };
            let expression = tree.inner(expression);
            if tree.kind(expression) == "AssignmentExpression"
                && tree.text(expression, "operator") == Some("=")
            {
                let target = tree.inner(tree.child(expression, "left")?);
                if tree.kind(target) != "Identifier" {
                    return None;
                }
                let symbol = self.scopes.symbol_at(tree.span(target).start)?;
                let binding = self.scopes.symbol(symbol);
                let Declaration::Variable(declaration) = binding.declaration else {
                    return None;
                };
                // A lexical cell has no setter. Parameters, globals, imports, consts and properties
                // cannot borrow this proof, nor can a callback-local temporary be useful setup.
                if tree.span(callback).contains(binding.span)
                    || tree.text(tree.parent(declaration)?, "kind") != Some("let")
                    || tree.flag(tree.parent(declaration)?, "declare")
                {
                    return None;
                }
                let value = tree.inner(tree.child(expression, "right")?);
                if !primitive(tree, value) {
                    return None;
                }
                let raw = tree.text(value, "raw")?;
                let fixed = if let Some(value) = self.hook_writes.get(&symbol) {
                    value.as_deref() == Some(raw)
                } else {
                    let value = self.hook_write_value(symbol);
                    let fixed = value.as_deref() == Some(raw);
                    self.hook_writes.insert(symbol, value);
                    fixed
                };
                if !fixed {
                    return None;
                }
                effects.push(format!("write:{symbol:?}:{raw}"));
            } else {
                return None;
            }
        }
        (!effects.is_empty()).then(|| serde_json::to_string(&effects).expect("strings serialize"))
    }

    /// A cell remains at one value after its first hook write, even across queued callbacks.
    fn hook_write_value(&self, symbol: SymbolId) -> Option<String> {
        let tree = self.tree;
        let mut raw = None;
        for reference in self
            .scopes
            .symbol(symbol)
            .references
            .iter()
            .filter(|reference| reference.write)
        {
            let node = reference.node?;
            let assignment = tree.outer_parent(node)?;
            if tree.kind(assignment) != "AssignmentExpression"
                || tree.text(assignment, "operator") != Some("=")
                || tree.inner(tree.child(assignment, "left")?) != node
            {
                return None;
            }
            let value = tree.inner(tree.child(assignment, "right")?);
            if !primitive(tree, value) {
                return None;
            }
            let next = tree.text(value, "raw")?;
            if raw.is_some_and(|prior| prior != next) {
                return None;
            }
            raw = Some(next);
        }
        raw.map(str::to_owned)
    }

    /// Whether an expression can be evaluated without calling code or reading a property.
    pub fn hook_registration_inert(&self, node: NodeId) -> bool {
        let tree = self.tree;
        let node = tree.inner(node);
        match tree.kind(node) {
            "FunctionDeclaration" | "FunctionExpression" | "ArrowFunctionExpression" => true,
            "Literal" => primitive(tree, node),
            "VariableDeclaration" => tree.list(node, "declarations").into_iter().all(|item| {
                item.is_some_and(|item| {
                    tree.child(item, "id")
                        .is_some_and(|id| tree.kind(id) == "Identifier")
                        && tree
                            .child(item, "init")
                            .is_none_or(|init| self.hook_registration_inert(init))
                })
            }),
            "ObjectExpression" => tree.list(node, "properties").into_iter().all(|item| {
                item.is_some_and(|item| {
                    tree.kind(item) == "Property"
                        && !tree.flag(item, "computed")
                        && tree
                            .child(item, "value")
                            .is_some_and(|value| self.hook_registration_inert(value))
                })
            }),
            "EmptyStatement" => true,
            _ => false,
        }
    }
}

/// Primitive values only; regular expressions allocate, and replacement-encoded text is uncertain.
fn primitive(tree: &crate::lang::Tree, node: NodeId) -> bool {
    tree.kind(node) == "Literal"
        && !matches!(tree.field_of(node, "regex"), Field::Object(_))
        && !matches!(tree.field_of(node, "value"), Field::Value(Value::String(text)) if text.contains('\u{fffd}'))
}
