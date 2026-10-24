//! Imported scalar constants in `.each`/`.for` table cells.
//!
//! `import { MODE } from './modes'` over `export const MODE = 'fast'` holds `'fast'` when the
//! table is built: an importer cannot assign an import, the declaring module cannot reassign a
//! `const`, and a static dependency finishes evaluating before the importing file's body runs.
//! The proof is made in the declaring module with its own semantic; the literal is then cloned
//! into the importing file's arena, so every renderer reads it through [`super::peeled`] exactly
//! as it reads a local `const` alias. Mocks and which bindings to try are the caller's question.

use super::{
    const_declarator, peeled, unwrap_transparent, FoldScope, ImportedConstants, MAX_VALUE_HOPS,
};
use crate::frontend::helpers::{find_value_export, with_parsed_file, ValueExport};
use oxc_allocator::{Allocator, CloneIn};
use oxc_ast::ast::{Argument, Expression, UnaryOperator};
use oxc_ast::AstKind;
use oxc_semantic::{Semantic, SemanticBuilder, SymbolId};
use oxc_span::{GetSpan, GetSpanMut, Span};
use std::path::Path;

/// Most import bindings one file tries to prove; each costs a parse of its export chain.
pub(in crate::frontend) const MAX_IMPORTED_CONSTANTS: usize = 64;

impl<'a> ImportedConstants<'a> {
    /// Records that the import binding `symbol` holds `value`.
    pub(in crate::frontend) fn insert(&mut self, symbol: SymbolId, value: &'a Expression<'a>) {
        self.values.insert(symbol, value);
    }
}

/// Whether some reference to `symbol` sits inside the arguments of an `.each`/`.for` call or the
/// quasi of an `.each`/`.for` tagged template, or inside a variable initializer (a local alias a
/// table may read): the only places an imported constant is read. A filter on which imports are
/// worth a proof, so a miss costs recall only.
pub(in crate::frontend) fn referenced_in_table(semantic: &Semantic, symbol: SymbolId) -> bool {
    let scoping = semantic.scoping();
    let nodes = semantic.nodes();
    let is_table = |callee: &Expression| {
        matches!(unwrap_transparent(callee), Expression::StaticMemberExpression(member)
            if matches!(member.property.name.as_str(), "each" | "for"))
    };
    scoping
        .get_resolved_reference_ids(symbol)
        .iter()
        .any(|reference_id| {
            let node = scoping.get_reference(*reference_id).node_id();
            let span = nodes.kind(node).span();
            // A `const` initializer that reads the import (`const L = X`, `const L = `${X}``) is a
            // local alias a table may read.
            let aliased = nodes.ancestor_kinds(node).any(|kind| {
                matches!(kind, AstKind::VariableDeclarator(declarator)
                    if declarator.init.as_ref().is_some_and(|init| init.span().contains_inclusive(span)))
            });
            aliased
                || nodes.ancestor_kinds(node).any(|kind| match kind {
                    AstKind::CallExpression(call) => {
                        is_table(&call.callee)
                            && call
                                .arguments
                                .iter()
                                .any(|argument| argument.span().contains_inclusive(span))
                    }
                    AstKind::TaggedTemplateExpression(tagged) => {
                        is_table(&tagged.tag) && tagged.quasi.span.contains_inclusive(span)
                    }
                    _ => false,
                })
        })
}

/// Whether this file can redefine a property accessor: the one way an export's value changes
/// under its importers. Vitest's module objects reject assignment and `vi.spyOn` of a non-function
/// value, but `vi.spyOn(module, 'X', 'get')` and `Object.defineProperty(module, 'X', …)` rewrite
/// the binding, whichever route reached the module object (`import * as`, `vi.importActual`, a
/// re-exported namespace). The target is not placed, and the names are read from the source
/// text, so renaming, destructuring or a computed key does not hide them: any mention of
/// `defineProperty`, `defineProperties`, `__defineGetter__` or `__defineSetter__` counts, and so
/// does every mention of `spyOn` that is not a direct call with at most two plain arguments.
/// Only a name built at runtime (`'spy' + 'On'`) is not seen.
pub(in crate::frontend) fn redefines_accessors(semantic: &Semantic, source: &str) -> bool {
    if [
        "defineProperty",
        "defineProperties",
        "__defineGetter__",
        "__defineSetter__",
    ]
    .iter()
    .any(|name| source.contains(name))
    {
        return true;
    }
    let nodes = semantic.nodes();
    let plain_spies = nodes
        .iter()
        .filter(|node| {
            let AstKind::CallExpression(call) = node.kind() else {
                return false;
            };
            let callee_is_spy_on = match &call.callee {
                Expression::Identifier(ident) => ident.name == "spyOn",
                Expression::StaticMemberExpression(member) => member.property.name == "spyOn",
                _ => false,
            };
            callee_is_spy_on
                && call.arguments.len() < 3
                && !call
                    .arguments
                    .iter()
                    .any(|argument| matches!(argument, Argument::SpreadElement(_)))
        })
        .count();
    source.matches("spyOn").count() > plain_spies
}

/// The closed scalar literal the first-party module at `path` exports as `export`, cloned into
/// `allocator` under `span`, or `None` unless it is proved.
///
/// Proved means: `path` declares the export with a single module-level, non-ambient `const`
/// binding (directly or `export { local as export }`), and its initializer, after the module's
/// own earlier `const` aliases, is a literal with no names in it. Computed initializers, globals,
/// further imports, composites and `let`/`var` are refused.
pub(in crate::frontend) fn exported_literal<'a>(
    allocator: &'a Allocator,
    path: &Path,
    export: &str,
    span: Span,
) -> Option<&'a Expression<'a>> {
    // A declaration file's values are erased: it describes a module, it is not one. Discovery's
    // classification is the one place that says which files those are.
    crate::discovery::classify_source_file(path)?;
    with_parsed_file(path, |program, _| {
        let ValueExport::Declared(local) = find_value_export(program, export)? else {
            return None;
        };
        let semantic_return = SemanticBuilder::new().with_build_nodes(true).build(program);
        if !semantic_return.diagnostics.is_empty() {
            return None;
        }
        let semantic = &semantic_return.semantic;
        let symbol = semantic.scoping().get_root_binding(local.as_str().into())?;
        // `declare const` emits nothing; the binder marks it ambient.
        if semantic.scoping().symbol_flags(symbol).is_ambient() {
            return None;
        }
        let init = const_declarator(semantic, symbol)?.init.as_ref()?;
        // Another module can write a member of an exported object before this initializer runs
        // (an import cycle), so only `const` names are followed, never a member read.
        if reads_a_member(init, semantic, 0) {
            return None;
        }
        let value = unwrap_transparent(peeled(init, Some(FoldScope::local(semantic))));
        if !is_closed_literal(value) {
            return None;
        }
        let mut cloned = value.clone_in(allocator);
        *cloned.span_mut() = span;
        match &mut cloned {
            Expression::UnaryExpression(unary) => *unary.argument.span_mut() = span,
            Expression::TemplateLiteral(template) => {
                for quasi in template.quasis.iter_mut() {
                    quasi.span = span;
                }
            }
            _ => {}
        }
        Some(&*allocator.alloc(cloned))
    })
}

/// Whether following `expr` through `const` names reaches a member expression, or more names than
/// the fold would follow.
fn reads_a_member(expr: &Expression, semantic: &Semantic, hops: u32) -> bool {
    match unwrap_transparent(expr) {
        Expression::StaticMemberExpression(_) | Expression::ComputedMemberExpression(_) => true,
        Expression::Identifier(ident) => {
            if hops >= MAX_VALUE_HOPS {
                return true;
            }
            let init = ident
                .reference_id
                .get()
                .and_then(|reference| semantic.scoping().get_reference(reference).symbol_id())
                .and_then(|symbol| const_declarator(semantic, symbol))
                .and_then(|declarator| declarator.init.as_ref());
            init.is_some_and(|init| reads_a_member(init, semantic, hops + 1))
        }
        _ => false,
    }
}

/// A literal whose value names nothing: a string, number, boolean, `null`, bigint, a template
/// without substitutions, or a signed number literal. The fold decides which signs it reads.
fn is_closed_literal(expr: &Expression) -> bool {
    match expr {
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_) => true,
        Expression::TemplateLiteral(template) => template.expressions.is_empty(),
        Expression::UnaryExpression(unary) => {
            matches!(
                unary.operator,
                UnaryOperator::UnaryNegation | UnaryOperator::UnaryPlus
            ) && matches!(
                unary.argument,
                Expression::NumericLiteral(_) | Expression::BigIntLiteral(_)
            )
        }
        _ => false,
    }
}
