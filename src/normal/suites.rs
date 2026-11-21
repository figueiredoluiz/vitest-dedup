//! Whether a registration, or a whole suite, runs at collection.

use super::{Modifiers, Normalizer};
use crate::framework::Chain;
use crate::lang::{member_name, property_name, Field, NodeId};
use serde_json::Value;

/// Whether `skip` and `todo` keep a registration from running, from its own chain and options
/// and those of every suite around it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Activation {
    /// A `skip`/`todo` chain member, or an option whose last write is a literal `true`: Vitest
    /// skips or todos the registration and everything declared inside it.
    pub inert: bool,
    /// [`Self::inert`], or a `skip`/`todo` option whose last write is not a literal `false`
    /// (`{ skip: flag }`, `{ ...options }`): the registration is not proved to run.
    pub may_skip: bool,
}

impl Activation {
    /// This registration's activation once a suite around it contributes `enclosing`.
    pub fn within(self, enclosing: Self) -> Self {
        Self {
            inert: self.inert || enclosing.inert,
            may_skip: self.may_skip || enclosing.may_skip,
        }
    }
}

impl Normalizer<'_> {
    /// What a registration's own chain (`modifiers`) and `arguments` say about whether it runs.
    pub fn activation(&self, modifiers: &Modifiers, arguments: &[NodeId]) -> Activation {
        let inert = modifiers.inert || self.inert_options(arguments);
        Activation {
            inert,
            may_skip: inert || self.may_skip_options(arguments),
        }
    }

    /// The activation the suites around `node` impose on it, each read through the API member
    /// `member` gives for its call ([`crate::inventory::Site::member`]). A registration inside a
    /// function that may run elsewhere has no enclosing suites to read.
    pub fn enclosing_activation(
        &mut self,
        node: NodeId,
        member: impl Fn(NodeId) -> Option<NodeId>,
    ) -> Activation {
        let tree = self.tree;
        let calls: Vec<NodeId> = self
            .suites_around(node)
            .unwrap_or_default()
            .iter()
            .filter_map(|scope| self.structure.suites.get(scope).copied())
            .collect();
        calls
            .into_iter()
            .fold(Activation::default(), |enclosing, call| {
                let Some(callee) = tree.child(call, "callee") else {
                    return enclosing;
                };
                let modifiers = self.modifiers(callee, member(call));
                let arguments: Vec<NodeId> =
                    tree.list(call, "arguments").into_iter().flatten().collect();
                self.activation(&modifiers, &arguments).within(enclosing)
            })
    }

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
