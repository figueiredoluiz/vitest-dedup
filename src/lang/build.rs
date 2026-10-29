//! From oxc's AST and semantic analysis to the owned [`Tree`] and [`Scopes`].

use super::scopes::{Declaration, Imported, Name, Reference, Scopes, Symbol, SymbolId};
use super::tree::{Span, Tree};
use super::{Parsed, Refusal};
use oxc_allocator::Allocator;
use oxc_ast::ast::ModuleExportName;
use oxc_ast::AstKind;
use oxc_parser::Parser;
use oxc_semantic::{Semantic, SemanticBuilder};
use oxc_span::{GetSpan, SourceType};
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

/// Parses `source` and builds its owned tree and scopes.
pub(super) fn parse(path: &Path, source: &str) -> Result<Parsed, Refusal> {
    let source_type = SourceType::from_path(path).map_err(|_| Refusal::UnsupportedSource)?;
    if super::limits::parse_refusal(source).is_some() {
        return Err(Refusal::SourceLimit);
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.fatal_error {
        return Err(Refusal::Fatal);
    }
    let program = &parsed.program;
    let json = program.to_estree_json(true, false);
    // The preflight above bounds how deep the tree nests, so serde's own depth limit, far below
    // what that admits, is lifted.
    let json = spell_lone_surrogates(&json);
    let mut deserializer = serde_json::Deserializer::from_str(&json);
    deserializer.disable_recursion_limit();
    let value = serde_json::Value::deserialize(&mut deserializer).map_err(|_| Refusal::Fatal)?;
    let tree = Tree::from_json(value).ok_or(Refusal::Fatal)?;
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(program)
        .semantic;
    let scopes = scopes(&semantic, &tree);
    Ok(Parsed {
        tree,
        scopes,
        parse_errors: parsed.diagnostics.len(),
    })
}

/// The owned span of an oxc span.
fn span(span: oxc_span::Span) -> Span {
    Span {
        start: span.start,
        end: span.end,
    }
}

/// The owned scopes of `semantic`, with declarations and references located in `tree`.
fn scopes(semantic: &Semantic, tree: &Tree) -> Scopes {
    let scoping = semantic.scoping();
    let nodes = semantic.nodes();
    let mut names = HashMap::new();
    for node in nodes.iter() {
        match node.kind() {
            AstKind::IdentifierReference(reference) => {
                let symbol = reference
                    .reference_id
                    .get()
                    .and_then(|id| scoping.get_reference(id).symbol_id());
                names.insert(reference.span.start, Name::Reference(symbol.map(owned)));
            }
            AstKind::BindingIdentifier(binding) => {
                names.insert(
                    binding.span.start,
                    Name::Binding(binding.symbol_id.get().map(owned)),
                );
            }
            _ => {}
        }
    }
    let root = scoping.root_scope_id();
    let mut top_level = HashMap::new();
    let symbols = scoping
        .symbol_ids()
        .map(|symbol| {
            let binding = span(scoping.symbol_span(symbol));
            let name = scoping.symbol_name(symbol).to_owned();
            let is_top = scoping.symbol_scope_id(symbol) == root;
            if is_top {
                top_level.entry(name.clone()).or_insert(owned(symbol));
            }
            let references = scoping
                .get_resolved_reference_ids(symbol)
                .iter()
                .map(|&id| {
                    let reference = scoping.get_reference(id);
                    let at = span(nodes.kind(reference.node_id()).span());
                    Reference {
                        node: tree.find(at, &["Identifier", "JSXIdentifier"]),
                        span: at,
                        write: reference.is_write(),
                    }
                })
                .collect();
            let declaration = declaration(
                nodes.kind(scoping.symbol_declaration(symbol)),
                binding,
                tree,
            );
            Symbol {
                name,
                span: binding,
                declaration,
                references,
                top_level: is_top,
            }
        })
        .collect();
    Scopes {
        symbols,
        names,
        top_level,
    }
}

/// The owned id of an oxc symbol: the same index.
fn owned(symbol: oxc_semantic::SymbolId) -> SymbolId {
    SymbolId(symbol.index() as u32)
}

/// What `kind`, the declaration oxc records for a symbol bound at `binding`, is in `tree`.
fn declaration(kind: AstKind, binding: Span, tree: &Tree) -> Declaration {
    let found = |at: oxc_span::Span, kinds: &[&str]| tree.find(span(at), kinds);
    let other = || Declaration::Other(tree.find(binding, &["Identifier"]));
    let import = |at: oxc_span::Span, specifier_kind: &str, imported: Imported| {
        found(at, &[specifier_kind])
            .and_then(|specifier| {
                let import = tree.parent(specifier)?;
                Some(Declaration::Import {
                    import,
                    specifier,
                    imported,
                })
            })
            .unwrap_or_else(other)
    };
    match kind {
        AstKind::VariableDeclarator(declarator) => found(declarator.span, &["VariableDeclarator"])
            .map_or_else(other, Declaration::Variable),
        AstKind::Function(function) => found(
            function.span,
            &[
                "FunctionDeclaration",
                "FunctionExpression",
                "TSDeclareFunction",
                "TSEmptyBodyFunctionExpression",
            ],
        )
        .map_or_else(other, Declaration::Function),
        AstKind::Class(class) => found(class.span, &["ClassDeclaration", "ClassExpression"])
            .map_or_else(other, Declaration::Class),
        AstKind::ImportSpecifier(specifier) => import(
            specifier.span,
            "ImportSpecifier",
            Imported::Named(export_name(&specifier.imported)),
        ),
        AstKind::ImportDefaultSpecifier(specifier) => {
            import(specifier.span, "ImportDefaultSpecifier", Imported::Default)
        }
        AstKind::ImportNamespaceSpecifier(specifier) => import(
            specifier.span,
            "ImportNamespaceSpecifier",
            Imported::Namespace,
        ),
        _ => other(),
    }
}

/// The name an import or export specifier spells.
fn export_name(name: &ModuleExportName) -> String {
    match name {
        ModuleExportName::IdentifierName(id) => id.name.to_string(),
        ModuleExportName::IdentifierReference(id) => id.name.to_string(),
        ModuleExportName::StringLiteral(literal) => {
            super::js_text::string_text(literal).into_owned()
        }
    }
}

/// `json` with every lone surrogate escape (`\ud800` not part of a pair) spelled as
/// [`super::js_text`] spells one: U+FFFD followed by the code unit in four lowercase hex digits. A JSON parser rejects a lone surrogate escape; oxc writes one for a string
/// value that holds one.
pub(super) fn spell_lone_surrogates(json: &str) -> Cow<'_, str> {
    if !json.contains("\\ud") && !json.contains("\\uD") {
        return Cow::Borrowed(json);
    }
    let unit = |at: usize| -> Option<u16> {
        let digits = json.get(at..at + 6)?;
        let hex = digits.strip_prefix("\\u")?;
        u16::from_str_radix(hex, 16).ok()
    };
    let mut out = String::with_capacity(json.len());
    let mut index = 0;
    while index < json.len() {
        let rest = &json[index..];
        let Some(slash) = rest.find('\\') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..slash]);
        index += slash;
        match unit(index) {
            Some(0xD800..=0xDBFF) if matches!(unit(index + 6), Some(0xDC00..=0xDFFF)) => {
                out.push_str(&json[index..index + 12]);
                index += 12;
            }
            Some(lone @ 0xD800..=0xDFFF) => {
                out.push_str(&format!("\u{FFFD}{lone:04x}"));
                index += 6;
            }
            _ => {
                // Any other escape is two bytes (`\\`, `\"`, `\n`) or a `\u` escape copied whole.
                let length = if json[index + 1..].starts_with('u') {
                    6
                } else {
                    2
                };
                let end = (index + length).min(json.len());
                out.push_str(&json[index..end]);
                index = end;
            }
        }
    }
    Cow::Owned(out)
}
