//! Imported scalar constants in `.each`/`.for` table cells.
//!
//! `import { MODE } from './modes'` over `export const MODE = 'fast'` holds `'fast'` when the
//! table is built: an importer cannot assign an import, the declaring module cannot reassign a
//! `const`, and a static dependency finishes evaluating before the importing file's body runs.
//! The proof is made in the declaring module with its own semantic; the literal is then cloned
//! into the importing file's arena, so every renderer reads it through [`super::peeled`] exactly
//! as it reads a local `const` alias. Mocks and which bindings to try are the caller's question.

use super::{const_declarator, peeled, unwrap_transparent, FoldScope, ImportedConstants};
use crate::frontend::helpers::{find_value_export, with_parsed_file, ValueExport};
use oxc_allocator::{Allocator, CloneIn};
use oxc_ast::ast::{Expression, UnaryOperator};
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
/// quasi of an `.each`/`.for` tagged template: the only places an imported constant is read.
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
            nodes.ancestor_kinds(node).any(|kind| match kind {
                AstKind::CallExpression(call) => {
                    is_table(&call.callee)
                        && call
                            .arguments
                            .iter()
                            .any(|argument| contains(argument.span(), span))
                }
                AstKind::TaggedTemplateExpression(tagged) => {
                    is_table(&tagged.tag) && contains(tagged.quasi.span, span)
                }
                _ => false,
            })
        })
}

/// Whether `outer` covers `inner`.
fn contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
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
    // A declaration file's values are erased: it describes a module, it is not one.
    let name = path.file_name()?.to_string_lossy();
    if [".d.ts", ".d.mts", ".d.cts"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
    {
        return None;
    }
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
        if semantic.scoping().symbol_flags(symbol).is_ambient() || is_declared(semantic, symbol) {
            return None;
        }
        let init = const_declarator(semantic, symbol)?.init.as_ref()?;
        let value = unwrap_transparent(peeled(init, Some(FoldScope::local(semantic))));
        if !is_closed_literal(value) {
            return None;
        }
        let mut cloned = value.clone_in(allocator);
        *cloned.span_mut() = span;
        if let Expression::UnaryExpression(unary) = &mut cloned {
            *unary.argument.span_mut() = span;
        }
        Some(&*allocator.alloc(cloned))
    })
}

/// Whether the declaration of `symbol` is a TypeScript `declare const`, which emits nothing.
fn is_declared(semantic: &Semantic, symbol: SymbolId) -> bool {
    let nodes = semantic.nodes();
    let node = semantic.scoping().symbol_declaration(symbol);
    [nodes.kind(node), nodes.parent_kind(node)].into_iter().any(
        |kind| matches!(kind, AstKind::VariableDeclaration(declaration) if declaration.declare),
    )
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
