//! The inventory of one module.

use super::{
    Escape, Export, Inventory, Label, Modules, Provenance, Reach, Setting, Site, SiteKind, ON_LOAD,
};
use crate::framework::{Chain, Registers};
use crate::lang::{
    bound_symbols, export_name, member_name, property_name, Declaration, Name, NodeId, Parsed,
    Span, SymbolId, Tree,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    /// A registration function (or one built from it) registering this.
    Api(Registers),
    /// The API as a namespace: `import * as v`, the in-source member.
    Namespace,
    /// A function whose call may register, and what.
    Wrapper(Reach),
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
    /// Functions of this module whose body holds a registration, by the symbol naming them, with
    /// what calling them may register.
    wrappers: HashMap<SymbolId, Reach>,
    loads: Vec<u32>,
    values: HashSet<NodeId>,
    /// Bindings declared more than once, replacing their value.
    redeclared: HashSet<SymbolId>,
    /// Whether a direct `eval` may write any binding of the module.
    evaluates: bool,
    /// Globals the module writes through the global object, `None` for a key only known when it
    /// runs, which may be any of them.
    rewritten_globals: HashSet<Option<String>>,
}

/// The inventory of a module, and the export names whose value registers when called with what
/// each may register (with [`ON_LOAD`] when the module may register while it loads).
pub(super) fn analyse(
    path: &Path,
    source: &str,
    parsed: &Parsed,
    setting: Setting,
    modules: &mut Modules,
    depth: usize,
) -> (Inventory, HashMap<String, Export>) {
    let mut module = Module {
        path,
        source,
        tree: &parsed.tree,
        parsed,
        setting,
        origins: HashMap::new(),
        wrappers: HashMap::new(),
        loads: Vec::new(),
        values: HashSet::new(),
        redeclared: HashSet::new(),
        evaluates: false,
        rewritten_globals: HashSet::new(),
    };
    module.redeclared = module.redeclarations();
    module.evaluates = module.has_direct_eval();
    module.rewritten_globals = module.global_writes();
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
        exports.insert(ON_LOAD.to_owned(), Export::Wrapper(Reach::Tasks));
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
                Name::Reference(Some(symbol)) if self.wrappers.contains_key(&symbol) => {
                    Some((Origin::Wrapper(self.wrappers[&symbol]), Provenance::Wrapper))
                }
                Name::Reference(Some(symbol)) => self.origins.get(&symbol).map(|origin| {
                    let provenance = match origin {
                        Origin::Wrapper(_) => Provenance::Wrapper,
                        _ => Provenance::Direct,
                    };
                    (*origin, provenance)
                }),
                Name::Reference(None) if self.setting.globals => {
                    let name = tree.text(node, "name")?;
                    let registration = self.setting.dialect.registration(name)?;
                    let rewritten = self.rewritten_globals.contains(&None)
                        || self.rewritten_globals.contains(&Some(name.to_owned()));
                    Some(if rewritten {
                        (Origin::Wrapper(Reach::Tasks), Provenance::Global)
                    } else {
                        (Origin::Api(registration.registers), Provenance::Global)
                    })
                }
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
                        (Some(Origin::Wrapper(_)), _) | (_, Some(Origin::Wrapper(_))) => {
                            steps.reverse();
                            return Some((
                                Origin::Wrapper(Reach::Tasks),
                                Provenance::Wrapper,
                                steps,
                            ));
                        }
                        _ => return None,
                    };
                    steps.reverse();
                    return Some((Origin::Api(registers), Provenance::Direct, steps));
                }
                "MemberExpression" => {
                    // `cases['with organization']` reads as `cases.name` does.
                    steps.push(Step::Member(member_name(tree, node)?));
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
                    // What it returns is not read: it may register anything.
                    return Some((Origin::Wrapper(Reach::Tasks), Provenance::Wrapper, steps));
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
        if let Origin::Wrapper(reach) = origin {
            // An alias of the wrapper keeps its reach; anything derived from it may register tests.
            let reach = if steps.is_empty() {
                reach
            } else {
                Reach::Tasks
            };
            return Some(Origin::Wrapper(reach));
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
        if let Origin::Wrapper(reach) = origin {
            // A wrapper, or a value derived from one: calling it may register. Only the call of
            // the wrapper itself is known to reach what its body does; a member or a call of its
            // result is any value.
            let reach = if steps.is_empty() {
                reach
            } else {
                Reach::Tasks
            };
            return Some((SiteKind::Wrapper(reach), chain, provenance));
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
                .is_some_and(|exports| exports.contains_key(ON_LOAD))
            {
                self.loads.push(tree.span(statement).start);
            }
            if !api && wrapper_exports.as_ref().is_none_or(HashMap::is_empty) {
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
                    Some(exports) => match exports.get(&name) {
                        Some(Export::Namespace) => {
                            self.origins.insert(symbol, Origin::Namespace);
                        }
                        Some(&Export::Api(registers)) => {
                            self.origins.insert(symbol, Origin::Api(registers));
                        }
                        Some(&Export::Wrapper(reach)) => {
                            self.origins.insert(symbol, Origin::Wrapper(reach));
                        }
                        None => {}
                    },
                }
            }
        }
        self.collect_aliases();
    }

    /// `const t = test`, `const myTest = test.extend({...})`, `const { test } = import.meta.vitest`.
    fn collect_aliases(&mut self) {
        let tree = self.tree;
        loop {
            let before = self.origins.clone();
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
                            self.alias(symbol, origin);
                        }
                    }
                    // A destructured binding holds a member of the wrapper, which may register
                    // tests whatever the wrapper registers when called.
                    "ObjectPattern" | "ArrayPattern"
                        if matches!(origin, Some(Origin::Wrapper(_))) =>
                    {
                        for symbol in bound_symbols(tree, &self.parsed.scopes, id) {
                            self.alias(symbol, Origin::Wrapper(Reach::Tasks));
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
                                    self.alias(symbol, Origin::Api(registers));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            if self.origins == before {
                break;
            }
        }
    }

    /// Records that `symbol` holds `origin`. A wrapper alias takes the widest reach seen, since a
    /// wrapper's reach only grows while the fixpoint runs; any other origin is set once.
    fn alias(&mut self, symbol: SymbolId, origin: Origin) {
        let origin = match origin {
            _ if self.reassigned(symbol) => Origin::Wrapper(Reach::Tasks),
            origin => origin,
        };
        let entry = self.origins.entry(symbol).or_insert(origin);
        if let (Origin::Wrapper(held), Origin::Wrapper(reach)) = (*entry, origin) {
            *entry = Origin::Wrapper(held.max(reach));
        }
    }

    /// Every registering call, with wrappers found to a fixpoint: a function whose body holds a
    /// registering call registers when called. Each starts as hooks-only when every registration
    /// in it is a hook or a hooks-only wrapper call; then any whose body is not closed may
    /// register tests, which may open the wrappers calling it, until none changes.
    fn registrations(&mut self) -> Vec<Found> {
        loop {
            let found = self.scan();
            let before = self.wrappers.clone();
            for (function, symbols) in self.functions() {
                let Some(reach) = self.reach(self.tree.span(function), &found) else {
                    continue;
                };
                for symbol in symbols {
                    let held = self.wrappers.entry(symbol).or_insert(reach);
                    *held = (*held).max(reach);
                }
            }
            let aliases = self.origins.clone();
            self.collect_aliases();
            if self.wrappers == before && self.origins == aliases {
                break;
            }
        }
        loop {
            let open: Vec<SymbolId> = self
                .functions()
                .into_iter()
                .filter(|(function, symbols)| {
                    symbols
                        .iter()
                        .any(|symbol| self.wrappers.get(symbol) == Some(&Reach::Hooks))
                        && (!self.closed(*function)
                            || symbols.iter().any(|&symbol| self.reassigned(symbol)))
                })
                .flat_map(|(_, symbols)| symbols)
                .collect();
            for symbol in &open {
                self.wrappers.insert(*symbol, Reach::Tasks);
            }
            let aliases = self.origins.clone();
            self.collect_aliases();
            if open.is_empty() && self.origins == aliases {
                return self.scan();
            }
        }
    }

    /// Whether `symbol` may hold anything but what it was declared with: it is declared more than
    /// once, something writes it, or a direct `eval` may. Every reading of a name's value asks this.
    fn reassigned(&self, symbol: SymbolId) -> bool {
        self.evaluates
            || self.redeclared.contains(&symbol)
            || self
                .parsed
                .scopes
                .symbol(symbol)
                .references
                .iter()
                .any(|reference| reference.write)
    }

    /// The bindings more than one declaration binds: patterns, parameters and catch bindings
    /// included. A type sharing a name binds no value, so it is not one.
    fn redeclarations(&self) -> HashSet<SymbolId> {
        let tree = self.tree;
        let mut seen = HashSet::new();
        tree.nodes()
            .flat_map(|node| {
                let (binding, params) = match tree.kind(node) {
                    "VariableDeclarator" => (tree.child(node, "id"), false),
                    "FunctionDeclaration" => (tree.child(node, "id"), true),
                    "CatchClause" => (tree.child(node, "param"), false),
                    "FunctionExpression" | "ArrowFunctionExpression" => (None, true),
                    _ => (None, false),
                };
                let params = if params {
                    tree.list(node, "params")
                } else {
                    Vec::new()
                };
                binding.into_iter().chain(params.into_iter().flatten())
            })
            .flat_map(|binding| bound_symbols(tree, &self.parsed.scopes, binding))
            .filter(|&symbol| !seen.insert(symbol))
            .collect()
    }

    /// Whether the module calls the global `eval`, which runs in the caller's scope.
    fn has_direct_eval(&self) -> bool {
        let tree = self.tree;
        tree.nodes()
            .filter(|&node| tree.kind(node) == "CallExpression")
            .filter_map(|node| tree.child(node, "callee"))
            .map(|callee| tree.inner(callee))
            .any(|callee| {
                tree.kind(callee) == "Identifier"
                    && tree.text(callee, "name") == Some("eval")
                    && self.read(callee).is_none()
            })
    }

    /// The names the module writes through the global object, in an assignment, a pattern, a
    /// `for` head or an update, `None` for a key only known when it runs.
    fn global_writes(&self) -> HashSet<Option<String>> {
        let tree = self.tree;
        tree.nodes()
            .filter_map(|node| match tree.kind(node) {
                "AssignmentExpression" | "ForOfStatement" | "ForInStatement" => {
                    tree.child(node, "left")
                }
                "UpdateExpression" => tree.child(node, "argument"),
                _ => None,
            })
            .flat_map(|target| std::iter::once(target).chain(tree.descendants(target)))
            .filter(|&target| self.global_member(target))
            .map(|member| {
                let member = tree.inner(member);
                let property = tree.child(member, "property")?;
                let key = if !tree.flag(member, "computed") {
                    tree.text(property, "name")
                } else if tree.kind(property) == "Literal" {
                    tree.text(property, "value")
                } else {
                    None
                };
                key.map(str::to_owned)
            })
            .collect()
    }

    /// Whether `node` is a member of the global object.
    fn global_member(&self, node: NodeId) -> bool {
        let tree = self.tree;
        let node = tree.inner(node);
        tree.kind(node) == "MemberExpression"
            && tree.child(node, "object").is_some_and(|object| {
                let object = tree.inner(object);
                tree.kind(object) == "Identifier"
                    && matches!(
                        tree.text(object, "name"),
                        Some("globalThis" | "global" | "window" | "self")
                    )
                    && self.read(object).is_none()
            })
    }

    /// Whether `symbol` is only declared to the type checker: `declare` gives it no value.
    fn ambient(&self, symbol: SymbolId) -> bool {
        let tree = self.tree;
        match self.parsed.scopes.symbol(symbol).declaration {
            Declaration::Variable(declarator) => tree
                .outer_parent(declarator)
                .is_some_and(|declaration| tree.flag(declaration, "declare")),
            Declaration::Function(function) => tree.kind(function) == "TSDeclareFunction",
            Declaration::Class(class) => tree.flag(class, "declare"),
            _ => false,
        }
    }

    /// What calling a function whose body is `body` may register, `None` when its body holds no
    /// registration: hooks only when every registration in it is a hook or a hooks-only wrapper
    /// call. Whether the body may also run anything else is [`Self::closed`].
    fn reach(&self, body: Span, found: &[Found]) -> Option<Reach> {
        found
            .iter()
            .filter(|(_, span, ..)| body.contains(*span))
            .map(|(_, _, kind, ..)| match kind {
                SiteKind::Registers(Registers::Hook(_))
                | SiteKind::Wrapper(Reach::Hooks | Reach::Hook) => Reach::Hooks,
                SiteKind::Registers(_) | SiteKind::Wrapper(Reach::Tasks) => Reach::Tasks,
            })
            .max()
    }

    /// Whether calling `function` runs nothing but hook registrations and calls of hooks-only
    /// wrappers: plain parameters, and a body of such calls (with arguments that run nothing),
    /// declarations and returns of values that run nothing, and blocks and `if`s of those on a
    /// test that runs nothing. Any other call, read, operator or default may run code that
    /// registers a test this analysis does not see.
    fn closed(&self, function: NodeId) -> bool {
        let tree = self.tree;
        let plain = tree
            .list(function, "params")
            .into_iter()
            .all(|param| param.is_some_and(|param| tree.kind(param) == "Identifier"));
        let Some(body) = tree.child(function, "body") else {
            return false;
        };
        if !plain {
            return false;
        }
        // An async function's returned value is resolved, which may read a `then` getter.
        let returns = !tree.flag(function, "async");
        if tree.kind(body) != "BlockStatement" {
            return returns && self.hook_call(body);
        }
        self.closed_statement(body, returns)
    }

    /// Whether running `statement` runs nothing but hook registrations and calls of hooks-only
    /// wrappers (see [`Self::closed`]).
    fn closed_statement(&self, statement: NodeId, returns: bool) -> bool {
        let tree = self.tree;
        let closed = |field: &str| {
            tree.child(statement, field)
                .is_none_or(|statement| self.closed_statement(statement, returns))
        };
        match tree.kind(statement) {
            "ExpressionStatement" => tree
                .child(statement, "expression")
                .is_some_and(|call| self.hook_call(call)),
            "BlockStatement" => tree.list(statement, "body").into_iter().all(|statement| {
                statement.is_some_and(|statement| self.closed_statement(statement, returns))
            }),
            "IfStatement" => {
                tree.child(statement, "test")
                    .is_some_and(|test| self.runs_nothing(test))
                    && closed("consequent")
                    && closed("alternate")
            }
            "FunctionDeclaration"
            | "EmptyStatement"
            | "TSTypeAliasDeclaration"
            | "TSInterfaceDeclaration" => true,
            "ReturnStatement" => tree
                .child(statement, "argument")
                .is_none_or(|value| returns && self.runs_nothing(value)),
            "VariableDeclaration" => {
                matches!(tree.text(statement, "kind"), Some("const" | "let" | "var"))
                    && tree
                        .list(statement, "declarations")
                        .into_iter()
                        .all(|declarator| {
                            declarator.is_some_and(|declarator| {
                                tree.child(declarator, "id")
                                    .is_some_and(|id| tree.kind(id) == "Identifier")
                                    && tree
                                        .child(declarator, "init")
                                        .is_none_or(|value| self.runs_nothing(value))
                            })
                        })
            }
            _ => false,
        }
    }

    /// Whether `node` is a call of a hook or of a hooks-only wrapper whose arguments run nothing.
    fn hook_call(&self, node: NodeId) -> bool {
        let tree = self.tree;
        let call = tree.inner(node);
        if tree.kind(call) != "CallExpression" || !self.plain_callee(self.callee(call)) {
            return false;
        }
        match self.registration(self.callee(call)) {
            Some((
                SiteKind::Registers(Registers::Hook(_)) | SiteKind::Wrapper(Reach::Hook),
                ..,
            )) => self.hook_arguments(call),
            Some((SiteKind::Wrapper(Reach::Hooks), ..)) => self.arguments_run_nothing(call),
            _ => false,
        }
    }

    /// Whether a hook's arguments are functions and literals: the framework reads them (it compares
    /// a timeout), which may run a value's `valueOf`.
    fn hook_arguments(&self, call: NodeId) -> bool {
        let tree = self.tree;
        // A hook used as a template tag is no hook call: its substitutions run first.
        tree.kind(call) == "CallExpression"
            && tree.list(call, "arguments").into_iter().all(|argument| {
                argument.is_some_and(|argument| {
                    matches!(
                        tree.kind(tree.inner(argument)),
                        "Literal" | "FunctionExpression" | "ArrowFunctionExpression"
                    )
                })
            })
    }

    /// Whether evaluating the arguments of `call`, or a tagged template's substitutions, runs no
    /// code.
    fn arguments_run_nothing(&self, call: NodeId) -> bool {
        let tree = self.tree;
        match tree.kind(call) {
            "TaggedTemplateExpression" => tree
                .child(call, "quasi")
                .is_some_and(|quasi| tree.list(quasi, "expressions").is_empty()),
            _ => tree
                .list(call, "arguments")
                .into_iter()
                .all(|argument| argument.is_some_and(|argument| self.runs_nothing(argument))),
        }
    }

    /// Whether evaluating callee `node` runs no code and is the binding it was declared as: a
    /// lexical name never reassigned, the API as a global, or a member of an API namespace.
    fn plain_callee(&self, node: NodeId) -> bool {
        let tree = self.tree;
        let node = tree.inner(node);
        match tree.kind(node) {
            "Identifier" => self
                .read(node)
                .map_or(self.setting.globals, |symbol| !self.reassigned(symbol)),
            "MemberExpression" => {
                !tree.flag(node, "computed")
                    && tree.child(node, "object").is_some_and(|object| {
                        let object = tree.inner(object);
                        tree.kind(object) == "Identifier"
                            && self.read(object).is_some_and(|symbol| {
                                self.origins.get(&symbol) == Some(&Origin::Namespace)
                                    && !self.reassigned(symbol)
                            })
                    })
            }
            _ => false,
        }
    }

    /// Whether evaluating `node` runs no code: a literal, a lexical name, a function, a template without
    /// substitutions, or an array or object of those (no spread, no computed key).
    fn runs_nothing(&self, node: NodeId) -> bool {
        let tree = self.tree;
        let node = tree.inner(node);
        match tree.kind(node) {
            "Literal" | "FunctionExpression" | "ArrowFunctionExpression" => true,
            // A global may be an accessor; these three are fixed properties of the global object.
            "Identifier" => {
                self.read(node).is_some_and(|symbol| !self.ambient(symbol))
                    || matches!(
                        tree.text(node, "name"),
                        Some("undefined" | "NaN" | "Infinity")
                    )
            }
            "TemplateLiteral" => tree.list(node, "expressions").is_empty(),
            "ArrayExpression" => tree
                .list(node, "elements")
                .into_iter()
                .flatten()
                .all(|element| self.runs_nothing(element)),
            "ObjectExpression" => tree.list(node, "properties").into_iter().all(|property| {
                property.is_some_and(|property| {
                    tree.kind(property) == "Property"
                        && !tree.flag(property, "computed")
                        && tree
                            .child(property, "value")
                            .is_some_and(|value| self.runs_nothing(value))
                })
            }),
            _ => false,
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
            .filter(|(_, symbols)| {
                symbols
                    .iter()
                    .any(|symbol| self.wrappers.contains_key(symbol))
            })
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
                let (mut kind, chain, provenance) = self.registration(callee)?;
                if self.unreachable(node) {
                    return None;
                }
                // Arguments are evaluated before the call, and may register anything.
                let inert = match kind {
                    SiteKind::Wrapper(Reach::Hooks) => self.arguments_run_nothing(node),
                    SiteKind::Wrapper(Reach::Hook) => self.hook_arguments(node),
                    _ => true,
                };
                if !inert {
                    kind = SiteKind::Wrapper(Reach::Tasks);
                }
                Some((node, tree.span(node), kind, chain, provenance))
            })
            .collect()
    }

    /// Whether `node`, or a function expression holding it, follows a `return` or `throw` in a
    /// statement list. The walk stops at a function declaration, which is hoisted and may be
    /// called. Only blocks are read, so any other way past a statement stays reachable.
    fn unreachable(&self, node: NodeId) -> bool {
        let tree = self.tree;
        let mut inner = node;
        for outer in tree.ancestors(node) {
            if tree.kind(outer) == "FunctionDeclaration" {
                return false;
            }
            if matches!(tree.kind(outer), "BlockStatement" | "Program")
                && tree
                    .list(outer, "body")
                    .into_iter()
                    .flatten()
                    .take_while(|&statement| statement != inner)
                    .any(|statement| {
                        matches!(tree.kind(statement), "ReturnStatement" | "ThrowStatement")
                    })
            {
                return true;
            }
            inner = outer;
        }
        false
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
            .chain(self.wrappers.keys())
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

    /// Every name this module spells in an export of its own, value or type: whether one replaces
    /// the same name from an `export *` depends on what the name erases to, which is not read here.
    fn spelled_exports(&self) -> HashSet<String> {
        let tree = self.tree;
        let mut names = HashSet::new();
        for statement in tree.list(tree.root(), "body").into_iter().flatten() {
            match tree.kind(statement) {
                "ExportAllDeclaration" => {
                    names.extend(
                        tree.child(statement, "exported")
                            .map(|exported| export_name(tree, exported)),
                    );
                }
                "ExportNamedDeclaration" => {
                    names.extend(
                        specifiers(tree, statement)
                            .into_iter()
                            .map(|(_, exported)| export_name(tree, exported)),
                    );
                    let Some(declaration) = tree.child(statement, "declaration") else {
                        continue;
                    };
                    let bindings = tree
                        .list(declaration, "declarations")
                        .into_iter()
                        .flatten()
                        .filter_map(|declarator| tree.child(declarator, "id"))
                        .flat_map(|id| bound_symbols(tree, &self.parsed.scopes, id));
                    names.extend(
                        bindings.map(|symbol| self.parsed.scopes.symbol(symbol).name.clone()),
                    );
                    names.extend(
                        tree.child(declaration, "id")
                            .and_then(|id| tree.text(id, "name"))
                            .map(str::to_owned),
                    );
                }
                _ => {}
            }
        }
        names
    }

    /// Export names whose value registers when called, with what each may register.
    fn exports(
        &self,
        modules: &mut Modules,
        depth: usize,
        found: &[Found],
    ) -> HashMap<String, Export> {
        let tree = self.tree;
        let spelled = self.spelled_exports();
        let dialect = self.setting.dialect;
        // Stars read so far: the first star holding a name wins it, and this table does not record
        // a name one holds without registering.
        let mut stars = 0;
        let mut exports: HashMap<String, Export> = HashMap::new();
        let mut add = |name: String, export: Export| {
            let held = exports.entry(name).or_insert(export);
            *held = held.widest(export);
        };
        let registers = |symbol: Option<SymbolId>| -> Option<Export> {
            let symbol = symbol?;
            let export = match self.wrappers.get(&symbol) {
                Some(&reach) => Export::Wrapper(reach),
                None => match self.origins.get(&symbol)? {
                    Origin::Api(Registers::Hook(_)) => Export::Wrapper(Reach::Hook),
                    Origin::Api(_) => Export::Wrapper(Reach::Tasks),
                    Origin::Wrapper(reach) => Export::Wrapper(*reach),
                    Origin::Namespace => Export::Namespace,
                },
            };
            // A reassigned binding may hold any value when an importer calls it.
            Some(if self.reassigned(symbol) {
                Export::Wrapper(Reach::Tasks)
            } else {
                export
            })
        };
        for statement in tree.list(tree.root(), "body").into_iter().flatten() {
            if type_only(tree, statement) {
                continue;
            }
            match tree.kind(statement) {
                "ExportNamedDeclaration" => {
                    let source = tree
                        .child(statement, "source")
                        .and_then(|s| tree.text(s, "value"));
                    if let Some(source) = source {
                        if dialect.is_api_module(source) {
                            for (local, exported) in specifiers(tree, statement) {
                                if let Some(registers) = self.api(&export_name(tree, local)) {
                                    add(export_name(tree, exported), Export::Api(registers));
                                }
                            }
                            continue;
                        }
                        let Some(file) = crate::lang::resolve(self.path, source, self.setting.root)
                        else {
                            continue;
                        };
                        let theirs = modules.exports(&file, self.setting, depth + 1);
                        // Re-exporting any name loads the module, and whatever it registers then.
                        if theirs.contains_key(ON_LOAD) {
                            add(ON_LOAD.to_owned(), Export::Wrapper(Reach::Tasks));
                        }
                        for (local, exported) in specifiers(tree, statement) {
                            if let Some(&export) = theirs.get(&export_name(tree, local)) {
                                add(export_name(tree, exported), export);
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
                            if let (Some(export), Some(name)) =
                                (registers(self.declares(id)), tree.text(id, "name"))
                            {
                                add(name.to_owned(), export);
                            }
                        }
                    }
                    for (local, exported) in specifiers(tree, statement) {
                        if let Some(export) = registers(self.read(local)) {
                            add(export_name(tree, exported), export);
                        }
                    }
                }
                "ExportDefaultDeclaration" => {
                    let Some(declaration) = tree.child(statement, "declaration") else {
                        continue;
                    };
                    let declaration = tree.inner(declaration);
                    let symbol = match tree.kind(declaration) {
                        "Identifier" => self.read(declaration),
                        _ => tree
                            .child(declaration, "id")
                            .and_then(|id| self.declares(id)),
                    };
                    let anonymous = (tree.is_function(declaration)
                        && tree.child(declaration, "id").is_none())
                    .then(|| {
                        let reach = self.reach(tree.span(declaration), found)?;
                        Some(if self.closed(declaration) {
                            reach
                        } else {
                            Reach::Tasks
                        })
                    })
                    .flatten();
                    if let Some(export) = registers(symbol).or(anonymous.map(Export::Wrapper)) {
                        add("default".to_owned(), export);
                    }
                }
                "ExportAllDeclaration" => {
                    let Some(source) = tree
                        .child(statement, "source")
                        .and_then(|s| tree.text(s, "value"))
                    else {
                        continue;
                    };
                    let name = tree.child(statement, "exported");
                    if let (Some(name), true) = (name, dialect.is_api_module(source)) {
                        add(export_name(tree, name), Export::Namespace);
                    }
                    if name.is_some() {
                        continue;
                    }
                    let forwarded = |name: &str, export: Export| match export {
                        Export::Wrapper(_) => export,
                        _ if stars == 0 && !spelled.contains(name) => export,
                        _ => Export::Wrapper(Reach::Tasks),
                    };
                    if dialect.is_api_module(source) {
                        for registration in dialect.registrations {
                            let export =
                                forwarded(registration.name, Export::Api(registration.registers));
                            add(registration.name.to_owned(), export);
                        }
                    } else if let Some(file) =
                        crate::lang::resolve(self.path, source, self.setting.root)
                    {
                        for (name, export) in modules.exports(&file, self.setting, depth + 1) {
                            if name != "default" {
                                let export = forwarded(&name, export);
                                add(name, export);
                            }
                        }
                    }
                    stars += 1;
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

/// Whether an export statement or specifier is erased: it loads nothing and exports no value.
fn type_only(tree: &Tree, node: NodeId) -> bool {
    tree.text(node, "exportKind") == Some("type")
}

/// The `(local, exported)` names of an export statement's specifiers.
fn specifiers(tree: &Tree, statement: NodeId) -> Vec<(NodeId, NodeId)> {
    tree.list(statement, "specifiers")
        .into_iter()
        .flatten()
        .filter(|&specifier| !type_only(tree, specifier))
        .filter_map(|specifier| {
            Some((
                tree.child(specifier, "local")?,
                tree.child(specifier, "exported")?,
            ))
        })
        .collect()
}
