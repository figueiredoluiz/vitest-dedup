//! Whole-suite identities, kept outside the executable-test population.

use super::{Setting, TestCase};
use crate::framework::{Chain, Registers};
use crate::inventory::{Inventory, Label, SiteKind};
use crate::lang::{Lines, NodeId, Parsed};
use crate::normal::{self, digest, Normalizer, Structure};
use std::collections::HashMap;

/// Keys whole inline suites only when their names, ancestry and contained tests are comparable.
pub(super) fn collect(
    parsed: &Parsed,
    found: &Inventory,
    structure: &Structure,
    contexts: &HashMap<Option<NodeId>, String>,
    tests: &[TestCase],
    normalizer: &mut Normalizer<'_>,
    setting: (Setting<'_>, &str),
) -> Vec<TestCase> {
    let (setting, source) = setting;
    let tree = &parsed.tree;
    let sites: HashMap<_, _> = found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Suite))
        .map(|site| (site.node, site))
        .collect();
    let lines = Lines::new(source);
    // Per-file invariants: with a focus anywhere in the file Vitest runs only the focused tests,
    // so unfocused suite copies do not run this file's collection as written (copies that are
    // themselves focused still run and are refused with it — over-refusal only). And a
    // module-level conditional ending before a suite leaves that suite's registration uncertain.
    let focused = found.sites.iter().any(|site| {
        site.chain
            .iter()
            .any(|member| matches!(setting.dialect.chain_member(member), Some(Chain::Focus)))
    });
    let conditional_from = tree
        .list(tree.root(), "body")
        .into_iter()
        .flatten()
        .filter(|statement| tree.kind(*statement) == "IfStatement")
        .map(|statement| tree.span(statement).end)
        .min();
    let unrun = Unrun::new(found, setting);
    let mut suites = Vec::new();
    for site in sites.values() {
        let Some(callback) = normalizer.callback(site.node).filter(|body| body.inline) else {
            continue;
        };
        let Some(body) = callback.function else {
            continue;
        };
        let Some(around) = normalizer.suites_around(site.node) else {
            continue;
        };
        let calls: Vec<_> = around
            .iter()
            .filter_map(|scope| structure.suites.get(scope).copied())
            .chain(std::iter::once(site.node))
            .collect();
        let mut names = Vec::new();
        let mut context = vec![contexts.get(&None).cloned().unwrap_or_default()];
        let mut eligible = true;
        for &call in &calls {
            let Some(owner) = sites.get(&call) else {
                eligible = false;
                break;
            };
            let Some(title) = normal::titles::title(parsed, call) else {
                eligible = false;
                break;
            };
            if owner
                .labels
                .iter()
                .any(|label| !matches!(label, Label::RegistrationBody))
                || owner.chain.iter().any(|member| {
                    matches!(
                        setting.dialect.chain_member(member),
                        Some(Chain::Skip | Chain::Todo | Chain::Table(_))
                    )
                })
                || !normal::row_identity::suite_prefix_known(parsed, call, owner.provenance)
            {
                eligible = false;
                break;
            }
            let Some(callee) = tree.child(call, "callee") else {
                eligible = false;
                break;
            };
            let Some(owner_body) = normalizer.callback(call) else {
                eligible = false;
                break;
            };
            let arguments: Vec<_> = tree.list(call, "arguments").into_iter().flatten().collect();
            if !normalizer.active_suite(callee, &arguments) {
                eligible = false;
                break;
            }
            names.push(title);
            if call != site.node {
                context.push(normalizer.of(callee));
                context.extend(
                    arguments
                        .into_iter()
                        .skip(1)
                        .filter(|&arg| arg != owner_body.argument)
                        .map(|arg| normalizer.of(arg)),
                );
            }
        }
        let children: Vec<_> = tests
            .iter()
            .filter(|test| {
                site.span.start <= test.span.start_byte && test.span.end_byte <= site.span.end
            })
            .collect();
        // Copies whose every child is skipped or a placeholder never run.
        let all_children_unrun = !children.is_empty()
            && children
                .iter()
                .all(|test| unrun.contains(test.span.start_byte, test.span.end_byte));
        let registered_conditionally = conditional_from.is_some_and(|from| {
            let mut at = site.node;
            while tree.parent(at) != Some(tree.root()) {
                at = tree.parent(at).expect("the chain reaches the root");
            }
            tree.span(at).start >= from
        });
        if !eligible
            || children.is_empty()
            || children.iter().any(|test| test.exclusion.is_some())
            || focused
            || all_children_unrun
            || registered_conditionally
            || normalizer.order_dependent(body, true)
        {
            continue;
        }
        for scope in around {
            context.push(contexts.get(&Some(scope)).cloned().unwrap_or_default());
        }
        context.push(normalizer.of(site.node));
        let key = digest(&serde_json::to_string(&context).expect("strings serialize"));
        let mut suite = super::unkeyed_case(
            super::at_span(&lines, source, site.span.start, site.span.end),
            names.last().cloned(),
            None,
        );
        suite.full_title = Some(names.join(" > "));
        suite.name = Some(names);
        suite.key = key;
        suites.push(suite);
    }
    suites.sort_by_key(|suite| suite.span.start_byte);
    suites
}

/// The spans of a file's skipped and placeholder test registrations, built once per file so each
/// enclosing suite's children are checked in logarithmic time instead of by a scan of every site.
struct Unrun {
    /// `(start, greatest end among spans starting at or before it)`, sorted by start.
    reach: Vec<(u32, u32)>,
}

impl Unrun {
    /// Collects the file's `skip` and `todo` test registrations.
    fn new(found: &Inventory, setting: Setting<'_>) -> Self {
        let mut spans: Vec<_> = found
            .sites
            .iter()
            .filter(|site| {
                site.kind == SiteKind::Registers(Registers::Test)
                    && site.chain.iter().any(|member| {
                        matches!(
                            setting.dialect.chain_member(member),
                            Some(Chain::Skip | Chain::Todo)
                        )
                    })
            })
            .map(|site| (site.span.start, site.span.end))
            .collect();
        spans.sort_unstable();
        let mut furthest = 0;
        let reach = spans
            .into_iter()
            .map(|(start, end)| {
                furthest = furthest.max(end);
                (start, furthest)
            })
            .collect();
        Self { reach }
    }

    /// Whether some skipped or placeholder registration spans `start..end`.
    fn contains(&self, start: u32, end: u32) -> bool {
        let before = self.reach.partition_point(|&(from, _)| from <= start);
        before > 0 && self.reach[before - 1].1 >= end
    }
}

/// Repeated suite rows are row claims, not whole-suite equivalence admissions.
pub(super) fn table_rows(
    path: &std::path::Path,
    parsed: &Parsed,
    found: &Inventory,
    structure: &Structure,
    tests: &[TestCase],
    setting: normal::Setting,
    source: &str,
) -> Vec<TestCase> {
    let tree = &parsed.tree;
    let lines = Lines::new(source);
    let mut cases = Vec::new();
    if found.direct_eval
        || found.non_hoisted_mock
        || found.unreadable_mock_factory
        || found.unreadable_fixture
    {
        return cases;
    }
    for site in found
        .sites
        .iter()
        .filter(|s| s.kind == SiteKind::Registers(Registers::Suite))
    {
        if site
            .labels
            .iter()
            .any(|label| !matches!(label, Label::RegistrationBody))
            || !normal::row_identity::suite_prefix_known(parsed, site.node, site.provenance)
        {
            continue;
        }
        let Some(callee) = tree.child(site.node, "callee") else {
            continue;
        };
        let mut normalizer = Normalizer::new(path, parsed, setting, structure);
        let modifiers = normalizer.modifiers(callee, site.member);
        let Some(rows) = modifiers.rows else {
            continue;
        };
        let Some(body) = normalizer
            .callback(site.node)
            .filter(|c| c.inline)
            .and_then(|c| c.function)
        else {
            continue;
        };
        let children = tests
            .iter()
            .filter(|c| site.span.start <= c.span.start_byte && c.span.end_byte <= site.span.end)
            .collect::<Vec<_>>();
        if children.is_empty() || children.iter().any(|c| c.exclusion.is_some()) {
            continue;
        }
        let Some(pattern) = normal::titles::title(parsed, site.node) else {
            continue;
        };
        for (parent_ordinal, frame) in
            super::table_frames::around(path, parsed, structure, setting, site.node, found)
                .into_iter()
                .enumerate()
        {
            if !frame.proven {
                continue;
            }
            let mut normalizer = Normalizer::new(path, parsed, setting, structure)
                .for_suite_values(frame.values.clone());
            if !normalizer.active_suite(
                callee,
                &tree
                    .list(site.node, "arguments")
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>(),
            ) || !pure_registration(tree, &mut normalizer, site.node)
                || !closed_collection(parsed, found, &mut normalizer, body, 16)
            {
                continue;
            }
            let Some(scopes) = normalizer.suites_around(site.node) else {
                continue;
            };
            let prefix = scopes
                .iter()
                .map(|scope| {
                    let call = *structure.suites.get(scope)?;
                    frame
                        .names
                        .get(&call)
                        .cloned()
                        .unwrap_or_else(|| normal::titles::title(parsed, call))
                })
                .collect::<Option<Vec<_>>>();
            let Some(prefix) = prefix else {
                continue;
            };
            for row in &rows {
                let Some(row_class) =
                    normal::row_identity::identity(&mut normalizer, &row.value, body)
                else {
                    continue;
                };
                let Some((bound, every)) = normalizer.bind_row(
                    &tree
                        .list(body, "params")
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>(),
                    &row.value,
                ) else {
                    continue;
                };
                let Some(title) = normalizer.render_table_title(
                    parsed,
                    &pattern,
                    row,
                    modifiers.whole_rows,
                    crate::framework::TitleMode::Row,
                ) else {
                    continue;
                };
                let mut name = prefix.clone();
                name.push(title.clone());
                let normal = normalizer.substituted(body, bound, every);
                let mut case = super::unkeyed_case(
                    super::at_span(&lines, source, site.span.start, site.span.end),
                    Some(title),
                    None,
                );
                case.full_title = Some(name.join(" > "));
                case.name = Some(name);
                case.row = Some(row.index);
                let stride = rows
                    .iter()
                    .map(|r| r.index)
                    .max()
                    .and_then(|n| n.checked_add(1))
                    .unwrap_or(1);
                case.runtime_row = parent_ordinal
                    .checked_mul(stride)
                    .and_then(|base| base.checked_add(row.index));
                if case.runtime_row.is_none() {
                    continue;
                }
                case.suite_rows = frame.rows.clone();
                case.row_class = Some(digest(&row_class));
                case.body = digest(&normal);
                case.key = digest(&format!("{}|{}", modifiers.untabled, normal));
                case.inert = modifiers.inert || frame.inert;
                cases.push(case);
            }
        }
    }
    cases
}

/// Collection-time statements must be inert declarations or closed test/suite registrations.
fn closed_collection(
    parsed: &Parsed,
    found: &Inventory,
    normalizer: &mut Normalizer<'_>,
    body: NodeId,
    depth: usize,
) -> bool {
    if depth == 0 {
        return false;
    }
    let tree = &parsed.tree;
    if tree
        .child(body, "body")
        .is_none_or(|body| tree.kind(body) != "BlockStatement")
    {
        return false;
    }
    normalizer
        .body_statements(body)
        .into_iter()
        .all(|statement| {
            if crate::inventory::runners::inert_declaration(parsed, statement) {
                return true;
            }
            let Some(call) = tree
                .child(statement, "expression")
                .filter(|_| tree.kind(statement) == "ExpressionStatement")
                .map(|n| tree.inner(n))
            else {
                return false;
            };
            let Some(site) = found.sites.iter().find(|s| s.node == call) else {
                return false;
            };
            if !pure_registration(tree, normalizer, call) {
                return false;
            }
            match site.kind {
                SiteKind::Registers(Registers::Test) => true,
                SiteKind::Registers(Registers::Suite) => normalizer
                    .callback(call)
                    .filter(|c| c.inline)
                    .and_then(|c| c.function)
                    .is_some_and(|f| closed_collection(parsed, found, normalizer, f, depth - 1)),
                _ => false,
            }
        })
}

/// A registration's options and table construction must execute no unknown project expression.
fn pure_registration(
    tree: &crate::lang::Tree,
    normalizer: &mut Normalizer<'_>,
    call: NodeId,
) -> bool {
    let Some(callee) = tree.child(call, "callee") else {
        return false;
    };
    tree.list(call, "arguments")
        .into_iter()
        .flatten()
        .all(|arg| {
            let arg = tree.inner(arg);
            matches!(
                tree.kind(arg),
                "ArrowFunctionExpression" | "FunctionExpression"
            ) || pure_input(tree, normalizer, arg, 32)
        })
        && std::iter::once(callee)
            .chain(tree.descendants(callee))
            .filter(|&n| matches!(tree.kind(n), "CallExpression" | "TaggedTemplateExpression"))
            .all(|call| {
                let inputs = if tree.kind(call) == "TaggedTemplateExpression" {
                    let Some(quasi) = tree.child(call, "quasi") else {
                        return false;
                    };
                    tree.list(quasi, "expressions")
                } else {
                    tree.list(call, "arguments")
                };
                inputs
                    .into_iter()
                    .flatten()
                    .all(|arg| pure_input(tree, normalizer, arg, 32))
            })
}

/// A bounded literal input is inert only when each of its evaluated children is also inert.
fn pure_input(
    tree: &crate::lang::Tree,
    normalizer: &mut Normalizer<'_>,
    node: NodeId,
    depth: usize,
) -> bool {
    if depth == 0 {
        return false;
    }
    if normalizer.row_scalar(node).is_some() {
        return true;
    }
    let node = tree.inner(node);
    match tree.kind(node) {
        "ArrayExpression" => tree
            .list(node, "elements")
            .into_iter()
            .all(|n| n.is_some_and(|n| pure_input(tree, normalizer, n, depth - 1))),
        "ObjectExpression" => tree.list(node, "properties").into_iter().all(|p| {
            p.is_some_and(|p| {
                tree.kind(p) == "Property"
                    && !tree.flag(p, "computed")
                    && !tree.flag(p, "method")
                    && tree.text(p, "kind") == Some("init")
                    && tree
                        .child(p, "value")
                        .is_some_and(|v| pure_input(tree, normalizer, v, depth - 1))
            })
        }),
        _ => false,
    }
}
