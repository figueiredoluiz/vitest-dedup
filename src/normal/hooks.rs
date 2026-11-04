//! A bounded proof that repeating a synchronous hook adds no effect.

use super::Normalizer;
use crate::lang::{Declaration, Field, NodeId, Span, SymbolId};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// One callback whose execution cannot establish the absence of deferred work.
pub(super) struct DeferredCallback {
    span: Span,
    hook_scope: Option<Option<Span>>,
}

impl Normalizer<'_> {
    /// A removed hook must not shift deferred work visible to any callback in its lifecycle.
    pub fn hook_scheduling_safe(&mut self, scope: Option<NodeId>) -> bool {
        let scope = scope.map(|scope| self.tree.span(scope));
        !self.deferred_callbacks().iter().any(|callback| {
            scope.is_none_or(|scope| {
                scope.contains(callback.span)
                    || callback
                        .hook_scope
                        .is_some_and(|outer| outer.is_none_or(|outer| outer.contains(scope)))
            })
        })
    }

    /// Only the registrations actually reached by this invocation contribute scheduling facts.
    pub fn runner_scheduling_safe(&mut self, sites: &[NodeId]) -> bool {
        let sites: Vec<_> = sites.iter().map(|site| self.tree.span(*site)).collect();
        !self
            .deferred_callbacks()
            .iter()
            .any(|callback| sites.iter().any(|site| site.contains(callback.span)))
    }

    /// Read each runtime callback once; scope queries never repeat joins over the module's hooks.
    fn deferred_callbacks(&mut self) -> &[DeferredCallback] {
        if self.hook_scheduling.is_none() {
            let tree = self.tree;
            let runtime: HashSet<_> = self.structure.run_time.iter().copied().collect();
            let hooks: HashMap<_, _> = self
                .structure
                .hooks
                .iter()
                .map(|hook| (hook.span, hook.scope))
                .collect();
            let deferred = tree
                .nodes()
                .filter(|node| tree.is_function(*node) && runtime.contains(&tree.span(*node)))
                .filter(|callback| {
                    std::iter::once(*callback)
                        .chain(tree.descendants(*callback))
                        .any(|node| match tree.kind(node) {
                            "AwaitExpression"
                            | "YieldExpression"
                            | "ImportExpression"
                            | "NewExpression"
                            | "TaggedTemplateExpression"
                            | "SpreadElement" => true,
                            "CallExpression" | "MemberExpression" => {
                                !self.synchronous_assertion_node(node)
                            }
                            _ if tree.is_function(node) => {
                                tree.flag(node, "async")
                                    || tree.flag(node, "generator")
                                    || !runtime.contains(&tree.span(node))
                            }
                            "Property" => {
                                tree.text(node, "kind").is_some_and(|kind| kind != "init")
                                    || tree.flag(node, "computed")
                            }
                            _ => false,
                        })
                })
                .map(|callback| DeferredCallback {
                    span: tree.span(callback),
                    hook_scope: hooks.get(&tree.span(callback)).copied(),
                })
                .collect();
            self.hook_scheduling = Some(deferred);
        }
        self.hook_scheduling
            .as_deref()
            .expect("callback facts initialized")
    }

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

    /// Only a directly imported framework assertion and its synchronous member chain qualify.
    fn synchronous_assertion_node(&self, node: NodeId) -> bool {
        let tree = self.tree;
        let mut current = node;
        for _ in 0..64 {
            current = tree.inner(current);
            match tree.kind(current) {
                "MemberExpression" => {
                    if tree.flag(current, "computed")
                        || tree
                            .child(current, "property")
                            .and_then(|property| tree.text(property, "name"))
                            .is_some_and(|name| matches!(name, "resolves" | "rejects"))
                    {
                        return false;
                    }
                    let Some(object) = tree.child(current, "object") else {
                        return false;
                    };
                    current = object;
                }
                "CallExpression" => {
                    let Some(callee) = tree.child(current, "callee") else {
                        return false;
                    };
                    current = callee;
                }
                "Identifier" => {
                    if !tree.outer_parent(current).is_some_and(|call| {
                        tree.kind(call) == "CallExpression"
                            && tree.child(call, "callee") == Some(current)
                    }) {
                        return false;
                    }
                    let Some(symbol) = self.scopes.symbol_at(tree.span(current).start) else {
                        return false;
                    };
                    let Declaration::Import {
                        import,
                        ref imported,
                        ..
                    } = self.scopes.symbol(symbol).declaration
                    else {
                        return false;
                    };
                    let crate::lang::Imported::Named(name) = imported else {
                        return false;
                    };
                    return self.setting.dialect.assertion(name).is_some()
                        && tree
                            .child(import, "source")
                            .and_then(|source| tree.text(source, "value"))
                            .is_some_and(|source| self.setting.dialect.is_api_module(source));
                }
                _ => return false,
            }
        }
        false
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
        if tree.erased_declaration(node) {
            return true;
        }
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
