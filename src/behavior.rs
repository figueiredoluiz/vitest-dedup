//! Behavior: a test callback's body turned into its ordered behavior trace.
//!
//! The trace is the ordered list of events the two similarity rules compare — one event per
//! recognized behavior step, in source order, from a closed grammar. Rules make two exact-match
//! decisions over it: equal sorted multiset (`near-duplicate-test`) and strict ordered
//! subsequence (`subsumed-test`). Both are equality gates, never similarity scores: any single
//! differing event decides, and a `distinct` verdict is a comparison result, never conflated
//! with `not-compared`.
//!
//! Every name an event carries is the normal form's name for the same read, written by
//! `normal`'s writer through one shared walk: locals numbered at first appearance in the emitted
//! events, outside bindings by their declaration, a table row's cell at its reads. Two policies
//! differ from the flat form, both because the rules compare across statement orders: locals
//! number at first *event* appearance, not body-root order, and an outside object is named by its
//! declaration without the walk's ordinal. An inlined local has no number at all.
//!
//! Nothing here knows a rule, and no framework API name is spelled: the assertion entries, their
//! qualifiers and their meaning-changing members are [`Dialect`] data.

pub(crate) mod dataflow;

#[cfg(test)]
mod tests;

use crate::framework::{AssertionEntry, Dialect};
use crate::lang::{Name, NodeId, Parsed, Scopes, Span, SymbolId, Tree};
use crate::model::sha256_128;
use crate::normal::{CallbackForm, Normalizer, Walk, MAX_CLASS_RENDER_BYTES, UNSCOPED_PRIVATE};
use dataflow::Dataflow;
use std::collections::HashMap;

/// The symbol an identifier node reads or binds, when the scope analysis recorded one.
fn symbol_at(parsed: &Parsed, node: NodeId) -> Option<SymbolId> {
    parsed.scopes.symbol_at(parsed.tree.span(node).start)
}

/// Whether the trace can prove a class's behavior from its content: it cannot when a key is
/// computed from anything but a literal (its value, and when it evaluates, are unproved), or when
/// a `with` statement may bind arbitrary names. A dependency the trace cannot name is an
/// abstention, never a claim.
fn class_provable(tree: &Tree, class: NodeId) -> bool {
    !tree.descendants(class).any(|node| match tree.kind(node) {
        "MethodDefinition" | "PropertyDefinition" | "AccessorProperty" => {
            tree.flag(node, "computed")
                && tree
                    .child(node, "key")
                    .is_some_and(|key| tree.kind(key) != "Literal")
        }
        "WithStatement" => true,
        _ => false,
    })
}

/// How deep the trace walks. Past it the walk stops and the trace is incomplete: a dropped
/// region is invisible in the events, so two bodies that differ only past the bound could
/// collect equal traces — a trace with a hole is not evidence of similarity in either direction,
/// so a rule keyed on the trace skips its case and says why.
const MAX_TRACE_DEPTH: u32 = 256;

/// How many links a chai-style property chain may carry. A chain is recognized whole or not at
/// all: a bound that truncated would report the longest accepted prefix of itself.
const MAX_ASSERTION_CHAIN: usize = 16;

/// A body's ordered behavior trace: one 128-bit digest per event, in source order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trace {
    /// The events, in source order.
    pub events: Vec<u128>,
    /// Whether the walk finished. A rule keyed on the trace must skip an unfinished one and say
    /// so, because silence about a body it could not read proves nothing. The flat rules do not
    /// read it.
    pub complete: bool,
}

impl Trace {
    /// How many events were recorded.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether no event was recorded.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// The trace of one callback body, sharing the caller's normalizer so every name the events
/// carry comes from the same declarations the flat normal form names. `function` is the callback
/// the registration passes; `substitutions` binds its parameters to a table row's cells, and
/// `bound_parameters` says all of them are bound.
pub fn trace<'s>(
    normalizer: &mut Normalizer<'s>,
    parsed: &'s Parsed,
    function: NodeId,
    substitutions: HashMap<SymbolId, String>,
    bound_parameters: bool,
) -> (Trace, CallbackForm) {
    let (events, complete, form) = trace_text(
        normalizer,
        parsed,
        function,
        substitutions,
        bound_parameters,
    );
    (
        Trace {
            events: events.iter().map(|event| sha256_128(event)).collect(),
            complete,
        },
        form,
    )
}

/// The events as text, for this layer's own controls; the corpus stores only their digests.
pub(crate) fn trace_text<'s>(
    normalizer: &mut Normalizer<'s>,
    parsed: &'s Parsed,
    function: NodeId,
    substitutions: HashMap<SymbolId, String>,
    bound_parameters: bool,
) -> (Vec<String>, bool, CallbackForm) {
    let form = normalizer.callback_form(function);
    let dataflow = dataflow::analyze(parsed, function);
    let mut walk = normalizer.body_walk(function, substitutions, bound_parameters);
    walk.suffixes = dataflow.names.clone();
    // A member-substituted read renders `object.property` everywhere it appears. The object is
    // declared outside the body, so its rendering is stable: precompute it once, before any
    // event, and the read needs no numbering of its own.
    for (symbol, (object, property)) in dataflow.member.clone() {
        let rendered = normalizer.form_of(object, &mut walk);
        let substituted = normalizer.member_form(&rendered, &property);
        walk.substitutions.insert(symbol, substituted);
    }
    walk.inline = dataflow
        .inline
        .iter()
        .map(|(symbol, inline)| (*symbol, inline.init))
        .collect();
    // A local class a shape names renders by its content, the way a constructor call already
    // does — otherwise two constructions over different classes share a shape inside any
    // assertion built on them. An unprovable class is not registered: its reads keep the local's
    // own name, which can only keep events apart, never merge them.
    {
        let tree = &parsed.tree;
        for node in tree.descendants(function) {
            let candidate = match tree.kind(node) {
                "ClassDeclaration" => tree
                    .child(node, "id")
                    .and_then(|id| symbol_at(parsed, id))
                    .map(|symbol| (symbol, node)),
                "VariableDeclarator" => {
                    let init = tree.child(node, "init");
                    let is_class = init.is_some_and(|init| tree.kind(init) == "ClassExpression");
                    if is_class {
                        tree.child(node, "id")
                            .and_then(|id| symbol_at(parsed, id))
                            .map(|symbol| (symbol, init.unwrap()))
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some((symbol, class)) = candidate {
                if class_provable(tree, class) {
                    walk.classes.insert(symbol, class);
                }
            }
        }
    }
    tag_retained_sources(normalizer, parsed, function, &dataflow, &mut walk);
    let root = walk.root;
    let mut walker = Walker {
        normalizer,
        parsed,
        walk,
        dataflow,
        root,
        events: Vec::new(),
        complete: true,
    };
    walker.run(function);
    let complete = walker.complete && !walker.walk.class_budget_spent;
    (walker.events, complete, form)
}

/// Tags every read of a retained name with what bound it, since its reads name only the local and
/// most sources emit no event. The tag spends no number and records no event, so numbering and a
/// body of bindings alone stay as they were. A source with its own event stays out of the tag,
/// which would make one difference every read's, and so does a plain name bound from one, which
/// leaves nothing to tag. Source order, so a source reading an earlier local carries its tag.
fn tag_retained_sources<'s>(
    normalizer: &mut Normalizer<'s>,
    parsed: &'s Parsed,
    function: NodeId,
    dataflow: &Dataflow,
    walk: &mut Walk,
) {
    let tree = &parsed.tree;
    for node in tree.descendants(function) {
        let Some((target, source)) = binding_of(tree, node) else {
            continue;
        };
        let named = names_itself(tree, source);
        if named && tree.kind(target) == "Identifier" {
            continue;
        }
        let names = bound_names(parsed, target);
        let retained = |symbol: &SymbolId| {
            !dataflow.inline.contains_key(symbol) && !dataflow.member.contains_key(symbol)
        };
        if !names.iter().any(retained) {
            continue;
        }
        let mut bound = normalizer.detached_shape(target, walk);
        if !named {
            bound.push_str(" = ");
            bound.push_str(&normalizer.detached_shape(source, walk));
        }
        // Rendered apart from its class, a private name is only its spelling.
        if bound.contains(UNSCOPED_PRIVATE) {
            continue;
        }
        let digest = sha256_128(&bound);
        for (position, symbol) in names.iter().enumerate() {
            if retained(symbol) {
                let tag = format!("<{digest:x}#{position}");
                walk.suffixes.entry(*symbol).or_default().push_str(&tag);
            }
        }
    }
}

/// Whether `source` records an event naming what it evaluates: a call or a construction, awaited
/// or not, or a function, whose body records its own.
fn names_itself(tree: &Tree, source: NodeId) -> bool {
    let source = tree.inner(source);
    match tree.kind(source) {
        "CallExpression" | "NewExpression" | "ArrowFunctionExpression" | "FunctionExpression" => {
            true
        }
        "AwaitExpression" => tree
            .child(source, "argument")
            .is_some_and(|argument| names_itself(tree, argument)),
        _ => false,
    }
}

/// The target and source `node` binds, when it is a declarator or a loop over a declaration.
fn binding_of(tree: &Tree, node: NodeId) -> Option<(NodeId, NodeId)> {
    match tree.kind(node) {
        "VariableDeclarator" => Some((tree.child(node, "id")?, tree.child(node, "init")?)),
        "ForInStatement" | "ForOfStatement" => {
            let left = tree.child(node, "left")?;
            let declarator = tree.list(left, "declarations").first().copied()??;
            Some((tree.child(declarator, "id")?, tree.child(node, "right")?))
        }
        _ => None,
    }
}

/// The symbols `pattern` declares, in source order. A local a default or a computed key reads is
/// not one of them.
fn bound_names(parsed: &Parsed, pattern: NodeId) -> Vec<SymbolId> {
    let tree = &parsed.tree;
    let mut names = Vec::new();
    for node in std::iter::once(pattern).chain(tree.descendants(pattern)) {
        if tree.kind(node) != "Identifier" {
            continue;
        }
        let Some(symbol) = symbol_at(parsed, node) else {
            continue;
        };
        let declared = tree.span(node).contains(parsed.scopes.symbol(symbol).span);
        if declared && !names.contains(&symbol) {
            names.push(symbol);
        }
    }
    names
}

/// One body's trace walk: the event grammar, driven over the owned tree, naming through the
/// shared walk.
struct Walker<'n, 's> {
    normalizer: &'n mut Normalizer<'s>,
    parsed: &'s Parsed,
    walk: Walk,
    dataflow: Dataflow,
    /// The body's root span: what separates a local from an outside binding, and the body's own
    /// name shadowing from the API.
    root: Span,
    events: Vec<String>,
    /// Whether every walk finished inside [`MAX_TRACE_DEPTH`], and every class the trace renders
    /// was provable. A walk that sets this to `false` has produced a trace with a hole.
    complete: bool,
}

impl<'s> Walker<'_, 's> {
    /// The body's tree.
    fn tree(&self) -> &'s Tree {
        &self.parsed.tree
    }

    /// The body's bindings and references.
    fn scopes(&self) -> &'s Scopes {
        &self.parsed.scopes
    }

    /// The dialect the walk reads its assertion vocabulary from.
    fn dialect(&self) -> &'s Dialect {
        self.normalizer.dialect()
    }

    /// Appends an event, prefixing it when its execution is deferred.
    fn push(&mut self, deferred: bool, event: String) {
        self.events.push(if deferred {
            format!("deferred-{event}")
        } else {
            event
        });
    }

    /// Renders a node's normal form in the shared walk: the shape an event carries.
    fn shape(&mut self, node: NodeId) -> String {
        self.normalizer.form_of(node, &mut self.walk)
    }

    /// Renders a node as shape: literals by kind, not by value. An assertion's subject tells
    /// assertions apart by what they compare, not by which literal each compares; the expected
    /// values keep theirs, because a literal difference there is a different claim.
    fn structural_shape(&mut self, node: NodeId) -> String {
        self.walk.structural = true;
        let shape = self.normalizer.form_of(node, &mut self.walk);
        self.walk.structural = false;
        shape
    }

    /// Walks the callback's body: its statements in order, or, for a concise arrow, the one
    /// expression — spliced when it is a returned-chain-shaped `.then`.
    fn run(&mut self, function: NodeId) {
        let tree = self.tree();
        let Some(body) = tree.child(function, "body") else {
            return;
        };
        if tree.flag(function, "expression") {
            if tree.kind(body) == "CallExpression" {
                if let Some((awaited, continuation)) = self.then_continuation(body) {
                    self.push(false, "await".to_owned());
                    self.walk_expr(awaited, false, 1);
                    for statement in continuation {
                        self.walk_stmt(statement, false, 1);
                    }
                    return;
                }
            }
            self.walk_expr(body, false, 0);
            return;
        }
        self.walk_block(body, false, 0);
    }

    /// Collects statement behavior, carrying deferred-execution state into nested expressions.
    fn walk_stmt(&mut self, statement: NodeId, deferred: bool, depth: u32) {
        if depth > MAX_TRACE_DEPTH {
            self.complete = false;
            return;
        }
        let tree = self.tree();
        match tree.kind(statement) {
            "BlockStatement" => {
                let inner: Vec<NodeId> =
                    tree.list(statement, "body").into_iter().flatten().collect();
                for statement in inner {
                    self.walk_stmt(statement, deferred, depth + 1);
                }
            }
            "ExpressionStatement" => {
                if let Some(expression) = tree.child(statement, "expression") {
                    self.walk_expr(expression, deferred, depth + 1);
                }
            }
            "VariableDeclaration" => {
                let declarators: Vec<NodeId> = tree
                    .list(statement, "declarations")
                    .into_iter()
                    .flatten()
                    .collect();
                for declarator in declarators {
                    // A fully inlined declarator records no event of its own: its one read
                    // accounts for the same initializer at the read site, rendered into the
                    // shapes and re-walked for its events there. Walking it here too would
                    // double-count it.
                    let inlined = tree
                        .child(declarator, "id")
                        .filter(|id| tree.kind(*id) == "Identifier")
                        .is_some_and(|id| self.dataflow.inlined(self.symbol_at(id)));
                    if inlined {
                        continue;
                    }
                    if let Some(init) = tree.child(declarator, "init") {
                        self.walk_expr(init, deferred, depth + 1);
                    }
                }
            }
            "ClassDeclaration" => self.class_definition(statement, deferred, depth + 1),
            "IfStatement" => {
                self.push(deferred, "if".to_owned());
                for key in ["test", "consequent", "alternate"] {
                    if let Some(part) = tree.child(statement, key) {
                        self.walk_part(part, deferred, depth + 1);
                    }
                }
            }
            "ForStatement" => {
                if let Some(init) = tree.child(statement, "init") {
                    if tree.kind(init) == "VariableDeclaration" {
                        self.declaration_inits(init, deferred, depth + 1);
                    } else {
                        self.walk_expr(init, deferred, depth + 1);
                    }
                }
                self.push(deferred, "loop".to_owned());
                for key in ["test", "update", "body"] {
                    if let Some(part) = tree.child(statement, key) {
                        self.walk_part(part, deferred, depth + 1);
                    }
                }
            }
            "ForInStatement" | "ForOfStatement" => {
                // Sync loop kinds deliberately share one token. Async iteration changes protocol
                // selection and suspension behavior, so it gets its own token.
                let protocol =
                    if tree.kind(statement) == "ForOfStatement" && tree.flag(statement, "await") {
                        "loop-await"
                    } else {
                        "loop"
                    };
                self.push(deferred, protocol.to_owned());
                if let Some(right) = tree.child(statement, "right") {
                    self.walk_expr(right, deferred, depth + 1);
                }
                if let Some(body) = tree.child(statement, "body") {
                    self.walk_part(body, deferred, depth + 1);
                }
            }
            "WhileStatement" => {
                self.push(deferred, "loop".to_owned());
                for key in ["test", "body"] {
                    if let Some(part) = tree.child(statement, key) {
                        self.walk_part(part, deferred, depth + 1);
                    }
                }
            }
            "DoWhileStatement" => {
                self.push(deferred, "loop".to_owned());
                for key in ["body", "test"] {
                    if let Some(part) = tree.child(statement, key) {
                        self.walk_part(part, deferred, depth + 1);
                    }
                }
            }
            "TryStatement" => {
                self.push(deferred, "try".to_owned());
                for key in ["block", "handler", "finalizer"] {
                    if let Some(part) = tree.child(statement, key) {
                        self.walk_part(part, deferred, depth + 1);
                    }
                }
            }
            "CatchClause" => {
                if let Some(body) = tree.child(statement, "body") {
                    self.walk_stmt(body, deferred, depth + 1);
                }
            }
            "SwitchStatement" => {
                // No marker of its own: the discriminant and the case bodies are the sequence
                // (a known gap — swapped branches carry equal events).
                if let Some(discriminant) = tree.child(statement, "discriminant") {
                    self.walk_expr(discriminant, deferred, depth + 1);
                }
                let cases: Vec<NodeId> = tree
                    .list(statement, "cases")
                    .into_iter()
                    .flatten()
                    .collect();
                for case in cases {
                    if let Some(test) = tree.child(case, "test") {
                        self.walk_expr(test, deferred, depth + 1);
                    }
                    let consequent: Vec<NodeId> = tree
                        .list(case, "consequent")
                        .into_iter()
                        .flatten()
                        .collect();
                    for statement in consequent {
                        self.walk_stmt(statement, deferred, depth + 1);
                    }
                }
            }
            "ReturnStatement" => {
                // `return <call>().then(callback)` — the resolved chain is what Vitest itself
                // awaits, so it canonicalizes to the shape a bare `await <call>(); <callback
                // body>` produces, rather than a `return` plus a `deferred-` callback body.
                let argument = tree.child(statement, "argument");
                let mut spliced = false;
                if let Some(argument) = argument {
                    if tree.kind(argument) == "CallExpression" {
                        if let Some((awaited, continuation)) = self.then_continuation(argument) {
                            self.push(deferred, "await".to_owned());
                            self.walk_expr(awaited, deferred, depth + 1);
                            for statement in continuation {
                                self.walk_stmt(statement, deferred, depth + 1);
                            }
                            spliced = true;
                        }
                    }
                }
                if spliced {
                    return;
                }
                self.push(deferred, "return".to_owned());
                if let Some(argument) = argument {
                    self.walk_expr(argument, deferred, depth + 1);
                }
            }
            "ThrowStatement" => {
                self.push(deferred, "throw".to_owned());
                if let Some(argument) = tree.child(statement, "argument") {
                    self.walk_expr(argument, deferred, depth + 1);
                }
            }
            "LabeledStatement" => {
                if let Some(body) = tree.child(statement, "body") {
                    self.walk_stmt(body, deferred, depth + 1);
                }
            }
            "FunctionDeclaration" => {
                // A nested declaration is retained as deferred behavior: this body may never run
                // it, or run it many times, so its events are not the test's own.
                if let Some(body) = tree.child(statement, "body") {
                    if tree.kind(body) == "BlockStatement" {
                        let inner: Vec<NodeId> =
                            tree.list(body, "body").into_iter().flatten().collect();
                        for statement in inner {
                            self.walk_stmt(statement, true, depth + 1);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Walks a statement or expression position of a control-flow part.
    fn walk_part(&mut self, part: NodeId, deferred: bool, depth: u32) {
        match self.tree().kind(part) {
            "BlockStatement"
            | "CatchClause"
            | "IfStatement"
            | "ForStatement"
            | "ForInStatement"
            | "ForOfStatement"
            | "WhileStatement"
            | "DoWhileStatement"
            | "TryStatement"
            | "ReturnStatement"
            | "ThrowStatement"
            | "LabeledStatement"
            | "FunctionDeclaration"
            | "SwitchStatement"
            | "VariableDeclaration"
            | "ClassDeclaration"
            | "ExpressionStatement" => self.walk_stmt(part, deferred, depth),
            "ArrowFunctionExpression" | "FunctionExpression" => self.deferred_body(part, depth),
            _ => self.walk_expr(part, deferred, depth),
        }
    }

    /// The initializers of a declaration used as a loop head.
    fn declaration_inits(&mut self, declaration: NodeId, deferred: bool, depth: u32) {
        let tree = self.tree();
        let declarators: Vec<NodeId> = tree
            .list(declaration, "declarations")
            .into_iter()
            .flatten()
            .collect();
        for declarator in declarators {
            if let Some(init) = tree.child(declarator, "init") {
                self.walk_expr(init, deferred, depth);
            }
        }
    }

    /// Recognizes `<call>().then(callback)`: a single awaited call chained straight into its own
    /// continuation. Every arm is a precondition, not a parsing convenience: a second argument is
    /// a rejection handler, which observes a different code path than any `await`; a chained
    /// `.then` means the continuation depends on what the first returns; and a parameterized or
    /// expression-bodied callback has no statement list to splice.
    fn then_continuation(&mut self, call: NodeId) -> Option<(NodeId, Vec<NodeId>)> {
        let tree = self.tree();
        let callee = tree.child(call, "callee")?;
        if tree.kind(callee) != "MemberExpression" || tree.flag(callee, "computed") {
            return None;
        }
        if tree
            .child(callee, "property")
            .and_then(|property| tree.text(property, "name"))
            != Some("then")
        {
            return None;
        }
        let inner = tree.child(callee, "object")?;
        if tree.kind(inner) != "CallExpression" {
            return None;
        }
        if let Some(inner_callee) = tree.child(inner, "callee") {
            if tree.kind(inner_callee) == "MemberExpression"
                && !tree.flag(inner_callee, "computed")
                && tree
                    .child(inner_callee, "property")
                    .and_then(|property| tree.text(property, "name"))
                    == Some("then")
            {
                return None; // a chained `.then().then()`: not "a single awaited call".
            }
        }
        let arguments = tree.list(call, "arguments");
        let [Some(argument)] = arguments.as_slice() else {
            return None; // zero, or two-or-more (a rejection handler): never equivalent.
        };
        if !tree.is_function(*argument)
            || !tree.list(*argument, "params").is_empty()
            || tree.flag(*argument, "expression")
            // A generator called returns an iterator: its body never runs here, so the spliced
            // statements would claim behavior the runtime does not have.
            || tree.flag(*argument, "generator")
        {
            return None;
        }
        // A `function` continuation may read the resolved value through `arguments[0]`, or its
        // own `this`; the splice would re-home those reads into the test body, where they name
        // something else.
        if tree.kind(*argument) == "FunctionExpression" {
            if let Some(body) = tree.child(*argument, "body") {
                if self.normalizer.binds_this(body) {
                    return None;
                }
            }
        }
        let body = tree.child(*argument, "body")?;
        (tree.kind(body) == "BlockStatement").then(|| {
            (
                inner,
                tree.list(body, "body").into_iter().flatten().collect(),
            )
        })
    }

    /// Collects expression behavior while distinguishing enclosing execution from deferred
    /// nested callables.
    fn walk_expr(&mut self, expression: NodeId, deferred: bool, depth: u32) {
        if depth > MAX_TRACE_DEPTH {
            self.complete = false;
            return;
        }
        let tree = self.tree();
        match tree.kind(expression) {
            "CallExpression" => self.walk_call(expression, deferred, depth),
            "NewExpression" => self.walk_new(expression, deferred, depth),
            "AwaitExpression" => {
                self.push(deferred, "await".to_owned());
                if let Some(argument) = tree.child(expression, "argument") {
                    self.walk_expr(argument, deferred, depth + 1);
                }
            }
            "AssignmentExpression" => {
                let operator = tree.text(expression, "operator").unwrap_or_default();
                let target = tree
                    .child(expression, "left")
                    .map(|target| self.shape(target))
                    .unwrap_or_default();
                let value = tree
                    .child(expression, "right")
                    .map(|value| self.shape(value))
                    .unwrap_or_default();
                self.push(
                    deferred,
                    format!("assign:{operator} target={target} value={value}"),
                );
            }
            "UpdateExpression" => {
                let operator = tree.text(expression, "operator").unwrap_or_default();
                let prefix = if tree.flag(expression, "prefix") {
                    "prefix"
                } else {
                    "postfix"
                };
                let target = tree
                    .child(expression, "argument")
                    .map(|target| self.shape(target))
                    .unwrap_or_default();
                self.push(
                    deferred,
                    format!("update:{prefix}{operator} target={target}"),
                );
            }
            // A nested function used as a value is retained as deferred behavior: this body may
            // never run it, or run it many times, so its calls and assertions are not the
            // enclosing test's own top-level behavior.
            "ArrowFunctionExpression" | "FunctionExpression" => {
                self.deferred_body(expression, depth + 1)
            }
            "MemberExpression" => self.walk_member_value(expression, deferred, depth),
            "Identifier" => {
                // A bare identifier is not itself behavior — except when it is the single read
                // of an inlined local, whose initializer's events belong here, at the position
                // the read occupies. Emitting at the read site, rather than back at the
                // declaration, keeps the sequence the written-out form has.
                if let Some(init) = self.dataflow.init_of(self.symbol_at(expression)) {
                    self.walk_expr(init, deferred, depth + 1);
                }
            }
            "ClassExpression" => self.class_definition(expression, deferred, depth + 1),
            "ChainExpression" => {
                if let Some(inner) = tree.child(expression, "expression") {
                    match tree.kind(inner) {
                        "CallExpression" => self.walk_call(inner, deferred, depth),
                        "MemberExpression" => self.walk_member_value(inner, deferred, depth),
                        _ => self.walk_expr(inner, deferred, depth),
                    }
                }
            }
            "BinaryExpression" | "LogicalExpression" => {
                for key in ["left", "right"] {
                    if let Some(part) = tree.child(expression, key) {
                        self.walk_expr(part, deferred, depth + 1);
                    }
                }
            }
            "UnaryExpression" => {
                if let Some(argument) = tree.child(expression, "argument") {
                    self.walk_expr(argument, deferred, depth + 1);
                }
            }
            "ConditionalExpression" => {
                for key in ["test", "consequent", "alternate"] {
                    if let Some(part) = tree.child(expression, key) {
                        self.walk_expr(part, deferred, depth + 1);
                    }
                }
            }
            "SequenceExpression" | "TemplateLiteral" => {
                let expressions: Vec<NodeId> = tree
                    .list(expression, "expressions")
                    .into_iter()
                    .flatten()
                    .collect();
                for expression in expressions {
                    self.walk_expr(expression, deferred, depth + 1);
                }
            }
            "ArrayExpression" => {
                let elements: Vec<NodeId> = tree
                    .list(expression, "elements")
                    .into_iter()
                    .flatten()
                    .collect();
                for element in elements {
                    // A spread or a hole contributes no event of its own; the reads inside the
                    // array's plain elements do.
                    if tree.kind(element) != "SpreadElement" {
                        self.walk_expr(element, deferred, depth + 1);
                    }
                }
            }
            "ObjectExpression" => {
                let properties: Vec<NodeId> = tree
                    .list(expression, "properties")
                    .into_iter()
                    .flatten()
                    .collect();
                for property in properties {
                    if tree.kind(property) == "Property" {
                        // A computed key evaluates when the literal does, before its value: its
                        // events belong to the sequence, or two orders would share one trace.
                        if tree.flag(property, "computed") {
                            if let Some(key) = tree.child(property, "key") {
                                self.walk_expr(key, deferred, depth + 1);
                            }
                        }
                        if let Some(value) = tree.child(property, "value") {
                            self.walk_expr(value, deferred, depth + 1);
                        }
                    }
                }
            }
            "ParenthesizedExpression"
            | "TSAsExpression"
            | "TSSatisfiesExpression"
            | "TSNonNullExpression" => {
                if let Some(inner) = tree.child(expression, "expression") {
                    self.walk_expr(inner, deferred, depth);
                }
            }
            _ => {}
        }
    }

    /// Walks a nested callable's body as deferred behavior.
    fn deferred_body(&mut self, function: NodeId, depth: u32) {
        let tree = self.tree();
        let Some(body) = tree.child(function, "body") else {
            return;
        };
        if tree.flag(function, "expression") {
            self.walk_expr(body, true, depth);
            return;
        }
        self.walk_block(body, true, depth);
    }

    /// Walks a block body's statements in order, carrying the caller's deferred state.
    fn walk_block(&mut self, body: NodeId, deferred: bool, depth: u32) {
        let tree = self.tree();
        if tree.kind(body) != "BlockStatement" {
            return;
        }
        let statements: Vec<NodeId> = tree.list(body, "body").into_iter().flatten().collect();
        for statement in statements {
            self.walk_stmt(statement, deferred, depth);
        }
    }

    /// Walks a static member expression standing in value position. The exception to "a member
    /// access is not behavior" is the chai-style property assertion, whose entire meaning is the
    /// member names this descent would otherwise walk straight past. A declined chain descends
    /// without re-offering the recognizer every proper prefix of a chain it has already refused.
    fn walk_member_value(&mut self, member: NodeId, deferred: bool, depth: u32) {
        if depth > MAX_TRACE_DEPTH {
            self.complete = false;
            return;
        }
        if let Some((event, base)) = self.property_chain_event(member, depth) {
            self.push(deferred, event);
            self.walk_call(base, deferred, depth + 1);
            return;
        }
        let tree = self.tree();
        if let Some(object) = tree.child(member, "object") {
            self.walk_under_members(object, deferred, depth);
        }
        // The object evaluates before its key: `object()[key()]` runs `object()` first, and the
        // trace keeps that order.
        if tree.flag(member, "computed") {
            if let Some(key) = tree.child(member, "property") {
                self.walk_expr(key, deferred, depth + 1);
            }
        }
    }

    /// Walks whatever sits underneath a member chain, emitting nothing for the links themselves:
    /// the callee of a call, whose own event already names them, and the object of a member
    /// access whose chain this recognizer declined.
    fn walk_under_members(&mut self, current: NodeId, deferred: bool, depth: u32) {
        if depth > MAX_TRACE_DEPTH {
            self.complete = false;
            return;
        }
        let tree = self.tree();
        match tree.kind(current) {
            "MemberExpression" => {
                // The chain evaluates from its object inward, and a computed key evaluates after
                // the object it reads from: `x()[y()][z()]` runs `x`, then `y`, then `z`.
                if let Some(object) = tree.child(current, "object") {
                    self.walk_under_members(object, deferred, depth + 1);
                }
                if tree.flag(current, "computed") {
                    if let Some(key) = tree.child(current, "property") {
                        self.walk_expr(key, deferred, depth + 1);
                    }
                }
            }
            "ParenthesizedExpression"
            | "TSAsExpression"
            | "TSSatisfiesExpression"
            | "TSNonNullExpression" => {
                if let Some(inner) = tree.child(current, "expression") {
                    self.walk_under_members(inner, deferred, depth);
                }
            }
            _ => self.walk_expr(current, deferred, depth),
        }
    }

    /// Collects one atomic assertion event or ordinary call behavior, respecting nesting and
    /// deferred execution.
    fn walk_call(&mut self, call: NodeId, deferred: bool, depth: u32) {
        if depth > MAX_TRACE_DEPTH {
            self.complete = false;
            return;
        }
        if let Some(event) = self.assertion_event(call, depth) {
            self.push(deferred, event);
            // The whole assertion is one atomic behavior event: its subject and expected values
            // are already captured in the event text, so descending further would count the
            // subject's own calls a second time.
            return;
        }
        let tree = self.tree();
        let callee = tree.child(call, "callee");
        let mut event = format!(
            "call:{}",
            callee
                .map(|callee| self.call_path(callee, depth + 1))
                .unwrap_or_default()
        );
        if let Some(type_arguments) = tree.child(call, "typeArguments") {
            event.push_str(&self.shape(type_arguments));
        }
        event.push_str(&self.arguments_fragment(call, depth + 1));
        self.push(deferred, event);
        if let Some(callee) = callee {
            self.walk_under_members(callee, deferred, depth + 1);
        }
        self.walk_arguments(call, deferred, depth + 1);
    }

    /// One argument's event shape: a spread renders with an explicit `...` prefix — erasing it
    /// would make `render(...adminProps)` and `render(...guestProps)` indistinguishable — and
    /// everything else as its own shape.
    fn argument_shape(&mut self, argument: NodeId) -> String {
        let tree = self.tree();
        if tree.kind(argument) != "SpreadElement" {
            return self.shape(argument);
        }
        match tree.child(argument, "argument") {
            Some(inner) => format!("...{}", self.shape(inner)),
            None => "...?".to_owned(),
        }
    }

    /// The data arguments of a call, rendered into its behavior event as a JSON array. Without
    /// them, `wrapper.find('.name-text')` and `wrapper.find('.unit-value')` emit the identical
    /// event, and two tests querying different things carry the same trace — the largest single
    /// measured cause of near-duplicate false positives. A spread renders with an explicit `...`
    /// prefix; erasing it would make `render(...adminProps)` and `render(...guestProps)`
    /// indistinguishable.
    fn arguments_fragment(&mut self, call: NodeId, _depth: u32) -> String {
        let tree = self.tree();
        let arguments: Vec<NodeId> = tree.list(call, "arguments").into_iter().flatten().collect();
        if arguments.is_empty() {
            return String::new();
        }
        let shapes: Vec<String> = arguments
            .into_iter()
            .map(|argument| self.argument_shape(argument))
            .collect();
        format!(
            " args={}",
            serde_json::to_string(&shapes).unwrap_or_default()
        )
    }

    /// Walks call and constructor argument effects, retaining spread traversal.
    fn walk_arguments(&mut self, call: NodeId, deferred: bool, depth: u32) {
        let tree = self.tree();
        let arguments: Vec<NodeId> = tree.list(call, "arguments").into_iter().flatten().collect();
        for argument in arguments {
            if tree.kind(argument) == "SpreadElement" {
                if let Some(inner) = tree.child(argument, "argument") {
                    self.walk_expr(inner, deferred, depth);
                }
            } else {
                self.walk_expr(argument, deferred, depth);
            }
        }
    }

    /// Retains constructor identity, then collects its nested effects. A callee that names a
    /// local class renders the class by its content, so two constructors over different classes
    /// never share an event while a renamed copy of one class still links; a dependency the trace
    /// cannot prove leaves the trace incomplete.
    fn walk_new(&mut self, expression: NodeId, deferred: bool, depth: u32) {
        if depth > MAX_TRACE_DEPTH {
            self.complete = false;
            return;
        }
        let tree = self.tree();
        let Some(callee) = tree.child(expression, "callee") else {
            return;
        };
        let shape = self.constructor_shape(callee, depth + 1);
        if !self.complete {
            return;
        }
        // The event keeps the callee, its ordered arguments and its type arguments: `new
        // URL('/a', base)` and the '/b' variant do different work, and without the arguments the
        // trace would erase it.
        let mut event = format!("new:{shape}");
        if let Some(type_arguments) = tree.child(expression, "typeArguments") {
            event.push_str(&self.shape(type_arguments));
        }
        event.push_str(&self.arguments_fragment(expression, depth + 1));
        self.push(deferred, event);
        self.walk_under_members(callee, deferred, depth + 1);
        self.walk_arguments(expression, deferred, depth + 1);
    }

    /// The shape a constructor's callee carries: a local class's content, or the ordinary
    /// canonical path.
    fn constructor_shape(&mut self, callee: NodeId, depth: u32) -> String {
        if self.tree().kind(callee) == "Identifier" {
            if let Some(class) = self.local_class(callee) {
                if !self.class_provable(class) {
                    self.complete = false;
                    return String::new();
                }
                if self.walk.class_bytes > MAX_CLASS_RENDER_BYTES {
                    self.complete = false;
                    return String::new();
                }
                let shape = self.shape(class);
                self.walk.class_bytes += shape.len();
                return shape;
            }
        }
        self.call_path(callee, depth)
    }

    /// The class node a constructor's callee names, when it is declared or bound inside this
    /// body: a class declaration, or a `const` bound to a class expression. A class declared
    /// outside needs no expansion — its declaration identity already carries its content.
    fn local_class(&self, callee: NodeId) -> Option<NodeId> {
        let tree = self.tree();
        let symbol = symbol_at(self.parsed, callee)?;
        if !self.root.contains(self.scopes().symbol(symbol).span) {
            return None;
        }
        match &self.scopes().symbol(symbol).declaration {
            crate::lang::Declaration::Class(class) => Some(*class),
            crate::lang::Declaration::Variable(declarator) => {
                let init = tree.child(*declarator, "init")?;
                (tree.kind(init) == "ClassExpression").then_some(init)
            }
            _ => None,
        }
    }

    /// Whether the trace can prove a class's behavior from its content. It cannot when a key is
    /// computed from anything but a literal (its value, and when it evaluates, are unproved), or
    /// when a `with` statement may bind arbitrary names. This is the bounded port of the current
    /// engine's `class_unresolved`: a dependency it cannot name is an abstention, never a claim.
    fn class_provable(&self, class: NodeId) -> bool {
        class_provable(self.tree(), class)
    }

    /// Emits the one definition-time event a class body carries: its heritage, its computed
    /// keys, its static blocks and its static fields run when the class is evaluated; inert
    /// methods add nothing.
    fn class_definition(&mut self, class: NodeId, deferred: bool, depth: u32) {
        if depth > MAX_TRACE_DEPTH {
            self.complete = false;
            return;
        }
        let tree = self.tree();
        let mut effects = Vec::new();
        if let Some(heritage) = tree.child(class, "superClass") {
            effects.push(format!("heritage:{}", self.shape(heritage)));
        }
        let Some(body) = tree.child(class, "body") else {
            return;
        };
        let elements: Vec<NodeId> = tree.list(body, "body").into_iter().flatten().collect();
        for element in elements {
            match tree.kind(element) {
                "StaticBlock" => {
                    let statements: Vec<String> = tree
                        .list(element, "body")
                        .into_iter()
                        .flatten()
                        .map(|statement| self.shape(statement))
                        .collect();
                    effects.push(format!("static-block:{}", statements.join(";")));
                }
                "MethodDefinition" | "PropertyDefinition" | "AccessorProperty" => {
                    if tree.flag(element, "computed") {
                        if let Some(key) = tree.child(element, "key") {
                            effects.push(format!("key:{}", self.shape(key)));
                        }
                    }
                    if tree.flag(element, "static")
                        && matches!(
                            tree.kind(element),
                            "PropertyDefinition" | "AccessorProperty"
                        )
                    {
                        let key = tree
                            .child(element, "key")
                            .map(|key| self.shape(key))
                            .unwrap_or_default();
                        let value = tree
                            .child(element, "value")
                            .map(|value| self.shape(value))
                            .unwrap_or_default();
                        effects.push(format!("static-field:{key}={value}"));
                    }
                }
                _ => {}
            }
        }
        if !effects.is_empty() {
            self.push(
                deferred,
                format!(
                    "class-eval:{}",
                    serde_json::to_string(&effects).unwrap_or_default()
                ),
            );
        }
    }

    /// Flattens a member/call chain into a canonical dotted path. An inner call in a fluent chain
    /// contributes its own type arguments to the outer path, so `expectTypeOf<A>().toEqualTypeOf<B>()`
    /// and `expectTypeOf<C>().toEqualTypeOf<B>()` produce different outer events rather than
    /// relying solely on the separate event the inner call emits. Past the bound the writer's
    /// position token stands in: deeper code can stop a match but never make one.
    fn call_path(&mut self, callee: NodeId, depth: u32) -> String {
        if depth > MAX_TRACE_DEPTH {
            return self.shape(callee);
        }
        let tree = self.tree();
        match tree.kind(callee) {
            "Identifier" => {
                // A read of an inlined local stands for its initializer, here too: `const f =
                // getFn(); f()` evaluates exactly what `getFn()()` does, in the same order, so
                // the two spell the same path.
                if let Some(init) = self
                    .symbol_at(callee)
                    .and_then(|symbol| self.dataflow.inline.get(&symbol))
                    .map(|inline| inline.init)
                {
                    return self.call_path(init, depth + 1);
                }
                // A member-substituted read stands for `object.property`, spelled as a path.
                if let Some((object, property)) = self
                    .symbol_at(callee)
                    .and_then(|symbol| self.dataflow.member.get(&symbol))
                    .cloned()
                {
                    let object_path = self.call_path(object, depth + 1);
                    return format!("{object_path}.{property}");
                }
                self.shape(callee)
            }
            "ThisExpression" => "this".to_owned(),
            "Super" => "super".to_owned(),
            "ParenthesizedExpression"
            | "TSAsExpression"
            | "TSSatisfiesExpression"
            | "TSNonNullExpression" => match tree.child(callee, "expression") {
                Some(inner) => self.call_path(inner, depth + 1),
                None => self.shape(callee),
            },
            "MemberExpression" => {
                let object = tree
                    .child(callee, "object")
                    .map(|object| self.call_path(object, depth + 1))
                    .unwrap_or_default();
                if tree.flag(callee, "computed") {
                    return format!("{object}[]");
                }
                let property = tree
                    .child(callee, "property")
                    .map(|property| match tree.kind(property) {
                        "PrivateIdentifier" => {
                            format!("#{}", tree.text(property, "name").unwrap_or_default())
                        }
                        _ => tree
                            .text(property, "name")
                            .or_else(|| tree.text(property, "value"))
                            .unwrap_or_default()
                            .to_owned(),
                    })
                    .unwrap_or_default();
                format!("{object}.{property}")
            }
            "CallExpression" => {
                let inner = tree
                    .child(callee, "callee")
                    .map(|callee| self.call_path(callee, depth + 1))
                    .unwrap_or_default();
                let type_arguments = tree
                    .child(callee, "typeArguments")
                    .map(|arguments| self.shape(arguments))
                    .unwrap_or_default();
                format!("{inner}{type_arguments}")
            }
            "ChainExpression" => match tree.child(callee, "expression") {
                Some(inner) => self.call_path(inner, depth + 1),
                None => self.shape(callee),
            },
            _ => self.shape(callee),
        }
    }

    /// Recognizes `expect(subject)[.not][.resolves|.rejects].matcher(...expected)`, including the
    /// dialect's qualifier members. Returns `None` for anything else, in which case the call is
    /// treated as an ordinary call event instead.
    fn assertion_event(&mut self, call: NodeId, depth: u32) -> Option<String> {
        if depth > MAX_TRACE_DEPTH {
            return None;
        }
        let tree = self.tree();
        let callee = tree.child(call, "callee")?;
        if tree.kind(callee) != "MemberExpression" || tree.flag(callee, "computed") {
            return None;
        }
        let matcher = tree
            .child(callee, "property")
            .and_then(|property| tree.text(property, "name"))?
            .to_owned();
        let mut links = Vec::new();
        let base = self.unwind_to_call(tree.child(callee, "object")?, &mut links)?;
        // Unlike a chai property chain, the links here are a closed set: anything else between
        // the base and the matcher means this is not an assertion at all, and the call falls
        // through to an ordinary call event.
        let (entry, qualifier_member) = self.assertion_base(base)?;
        let mut modifiers = Vec::new();
        for name in links.iter().rev() {
            modifiers.push(*entry.modifiers.iter().find(|modifier| *modifier == name)?);
        }
        let subject_shape = tree
            .list(base, "arguments")
            .into_iter()
            .flatten()
            .next()
            .map(|subject| {
                if tree.kind(subject) == "SpreadElement" {
                    match tree.child(subject, "argument") {
                        Some(inner) => format!("...{}", self.structural_shape(inner)),
                        None => "...?".to_owned(),
                    }
                } else {
                    self.structural_shape(subject)
                }
            })
            .unwrap_or_else(|| "()".to_owned());
        let mut qualifier = entry.label.to_owned();
        if let Some(member) = qualifier_member {
            qualifier = format!("{qualifier}.{member}");
        }
        for modifier in &modifiers {
            qualifier = format!("{qualifier}.{modifier}");
        }
        let base_type_arguments = tree
            .child(base, "typeArguments")
            .map(|arguments| self.shape(arguments))
            .unwrap_or_default();
        let matcher_type_arguments = tree
            .child(call, "typeArguments")
            .map(|arguments| self.shape(arguments))
            .unwrap_or_default();
        let values: Vec<String> = self.value_shapes(call);
        Some(format!(
            "assert:{qualifier}{base_type_arguments}.{matcher}{matcher_type_arguments}\
             (subject={subject_shape}; value={})",
            serde_json::to_string(&values).unwrap_or_default()
        ))
    }

    /// The expected values of an assertion, each rendered with its literals, spreads included.
    fn value_shapes(&mut self, call: NodeId) -> Vec<String> {
        let tree = self.tree();
        tree.list(call, "arguments")
            .into_iter()
            .flatten()
            .map(|argument| self.argument_shape(argument))
            .collect()
    }

    /// The assertion entry a base call hangs off, with the qualifier member it was spelled
    /// through, if any.
    fn assertion_base(&self, base: NodeId) -> Option<(&'s AssertionEntry, Option<&'s str>)> {
        let tree = self.tree();
        let callee = tree.child(base, "callee")?;
        match tree.kind(callee) {
            "Identifier" => self.assertion_entry(callee).map(|entry| (entry, None)),
            "MemberExpression" if !tree.flag(callee, "computed") => {
                let qualifier = tree
                    .child(callee, "property")
                    .and_then(|property| tree.text(property, "name"))?;
                let object = tree.child(callee, "object")?;
                let entry = self.assertion_entry(object)?;
                entry
                    .qualifiers
                    .contains(&qualifier)
                    .then_some((entry, Some(qualifier)))
            }
            _ => None,
        }
    }

    /// The dialect's assertion entry an identifier spells, when the identifier reads no binding
    /// declared inside the body: a local shadowing the name is not the API.
    fn assertion_entry(&self, identifier: NodeId) -> Option<&'s AssertionEntry> {
        let tree = self.tree();
        let name = tree.text(identifier, "name")?;
        let entry = self.dialect().assertion(name)?;
        let inside = match self.scopes().name_at(tree.span(identifier).start) {
            Some(Name::Reference(Some(symbol))) => {
                self.root.contains(self.scopes().symbol(symbol).span)
            }
            Some(Name::Binding(_)) => true,
            _ => false,
        };
        (!inside).then_some(entry)
    }

    /// Recognizes a chai-style property-chain assertion and returns its event text together with
    /// the base call the chain hangs off. The links are recorded verbatim, the framework's no-op
    /// language included: normalizing them away needs a hardcoded vocabulary of one assertion
    /// library's filler words, and any word missing from that list would silently become
    /// load-bearing. The event is additive — the caller still walks the base call, so every
    /// event the body produced before is produced still.
    fn property_chain_event(&mut self, member: NodeId, depth: u32) -> Option<(String, NodeId)> {
        if depth > MAX_TRACE_DEPTH {
            return None;
        }
        let tree = self.tree();
        // A computed terminal link reads a value, not a name: `expect(false)[ok]` is no more the
        // `ok` assertion than any other dynamic member, and spelling it as one would make two
        // different assertions share an event.
        if tree.flag(member, "computed") {
            return None;
        }
        let mut links = vec![tree
            .child(member, "property")
            .and_then(|property| tree.text(property, "name"))?
            .to_owned()];
        let base = self.unwind_to_call(tree.child(member, "object")?, &mut links)?;
        if links.len() > MAX_ASSERTION_CHAIN {
            return None;
        }
        let (entry, qualifier_member) = self.assertion_base(base)?;
        let mut qualifier = entry.label.to_owned();
        if let Some(member) = qualifier_member {
            qualifier = format!("{qualifier}.{member}");
        }
        links.reverse();
        Some((
            format!("assert:chain:{qualifier}.{}", links.join(".")),
            base,
        ))
    }

    /// Unwinds a member chain to the call it hangs off, collecting the property names on the
    /// way. `None` for any chain that does not end in a plain call.
    fn unwind_to_call(&mut self, mut current: NodeId, links: &mut Vec<String>) -> Option<NodeId> {
        loop {
            let tree = self.tree();
            match tree.kind(current) {
                "MemberExpression" if !tree.flag(current, "computed") => {
                    let property = tree.child(current, "property")?;
                    links.push(tree.text(property, "name")?.to_owned());
                    current = tree.child(current, "object")?;
                }
                "CallExpression" => return Some(current),
                _ => return None,
            }
        }
    }

    /// The symbol an identifier node reads or binds, when the scope analysis recorded one.
    fn symbol_at(&self, node: NodeId) -> Option<SymbolId> {
        self.scopes().symbol_at(self.tree().span(node).start)
    }
}
