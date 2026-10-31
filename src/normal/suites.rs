//! Whether a whole suite is known to be active at collection.

use super::Normalizer;
use crate::framework::Chain;
use crate::lang::{member_name, property_name, Field, NodeId};
use serde_json::Value;

impl Normalizer<'_> {
    /// Requires enabling literal conditions and no option that might skip the suite.
    pub fn active_suite(&self, callee: NodeId, arguments: &[NodeId]) -> bool {
        let tree = self.tree;
        let boolean = |node| match tree.field_of(tree.inner(node), "value") {
            Field::Value(Value::Bool(value)) => Some(*value),
            _ => None,
        };
        let conditions_hold = std::iter::once(callee)
            .chain(tree.descendants(callee))
            .filter(|&node| tree.kind(node) == "CallExpression")
            .filter_map(|node| {
                let member = tree.inner(tree.child(node, "callee")?);
                let name = member_name(tree, member)?;
                match self.setting.dialect.chain_member(name)? {
                    Chain::RunWhen => Some((node, true)),
                    Chain::SkipWhen => Some((node, false)),
                    _ => None,
                }
            })
            .all(|(node, enabling)| {
                let values = tree.list(node, "arguments");
                values.len() == 1 && values[0].and_then(boolean) == Some(enabling)
            });
        if !conditions_hold {
            return false;
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
                            property_name(tree, property).is_some_and(|name| {
                                !matches!(
                                    self.setting.dialect.chain_member(name),
                                    Some(Chain::Skip | Chain::Todo)
                                ) || tree.child(property, "value").and_then(boolean) == Some(false)
                            })
                        })
                }
                _ => false,
            }
        })
    }
}
