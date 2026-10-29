//! Identities: how a binding declared outside the normalized node is named.

use super::{digest, form, Normalizer, Setting, Structure, Walk, MAX_NESTING};
use crate::lang::{bound_symbols, export_name, Declaration, Imported, NodeId, SymbolId};
use std::collections::HashMap;
use std::path::Path;

/// Ends the identity of a binding that holds one object (a function, or a `const` that allocates
/// one): equal declarations are one identity, but each binding is its own object, numbered within
/// a normal form so `a === a` and `a === b` differ.
const OBJECT: &str = ":object";

impl Normalizer<'_> {
    /// The text an identifier bound to `symbol` is written as.
    pub(super) fn identifier(&mut self, symbol: SymbolId, walk: &mut Walk) -> String {
        if let Some(substitute) = walk.substitutions.get(&symbol) {
            return substitute.clone();
        }
        let declared = self.scopes.symbol(symbol).span;
        if walk.root.contains(declared) {
            if walk.names_observed && self.named(symbol) {
                return format!("n:{}", self.scopes.symbol(symbol).name);
            }
            let next = walk.locals.len();
            return format!("${}", walk.locals.entry(symbol).or_insert(next));
        }
        let mut identity = self.external(symbol, walk.depth);
        if identity.ends_with(OBJECT) {
            let next = walk.objects.len();
            let number = *walk.objects.entry(symbol).or_insert(next);
            identity = format!("{identity}#{number}");
        }
        // A reader that observes names (`f.name`) tells apart functions that differ only in name.
        if walk.names_observed && self.named(symbol) {
            identity = format!("{identity}:n:{}", self.scopes.symbol(symbol).name);
        }
        identity
    }

    /// Whether `symbol` names a function or class, whose `name` property carries it.
    fn named(&self, symbol: SymbolId) -> bool {
        let tree = self.tree;
        match self.scopes.symbol(symbol).declaration {
            Declaration::Function(_) | Declaration::Class(_) => true,
            Declaration::Variable(declarator) => {
                tree.child(declarator, "init").is_some_and(|init| {
                    matches!(
                        tree.kind(tree.inner(init)),
                        "FunctionExpression" | "ArrowFunctionExpression" | "ClassExpression"
                    )
                })
            }
            _ => false,
        }
    }

    /// Names a binding declared outside the node being normalized by its declaration, not its
    /// value: an import by what the module exports, a module- or suite-level declaration by kind,
    /// name and the normal form of what declares it. Anything else (a parameter, a loop variable, a
    /// binding written while tests register) is named by identity, so it equals only itself.
    pub fn external(&mut self, symbol: SymbolId, depth: usize) -> String {
        if let Some(known) = self.externals.get(&symbol) {
            return known.clone();
        }
        let opaque = format!(
            "sym:{}:{}",
            self.path.display(),
            self.scopes.symbol(symbol).span.start
        );
        if depth >= MAX_NESTING || !self.active.insert(symbol) {
            return opaque;
        }
        let named = self.declared(symbol, depth).unwrap_or(opaque);
        self.active.remove(&symbol);
        self.externals.insert(symbol, named.clone());
        named
    }

    /// Whether a binding is assigned while tests register: outside every callback that runs later.
    pub fn written_while_registering(&self, symbol: SymbolId) -> bool {
        self.scopes
            .symbol(symbol)
            .references
            .iter()
            .any(|reference| reference.write && !self.structure.at_run_time(reference.span))
    }

    /// Where a declaration sits: `m` at module level, `s` directly in a suite body, `None` inside a
    /// loop, a `catch` or any other function.
    fn level(&self, symbol: SymbolId) -> Option<&'static str> {
        let tree = self.tree;
        let node = self.scopes.declaration_node(symbol)?;
        for ancestor in std::iter::once(node).chain(tree.ancestors(node)) {
            match tree.kind(ancestor) {
                "ForStatement" | "ForInStatement" | "ForOfStatement" | "WhileStatement"
                | "DoWhileStatement" | "CatchClause" => return None,
                _ if tree.is_function(ancestor) && ancestor != node => {
                    return self.structure.suites.contains_key(&ancestor).then_some("s");
                }
                _ => {}
            }
        }
        Some("m")
    }

    /// The identity [`Self::external`] gives a symbol, by its declaration; `None` when it equals
    /// only itself.
    fn declared(&mut self, symbol: SymbolId, depth: usize) -> Option<String> {
        if self.written_while_registering(symbol) {
            return None;
        }
        let level = self.level(symbol)?;
        let tree = self.tree;
        let name = self.scopes.symbol(symbol).name.clone();
        match self.scopes.symbol(symbol).declaration.clone() {
            Declaration::Import {
                import, imported, ..
            } => {
                let source = tree
                    .child(import, "source")
                    .and_then(|s| tree.text(s, "value"))?;
                let imported = match imported {
                    Imported::Named(name) => name,
                    Imported::Default => "default".to_owned(),
                    Imported::Namespace => "*".to_owned(),
                };
                let dialect = self.setting.dialect;
                if dialect.is_api_module(source) {
                    let canonical = dialect
                        .registration(&imported)
                        .map(|registration| registration.canonical)
                        .or_else(|| dialect.namespace(&imported))
                        .unwrap_or(&imported);
                    return Some(format!("api:{canonical}"));
                }
                // What a project module exports is named the way its own declarations are, so
                // equal helpers in two modules are one identity.
                if let Some(file) = crate::lang::resolve(self.path, source, self.setting.root) {
                    if let Some(exported) = exported(&file, &imported, depth + 1, self.setting) {
                        return Some(exported);
                    }
                }
                // Anything else is the file it resolves to (a stylesheet two test files name with
                // one spelling may be two files), or its spelling when it resolves to none.
                let module = crate::lang::resolve_file(self.path, source, self.setting.root)
                    .map_or_else(|| source.to_owned(), |file| file.display().to_string());
                Some(format!("import:{module}:{imported}"))
            }
            Declaration::Variable(declarator) => {
                let declaration = tree.parent(declarator)?;
                let kind = tree
                    .text(declaration, "kind")
                    .unwrap_or_default()
                    .to_owned();
                let id = tree.child(declarator, "id")?;
                let simple = tree.kind(id) == "Identifier";
                // A plain binding's pattern is its name, which the identity below already carries.
                let pattern = if simple {
                    String::new()
                } else {
                    self.at_depth(id, depth + 1)
                };
                let init_node = tree.child(declarator, "init");
                let init = init_node
                    .map(|init| self.at_depth(init, depth + 1))
                    .unwrap_or_default();
                let declared = digest(&format!("{pattern}={init}"));
                let position = bound_symbols(tree, self.scopes, id)
                    .iter()
                    .position(|bound| *bound == symbol)
                    .unwrap_or(0);
                let constant = kind == "const";
                // A `const` bound to a primitive literal is that literal.
                let primitive = init_node.is_some_and(|init| {
                    let init = tree.inner(init);
                    (tree.kind(init) == "Literal" && !form::allocates(tree, init))
                        || (tree.kind(init) == "TemplateLiteral"
                            && tree.list(init, "expressions").is_empty())
                });
                if primitive && constant && simple {
                    return Some(init);
                }
                // Statements outside test bodies that use the binding (`app.use(...)`, a component
                // that assigns it as a ref) are part of what it is when a test reads it.
                let uses = digest(&self.registration_uses(symbol, depth));
                // Two `const`s that create nothing are their initializer; two that each create an
                // object (`vi.fn()`, `ref(0)`) are two objects, told apart by name and module.
                let creates = init_node.is_none_or(|init| form::creates(self, init));
                // One that allocates (`{}`, `[]`, a function) is one object; see [`OBJECT`].
                let allocates = init_node.is_some_and(|init| form::allocates(tree, init));
                let object = if allocates { OBJECT } else { "" };
                Some(match (constant && !creates, simple) {
                    (true, true) => format!("var:{level}:const:{declared}:{uses}{object}"),
                    (true, false) => {
                        format!("var:{level}:const:{declared}:{position}:{uses}{object}")
                    }
                    (false, _) => format!(
                        "var:{level}:{kind}:{name}:{declared}:{position}:{uses}:{}",
                        self.path.display()
                    ),
                })
            }
            Declaration::Function(function) => Some(format!(
                "fn:{level}:{}{OBJECT}",
                digest(&self.at_depth(function, depth + 1))
            )),
            Declaration::Class(class) => Some(format!(
                "class:{level}:{name}:{}",
                digest(&self.at_depth(class, depth + 1))
            )),
            Declaration::Other(_) => None,
        }
    }

    /// The statement directly in a function body or the program that holds `node`.
    fn top_statement(&self, node: NodeId) -> Option<NodeId> {
        let tree = self.tree;
        let mut previous = node;
        for ancestor in tree.ancestors(node) {
            let body = match tree.kind(ancestor) {
                "Program" => true,
                "BlockStatement" => tree.parent(ancestor).is_some_and(|function| {
                    tree.is_function(function) && tree.child(function, "body") == Some(ancestor)
                }),
                // A concise arrow body is its own statement.
                _ => tree.is_function(ancestor) && tree.child(ancestor, "body") == Some(previous),
            };
            if body {
                return Some(previous);
            }
            previous = ancestor;
        }
        None
    }

    /// The normal forms of the statements outside test and hook bodies that reference `symbol`,
    /// other than its declaration and registrations, in source order, with `symbol` itself
    /// written as `self`.
    fn registration_uses(&mut self, symbol: SymbolId, depth: usize) -> String {
        let tree = self.tree;
        let declared = self
            .scopes
            .declaration_node(symbol)
            .map(|node| tree.span(node));
        let mut statements: Vec<NodeId> = Vec::new();
        for reference in &self.scopes.symbol(symbol).references {
            if self.structure.at_run_time(reference.span) {
                continue;
            }
            let Some(statement) = reference.node.and_then(|node| self.top_statement(node)) else {
                continue;
            };
            let span = tree.span(statement);
            // A registration that uses the binding (`myTest('a', ...)`) is a test, not a use.
            let registers = self
                .structure
                .run_time
                .iter()
                .any(|body| span.contains(*body))
                || self
                    .structure
                    .suites
                    .keys()
                    .any(|&suite| span.contains(tree.span(suite)));
            if registers
                || declared.is_some_and(|declared| span.contains(declared))
                || statements.contains(&statement)
            {
                continue;
            }
            statements.push(statement);
        }
        statements.sort_by_key(|&statement| tree.span(statement).start);
        let substitutions = HashMap::from([(symbol, "self".to_owned())]);
        statements
            .into_iter()
            .map(|statement| self.substituted(statement, depth + 1, substitutions.clone(), false))
            .collect::<Vec<_>>()
            .join(";")
    }
}

/// The identity of what `file` exports as `name`, followed through re-exports; `None` when it is
/// not a declaration this names, or the modules cycle.
fn exported(file: &Path, name: &str, depth: usize, setting: Setting) -> Option<String> {
    let key = (file.to_path_buf(), name.to_owned());
    if let Some(known) = setting.exports.known.borrow().get(&key) {
        return known.clone();
    }
    if depth >= MAX_NESTING {
        return None;
    }
    // Marks the export in progress, so a cycle resolves to `None`.
    setting.exports.known.borrow_mut().insert(key.clone(), None);
    let found = export_of(file, name, depth, setting);
    setting
        .exports
        .known
        .borrow_mut()
        .insert(key, found.clone());
    found
}

/// Parses `file` and names what it exports as `name`, following one re-export at a time.
fn export_of(file: &Path, name: &str, depth: usize, setting: Setting) -> Option<String> {
    let source =
        crate::discovery::read_bounded(file, crate::discovery::MAX_SOURCE_FILE_BYTES).ok()??;
    let parsed = crate::lang::parse(file, &source).ok()?;
    let structure = Structure::default();
    let mut normalizer = Normalizer::new(file, &parsed, setting, &structure);
    let tree = &parsed.tree;
    let scopes = &parsed.scopes;
    for statement in tree.list(tree.root(), "body").into_iter().flatten() {
        let symbol = match tree.kind(statement) {
            "ExportNamedDeclaration" => {
                if let Some(source) = tree
                    .child(statement, "source")
                    .and_then(|s| tree.text(s, "value"))
                {
                    let Some(specifier) = tree
                        .list(statement, "specifiers")
                        .into_iter()
                        .flatten()
                        .find(|&specifier| {
                            tree.child(specifier, "exported")
                                .is_some_and(|exported| export_name(tree, exported) == name)
                        })
                    else {
                        continue;
                    };
                    let target = crate::lang::resolve(file, source, setting.root)?;
                    let local = export_name(tree, tree.child(specifier, "local")?);
                    return exported(&target, &local, depth + 1, setting);
                }
                let declared = tree
                    .child(statement, "declaration")
                    .and_then(|declaration| match tree.kind(declaration) {
                        "FunctionDeclaration" | "ClassDeclaration" => tree
                            .child(declaration, "id")
                            .filter(|id| tree.text(*id, "name") == Some(name))
                            .and_then(|id| scopes.symbol_at(tree.span(id).start)),
                        "VariableDeclaration" => tree
                            .list(declaration, "declarations")
                            .into_iter()
                            .flatten()
                            .filter_map(|declarator| tree.child(declarator, "id"))
                            .flat_map(|id| bound_symbols(tree, scopes, id))
                            .find(|symbol| scopes.symbol(*symbol).name == name),
                        _ => None,
                    });
                declared.or_else(|| {
                    tree.list(statement, "specifiers")
                        .into_iter()
                        .flatten()
                        .find(|&specifier| {
                            tree.child(specifier, "exported")
                                .is_some_and(|exported| export_name(tree, exported) == name)
                        })
                        .and_then(|specifier| tree.child(specifier, "local"))
                        .and_then(|local| scopes.top_level(&export_name(tree, local)))
                })
            }
            "ExportDefaultDeclaration" if name == "default" => {
                let declaration = tree.child(statement, "declaration")?;
                return Some(format!(
                    "export-default:{}",
                    digest(&normalizer.at_depth(declaration, depth))
                ));
            }
            _ => None,
        };
        if let Some(symbol) = symbol {
            let identity = normalizer.external(symbol, depth);
            return (!identity.starts_with("sym:")).then_some(identity);
        }
    }
    None
}
