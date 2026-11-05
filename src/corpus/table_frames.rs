//! Bounded enclosing suite-row invocations, independent of a child's own table rows.
use crate::lang::{NodeId, Parsed, Scalar, SymbolId};
use crate::normal::{Normalizer, Setting, Structure};
use std::collections::HashMap;
use std::path::Path;

/// A suite row's own values and rendered ancestry; identifiers belong to this parsed module.
#[derive(Clone, Debug)]
pub(super) struct Frame {
    /// Whether an enclosing table has been expanded.
    pub table: bool,
    /// Whether every input and activation was proven.
    pub proven: bool,
    /// A readonly literal array is opaque and can compare only within this invocation.
    pub isolated: bool,
    /// A parent skip/todo applies to each generated child.
    pub inert: bool,
    /// Primitive bindings shared by this invocation's children.
    pub values: HashMap<SymbolId, Scalar>,
    /// Comparison titles, by suite registering call.
    pub names: HashMap<NodeId, Option<String>>,
    /// Display titles, separately from comparison proof.
    pub display: HashMap<NodeId, Option<String>>,
    /// Ancestor table declarations and their invocation ordinals.
    pub rows: Vec<(u32, usize)>,
}

impl Default for Frame {
    /// A non-table scope contributes no new invocation identity.
    fn default() -> Self {
        Self {
            table: false,
            proven: true,
            isolated: false,
            inert: false,
            values: HashMap::new(),
            names: HashMap::new(),
            display: HashMap::new(),
            rows: Vec::new(),
        }
    }
}

/// Expands the cartesian parent rows in lexical order, retaining uncertainty as a refusal.
pub(super) fn around(
    path: &Path,
    parsed: &Parsed,
    structure: &Structure,
    setting: Setting,
    node: NodeId,
    found: &crate::inventory::Inventory,
) -> Vec<Frame> {
    let normalizer = Normalizer::new(path, parsed, setting, structure);
    let Some(scopes) = normalizer.suites_around(node) else {
        return vec![Frame::default()];
    };
    let tree = &parsed.tree;
    let table = |callee| {
        std::iter::once(callee)
            .chain(tree.descendants(callee))
            .any(|n| {
                tree.kind(n) == "MemberExpression"
                    && tree
                        .child(n, "property")
                        .and_then(|p| tree.text(p, "name"))
                        .is_some_and(|name| {
                            matches!(
                                setting.dialect.chain_member(name),
                                Some(crate::framework::Chain::Table(_))
                            )
                        })
            })
    };
    if !scopes.iter().any(|scope| {
        structure
            .suites
            .get(scope)
            .and_then(|call| tree.child(*call, "callee"))
            .is_some_and(table)
    }) {
        return vec![Frame::default()];
    }
    let mut frames = vec![Frame::default()];
    for scope in scopes {
        let Some(&call) = structure.suites.get(&scope) else {
            continue;
        };
        let Some(callee) = tree.child(call, "callee") else {
            continue;
        };
        let mut next = Vec::new();
        for (parent_ordinal, frame) in frames.into_iter().enumerate() {
            let mut normalizer = Normalizer::new(path, parsed, setting, structure)
                .for_suite_values(frame.values.clone());
            let modifiers = normalizer.modifiers(callee, None);
            let registration_body =
                found
                    .sites
                    .iter()
                    .find(|s| s.node == call)
                    .is_some_and(|site| {
                        site.labels
                            .iter()
                            .all(|label| matches!(label, crate::inventory::Label::RegistrationBody))
                    });
            let Some(rows) = modifiers.rows else {
                // Ordinary suites need no extra invocation. Unknown table calls keep one
                // refused population marker rather than pretending their rows are proven.
                let is_table = table(callee);
                let mut frame = frame;
                frame.proven &= registration_body;
                let arguments = tree
                    .list(call, "arguments")
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>();
                frame.inert |= modifiers.inert || normalizer.inert_options(&arguments);
                frame.proven &= normalizer.active_suite(callee, &arguments);
                if is_table {
                    frame.table = true;
                    frame.proven = false;
                }
                next.push(frame);
                continue;
            };
            if next.len().saturating_add(rows.len()) > 2048 {
                return vec![Frame {
                    table: true,
                    proven: false,
                    ..Frame::default()
                }];
            }
            let arguments = tree
                .list(call, "arguments")
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            let active = normalizer.active_suite(callee, &arguments);
            let callback = normalizer.callback(call).and_then(|c| c.function);
            let pattern = crate::normal::titles::title(parsed, call);
            let stride = rows
                .iter()
                .map(|row| row.index)
                .max()
                .and_then(|n| n.checked_add(1))
                .unwrap_or(1);
            for row in rows {
                let mut child = frame.clone();
                child.table = true;
                let Some(invocation) = parent_ordinal
                    .checked_mul(stride)
                    .and_then(|base| base.checked_add(row.index))
                else {
                    return vec![Frame {
                        table: true,
                        proven: false,
                        ..Frame::default()
                    }];
                };
                child.rows.push((tree.span(call).start, invocation));
                child.proven &= registration_body;
                child.inert |= modifiers.inert || normalizer.inert_options(&arguments);
                let values = callback.and_then(|f| normalizer.suite_row_values(f, &row.value));
                child.proven &= active && values.is_some();
                if let Some((values, isolated)) = values {
                    child.values.extend(values);
                    child.isolated |= isolated;
                }
                child.names.insert(
                    call,
                    pattern.as_deref().and_then(|p| {
                        normalizer.render_table_title(
                            parsed,
                            p,
                            &row,
                            modifiers.whole_rows,
                            crate::framework::TitleMode::Comparison,
                        )
                    }),
                );
                child.display.insert(
                    call,
                    pattern.as_deref().and_then(|p| {
                        normalizer.render_table_title(
                            parsed,
                            p,
                            &row,
                            modifiers.whole_rows,
                            crate::framework::TitleMode::Display,
                        )
                    }),
                );
                next.push(child);
            }
        }
        frames = next;
    }
    frames
}
