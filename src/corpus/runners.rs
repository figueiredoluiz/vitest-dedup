//! Invocation frames for owned imported registration templates.
use super::{Hook, Origin, Run, Setting, TestCase};
use crate::framework::Registers;
use crate::inventory::{self, Inventory, SiteKind};
use crate::lang::{Declaration, Imported, Lines, NodeId, Parsed, SymbolId};
use crate::model::{Position, Span};
use crate::normal::{self, Normalizer, Structure};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One already bound template; its node and symbol IDs belong only to its own parsed module.
#[derive(Clone)]
pub(super) struct Frame {
    pub function: NodeId,
    pub bindings: HashMap<SymbolId, normal::InvocationValue>,
    pub values: HashMap<SymbolId, Value>,
    pub sites: HashSet<NodeId>,
    /// In the calling file's own module, its top-level registrations: a read of that module for a
    /// same-file helper's tests treats them as the caller's read does, not as module setup.
    pub registrations: HashSet<NodeId>,
}

/// One fully read call; a failure discards the complete file's proposed expansion batch.
pub(super) struct Expansion {
    pub scheduling_safe: bool,
    pub call: NodeId,
    pub tests: Vec<TestCase>,
    pub sources: Vec<(PathBuf, String)>,
    /// Hooks registered by this proved invocation, attached to its caller scope.
    pub hooks: Vec<ImportedHook>,
    pub diagnostic_hooks: Vec<(PathBuf, super::records::DiagnosticHook)>,
}

/// A hook written in a runner and registered at one caller invocation.
#[derive(Clone)]
pub(super) struct ImportedHook {
    pub call: NodeId,
    pub hook: Hook,
}

/// The registrations one invocation contributes before the caller assigns their scope.
struct Invocation {
    scheduling_safe: bool,
    tests: Vec<TestCase>,
    hooks: Vec<Hook>,
    diagnostic_hooks: Vec<(PathBuf, super::records::DiagnosticHook)>,
}

/// Complete expansions, or the caller scopes whose effects remain unproved.
#[derive(Default)]
pub(super) struct Batch {
    pub calls: Vec<Expansion>,
    pub refused: HashSet<Option<NodeId>>,
    /// Sites written inside a same-file helper: they register only through the admitted calls,
    /// which carry their cases, so the file does not read them again.
    pub templates: HashSet<NodeId>,
}

/// Titles read the invocation's symbol bindings, never same-spelled names in another scope.
pub(super) fn title(parsed: &Parsed, call: NodeId, frame: Option<&Frame>) -> Option<String> {
    match frame {
        Some(frame) => normal::titles::title_with_bindings(parsed, call, &frame.values),
        None => normal::titles::title(parsed, call),
    }
}

/// [`title`] inside enclosing suite rows, which also bind their proved parameters, and may read a
/// `let` the body around the suite callbacks leaves. A value JSON
/// cannot hold (undefined, bigint, a non-finite number) is left unbound, so reading it refuses.
pub(super) fn title_in_suite(
    parsed: &Parsed,
    call: NodeId,
    frame: Option<&Frame>,
    suite: &HashMap<SymbolId, crate::lang::Scalar>,
    deferral: &normal::titles::Deferral<'_>,
) -> Option<String> {
    use crate::lang::Scalar;
    let mut values = frame.map(|frame| frame.values.clone()).unwrap_or_default();
    values.extend(suite.iter().filter_map(|(&symbol, scalar)| {
        let value = match scalar {
            Scalar::Null => Value::Null,
            Scalar::Bool(value) => Value::Bool(*value),
            Scalar::Number(value) => Value::Number(serde_json::Number::from_f64(*value)?),
            Scalar::String(text) => Value::String(text.clone()),
            Scalar::Undefined | Scalar::BigInt(_) => return None,
        };
        Some((symbol, value))
    }));
    normal::titles::title_deferred(parsed, call, &values, Some(deferral))
}

/// Expand a closed root registration scope; other contexts retain their explicit exclusions.
/// No subset of a failed batch may erase a wrapper's context or claim complete admission.
pub(super) fn expand(
    path: &Path,
    source: &str,
    parsed: &Parsed,
    found: &Inventory,
    setting: Setting,
    run: &mut Run,
) -> Batch {
    if let Some((calls, templates)) = closed_batch(path, source, parsed, found, setting, run) {
        return Batch {
            calls,
            refused: HashSet::new(),
            templates,
        };
    }
    let structure = normal::Structure::of(parsed, found);
    let normalizer = normal::Normalizer::new(
        path,
        parsed,
        normal::Setting {
            dialect: setting.dialect,
            root: setting.root,
            exports: &run.exports,
        },
        &structure,
    );
    let refused = found
        .sites
        .iter()
        .filter(|site| {
            site.kind == SiteKind::Wrapper(inventory::Reach::Tasks)
                && site.provenance != inventory::Provenance::Derived
        })
        .filter_map(|site| {
            normalizer
                .suites_around(site.node)
                .map(|scopes| scopes.last().copied())
        })
        .collect();
    Batch {
        calls: Vec::new(),
        refused,
        templates: HashSet::new(),
    }
}

/// The caller is a finite root sequence; imported and same-file callables use one bounded proof.
/// The sequence recurses into bare, provably titled suite callbacks: their bodies close the same
/// way, and a helper call inside one registers into that suite (measured: Vitest 5.0.5 runs
/// describe callbacks after the module body, so every top-level binding is initialized there).
/// A caller that writes a hook refuses, as does a call inside a suite whose invocation registers
/// hooks: hook context reaches handwritten and expanded tests by different paths, so it is not
/// composed here. Also returns the sites written inside the same-file helpers the calls run.
fn closed_batch(
    path: &Path,
    source: &str,
    parsed: &Parsed,
    found: &Inventory,
    setting: Setting,
    run: &mut Run,
) -> Option<(Vec<Expansion>, HashSet<NodeId>)> {
    let tree = &parsed.tree;
    let helpers = local_helpers(parsed);
    if parsed.parse_errors != 0
        || found.setup_access
        || found.sites.len() > 2000
        || !found.loads.is_empty()
        || !found.escapes.is_empty()
        || found.sites.iter().any(|site| {
            !matches!(
                site.kind,
                SiteKind::Registers(Registers::Test | Registers::Suite) | SiteKind::Wrapper(_)
            )
        })
    {
        return None;
    }
    // The closed sequence: the file root plus the callback bodies of bare suite sites whose
    // titles render. Statements inside other shapes (a skipped or table suite, a loop) are not
    // admitted positions, so a helper call there refuses.
    let sites: HashMap<NodeId, &crate::inventory::Site> =
        found.sites.iter().map(|site| (site.node, site)).collect();
    let mut top_level_statements = HashSet::new();
    let mut suite_statements = HashSet::new();
    let mut registrations = HashSet::new();
    let mut stated: HashMap<NodeId, NodeId> = HashMap::new();
    let mut suite_bodies: Vec<Vec<NodeId>> = Vec::new();
    let walk = |statements: &[NodeId],
                top_level: bool,
                top_level_statements: &mut HashSet<NodeId>,
                suite_statements: &mut HashSet<NodeId>,
                registrations: &mut HashSet<NodeId>,
                stated: &mut HashMap<NodeId, NodeId>,
                suite_bodies: &mut Vec<Vec<NodeId>>|
     -> Option<()> {
        for &statement in statements {
            if tree.kind(statement) == "ImportDeclaration" {
                continue;
            }
            if tree.kind(statement) == "ExpressionStatement" {
                if let Some(expression) = tree
                    .child(statement, "expression")
                    .map(|node| tree.inner(node))
                {
                    if let Some(&site) = sites.get(&expression) {
                        registrations.insert(site.node);
                        stated.insert(site.node, statement);
                        if top_level {
                            top_level_statements.insert(statement);
                        } else {
                            suite_statements.insert(statement);
                        }
                        // Every suite's callback runs while the file is collected (a skipped or
                        // table suite's too), so a suite is admitted only when its body opens to
                        // the sequence: a bare suite whose title renders. Any other suite leaves
                        // collection effects unproved, and the file refuses.
                        if matches!(site.kind, SiteKind::Registers(Registers::Suite)) {
                            if !site.chain.is_empty()
                                || normal::titles::title(parsed, site.node).is_none()
                            {
                                return None;
                            }
                            // Exactly a title and a callback that takes nothing: an options
                            // argument or a parameter default is evaluated while the suite
                            // registers, and can throw before its body runs.
                            let arguments = tree.list(site.node, "arguments");
                            let body = (arguments.len() == 2)
                                .then(|| arguments[1])
                                .flatten()
                                .map(|argument| tree.inner(argument))
                                .filter(|&argument| {
                                    tree.is_function(argument)
                                        && !tree.flag(argument, "async")
                                        && !tree.flag(argument, "generator")
                                        && tree.list(argument, "params").is_empty()
                                })
                                .and_then(|callback| tree.child(callback, "body"))
                                .filter(|&body| tree.kind(body) == "BlockStatement")
                                .map(|body| {
                                    tree.list(body, "body")
                                        .into_iter()
                                        .flatten()
                                        .collect::<Vec<_>>()
                                })?;
                            suite_bodies.push(body);
                        }
                        continue;
                    }
                }
            }
            if !inventory::runners::inert_declaration(parsed, statement) {
                return None;
            }
        }
        Some(())
    };
    walk(
        &tree
            .list(tree.root(), "body")
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
        true,
        &mut top_level_statements,
        &mut suite_statements,
        &mut registrations,
        &mut stated,
        &mut suite_bodies,
    )?;
    while let Some(statements) = suite_bodies.pop() {
        walk(
            &statements,
            false,
            &mut top_level_statements,
            &mut suite_statements,
            &mut registrations,
            &mut stated,
            &mut suite_bodies,
        )?;
    }
    // A site inside a helper is its template; it registers only when an admitted call runs it.
    // Which of its statements run is the template reader's to prove when a call reaches it. A
    // labeled site anywhere else (a loop, a branch, a non-suite callback) is no closed position:
    // the file keeps today's exclusions.
    let mut templates = HashSet::new();
    for site in &found.sites {
        if site.labels.is_empty() || stated.contains_key(&site.node) {
            continue;
        }
        let enclosing = tree
            .ancestors(site.node)
            .find(|&node| tree.is_function(node));
        if !enclosing
            .is_some_and(|function| helpers.values().any(|helper| helper.function == function))
        {
            return None;
        }
        templates.insert(site.node);
    }
    // A call is admitted where its statement sits in a closed position; a call inside a suite
    // body registers into that suite (its labels are exactly the registration body's).
    let calls: Vec<_> = found
        .sites
        .iter()
        .filter(|site| {
            matches!(site.kind, SiteKind::Wrapper(_))
                && site
                    .labels
                    .iter()
                    .all(|label| matches!(label, crate::inventory::Label::RegistrationBody))
                && stated.get(&site.node).is_some_and(|statement| {
                    top_level_statements.contains(statement) || suite_statements.contains(statement)
                })
        })
        .collect();
    if calls.is_empty() || calls.len() > 2000 {
        return None;
    }
    let owner = run.sources.relative(path, setting.root)?;
    let mut planner = Planner {
        setting,
        owner,
        sources: BTreeMap::new(),
        checked: HashSet::new(),
        read_sizes: HashMap::new(),
        source_bytes: 0,
        active: HashSet::new(),
        left: 2000,
        work: 0,
        registrations: registrations.clone(),
        invocation: Vec::new(),
    };
    // A template's title reads its helper's parameters; each call checks it with their values.
    // An inert chain (`skip`, `todo`) is the caller's own labeled case and reads the same way.
    for site in found.sites.iter().filter(|site| {
        site.kind == SiteKind::Registers(Registers::Test) && !templates.contains(&site.node)
    }) {
        let inert = site.chain.iter().all(|member| {
            matches!(
                setting.dialect.chain_member(member),
                Some(crate::framework::Chain::Skip | crate::framework::Chain::Todo)
            )
        });
        if !site.chain.is_empty() && !inert {
            return None;
        }
        let arguments = tree.list(site.node, "arguments");
        if arguments.len() != 2
            || !arguments[1].is_some_and(|argument| tree.is_function(tree.inner(argument)))
        {
            return None;
        }
        // Collection evaluates the title, so it must run no code. A title too long to compare is
        // still inert: read without that limit, within the batch's work budget.
        if normal::titles::title(parsed, site.node).is_none() {
            let (_, spent) = arguments[0]
                .and_then(|title| normal::titles::title_within(parsed, title, planner.left()))?;
            planner.charge(spent)?;
        }
    }
    let mut expansions = Vec::new();
    let mut checked_bindings = HashSet::new();
    for &call in &calls {
        let callee = tree.inner(tree.child(call.node, "callee")?);
        if tree.kind(callee) != "Identifier" {
            return None;
        }
        let symbol = parsed.scopes.symbol_at(tree.span(callee).start)?;
        if !matches!(
            parsed.scopes.symbol(symbol).declaration,
            Declaration::Import { .. }
        ) && !helpers.contains_key(&symbol)
        {
            return None;
        }
        checked_bindings.insert(symbol);
        planner.invocation = vec![tree.span(call.node).start];
        let invocation = planner.call(
            Module {
                path,
                source,
                parsed,
            },
            call.node,
            None,
            run,
            8,
        )?;
        // Hooks registered inside a suite apply to every test in it, handwritten ones included;
        // only the file-wide imported lifecycle is composed, so a suite-position call refuses.
        let in_suite = stated
            .get(&call.node)
            .is_some_and(|statement| suite_statements.contains(statement));
        if in_suite && !invocation.hooks.is_empty() {
            return None;
        }
        expansions.push(Expansion {
            scheduling_safe: invocation.scheduling_safe,
            call: call.node,
            tests: invocation.tests,
            sources: Vec::new(),
            hooks: invocation
                .hooks
                .into_iter()
                .map(|hook| ImportedHook {
                    call: call.node,
                    hook,
                })
                .collect(),
            diagnostic_hooks: invocation.diagnostic_hooks,
        });
    }
    // An unused import is still evaluated. Every project dependency must have a completed proof.
    for statement in tree
        .list(tree.root(), "body")
        .into_iter()
        .flatten()
        .filter(|&node| {
            tree.kind(node) == "ImportDeclaration" && tree.text(node, "importKind") != Some("type")
        })
    {
        let specifier = tree
            .child(statement, "source")
            .and_then(|node| tree.text(node, "value"))?;
        if !setting.dialect.is_api_module(specifier) {
            let target = run.sources.resolve(path, specifier, setting.root)?;
            if !planner.checked.contains(&target) {
                return None;
            }
            for specifier in tree.list(statement, "specifiers").into_iter().flatten() {
                if tree.text(specifier, "importKind") == Some("type") {
                    continue;
                }
                let local = tree.child(specifier, "local")?;
                let symbol = parsed.scopes.symbol_at(tree.span(local).start)?;
                if !checked_bindings.contains(&symbol) {
                    return None;
                }
            }
        }
    }
    // A helper registers only through calls the batch runs: every use of it is such a call's
    // callee, and a `const` helper is initialized before the first top-level call runs.
    // A helper registers only through calls the batch runs: every use of it is such a call's
    // callee — the call itself, or a nested call the planner proves inside a helper body. A
    // reference anywhere else (a skipped suite's body, a value read) is an escape.
    let admitted_callees: HashSet<u32> = calls
        .iter()
        .filter_map(|site| tree.child(site.node, "callee"))
        .map(|callee| tree.span(tree.inner(callee)).start)
        .collect();
    let helper_callees: HashSet<u32> = found
        .sites
        .iter()
        .filter(|site| matches!(site.kind, SiteKind::Wrapper(_)))
        .filter(|site| {
            helpers
                .values()
                .any(|helper| tree.span(helper.function).contains(tree.span(site.node)))
        })
        .filter_map(|site| tree.child(site.node, "callee"))
        .map(|callee| tree.span(tree.inner(callee)).start)
        .collect();
    let callees: HashSet<u32> = admitted_callees.union(&helper_callees).copied().collect();
    // Only a top-level call executes during the module body, in statement order: that is the
    // early execution a `const` initializer and every top-level read must precede. A
    // suite-position call runs after the whole body (measured: Vitest 5.0.5 describe callbacks
    // run last). A top-level call can reach any helper through another, so while one exists
    // every helper is checked; a file with only suite-position calls has no early execution.
    let first_call = calls
        .iter()
        .filter(|site| {
            stated
                .get(&site.node)
                .is_some_and(|statement| top_level_statements.contains(statement))
        })
        .map(|call| tree.span(call.node).start)
        .min();
    for (&symbol, helper) in &helpers {
        let binding = parsed.scopes.symbol(symbol);
        // A function that registers nothing and no call runs as a wrapper is ordinary code.
        let registers = templates
            .iter()
            .any(|&site| tree.span(helper.function).contains(tree.span(site)))
            || binding
                .references
                .iter()
                .any(|reference| callees.contains(&reference.span.start));
        if !registers {
            continue;
        }
        if binding
            .references
            .iter()
            .any(|reference| reference.write || !callees.contains(&reference.span.start))
        {
            return None;
        }
        if let Some(first_call) = first_call {
            if helper.constant && tree.span(helper.statement).end > first_call {
                return None;
            }
            // What the helper reads while it registers (a title, a nested call's argument) is
            // read when the first top-level call runs: a top-level variable or class declared
            // after that call is uninitialized then, or `undefined` for a `var`.
            let span = tree.span(helper.function);
            let late = parsed.scopes.symbols().any(|read| {
                let declared = parsed.scopes.symbol(read);
                declared.top_level
                    && matches!(
                        declared.declaration,
                        Declaration::Variable(_) | Declaration::Class(_)
                    )
                    && declared
                        .references
                        .iter()
                        .any(|reference| span.contains(reference.span))
                    && parsed
                        .scopes
                        .declaration_node(read)
                        .and_then(|node| top_level_statement(tree, node))
                        .is_none_or(|statement| tree.span(statement).end > first_call)
            });
            if late {
                return None;
            }
        }
    }
    if let Some(last) = expansions.last_mut() {
        last.sources = planner.sources.into_iter().collect();
    }
    Some((expansions, templates))
}

/// The module-body statement holding `node`.
fn top_level_statement(tree: &crate::lang::Tree, node: NodeId) -> Option<NodeId> {
    std::iter::once(node)
        .chain(tree.ancestors(node))
        .find(|&ancestor| tree.parent(ancestor) == Some(tree.root()))
}

/// A top-level function a call in the same file can run: a `function` declaration, or a `const`
/// bound to a function or arrow.
struct Helper {
    /// The function the call runs.
    function: NodeId,
    /// The top-level statement declaring it.
    statement: NodeId,
    /// Whether it is a `const`, which is not initialized until its statement runs.
    constant: bool,
}

/// The module's same-file helpers, by their binding.
fn local_helpers(parsed: &Parsed) -> HashMap<SymbolId, Helper> {
    let tree = &parsed.tree;
    let mut helpers = HashMap::new();
    for statement in tree.list(tree.root(), "body").into_iter().flatten() {
        let (id, function, constant) = match tree.kind(statement) {
            "FunctionDeclaration" => (tree.child(statement, "id"), Some(statement), false),
            "VariableDeclaration" if tree.text(statement, "kind") == Some("const") => {
                match tree.list(statement, "declarations").as_slice() {
                    [Some(declarator)] => (
                        tree.child(*declarator, "id"),
                        tree.child(*declarator, "init")
                            .map(|init| tree.inner(init))
                            .filter(|&init| tree.is_function(init)),
                        true,
                    ),
                    _ => continue,
                }
            }
            _ => continue,
        };
        let (Some(id), Some(function)) = (id, function) else {
            continue;
        };
        if tree.kind(id) != "Identifier" {
            continue;
        }
        if let Some(symbol) = parsed.scopes.symbol_at(tree.span(id).start) {
            helpers.insert(
                symbol,
                Helper {
                    function,
                    statement,
                    constant,
                },
            );
        }
    }
    helpers
}

/// One lexical module view; IDs never move from this tree into another module's environment.
#[derive(Clone, Copy)]
struct Module<'a> {
    path: &'a Path,
    source: &'a str,
    parsed: &'a Parsed,
}

/// The work one expansion batch may do, in bytes read or rendered.
const MAX_WORK: usize = 8 * 1024 * 1024;

/// One atomic expansion batch shares its source, work, admission and recursion bounds.
struct Planner<'a> {
    setting: Setting<'a>,
    owner: PathBuf,
    sources: BTreeMap<PathBuf, String>,
    checked: HashSet<PathBuf>,
    read_sizes: HashMap<PathBuf, usize>,
    source_bytes: usize,
    active: HashSet<(PathBuf, u32, u32)>,
    left: usize,
    work: usize,
    /// The calling file's top-level registering calls; see [`Frame::registrations`].
    registrations: HashSet<NodeId>,
    /// The registering calls of the invocation being planned, by start byte, outermost first: the
    /// cases it produces are distinct declarations, one per execution path.
    invocation: Vec<u32>,
}

impl Planner<'_> {
    /// Refuse work before a duplicated argument or a rendered frame can amplify it further.
    fn charge(&mut self, cost: usize) -> Option<()> {
        self.work = self.work.saturating_add(cost);
        (self.work <= MAX_WORK).then_some(())
    }

    /// The work the batch may still do.
    fn left(&self) -> usize {
        MAX_WORK.saturating_sub(self.work)
    }

    /// Resolve and bind a call from its actual lexical environment, charging export hops too.
    fn call(
        &mut self,
        module: Module<'_>,
        call: NodeId,
        parent: Option<&Frame>,
        run: &mut Run,
        depth: usize,
    ) -> Option<Invocation> {
        if depth == 0 {
            return None;
        }
        let parsed = module.parsed;
        let tree = &parsed.tree;
        self.charge(tree.nodes().count())?;
        let callee = tree.inner(tree.child(call, "callee")?);
        if tree.kind(callee) != "Identifier" {
            return None;
        }
        let symbol = parsed.scopes.symbol_at(tree.span(callee).start)?;
        let binding = parsed.scopes.symbol(symbol);
        if binding.references.iter().any(|reference| reference.write) {
            return None;
        }
        let empty = HashMap::new();
        let values = parent.map_or(&empty, |frame| &frame.values);
        let arguments = tree.list(call, "arguments");
        if arguments.len() > 2000 {
            return None;
        }
        let mut actual = Vec::new();
        for argument in arguments {
            // Reading an argument may copy more text than it keeps: a template of an object
            // copies each property. Both the reading and what the frame keeps are charged.
            let (value, spent) =
                normal::titles::value_within(parsed, argument?, values, self.left())?;
            self.charge(spent)?;
            self.charge(serde_json::to_vec(&value).ok()?.len())?;
            actual.push(value);
        }
        if let Declaration::Import {
            import,
            ref imported,
            ..
        } = binding.declaration
        {
            let name = match imported {
                Imported::Named(name) => name.as_str(),
                Imported::Default => "default",
                Imported::Namespace => return None,
            };
            let specifier = tree
                .child(import, "source")
                .and_then(|node| tree.text(node, "value"))?;
            if self.setting.dialect.is_api_module(specifier) {
                return None;
            }
            let helper = run
                .sources
                .resolve(module.path, specifier, self.setting.root)?;
            let template = inventory::runners::template(
                &run.sources,
                &helper,
                name,
                inventory::Setting {
                    dialect: self.setting.dialect,
                    globals: self.setting.globals,
                    root: self.setting.root,
                },
            )
            .ok()?;
            let depth = depth.checked_sub(template.resolution_depth)?;
            for (path, bytes) in &template.dependency_bytes {
                let path = run.sources.relative(path, self.setting.root)?;
                if let Some(known) = self.read_sizes.get(&path) {
                    if known != bytes {
                        return None;
                    }
                } else {
                    self.source_bytes = self.source_bytes.saturating_add(*bytes);
                    if self.source_bytes > 8 * 1024 * 1024 {
                        return None;
                    }
                    self.read_sizes.insert(path, *bytes);
                }
            }
            self.checked.extend(template.dependencies);
            let plan = inventory::runners::Plan {
                function: template.function,
                parameters: template.parameters,
                steps: template.steps,
            };
            self.execute(
                Module {
                    path: &template.path,
                    source: &template.source,
                    parsed: &template.parsed,
                },
                plan,
                actual,
                None,
                run,
                depth,
            )
        } else {
            let found = self.inventory(module, run)?;
            let plan = inventory::runners::local(parsed, &found, callee).ok()?;
            self.execute(module, plan, actual, parent, run, depth - 1)
        }
    }

    /// Normalize a frame once, then interleave its direct registrations and child invocations.
    fn execute(
        &mut self,
        module: Module<'_>,
        plan: inventory::runners::Plan,
        actual: Vec<Value>,
        parent: Option<&Frame>,
        run: &mut Run,
        depth: usize,
    ) -> Option<Invocation> {
        let parsed = module.parsed;
        let tree = &parsed.tree;
        let relative = run.sources.relative(module.path, self.setting.root)?;
        let span = tree.span(plan.function);
        let active = (relative.clone(), span.start, span.end);
        if !self.active.insert(active.clone()) {
            return None;
        }
        let result = (|| {
            self.left = self.left.checked_sub(plan.steps.len().max(1))?;
            if let Some(known) = self.sources.get(&relative) {
                if known != module.source {
                    return None;
                }
            } else {
                self.sources
                    .insert(relative.clone(), module.source.to_owned());
            }
            let mut frame = Frame {
                function: plan.function,
                bindings: parent.map_or_else(HashMap::new, |frame| frame.bindings.clone()),
                values: parent.map_or_else(HashMap::new, |frame| frame.values.clone()),
                sites: HashSet::new(),
                registrations: if relative == self.owner {
                    self.registrations.clone()
                } else {
                    HashSet::new()
                },
            };
            for (index, &parameter) in plan.parameters.iter().enumerate() {
                if parsed.scopes.symbol(parameter).references.is_empty() {
                    continue;
                }
                let value = actual.get(index)?;
                let binding = normal::InvocationValue::scalar(value)?;
                self.charge(
                    binding
                        .form
                        .len()
                        .saturating_add(binding.shape.len())
                        .saturating_mul(tree.nodes().count()),
                )?;
                frame.bindings.insert(parameter, binding);
                frame.values.insert(parameter, value.clone());
            }
            for step in &plan.steps {
                match step {
                    inventory::runners::Step::Registration(node, Registers::Test) => {
                        let callback = normal::callback(tree, &parsed.scopes, *node, true)?;
                        if !callback.inline || tree.list(*node, "arguments").len() != 2 {
                            return None;
                        }
                        let callback = callback.function?;
                        if tree.child(callback, "body").is_some_and(|body| {
                            crate::lang::uses_receiver(tree, &parsed.scopes, body)
                        }) {
                            return None;
                        }
                        title(parsed, *node, Some(&frame))?;
                        frame.sites.insert(*node);
                    }
                    // The caller owns lifecycle context. These declarations are captured below
                    // without entering the helper test frame's ambient context.
                    inventory::runners::Step::Registration(_, Registers::Hook(_)) => {}
                    inventory::runners::Step::Call(_) => {}
                    _ => return None,
                }
            }
            let mut direct: HashMap<u32, Vec<TestCase>> = HashMap::new();
            let mut diagnostic_hooks = Vec::new();
            let hook_sites: Vec<_> = plan
                .steps
                .iter()
                .filter_map(|step| match step {
                    inventory::runners::Step::Registration(node, Registers::Hook(_)) => Some(*node),
                    _ => None,
                })
                .collect();
            if !frame.sites.is_empty() {
                let corpus = super::read_parsed(
                    module.path,
                    module.source,
                    self.setting,
                    run,
                    parsed,
                    Some(frame.clone()),
                )
                .ok()?;
                diagnostic_hooks.extend(corpus.diagnostic_hooks.clone());
                for mut case in corpus.tests {
                    case.origin = Some(Origin {
                        path: relative.clone(),
                        registration_files: vec![self.owner.clone()],
                        call: self.invocation.clone(),
                    });
                    direct.entry(case.span.start_byte).or_default().push(case);
                }
            }
            if run.diagnostics && !hook_sites.is_empty() {
                let mut diagnostic_frame = frame.clone();
                diagnostic_frame.sites.extend(hook_sites.iter().copied());
                let diagnostics = super::read_parsed(
                    module.path,
                    module.source,
                    self.setting,
                    run,
                    parsed,
                    Some(diagnostic_frame),
                )
                .ok()?;
                diagnostic_hooks.extend(diagnostics.diagnostic_hooks);
            }
            let mut tests = Vec::new();
            let mut hooks = Vec::new();
            let found = self.inventory(module, run)?;
            let structure = Structure::of(parsed, &found);
            let mut hook_normalizer = Normalizer::new(
                module.path,
                parsed,
                normal::Setting {
                    dialect: self.setting.dialect,
                    root: self.setting.root,
                    exports: &run.exports,
                },
                &structure,
            )
            .for_registration(frame.function, frame.bindings.clone());
            let mut normalized_hooks = HashMap::new();
            for step in &plan.steps {
                if let inventory::runners::Step::Registration(node, Registers::Hook(timing)) = step
                {
                    normalized_hooks.insert(
                        *node,
                        self.hook(module, *node, *timing, &mut hook_normalizer, &relative)?,
                    );
                }
            }
            let scheduled: Vec<_> = frame.sites.iter().chain(&hook_sites).copied().collect();
            let mut scheduling_safe = hook_normalizer.runner_scheduling_safe(&scheduled);
            drop(hook_normalizer);
            for step in plan.steps {
                match step {
                    inventory::runners::Step::Registration(node, Registers::Test) => {
                        tests.extend(direct.remove(&tree.span(node).start)?)
                    }
                    inventory::runners::Step::Registration(node, Registers::Hook(_)) => {
                        hooks.push(normalized_hooks.remove(&node)?);
                    }
                    inventory::runners::Step::Call(call) => {
                        // A nested call is its own execution: two calls of one runner inside a
                        // helper register two runtime cases each time the helper runs.
                        self.invocation.push(tree.span(call).start);
                        let nested = self.call(module, call, Some(&frame), run, depth);
                        self.invocation.pop();
                        let nested = nested?;
                        scheduling_safe &= nested.scheduling_safe;
                        tests.extend(nested.tests);
                        hooks.extend(nested.hooks);
                        diagnostic_hooks.extend(nested.diagnostic_hooks);
                    }
                    _ => return None,
                }
            }
            Some(Invocation {
                scheduling_safe,
                tests,
                hooks,
                diagnostic_hooks,
            })
        })();
        self.active.remove(&active);
        result
    }

    /// Read one module's registrations using the planner's shared resolution cache.
    fn inventory(&self, module: Module<'_>, run: &mut Run) -> Option<Inventory> {
        let found = inventory::inventory(
            module.path,
            module.source,
            module.parsed,
            inventory::Setting {
                dialect: self.setting.dialect,
                globals: self.setting.globals,
                root: self.setting.root,
            },
            &mut run.modules,
        );
        found.import_limit.is_none().then_some(found)
    }

    /// Read an imported hook's body under the invocation bindings. A runner with an opaque hook
    /// stays unexpanded: the ordinary test rules do not model that hook's setup semantics.
    fn hook(
        &mut self,
        module: Module<'_>,
        node: NodeId,
        timing: crate::framework::HookTiming,
        normalizer: &mut Normalizer<'_>,
        relative: &Path,
    ) -> Option<Hook> {
        // Only per-test setup establishes the same starting state for every test. Teardown
        // writes can distinguish the first execution from later ones despite being idempotent.
        if timing != crate::framework::HookTiming::BeforeEach {
            return None;
        }
        let tree = &module.parsed.tree;
        let arguments = tree.list(node, "arguments");
        let callback = (arguments.len() == 1)
            .then(|| arguments.first().copied().flatten())
            .flatten()
            .map(|argument| tree.inner(argument))
            .filter(|&callback| tree.is_function(callback));
        let key = callback.and_then(|callback| normalizer.hook_effects(callback))?;
        let lines = Lines::new(module.source);
        let position = |offset| {
            let (line, column) = lines.position(module.source, offset);
            Position { line, column }
        };
        Some(Hook {
            path: Some(relative.to_path_buf()),
            registration_files: vec![self.owner.clone()],
            span: Span {
                start: position(tree.span(node).start),
                end: position(tree.span(node).end),
                start_byte: tree.span(node).start,
                end_byte: tree.span(node).end,
            },
            timing,
            label: self
                .setting
                .dialect
                .registrations
                .iter()
                .find(|entry| entry.registers == Registers::Hook(timing))?
                .canonical,
            scope: None,
            // Symbol IDs are local to this parsed module. The module path makes two helpers'
            // otherwise equal lexical IDs distinct without weakening comparisons within one helper.
            key: Some(format!("{}\u{0}{key}", module.path.display())),
        })
    }
}
