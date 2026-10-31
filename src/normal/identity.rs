//! Identities: how a binding declared outside the normalized node is named.

use super::{digest, form, Normalizer, Setting, Structure, Walk};
use crate::lang::Scopes;
use crate::lang::{bound_symbols, export_name, Declaration, Imported, NodeId, SymbolId};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Ends the identity of a binding that holds one object (a function, or a `const` that allocates
/// one): equal declarations are one identity, but each binding is its own object, numbered within
/// a normal form so `a === a` and `a === b` differ.
const OBJECT: &str = ":object";

/// A binding's declaration span, as the identity's own stable discriminator.
fn symbol_span(scopes: &Scopes, symbol: SymbolId) -> u32 {
    scopes.symbol(symbol).span.start
}

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
        let mut identity = match (self.externals.get(&symbol), &mut self.discovering) {
            (Some(known), _) => known.clone(),
            // Listing what a declaration reads: the read is recorded, not followed.
            (None, Some(found)) => {
                found.push(symbol);
                return "?".to_owned();
            }
            (None, None) => self.external(symbol),
        };
        if identity.ends_with(OBJECT) {
            if walk.trace {
                // The trace's policy: named by its declaration alone. The declaration's position
                // disambiguates two bindings whose declarations and uses are identical — which
                // the flat form's per-walk ordinal would have told apart — and is stable across
                // every test of the file, which is all the same-file rules need.
                identity = format!("{identity}@{}", symbol_span(self.scopes, symbol));
            } else {
                let next = walk.objects.len();
                let number = *walk.objects.entry(symbol).or_insert(next);
                identity = format!("{identity}#{number}");
            }
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
    ///
    /// The identity depends on the declaration alone, never on which reader asked first: what a
    /// declaration reads is named before it, and a cycle of declarations is named by identity.
    pub fn external(&mut self, symbol: SymbolId) -> String {
        if let Some(known) = self.externals.get(&symbol) {
            return known.clone();
        }
        if self.resolving {
            debug_assert!(false, "a declaration read a binding its listing missed");
            return self.opaque(symbol);
        }
        self.resolving = true;
        self.resolve(symbol);
        self.resolving = false;
        self.externals[&symbol].clone()
    }

    /// The identity of a binding that equals only itself.
    fn opaque(&self, symbol: SymbolId) -> String {
        format!(
            "sym:{}:{}",
            self.path.display(),
            self.scopes.symbol(symbol).span.start
        )
    }

    /// Names `start` and every unnamed declaration it reads, in an order where each is written
    /// after what it reads (Tarjan's strongly connected components, iteratively): each declaration
    /// is written once, on a stack one walk deep, and a component that reads itself is opaque.
    fn resolve(&mut self, start: SymbolId) {
        let mut state = Components::default();
        state.enter(start, self.reads(start));
        while let Some(visit) = state.visits.last_mut() {
            if let Some(&read) = visit.reads.get(visit.next) {
                visit.next += 1;
                let symbol = visit.symbol;
                if self.externals.contains_key(&read) {
                    continue;
                }
                match state.order.get(&read) {
                    Some(&(index, _)) if state.on_open.contains(&read) => {
                        state.lower(symbol, index)
                    }
                    Some(_) => {}
                    None => {
                        let reads = self.reads(read);
                        state.enter(read, reads);
                    }
                }
                continue;
            }
            let Visit { symbol, reads, .. } = state.visits.pop().unwrap();
            let (index, low) = state.order[&symbol];
            if let Some(parent) = state.visits.last().map(|visit| visit.symbol) {
                state.lower(parent, low);
            }
            if low != index {
                continue;
            }
            let at = state
                .open
                .iter()
                .rposition(|&member| member == symbol)
                .unwrap();
            let component: Vec<SymbolId> = state.open.drain(at..).collect();
            let cyclic = component.len() > 1 || reads.contains(&symbol);
            for member in component {
                state.on_open.remove(&member);
                let identity = if cyclic {
                    self.opaque(member)
                } else {
                    self.named_by_declaration(member)
                };
                self.externals.insert(member, identity);
            }
        }
    }

    /// The outside bindings `symbol`'s declaration reads, unnamed ones in the order its walks meet
    /// them: the walks run with each such read recorded instead of followed.
    fn reads(&mut self, symbol: SymbolId) -> Vec<SymbolId> {
        let saved = self.discovering.replace(Vec::new());
        let nesting = std::mem::take(&mut self.nesting);
        let _ = self.declared(symbol);
        self.nesting = nesting;
        let mut reads = std::mem::replace(&mut self.discovering, saved).unwrap_or_default();
        let mut seen = HashSet::new();
        reads.retain(|read| seen.insert(*read));
        reads
    }

    /// `symbol`'s identity by its declaration, every read already named; its own walks start at
    /// depth zero, whoever asked.
    fn named_by_declaration(&mut self, symbol: SymbolId) -> String {
        let nesting = std::mem::take(&mut self.nesting);
        let named = self.declared(symbol).unwrap_or_else(|| self.opaque(symbol));
        self.nesting = nesting;
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
    fn declared(&mut self, symbol: SymbolId) -> Option<String> {
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
                // equal helpers in two modules are one identity. Inside such a module, its own
                // imports are named by where they resolve: one module deep, never a cycle.
                if !self.shallow_imports {
                    if let Some(file) = crate::lang::resolve(self.path, source, self.setting.root) {
                        if let Some(exported) = exported(&file, &imported, self.setting) {
                            return Some(exported);
                        }
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
                let pattern = if simple { String::new() } else { self.of(id) };
                let init_node = tree.child(declarator, "init");
                let init = init_node.map(|init| self.of(init)).unwrap_or_default();
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
                let uses = digest(&self.registration_uses(symbol));
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
            Declaration::Function(function) => {
                Some(format!("fn:{level}:{}{OBJECT}", digest(&self.of(function))))
            }
            Declaration::Class(class) => {
                Some(format!("class:{level}:{name}:{}", digest(&self.of(class))))
            }
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
    fn registration_uses(&mut self, symbol: SymbolId) -> String {
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
            .map(|statement| self.substituted(statement, substitutions.clone(), false))
            .collect::<Vec<_>>()
            .join(";")
    }
}

/// A declaration being visited by [`Normalizer::resolve`]: what it reads and how much of that is
/// done.
struct Visit {
    symbol: SymbolId,
    reads: Vec<SymbolId>,
    next: usize,
}

/// The state of Tarjan's algorithm over declarations.
#[derive(Default)]
struct Components {
    /// Each visited declaration's visit index and lowest index reachable.
    order: HashMap<SymbolId, (usize, usize)>,
    /// Declarations visited but not yet in a finished component.
    open: Vec<SymbolId>,
    on_open: HashSet<SymbolId>,
    visits: Vec<Visit>,
}

impl Components {
    /// Starts visiting `symbol`, which reads `reads`.
    fn enter(&mut self, symbol: SymbolId, reads: Vec<SymbolId>) {
        let index = self.order.len();
        self.order.insert(symbol, (index, index));
        self.open.push(symbol);
        self.on_open.insert(symbol);
        self.visits.push(Visit {
            symbol,
            reads,
            next: 0,
        });
    }

    /// Lowers `symbol`'s lowest reachable index to `index`.
    fn lower(&mut self, symbol: SymbolId, index: usize) {
        let low = &mut self.order.get_mut(&symbol).unwrap().1;
        *low = (*low).min(index);
    }
}

/// The identity of what `file` exports as `name`, followed through re-exports; `None` when it is
/// not a declaration this names, or the re-exports cycle without reaching one.
fn exported(file: &Path, name: &str, setting: Setting) -> Option<String> {
    let key = (file.to_path_buf(), name.to_owned());
    if let Some(known) = setting.exports.known.borrow().get(&key) {
        return known.clone();
    }
    let found = export_of(file, name, setting);
    setting
        .exports
        .known
        .borrow_mut()
        .insert(key, found.clone());
    found
}

/// Follows re-exports from `file`'s `name` to the module that declares it and names the
/// declaration there. A re-export chain is a lookup, not a walk: one that cycles reaches no
/// declaration from any of its entries.
fn export_of(file: &Path, name: &str, setting: Setting) -> Option<String> {
    let mut at = (file.to_path_buf(), name.to_owned());
    let mut visited: HashSet<(PathBuf, String)> = HashSet::new();
    while visited.insert(at.clone()) {
        let (file, name) = &at;
        let source = crate::discovery::read_bounded(file, crate::discovery::MAX_SOURCE_FILE_BYTES)
            .ok()??;
        let parsed = crate::lang::parse(file, &source).ok()?;
        match declaration_of(file, name, &parsed, setting)? {
            Export::Named(identity) => return identity,
            Export::Reexport(next) => at = next,
        }
    }
    None
}

/// What a module's export is: named here, or re-exported from another module.
enum Export {
    /// Declared in this module: its identity, `None` when it equals only itself.
    Named(Option<String>),
    /// Re-exported from another module under a name.
    Reexport((PathBuf, String)),
}

/// What `file` (parsed as `parsed`) exports as `name`; `None` when it exports no such name.
fn declaration_of(
    file: &Path,
    name: &str,
    parsed: &crate::lang::Parsed,
    setting: Setting,
) -> Option<Export> {
    let structure = Structure::default();
    let mut normalizer = Normalizer::new(file, parsed, setting, &structure);
    normalizer.shallow_imports = true;
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
                    return Some(Export::Reexport((target, local)));
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
                return Some(Export::Named(Some(format!(
                    "export-default:{}",
                    digest(&normalizer.of(declaration))
                ))));
            }
            _ => None,
        };
        if let Some(symbol) = symbol {
            let identity = normalizer.external(symbol);
            return Some(Export::Named(
                (!identity.starts_with("sym:")).then_some(identity),
            ));
        }
    }
    None
}
