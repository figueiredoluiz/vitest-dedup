//! Whether a registration, or a whole suite, runs at collection.

use super::{Modifiers, Normalizer};
use crate::framework::Chain;
use crate::lang::{member_name, property_name, Field, NodeId};
use serde_json::Value;

/// Whether `skip` and `todo` keep a registration from running, from its own chain and options
/// and those of every suite around it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Activation {
    /// A `skip`/`todo` chain member, or an option whose last write is a literal `true`, on a
    /// registration that nothing may focus: Vitest skips or todos it and everything declared
    /// inside it.
    pub inert: bool,
    /// A `skip`/`todo` that `inert` would count, focused or not, or a `skip`/`todo` option whose
    /// last write is not a literal `false` (`{ skip: flag }`, `{ ...options }`): the registration
    /// is not proved to run.
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
    /// A focus (`only`) on the same registration overrides its skip or todo in most spellings but
    /// not all (`describe.only(name, { skip: true })` stays skipped), so it leaves the registration
    /// unproved rather than inert. Focus never reaches a skip on another registration.
    pub fn activation(&self, modifiers: &Modifiers, arguments: &[NodeId]) -> Activation {
        let skipped = modifiers.inert || self.inert_options(arguments);
        let focused = modifiers.focus || self.may_focus_options(arguments);
        Activation {
            inert: skipped && !focused,
            may_skip: skipped || self.may_skip_options(arguments),
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
                let mut activation = self.activation(&modifiers, &arguments);
                activation.may_skip |= modifiers.conditional_may_skip;
                activation.within(enclosing)
            })
    }

    /// A conditional modifier only guarantees activation with one enabling boolean literal.
    pub(super) fn condition_may_skip(&self, call: NodeId, member: NodeId) -> bool {
        let enabling = match member_name(self.tree, member)
            .and_then(|name| self.setting.dialect.chain_member(name))
        {
            Some(Chain::RunWhen) => true,
            Some(Chain::SkipWhen) => false,
            _ => return false,
        };
        match self.tree.list(call, "arguments").as_slice() {
            [Some(argument)] => self.literal_boolean(*argument) != Some(enabling),
            _ => true,
        }
    }

    /// A boolean literal after removing transparent expression wrappers.
    fn literal_boolean(&self, node: NodeId) -> Option<bool> {
        match self.tree.field_of(self.tree.inner(node), "value") {
            Field::Value(Value::Bool(value)) => Some(*value),
            _ => None,
        }
    }

    /// Requires enabling literal conditions and no option that might skip the suite.
    pub fn active_suite(&self, callee: NodeId, arguments: &[NodeId]) -> bool {
        let tree = self.tree;
        let conditions_hold = std::iter::once(callee)
            .chain(tree.descendants(callee))
            .filter(|&node| tree.kind(node) == "CallExpression")
            .all(|node| {
                tree.child(node, "callee")
                    .is_none_or(|callee| !self.condition_may_skip(node, tree.inner(callee)))
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
                                ) || tree
                                    .child(property, "value")
                                    .and_then(|value| self.literal_boolean(value))
                                    == Some(false)
                            })
                        })
                }
                _ => false,
            }
        })
    }
}
