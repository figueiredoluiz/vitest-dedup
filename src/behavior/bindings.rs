//! Whole-pattern accounting shared by substitution and the behavior walk.

use super::{dataflow::Dataflow, symbol_at};
use crate::lang::{NodeId, Parsed, Tree};

/// Where evaluation of a binding pattern is represented.
pub(super) enum Accounting {
    /// A plain identifier has no destructuring evaluation.
    None,
    /// The recorded single-property rewrite represents the entire pattern at one safe read.
    AtRead,
    /// Keep the whole pattern, including unused names and implicit property/iterator effects.
    Opaque,
}

/// Every non-identifier binding may evaluate defaults, property access or an iterator protocol.
pub(super) fn evaluates(tree: &Tree, target: NodeId) -> bool {
    tree.kind(tree.inner(target)) != "Identifier"
}

/// A whole pattern consisting of exactly one ordinary property binding one identifier.
/// Defaults, computed keys, rest and nested patterns cannot be replaced by one member read.
pub(super) fn single_property(tree: &Tree, pattern: NodeId) -> Option<(NodeId, &str)> {
    let properties = tree.list(pattern, "properties");
    let [Some(property)] = properties.as_slice() else {
        return None;
    };
    if tree.kind(*property) != "Property" || tree.flag(*property, "computed") {
        return None;
    }
    let key = tree.child(*property, "key")?;
    let value = tree.child(*property, "value")?;
    if tree.kind(key) != "Identifier" || tree.kind(value) != "Identifier" {
        return None;
    }
    Some((value, tree.text(key, "name")?))
}

/// Reads the completed substitution result instead of predicting which rewrite will succeed.
pub(super) fn accounting(parsed: &Parsed, target: NodeId, dataflow: &Dataflow) -> Accounting {
    let target = parsed.tree.inner(target);
    if !evaluates(&parsed.tree, target) {
        return Accounting::None;
    }
    if single_property(&parsed.tree, target)
        .and_then(|(value, _)| symbol_at(parsed, value))
        .is_some_and(|symbol| dataflow.member.contains_key(&symbol))
    {
        Accounting::AtRead
    } else {
        Accounting::Opaque
    }
}
