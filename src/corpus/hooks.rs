//! Owned hook declarations and distinct, proven lifecycle chains.

use super::TestCase;
use crate::framework::{HookTiming, Registers};
use crate::inventory::{Inventory, Label, Site, SiteKind};
use crate::lang::{Lines, NodeId, Parsed};
use crate::model::{Position, Span};
use crate::normal::{Normalizer, Structure};
use std::collections::{BTreeSet, HashMap};

/// One hook declaration, retained even when its body cannot license a redundancy claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hook {
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

/// Collects chains only for ordinary, comparable tests under scopes closed to unknown registration.
pub(super) fn collect(
    parsed: &Parsed,
    found: &Inventory,
    structure: &Structure,
    tests: &[TestCase],
    normalizer: &mut Normalizer<'_>,
    dialect: &crate::framework::Dialect,
    source: &str,
) -> Hooks {
    let mut result = Hooks::default();
    if found
        .sites
        .iter()
        .filter(|site| matches!(site.kind, SiteKind::Registers(Registers::Hook(_))))
        .count()
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
    if std::iter::once(None)
        .chain(structure.suites.keys().copied().map(Some))
        .any(|scope| !closed_scope(scope, parsed, &sites, normalizer))
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
    let mut declarations: HashMap<Option<NodeId>, Vec<usize>> = HashMap::new();
    for site in &found.sites {
        let SiteKind::Registers(Registers::Hook(timing)) = site.kind else {
            continue;
        };
        let Some(&scope) = scopes.get(&site.node) else {
            continue;
        };
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
            .push(result.declarations.len() - 1);
    }
    let eligible: BTreeSet<_> = tests
        .iter()
        .filter(|test| !test.inert && test.exclusion.is_none())
        .map(|test| test.span.start_byte)
        .collect();
    let mut chains = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for site in found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Test))
    {
        if !site.chain.is_empty() || uncertain(site) || !eligible.contains(&site.span.start) {
            continue;
        }
        let Some(around) = normalizer.suites_around(site.node) else {
            continue;
        };
        let path: Vec<_> = std::iter::once(None)
            .chain(around.iter().copied().map(Some))
            .collect();
        if !paths.insert(path.clone()) {
            continue;
        }
        let mut chain = Vec::new();
        for scope in &path {
            chain.extend(declarations.get(scope).into_iter().flatten().copied());
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
) -> bool {
    let tree = &parsed.tree;
    let statements = match scope {
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
        if !site.chain.is_empty() || uncertain(site) || matches!(site.kind, SiteKind::Wrapper(_)) {
            return false;
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
