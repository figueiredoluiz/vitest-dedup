//! The normal form of a node, and the predicates it is decided by.

use super::{Normalizer, Walk};
use crate::framework::Dialect;
use crate::lang::{Field, Name, NodeId, Tree};
use serde_json::Value;
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
    match tree.kind(node) {
        "FunctionExpression" | "ArrowFunctionExpression" | "ClassBody" => false,
        "CallExpression" if normalizer.structure.registration_values.contains(&node) => tree
            .list(node, "arguments")
            .into_iter()
            .flatten()
            .any(|argument| creates(normalizer, argument)),
        "CallExpression"
        | "NewExpression"
        | "AwaitExpression"
        | "TaggedTemplateExpression"
        | "UpdateExpression"
        | "AssignmentExpression"
        | "YieldExpression"
        | "ImportExpression" => true,
        _ => children(tree, node)
            .into_iter()
            .any(|child| creates(normalizer, child)),
    }
}

/// Whether evaluating `node` makes an object literal, array, function, class or regular expression:
/// each evaluation is a new object, so two equal initializers are still two identities.
pub(super) fn allocates(tree: &Tree, node: NodeId) -> bool {
    match tree.kind(node) {
        "ObjectExpression"
        | "ArrayExpression"
        | "FunctionExpression"
        | "ArrowFunctionExpression"
        | "ClassExpression" => true,
        "Literal" => matches!(tree.field_of(node, "regex"), Field::Object(_)),
        _ => children(tree, node)
            .into_iter()
            .any(|child| allocates(tree, child)),
    }
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
fn binds_this(tree: &Tree, node: NodeId) -> bool {
    match tree.kind(node) {
        "ThisExpression" | "Super" | "MetaProperty" => true,
        "Identifier" => tree.text(node, "name") == Some("arguments"),
        "FunctionExpression" | "FunctionDeclaration" | "ClassBody" => false,
        _ => children(tree, node)
            .into_iter()
            .any(|child| binds_this(tree, child)),
    }
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

    /// Writes the normal form of a node.
    pub(super) fn write_node(&mut self, node: NodeId, walk: &mut Walk, out: &mut String) {
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
                return;
            }
            // A tag reads the raw text, so `` String.raw`\n` `` is not a line break.
            "TemplateElement"
                if tree
                    .parent(node)
                    .is_some_and(|literal| tagged(tree, literal)) =>
            {
                let _ = write!(out, "R{}", quasi_text(tree, node, "raw"));
                return;
            }
            // `` `text` `` is the string it spells.
            "TemplateLiteral"
                if tree.list(node, "expressions").is_empty() && !tagged(tree, node) =>
            {
                let cooked = tree
                    .list(node, "quasis")
                    .first()
                    .copied()
                    .flatten()
                    .map(|quasi| quasi_text(tree, quasi, "cooked"))
                    .unwrap_or(Value::Null);
                let _ = write!(out, "L{cooked}");
                return;
            }
            "Literal" => {
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
                return;
            }
            "JSXText" => {
                let _ = write!(
                    out,
                    "J{:?}",
                    jsx_text(tree.text(node, "value").unwrap_or_default())
                );
                return;
            }
            _ => {}
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
                self.write_field(field, walk, out);
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
                let text = self.identifier(symbol, walk);
                out.push_str(&text);
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
