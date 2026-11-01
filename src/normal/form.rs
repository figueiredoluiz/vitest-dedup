//! The normal form of a node, and the predicates it is decided by.

use super::{digest, Normalizer, Walk};
use crate::framework::Dialect;
use crate::lang::{Field, Name, NodeId, Tree};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt::Write as _;

/// ESTree keys that carry source spelling or TypeScript-only information.
const DROPPED: [&str; 12] = [
    "raw",
    "loc",
    "typeAnnotation",
    "returnType",
    "shorthand",
    "accessibility",
    "declare",
    "definite",
    "readonly",
    "override",
    "abstract",
    "range",
];
/// Keys `DROPPED` erases from runtime code but keeps inside a type-level declaration, where they
/// are what a type test checks.
const TYPE_KEYS: [&str; 2] = ["typeAnnotation", "returnType"];

/// How many nodes deep a normal form is written. The parser admits far deeper chains on a stack of
/// its own; a form is written on its caller's stack, where a 2 MiB test thread must hold it.
pub(super) const MAX_FORM_DEPTH: usize = 256;

/// How many bytes one walk's class expansion may write. Depth bounds the recursion and not the
/// output: a class reading another twice doubles it per level. Past this a read renders the local's
/// name and the trace is incomplete.
pub(crate) const MAX_CLASS_RENDER_BYTES: usize = 256 * 1024;

/// The fields of `node` that are child nodes or lists of them, for the predicates below.
fn children(tree: &Tree, node: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    for (_, field) in tree.fields(node) {
        collect(field, &mut out);
    }
    out
}

/// Adds the nodes `field` holds, at any depth of lists and objects, to `out`.
fn collect(field: &Field, out: &mut Vec<NodeId>) {
    match field {
        Field::Node(child) => out.push(*child),
        Field::List(items) => items.iter().for_each(|item| collect(item, out)),
        Field::Object(fields) => fields.iter().for_each(|(_, field)| collect(field, out)),
        Field::Value(_) => {}
    }
}

/// A literal or constant template carries an exact selector without a value proof.
fn static_selector(tree: &Tree, node: NodeId) -> bool {
    let node = tree.inner(node);
    tree.kind(node) == "Literal"
        || (tree.kind(node) == "TemplateLiteral" && tree.list(node, "expressions").is_empty())
}

/// Known source-sensitive names and potentially lossy text cannot establish consolidation.
fn uncertain_text(dialect: &Dialect, text: &str) -> bool {
    text.contains('\u{fffd}')
        || dialect
            .source_keyed_matchers
            .iter()
            .any(|fragment| text.contains(fragment))
}

/// Source-sensitive matchers, unknown selectors and potentially lossy text refuse consolidation.
/// Local dependency reads and aliases share one graph; cycles are visited once.
pub(super) fn parameterization_uncertain(normalizer: &Normalizer, node: NodeId) -> bool {
    let tree = normalizer.tree;
    let mut pending = vec![node];
    let mut seen = HashSet::new();
    while let Some(node) = pending.pop() {
        if !seen.insert(node) {
            continue;
        }
        if tree.kind(node) == "ImportExpression"
            && (tree
                .child(node, "source")
                .is_none_or(|source| !static_selector(tree, source))
                || !is_empty(tree.field_of(node, "options")))
        {
            return true;
        }
        let property_field = match tree.kind(node) {
            "MemberExpression" => Some("property"),
            "Property"
                if tree
                    .parent(node)
                    .is_some_and(|parent| tree.kind(parent) == "ObjectPattern") =>
            {
                Some("key")
            }
            _ => None,
        };
        if property_field.is_some()
            && matches!(value(tree, node, "computed"), Value::Bool(true))
            && property_field
                .and_then(|field| tree.child(node, field))
                .is_none_or(|property| !static_selector(tree, property))
        {
            return true;
        }
        if tree
            .text(node, "name")
            .or_else(|| tree.text(node, "value"))
            .is_some_and(|name| uncertain_text(normalizer.setting.dialect, name))
        {
            return true;
        }
        if let Some(declaration) = (tree.kind(node) == "Identifier")
            .then(|| normalizer.scopes.symbol_at(tree.span(node).start))
            .flatten()
            .and_then(|symbol| normalizer.scopes.declaration_node(symbol))
        {
            pending.push(declaration);
        }
        if tree.kind(node) == "TemplateElement"
            && quasi_text(tree, node, "cooked")
                .as_str()
                .is_some_and(|name| uncertain_text(normalizer.setting.dialect, name))
        {
            return true;
        }
        pending.extend(children(tree, node));
    }
    false
}

/// Whether `node` can observe a declaration's name: `.name`, `.constructor`, `.toString`, or a
/// matcher the dialect says prints names.
pub(super) fn observes_names(tree: &Tree, node: NodeId, dialect: &Dialect) -> bool {
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        let matcher = tree.text(node, "name").is_some_and(|name| {
            dialect
                .name_observing_matchers
                .iter()
                .any(|marker| name.contains(marker))
        });
        let member = tree.kind(node) == "MemberExpression"
            && tree.child(node, "property").is_some_and(|property| {
                matches!(
                    tree.text(property, "name")
                        .or_else(|| tree.text(property, "value")),
                    Some("name" | "constructor" | "toString")
                )
            });
        if matcher || member {
            return true;
        }
        pending.extend(children(tree, node));
    }
    false
}

/// Whether evaluating `node` may create or change an object: a call, `new`, `await` or an
/// assignment outside a nested function. A call whose value is a registration function
/// (`test.extend(...)`) creates nothing a test can tell apart, unless its arguments do.
pub(super) fn creates(normalizer: &Normalizer, node: NodeId) -> bool {
    let tree = normalizer.tree;
    any_inside(node, |node| match tree.kind(node) {
        "FunctionExpression" | "ArrowFunctionExpression" | "ClassBody" => Visit::Skip,
        "CallExpression" if normalizer.structure.registration_values.contains(&node) => {
            Visit::Into(tree.list(node, "arguments").into_iter().flatten().collect())
        }
        "CallExpression"
        | "NewExpression"
        | "AwaitExpression"
        | "TaggedTemplateExpression"
        | "UpdateExpression"
        | "AssignmentExpression"
        | "YieldExpression"
        | "ImportExpression" => Visit::Found,
        _ => Visit::Into(children(tree, node)),
    })
}

/// What [`any_inside`] does at a node.
enum Visit {
    /// The predicate holds.
    Found,
    /// Nothing below this node counts.
    Skip,
    /// Look at these nodes next.
    Into(Vec<NodeId>),
}

/// Whether `visit` finds a node at or below `node`. A loop rather than recursion: the parser admits
/// nesting deeper than any caller's stack holds.
fn any_inside(node: NodeId, visit: impl Fn(NodeId) -> Visit) -> bool {
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        match visit(node) {
            Visit::Found => return true,
            Visit::Skip => {}
            Visit::Into(next) => pending.extend(next),
        }
    }
    false
}

/// Whether evaluating `node` makes an object literal, array, function, class or regular expression:
/// each evaluation is a new object, so two equal initializers are still two identities.
pub(super) fn allocates(tree: &Tree, node: NodeId) -> bool {
    any_inside(node, |node| match tree.kind(node) {
        "ObjectExpression"
        | "ArrayExpression"
        | "FunctionExpression"
        | "ArrowFunctionExpression"
        | "ClassExpression" => Visit::Found,
        "Literal" if matches!(tree.field_of(node, "regex"), Field::Object(_)) => Visit::Found,
        "Literal" => Visit::Skip,
        _ => Visit::Into(children(tree, node)),
    })
}

/// A template element's `raw` or `cooked` text.
fn quasi_text(tree: &Tree, element: NodeId, key: &str) -> Value {
    match tree.field_of(element, "value") {
        Field::Object(fields) => {
            fields
                .iter()
                .find(|(name, _)| &**name == key)
                .map_or(Value::Null, |(_, field)| match field {
                    Field::Value(value) => value.clone(),
                    _ => Value::Null,
                })
        }
        _ => Value::Null,
    }
}

/// Whether a template literal is the one a tag is called with.
fn tagged(tree: &Tree, literal: NodeId) -> bool {
    tree.parent(literal).is_some_and(|parent| {
        tree.kind(parent) == "TaggedTemplateExpression"
            && tree.child(parent, "quasi") == Some(literal)
    })
}

/// Whether a function body reads the function's own `this`, `arguments`, `super` or `new.target`.
pub(super) fn binds_this(tree: &Tree, node: NodeId) -> bool {
    any_inside(node, |node| match tree.kind(node) {
        "ThisExpression" | "Super" | "MetaProperty" => Visit::Found,
        "Identifier" if tree.text(node, "name") == Some("arguments") => Visit::Found,
        "Identifier" | "FunctionExpression" | "FunctionDeclaration" | "ClassBody" => Visit::Skip,
        _ => Visit::Into(children(tree, node)),
    })
}

/// JSX text as it renders: lines trimmed where they meet a line break, blank lines dropped.
pub(super) fn jsx_text(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len() - 1;
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let line = if index > 0 {
                line.trim_start_matches([' ', '\t', '\r'])
            } else {
                line
            };
            if index < last {
                line.trim_end_matches([' ', '\t', '\r'])
            } else {
                line
            }
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A number as JavaScript's `String(n)` spells it, which is the property key it names.
pub(super) fn js_number(number: f64) -> String {
    ryu_js::Buffer::new().format(number).to_owned()
}

/// The value field of a node, as JSON.
fn value(tree: &Tree, node: NodeId, key: &str) -> Value {
    match tree.field_of(node, key) {
        Field::Value(value) => value.clone(),
        _ => Value::Null,
    }
}

/// The kind a literal's JSON value names, for the structural rendering.
fn literal_kind(value: Value) -> &'static str {
    match value {
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Bool(_) => "boolean",
        _ => "null",
    }
}

/// Whether a field is absent in effect: `null`, an empty list or `false`.
fn is_empty(field: &Field) -> bool {
    match field {
        Field::Value(Value::Null | Value::Bool(false)) => true,
        Field::List(items) => items.is_empty(),
        _ => false,
    }
}

impl Normalizer<'_> {
    /// Writes the normal form of a field.
    fn write_field(&mut self, field: &Field, walk: &mut Walk, out: &mut String) {
        match field {
            Field::Node(child) => self.write_node(*child, walk, out),
            Field::List(items) => {
                out.push('[');
                for item in items {
                    // Whitespace-only JSX text renders nothing.
                    if let Field::Node(child) = item {
                        if self.tree.kind(*child) == "JSXText"
                            && jsx_text(self.tree.text(*child, "value").unwrap_or_default())
                                .is_empty()
                        {
                            continue;
                        }
                    }
                    self.write_field(item, walk, out);
                    out.push(',');
                }
                out.push(']');
            }
            Field::Object(fields) => {
                out.push('(');
                let mut keys: Vec<&(Box<str>, Field)> = fields
                    .iter()
                    .filter(|(key, _)| !DROPPED.contains(&&**key))
                    .collect();
                keys.sort_by(|left, right| left.0.cmp(&right.0));
                for (key, field) in keys {
                    let _ = write!(out, " {key}:");
                    self.write_field(field, walk, out);
                }
                out.push(')');
            }
            Field::Value(value) => out.push_str(&value.to_string()),
        }
    }

    /// Writes the normal form of a node. Past [`MAX_FORM_DEPTH`] levels, counted across the nested
    /// walks of the declarations a form reads, it writes a token only this node, read with these
    /// substitutions (a table row's cells), writes: deeper code can stop a match but never make one.
    pub(super) fn write_node(&mut self, node: NodeId, walk: &mut Walk, out: &mut String) {
        if self.nesting >= MAX_FORM_DEPTH {
            // A declaration's identity is cached and equals only itself, so its depth is not the
            // reader's: two reads of it read the same code.
            self.truncated |= !self.resolving;
            return self.write_deep(node, walk, out);
        }
        self.nesting += 1;
        self.write_kind(node, walk, out);
        self.nesting -= 1;
    }

    /// The token [`Self::write_node`] writes past the depth bound. Never inlined: the recursion's
    /// frame stays the size of what it needs below the bound.
    #[inline(never)]
    fn write_deep(&self, node: NodeId, walk: &Walk, out: &mut String) {
        let span = self.tree.span(node);
        let mut substitutions: Vec<_> = walk.substitutions.iter().collect();
        substitutions.sort();
        let context = format!("{substitutions:?}{}", walk.bound_parameters);
        let _ = write!(
            out,
            "(deep {}:{}:{}:{:x})",
            self.path.display(),
            span.start,
            span.end,
            digest(&context)
        );
    }

    /// Writes terminal forms outside the recursive frame so their formatting temporaries do not
    /// consume stack space at every level of a deep expression.
    #[inline(never)]
    fn write_atom(&mut self, node: NodeId, walk: &mut Walk, out: &mut String) -> bool {
        let tree = self.tree;
        match tree.kind(node) {
            "PrivateIdentifier" => {
                let name = tree.text(node, "name").unwrap_or_default();
                let owner = walk
                    .privates
                    .iter()
                    .rev()
                    .enumerate()
                    .find_map(|(distance, names)| {
                        names
                            .iter()
                            .position(|declared| declared == name)
                            .map(|index| (distance, index))
                    });
                match owner {
                    Some((distance, index)) => {
                        let _ = write!(out, "#p{distance}.{index}");
                    }
                    None => {
                        let _ = write!(out, "#?{name}");
                    }
                }
                return true;
            }
            // A tag reads the raw text, so `` String.raw`\n` `` is not a line break.
            "TemplateElement"
                if tree
                    .parent(node)
                    .is_some_and(|literal| tagged(tree, literal)) =>
            {
                let _ = write!(out, "R{}", quasi_text(tree, node, "raw"));
                return true;
            }
            // An untagged constant template is the string it spells. Runtime structural forms
            // keep its kind; flat forms of type-level values keep the value too.
            "TemplateLiteral"
                if tree.list(node, "expressions").is_empty() && !tagged(tree, node) =>
            {
                if walk.structural && (walk.trace || walk.in_type == 0) {
                    let _ = write!(out, "L<string>");
                } else {
                    let cooked = tree
                        .list(node, "quasis")
                        .first()
                        .copied()
                        .flatten()
                        .map(|quasi| quasi_text(tree, quasi, "cooked"))
                        .unwrap_or(Value::Null);
                    let _ = write!(out, "L{cooked}");
                }
                return true;
            }
            "Literal" => {
                if walk.structural && (walk.trace || walk.in_type == 0) {
                    // A bigint or a regexp carries no JSON `value`, so its kind comes from the
                    // field that holds it: `1n`, `/a/` and `null` are three different subjects.
                    let kind = if !is_empty(tree.field_of(node, "regex")) {
                        "regexp"
                    } else if !is_empty(tree.field_of(node, "bigint")) {
                        "bigint"
                    } else {
                        literal_kind(value(tree, node, "value"))
                    };
                    let _ = write!(out, "L<{kind}>");
                } else {
                    let literal = value(tree, node, "value");
                    let _ = write!(out, "L{literal}");
                    for extra in ["regex", "bigint"] {
                        let field = tree.field_of(node, extra);
                        if !is_empty(field) {
                            self.write_field(field, walk, out);
                            break;
                        }
                    }
                    if literal.is_null() {
                        out.push_str(&value(tree, node, "raw").to_string());
                    }
                }
                return true;
            }
            "JSXText" => {
                let _ = write!(
                    out,
                    "J{:?}",
                    jsx_text(tree.text(node, "value").unwrap_or_default())
                );
                return true;
            }
            _ => {}
        }
        false
    }

    /// [`Self::write_node`] below the depth bound.
    fn write_kind(&mut self, node: NodeId, walk: &mut Walk, out: &mut String) {
        let tree = self.tree;
        let kind = tree.kind(node);
        match kind {
            "TSAsExpression"
            | "TSSatisfiesExpression"
            | "TSNonNullExpression"
            | "TSTypeAssertion"
            | "ParenthesizedExpression" => {
                if let Some(inner) = tree.child(node, "expression") {
                    return self.write_node(inner, walk, out);
                }
            }
            "Identifier" | "JSXIdentifier" => return self.write_identifier(node, walk, out),
            _ => {}
        }
        if self.write_atom(node, walk, out) {
            return;
        }
        // `function () {}` is `() => {}` when it never reads its own `this`, `arguments` or
        // `new.target`, and is the callback being normalized, which the runner only calls. Any
        // other function could be constructed or have its `prototype` read, which an arrow cannot.
        let arrow = kind == "FunctionExpression"
            && tree.span(node) == walk.root
            && tree.child(node, "id").is_none()
            && !tree.flag(node, "generator")
            && tree
                .child(node, "body")
                .is_some_and(|body| !binds_this(tree, body));
        out.push('(');
        out.push_str(if arrow {
            "ArrowFunctionExpression"
        } else {
            kind
        });
        let class_body = kind == "ClassBody";
        if class_body {
            let declared = tree
                .list(node, "body")
                .into_iter()
                .flatten()
                .filter_map(|element| tree.child(element, "key"))
                .filter(|key| tree.kind(*key) == "PrivateIdentifier")
                .filter_map(|key| tree.text(key, "name").map(str::to_owned))
                .collect();
            walk.privates.push(declared);
        }
        // `() => x` is `() => { return x }`.
        let concise = kind == "ArrowFunctionExpression" && tree.flag(node, "expression");
        // `{ __proto__ }` defines an own property, as `{ ['__proto__']: x }` does; `{ __proto__: x }`
        // sets the prototype.
        let mut computed = tree.flag(node, "computed")
            || (tree.flag(node, "shorthand")
                && tree
                    .child(node, "key")
                    .and_then(|key| tree.text(key, "name"))
                    == Some("__proto__"));
        let type_level = kind.starts_with("TS");
        walk.in_type += usize::from(type_level);
        let function = matches!(kind, "ArrowFunctionExpression" | "FunctionExpression");
        let mut keys: Vec<&(Box<str>, Field)> = tree
            .fields(node)
            .iter()
            .filter(|(key, _)| {
                let key = &**key;
                if key == "expression" {
                    return !function;
                }
                !DROPPED.contains(&key) || (walk.in_type > 0 && TYPE_KEYS.contains(&key))
            })
            .collect();
        keys.sort_by(|left, right| left.0.cmp(&right.0));
        let keyed = matches!(
            kind,
            "Property" | "MethodDefinition" | "PropertyDefinition" | "AccessorProperty"
        );
        let property_key = |key: &str| {
            (keyed && key == "key") || (kind == "MemberExpression" && key == "property")
        };
        // `{ 'a': 1 }`, `{ a: 1 }` and `{ ['a']: 1 }` define the same key; `o['a']` reads `o.a`.
        // Only a number or string key is spelled here; `[true]`, `[null]` and the rest keep theirs.
        let literal_key = |child: NodeId| {
            tree.kind(child) == "Literal"
                && matches!(
                    value(tree, child, "value"),
                    Value::Number(_) | Value::String(_)
                )
                && !(computed && keyed && tree.text(child, "value") == Some("__proto__"))
        };
        let literal = keys.iter().any(|(key, field)| {
            property_key(key) && matches!(field, Field::Node(child) if literal_key(*child))
        });
        if computed && literal {
            computed = false;
        }
        for (key, field) in keys {
            let _ = write!(out, " {key}:");
            let child = match field {
                Field::Node(child) => Some(*child),
                _ => None,
            };
            if &**key == "computed" {
                out.push_str(if computed { "true" } else { "false" });
            } else if &**key == "params"
                && walk.bound_parameters
                && tree.span(node).start == walk.root.start
            {
                out.push_str("[]");
            } else if let Some(child) = child.filter(|child| {
                property_key(key) && !computed && tree.kind(*child) != "PrivateIdentifier"
            }) {
                let name = match value(tree, child, "value") {
                    Value::Number(number) => js_number(number.as_f64().unwrap_or(f64::NAN)),
                    Value::String(text) => text,
                    _ => tree.text(child, "name").unwrap_or_default().to_owned(),
                };
                let _ = write!(out, "#{name}");
            } else if let Some(child) = child.filter(|_| {
                kind == "MetaProperty"
                    || (kind == "JSXAttribute" && &**key == "name")
                    || &**key == "label"
            }) {
                let _ = write!(out, "#{}", tree.text(child, "name").unwrap_or_default());
            } else if let Some(child) = child.filter(|_| concise && &**key == "body") {
                out.push_str("(BlockStatement body:[(ReturnStatement argument:");
                self.write_node(child, walk, out);
                // The trailing comma is the one a written-out statement list carries.
                out.push_str("),])");
            } else {
                let structural = walk.structural;
                if !walk.trace
                    && (property_key(key) || (kind == "ImportExpression" && &**key == "source"))
                {
                    walk.structural = false;
                }
                self.write_field(field, walk, out);
                walk.structural = structural;
            }
        }
        if class_body {
            walk.privates.pop();
        }
        walk.in_type -= usize::from(type_level);
        out.push(')');
    }

    /// Writes an identifier: a local by its order of appearance, anything else by what declares it.
    fn write_identifier(&mut self, node: NodeId, walk: &mut Walk, out: &mut String) {
        let tree = self.tree;
        let name = tree.text(node, "name").unwrap_or_default();
        match self.scopes.name_at(tree.span(node).start) {
            None => {
                let _ = write!(out, "#{name}");
            }
            Some(Name::Reference(None) | Name::Binding(None)) => {
                let _ = write!(out, "g:{name}");
            }
            Some(Name::Binding(Some(symbol)) | Name::Reference(Some(symbol))) => {
                // A read of an inlined local renders the initializer's form, where the read
                // stands, with the numbering the walk has reached — exactly what the read site's
                // own event walk does for the events. Rendering the local's own number instead
                // would erase the initializer's content from every shape.
                if let Some(init) = walk.inline.get(&symbol) {
                    let init = *init;
                    // Written in place, not through a fresh `form_of`: the current nesting count
                    // carries into the expansion, so a chain of single-use locals stays under one
                    // depth bound — a chain a hundred links long renders past the bound's position
                    // token instead of recursing once per link with no bound at all.
                    self.write_node(init, walk, out);
                } else if let Some(class) = walk
                    .classes
                    .get(&symbol)
                    .copied()
                    .filter(|_| walk.class_bytes <= MAX_CLASS_RENDER_BYTES)
                {
                    // The same in-place rule: a class a shape names renders by its content, so
                    // two constructions over different classes never share one. The class's own
                    // name is taken out for the duration: its content's references to itself
                    // render as the local, or the expansion would never end.
                    let start = out.len();
                    walk.classes.remove(&symbol);
                    self.write_node(class, walk, out);
                    walk.classes.insert(symbol, class);
                    walk.class_bytes += out.len() - start;
                } else {
                    walk.class_budget_spent |= walk.classes.contains_key(&symbol);
                    let mut text = self.identifier(symbol, walk);
                    if let Some(suffix) = walk.suffixes.get(&symbol) {
                        text.push_str(suffix);
                    }
                    out.push_str(&text);
                }
            }
        }
        for key in ["decorators", "typeAnnotation", "optional"] {
            if key != "decorators" && walk.in_type == 0 {
                continue;
            }
            let field = tree.field_of(node, key);
            if !is_empty(field) {
                let _ = write!(out, " {key}:");
                self.write_field(field, walk, out);
            }
        }
    }
}
