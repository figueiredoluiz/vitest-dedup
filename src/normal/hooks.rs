//! A bounded proof that repeating a synchronous hook adds no effect.

use super::Normalizer;
use crate::lang::{Declaration, NodeId, Span, SymbolId};
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
                                    // A member of a proven plain receiver runs no getter, so it
                                    // defers nothing either read or written.
                                    && !(tree.kind(node) == "MemberExpression"
                                        && self.plain_member(node).is_some())
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
                let value = tree.inner(tree.child(expression, "right")?);
                let effect = match tree.kind(target) {
                    "Identifier" => self.lexical_write_effect(callback, target, value)?,
                    "MemberExpression" => self.plain_write(target)?,
                    _ => return None,
                };
                effects.push(effect);
            } else {
                return None;
            }
        }
        (!effects.is_empty()).then(|| serde_json::to_string(&effects).expect("strings serialize"))
    }

    /// An overwrite of one lexical cell, or no proof.
    fn lexical_write_effect(
        &mut self,
        callback: NodeId,
        target: NodeId,
        value: NodeId,
    ) -> Option<String> {
        let tree = self.tree;
        if !primitive(tree, value) {
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
        let raw = tree.text(value, "raw")?;
        let fixed = if let Some(value) = self.hook_writes.get(&symbol) {
            value.as_deref() == Some(raw)
        } else {
            let value = self.hook_write_value(symbol);
            let fixed = value.as_deref() == Some(raw);
            self.hook_writes.insert(symbol, value);
            fixed
        };
        fixed.then(|| format!("write:{symbol:?}:{raw}"))
    }

    /// One data property of a proven plain receiver: no getter can run when it is read or
    /// written, so the expression defers nothing and the write carries no user code. The
    /// receiver is a lexical binding assigned nowhere whose only value is a plain object
    /// literal — own data properties, static keys, no `__proto__`.
    fn plain_member(&self, member: NodeId) -> Option<(SymbolId, &str)> {
        let tree = self.tree;
        if tree.flag(member, "computed") {
            return None;
        }
        let object = tree.inner(tree.child(member, "object")?);
        if tree.kind(object) != "Identifier" {
            return None;
        }
        let property = tree.inner(tree.child(member, "property")?);
        let name = tree.text(property, "name")?;
        if name == "__proto__" || self.prototype_polluted() {
            return None;
        }
        let symbol = self.scopes.symbol_at(tree.span(object).start)?;
        let binding = self.scopes.symbol(symbol);
        let Declaration::Variable(declaration) = binding.declaration else {
            return None;
        };
        // A `declare`d receiver has no initializer to prove.
        if tree.flag(tree.parent(declaration)?, "declare") {
            return None;
        }
        // The receiver is assigned nowhere, so the initializer's object literal is its only
        // value.
        if binding.references.iter().any(|reference| reference.write) {
            return None;
        }
        let init = tree.inner(tree.child(declaration, "init")?);
        if tree.kind(init) != "ObjectExpression" {
            return None;
        }
        for item in tree.list(init, "properties") {
            let item = tree.inner(item?);
            if tree.kind(item) != "Property"
                || tree.text(item, "kind") != Some("init")
                || tree.flag(item, "computed")
            {
                return None;
            }
            let key = tree.inner(tree.child(item, "key")?);
            let key = if tree.kind(key) == "Identifier" {
                tree.text(key, "name")?
            } else {
                tree.text(key, "value")?
            };
            if key == "__proto__" {
                return None;
            }
        }
        // Every use of the receiver is a static member read or a member-path write: a bare value
        // can grow properties elsewhere, and a method call exposes the receiver as `this`, which
        // can install a setter no global-object scan sees.
        let binding = self.scopes.symbol(symbol);
        for reference in &binding.references {
            let node = reference.node?;
            let (_, current, use_of) = self.member_chain(node)?;
            if tree.kind(use_of) == "CallExpression"
                && tree.inner(tree.child(use_of, "callee")?) == current
            {
                return None;
            }
            if tree.kind(use_of) == "UnaryExpression"
                && tree.text(use_of, "operator") == Some("delete")
            {
                return None;
            }
        }
        Some((symbol, name))
    }

    /// The static member chain a reference sits under: its segments, the chain's outermost
    /// member, and the node that uses the chain's value. A computed access, a `__proto__`
    /// segment or a bare use (no member at all) refuses.
    fn member_chain(&self, node: NodeId) -> Option<(Vec<&str>, NodeId, NodeId)> {
        let tree = self.tree;
        let mut current = node;
        let mut segments: Vec<&str> = Vec::new();
        loop {
            let parent = tree.outer_parent(current)?;
            if tree.kind(parent) == "MemberExpression"
                && tree.inner(tree.child(parent, "object")?) == current
            {
                if tree.flag(parent, "computed") {
                    return None;
                }
                let segment = tree.text(tree.inner(tree.child(parent, "property")?), "name")?;
                if segment == "__proto__" {
                    return None;
                }
                segments.push(segment);
                current = parent;
            } else {
                break;
            }
        }
        if segments.is_empty() {
            return None;
        }
        let use_of = tree.outer_parent(current)?;
        Some((segments, current, use_of))
    }

    /// One own-property write over a proven plain receiver, valued by every module-wide write
    /// to the same path, or no proof. A receiver local to the hook callback never reaches this:
    /// its declaration is a statement the statement-shape check refuses first.
    fn plain_write(&self, member: NodeId) -> Option<String> {
        let tree = self.tree;
        let (symbol, name) = self.plain_member(member)?;
        // The tracked path keeps one literal value across the module.
        let binding = self.scopes.symbol(symbol);
        let mut written = Vec::new();
        for reference in &binding.references {
            let node = reference.node?;
            let (segments, current, use_of) = self.member_chain(node)?;
            let tracked = segments.len() == 1 && segments[0] == name;
            match tree.kind(use_of) {
                "AssignmentExpression"
                    if tracked && tree.inner(tree.child(use_of, "left")?) == current =>
                {
                    // The tracked write keeps one primitive value across the module.
                    if tree.text(use_of, "operator") != Some("=") {
                        return None;
                    }
                    let right = tree.inner(tree.child(use_of, "right")?);
                    if !primitive(tree, right) {
                        return None;
                    }
                    written.push(tree.text(right, "raw")?);
                }
                "UpdateExpression"
                    if tracked && tree.inner(tree.child(use_of, "argument")?) == current =>
                {
                    return None;
                }
                "UnaryExpression" if tree.text(use_of, "operator") == Some("delete") => {
                    return None;
                }
                _ => {}
            }
        }
        let raw = written.first()?;
        written
            .iter()
            .all(|other| other == raw)
            .then(|| format!("pwrite:{symbol:?}:{name}:{raw}"))
    }

    /// A member write or define-property call rooted at the global `Object` or `Reflect` could
    /// give the receiver's prototype a setter.
    fn prototype_polluted(&self) -> bool {
        if self.hook_pollution.get().is_none() {
            let tree = self.tree;
            let mut polluted = false;
            'scan: for node in tree.nodes() {
                let global = |member: NodeId| -> Option<bool> {
                    // The chain's root decides: `Object.prototype.__defineSetter__` reaches the
                    // global through `Object.prototype`.
                    let mut current = tree.inner(tree.child(member, "object")?);
                    while tree.kind(current) == "MemberExpression" {
                        current = tree.inner(tree.child(current, "object")?);
                    }
                    if tree.kind(current) != "Identifier" {
                        return None;
                    }
                    let name = tree.text(current, "name")?;
                    Some(
                        matches!(name, "Object" | "Reflect")
                            && self.scopes.is_global_at(tree.span(current).start),
                    )
                };
                polluted = match tree.kind(node) {
                    "AssignmentExpression" => tree
                        .child(node, "left")
                        .map(|left| tree.inner(left))
                        .and_then(|left| (tree.kind(left) == "MemberExpression").then_some(left))
                        .is_some_and(|left| global(left) == Some(true)),
                    "CallExpression" => tree
                        .child(node, "callee")
                        .map(|callee| tree.inner(callee))
                        .and_then(|callee| {
                            (tree.kind(callee) == "MemberExpression"
                                && !tree.flag(callee, "computed"))
                            .then_some(callee)
                        })
                        .is_some_and(|callee| {
                            global(callee) == Some(true)
                                && tree
                                    .child(callee, "property")
                                    .and_then(|property| tree.text(property, "name"))
                                    .is_some_and(|name| {
                                        matches!(
                                            name,
                                            "defineProperty"
                                                | "defineProperties"
                                                | "__defineSetter__"
                                                | "__defineGetter__"
                                                | "set"
                                        )
                                    })
                        }),
                    _ => polluted,
                };
                if polluted {
                    break 'scan;
                }
            }
            self.hook_pollution.set(Some(polluted));
        }
        self.hook_pollution
            .get()
            .expect("pollution facts initialized")
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
            "ArrayExpression" => tree
                .list(node, "elements")
                .into_iter()
                .all(|item| item.is_some_and(|item| self.hook_registration_inert(item))),
            "EmptyStatement" => true,
            _ => false,
        }
    }
}

/// Primitive values only; see [`crate::lang::primitive`].
fn primitive(tree: &crate::lang::Tree, node: NodeId) -> bool {
    crate::lang::primitive(tree, node)
}
