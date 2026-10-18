//! Binding-aware class summaries and point-sensitive owned capture environments.
//!
//! Class bodies normalize once per class and mode. References carry a compact strong digest,
//! a nominal class slot and an environment digest; repeated `new` expressions do not clone
//! class bodies. All state here dies with its callback, before the owned IR is retained.
use super::*;

use captures::{initializer, HistoryAction, HistoryIndex};
use escape::{EscapedThis, PrivateIndex};
use oxc_ast::ast::{
    AssignmentExpression, Class, ClassElement, MethodDefinitionKind, PropertyKind, ReturnStatement,
};
use oxc_ast::AstKind;
use summary::ClassSummary;

mod callables;
mod captures;
mod escape;
mod identity;
mod summary;

/// Per-callback temporary summaries; neither parser nodes nor cache strings enter the IR.
#[derive(Default)]
pub(super) struct ClassCache {
    bodies: HashMap<(u32, bool), ClassSummary>,
    bytes: usize,
    nominals: HashMap<u32, usize>,
    histories: HashMap<(SymbolId, u32, bool), HistoryIndex>,
    class_mutations: HashMap<SymbolId, Option<u32>>,
    writes: HashMap<SymbolId, Vec<oxc_semantic::NodeId>>,
    callable_metadata: HashMap<SymbolId, bool>,
    instance_bindings: HashMap<SymbolId, String>,
    private_members: HashMap<u32, PrivateIndex>,
    private_stability: HashMap<(u32, String), bool>,
}
