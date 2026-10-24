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
use crate::frontend::resolver::{self, Resolved};
use oxc_allocator::{Allocator, CloneIn};
use oxc_ast::ast::{Expression, ImportDeclarationSpecifier, UnaryOperator};
use oxc_ast::AstKind;
use oxc_semantic::{Semantic, SemanticBuilder, SymbolId};
use oxc_span::{GetSpan, GetSpanMut, Span};
use std::path::{Path, PathBuf};

/// Most import bindings one file tries to prove; each costs a parse of its export chain.
pub(in crate::frontend) const MAX_IMPORTED_CONSTANTS: usize = 64;

impl<'a> ImportedConstants<'a> {
    /// Records that the import binding `symbol` holds `value`.
    pub(in crate::frontend) fn insert(&mut self, symbol: SymbolId, value: &'a Expression<'a>) {
        self.values.insert(symbol, value);
    }
}

/// Whether some reference to `symbol` sits inside the arguments of an `.each`/`.for` call or the
/// quasi of an `.each`/`.for` tagged template, or initializes a `const` (a local alias a table may
/// read): the only places an imported constant is read.
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
            let aliased = matches!(nodes.parent_kind(node), AstKind::VariableDeclarator(declarator)
                if declarator.init.as_ref().is_some_and(|init| init.span() == span));
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

/// The canonical paths of the modules this file can hold a namespace object of: every
/// `import * as` and every `import('…')`, resolved. `None` when a dynamic import's specifier is
/// not a string literal, or does not resolve, since it could name any module.
pub(in crate::frontend) fn namespace_modules(
    semantic: &Semantic,
    file: &Path,
) -> Option<Vec<PathBuf>> {
    let canonical = |specifier: &str| match resolver::resolve(file, specifier) {
        Resolved::Module(path) => Some(path.canonicalize().unwrap_or(path)),
        Resolved::Unresolved => None,
    };
    let mut modules = Vec::new();
    for node in semantic.nodes().iter() {
        match node.kind() {
            AstKind::ImportDeclaration(import) => {
                let namespace = import.specifiers.iter().flatten().any(|specifier| {
                    matches!(
                        specifier,
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(_)
                    )
                });
                if namespace {
                    // An unresolved namespace names no module a chain can reach.
                    modules.extend(canonical(import.source.value.as_str()));
                }
            }
            AstKind::ImportExpression(import) => {
                let Expression::StringLiteral(specifier) = &import.source else {
                    return None;
                };
                modules.push(canonical(specifier.value.as_str())?);
            }
            _ => {}
        }
    }
    Some(modules)
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
