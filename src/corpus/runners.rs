//! Invocation frames for owned imported registration templates.
use super::{Origin, Run, Setting, TestCase};
use crate::framework::Registers;
use crate::inventory::{self, Inventory, SiteKind};
use crate::lang::{Declaration, Imported, NodeId, Parsed, SymbolId};
use crate::normal;
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
}

/// One fully read call; a failure discards the complete file's proposed expansion batch.
pub(super) struct Expansion {
    pub call: NodeId,
    pub tests: Vec<TestCase>,
    pub sources: Vec<(PathBuf, String)>,
}

/// Complete expansions, or the caller scopes whose effects remain unproved.
#[derive(Default)]
pub(super) struct Batch {
    pub calls: Vec<Expansion>,
    pub refused: HashSet<Option<NodeId>>,
}

/// Titles read the invocation's symbol bindings, never same-spelled names in another scope.
pub(super) fn title(parsed: &Parsed, call: NodeId, frame: Option<&Frame>) -> Option<String> {
    match frame {
        Some(frame) => normal::titles::title_with_bindings(parsed, call, &frame.values),
        None => normal::titles::title(parsed, call),
    }
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
    if let Some(calls) = closed_batch(path, source, parsed, found, setting, run) {
        return Batch {
            calls,
            refused: HashSet::new(),
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
    }
}

/// The caller is a finite root sequence; imported and nested callables use one bounded proof.
/// Hooks and suite/loop frames still need their own lifecycle/iteration proofs.
fn closed_batch(
    path: &Path,
    source: &str,
    parsed: &Parsed,
    found: &Inventory,
    setting: Setting,
    run: &mut Run,
) -> Option<Vec<Expansion>> {
    let tree = &parsed.tree;
    let calls: Vec<_> = found
        .sites
        .iter()
        .filter(|site| matches!(site.kind, SiteKind::Wrapper(_)))
        .collect();
    if parsed.parse_errors != 0
        || calls.is_empty()
        || found.setup_access
        || calls.len() > 2000
        || !found.loads.is_empty()
        || !found.escapes.is_empty()
        || found.sites.iter().any(|site| {
            !site.labels.is_empty()
                || !matches!(
                    site.kind,
                    SiteKind::Registers(Registers::Test) | SiteKind::Wrapper(_)
                )
        })
    {
        return None;
    }
    for statement in tree.list(tree.root(), "body").into_iter().flatten() {
        if tree.kind(statement) == "ImportDeclaration" {
            continue;
        }
        if tree.kind(statement) == "ExpressionStatement"
            && tree
                .child(statement, "expression")
                .is_some_and(|expression| {
                    found
                        .sites
                        .iter()
                        .any(|site| site.node == tree.inner(expression))
                })
        {
            continue;
        }
        if !inventory::runners::inert_declaration(parsed, statement) {
            return None;
        }
    }
    for site in found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Test))
    {
        let arguments = tree.list(site.node, "arguments");
        if !site.chain.is_empty()
            || arguments.len() != 2
            || normal::titles::title(parsed, site.node).is_none()
            || !arguments[1].is_some_and(|argument| tree.is_function(tree.inner(argument)))
        {
            return None;
        }
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
    };
    let mut expansions = Vec::new();
    let mut checked_bindings = HashSet::new();
    for call in calls {
        let callee = tree.inner(tree.child(call.node, "callee")?);
        if tree.kind(callee) != "Identifier"
            || !tree.outer_parent(call.node).is_some_and(|statement| {
                tree.kind(statement) == "ExpressionStatement"
                    && tree.parent(statement) == Some(tree.root())
            })
        {
            return None;
        }
        let symbol = parsed.scopes.symbol_at(tree.span(callee).start)?;
        if !matches!(
            parsed.scopes.symbol(symbol).declaration,
            Declaration::Import { .. }
        ) {
            return None;
        }
        checked_bindings.insert(symbol);
        let tests = planner.call(
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
        expansions.push(Expansion {
            call: call.node,
            tests,
            sources: Vec::new(),
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
    if let Some(last) = expansions.last_mut() {
        last.sources = planner.sources.into_iter().collect();
    }
    Some(expansions)
}

/// One lexical module view; IDs never move from this tree into another module's environment.
#[derive(Clone, Copy)]
struct Module<'a> {
    path: &'a Path,
    source: &'a str,
    parsed: &'a Parsed,
}

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
}

impl Planner<'_> {
    /// Refuse work before a duplicated argument or a rendered frame can amplify it further.
    fn charge(&mut self, cost: usize) -> Option<()> {
        self.work = self.work.saturating_add(cost);
        (self.work <= 8 * 1024 * 1024).then_some(())
    }

    /// Resolve and bind a call from its actual lexical environment, charging export hops too.
    fn call(
        &mut self,
        module: Module<'_>,
        call: NodeId,
        parent: Option<&Frame>,
        run: &mut Run,
        depth: usize,
    ) -> Option<Vec<TestCase>> {
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
            let value =
                normal::titles::value_with_bindings(parsed, argument?, 0, &mut 256, values)?;
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
            let found = inventory::inventory(
                module.path,
                module.source,
                parsed,
                inventory::Setting {
                    dialect: self.setting.dialect,
                    globals: self.setting.globals,
                    root: self.setting.root,
                },
                &mut run.modules,
            );
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
    ) -> Option<Vec<TestCase>> {
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
                        if tree
                            .child(callback, "body")
                            .is_some_and(|body| crate::lang::uses_receiver(tree, body))
                        {
                            return None;
                        }
                        title(parsed, *node, Some(&frame))?;
                        frame.sites.insert(*node);
                    }
                    inventory::runners::Step::Call(_) => {}
                    _ => return None,
                }
            }
            let mut direct: HashMap<u32, Vec<TestCase>> = HashMap::new();
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
                for mut case in corpus.tests {
                    case.origin = Some(Origin {
                        path: relative.clone(),
                        registration_files: vec![self.owner.clone()],
                    });
                    direct.entry(case.span.start_byte).or_default().push(case);
                }
            }
            let mut tests = Vec::new();
            for step in plan.steps {
                match step {
                    inventory::runners::Step::Registration(node, _) => {
                        tests.extend(direct.remove(&tree.span(node).start)?)
                    }
                    inventory::runners::Step::Call(call) => {
                        tests.extend(self.call(module, call, Some(&frame), run, depth)?)
                    }
                    _ => return None,
                }
            }
            Some(tests)
        })();
        self.active.remove(&active);
        result
    }
}
