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
use oxc_span::{GetSpanMut, Span};
use std::path::Path;

/// Most import bindings one file proves; each costs a parse of its export chain.
pub(in crate::frontend) const MAX_IMPORTED_CONSTANTS: usize = 64;

/// What proving one import binding gave.
pub(in crate::frontend) enum Proof<'a> {
    /// The binding holds this closed literal.
    Value(&'a Expression<'a>),
    /// The binding would hold a literal, but this file cannot use it (a revision baseline).
    Unavailable,
    /// Not proved.
    Refused,
}

impl<'a> ImportedConstants<'a> {
    /// Constants proved on demand by `prove`, which also bounds how many imports it proves.
    pub(in crate::frontend) fn lazy(prove: Box<dyn Fn(SymbolId) -> Proof<'a> + 'a>) -> Self {
        Self {
            prove: Some(prove),
            ..Self::default()
        }
    }

    /// The literal the import binding `symbol` holds, proved on its first read.
    pub(super) fn lookup(&self, symbol: SymbolId) -> Option<&'a Expression<'a>> {
        let prove = self.prove.as_ref()?;
        if let Some(outcome) = self.proved.borrow().get(&symbol) {
            return *outcome;
        }
        let outcome = match prove(symbol) {
            Proof::Value(value) => Some(value),
            Proof::Unavailable => {
                self.unavailable.set(true);
                None
            }
            Proof::Refused => None,
        };
        self.proved.borrow_mut().insert(symbol, outcome);
        outcome
    }

    /// Whether a table read a value this file could not use.
    pub(in crate::frontend) fn read_unavailable(&self) -> bool {
        self.unavailable.get()
    }
}

/// Names whose mention lets a file redefine or remove a property of a module object.
const REDEFINING_NAMES: [&str; 5] = [
    "defineProperty",
    "defineProperties",
    "__defineGetter__",
    "__defineSetter__",
    "deleteProperty",
];

/// Whether this file can redefine or remove a property of a module object: the way an export's
/// value changes under its importers. Vitest's module objects reject assignment and `vi.spyOn` of a
/// non-function value, but `vi.spyOn(module, 'X', 'get')`, `Object.defineProperty(module, 'X', …)`
/// and `delete module.X` rewrite the binding, whichever route reached the module object
/// (`import * as`, `vi.importActual`, a re-exported namespace). The target is not placed.
///
/// Every name the file spells is read decoded (identifiers, property keys, strings and template
/// text, so an escape, a rename or a computed key does not hide one): any of
/// [`REDEFINING_NAMES`] counts, and so does a `spyOn` that is not the callee of a direct call with
/// at most two plain arguments. So does `delete` of any property but `process.env`'s. Only a name
/// assembled at runtime (`'spy' + 'On'`) is not seen.
pub(in crate::frontend) fn redefines_accessors(semantic: &Semantic) -> bool {
    let mut spy_mentions = 0;
    let mut plain_spies = 0;
    for node in semantic.nodes().iter() {
        let name = match node.kind() {
            AstKind::IdentifierName(name) => Some(name.name.as_str()),
            AstKind::IdentifierReference(ident) => Some(ident.name.as_str()),
            AstKind::BindingIdentifier(binding) => Some(binding.name.as_str()),
            AstKind::StringLiteral(literal) => Some(literal.value.as_str()),
            AstKind::TemplateElement(element) => element.value.cooked.as_ref().map(|c| c.as_str()),
            AstKind::UnaryExpression(unary)
                if unary.operator == UnaryOperator::Delete && !deletes_env(&unary.argument) =>
            {
                return true;
            }
            AstKind::CallExpression(call) => {
                let spy_callee = match &call.callee {
                    Expression::Identifier(ident) => ident.name == "spyOn",
                    Expression::StaticMemberExpression(member) => member.property.name == "spyOn",
                    _ => false,
                };
                if spy_callee
                    && call.arguments.len() < 3
                    && !call
                        .arguments
                        .iter()
                        .any(|argument| matches!(argument, Argument::SpreadElement(_)))
                {
                    plain_spies += 1;
                }
                None
            }
            _ => None,
        };
        match name {
            Some(name) if REDEFINING_NAMES.contains(&name) => return true,
            // A type (`ReturnType<typeof vi.spyOn>`) runs nothing.
            Some("spyOn") if !in_type(semantic, node.id()) => spy_mentions += 1,
            _ => {}
        }
    }
    spy_mentions > plain_spies
}

/// Whether `node` sits inside a TypeScript type, which is erased.
fn in_type(semantic: &Semantic, node: oxc_semantic::NodeId) -> bool {
    semantic.nodes().ancestor_kinds(node).any(|kind| {
        matches!(
            kind,
            AstKind::TSTypeAnnotation(_)
                | AstKind::TSTypeQuery(_)
                | AstKind::TSTypeReference(_)
                | AstKind::TSQualifiedName(_)
                | AstKind::TSTypeAliasDeclaration(_)
                | AstKind::TSInterfaceDeclaration(_)
        )
    })
}

/// Whether a `delete` operand is a property of `process.env`, a plain object no module is.
fn deletes_env(argument: &Expression) -> bool {
    let object = match unwrap_transparent(argument) {
        Expression::StaticMemberExpression(member) => &member.object,
        Expression::ComputedMemberExpression(member) => &member.object,
        _ => return false,
    };
    matches!(unwrap_transparent(object), Expression::StaticMemberExpression(env)
        if env.property.name == "env"
            && matches!(&env.object, Expression::Identifier(process) if process.name == "process"))
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
