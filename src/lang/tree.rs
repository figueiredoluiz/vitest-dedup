//! The owned syntax tree: ESTree nodes with parent links, addressed by [`NodeId`].

use serde_json::{Map, Value};
use std::collections::HashMap;

/// A node of a [`Tree`]. Ids follow source order: a node's id is greater than its parent's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

/// Byte offsets into the source, end exclusive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    /// First byte.
    pub start: u32,
    /// One past the last byte.
    pub end: u32,
}

impl Span {
    /// Whether `other` lies within this span.
    pub fn contains(self, other: Span) -> bool {
        self.start <= other.start && other.end <= self.end
    }
}

/// A property of a node.
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    /// A child node.
    Node(NodeId),
    /// A list of fields: child nodes, `null` holes, or values.
    List(Vec<Field>),
    /// An object that is not a node (a template element's `value`, a regex's `pattern` and
    /// `flags`).
    Object(Vec<(Box<str>, Field)>),
    /// A string, number, boolean or `null`.
    Value(Value),
}

#[derive(Debug)]
struct Node {
    kind: Box<str>,
    span: Span,
    parent: Option<NodeId>,
    fields: Vec<(Box<str>, Field)>,
}

/// An ESTree syntax tree.
#[derive(Debug)]
pub struct Tree {
    nodes: Vec<Node>,
    /// Every node at each span, outermost first.
    at: HashMap<Span, Vec<NodeId>>,
}

const NULL: Field = Field::Value(Value::Null);

impl Tree {
    /// The tree an ESTree JSON value spells, or `None` when the value is not a node.
    pub(super) fn from_json(value: Value) -> Option<Self> {
        let mut tree = Tree {
            nodes: Vec::new(),
            at: HashMap::new(),
        };
        let Value::Object(map) = value else {
            return None;
        };
        tree.node(map, None)?;
        Some(tree)
    }

    /// Adds the node `map` spells, under `parent`, and returns its id.
    fn node(&mut self, mut map: Map<String, Value>, parent: Option<NodeId>) -> Option<NodeId> {
        let Some(Value::String(kind)) = map.remove("type") else {
            return None;
        };
        let offset =
            |value: Option<Value>| value.and_then(|value| value.as_u64()).map(|n| n as u32);
        let span = Span {
            start: offset(map.remove("start"))?,
            end: offset(map.remove("end"))?,
        };
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Node {
            kind: kind.into(),
            span,
            parent,
            fields: Vec::new(),
        });
        self.at.entry(span).or_default().push(id);
        let fields = map
            .into_iter()
            .map(|(key, value)| (key.into_boxed_str(), self.field(value, id)))
            .collect();
        self.nodes[id.0 as usize].fields = fields;
        Some(id)
    }

    /// The field `value` spells, its nodes added under `parent`.
    fn field(&mut self, value: Value, parent: NodeId) -> Field {
        match value {
            Value::Array(items) => Field::List(
                items
                    .into_iter()
                    .map(|item| self.field(item, parent))
                    .collect(),
            ),
            Value::Object(map) if map.get("type").is_some_and(Value::is_string) => {
                match self.node(map, Some(parent)) {
                    Some(id) => Field::Node(id),
                    None => NULL,
                }
            }
            Value::Object(map) => Field::Object(
                map.into_iter()
                    .map(|(key, value)| (key.into_boxed_str(), self.field(value, parent)))
                    .collect(),
            ),
            other => Field::Value(other),
        }
    }

    /// The root node, the `Program`.
    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    /// Every node, in source order (parents before children).
    pub fn nodes(&self) -> impl Iterator<Item = NodeId> {
        (0..self.nodes.len() as u32).map(NodeId)
    }

    /// The nodes inside `id`, in source order: ids follow source order, so they are the ids after
    /// it up to the first one outside its span.
    pub fn descendants(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let span = self.span(id);
        (id.0 + 1..self.nodes.len() as u32)
            .map(NodeId)
            .take_while(move |&node| span.contains(self.span(node)))
    }

    /// The node's ESTree `type`.
    pub fn kind(&self, id: NodeId) -> &str {
        &self.nodes[id.0 as usize].kind
    }

    /// The node's span.
    pub fn span(&self, id: NodeId) -> Span {
        self.nodes[id.0 as usize].span
    }

    /// The node holding this one, `None` for the root.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0 as usize].parent
    }

    /// The nodes holding this one, innermost first.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.parent(id), |&node| self.parent(node))
    }

    /// The node's fields, in serialization order.
    pub fn fields(&self, id: NodeId) -> &[(Box<str>, Field)] {
        &self.nodes[id.0 as usize].fields
    }

    /// The field named `key`.
    pub fn field_of(&self, id: NodeId, key: &str) -> &Field {
        self.fields(id)
            .iter()
            .find(|(name, _)| &**name == key)
            .map_or(&NULL, |(_, field)| field)
    }

    /// The child node in field `key`.
    pub fn child(&self, id: NodeId, key: &str) -> Option<NodeId> {
        match self.field_of(id, key) {
            Field::Node(child) => Some(*child),
            _ => None,
        }
    }

    /// The entries of list field `key`: a node, or `None` for a hole or a non-node.
    pub fn list(&self, id: NodeId, key: &str) -> Vec<Option<NodeId>> {
        match self.field_of(id, key) {
            Field::List(items) => items
                .iter()
                .map(|item| match item {
                    Field::Node(child) => Some(*child),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The string in field `key`.
    pub fn text(&self, id: NodeId, key: &str) -> Option<&str> {
        match self.field_of(id, key) {
            Field::Value(Value::String(text)) => Some(text),
            _ => None,
        }
    }

    /// Whether field `key` is `true`.
    pub fn flag(&self, id: NodeId, key: &str) -> bool {
        matches!(self.field_of(id, key), Field::Value(Value::Bool(true)))
    }

    /// The outermost node at `span`.
    pub fn outermost(&self, span: Span) -> Option<NodeId> {
        self.at.get(&span).and_then(|nodes| nodes.first().copied())
    }

    /// The innermost node at `span` whose type is one of `kinds`.
    pub fn find(&self, span: Span, kinds: &[&str]) -> Option<NodeId> {
        self.at
            .get(&span)?
            .iter()
            .rev()
            .copied()
            .find(|&node| kinds.contains(&self.kind(node)))
    }

    /// The expression `id` evaluates to with parentheses and TypeScript-only wrappers removed.
    pub fn inner(&self, mut id: NodeId) -> NodeId {
        while matches!(
            self.kind(id),
            "ParenthesizedExpression"
                | "TSAsExpression"
                | "TSSatisfiesExpression"
                | "TSNonNullExpression"
                | "TSTypeAssertion"
                | "TSInstantiationExpression"
        ) {
            match self.child(id, "expression") {
                Some(inner) => id = inner,
                None => break,
            }
        }
        id
    }

    /// The node `id` sits in, past parentheses and TypeScript-only wrappers.
    pub fn outer_parent(&self, id: NodeId) -> Option<NodeId> {
        self.ancestors(id).find(|&node| {
            !matches!(
                self.kind(node),
                "ParenthesizedExpression"
                    | "TSAsExpression"
                    | "TSSatisfiesExpression"
                    | "TSNonNullExpression"
                    | "TSTypeAssertion"
                    | "TSInstantiationExpression"
            )
        })
    }

    /// Whether the node is a function: a declaration, an expression or an arrow.
    pub fn is_function(&self, id: NodeId) -> bool {
        matches!(
            self.kind(id),
            "FunctionDeclaration" | "FunctionExpression" | "ArrowFunctionExpression"
        )
    }
}
