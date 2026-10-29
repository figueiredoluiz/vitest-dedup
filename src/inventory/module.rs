//! The inventory of one module.

use super::{Escape, Inventory, Label, Modules, Provenance, Setting, Site, SiteKind, ON_LOAD};
use crate::framework::{Chain, Registers};
use crate::lang::{
    bound_symbols, export_name, property_name, Name, NodeId, Parsed, Span, SymbolId, Tree,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    /// A registration function (or one built from it) registering this.
    Api(Registers),
    /// The API as a namespace: `import * as v`, the in-source member.
    Namespace,
    /// A function whose call may register.
    Wrapper,
}

/// One step of a callee chain, from the root outwards.
#[derive(Clone, Copy, Debug)]
enum Step<'t> {
    Member(&'t str),
    Call,
}

/// The analysis of one module.
struct Module<'s> {
    path: &'s Path,
    source: &'s str,
    tree: &'s Tree,
    parsed: &'s Parsed,
    setting: Setting<'s>,
    origins: HashMap<SymbolId, Origin>,
    /// Functions of this module whose body holds a registration, by the symbol naming them.
    wrappers: HashSet<SymbolId>,
    loads: Vec<u32>,
    values: HashSet<NodeId>,
}

/// The inventory of a module, and the export names whose value registers when called (with
/// [`ON_LOAD`] when the module may register while it loads).
pub(super) fn analyse(
    path: &Path,
    source: &str,
    parsed: &Parsed,
    setting: Setting,
    modules: &mut Modules,
    depth: usize,
) -> (Inventory, HashSet<String>) {
    let mut module = Module {
        path,
        source,
        tree: &parsed.tree,
        parsed,
        setting,
        origins: HashMap::new(),
        wrappers: HashSet::new(),
        loads: Vec::new(),
        values: HashSet::new(),
    };
    module.collect_origins(modules, depth);
    let found = module.registrations();
    let mut exports = module.exports(modules, depth, &found);
    // A registration outside every wrapper may run while the module loads: at top level, in a
    // getter or an initializer, or in a function this analysis does not see called.
    let bodies = module.wrapper_bodies();
    if !module.loads.is_empty()
        || found
            .iter()
            .any(|(_, span, ..)| !bodies.iter().any(|body| body.contains(*span)))
    {
        exports.insert(ON_LOAD.to_owned());
    }
    let escapes = module.escapes();
    let calls: HashSet<NodeId> = found.iter().map(|(node, ..)| *node).collect();
    let sites = found
        .into_iter()
        .map(|(node, span, kind, chain, provenance)| {
            let callee = module.callee(node);
            Site {
                node,
                span,
                kind,
                chain,
                provenance,
                title: module.title(node),
                position: module.v8_position(callee),
                from: module
                    .mapped_from(callee)
                    .unwrap_or_else(|| module.v8_position(callee)),
                labels: module.labels(node, &calls),
            }
        })
        .collect();
    let inventory = Inventory {
        sites,
        loads: module.loads,
        escapes,
        registration_values: module.values,
    };
    (inventory, exports)
}

/// A registering call: node, span, kind, chain and provenance.
type Found = (NodeId, Span, SiteKind, Vec<String>, Provenance);

impl<'s> Module<'s> {
    /// The symbol an identifier node reads.
    fn read(&self, node: NodeId) -> Option<SymbolId> {
        match self.parsed.scopes.name_at(self.tree.span(node).start)? {
            Name::Reference(symbol) => symbol,
            Name::Binding(_) => None,
        }
    }

    /// The symbol a binding identifier node declares.
    fn declares(&self, node: NodeId) -> Option<SymbolId> {
        match self.parsed.scopes.name_at(self.tree.span(node).start)? {
            Name::Binding(symbol) => symbol,
            Name::Reference(_) => None,
        }
    }

    /// The origin of a chain root, and whether it is a free global.
    fn root_origin(&self, node: NodeId) -> Option<(Origin, Provenance)> {
        let tree = self.tree;
        match tree.kind(node) {
            "MemberExpression" if !tree.flag(node, "computed") => {
                let object = tree.child(node, "object")?;
                let property = tree.child(node, "property")?;
                let in_source = tree.kind(object) == "MetaProperty"
                    && tree
                        .child(object, "meta")
                        .and_then(|meta| tree.text(meta, "name"))
                        == Some("import")
                    && self.setting.dialect.in_source_member.is_some()
                    && tree.text(property, "name") == self.setting.dialect.in_source_member;
                in_source.then_some((Origin::Namespace, Provenance::Direct))
            }
            "Identifier" => match self.parsed.scopes.name_at(tree.span(node).start)? {
                Name::Reference(Some(symbol)) if self.wrappers.contains(&symbol) => {
                    Some((Origin::Wrapper, Provenance::Wrapper))
                }
                Name::Reference(Some(symbol)) => self.origins.get(&symbol).map(|origin| {
                    let provenance = match origin {
                        Origin::Wrapper => Provenance::Wrapper,
                        _ => Provenance::Direct,
                    };
                    (*origin, provenance)
                }),
                Name::Reference(None) if self.setting.globals => self
                    .setting
                    .dialect
                    .registration(tree.text(node, "name")?)
                    .map(|registration| (Origin::Api(registration.registers), Provenance::Global)),
                _ => None,
            },
            _ => None,
        }
    }

    /// A callee chain's root origin and the steps applied to it.
    fn chain(&self, mut node: NodeId) -> Option<(Origin, Provenance, Vec<Step<'s>>)> {
        let tree = self.tree;
        let mut steps = Vec::new();
        loop {
            node = tree.inner(node);
            if let Some((origin, provenance)) = self.root_origin(node) {
                steps.reverse();
                return Some((origin, provenance, steps));
            }
            match tree.kind(node) {
                // `only ? it.only : it`: both branches the same kind of registration function.
                "ConditionalExpression" => {
                    let branch = |key| {
                        tree.child(node, key)
                            .and_then(|branch| self.value_origin(branch))
                    };
                    let registers = match (branch("consequent"), branch("alternate")) {
                        (Some(Origin::Api(left)), Some(Origin::Api(right))) if left == right => {
                            left
                        }
                        _ => return None,
                    };
                    steps.reverse();
                    return Some((Origin::Api(registers), Provenance::Direct, steps));
                }
                "MemberExpression" => {
                    let property = tree.child(node, "property")?;
                    let name = if tree.flag(node, "computed") {
                        // `cases['with organization']`
                        let key = tree.inner(property);
                        (tree.kind(key) == "Literal").then(|| tree.text(key, "value"))??
                    } else {
                        tree.text(property, "name")?
                    };
                    steps.push(Step::Member(name));
                    node = tree.child(node, "object")?;
                }
                // `filterable(it)`: what a call handed a registration function returns may
                // register.
                "CallExpression"
                    if tree
                        .list(node, "arguments")
                        .into_iter()
                        .flatten()
                        .any(|argument| {
                            self.value_origin(argument)
                                .is_some_and(|origin| origin != Origin::Namespace)
                        }) =>
                {
                    steps.reverse();
                    return Some((Origin::Wrapper, Provenance::Wrapper, steps));
                }
                "CallExpression" => {
                    steps.push(Step::Call);
                    node = tree.child(node, "callee")?;
                }
                "TaggedTemplateExpression" => {
                    steps.push(Step::Call);
                    node = tree.child(node, "tag")?;
                }
                _ => return None,
            }
        }
    }

    /// Whether chaining `member` builds another registration function rather than registering.
    fn builds(&self, member: &str) -> bool {
        matches!(
            self.setting.dialect.chain_member(member),
            Some(
                Chain::Table(_)
                    | Chain::SkipWhen
                    | Chain::RunWhen
                    | Chain::FixtureExtension
                    | Chain::FixtureScoping
            )
        )
    }

    /// Whether a chain ending in `member` is a value that registers nothing when called.
    fn inert(&self, member: &str) -> bool {
        matches!(
            self.setting.dialect.chain_member(member),
            Some(Chain::FixtureExtension | Chain::FixtureScoping)
        )
    }

    /// What a registration name reached through a namespace registers.
    fn api(&self, name: &str) -> Option<Registers> {
        self.setting
            .dialect
            .registration(name)
            .map(|registration| registration.registers)
    }

    /// What applying `steps` to a root of `origin` registers when called, and whether the last
    /// member builds another registration function that has not been called yet; `None` when a
    /// step leaves the API (a call of a registration function, a namespace member that is not one).
    fn apply(&self, origin: Origin, steps: &[Step]) -> Option<(Option<Registers>, bool)> {
        let mut registers = match origin {
            Origin::Api(registers) => Some(registers),
            _ => None,
        };
        let mut pending = false;
        for step in steps {
            match step {
                Step::Member(name) if registers.is_none() => registers = Some(self.api(name)?),
                Step::Member(name) => pending = self.builds(name),
                Step::Call if pending => pending = false,
                Step::Call => return None,
            }
        }
        Some((registers, pending))
    }

    /// The registration function an expression's value is (`test`, `test.skip`,
    /// `test.extend({...})`, `v.describe`), or the namespace.
    fn value_origin(&self, node: NodeId) -> Option<Origin> {
        let (origin, _, steps) = self.chain(node)?;
        if origin == Origin::Wrapper {
            return Some(Origin::Wrapper);
        }
        let (registers, _) = self.apply(origin, &steps)?;
        Some(registers.map_or(Origin::Namespace, Origin::Api))
    }

    /// What calling `callee` registers, if anything.
    fn registration(&self, callee: NodeId) -> Option<(SiteKind, Vec<String>, Provenance)> {
        let (origin, provenance, steps) = self.chain(callee)?;
        let chain: Vec<String> = steps
            .iter()
            .map(|step| match step {
                Step::Member(name) => (*name).to_owned(),
                Step::Call => "()".to_owned(),
            })
            .collect();
        if origin == Origin::Wrapper {
            // A wrapper, or a value derived from one: calling it may register.
            return Some((SiteKind::Wrapper, chain, provenance));
        }
        let (registers, pending) = self.apply(origin, &steps)?;
        let registers = registers?;
        if pending || chain.last().is_some_and(|last| self.inert(last)) {
            return None;
        }
        Some((SiteKind::Registers(registers), chain, provenance))
    }

    /// Records which imported bindings are the API, its namespace, or a wrapper another module
    /// exports, then the aliases derived from them.
    fn collect_origins(&mut self, modules: &mut Modules, depth: usize) {
        let tree = self.tree;
        for statement in tree.list(tree.root(), "body").into_iter().flatten() {
            if tree.kind(statement) != "ImportDeclaration"
                || tree.text(statement, "importKind") == Some("type")
            {
                continue;
            }
            let Some(source) = tree
                .child(statement, "source")
                .and_then(|s| tree.text(s, "value"))
            else {
                continue;
            };
            let api = self.setting.dialect.is_api_module(source);
            let wrapper_exports = if api {
                None
            } else {
                crate::lang::resolve(self.path, source, self.setting.root)
                    .map(|file| modules.exports(&file, self.setting, depth + 1))
            };
            if wrapper_exports
                .as_ref()
                .is_some_and(|exports| exports.contains(ON_LOAD))
            {
                self.loads.push(tree.span(statement).start);
            }
            if !api && wrapper_exports.as_ref().is_none_or(HashSet::is_empty) {
                continue;
            }
            for specifier in tree.list(statement, "specifiers").into_iter().flatten() {
                let Some(local) = tree.child(specifier, "local") else {
                    continue;
                };
                let Some(symbol) = self.declares(local) else {
                    continue;
                };
                let name = match tree.kind(specifier) {
                    "ImportSpecifier" => match tree.child(specifier, "imported") {
                        Some(imported) => export_name(tree, imported),
                        None => continue,
                    },
                    "ImportDefaultSpecifier" => "default".to_owned(),
                    _ => {
                        if api {
                            self.origins.insert(symbol, Origin::Namespace);
                        }
                        continue;
                    }
                };
                match &wrapper_exports {
                    None => {
                        if let Some(registers) = self.api(&name) {
                            self.origins.insert(symbol, Origin::Api(registers));
                        }
                    }
                    Some(exports) if exports.contains(&name) => {
                        self.origins.insert(symbol, Origin::Wrapper);
                    }
                    Some(_) => {}
                }
            }
        }
        self.collect_aliases();
    }

    /// `const t = test`, `const myTest = test.extend({...})`, `const { test } = import.meta.vitest`.
    fn collect_aliases(&mut self) {
        let tree = self.tree;
        loop {
            let before = self.origins.len();
            for node in tree.nodes() {
                if tree.kind(node) != "VariableDeclarator" {
                    continue;
                }
                let (Some(id), Some(init)) = (tree.child(node, "id"), tree.child(node, "init"))
                else {
                    continue;
                };
                let origin = self.value_origin(init);
                if matches!(origin, Some(Origin::Api(_))) {
                    self.values.insert(tree.inner(init));
                }
                match tree.kind(id) {
                    "Identifier" => {
                        let Some(symbol) = self.declares(id) else {
                            continue;
                        };
                        if let Some(origin) = origin {
                            self.origins.entry(symbol).or_insert(origin);
                        }
                    }
                    "ObjectPattern" | "ArrayPattern" if origin == Some(Origin::Wrapper) => {
                        for symbol in bound_symbols(tree, &self.parsed.scopes, id) {
                            self.origins.entry(symbol).or_insert(Origin::Wrapper);
                        }
                    }
                    "ObjectPattern" if origin == Some(Origin::Namespace) => {
                        for property in tree.list(id, "properties").into_iter().flatten() {
                            let (Some(key), Some(value)) =
                                (property_name(tree, property), tree.child(property, "value"))
                            else {
                                continue;
                            };
                            let Some(registers) = self.api(key) else {
                                continue;
                            };
                            if tree.kind(value) == "Identifier" {
                                if let Some(symbol) = self.declares(value) {
                                    self.origins.entry(symbol).or_insert(Origin::Api(registers));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            if self.origins.len() == before {
                break;
            }
        }
    }

    /// Every registering call, with wrappers found to a fixpoint: a function whose body holds a
    /// registering call registers when called.
    fn registrations(&mut self) -> Vec<Found> {
        loop {
            let found = self.scan();
            let before = self.wrappers.len();
            for (function, symbols) in self.functions() {
                let body = self.tree.span(function);
                if found.iter().any(|(_, span, ..)| body.contains(*span)) {
                    self.wrappers.extend(symbols);
                }
            }
            let aliases = self.origins.len();
            self.collect_aliases();
            if self.wrappers.len() == before && self.origins.len() == aliases {
                return found;
            }
        }
    }

    /// Every function of the module with the symbols naming it: its own name and the `const` it
    /// alone initializes (`const run = function inner() {}` is called as either).
    fn functions(&self) -> Vec<(NodeId, Vec<SymbolId>)> {
        let tree = self.tree;
        tree.nodes()
            .filter(|&node| tree.is_function(node))
            .map(|node| {
                let own = tree.child(node, "id").and_then(|id| self.declares(id));
                let symbols = own
                    .into_iter()
                    .chain(self.declarator_symbol(node))
                    .collect();
                (node, symbols)
            })
            .collect()
    }

    /// The bodies of this module's wrappers.
    fn wrapper_bodies(&self) -> Vec<Span> {
        self.functions()
            .into_iter()
            .filter(|(_, symbols)| symbols.iter().any(|symbol| self.wrappers.contains(symbol)))
            .map(|(function, _)| self.tree.span(function))
            .collect()
    }

    /// The `const` a function expression initializes, when it is the whole initializer.
    fn declarator_symbol(&self, function: NodeId) -> Option<SymbolId> {
        let tree = self.tree;
        let parent = tree.outer_parent(function)?;
        if tree.kind(parent) != "VariableDeclarator" {
            return None;
        }
        let id = tree.child(parent, "id")?;
        (tree.kind(id) == "Identifier").then(|| self.declares(id))?
    }

    /// Every call or tagged template of the module whose callee registers, as it stands now.
    fn scan(&self) -> Vec<Found> {
        let tree = self.tree;
        tree.nodes()
            .filter_map(|node| {
                let callee = match tree.kind(node) {
                    "CallExpression" => tree.child(node, "callee")?,
                    "TaggedTemplateExpression" => tree.child(node, "tag")?,
                    _ => return None,
                };
                let (kind, chain, provenance) = self.registration(callee)?;
                Some((node, tree.span(node), kind, chain, provenance))
            })
            .collect()
    }

    /// The callee of a call, or the tag of a tagged template.
    fn callee(&self, call: NodeId) -> NodeId {
        self.tree
            .child(call, "callee")
            .or_else(|| self.tree.child(call, "tag"))
            .unwrap_or(call)
    }

    /// The title, when the first argument is a string the source spells.
    fn title(&self, call: NodeId) -> Option<String> {
        let tree = self.tree;
        let first = tree.inner(tree.list(call, "arguments").first().copied()??);
        match tree.kind(first) {
            "Literal" => tree.text(first, "value").map(str::to_owned),
            "TemplateLiteral" if tree.list(first, "expressions").is_empty() => {
                let quasi = tree.list(first, "quasis").first().copied()??;
                match tree.field_of(quasi, "value") {
                    crate::lang::Field::Object(fields) => {
                        fields
                            .iter()
                            .find_map(|(key, field)| match (&**key, field) {
                                (
                                    "cooked",
                                    crate::lang::Field::Value(serde_json::Value::String(text)),
                                ) => Some(text.clone()),
                                _ => None,
                            })
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Where V8 places a call in a stack frame (see [`Site::position`]).
    fn v8_position(&self, callee: NodeId) -> u32 {
        let tree = self.tree;
        let inner = tree.inner(callee);
        match tree.kind(inner) {
            "MemberExpression" if !tree.flag(inner, "computed") => tree
                .child(inner, "property")
                .map_or(tree.span(inner).start, |property| tree.span(property).start),
            "CallExpression" | "TaggedTemplateExpression" => {
                let end = tree.span(callee).end as usize;
                self.source[end..]
                    .find('(')
                    .map_or(end as u32, |offset| (end + offset) as u32)
            }
            _ => tree.span(inner).start,
        }
    }

    /// For a call of a call or tagged template, where the callee starts.
    fn mapped_from(&self, callee: NodeId) -> Option<u32> {
        matches!(
            self.tree.kind(self.tree.inner(callee)),
            "CallExpression" | "TaggedTemplateExpression"
        )
        .then(|| self.tree.span(callee).start)
    }

    /// Registration functions read as values rather than called.
    fn escapes(&self) -> Vec<Escape> {
        let tree = self.tree;
        let scopes = &self.parsed.scopes;
        let mut escapes = Vec::new();
        // Every read of a registration function: of a binding that holds one, and, when free names
        // are the API, of a free name the dialect registers with.
        let bound = self
            .origins
            .keys()
            .chain(self.wrappers.iter())
            .flat_map(|&symbol| scopes.symbol(symbol).references.iter())
            .filter_map(|reference| reference.node);
        let free = tree.nodes().filter(|&node| {
            self.setting.globals
                && tree.kind(node) == "Identifier"
                && scopes.is_global_at(tree.span(node).start)
                && tree
                    .text(node, "name")
                    .is_some_and(|name| self.setting.dialect.registration(name).is_some())
        });
        for read in bound.chain(free) {
            let mut current = read;
            let escaped = loop {
                let Some(parent) = tree.parent(current) else {
                    break true;
                };
                let holds = |key| tree.child(parent, key) == Some(current);
                match tree.kind(parent) {
                    "MemberExpression" if holds("object") => current = parent,
                    "ParenthesizedExpression"
                    | "TSAsExpression"
                    | "TSSatisfiesExpression"
                    | "TSNonNullExpression" => current = parent,
                    "CallExpression" if holds("callee") => break false,
                    "TaggedTemplateExpression" if holds("tag") => break false,
                    "VariableDeclarator" if holds("init") => break false,
                    "ExportSpecifier" | "ExportDefaultDeclaration" | "UnaryExpression" => {
                        break false
                    }
                    _ => break true,
                }
            };
            if escaped {
                let span = tree.span(current);
                escapes.push(Escape {
                    offset: span.start,
                    text: self.source[span.start as usize..span.end as usize]
                        .chars()
                        .take(80)
                        .collect(),
                });
            }
        }
        escapes.sort_by_key(|escape| escape.offset);
        escapes
    }

    /// Export names whose value registers when called.
    fn exports(&self, modules: &mut Modules, depth: usize, found: &[Found]) -> HashSet<String> {
        let tree = self.tree;
        let mut exports = HashSet::new();
        let registers = |symbol: Option<SymbolId>| {
            symbol.is_some_and(|symbol| {
                self.wrappers.contains(&symbol)
                    || self
                        .origins
                        .get(&symbol)
                        .is_some_and(|origin| *origin != Origin::Namespace)
            })
        };
        for statement in tree.list(tree.root(), "body").into_iter().flatten() {
            match tree.kind(statement) {
                "ExportNamedDeclaration" => {
                    let source = tree
                        .child(statement, "source")
                        .and_then(|s| tree.text(s, "value"));
                    if let Some(source) = source {
                        let Some(file) = crate::lang::resolve(self.path, source, self.setting.root)
                        else {
                            continue;
                        };
                        let theirs = modules.exports(&file, self.setting, depth + 1);
                        // Re-exporting any name loads the module, and whatever it registers then.
                        if theirs.contains(ON_LOAD) {
                            exports.insert(ON_LOAD.to_owned());
                        }
                        for (local, exported) in specifiers(tree, statement) {
                            if theirs.contains(&export_name(tree, local)) {
                                exports.insert(export_name(tree, exported));
                            }
                        }
                        continue;
                    }
                    if let Some(declaration) = tree.child(statement, "declaration") {
                        let ids: Vec<NodeId> = match tree.kind(declaration) {
                            "FunctionDeclaration" => {
                                tree.child(declaration, "id").into_iter().collect()
                            }
                            "VariableDeclaration" => tree
                                .list(declaration, "declarations")
                                .into_iter()
                                .flatten()
                                .filter_map(|declarator| tree.child(declarator, "id"))
                                .filter(|id| tree.kind(*id) == "Identifier")
                                .collect(),
                            _ => Vec::new(),
                        };
                        for id in ids {
                            if registers(self.declares(id)) {
                                exports.extend(tree.text(id, "name").map(str::to_owned));
                            }
                        }
                    }
                    for (local, exported) in specifiers(tree, statement) {
                        if registers(self.read(local)) {
                            exports.insert(export_name(tree, exported));
                        }
                    }
                }
                "ExportDefaultDeclaration" => {
                    let Some(declaration) = tree.child(statement, "declaration") else {
                        continue;
                    };
                    let symbol = match tree.kind(declaration) {
                        "Identifier" => self.read(declaration),
                        _ => tree
                            .child(declaration, "id")
                            .and_then(|id| self.declares(id)),
                    };
                    let body = tree.span(declaration);
                    let anonymous = tree.is_function(declaration)
                        && tree.child(declaration, "id").is_none()
                        && found.iter().any(|(_, span, ..)| body.contains(*span));
                    if registers(symbol) || anonymous {
                        exports.insert("default".to_owned());
                    }
                }
                "ExportAllDeclaration" if tree.child(statement, "exported").is_none() => {
                    let Some(source) = tree
                        .child(statement, "source")
                        .and_then(|s| tree.text(s, "value"))
                    else {
                        continue;
                    };
                    if let Some(file) = crate::lang::resolve(self.path, source, self.setting.root) {
                        exports.extend(
                            modules
                                .exports(&file, self.setting, depth + 1)
                                .into_iter()
                                .filter(|name| name != "default"),
                        );
                    }
                }
                _ => {}
            }
        }
        exports
    }

    /// The lexical facts around a site, innermost first.
    fn labels(&self, node: NodeId, calls: &HashSet<NodeId>) -> Vec<Label> {
        let tree = self.tree;
        let mut labels = Vec::new();
        for ancestor in tree.ancestors(node) {
            let label = match tree.kind(ancestor) {
                "ForStatement" | "ForInStatement" | "ForOfStatement" | "WhileStatement"
                | "DoWhileStatement" => Label::Loop,
                "IfStatement"
                | "ConditionalExpression"
                | "LogicalExpression"
                | "SwitchStatement" => Label::Conditional,
                "TryStatement" => Label::Try,
                _ if tree.is_function(ancestor) => match tree.outer_parent(ancestor) {
                    Some(call) if tree.kind(call) == "CallExpression" && calls.contains(&call) => {
                        Label::RegistrationBody
                    }
                    Some(call) if tree.kind(call) == "CallExpression" => Label::Callback,
                    _ => Label::Function,
                },
                _ => continue,
            };
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
        labels
    }
}

/// The `(local, exported)` names of an export statement's specifiers.
fn specifiers(tree: &Tree, statement: NodeId) -> Vec<(NodeId, NodeId)> {
    tree.list(statement, "specifiers")
        .into_iter()
        .flatten()
        .filter_map(|specifier| {
            Some((
                tree.child(specifier, "local")?,
                tree.child(specifier, "exported")?,
            ))
        })
        .collect()
}
