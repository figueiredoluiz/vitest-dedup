//! What a setup call stands on: the text of everything its arguments read.
//!
//! A call such as `registerEndpoint('/x', { handler })` behaves according to `handler`, which is
//! declared elsewhere in the file. The call's own text is equal in two files whose `handler`s
//! differ, so text alone would let those files compare equal. The identity built here adds, for
//! every name the call reads, what the name is: the declaration's text for a file-local binding
//! (and, transitively, what that reads), the module and export for an import, the bare name for a
//! global. A name the scan cannot place makes the identity unavailable and the call is refused.
//!
//! Names written after their declaration are refused by the proof that runs first
//! ([`super::registration_free::arguments_register_nothing`]), so the declaration read here is the
//! value the call sees.

use super::*;
use oxc_ast::AstKind;
use std::collections::BTreeSet;

/// Most bytes one identity may hold.
const MAX_IDENTITY_BYTES: usize = 32 * 1024;

struct Closure<'s, 'a> {
    semantic: &'s Semantic<'a>,
    source: &'s str,
    imports: &'s HashMap<SymbolId, super::imported::RunnerImport>,
    /// The span of the call being identified: bindings declared inside it are part of its text.
    call: Span,
    /// Declarations whose full text is already a part: bindings inside them (parameters, locals)
    /// add nothing.
    covered: Vec<Span>,
    /// The file the call is written in, which `import.meta` stands for.
    file: &'s std::path::Path,
    seen: HashSet<SymbolId>,
    parts: BTreeSet<String>,
    bytes: usize,
    failed: bool,
}

impl<'a> Visit<'a> for Closure<'_, 'a> {
    fn visit_import_meta(&mut self, _it: &oxc_ast::ast::ImportMeta) {
        self.add(format!("import.meta:{}", self.file.display()));
    }

    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        if self.failed {
            return;
        }
        let reference = ident
            .reference_id
            .get()
            .map(|reference| self.semantic.scoping().get_reference(reference));
        // A name used only as a type is erased and contributes nothing at runtime.
        if reference.is_some_and(|reference| !reference.is_value()) {
            return;
        }
        let symbol = reference.and_then(|reference| reference.symbol_id());
        let Some(symbol) = symbol else {
            self.add(format!("global:{}", ident.name));
            return;
        };
        if !self.seen.insert(symbol) {
            return;
        }
        if let Some(import) = self.imports.get(&symbol) {
            if import.erased {
                return;
            }
            self.add(format!(
                "import:{}={}#{}",
                ident.name, import.origin.component, import.origin.exported
            ));
            return;
        }
        // The declaration that runs, as the proof read it: an overload set's implementation.
        let declaration = super::registration_free::runtime_declaration(self.semantic, symbol)
            .unwrap_or(self.semantic.scoping().symbol_declaration(symbol));
        let node = self.semantic.nodes().get_node(declaration);
        let span = node.kind().span();
        let inside = |outer: &Span| span.start >= outer.start && span.end <= outer.end;
        if inside(&self.call) || self.covered.iter().any(inside) {
            return;
        }
        match node.kind() {
            // Names taken from `vitest` (`vi.fn()`, `expect`) are not collected as imports; they
            // stand for the framework, whose registration API the proof already ruled out.
            AstKind::ImportSpecifier(specifier) => {
                self.add(format!(
                    "vitest:{}={}",
                    ident.name,
                    specifier.imported.name()
                ));
            }
            AstKind::ImportDefaultSpecifier(_) | AstKind::ImportNamespaceSpecifier(_) => {
                self.add(format!("vitest:{}", ident.name));
            }
            AstKind::VariableDeclarator(declarator) => {
                self.covered.push(declarator.span);
                self.add(format!(
                    "local:{}",
                    &self.source[declarator.span.start as usize..declarator.span.end as usize]
                ));
                // The pattern too: a default (`{ value = fallback }`) or a computed key reads names.
                self.visit_variable_declarator(declarator);
            }
            AstKind::Class(class) => {
                self.covered.push(class.span);
                self.add(format!(
                    "local:{}",
                    &self.source[class.span.start as usize..class.span.end as usize]
                ));
                self.visit_class(class);
            }
            AstKind::Function(function) => {
                self.covered.push(function.span);
                self.add(format!(
                    "local:{}",
                    &self.source[function.span.start as usize..function.span.end as usize]
                ));
                self.visit_function(function, oxc_syntax::scope::ScopeFlags::Function);
            }
            _ => self.failed = true,
        }
    }
}

impl Closure<'_, '_> {
    fn add(&mut self, part: String) {
        self.bytes += part.len();
        if self.bytes > MAX_IDENTITY_BYTES {
            self.failed = true;
        } else {
            self.parts.insert(part);
        }
    }
}

impl<'a> Walker<'a> {
    /// The identity of `call` as setup: its text plus what every name it reads stands for.
    /// `None` when a name cannot be placed or the identity would be too large.
    pub(super) fn setup_call_identity(&self, call: &CallExpression<'a>) -> Option<String> {
        let mut closure = Closure {
            semantic: self.semantic,
            source: self.source,
            imports: &self.runner_imports,
            call: call.span,
            covered: Vec::new(),
            file: &self.file.path,
            seen: HashSet::new(),
            parts: BTreeSet::new(),
            bytes: 0,
            failed: false,
        };
        for argument in &call.arguments {
            closure.visit_argument(argument);
        }
        (!closure.failed).then(|| {
            let parts: Vec<String> = closure.parts.into_iter().collect();
            format!(
                "{}|{}",
                source_text(self.source, call.span),
                parts.join("\u{1f}")
            )
        })
    }
}
