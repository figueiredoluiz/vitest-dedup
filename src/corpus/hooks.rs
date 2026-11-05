//! Owned hook declarations and distinct, proven lifecycle chains.

use super::TestCase;
use crate::framework::{HookTiming, Registers};
use crate::inventory::{Inventory, Label, Site, SiteKind};
use crate::lang::{Lines, NodeId, Parsed};
use crate::model::{Position, Span};
use crate::normal::{Normalizer, Structure};
use std::collections::{BTreeSet, HashMap, HashSet};

/// One hook declaration, retained even when its body cannot license a redundancy claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hook {
    /// The helper file that owns this declaration, when it was registered through a proven runner.
    /// Local declarations use the surrounding corpus file's path.
    pub path: Option<std::path::PathBuf>,
    /// Caller files whose registrations cause this hook to execute. Local hooks have none.
    pub registration_files: Vec<std::path::PathBuf>,
    /// The registering call, in report coordinates.
    pub span: Span,
    /// Its lifecycle phase.
    pub timing: HookTiming,
    /// The adapter's name for this phase.
    pub label: &'static str,
    /// The containing suite callback's start, or the file scope.
    pub scope: Option<u32>,
    /// Proven nonempty idempotent effects; no proof remains an intervening barrier.
    pub key: Option<String>,
}

/// One file's hook population; shared chains are stored once rather than copied per test or row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hooks {
    /// Declarations in source order.
    pub declarations: Vec<Hook>,
    /// Indices into declarations, in outer-to-inner registration order.
    pub chains: Vec<Vec<usize>>,
}

/// What a suite table's rows normalization proves about its callback's population. Only a bare
/// table callee (`describe.each`/`describe.for`) reaches the proof: any other chained modifier
/// keeps the refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableRows {
    /// Proven zero rows: the callback never runs, so nothing inside it registers.
    Empty,
    /// Proven at least one row: the callback runs once per row as an ordinary suite invocation.
    Populated,
    /// Unread rows, a chained modifier, or not a table: the population is unknown.
    Unproven,
}

/// A suite table's rows proof, by its registering call. Absent: not a table, or a chained shape
/// this proof does not reach.
fn table_rows(
    found: &Inventory,
    structure: &Structure,
    parsed: &Parsed,
    normalizer: &mut Normalizer<'_>,
) -> (HashMap<NodeId, TableRows>, HashSet<NodeId>) {
    let tree = &parsed.tree;
    let members: HashMap<NodeId, NodeId> = found
        .sites
        .iter()
        .filter_map(|site| Some((site.node, site.member?)))
        .collect();
    let mut tables = HashMap::new();
    for (site, callee) in found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Suite))
        .filter_map(|site| Some((site, tree.child(site.node, "callee")?)))
    {
        let modifiers = normalizer.modifiers(callee, members.get(&site.node).copied());
        // `untabled` holds every chain step outside the table; a bare table leaves only the
        // registration's root, and an inert member (`skip`, `todo`) never runs.
        let proof = match modifiers.rows {
            Some(rows) if !modifiers.inert && modifiers.untabled.split('.').count() <= 1 => {
                if rows.is_empty() {
                    TableRows::Empty
                } else {
                    TableRows::Populated
                }
            }
            _ => TableRows::Unproven,
        };
        tables.insert(site.node, proof);
    }
    // Suite callbacks whose table proved empty never run, so nothing declared inside one, at any
    // depth, registers.
    let dead: HashSet<NodeId> = structure
        .suites
        .iter()
        .filter(|(_, &call)| tables.get(&call) == Some(&TableRows::Empty))
        .map(|(&scope, _)| scope)
        .collect();
    (tables, dead)
}

/// Collects chains only for ordinary, comparable tests under scopes closed to unknown registration.
pub(super) fn collect(
    parsed: &Parsed,
    found: &Inventory,
    structure: &Structure,
    tests: &[TestCase],
    normalizer: &mut Normalizer<'_>,
    input: (&crate::framework::Dialect, &str),
    expansions: (&[super::runners::ImportedHook], &[NodeId], &[NodeId], bool),
) -> Hooks {
    let (dialect, source) = input;
    let (imported, imported_tests, admitted_calls, scheduling_safe) = expansions;
    let mut result = Hooks::default();
    if found
        .sites
        .iter()
        .filter(|site| matches!(site.kind, SiteKind::Registers(Registers::Hook(_))))
        .count()
        + imported.len()
        < 2
        || !found.loads.is_empty()
        || found.sites.iter().any(|site| {
            site.chain.iter().any(|member| {
                matches!(
                    dialect.chain_member(member),
                    Some(crate::framework::Chain::Focus | crate::framework::Chain::Concurrent)
                )
            })
        })
    {
        return result;
    }
    let tree = &parsed.tree;
    let sites: HashMap<_, _> = found.sites.iter().map(|site| (site.node, site)).collect();
    let (tables, dead) = table_rows(found, structure, parsed, normalizer);
    if std::iter::once(None)
        .chain(structure.suites.keys().copied().map(Some))
        .any(|scope| {
            !scope.is_some_and(|scope| dead.contains(&scope))
                && !closed_scope(scope, parsed, &sites, normalizer, admitted_calls, &tables)
        })
    {
        return result;
    }
    let lines = Lines::new(source);
    let scopes: HashMap<_, _> = found
        .sites
        .iter()
        .filter_map(|site| {
            Some((
                site.node,
                normalizer.suites_around(site.node)?.last().copied(),
            ))
        })
        .collect();
    let mut declarations: HashMap<Option<NodeId>, Vec<(u32, usize)>> = HashMap::new();
    for site in &found.sites {
        let SiteKind::Registers(Registers::Hook(timing)) = site.kind else {
            continue;
        };
        let Some(&scope) = scopes.get(&site.node) else {
            continue;
        };
        // A hook under a proven-empty table never registers.
        if normalizer
            .suites_around(site.node)
            .is_some_and(|around| around.iter().any(|scope| dead.contains(scope)))
        {
            continue;
        }
        let arguments = tree.list(site.node, "arguments");
        let callback = arguments
            .first()
            .copied()
            .flatten()
            .map(|arg| tree.inner(arg));
        let key = if arguments.len() == 1 && site.chain.is_empty() {
            callback
                .filter(|&node| tree.is_function(node))
                .and_then(|node| normalizer.hook_effects(node))
        } else {
            None
        };
        let key = key.filter(|_| scheduling_safe && normalizer.hook_scheduling_safe(scope));
        let position = |offset| {
            let (line, column) = lines.position(source, offset);
            Position { line, column }
        };
        let label = dialect
            .registrations
            .iter()
            .find(|entry| entry.registers == Registers::Hook(timing))
            .expect("inventoried hook belongs to dialect")
            .canonical;
        result.declarations.push(Hook {
            path: None,
            registration_files: Vec::new(),
            span: Span {
                start: position(site.span.start),
                end: position(site.span.end),
                start_byte: site.span.start,
                end_byte: site.span.end,
            },
            timing,
            label,
            scope: scope.map(|node| tree.span(node).start),
            key,
        });
        declarations
            .entry(scope)
            .or_default()
            .push((site.span.start, result.declarations.len() - 1));
    }
    // A runner's hook executes at its proved invocation, in that invocation's caller scope. Its
    // body and source location remain the helper's, but its lifecycle position is the call site.
    for imported in imported {
        let &scope = scopes
            .get(&imported.call)
            .expect("admitted hook call has a caller scope");
        let mut hook = imported.hook.clone();
        if !scheduling_safe || !normalizer.hook_scheduling_safe(scope) {
            hook.key = None;
        }
        hook.scope = scope.map(|node| tree.span(node).start);
        result.declarations.push(hook);
        declarations.entry(scope).or_default().push((
            tree.span(imported.call).start,
            result.declarations.len() - 1,
        ));
    }
    for entries in declarations.values_mut() {
        entries.sort_by_key(|(order, _)| *order);
    }
    let eligible: BTreeSet<_> = tests
        .iter()
        .filter(|test| !test.inert && test.exclusion.is_none())
        .map(|test| test.span.start_byte)
        .collect();
    let mut chains = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let direct_tests = found
        .sites
        .iter()
        .filter(|site| {
            site.kind == SiteKind::Registers(Registers::Test)
                && site.chain.is_empty()
                && !uncertain(site)
                && eligible.contains(&site.span.start)
        })
        .map(|site| site.node);
    for call in direct_tests.chain(imported_tests.iter().copied()) {
        let around = normalizer
            .suites_around(call)
            .expect("eligible registration has a closed scope");
        let path: Vec<_> = std::iter::once(None)
            .chain(around.iter().copied().map(Some))
            .collect();
        if !paths.insert(path.clone()) {
            continue;
        }
        let mut chain = Vec::new();
        for scope in &path {
            chain.extend(
                declarations
                    .get(scope)
                    .into_iter()
                    .flatten()
                    .map(|(_, index)| *index),
            );
        }
        if chain.iter().any(|&index| {
            !matches!(
                result.declarations[index].timing,
                HookTiming::BeforeEach
                    | HookTiming::AfterEach
                    | HookTiming::BeforeAll
                    | HookTiming::AfterAll
            )
        }) {
            continue;
        }
        if chain.len() >= 2 {
            chains.insert(chain);
        }
    }
    result.chains = chains.into_iter().collect();
    result
}

/// A branch, loop or indirect callback cannot establish that two registrations both execute.
fn uncertain(site: &Site) -> bool {
    site.labels
        .iter()
        .any(|label| !matches!(label, Label::RegistrationBody))
}

/// Every evaluated statement is inert or a recorded direct registration with inert arguments.
fn closed_scope(
    scope: Option<NodeId>,
    parsed: &Parsed,
    sites: &HashMap<NodeId, &Site>,
    normalizer: &Normalizer<'_>,
    admitted_calls: &[NodeId],
    tables: &HashMap<NodeId, TableRows>,
) -> bool {
    let tree = &parsed.tree;
    let statements = match scope {
        // The caller skips proven-empty table scopes: their callbacks never run, so their
        // statements close vacuously and contribute no declarations.
        Some(scope) => {
            if tree
                .child(scope, "body")
                .is_none_or(|body| tree.kind(body) != "BlockStatement")
            {
                return false;
            }
            normalizer.body_statements(scope)
        }
        None => tree
            .list(tree.root(), "body")
            .into_iter()
            .flatten()
            .collect(),
    };
    statements.into_iter().all(|statement| {
        if tree.kind(statement) == "ImportDeclaration"
            || normalizer.hook_registration_inert(statement)
        {
            return true;
        }
        let Some(call) = tree
            .child(statement, "expression")
            .map(|node| tree.inner(node))
        else {
            return false;
        };
        let Some(site) = sites.get(&call) else {
            return false;
        };
        if !site.chain.is_empty() {
            // Only a suite table whose rows normalization proved its population closes: an empty
            // proof runs the callback zero times, a populated one runs it once per row as an
            // ordinary suite invocation. The rows argument still evaluates at registration, so
            // only an inert one can hide no registration, and any other chained callee keeps the
            // refusal.
            let rows = tree
                .child(call, "callee")
                .and_then(|callee| tree.list(callee, "arguments").into_iter().next().flatten());
            return matches!(
                tables.get(&site.node),
                Some(TableRows::Empty | TableRows::Populated)
            ) && !uncertain(site)
                && rows.is_some_and(|rows| normalizer.hook_registration_inert(rows));
        }
        if matches!(site.kind, SiteKind::Wrapper(_)) {
            return admitted_calls.contains(&site.node);
        }
        let arguments = tree.list(call, "arguments");
        !arguments.is_empty()
            && arguments.into_iter().all(|arg| {
                arg.is_some_and(|arg| {
                    let arg = tree.inner(arg);
                    (tree.is_function(arg) || tree.kind(arg) == "Literal")
                        && normalizer.hook_registration_inert(arg)
                })
            })
    })
}
