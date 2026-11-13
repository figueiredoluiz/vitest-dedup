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
    /// Derived registrations proved throughout this caller batch, used only for mutation facts.
    pub mutation_sites: HashSet<(u32, u32)>,
    /// Complete module inventory checked by the planner before normalization.
    pub inventory: Option<Box<Inventory>>,
    pub function: NodeId,
    pub bindings: HashMap<SymbolId, normal::InvocationValue>,
    pub values: HashMap<SymbolId, Value>,
    pub sites: HashSet<NodeId>,
    /// In the calling file's own module, its top-level registrations: a read of that module for a
    /// same-file helper's tests treats them as the caller's read does, not as module setup.
    pub registrations: HashSet<NodeId>,
    /// Returned-closure wrapper sites proved to be tests, with their derived modifier chain.
    pub derived: HashMap<NodeId, normal::Modifiers>,
}

impl Frame {
    /// Every invocation consumer sees the same caller-batch and per-call derived sites.
    pub(super) fn promote_inventory(&self, parsed: &Parsed, found: &mut Inventory) {
        if self.mutation_sites.is_empty() && self.derived.is_empty() {
            return;
        }
        promote_sites(found, |node| {
            let span = parsed.tree.span(node);
            self.derived.contains_key(&node)
                || self.mutation_sites.contains(&(span.start, span.end))
        });
    }
}

/// Apply recorded site proofs to a private inventory before any structural consumer reads it.
fn promote_sites(found: &mut Inventory, proved: impl Fn(NodeId) -> bool) {
    for site in &mut found.sites {
        if proved(site.node) {
            site.kind = SiteKind::Registers(Registers::Test);
            site.provenance = inventory::Provenance::Derived;
            site.chain.clear();
        }
    }
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
    /// Proved callback sites belonging to the caller's own parsed module.
    pub mutation_sites: HashSet<(u32, u32)>,
}

impl Batch {
    /// Carry a completed caller's mutation proofs into handwritten-test normalization.
    pub(super) fn promote_inventory(&self, parsed: &Parsed, found: &mut Inventory) {
        if self.mutation_sites.is_empty() {
            return;
        }
        promote_sites(found, |node| {
            let span = parsed.tree.span(node);
            self.mutation_sites.contains(&(span.start, span.end))
        });
    }
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
    if let Some(batch) = closed_batch(path, source, parsed, found, setting, run) {
        return batch;
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
        mutation_sites: HashSet::new(),
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
) -> Option<Batch> {
    let tree = &parsed.tree;
    let helpers = local_helpers(parsed);
    let initially_refused = parsed.parse_errors != 0
        || found.setup_access
        || found.sites.len() > 2000
        || !found.loads.is_empty()
        || !found.escapes.is_empty()
        || found.sites.iter().any(|site| {
            !matches!(
                site.kind,
                SiteKind::Registers(Registers::Test | Registers::Suite) | SiteKind::Wrapper(_)
            )
        });
    if initially_refused {
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
        registrations: HashSet::new(),
        invocation: Vec::new(),
        preparing: false,
        mutation_sites: HashMap::new(),
        local_templates: HashSet::new(),
    };
    let mut checked_bindings = HashSet::new();
    let mut result_sites = HashSet::new();
    let mut returned_bindings = Vec::new();
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
                suite_bodies: &mut Vec<Vec<NodeId>>,
                result_sites: &mut HashSet<NodeId>,
                returned_bindings: &mut Vec<(ReturnedBinding, bool)>|
     -> Option<()> {
        for &statement in statements {
            if tree.kind(statement) == "ImportDeclaration" {
                continue;
            }
            if let Some(binding) = returned_result_binding(parsed, statement, found.direct_eval) {
                let invalid_calls = binding.calls.iter().any(|member| {
                    tree.span(member.call).start < tree.span(statement).end
                        || !statements.iter().any(|&candidate| {
                            tree.kind(candidate) == "ExpressionStatement"
                                && tree
                                    .child(candidate, "expression")
                                    .is_some_and(|expression| {
                                        tree.inner(expression) == tree.inner(member.call)
                                    })
                        })
                });
                if invalid_calls {
                    return None;
                }
                result_sites.insert(binding.factory_call);
                result_sites.extend(binding.calls.iter().map(|member| member.call));
                returned_bindings.push((binding, !top_level));
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
        &mut result_sites,
        &mut returned_bindings,
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
            &mut result_sites,
            &mut returned_bindings,
        )?;
    }
    planner.registrations = registrations.clone();
    planner.preparing = true;
    for (binding, _) in &returned_bindings {
        planner.returned(
            Module {
                path,
                source,
                parsed,
            },
            binding,
            run,
            8,
        )?;
    }
    planner.preparing = false;
    // A site inside a helper is its template; it registers only when an admitted call runs it.
    // Which of its statements run is the template reader's to prove when a call reaches it. A
    // labeled site anywhere else (a loop, a branch, a non-suite callback) is no closed position:
    // the file keeps today's exclusions.
    let mut templates = planner.local_templates.clone();
    for site in &found.sites {
        if site.labels.is_empty()
            || stated.contains_key(&site.node)
            || result_sites.contains(&site.node)
            || templates.contains(&site.node)
        {
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
                && !result_sites.contains(&site.node)
                && site
                    .labels
                    .iter()
                    .all(|label| matches!(label, crate::inventory::Label::RegistrationBody))
                && stated.get(&site.node).is_some_and(|statement| {
                    top_level_statements.contains(statement) || suite_statements.contains(statement)
                })
        })
        .collect();
    let returned_calls = returned_bindings
        .iter()
        .map(|(binding, _)| binding.calls.len())
        .sum::<usize>();
    if (calls.is_empty() && returned_bindings.is_empty())
        || calls.len() > 2000
        || returned_calls > 2000
    {
        return None;
    }
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
    for (binding, in_suite) in &returned_bindings {
        let (symbol, invocations) = planner.returned(
            Module {
                path,
                source,
                parsed,
            },
            binding,
            run,
            8,
        )?;
        checked_bindings.insert(symbol);
        expansions.push(Expansion {
            scheduling_safe: true,
            call: binding.factory_call,
            tests: Vec::new(),
            sources: Vec::new(),
            hooks: Vec::new(),
            diagnostic_hooks: Vec::new(),
        });
        for (call, invocation) in invocations {
            if *in_suite && !invocation.hooks.is_empty() {
                return None;
            }
            expansions.push(Expansion {
                scheduling_safe: invocation.scheduling_safe,
                call,
                tests: invocation.tests,
                sources: Vec::new(),
                hooks: invocation
                    .hooks
                    .into_iter()
                    .map(|hook| ImportedHook { call, hook })
                    .collect(),
                diagnostic_hooks: invocation.diagnostic_hooks,
            });
        }
    }
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
        .chain(
            returned_bindings
                .iter()
                .filter_map(|(binding, _)| tree.child(binding.factory_call, "callee")),
        )
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
        .chain(
            returned_bindings
                .iter()
                .filter(|(_, in_suite)| !in_suite)
                .map(|(binding, _)| tree.span(binding.factory_call).start),
        )
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
    expansions.sort_by_key(|expansion| tree.span(expansion.call).start);
    let mutation_sites = planner.mutation_sites.remove(path).unwrap_or_default();
    if let Some(last) = expansions.last_mut() {
        last.sources = planner.sources.into_iter().collect();
    }
    Some(Batch {
        calls: expansions,
        refused: HashSet::new(),
        templates,
        mutation_sites,
    })
}

/// The module-body statement holding `node`.
fn top_level_statement(tree: &crate::lang::Tree, node: NodeId) -> Option<NodeId> {
    std::iter::once(node)
        .chain(tree.ancestors(node))
        .find(|&ancestor| tree.parent(ancestor) == Some(tree.root()))
}

/// One call through a statically selected returned-object path.
struct ReturnedMemberCall {
    call: NodeId,
    path: Vec<String>,
}

/// One `const` factory result and every member invocation using it.
struct ReturnedBinding {
    factory_call: NodeId,
    calls: Vec<ReturnedMemberCall>,
}

/// A `const` binding whose initializer is a factory call and whose results do not escape static
/// member-call chains.
fn returned_result_binding(
    parsed: &Parsed,
    statement: NodeId,
    direct_eval: bool,
) -> Option<ReturnedBinding> {
    if direct_eval {
        return None;
    }
    let tree = &parsed.tree;
    if tree.kind(statement) != "VariableDeclaration"
        || tree.text(statement, "kind") != Some("const")
    {
        return None;
    }
    let declarations = tree.list(statement, "declarations");
    let [Some(declarator)] = declarations.as_slice() else {
        return None;
    };
    let pattern = tree.child(*declarator, "id")?;
    let init = tree.inner(tree.child(*declarator, "init")?);
    if tree.kind(init) != "CallExpression"
        || tree
            .child(init, "callee")
            .map(|callee| tree.kind(tree.inner(callee)))
            != Some("Identifier")
    {
        return None;
    }
    let bindings: Vec<(SymbolId, Vec<String>)> = match tree.kind(pattern) {
        "Identifier" => vec![(
            parsed.scopes.symbol_at(tree.span(pattern).start)?,
            Vec::new(),
        )],
        "ObjectPattern" => tree
            .list(pattern, "properties")
            .into_iter()
            .map(|property| {
                let property = property?;
                if tree.kind(property) != "Property"
                    || tree.flag(property, "computed")
                    || tree.text(property, "kind") != Some("init")
                    || crate::lang::property_name(tree, property) == Some("__proto__")
                {
                    return None;
                }
                let name = crate::lang::property_name(tree, property)?.to_owned();
                let value = tree.child(property, "value")?;
                (tree.kind(value) == "Identifier")
                    .then(|| {
                        parsed
                            .scopes
                            .symbol_at(tree.span(value).start)
                            .map(|symbol| (symbol, vec![name]))
                    })
                    .flatten()
            })
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    if bindings.is_empty()
        || bindings.iter().any(|(symbol, _)| {
            parsed
                .scopes
                .symbol(*symbol)
                .references
                .iter()
                .any(|reference| {
                    reference.write
                        || reference
                            .node
                            .is_none_or(|node| static_member_call(tree, node).is_none())
                })
        })
    {
        return None;
    }
    let calls = bindings
        .into_iter()
        .flat_map(|(symbol, prefix)| {
            parsed
                .scopes
                .symbol(symbol)
                .references
                .iter()
                .map(move |reference| {
                    let (call, suffix) = static_member_call(tree, reference.node?)?;
                    let mut path = prefix.clone();
                    path.extend(suffix);
                    Some(ReturnedMemberCall { call, path })
                })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(ReturnedBinding {
        factory_call: init,
        calls,
    })
}

/// A binding read is the base of at least one non-computed member and the complete chain is called.
fn static_member_call(tree: &crate::lang::Tree, node: NodeId) -> Option<(NodeId, Vec<String>)> {
    let mut value = node;
    let mut path = Vec::new();
    loop {
        let parent = tree.outer_parent(value)?;
        if tree.kind(parent) == "MemberExpression"
            && !tree.flag(parent, "computed")
            && tree
                .child(parent, "object")
                .is_some_and(|object| tree.inner(object) == tree.inner(value))
        {
            path.push(crate::lang::member_name(tree, parent)?.to_owned());
            value = parent;
            continue;
        }
        return (!path.is_empty()
            && tree.kind(parent) == "CallExpression"
            && tree
                .child(parent, "callee")
                .is_some_and(|callee| tree.inner(callee) == tree.inner(value)))
        .then_some((parent, path));
    }
}

/// Resolve one path through the proved ordinary-property object tree.
fn returned_leaf(parsed: &Parsed, object: NodeId, path: &[String]) -> Option<NodeId> {
    let tree = &parsed.tree;
    let mut value = object;
    for (index, name) in path.iter().enumerate() {
        if tree.kind(value) != "ObjectExpression" {
            return None;
        }
        let property = tree
            .list(value, "properties")
            .into_iter()
            .flatten()
            .find(|&property| crate::lang::property_name(tree, property) == Some(name.as_str()))?;
        value = tree.inner(tree.child(property, "value")?);
        if index + 1 != path.len() && tree.kind(value) != "ObjectExpression" {
            return None;
        }
    }
    tree.is_function(value).then_some(value)
}

/// Resolve one immutable same-module callable or const alias.
fn local_callable(parsed: &Parsed, identifier: NodeId, depth: usize) -> Option<NodeId> {
    if depth == 0 {
        return None;
    }
    inventory::runners::callable(parsed, identifier, None).ok()
}

/// Resolve one immutable first-party import, following only plain `const` identifier aliases.
fn imported_callable(
    parsed: &Parsed,
    identifier: NodeId,
    depth: usize,
) -> Option<(NodeId, Imported)> {
    if depth == 0 {
        return None;
    }
    let tree = &parsed.tree;
    let symbol = parsed.scopes.symbol_at(tree.span(identifier).start)?;
    let binding = parsed.scopes.symbol(symbol);
    if binding.references.iter().any(|reference| reference.write) {
        return None;
    }
    if matches!(binding.declaration, Declaration::Variable(_)) {
        inventory::runners::immutable(parsed, symbol).ok()?;
    }
    match &binding.declaration {
        Declaration::Import {
            import, imported, ..
        } => Some((*import, imported.clone())),
        Declaration::Variable(declarator)
            if tree
                .parent(*declarator)
                .and_then(|parent| tree.text(parent, "kind"))
                == Some("const") =>
        {
            let init = tree.inner(tree.child(*declarator, "init")?);
            let next = parsed.scopes.symbol_at(tree.span(init).start)?;
            if let Declaration::Variable(next_declaration) = parsed.scopes.symbol(next).declaration
            {
                if tree.span(next_declaration).start >= tree.span(*declarator).start {
                    return None;
                }
            }
            (tree.kind(init) == "Identifier")
                .then(|| imported_callable(parsed, init, depth - 1))
                .flatten()
        }
        _ => None,
    }
}

/// The outer registration call when an identifier is the inner call in `alias(test)(...)`.
fn transformation_reference(tree: &crate::lang::Tree, identifier: NodeId) -> Option<NodeId> {
    let transform = tree.parent(identifier)?;
    if tree.kind(transform) != "CallExpression"
        || !tree
            .child(transform, "callee")
            .is_some_and(|callee| tree.inner(callee) == tree.inner(identifier))
    {
        return None;
    }
    tree.parent(transform).filter(|registration| {
        tree.kind(*registration) == "CallExpression"
            && tree
                .child(*registration, "callee")
                .is_some_and(|callee| tree.inner(callee) == tree.inner(transform))
    })
}

/// The sole expression returned by a transformation helper.
fn returned_expression(tree: &crate::lang::Tree, function: NodeId) -> Option<NodeId> {
    let body = tree.inner(tree.child(function, "body")?);
    if tree.kind(body) != "BlockStatement" {
        return Some(body);
    }
    let statements = inventory::runners::runtime_statements(tree, body);
    let [statement] = statements.as_slice() else {
        return None;
    };
    (tree.kind(*statement) == "ReturnStatement")
        .then(|| tree.child(*statement, "argument"))
        .flatten()
        .map(|node| tree.inner(node))
}

/// Own-property absence still reads these inherited values from an ordinary object literal.
fn inherited_object_property(name: &str) -> bool {
    matches!(
        name,
        "constructor"
            | "__defineGetter__"
            | "__defineSetter__"
            | "hasOwnProperty"
            | "__lookupGetter__"
            | "__lookupSetter__"
            | "isPrototypeOf"
            | "propertyIsEnumerable"
            | "toString"
            | "valueOf"
            | "__proto__"
            | "toLocaleString"
    )
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

/// The caller-side half of one returned factory expansion.
struct ReturnedCalls<'a, 'b> {
    caller: Module<'a>,
    binding: &'b ReturnedBinding,
}

/// The work one expansion batch may do, in visited nodes and copied or rendered bytes.
const MAX_WORK: usize = 8 * 1024 * 1024;

/// One atomic expansion batch shares its source, work, admission and recursion bounds.
struct Planner<'a> {
    /// First pass records all derived sites before any returned invocation is normalized.
    preparing: bool,
    mutation_sites: HashMap<PathBuf, HashSet<(u32, u32)>>,
    /// Caller-owned registrations reached only through already proved returned leaves.
    local_templates: HashSet<NodeId>,
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
    /// The parser-independent inventory policy shared by all template readers.
    fn inventory_setting(&self) -> inventory::Setting<'_> {
        inventory::Setting {
            dialect: self.setting.dialect,
            globals: self.setting.globals,
            root: self.setting.root,
        }
    }

    /// Resolve an imported first-party helper without accepting framework API imports.
    fn imported_source(&self, module: Module<'_>, import: NodeId, run: &Run) -> Option<PathBuf> {
        let tree = &module.parsed.tree;
        let specifier = tree
            .child(import, "source")
            .and_then(|node| tree.text(node, "value"))?;
        if self.setting.dialect.is_api_module(specifier) {
            return None;
        }
        run.sources
            .resolve(module.path, specifier, self.setting.root)
    }
    /// Charge unique dependency sources and retain their completed module proofs.
    fn admit_dependencies(
        &mut self,
        bytes: &[(PathBuf, usize)],
        dependencies: Vec<PathBuf>,
        run: &Run,
    ) -> Option<()> {
        for (path, bytes) in bytes {
            let path = run.sources.relative(path, self.setting.root)?;
            if let Some(known) = self.read_sizes.get(&path) {
                if known != bytes {
                    return None;
                }
            } else {
                self.source_bytes = self.source_bytes.saturating_add(*bytes);
                if self.source_bytes > MAX_WORK {
                    return None;
                }
                self.read_sizes.insert(path, *bytes);
            }
        }
        self.checked.extend(dependencies);
        Some(())
    }
    /// Refuse work before a duplicated argument or a rendered frame can amplify it further.
    fn charge(&mut self, cost: usize) -> Option<()> {
        self.work = self.work.saturating_add(cost);
        (self.work <= MAX_WORK).then_some(())
    }

    /// The work the batch may still do.
    fn left(&self) -> usize {
        MAX_WORK.saturating_sub(self.work)
    }

    /// Reserve a complete inventory's owned storage before cloning it.
    fn inventory_copy_work(&mut self, found: &Inventory) -> Option<()> {
        self.charge(std::mem::size_of::<Inventory>())?;
        for cost in [
            found
                .sites
                .len()
                .saturating_mul(std::mem::size_of::<inventory::Site>()),
            found.loads.len().saturating_mul(std::mem::size_of::<u32>()),
            found
                .escapes
                .len()
                .saturating_mul(std::mem::size_of::<inventory::Escape>()),
            found
                .in_place_mocks
                .len()
                .saturating_mul(std::mem::size_of::<inventory::InPlaceMock>()),
            found
                .registration_values
                .capacity()
                .saturating_mul(std::mem::size_of::<NodeId>().saturating_add(1))
                .saturating_mul(2),
        ] {
            self.charge(cost)?;
        }
        if let Some(limit) = &found.import_limit {
            self.charge(limit.module.as_os_str().len())?;
        }
        for site in &found.sites {
            self.charge(
                site.chain
                    .len()
                    .saturating_mul(std::mem::size_of::<String>())
                    .saturating_add(
                        site.labels
                            .len()
                            .saturating_mul(std::mem::size_of::<inventory::Label>()),
                    )
                    .saturating_add(site.title.as_ref().map_or(0, String::len)),
            )?;
            for member in &site.chain {
                self.charge(member.len())?;
            }
        }
        for escape in &found.escapes {
            self.charge(escape.text.len())?;
        }
        for mock in &found.in_place_mocks {
            self.charge(mock.callee.len())?;
        }
        Some(())
    }

    /// Reserve facts copied by an invocation frame or its private normalization consumer.
    fn frame_facts_copy_work(&mut self, frame: &Frame) -> Option<()> {
        if let Some(found) = &frame.inventory {
            self.inventory_copy_work(found)?;
        }
        self.derived_copy_work(&frame.derived)
    }

    /// Reuse checked parent facts only after reserving their private invocation copy.
    fn invocation_inventory(
        &mut self,
        module: Module<'_>,
        parent: Option<&Frame>,
        run: &mut Run,
    ) -> Option<Inventory> {
        match parent.and_then(|frame| frame.inventory.as_ref()) {
            Some(found) => {
                self.inventory_copy_work(found)?;
                Some((**found).clone())
            }
            None => self.inventory(module, run),
        }
    }

    /// Reserve owned modifier storage before copying a derived map or its consumer values.
    fn derived_copy_work(&mut self, derived: &HashMap<NodeId, normal::Modifiers>) -> Option<()> {
        for modifiers in derived.values() {
            // Transformation proofs produce scalar scheduling chains, never table payloads.
            if modifiers.rows.is_some() {
                return None;
            }
            self.charge(
                std::mem::size_of::<(NodeId, normal::Modifiers)>()
                    .saturating_mul(2)
                    .saturating_add(modifiers.chain.len())
                    .saturating_add(modifiers.untabled.len()),
            )?;
        }
        Some(())
    }

    /// Reserve a cached leaf's vectors, parameter paths and modifiers before cloning it.
    fn leaf_copy_work(
        &mut self,
        plan: &inventory::runners::Plan,
        derived: &HashMap<NodeId, normal::Modifiers>,
    ) -> Option<()> {
        self.charge(
            plan.steps
                .len()
                .saturating_mul(std::mem::size_of::<inventory::runners::Step>())
                .saturating_add(
                    plan.parameters
                        .len()
                        .saturating_mul(std::mem::size_of::<inventory::runners::Parameter>()),
                ),
        )?;
        for parameter in &plan.parameters {
            self.charge(
                parameter
                    .bindings
                    .len()
                    .saturating_mul(std::mem::size_of::<inventory::runners::ParameterBinding>()),
            )?;
            for binding in &parameter.bindings {
                self.charge(
                    binding
                        .path
                        .len()
                        .saturating_mul(std::mem::size_of::<String>()),
                )?;
                for key in &binding.path {
                    self.charge(key.len())?;
                }
            }
        }
        self.derived_copy_work(derived)
    }

    /// Charge inherited storage and rendered copies before cloning or normalizing a function.
    fn inherited_work(
        &mut self,
        parsed: &Parsed,
        function: NodeId,
        bindings: &HashMap<SymbolId, normal::InvocationValue>,
    ) -> Option<()> {
        let span = parsed.tree.span(function);
        let nodes = parsed.tree.descendants(function).count().saturating_add(1);
        self.charge(nodes)?;
        for (&symbol, binding) in bindings {
            let references = &parsed.scopes.symbol(symbol).references;
            self.charge(references.len().saturating_add(1))?;
            let used = references
                .iter()
                .any(|reference| span.contains(reference.span));
            self.charge(
                binding
                    .form
                    .len()
                    .saturating_add(binding.shape.len())
                    .saturating_mul(if used { nodes } else { 1 }),
            )?;
        }
        Some(())
    }

    /// Prove a caller's factory invocation, resolve every selected leaf and instantiate it.
    fn returned(
        &mut self,
        caller: Module<'_>,
        binding: &ReturnedBinding,
        run: &mut Run,
        depth: usize,
    ) -> Option<(SymbolId, Vec<(NodeId, Invocation)>)> {
        if depth == 0 {
            return None;
        }
        let parsed = caller.parsed;
        let tree = &parsed.tree;
        self.charge(tree.nodes().count())?;
        let callee = tree.inner(tree.child(binding.factory_call, "callee")?);
        if tree.kind(callee) != "Identifier" {
            return None;
        }
        let symbol = parsed.scopes.symbol_at(tree.span(callee).start)?;
        let symbol_binding = parsed.scopes.symbol(symbol);
        if symbol_binding
            .references
            .iter()
            .any(|reference| reference.write)
        {
            return None;
        }
        let actual = self.arguments(caller, binding.factory_call, &HashMap::new())?;
        let invocations = if let Declaration::Import {
            import,
            ref imported,
            ..
        } = symbol_binding.declaration
        {
            let name = match imported {
                Imported::Named(name) => name.as_str(),
                Imported::Default => "default",
                Imported::Namespace => return None,
            };
            let helper = self.imported_source(caller, import, run)?;
            let template = inventory::runners::factory_template(
                &run.sources,
                &helper,
                name,
                self.inventory_setting(),
                self.left(),
            )
            .ok()?;
            let depth = depth.checked_sub(template.resolution_depth)?;
            if !template
                .parsed
                .tree
                .span(template.function)
                .contains(template.parsed.tree.span(template.object))
            {
                return None;
            }
            self.charge(template.parsed.tree.descendants(template.object).count() + 1)?;
            self.admit_dependencies(&template.dependency_bytes, template.dependencies, run)?;
            let plan = inventory::runners::ReturnedPlan {
                function: template.function,
                parameters: template.parameters,
                object: template.object,
                setup: template.setup,
                work: template.work,
            };
            self.execute_returned(
                Module {
                    path: &template.path,
                    source: &template.source,
                    parsed: &template.parsed,
                },
                plan,
                actual,
                ReturnedCalls { caller, binding },
                run,
                depth,
            )?
        } else {
            let found = self.inventory(caller, run)?;
            let plan =
                inventory::runners::returned_local(parsed, &found, callee, self.left()).ok()?;
            if !tree.span(plan.function).contains(tree.span(plan.object)) {
                return None;
            }
            self.charge(tree.descendants(plan.object).count() + 1)?;
            self.execute_returned(
                caller,
                plan,
                actual,
                ReturnedCalls { caller, binding },
                run,
                depth - 1,
            )?
        };
        Some((symbol, invocations))
    }

    /// Read one call's literal arguments under already proved invocation values.
    fn arguments(
        &mut self,
        module: Module<'_>,
        call: NodeId,
        values: &HashMap<SymbolId, Value>,
    ) -> Option<Vec<Value>> {
        let arguments = module.parsed.tree.list(call, "arguments");
        if arguments.len() > 2000 {
            return None;
        }
        arguments
            .into_iter()
            .map(|argument| {
                let (value, spent) =
                    normal::titles::value_within(module.parsed, argument?, values, self.left())?;
                self.charge(spent)?;
                self.charge(serde_json::to_vec(&value).ok()?.len())?;
                Some(value)
            })
            .collect()
    }

    /// Bind the factory frame once, then run each selected returned closure with its own arguments.
    fn execute_returned(
        &mut self,
        factory: Module<'_>,
        plan: inventory::runners::ReturnedPlan,
        actual: Vec<Value>,
        calls: ReturnedCalls<'_, '_>,
        run: &mut Run,
        depth: usize,
    ) -> Option<Vec<(NodeId, Invocation)>> {
        self.charge(plan.work)?;
        let mut frame = self.frame(factory, plan.function, &plan.parameters, &actual, None)?;
        self.bind_factory_setup(factory, &plan.setup, &mut frame)?;
        let found = self.inventory(factory, run)?;
        self.inventory_copy_work(&found)?;
        frame.inventory = Some(Box::new(found.clone()));
        if calls.binding.calls.len() > 2000 {
            return None;
        }
        let mut plans: HashMap<
            NodeId,
            (inventory::runners::Plan, HashMap<NodeId, normal::Modifiers>),
        > = HashMap::new();
        let mut prepared = Vec::new();
        for member in &calls.binding.calls {
            self.charge(member.path.len().saturating_add(1))?;
            let leaf = returned_leaf(factory.parsed, plan.object, &member.path)?;
            let (leaf_plan, derived) = if let Some(plan) = plans.get(&leaf) {
                self.leaf_copy_work(&plan.0, &plan.1)?;
                plan.clone()
            } else {
                let leaf_plan =
                    self.returned_leaf_plan(factory, &found, leaf, &frame, run, depth)?;
                self.captured_values(factory.parsed, plan.function, leaf, &frame, &leaf_plan.1)?;
                self.leaf_copy_work(&leaf_plan.0, &leaf_plan.1)?;
                plans.insert(leaf, leaf_plan.clone());
                leaf_plan
            };
            let actual = self.arguments(calls.caller, member.call, &HashMap::new())?;
            prepared.push((member.call, leaf_plan, actual, derived));
        }
        self.derived_copy_work(&frame.derived)?;
        let mut all_derived = frame.derived.clone();
        for (_, _, _, derived) in &prepared {
            self.derived_copy_work(derived)?;
            all_derived.extend(derived.clone());
        }
        let mutation_sites = self
            .mutation_sites
            .entry(factory.path.to_path_buf())
            .or_default();
        for &node in all_derived.keys() {
            let span = factory.parsed.tree.span(node);
            mutation_sites.insert((span.start, span.end));
        }
        frame.mutation_sites.clone_from(mutation_sites);
        if self.preparing {
            if factory.path == calls.caller.path {
                // Construction is proved inert and every result use is a selected, proved call.
                // Unselected returned functions remain dormant, including their nested callbacks.
                let object = factory.parsed.tree.span(plan.object);
                self.local_templates.extend(
                    found
                        .sites
                        .iter()
                        .filter(|site| object.contains(factory.parsed.tree.span(site.node)))
                        .map(|site| site.node),
                );
            }
            return Some(Vec::new());
        }
        let mut invocations = Vec::new();
        for (call, leaf_plan, actual, _) in prepared {
            self.invocation = vec![
                calls
                    .caller
                    .parsed
                    .tree
                    .span(calls.binding.factory_call)
                    .start,
                calls.caller.parsed.tree.span(call).start,
            ];
            self.frame_facts_copy_work(&frame)?;
            let mut parent = frame.clone();
            self.derived_copy_work(&all_derived)?;
            parent.derived.clone_from(&all_derived);
            let invocation = self.execute(factory, leaf_plan, actual, Some(&parent), run, depth)?;
            invocations.push((call, invocation));
        }
        Some(invocations)
    }

    /// Read a direct leaf template, or one whose only operations are proved derived test calls.
    fn returned_leaf_plan(
        &mut self,
        module: Module<'_>,
        found: &Inventory,
        function: NodeId,
        frame: &Frame,
        run: &mut Run,
        depth: usize,
    ) -> Option<(inventory::runners::Plan, HashMap<NodeId, normal::Modifiers>)> {
        if let Ok(plan) = inventory::runners::returned_leaf(module.parsed, found, function) {
            if plan
                .steps
                .iter()
                .all(|step| !matches!(step, inventory::runners::Step::Call(_)))
            {
                return Some((plan, HashMap::new()));
            }
        }
        let parsed = module.parsed;
        let tree = &parsed.tree;
        let parameters =
            inventory::runners::returned_parameters(parsed, function, found.direct_eval).ok()?;
        let body = tree.child(function, "body")?;
        let statements = if tree.kind(body) == "BlockStatement" {
            tree.list(body, "body").into_iter().flatten().collect()
        } else {
            vec![body]
        };
        let mut steps = Vec::new();
        let mut derived = HashMap::new();
        for statement in statements {
            if tree.erased_declaration(statement) || tree.kind(statement) == "EmptyStatement" {
                continue;
            }
            let call = if tree.kind(statement) == "ExpressionStatement" {
                tree.inner(tree.child(statement, "expression")?)
            } else {
                tree.inner(statement)
            };
            let site = found.sites.iter().find(|site| site.node == call)?;
            if !matches!(site.kind, SiteKind::Wrapper(_)) {
                return None;
            }
            let modifiers = self.derived_registration(module, found, call, frame, run, depth)?;
            steps.push(inventory::runners::Step::Registration(
                call,
                Registers::Test,
            ));
            derived.insert(call, modifiers);
        }
        (!steps.is_empty()).then_some((
            inventory::runners::Plan {
                function,
                parameters,
                steps,
            },
            derived,
        ))
    }

    /// Prove `r(test)(title, callback)` where `r` is identity/runIf/skipIf.
    fn derived_registration(
        &mut self,
        module: Module<'_>,
        found: &Inventory,
        call: NodeId,
        frame: &Frame,
        run: &mut Run,
        depth: usize,
    ) -> Option<normal::Modifiers> {
        let parsed = module.parsed;
        let tree = &parsed.tree;
        let arguments = tree.list(call, "arguments");
        if arguments.len() != 2
            || !arguments[1].is_some_and(|callback| tree.is_function(tree.inner(callback)))
        {
            return None;
        }
        let transform_call = tree.inner(tree.child(call, "callee")?);
        if tree.kind(transform_call) != "CallExpression" {
            return None;
        }
        let supplied = tree.list(transform_call, "arguments");
        let [Some(api)] = supplied.as_slice() else {
            return None;
        };
        let api = tree.inner(*api);
        if tree.kind(api) != "Identifier" {
            return None;
        }
        let api_symbol = parsed.scopes.symbol_at(tree.span(api).start)?;
        let Declaration::Import {
            import,
            ref imported,
            ..
        } = parsed.scopes.symbol(api_symbol).declaration
        else {
            return None;
        };
        let specifier = tree
            .child(import, "source")
            .and_then(|source| tree.text(source, "value"))?;
        let imported = match imported {
            Imported::Named(name) => name.as_str(),
            Imported::Default | Imported::Namespace => return None,
        };
        if !self.setting.dialect.is_api_module(specifier)
            || self.setting.dialect.registration(imported)?.registers != Registers::Test
        {
            return None;
        }
        let transform = tree.inner(tree.child(transform_call, "callee")?);
        if tree.kind(transform) != "Identifier" {
            return None;
        }
        if let Some(function) = local_callable(parsed, transform, 8) {
            return self.transformation_modifiers(
                module,
                found,
                function,
                frame.function,
                frame.bindings.clone(),
                frame.values.clone(),
            );
        }
        let (import, imported) = imported_callable(parsed, transform, 8)?;
        let name = match imported {
            Imported::Named(name) => name,
            Imported::Default => "default".to_owned(),
            Imported::Namespace => return None,
        };
        let helper = self.imported_source(module, import, run)?;
        let template = inventory::runners::callable_template(
            &run.sources,
            &helper,
            &name,
            self.inventory_setting(),
        )
        .ok()?;
        depth.checked_sub(template.resolution_depth)?;
        self.admit_dependencies(&template.dependency_bytes, template.dependencies, run)?;
        let helper = Module {
            path: &template.path,
            source: &template.source,
            parsed: &template.parsed,
        };
        let found = self.inventory(helper, run)?;
        self.transformation_modifiers(
            helper,
            &found,
            template.function,
            template.function,
            HashMap::new(),
            HashMap::new(),
        )
    }

    /// Interpret one already linked transformation helper under its own lexical bindings.
    fn transformation_modifiers(
        &mut self,
        module: Module<'_>,
        found: &Inventory,
        function: NodeId,
        binding_function: NodeId,
        bindings: HashMap<SymbolId, normal::InvocationValue>,
        values: HashMap<SymbolId, Value>,
    ) -> Option<normal::Modifiers> {
        let parsed = module.parsed;
        let tree = &parsed.tree;
        if tree.flag(function, "async")
            || tree.flag(function, "generator")
            || tree
                .child(function, "body")
                .is_some_and(|body| crate::lang::uses_receiver(tree, &parsed.scopes, body))
        {
            return None;
        }
        self.inherited_work(parsed, function, &bindings)?;
        let params = tree.list(function, "params");
        let [Some(parameter)] = params.as_slice() else {
            return None;
        };
        if tree.kind(*parameter) != "Identifier" {
            return None;
        }
        let parameter = parsed.scopes.symbol_at(tree.span(*parameter).start)?;
        let returned = returned_expression(tree, function)?;
        let mut modifiers = normal::Modifiers::default();
        let exports = normal::Exports::default();
        let structure = Structure::of(parsed, found);
        let mut normalizer = Normalizer::new(
            module.path,
            parsed,
            normal::Setting {
                dialect: self.setting.dialect,
                root: self.setting.root,
                exports: &exports,
            },
            &structure,
        )
        .for_registration(binding_function, bindings);
        let chain = if tree.kind(returned) == "Identifier"
            && parsed.scopes.symbol_at(tree.span(returned).start) == Some(parameter)
        {
            "api:test".to_owned()
        } else {
            let returned = tree.inner(returned);
            if tree.kind(returned) != "CallExpression" {
                return None;
            }
            let callee = tree.inner(tree.child(returned, "callee")?);
            if tree.kind(callee) != "MemberExpression" || tree.flag(callee, "computed") {
                return None;
            }
            let object = tree.inner(tree.child(callee, "object")?);
            if tree.kind(object) != "Identifier"
                || parsed.scopes.symbol_at(tree.span(object).start) != Some(parameter)
            {
                return None;
            }
            let name = crate::lang::member_name(tree, callee)?;
            let enabling = match self.setting.dialect.chain_member(name) {
                Some(crate::framework::Chain::RunWhen) => true,
                Some(crate::framework::Chain::SkipWhen) => false,
                _ => return None,
            };
            let conditions = tree.list(returned, "arguments");
            let [Some(condition)] = conditions.as_slice() else {
                return None;
            };
            let (value, spent) =
                normal::titles::value_within(parsed, *condition, &values, self.left())?;
            self.charge(spent)?;
            let truthy = match value {
                serde_json::Value::Null => false,
                serde_json::Value::Bool(value) => value,
                serde_json::Value::Number(value) => value.as_f64()? != 0.0,
                serde_json::Value::String(value) => !value.is_empty(),
                serde_json::Value::Array(_) | serde_json::Value::Object(_) => true,
            };
            modifiers.inert = truthy != enabling;
            format!("api:test.{name}.({})", normalizer.of(*condition))
        };
        self.charge(chain.len().saturating_mul(2))?;
        modifiers.untabled = chain.clone();
        modifiers.chain = chain;
        Some(modifiers)
    }

    /// Bind inert scalar/object locals evaluated once by the factory call.
    fn bind_factory_setup(
        &mut self,
        module: Module<'_>,
        setup: &[NodeId],
        frame: &mut Frame,
    ) -> Option<()> {
        let tree = &module.parsed.tree;
        for &statement in setup {
            if tree.kind(statement) != "VariableDeclaration" {
                continue;
            }
            for declarator in tree.list(statement, "declarations").into_iter().flatten() {
                let id = tree.child(declarator, "id")?;
                if tree.kind(id) != "Identifier" {
                    return None;
                }
                let symbol = module.parsed.scopes.symbol_at(tree.span(id).start)?;
                let Some(init) = tree.child(declarator, "init") else {
                    continue;
                };
                if let Some((value, spent)) =
                    normal::titles::value_within(module.parsed, init, &frame.values, self.left())
                {
                    self.charge(spent)?;
                    if let Some(binding) = normal::InvocationValue::scalar(&value) {
                        self.charge(binding.form.len().saturating_add(binding.shape.len()))?;
                        frame.bindings.insert(symbol, binding);
                        frame.values.insert(symbol, value);
                    }
                }
            }
        }
        Some(())
    }

    /// Every capture is bound or used only through proved registration transformations.
    fn captured_values(
        &mut self,
        parsed: &Parsed,
        factory: NodeId,
        leaf: NodeId,
        frame: &Frame,
        derived: &HashMap<NodeId, normal::Modifiers>,
    ) -> Option<()> {
        let tree = &parsed.tree;
        let factory_span = tree.span(factory);
        let leaf_span = tree.span(leaf);
        parsed.scopes.symbols().try_for_each(|symbol| {
            self.charge(1)?;
            if frame.bindings.contains_key(&symbol) {
                return Some(());
            }
            let declared = parsed.scopes.symbol(symbol);
            let declaration = parsed.scopes.declaration_node(symbol);
            if !declaration.is_some_and(|node| {
                factory_span.contains(tree.span(node)) && !leaf_span.contains(tree.span(node))
            }) {
                return Some(());
            }
            self.charge(declared.references.len().saturating_mul(2))?;
            if !declared
                .references
                .iter()
                .any(|reference| leaf_span.contains(reference.span))
            {
                return Some(());
            }
            // The derived-registration reader already proved and linked the callable.
            declared
                .references
                .iter()
                .filter(|reference| leaf_span.contains(reference.span))
                .all(|reference| {
                    reference
                        .node
                        .and_then(|node| transformation_reference(tree, node))
                        .is_some_and(|registration| derived.contains_key(&registration))
                })
                .then_some(())
        })
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
            let helper = self.imported_source(module, import, run)?;
            let template =
                inventory::runners::template(&run.sources, &helper, name, self.inventory_setting())
                    .ok()?;
            let depth = depth.checked_sub(template.resolution_depth)?;
            self.admit_dependencies(&template.dependency_bytes, template.dependencies, run)?;
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
            let found = self.invocation_inventory(module, parent, run)?;
            let plan = inventory::runners::local(parsed, &found, callee).ok()?;
            self.execute(module, plan, actual, parent, run, depth - 1)
        }
    }

    /// Construct one immutable invocation frame, extending an enclosing factory frame.
    fn frame(
        &mut self,
        module: Module<'_>,
        function: NodeId,
        parameters: &[inventory::runners::Parameter],
        actual: &[Value],
        parent: Option<&Frame>,
    ) -> Option<Frame> {
        let parsed = module.parsed;
        let tree = &parsed.tree;
        if let Some(parent) = parent {
            self.derived_copy_work(&parent.derived)?;
            self.inherited_work(parsed, function, &parent.bindings)?;
        }
        let mut frame = Frame {
            mutation_sites: self
                .mutation_sites
                .get(module.path)
                .cloned()
                .unwrap_or_default(),
            inventory: None,
            function,
            bindings: parent.map_or_else(HashMap::new, |frame| frame.bindings.clone()),
            values: parent.map_or_else(HashMap::new, |frame| frame.values.clone()),
            sites: HashSet::new(),
            registrations: HashSet::new(),
            derived: parent.map_or_else(HashMap::new, |frame| frame.derived.clone()),
        };
        for (index, parameter) in parameters.iter().enumerate() {
            for selected in &parameter.bindings {
                let used = !parsed.scopes.symbol(selected.symbol).references.is_empty();
                let mut value = actual.get(index);
                if value.is_none() && selected.path.is_empty() && selected.default.is_none() {
                    if used {
                        return None;
                    }
                    continue;
                }
                for key in &selected.path {
                    let object = value?.as_object()?;
                    if !object.contains_key(key) && inherited_object_property(key) {
                        return None;
                    }
                    value = object.get(key);
                }
                let value = if let Some(value) = value {
                    value.clone()
                } else {
                    let (value, spent) = normal::titles::value_within(
                        parsed,
                        selected.default?,
                        &frame.values,
                        self.left(),
                    )?;
                    self.charge(spent)?;
                    value
                };
                if !used {
                    continue;
                }
                let binding = normal::InvocationValue::scalar(&value)?;
                self.charge(
                    binding
                        .form
                        .len()
                        .saturating_add(binding.shape.len())
                        .saturating_mul(tree.nodes().count()),
                )?;
                frame.bindings.insert(selected.symbol, binding);
                frame.values.insert(selected.symbol, value);
            }
        }
        Some(frame)
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
            let mut found = self.invocation_inventory(module, parent, run)?;
            if let Some(known) = self.sources.get(&relative) {
                if known != module.source {
                    return None;
                }
            } else {
                self.sources
                    .insert(relative.clone(), module.source.to_owned());
            }
            let mut frame = self.frame(module, plan.function, &plan.parameters, &actual, parent)?;
            frame.promote_inventory(parsed, &mut found);
            self.inventory_copy_work(&found)?;
            frame.inventory = Some(Box::new(found.clone()));
            frame.registrations = if relative == self.owner {
                self.registrations.clone()
            } else {
                HashSet::new()
            };
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
                self.frame_facts_copy_work(&frame)?;
                // read_parsed privately copies the inventory and selected modifiers once more.
                self.frame_facts_copy_work(&frame)?;
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
                self.frame_facts_copy_work(&frame)?;
                self.frame_facts_copy_work(&frame)?;
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
            self.inventory_setting(),
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
