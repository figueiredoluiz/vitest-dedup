//! Whether a whole suite is known to be active at collection.

use super::Normalizer;
use crate::framework::Chain;
use crate::lang::{property_name, Field, NodeId};
use serde_json::Value;

impl Normalizer<'_> {
    /// Requires enabling literal conditions and no option that might skip the suite.
    pub fn active_suite(&self, callee: NodeId, arguments: &[NodeId]) -> bool {
        let tree = self.tree;
        let boolean = |node| match tree.field_of(tree.inner(node), "value") {
            Field::Value(Value::Bool(value)) => Some(*value),
            _ => None,
        };
        for node in std::iter::once(callee).chain(tree.descendants(callee)) {
            if tree.kind(node) != "CallExpression" {
                continue;
            }
            let Some(member) = tree.child(node, "callee").map(|node| tree.inner(node)) else {
                return false;
            };
            let meaning = tree
                .child(member, "property")
                .and_then(|property| tree.text(property, "name"))
                .and_then(|name| self.setting.dialect.chain_member(name));
            let enabling = match meaning {
                Some(Chain::RunWhen) => true,
                Some(Chain::SkipWhen) => false,
                _ => continue,
            };
            let values = tree.list(node, "arguments");
            if values.len() != 1 || values[0].and_then(boolean) != Some(enabling) {
                return false;
            }
        }
        arguments.iter().skip(1).all(|&argument| {
            let argument = tree.inner(argument);
            match tree.kind(argument) {
                "ArrowFunctionExpression" | "FunctionExpression" | "Literal" => true,
                "ObjectExpression" => {
                    tree.list(argument, "properties")
                        .into_iter()
                        .flatten()
                        .all(|property| {
                            if tree.kind(property) != "Property" || tree.flag(property, "computed")
                            {
                                return false;
                            }
                            let Some(name) = property_name(tree, property) else {
                                return false;
                            };
                            !matches!(
                                self.setting.dialect.chain_member(name),
                                Some(Chain::Skip | Chain::Todo)
                            ) || tree.child(property, "value").and_then(boolean) == Some(false)
                        })
                }
                _ => false,
            }
        })
    }
}
