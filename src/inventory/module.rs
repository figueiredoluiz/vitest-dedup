//! The inventory of one module.

use super::reach::ReachIndex;
use super::{
    Escape, Export, InPlaceMock, Inventory, Label, Modules, ProjectImports, Provenance, Reach,
    Setting, Site, SiteKind, ON_LOAD, UNREADABLE_FIXTURE,
};
use crate::framework::{Chain, Mocking, Registers};
use crate::lang::{
    bound_symbols, callee_link, export_name, logical_results, member_name, property_name,
    value_role, CalleeLinkKind, Declaration, Imported, Name, NodeId, Parsed, Span, SymbolId, Tree,
    ValueRole,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[cfg(test)]
thread_local! {
    static RESOLUTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Reset and return this thread's resolver entries for deterministic work regressions.
#[cfg(test)]
pub(super) fn take_resolutions() -> usize {
    RESOLUTIONS.with(|resolutions| resolutions.replace(0))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    /// A registration function (or one built from it) registering this.
    Api(Registers),
    /// The API as a namespace: `import * as v`, the in-source member.
    Namespace,
    /// A function whose call may register, and what.
    Wrapper(Reach),
    /// An imported framework-derived registration value, not an arbitrary callable body.
    Derived(Registers),
}

impl Origin {
    /// The existing wrapper reach, retaining a separate origin for framework-derived values.
    fn reach(self) -> Option<Reach> {
        match self {
            Self::Wrapper(reach) => Some(reach),
            Self::Derived(Registers::Hook(_)) => Some(Reach::Hook),
            Self::Derived(_) => Some(Reach::Tasks),
            _ => None,
        }
    }
}

/// What binding analysis proves about a bare registration value.
#[derive(Clone, Copy)]
enum RegistrationBinding {
    /// An unwritten, named framework registration import.
    FrameworkImport,
    /// A value with registration provenance, and its declarator when it holds the callable.
    Alias(Option<NodeId>),
}

/// Which registration results may participate while alias origins settle.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AliasPhase {
    /// Settle exact API origins before joining uncertain arms.
    Exact,
    /// Collect mixed results that read proved bare aliases.
    Proved,
    /// All alias collection is complete, so the remaining API shapes may join.
    Settled,
}

/// When a call of the framework's mocking namespace changes module state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MockCall {
    /// A hoisted member written the way the framework hoists it: it applies before the imports.
    Hoisted,
    /// A hoisted member reached through any other spelling: it runs where it is written.
    InPlace,
    /// A member whose state changes only where the call runs.
    Dynamic,
}

/// One step of a callee chain, from the root outwards.
#[derive(Clone, Copy, Debug)]
enum Step<'t> {
    Member(&'t str),
    Call,
}

/// What a callee chain reads as when applied to its root.
struct Applied {
    /// What calling it registers, if anything.
    registers: Option<Registers>,
    /// Whether the last member builds another registration function not called yet.
    pending: bool,
    /// How many leading steps a test function's registration member (`test.skip.describe`)
    /// supersedes.
    start: usize,
}

/// The value, registration and source identity derived from one callee-chain application.
struct ResolvedCallee<'t> {
    /// What the expression holds, including an uncalled builder.
    value: Origin,
    /// What calling it registers, when admitted.
    registration: Option<SiteKind>,
    /// The provenance attached to an admitted registration.
    provenance: Provenance,
    /// The complete chain, including any superseded prefix.
    steps: Vec<Step<'t>>,
    /// Whether the first member selects the registration API from a namespace.
    namespace_root: bool,
    /// The first step retained in the registration record.
    start: usize,
    /// The superseding registration member, unless its prefix may run project code.
    member: Option<NodeId>,
}

/// The analysis of one module.
struct Module<'s> {
    path: &'s Path,
    source: &'s str,
    tree: &'s Tree,
    parsed: &'s Parsed,
    setting: Setting<'s>,
    origins: HashMap<SymbolId, Origin>,
    /// The alias pass in progress, or its completed state.
    alias_phase: AliasPhase,
    /// Functions of this module whose body holds a registration, by the symbol naming them, with
    /// what calling them may register.
    wrappers: HashMap<SymbolId, Reach>,
    /// Functions of this module whose returned value may be a registration function: calling
    /// what one returns may register tests.
    returners: HashSet<SymbolId>,
    /// Direct framework bindings and local aliases, with any proof of the value they hold.
    registration_bindings: HashMap<SymbolId, RegistrationBinding>,
    /// Direct calls of a task wrapper selected from an imported project namespace.
    namespace_task_calls: HashSet<NodeId>,
    loads: Vec<u32>,
    values: HashSet<NodeId>,
    /// Bindings declared more than once, replacing their value.
    redeclared: HashSet<SymbolId>,
    /// Whether a direct `eval` may write any binding of the module.
    evaluates: bool,
    /// Globals the module writes through the global object, `None` for a key only known when it
    /// runs, which may be any of them.
    rewritten_globals: HashSet<Option<String>>,
    /// Set while one scan runs: origins and wrappers cannot change until it ends, so neither can a
    /// callee chain's answer.
    scanning: std::cell::Cell<bool>,
    /// During a scan, nodes from which a callee chain reaches no registration root. A chain through
    /// one of them reaches none either, so a chain of `n` calls is walked once, not `n` times.
    dead_chains: std::cell::RefCell<HashSet<NodeId>>,
    /// An imported registration API was built with a fixture value this core cannot model.
    imported_unreadable_fixture: bool,
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
    imports: &mut ProjectImports,
) -> (Inventory, HashMap<String, Export>) {
    let mut module = Module {
        path,
        source,
        tree: &parsed.tree,
        parsed,
        setting,
        origins: HashMap::new(),
        alias_phase: AliasPhase::Exact,
        wrappers: HashMap::new(),
        returners: HashSet::new(),
        registration_bindings: HashMap::new(),
        namespace_task_calls: HashSet::new(),
        loads: Vec::new(),
        values: HashSet::new(),
        redeclared: HashSet::new(),
        evaluates: false,
        rewritten_globals: HashSet::new(),
        scanning: std::cell::Cell::new(false),
        dead_chains: std::cell::RefCell::default(),
        imported_unreadable_fixture: false,
    };
    module.redeclared = module.redeclarations();
    module.evaluates = super::direct_eval(parsed);
    module.rewritten_globals = module.global_writes();
    module.collect_origins(modules, depth, imports);
    module.alias_phase = AliasPhase::Settled;
    let found = module.registrations();
    let mut exports = module.exports(modules, depth, &found, imports);
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
    let calls: HashMap<NodeId, SiteKind> = found
        .iter()
        .map(|(node, _, kind, ..)| (*node, *kind))
        .collect();
    let escapes = module.escapes(
        &calls,
        matches!(exports.get("default"), Some(Export::Wrapper(_))),
    );
    let sites = found
        .into_iter()
        .map(|(node, span, kind, chain, provenance)| {
            let callee = module.callee(node);
            Site {
                node,
                span,
                kind,
                chain,
                member: module.registration_member(callee),
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
    let unreadable_fixture_exports = module.unreadable_fixture_exports();
    let direct_unreadable_fixture = module.unreadable_fixture();
    let unreadable_fixture = direct_unreadable_fixture || module.imported_unreadable_fixture;
    let fixture_export_names = module.fixture_export_names(&unreadable_fixture_exports);
    let inventory = Inventory {
        imports_limited: modules.work.refusal.is_some(),
        import_limit: modules.work.refusal.clone(),
        direct_eval: module.evaluates,
        non_hoisted_mock: module.non_hoisted_mock(),
        in_place_mocks: module.in_place_mocks(),
        unreadable_mock_factory: module.unreadable_mock_factory(),
        unreadable_fixture,
        setup_access: super::setup_access(parsed, setting),
        sites,
        loads: module.loads,
        escapes,
        registration_values: module.values,
    };
    if direct_unreadable_fixture {
        for name in fixture_export_names {
            if exports.contains_key(&name) {
                exports.insert(
                    unreadable_fixture_export(&name),
                    Export::Wrapper(Reach::Tasks),
                );
            }
        }
    }
    (inventory, exports)
}

/// A registering call: node, span, kind, chain and provenance.
type Found = (NodeId, Span, SiteKind, Vec<String>, Provenance);

impl<'s> Module<'s> {
    /// Whether this module reaches Vitest's mock APIs through a stable API binding in a way the
    /// file-wide model cannot answer: a `doMock`/`doUnmock` anywhere, a `mock`/`unmock` Vitest
    /// does not hoist outside a top-level statement, or one it hoists from anywhere but the top
    /// level or from inside another hoisted call, which makes Vitest refuse the file at load.
    ///
    /// The first two affect imports when they run, and the next core does not model that
    /// dynamic-import state; the last has no runtime context at all. The corpus refuses the file
    /// rather than comparing tests under an invented shared context.
    fn non_hoisted_mock(&self) -> bool {
        self.tree.nodes().any(|call| match self.mock_call(call) {
            Some(MockCall::Dynamic) => !self.unreachable(call),
            Some(MockCall::InPlace) => {
                !self.unreachable(call) && self.top_level_statement(call).is_none()
            }
            // Vitest refuses the file whenever it hoists a call from anywhere but the top level,
            // reachable or not, or from inside another call it hoists.
            Some(MockCall::Hoisted) => {
                self.inside_hoisted_call(call)
                    || (self.top_level_statement(call).is_none() && !self.in_source_statement(call))
            }
            // A helper Vitest hoists (`vi.hoisted`) refuses the file the same way when nested.
            None => self.hoisted_by_spelling(call) && self.inside_hoisted_call(call),
        })
    }

    /// The reachable top-level statements calling a `mock`/`unmock` Vitest does not hoist.
    fn in_place_mocks(&self) -> Vec<InPlaceMock> {
        let Some(namespace) = self.setting.dialect.mocking_namespace else {
            return Vec::new();
        };
        self.tree
            .nodes()
            .filter(|&call| {
                self.mock_call(call) == Some(MockCall::InPlace) && !self.unreachable(call)
            })
            .filter_map(|call| {
                let statement = self.top_level_statement(call)?;
                let (_, member) = self.member_call(call)?;
                Some(InPlaceMock {
                    statement,
                    call,
                    callee: format!("{namespace}.{member}"),
                })
            })
            .collect()
    }

    /// How a call reaching the framework's mocking namespace through a stable binding changes
    /// module state, if it is such a call.
    ///
    /// Vitest hoists by spelling, not by binding: a hoisted member is hoisted only as a plain,
    /// non-optional member of an identifier the dialect names (`vi.mock`, `vitest.unmock`).
    fn mock_call(&self, call: NodeId) -> Option<MockCall> {
        let (object, member) = self.member_call(call)?;
        let mocking = self.setting.dialect.mocking_member(member)?;
        if !self.mocking_namespace(object, &mut HashSet::new()) {
            return None;
        }
        Some(match mocking {
            Mocking::Dynamic => MockCall::Dynamic,
            // The hoist visitor still reaches an optional call when anything else in the file
            // triggers the transform, so where it runs is not known.
            Mocking::Hoisted if self.optional_call(call) => MockCall::Dynamic,
            Mocking::Hoisted if self.hoisting_spelling(call, object) => MockCall::Hoisted,
            // Only the hoisting transform rewrites an `import()` target to its path; in place,
            // the call throws when the module loads.
            Mocking::Hoisted if self.imported_mock_target(call) => MockCall::Dynamic,
            Mocking::Hoisted => MockCall::InPlace,
        })
    }

    /// Whether the call's first argument is an `import()` or an awaited one.
    fn imported_mock_target(&self, call: NodeId) -> bool {
        let tree = self.tree;
        tree.list(call, "arguments")
            .into_iter()
            .flatten()
            .next()
            .map(|target| tree.inner(target))
            .map(|target| {
                if tree.kind(target) == "AwaitExpression" {
                    tree.child(target, "argument")
                        .map_or(target, |argument| tree.inner(argument))
                } else {
                    target
                }
            })
            .is_some_and(|target| tree.kind(target) == "ImportExpression")
    }

    /// Whether `call` or its callee member is optional (`vi?.mock()`, `vi.mock?.()`).
    fn optional_call(&self, call: NodeId) -> bool {
        let tree = self.tree;
        tree.flag(call, "optional")
            || tree
                .child(call, "callee")
                .is_some_and(|callee| tree.flag(tree.inner(callee), "optional"))
    }

    /// Whether a member call is written the way the framework hoists it: a non-computed member
    /// of an identifier the dialect spells for hoisting.
    fn hoisting_spelling(&self, call: NodeId, object: NodeId) -> bool {
        let tree = self.tree;
        let object = tree.inner(object);
        tree.child(call, "callee")
            .map(|callee| tree.inner(callee))
            .is_some_and(|callee| !tree.flag(callee, "computed"))
            && tree.kind(object) == "Identifier"
            && tree
                .text(object, "name")
                .is_some_and(|name| self.setting.dialect.hoists(name))
    }

    /// Whether `node` lies inside a call the framework hoists by its spelling alone
    /// (`vi.mock`, `vitest.unmock`, `vi.hoisted`), whatever that call's binding.
    fn inside_hoisted_call(&self, node: NodeId) -> bool {
        self.tree
            .ancestors(node)
            .any(|outer| self.hoisted_by_spelling(outer))
    }

    /// Whether the framework hoists `call` by its spelling alone, whatever its binding.
    fn hoisted_by_spelling(&self, call: NodeId) -> bool {
        self.member_call(call).is_some_and(|(object, member)| {
            self.setting.dialect.hoisted_member(member) && self.hoisting_spelling(call, object)
        })
    }

    /// The module-top-level expression statement whose expression is `call`, if there is one.
    fn top_level_statement(&self, call: NodeId) -> Option<NodeId> {
        let tree = self.tree;
        let mut parent = tree.outer_parent(call)?;
        if tree.kind(parent) == "ChainExpression" {
            parent = tree.outer_parent(parent)?;
        }
        (tree.kind(parent) == "ExpressionStatement" && tree.parent(parent) == Some(tree.root()))
            .then_some(parent)
    }

    /// Whether `call` is a statement directly inside the block of an in-source guard,
    /// `if (import.meta.<in-source member>) { ... }`, which the framework hoists from as it does
    /// from the module's top level.
    fn in_source_statement(&self, call: NodeId) -> bool {
        let tree = self.tree;
        let Some(statement) = tree
            .outer_parent(call)
            .filter(|&parent| tree.kind(parent) == "ExpressionStatement")
        else {
            return false;
        };
        let Some(block) = tree
            .parent(statement)
            .filter(|&block| tree.kind(block) == "BlockStatement")
        else {
            return false;
        };
        let Some(guard) = tree.parent(block).filter(|&guard| {
            tree.kind(guard) == "IfStatement" && tree.child(guard, "consequent") == Some(block)
        }) else {
            return false;
        };
        let Some(member) = self.setting.dialect.in_source_member else {
            return false;
        };
        tree.child(guard, "test")
            .map(|test| tree.inner(test))
            .is_some_and(|test| {
                tree.kind(test) == "MemberExpression"
                    && !tree.flag(test, "computed")
                    && tree
                        .child(test, "property")
                        .and_then(|property| tree.text(property, "name"))
                        == Some(member)
                    && tree
                        .child(test, "object")
                        .map(|object| tree.inner(object))
                        .is_some_and(|object| {
                            tree.kind(object) == "MetaProperty"
                                && tree
                                    .child(object, "meta")
                                    .and_then(|meta| tree.text(meta, "name"))
                                    == Some("import")
                                && tree
                                    .child(object, "property")
                                    .and_then(|property| tree.text(property, "name"))
                                    == Some("meta")
                        })
            })
    }

    /// Whether a `vi.mock`, hoisted or in place, has a factory whose behavior is not written
    /// inline.
    fn unreadable_mock_factory(&self) -> bool {
        self.tree.nodes().any(|call| {
            if !matches!(
                self.mock_call(call),
                Some(MockCall::Hoisted | MockCall::InPlace)
            ) {
                return false;
            }
            let arguments: Vec<NodeId> = self
                .tree
                .list(call, "arguments")
                .into_iter()
                .flatten()
                .collect();
            arguments.len() > 1
                && !self.inline_factory(arguments[1])
                && !self.spy_options(arguments[1])
        })
    }

    /// An inline factory is the only factory value the next core can preserve without evaluating
    /// a binding.
    fn inline_factory(&self, node: NodeId) -> bool {
        matches!(
            self.tree.kind(self.tree.inner(node)),
            "ArrowFunctionExpression" | "FunctionExpression"
        )
    }

    /// The one `vi.mock` option the legacy core distinguishes from both a factory and automock.
    fn spy_options(&self, node: NodeId) -> bool {
        let node = self.tree.inner(node);
        if self.tree.kind(node) != "ObjectExpression" {
            return false;
        }
        let properties: Vec<_> = self
            .tree
            .list(node, "properties")
            .into_iter()
            .flatten()
            .collect();
        matches!(properties.as_slice(), [property]
        if self.tree.kind(*property) == "Property"
            && !self.tree.flag(*property, "computed")
            && self.tree.text(*property, "kind") == Some("init")
            && property_name(self.tree, *property) == Some("spy")
            && self.tree.child(*property, "value")
                .is_some_and(|value| {
                    self.tree.text(self.tree.inner(value), "raw") == Some("true")
                }))
    }

    /// Whether a `test.extend` call supplies a fixture value the next core cannot model.
    fn unreadable_fixture(&self) -> bool {
        self.tree
            .nodes()
            .any(|call| self.unreadable_fixture_call(call))
    }

    /// The directly exported builders whose fixture values are unreadable.
    fn unreadable_fixture_exports(&self) -> HashSet<SymbolId> {
        self.tree
            .nodes()
            .filter(|&call| self.unreadable_fixture_call(call))
            .filter_map(|call| {
                let declarator = self.tree.parent(call)?;
                (self.tree.kind(declarator) == "VariableDeclarator")
                    .then(|| self.tree.child(declarator, "id"))
                    .flatten()
                    .filter(|&id| self.tree.kind(id) == "Identifier")
                    .and_then(|id| self.declares(id))
            })
            .collect()
    }

    /// Whether one reachable `test.extend` call supplies a fixture value the next core cannot
    /// model.
    fn unreadable_fixture_call(&self, call: NodeId) -> bool {
        let Some((object, member)) = self.member_call(call) else {
            return false;
        };
        if self.setting.dialect.chain_member(member) != Some(Chain::FixtureExtension)
            || !matches!(
                self.value_origin(object),
                Some(Origin::Api(Registers::Test) | Origin::Derived(Registers::Test))
            )
        {
            return false;
        }
        if self.unreachable(call) {
            return false;
        }
        let arguments: Vec<NodeId> = self
            .tree
            .list(call, "arguments")
            .into_iter()
            .flatten()
            .collect();
        if self.readable_fixture_builder(&arguments) {
            return false;
        }
        let Some(fixtures) = arguments.first().copied() else {
            return true;
        };
        let fixtures = self.tree.inner(fixtures);
        if self.tree.kind(fixtures) != "ObjectExpression" {
            return true;
        }
        self.tree
            .list(fixtures, "properties")
            .into_iter()
            .flatten()
            .any(|property| {
                self.tree.kind(property) != "Property"
                    || self.tree.flag(property, "computed")
                    || self
                        .tree
                        .child(property, "value")
                        .is_none_or(|value| !self.fixture_value(value))
            })
    }

    /// The object and static member name of a call through a member expression.
    fn member_call(&self, call: NodeId) -> Option<(NodeId, &str)> {
        (self.tree.kind(call) == "CallExpression")
            .then(|| self.tree.child(call, "callee"))
            .flatten()
            .map(|callee| self.tree.inner(callee))
            .filter(|&callee| self.tree.kind(callee) == "MemberExpression")
            .and_then(|callee| {
                Some((
                    self.tree.child(callee, "object")?,
                    member_name(self.tree, callee)?,
                ))
            })
    }

    /// Whether builder arguments need no evaluation beyond the value forms this core retains.
    fn readable_fixture_builder(&self, arguments: &[NodeId]) -> bool {
        match arguments {
            [name, value] => {
                self.tree.kind(self.tree.inner(*name)) == "Literal" && self.fixture_value(*value)
            }
            [name, options, value] => {
                self.tree.kind(self.tree.inner(*name)) == "Literal"
                    && self.tree.kind(self.tree.inner(*options)) == "ObjectExpression"
                    && self.fixture_value(*options)
                    && self.fixture_value(*value)
            }
            _ => false,
        }
    }

    /// The public names that export a local builder with an unreadable fixture.
    fn fixture_export_names(&self, symbols: &HashSet<SymbolId>) -> HashSet<String> {
        let tree = self.tree;
        let mut names = HashSet::new();
        let statements = tree.list(tree.root(), "body");
        for statement in statements.into_iter().flatten() {
            match tree.kind(statement) {
                "ExportNamedDeclaration" if tree.child(statement, "source").is_none() => {
                    if let Some(declaration) = tree.child(statement, "declaration") {
                        for declarator in
                            tree.list(declaration, "declarations").into_iter().flatten()
                        {
                            if let Some(id) = tree.child(declarator, "id") {
                                if self
                                    .declares(id)
                                    .is_some_and(|symbol| symbols.contains(&symbol))
                                {
                                    if let Some(name) = tree.text(id, "name") {
                                        names.insert(name.to_owned());
                                    }
                                }
                            }
                        }
                    }
                    for (local, exported) in specifiers(tree, statement) {
                        if self
                            .read(local)
                            .is_some_and(|symbol| symbols.contains(&symbol))
                        {
                            names.insert(export_name(tree, exported));
                        }
                    }
                }
                "ExportDefaultDeclaration" => {
                    if let Some(declaration) = tree.child(statement, "declaration") {
                        let declaration = tree.inner(declaration);
                        if self
                            .read(declaration)
                            .is_some_and(|symbol| symbols.contains(&symbol))
                        {
                            names.insert("default".to_owned());
                        }
                    }
                }
                _ => {}
            }
        }
        names
    }

    /// Fixture literals, setup functions and recursively literal containers are stable values;
    /// every other expression requires evaluation or a normal-form representation this core does
    /// not yet retain.
    fn fixture_value(&self, node: NodeId) -> bool {
        let node = self.tree.inner(node);
        match self.tree.kind(node) {
            "ArrowFunctionExpression" | "FunctionExpression" | "Literal" => true,
            "UnaryExpression" => {
                matches!(self.tree.text(node, "operator"), Some("+") | Some("-"))
                    && self.tree.child(node, "argument").is_some_and(|argument| {
                        self.tree.kind(self.tree.inner(argument)) == "Literal"
                    })
            }
            "TemplateLiteral" => self.tree.list(node, "expressions").is_empty(),
            "ArrayExpression" => self
                .tree
                .list(node, "elements")
                .into_iter()
                .flatten()
                .all(|element| self.fixture_value(element)),
            "ObjectExpression" => self
                .tree
                .list(node, "properties")
                .into_iter()
                .flatten()
                .all(|property| {
                    self.tree.kind(property) == "Property"
                        && !self.tree.flag(property, "computed")
                        && self
                            .tree
                            .child(property, "value")
                            .is_some_and(|value| self.fixture_value(value))
                }),
            _ => false,
        }
    }

    /// Whether `node` is a stable binding of the framework's mocking namespace.
    fn mocking_namespace(&self, node: NodeId, seen: &mut HashSet<SymbolId>) -> bool {
        let tree = self.tree;
        let node = tree.inner(node);
        if tree.kind(node) == "MemberExpression"
            && member_name(tree, node).is_some_and(|name| {
                self.setting.dialect.namespace(name) == self.setting.dialect.mocking_namespace
            })
        {
            return tree
                .child(node, "object")
                .is_some_and(|object| self.vitest_module_namespace(object));
        }
        if tree.kind(node) != "Identifier" {
            return false;
        }
        let Some(symbol) = self.read(node) else {
            return self.setting.globals
                && tree.text(node, "name").is_some_and(|name| {
                    self.setting.dialect.namespace(name) == self.setting.dialect.mocking_namespace
                        && !self.global_was_rewritten(name)
                });
        };
        if self.reassigned(symbol) || !seen.insert(symbol) {
            return false;
        }
        match &self.parsed.scopes.symbol(symbol).declaration {
            Declaration::Import {
                import,
                specifier,
                imported: Imported::Named(name),
            } => {
                self.runtime_api_import(*import, *specifier)
                    && self.setting.dialect.namespace(name)
                        == self.setting.dialect.mocking_namespace
            }
            Declaration::Variable(declarator)
                if tree
                    .outer_parent(*declarator)
                    .and_then(|declaration| tree.text(declaration, "kind"))
                    == Some("const")
                    && tree
                        .child(*declarator, "id")
                        .is_some_and(|id| tree.kind(id) == "Identifier") =>
            {
                tree.child(*declarator, "init")
                    .is_some_and(|init| self.mocking_namespace(init, seen))
            }
            _ => false,
        }
    }

    /// Whether `node` is a namespace import from the framework module.
    fn vitest_module_namespace(&self, node: NodeId) -> bool {
        let tree = self.tree;
        let node = tree.inner(node);
        if tree.kind(node) != "Identifier" {
            return false;
        }
        let Some(symbol) = self.read(node) else {
            return false;
        };
        !self.reassigned(symbol)
            && matches!(
                &self.parsed.scopes.symbol(symbol).declaration,
                Declaration::Import {
                    import,
                    specifier,
                    imported: Imported::Namespace,
                } if self.runtime_api_import(*import, *specifier)
            )
    }

    /// Whether an import specifier is a runtime import from the configured framework module.
    fn runtime_api_import(&self, import: NodeId, specifier: NodeId) -> bool {
        self.tree.text(import, "importKind") != Some("type")
            && self.tree.text(specifier, "importKind") != Some("type")
            && self
                .tree
                .child(import, "source")
                .and_then(|source| self.tree.text(source, "value"))
                .is_some_and(|source| self.setting.dialect.is_api_module(source))
    }

    /// Whether a write through the global object may have replaced this free global.
    fn global_was_rewritten(&self, name: &str) -> bool {
        self.rewritten_globals.contains(&None)
            || self.rewritten_globals.contains(&Some(name.to_owned()))
    }

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
                        Origin::Derived(_) => Provenance::Derived,
                        _ => Provenance::Direct,
                    };
                    (*origin, provenance)
                }),
                Name::Reference(None) if self.setting.globals => {
                    let name = tree.text(node, "name")?;
                    let registration = self.setting.dialect.registration(name)?;
                    let rewritten = self.global_was_rewritten(name);
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
    fn chain(&self, node: NodeId) -> Option<(Origin, Provenance, Vec<Step<'s>>)> {
        if !self.scanning.get() {
            return self.walk(node, &mut Vec::new());
        }
        let mut visited = Vec::new();
        let found = self.walk(node, &mut visited);
        if found.is_none() {
            self.dead_chains.borrow_mut().extend(visited);
        }
        found
    }

    /// [`Self::chain`]'s walk, recording each node it passes in `visited`; a node already known to
    /// reach no root ends it.
    fn walk(
        &self,
        mut node: NodeId,
        visited: &mut Vec<NodeId>,
    ) -> Option<(Origin, Provenance, Vec<Step<'s>>)> {
        let tree = self.tree;
        let mut steps = Vec::new();
        loop {
            node = tree.inner(node);
            if self.dead_chains.borrow().contains(&node) {
                return None;
            }
            if let Some(mut result) = tree.sequence_result(node) {
                // A failed bare sequence rules out a root. A longer chain can fail even
                // when calling the sequence itself registers.
                if steps.is_empty() {
                    visited.push(node);
                }
                result = tree.inner(result);
                while let Some(next) = tree.sequence_result(result) {
                    result = tree.inner(next);
                }
                let selected = self.resolve(result)?;
                let reach = match selected.value {
                    Origin::Wrapper(reach) => reach,
                    _ if steps.is_empty()
                        && self.invokes_sequence_fixture(node, result, &selected) =>
                    {
                        Reach::Tasks
                    }
                    _ => return None,
                };
                steps.reverse();
                return Some((Origin::Wrapper(reach), Provenance::Wrapper, steps));
            }
            visited.push(node);
            if let Some((origin, provenance)) = self.root_origin(node) {
                steps.reverse();
                return Some((origin, provenance, steps));
            }
            match tree.kind(node) {
                // Join resolved result capabilities; missing proof does not make an arm inert.
                "ConditionalExpression" => {
                    let arms = [
                        tree.child(node, "consequent")?,
                        tree.child(node, "alternate")?,
                    ];
                    let resolved = arms.map(|arm| self.resolve(arm));
                    let origins = resolved
                        .each_ref()
                        .map(|arm| arm.as_ref().map(|callee| callee.value));
                    let registers = match (origins[0], origins[1]) {
                        (Some(Origin::Api(left)), Some(Origin::Api(right))) if left == right => {
                            left
                        }
                        _ if arms.into_iter().zip(resolved.each_ref()).any(
                            |(arm, resolved)| self.registration_candidate(arm, resolved.as_ref()),
                        ) =>
                        {
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
                "LogicalExpression" => {
                    if !logical_results(tree, node).any(|operand| {
                        self.registration_candidate(operand, self.resolve(operand).as_ref())
                    }) {
                        return None;
                    }
                    steps.reverse();
                    return Some((Origin::Wrapper(Reach::Tasks), Provenance::Wrapper, steps));
                }
                // What awaiting a registration function resolves to is not read: it may register.
                "AwaitExpression" if self.holds_api(tree.child(node, "argument")?) => {
                    steps.reverse();
                    return Some((Origin::Wrapper(Reach::Tasks), Provenance::Wrapper, steps));
                }
                _ => {
                    let link = callee_link(tree, node)?;
                    match link.kind {
                        CalleeLinkKind::Member(name) => steps.push(Step::Member(name?)),
                        // What a call handed a registration function returns may register.
                        CalleeLinkKind::Call
                            if tree.list(link.node, "arguments").into_iter().flatten().any(
                                |argument| {
                                    self.value_origin(argument)
                                        .is_some_and(|origin| origin != Origin::Namespace)
                                },
                            ) =>
                        {
                            steps.reverse();
                            return Some((
                                Origin::Wrapper(Reach::Tasks),
                                Provenance::Wrapper,
                                steps,
                            ));
                        }
                        CalleeLinkKind::Call | CalleeLinkKind::Tagged => steps.push(Step::Call),
                    }
                    node = link.input;
                }
            }
        }
    }

    /// An immediate call of a sequence-selected fixture extension may register tasks.
    fn invokes_sequence_fixture(
        &self,
        sequence: NodeId,
        result: NodeId,
        selected: &ResolvedCallee<'s>,
    ) -> bool {
        let tree = self.tree;
        let api_steps = selected.steps.get(usize::from(selected.namespace_root)..);
        selected.registration == Some(SiteKind::Registers(Registers::Test))
            && matches!(
                api_steps,
                Some([Step::Member(member), Step::Call])
                    if self.setting.dialect.chain_member(member) == Some(Chain::FixtureExtension)
            )
            && tree
                .outer_parent(sequence)
                .and_then(|parent| callee_link(tree, parent))
                .is_some_and(|link| {
                    link.kind == CalleeLinkKind::Call && tree.inner(link.input) == sequence
                })
            && self.holds_api_with(result, Some(selected))
    }

    /// A result candidate may register; a coarse API origin alone does not prove callability.
    /// Settle exact origins before admitting mixed results from proved bare aliases.
    fn registration_candidate(&self, node: NodeId, resolved: Option<&ResolvedCallee<'s>>) -> bool {
        match resolved.map(|callee| callee.value) {
            Some(Origin::Wrapper(_) | Origin::Derived(_)) => true,
            Some(Origin::Api(_)) if self.alias_phase != AliasPhase::Exact => {
                matches!(
                    self.proved_registration_binding(node),
                    Some(RegistrationBinding::Alias(Some(_)))
                ) || (self.alias_phase == AliasPhase::Settled
                    && self.holds_api_with(node, resolved))
            }
            _ => false,
        }
    }

    /// The proof that a bare identifier reads a callable registration, after initialization.
    fn proved_registration_binding(&self, node: NodeId) -> Option<RegistrationBinding> {
        let node = self.tree.inner(node);
        if self.tree.kind(node) != "Identifier" {
            return None;
        }
        let binding = *self.registration_bindings.get(&self.read(node)?)?;
        match binding {
            RegistrationBinding::FrameworkImport => Some(binding),
            RegistrationBinding::Alias(Some(declarator)) => {
                crate::lang::proof::initialized(self.tree, declarator, node)?;
                Some(binding)
            }
            RegistrationBinding::Alias(None) => None,
        }
    }

    /// Both result arms read the same proved callable, independent of the guard's value.
    fn conditional_registration_binding(&self, node: NodeId) -> Option<SymbolId> {
        let tree = self.tree;
        if tree.kind(node) != "ConditionalExpression" {
            return None;
        }
        let consequent = tree.inner(tree.child(node, "consequent")?);
        let alternate = tree.inner(tree.child(node, "alternate")?);
        self.proved_registration_binding(consequent)?;
        self.proved_registration_binding(alternate)?;
        let symbol = self.read(consequent)?;
        (self.read(alternate) == Some(symbol)).then_some(symbol)
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

    /// What applying `steps` to a root of `origin` reads as ([`Applied`]); `None` when a step
    /// leaves the API (a call of a registration function, a namespace member that is not one).
    fn apply(&self, origin: Origin, steps: &[Step]) -> Option<Applied> {
        let mut registers = match origin {
            Origin::Api(registers) => Some(registers),
            _ => None,
        };
        let mut pending = false;
        // A table's call returns a plain function, which carries no registration members.
        let mut tabled = false;
        let mut start = 0;
        for (index, step) in steps.iter().enumerate() {
            match step {
                Step::Member(name) if registers.is_none() => registers = Some(self.api(name)?),
                Step::Member(name) if !pending && !tabled && registers == Some(Registers::Test) => {
                    if let Some(member) = self.setting.dialect.test_member(name) {
                        registers = Some(member);
                        start = index + 1;
                    } else {
                        tabled = matches!(
                            self.setting.dialect.chain_member(name),
                            Some(Chain::Table(_))
                        );
                        pending = self.builds(name);
                    }
                }
                Step::Member(name) => pending = self.builds(name),
                Step::Call if pending => pending = false,
                Step::Call => return None,
            }
        }
        Some(Applied {
            registers,
            pending,
            start,
        })
    }

    /// Resolve a callee's value and admitted registration from the same application and anchor.
    fn resolve(&self, node: NodeId) -> Option<ResolvedCallee<'s>> {
        #[cfg(test)]
        RESOLUTIONS.with(|resolutions| resolutions.set(resolutions.get() + 1));
        let (origin, provenance, steps) = self.chain(node)?;
        if let Some(reach) = origin.reach() {
            // Only a bare wrapper retains its reach; anything derived from it may register tasks.
            let bare = steps.is_empty();
            let reach = if bare { reach } else { Reach::Tasks };
            let derived = matches!(origin, Origin::Derived(_));
            return Some(ResolvedCallee {
                value: if derived && bare {
                    origin
                } else {
                    Origin::Wrapper(reach)
                },
                registration: Some(SiteKind::Wrapper(reach)),
                provenance: if derived && !bare {
                    Provenance::Wrapper
                } else {
                    provenance
                },
                steps,
                namespace_root: false,
                start: 0,
                member: None,
            });
        }
        let Applied {
            registers,
            pending,
            start,
        } = self.apply(origin, &steps)?;
        let member = (start > 0)
            .then(|| self.member_at(node, steps.len() - start))
            .flatten();
        let evaluated = start > 0
            && !member
                .and_then(|member| self.tree.child(member, "object"))
                .is_some_and(|object| self.reads_only(object));
        let value = if evaluated {
            Origin::Wrapper(Reach::Tasks)
        } else {
            registers.map_or(Origin::Namespace, Origin::Api)
        };
        let registration = if pending
            || steps
                .last()
                .is_some_and(|step| matches!(step, Step::Member(name) if self.inert(name)))
        {
            None
        } else {
            registers.map(|registers| {
                if evaluated {
                    SiteKind::Wrapper(Reach::Tasks)
                } else {
                    SiteKind::Registers(registers)
                }
            })
        };
        Some(ResolvedCallee {
            value,
            registration,
            provenance: if evaluated {
                Provenance::Wrapper
            } else {
                provenance
            },
            steps,
            namespace_root: origin == Origin::Namespace,
            start: if evaluated { 0 } else { start },
            member: if evaluated { None } else { member },
        })
    }

    /// The expression `outer` member or call steps into the callee chain at `node`.
    fn member_at(&self, node: NodeId, outer: usize) -> Option<NodeId> {
        let tree = self.tree;
        let mut node = tree.inner(node);
        for _ in 0..outer {
            node = tree.inner(callee_link(tree, node)?.input);
        }
        Some(node)
    }

    /// Whether evaluating `node` runs no code of the project: a name, or non-computed member reads
    /// of one (`test.skip`).
    fn reads_only(&self, node: NodeId) -> bool {
        let tree = self.tree;
        let node = tree.inner(node);
        match tree.kind(node) {
            "Identifier" => true,
            "MemberExpression" if !tree.flag(node, "computed") => tree
                .child(node, "object")
                .is_some_and(|object| self.reads_only(object)),
            _ => false,
        }
    }

    /// The registration function an expression's value is (`test`, `test.skip`,
    /// `test.extend({...})`, `v.describe`), or the namespace.
    fn value_origin(&self, node: NodeId) -> Option<Origin> {
        self.resolve(node).map(|callee| callee.value)
    }

    /// The member expression of a test function `callee` registers through (`test.skip.describe`
    /// for `test.skip.describe.each(rows)`), if it registers through one.
    fn registration_member(&self, callee: NodeId) -> Option<NodeId> {
        self.resolve(callee)?.member
    }

    /// What this call registers, reading the namespace proof or resolving its callee.
    fn registration(&self, call: NodeId) -> Option<(SiteKind, Vec<String>, Provenance)> {
        if self.namespace_task_calls.contains(&call) {
            return Some((
                SiteKind::Wrapper(Reach::Tasks),
                Vec::new(),
                Provenance::Wrapper,
            ));
        }
        let resolved = self.resolve(callee_link(self.tree, call)?.input)?;
        let kind = resolved.registration?;
        let chain = resolved.steps[resolved.start..]
            .iter()
            .map(|step| match step {
                Step::Member(name) => (*name).to_owned(),
                Step::Call => "()".to_owned(),
            })
            .collect();
        Some((kind, chain, resolved.provenance))
    }

    /// Records which imported bindings are the API, its namespace, or a wrapper another module
    /// exports, then the aliases derived from them.
    fn collect_origins(
        &mut self,
        modules: &mut Modules,
        depth: usize,
        imports: &mut ProjectImports,
    ) {
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
                modules.project_exports(self.path, source, self.setting, depth + 1, imports)
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
                if tree.text(specifier, "importKind") == Some("type") {
                    continue;
                }
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
                    "ImportNamespaceSpecifier" => {
                        if api {
                            self.origins.insert(symbol, Origin::Namespace);
                        } else if let Some(exports) = &wrapper_exports {
                            self.collect_namespace_task_calls(symbol, exports);
                        }
                        continue;
                    }
                    _ => continue,
                };
                self.imported_unreadable_fixture |= wrapper_exports
                    .as_ref()
                    .is_some_and(|exports| exports.contains_key(&unreadable_fixture_export(&name)));
                match &wrapper_exports {
                    None => {
                        if let Some(registers) = self.api(&name) {
                            self.origins.insert(symbol, Origin::Api(registers));
                            if tree.kind(specifier) == "ImportSpecifier" && !self.reassigned(symbol)
                            {
                                self.registration_bindings
                                    .insert(symbol, RegistrationBinding::FrameworkImport);
                            }
                        }
                    }
                    Some(exports) => match exports.get(&name) {
                        Some(Export::Namespace) => {
                            self.origins.insert(symbol, Origin::Namespace);
                        }
                        Some(&Export::Api { registers, .. }) => {
                            self.origins.insert(symbol, Origin::Api(registers));
                        }
                        Some(&Export::Wrapper(reach)) => {
                            self.origins.insert(symbol, Origin::Wrapper(reach));
                        }
                        Some(&Export::Derived(registers)) => {
                            self.origins.insert(symbol, Origin::Derived(registers));
                        }
                        None => {}
                    },
                }
            }
        }
        self.collect_aliases();
        if self
            .registration_bindings
            .values()
            .any(|binding| matches!(binding, RegistrationBinding::Alias(Some(_))))
        {
            self.alias_phase = AliasPhase::Proved;
            self.collect_aliases();
        }
    }

    /// Match direct invocations against the same export summaries named imports use. A read of a
    /// member as a value supplies no proof about what a later call of that value would do.
    fn collect_namespace_task_calls(
        &mut self,
        symbol: SymbolId,
        exports: &HashMap<String, Export>,
    ) {
        if self.reassigned(symbol) {
            return;
        }
        let tree = self.tree;
        let binding = self.parsed.scopes.symbol(symbol);
        self.namespace_task_calls
            .extend(binding.references.iter().filter_map(|reference| {
                let read = reference.node?;
                let member = callee_link(tree, tree.outer_parent(read)?)?;
                let CalleeLinkKind::Member(Some(name)) = member.kind else {
                    return None;
                };
                if tree.inner(member.input) != read
                    || tree.flag(member.node, "optional")
                    || name.starts_with('\0')
                    || exports.get(name) != Some(&Export::Wrapper(Reach::Tasks))
                {
                    return None;
                }
                let call = callee_link(tree, tree.outer_parent(member.node)?)?;
                (call.kind == CalleeLinkKind::Call
                    && tree.inner(call.input) == member.node
                    && !tree.flag(call.node, "optional"))
                .then_some(call.node)
            }));
    }

    /// `const t = test`, `const myTest = test.extend({...})`, `const { test } = import.meta.vitest`.
    fn collect_aliases(&mut self) {
        let tree = self.tree;
        loop {
            let before = self.origins.clone();
            let mut learned_callable_alias = false;
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
                            let origin = self.alias(symbol, origin);
                            let callable = matches!(origin, Origin::Api(_))
                                && self.parsed.scopes.symbol(symbol).declaration
                                    == Declaration::Variable(node)
                                && tree.parent(node).is_some_and(|declaration| {
                                    tree.text(declaration, "kind") == Some("const")
                                        && !tree.flag(declaration, "declare")
                                })
                                && self.proved_registration_binding(init).is_some();
                            let previous = self.registration_bindings.insert(
                                symbol,
                                RegistrationBinding::Alias(callable.then_some(node)),
                            );
                            learned_callable_alias |= callable
                                && !matches!(previous, Some(RegistrationBinding::Alias(Some(_))));
                        }
                    }
                    // A destructured binding holds a member of the wrapper, which may register
                    // tests whatever the wrapper registers when called.
                    "ObjectPattern" | "ArrayPattern"
                        if matches!(origin, Some(Origin::Wrapper(_) | Origin::Derived(_))) =>
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
            if self.origins == before && !learned_callable_alias {
                break;
            }
        }
    }

    /// Records that `symbol` holds `origin`. A wrapper alias takes the widest reach seen, since a
    /// wrapper's reach only grows while the fixpoint runs. Return the incoming origin after
    /// accounting for writes to the binding.
    fn alias(&mut self, symbol: SymbolId, origin: Origin) -> Origin {
        let origin = match origin {
            _ if self.reassigned(symbol) => Origin::Wrapper(Reach::Tasks),
            origin => origin,
        };
        let entry = self.origins.entry(symbol).or_insert(origin);
        if let (Origin::Wrapper(held), Origin::Wrapper(reach)) = (*entry, origin) {
            *entry = Origin::Wrapper(held.max(reach));
        }
        origin
    }

    /// Every registering call, with wrappers found to a fixpoint: a function whose body holds a
    /// registering call registers when called. Each starts as hooks-only when every registration
    /// in it is a hook or a hooks-only wrapper call; then any whose body is not closed may
    /// register tests, which may open the wrappers calling it, until none changes.
    fn registrations(&mut self) -> Vec<Found> {
        loop {
            let found = self.scan();
            let reaches = ReachIndex::new(found.iter().map(|(_, span, kind, ..)| (*span, *kind)));
            let before = self.wrappers.clone();
            // A new returner can make its callers return the API without changing a wrapper.
            let returners = self.returners.len();
            // Vitest calls a suite factory with its `test` API, so a call through a parameter of a
            // suite callback may register tests the inventory cannot attribute.
            for symbol in self.suite_parameters(&found) {
                self.wrappers.insert(symbol, Reach::Tasks);
            }
            for (function, symbols) in self.functions() {
                let Some(reach) = reaches.within(self.tree.span(function)) else {
                    continue;
                };
                for symbol in symbols {
                    let held = self.wrappers.entry(symbol).or_insert(reach);
                    *held = (*held).max(reach);
                }
            }
            // What a call of a function returning a registration value returns may register. A
            // wrapper's result already may, so one keeps the reach of its own call.
            for symbol in self.returns_registration() {
                self.wrappers.entry(symbol).or_insert(Reach::Tasks);
                self.returners.insert(symbol);
            }
            let aliases = self.origins.clone();
            self.collect_aliases();
            if self.wrappers == before
                && self.origins == aliases
                && self.returners.len() == returners
            {
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

    /// The bindings the parameters of a suite's callback declare. A table's callback (`.each`,
    /// `.for`) receives a row, not the API, so its parameters are not among them.
    fn suite_parameters(&self, found: &[Found]) -> Vec<SymbolId> {
        let tree = self.tree;
        found
            .iter()
            .filter(|(_, _, kind, chain, _)| {
                *kind == SiteKind::Registers(Registers::Suite)
                    && !chain.iter().any(|step| {
                        matches!(
                            self.setting.dialect.chain_member(step),
                            Some(Chain::Table(_))
                        )
                    })
            })
            .filter_map(|(node, ..)| {
                let callback = tree.inner(tree.list(*node, "arguments").last().copied().flatten()?);
                tree.is_function(callback).then_some(callback)
            })
            .flat_map(|callback| tree.list(callback, "params").into_iter().flatten())
            .flat_map(|param| bound_symbols(tree, &self.parsed.scopes, param))
            .collect()
    }

    /// Every read of a registration function: of a binding that holds one, and, when free names
    /// are the API, of a free name the dialect registers with.
    fn registration_reads(&self) -> impl Iterator<Item = NodeId> + '_ {
        let tree = self.tree;
        let scopes = &self.parsed.scopes;
        let bound = self
            .origins
            .keys()
            .chain(self.wrappers.keys())
            .flat_map(|&symbol| scopes.symbol(symbol).references.iter())
            .filter_map(|reference| reference.node);
        let free = tree.nodes().filter(move |&node| {
            self.setting.globals
                && tree.kind(node) == "Identifier"
                && scopes.is_global_at(tree.span(node).start)
                && tree
                    .text(node, "name")
                    .is_some_and(|name| self.setting.dialect.registration(name).is_some())
        });
        bound.chain(free)
    }

    /// Whether `node`'s value is seen here to be a registration function or the API's namespace, or
    /// is what a call of a function returning one returns. A registration value must be read from
    /// the framework's own binding (an import, a namespace member, a global) through members the
    /// dialect knows: through a local alias, a conditional or another module's export the value is
    /// not read here, and `test.name` is a string.
    fn holds_api(&self, node: NodeId) -> bool {
        self.holds_api_with(node, None)
    }

    /// Check callability using an existing resolution, or resolve after rejecting non-chain roots.
    fn holds_api_with(&self, node: NodeId, resolved: Option<&ResolvedCallee<'s>>) -> bool {
        let tree = self.tree;
        let mut root = tree.inner(node);
        // Only `await make()` holds the API through an `await`: a chain through one is a wrapper
        // (`walk`), so the scan stops at the next and nested awaits stay linear.
        if tree.kind(root) == "AwaitExpression" {
            let Some(argument) = tree.child(root, "argument") else {
                return false;
            };
            root = tree.inner(argument);
        }
        // A resolved join candidate can use the namespace root already proved by the resolver.
        // Other callers retain the identifier and returner boundary below.
        let namespace = |root| {
            resolved.is_some() && matches!(self.root_origin(root), Some((Origin::Namespace, _)))
        };
        while let Some(link) = callee_link(tree, root) {
            if namespace(root) {
                break;
            }
            root = tree.inner(link.input);
        }
        let symbol = if tree.kind(root) == "Identifier" {
            self.read(root)
        } else if namespace(root) {
            None
        } else {
            return false;
        };
        // One walk: each `await` in the chain asks this again, so walking twice doubles per level.
        let owned = if resolved.is_none() {
            self.resolve(node)
        } else {
            None
        };
        let resolved = resolved.or(owned.as_ref());
        let value = resolved.map(|callee| callee.value);
        if !matches!(value, Some(Origin::Api(_) | Origin::Namespace)) {
            // Only the call itself: `make.name` and `make().name` are not what `make` returns.
            let mut value = tree.inner(node);
            if tree.kind(value) == "AwaitExpression" {
                let Some(argument) = tree.child(value, "argument") else {
                    return false;
                };
                value = tree.inner(argument);
            }
            return tree.kind(value) == "CallExpression"
                && tree.child(value, "callee").map(|callee| tree.inner(callee)) == Some(root)
                && symbol.is_some_and(|symbol| self.returners.contains(&symbol));
        }
        let own = symbol.is_none_or(|symbol| {
            !matches!(
                self.registration_bindings.get(&symbol),
                Some(RegistrationBinding::Alias(_))
            ) && matches!(
                self.origins.get(&symbol),
                Some(Origin::Api(_) | Origin::Namespace)
            )
        });
        let dialect = &self.setting.dialect;
        own && resolved.is_some_and(|callee| {
            callee.steps.iter().all(|step| match step {
                Step::Member(name) => {
                    dialect.registration(name).is_some()
                        || dialect.test_member(name).is_some()
                        || dialect.chain_member(name).is_some()
                }
                Step::Call => true,
            })
        })
    }

    /// The symbols of functions whose returned value may hold a registration value: a read of one,
    /// still a registration value after its member and call steps, within a `return` argument or
    /// an arrow's expression body and no nested function, and reachable. Type positions are erased.
    fn returns_registration(&self) -> Vec<SymbolId> {
        let tree = self.tree;
        let mut symbols = Vec::new();
        for read in self.registration_reads() {
            let mut value = read;
            while let Some(parent) = tree.parent(value) {
                if !matches!(
                    value_role(tree, parent, value),
                    ValueRole::Transparent
                        | ValueRole::TypeAssertion
                        | ValueRole::Instantiation
                        | ValueRole::Callee(_)
                ) {
                    break;
                }
                value = parent;
            }
            let mut inner = value;
            let function = tree.ancestors(value).find_map(|outer| {
                let found = if tree.kind(outer) == "ReturnStatement" {
                    tree.ancestors(outer).find(|&node| tree.is_function(node))
                } else if tree.is_function(outer) {
                    (tree.child(outer, "body") == Some(inner)
                        && tree.kind(inner) != "BlockStatement")
                        .then_some(outer)
                } else {
                    None
                };
                let stop = found.is_some()
                    || tree.is_function(outer)
                    || !self.may_evaluate_to(outer, inner);
                inner = outer;
                stop.then_some(found)
            });
            let Some(Some(function)) = function else {
                continue;
            };
            if self.unreachable(read) || !self.holds_api(value) {
                continue;
            }
            symbols.extend(
                tree.child(function, "id")
                    .and_then(|id| self.declares(id))
                    .into_iter()
                    .chain(self.declarator_symbol(function)),
            );
        }
        symbols
    }

    /// Whether `outer`'s value may be, or hold, its child `inner`'s value. Operators that consume a
    /// value (`typeof`, comparison, a call argument, a template, `+=`) and type positions do not.
    fn may_evaluate_to(&self, outer: NodeId, inner: NodeId) -> bool {
        matches!(
            value_role(self.tree, outer, inner),
            ValueRole::Transparent
                | ValueRole::TypeAssertion
                | ValueRole::Instantiation
                | ValueRole::Result
                | ValueRole::Contained
                | ValueRole::LogicalLeft("||" | "??")
        )
    }

    /// Whether `symbol` may hold anything but what it was declared with: it is declared more than
    /// once, something writes it, or a direct `eval` may. Every reading of a name's value asks this.
    fn reassigned(&self, symbol: SymbolId) -> bool {
        self.evaluates
            || self.redeclared.contains(&symbol)
            || crate::lang::proof::unwritten(&self.parsed.scopes, symbol).is_none()
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
                                let Some(id) = tree
                                    .child(declarator, "id")
                                    .filter(|&id| tree.kind(id) == "Identifier")
                                else {
                                    return false;
                                };
                                tree.child(declarator, "init").is_none_or(|value| {
                                    self.runs_nothing(value)
                                        || (self.hook_call(value)
                                            && self.declares(id).is_some_and(|symbol| {
                                                self.parsed
                                                    .scopes
                                                    .symbol(symbol)
                                                    .references
                                                    .is_empty()
                                            }))
                                })
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
        match self.registration(call) {
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

    /// Every call or tagged template of the module whose callee registers, as it stands now. Chains
    /// found to reach no root are remembered for the scan alone.
    fn scan(&self) -> Vec<Found> {
        self.dead_chains.borrow_mut().clear();
        self.scanning.set(true);
        let mut found = self.scan_calls();
        self.scanning.set(false);
        // Vitest collects a file before any test, row or hook callback runs, and a registration
        // made while one runs is refused or ignored, so nothing that runs only then is a site.
        let calls: HashSet<NodeId> = found
            .iter()
            .filter(|(_, _, kind, ..)| {
                matches!(
                    kind,
                    SiteKind::Registers(Registers::Test | Registers::Bench | Registers::Hook(_))
                )
            })
            .map(|(node, ..)| *node)
            .collect();
        let functions = self.runtime_functions(&calls);
        found.retain(|(node, ..)| !self.runs_at_runtime(*node, &calls, &functions));
        found
    }

    /// Whether `node` runs only while a test, row or hook callback of `calls` runs: it lies inside
    /// such a callback or inside one of `functions`.
    fn runs_at_runtime(
        &self,
        node: NodeId,
        calls: &HashSet<NodeId>,
        functions: &HashSet<NodeId>,
    ) -> bool {
        let tree = self.tree;
        tree.ancestors(node).any(|ancestor| {
            tree.is_function(ancestor)
                && (functions.contains(&ancestor)
                    || tree
                        .outer_parent(ancestor)
                        .is_some_and(|call| calls.contains(&call)))
        })
    }

    /// The local functions that run only at runtime: a function declaration, or a `const`/`let`
    /// binding of a function, that is not exported, never reassigned, and whose every reference
    /// is the callee of a call that itself runs only at runtime (a greatest fixpoint, so a
    /// function calling itself stays in).
    fn runtime_functions(&self, calls: &HashSet<NodeId>) -> HashSet<NodeId> {
        let tree = self.tree;
        let exported = |node: NodeId| {
            tree.outer_parent(node).is_some_and(|parent| {
                matches!(
                    tree.kind(parent),
                    "ExportNamedDeclaration" | "ExportDefaultDeclaration"
                )
            })
        };
        let mut candidates: Vec<(NodeId, SymbolId)> = Vec::new();
        for node in tree.nodes() {
            let (function, id, statement) = match tree.kind(node) {
                "FunctionDeclaration" => (node, tree.child(node, "id"), node),
                "VariableDeclarator" => {
                    let Some(init) = tree.child(node, "init").map(|init| tree.inner(init)) else {
                        continue;
                    };
                    let Some(statement) = tree.outer_parent(node) else {
                        continue;
                    };
                    if !tree.is_function(init)
                        || !matches!(tree.text(statement, "kind"), Some("const" | "let"))
                    {
                        continue;
                    }
                    (init, tree.child(node, "id"), statement)
                }
                _ => continue,
            };
            let Some(symbol) = id
                .filter(|&id| tree.kind(id) == "Identifier")
                .and_then(|id| self.declares(id))
            else {
                continue;
            };
            if !exported(statement) && !self.reassigned(symbol) {
                candidates.push((function, symbol));
            }
        }
        let mut functions: HashSet<NodeId> = candidates.iter().map(|&(node, _)| node).collect();
        // Each call reference stays at runtime while one candidate enclosing it does, unless a
        // runtime callback encloses it. Removing a candidate revisits only what it encloses.
        let mut remaining: Vec<(NodeId, usize)> = Vec::new();
        let mut enclosed: HashMap<NodeId, Vec<usize>> = HashMap::new();
        let mut removed = Vec::new();
        for &(function, symbol) in &candidates {
            let references = &self.parsed.scopes.symbol(symbol).references;
            // An uncalled function keeps its sites: they are what proves what it would register.
            let span = tree.span(function);
            let mut runtime = references
                .iter()
                .any(|reference| !span.contains(reference.span));
            for reference in references {
                let call = reference.node.and_then(|name| {
                    tree.outer_parent(name).filter(|&call| {
                        tree.kind(call) == "CallExpression"
                            && tree.child(call, "callee").map(|callee| tree.inner(callee))
                                == Some(name)
                    })
                });
                let Some(call) = call else {
                    runtime = false;
                    break;
                };
                let mut holders = Vec::new();
                let mut fixed = false;
                for ancestor in tree.ancestors(call).filter(|&node| tree.is_function(node)) {
                    if tree
                        .outer_parent(ancestor)
                        .is_some_and(|call| calls.contains(&call))
                    {
                        fixed = true;
                        break;
                    }
                    if functions.contains(&ancestor) {
                        holders.push(ancestor);
                    }
                }
                if fixed {
                    continue;
                }
                if holders.is_empty() {
                    runtime = false;
                    break;
                }
                for holder in &holders {
                    enclosed.entry(*holder).or_default().push(remaining.len());
                }
                remaining.push((function, holders.len()));
            }
            if !runtime {
                removed.push(function);
            }
        }
        for function in &removed {
            functions.remove(function);
        }
        while let Some(function) = removed.pop() {
            for &index in enclosed.get(&function).into_iter().flatten() {
                let (owner, count) = &mut remaining[index];
                *count -= 1;
                if *count == 0 && functions.remove(owner) {
                    removed.push(*owner);
                }
            }
        }
        functions
    }

    /// [`Self::scan`]'s walk over the calls.
    fn scan_calls(&self) -> Vec<Found> {
        let tree = self.tree;
        tree.nodes()
            .filter_map(|node| {
                if !matches!(
                    tree.kind(node),
                    "CallExpression" | "TaggedTemplateExpression"
                ) {
                    return None;
                }
                let (mut kind, chain, provenance) = self.registration(node)?;
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
        // Every test the suite holds copies its title, so a long one is not read.
        if !crate::lang::limits::text_fits(tree, first) {
            return None;
        }
        match tree.kind(first) {
            "Literal" => tree.text(first, "value").map(str::to_owned),
            "TemplateLiteral" if tree.list(first, "expressions").is_empty() => {
                let quasi = tree.list(first, "quasis").first().copied()??;
                crate::lang::limits::cooked(tree, quasi).map(str::to_owned)
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

    /// Whether a selected sequence value reaches an already recorded wrapper destination.
    /// Selection does not discharge member reads, assignments or awaiting their result.
    fn accounted_sequence_read(
        &self,
        read: NodeId,
        calls: &HashMap<NodeId, SiteKind>,
        default_wrapper: bool,
    ) -> bool {
        let tree = self.tree;
        let mut current = read;
        let mut sequence = false;
        while let Some(parent) = tree.parent(current) {
            if tree.selects_value(parent, current) {
                sequence |= tree.sequence_result(parent).is_some();
                current = parent;
            } else if value_role(tree, parent, current) == ValueRole::Transparent {
                current = parent;
            } else {
                if !sequence {
                    return false;
                }
                return match value_role(tree, parent, current) {
                    ValueRole::Callee(CalleeLinkKind::Call | CalleeLinkKind::Tagged) => {
                        matches!(calls.get(&parent), Some(SiteKind::Wrapper(_)))
                    }
                    ValueRole::Initializer => tree
                        .child(parent, "id")
                        .filter(|&id| tree.kind(id) == "Identifier")
                        .and_then(|id| self.declares(id))
                        .and_then(|symbol| self.origins.get(&symbol))
                        .is_some_and(|origin| matches!(origin, Origin::Wrapper(_))),
                    ValueRole::Exported => default_wrapper,
                    _ => false,
                };
            }
        }
        false
    }

    /// Registration functions read as values rather than called.
    fn escapes(&self, calls: &HashMap<NodeId, SiteKind>, default_wrapper: bool) -> Vec<Escape> {
        let tree = self.tree;
        let mut escapes = Vec::new();
        for read in self.registration_reads() {
            let mut current = read;
            let escaped = loop {
                let Some(parent) = tree.parent(current) else {
                    break true;
                };
                match value_role(tree, parent, current) {
                    ValueRole::Transparent | ValueRole::Callee(CalleeLinkKind::Member(_)) => {
                        current = parent
                    }
                    ValueRole::Callee(CalleeLinkKind::Call | CalleeLinkKind::Tagged)
                    | ValueRole::Initializer
                    | ValueRole::Exported
                    | ValueRole::UnaryOperand => break false,
                    _ => break true,
                }
            };
            if escaped && !self.accounted_sequence_read(read, calls, default_wrapper) {
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
        imports: &mut ProjectImports,
    ) -> HashMap<String, Export> {
        let tree = self.tree;
        let reaches = ReachIndex::new(found.iter().map(|(_, span, kind, ..)| (*span, *kind)));
        let spelled = self.spelled_exports();
        let dialect = self.setting.dialect;
        // The registering-only table cannot prove what an earlier opaque star omits.
        let mut stars = 0;
        let mut star_exports: HashMap<String, Export> = HashMap::new();
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
                    Origin::Api(registers) | Origin::Derived(registers) => {
                        Export::Derived(*registers)
                    }
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
                        if let Some(module) =
                            dialect.api_modules.iter().copied().find(|m| *m == source)
                        {
                            for (local, exported) in specifiers(tree, statement) {
                                if let Some(registration) =
                                    dialect.registration(&export_name(tree, local))
                                {
                                    add(
                                        export_name(tree, exported),
                                        Export::Api {
                                            registers: registration.registers,
                                            module,
                                            name: registration.name,
                                        },
                                    );
                                }
                            }
                            continue;
                        }
                        let Some(theirs) = modules.project_exports(
                            self.path,
                            source,
                            self.setting,
                            depth + 1,
                            imports,
                        ) else {
                            continue;
                        };
                        // Re-exporting any name loads the module, and whatever it registers then.
                        if theirs.contains_key(ON_LOAD) {
                            add(ON_LOAD.to_owned(), Export::Wrapper(Reach::Tasks));
                        }
                        for (local, exported) in specifiers(tree, statement) {
                            let local = export_name(tree, local);
                            let exported = export_name(tree, exported);
                            if let Some(&export) = theirs.get(&local) {
                                add(exported.clone(), export);
                            }
                            if theirs.contains_key(&unreadable_fixture_export(&local)) {
                                add(
                                    unreadable_fixture_export(&exported),
                                    Export::Wrapper(Reach::Tasks),
                                );
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
                        let reach = reaches.within(tree.span(declaration))?;
                        Some(if self.closed(declaration) {
                            reach
                        } else {
                            Reach::Tasks
                        })
                    })
                    .flatten();
                    let export = registers(symbol)
                        .or(anonymous.map(Export::Wrapper))
                        .or_else(|| registers(self.conditional_registration_binding(declaration)))
                        .or_else(|| match self.value_origin(declaration) {
                            Some(Origin::Wrapper(reach)) => Some(Export::Wrapper(reach)),
                            _ => None,
                        });
                    if let Some(export) = export {
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
                    let theirs = if let Some(module) =
                        dialect.api_modules.iter().copied().find(|m| *m == source)
                    {
                        dialect
                            .registrations
                            .iter()
                            .map(|registration| {
                                (
                                    registration.name.to_owned(),
                                    Export::Api {
                                        registers: registration.registers,
                                        module,
                                        name: registration.name,
                                    },
                                )
                            })
                            .collect()
                    } else {
                        modules
                            .project_exports(self.path, source, self.setting, depth + 1, imports)
                            .unwrap_or_default()
                    };
                    for (name, export) in theirs {
                        if name != "default" && name != unreadable_fixture_export("default") {
                            let forwarded = export.through_star(
                                star_exports.get(&name).copied(),
                                stars == 0,
                                spelled.contains(&name),
                            );
                            let held = star_exports.entry(name).or_insert(forwarded);
                            *held = held.widest(forwarded);
                        }
                    }
                    stars += 1;
                }
                _ => {}
            }
        }
        // Explicit known exports win independently of declaration order. Spelled-but-unproved
        // values already made the corresponding star conservative above.
        star_exports.extend(exports);
        star_exports
    }

    /// The lexical facts around a site, innermost first.
    fn labels(&self, node: NodeId, calls: &HashMap<NodeId, SiteKind>) -> Vec<Label> {
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
                    Some(call)
                        if tree.kind(call) == "CallExpression" && calls.contains_key(&call) =>
                    {
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

/// The private export-cache key recording that a derived test API has unreadable fixtures.
fn unreadable_fixture_export(name: &str) -> String {
    format!("{UNREADABLE_FIXTURE}:{name}")
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
