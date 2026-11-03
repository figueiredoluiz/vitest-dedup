//! Optional evidence from owned normalization; never a second rule implementation.
use super::{Hooks, Span};
use crate::framework::{Dialect, Registers};
use crate::inventory::{Inventory, SiteKind};
use crate::lang::{Lines, Parsed};
use crate::memory::HeapBytes;
use crate::normal::{digest, Normalizer};
use std::collections::HashMap;

/// Independent owned views used to check a fixture's declared isolation dimension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Views {
    /// Exact owned normal form, with literal values retained.
    pub body: u64,
    /// Literal-shaped owned form; absent when rendering exceeded its bound.
    pub shape: Option<u64>,
    /// Ordered event digest; absent when the trace was not read to the end.
    pub trace: Option<u64>,
    /// Ambient context including inherited suite settings and module-load effects.
    pub context: u64,
    /// Test chain, expected failure, options and unresolved row inputs.
    pub modifiers: u64,
}

/// A known hook declaration, independently of whether a redundancy proof exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticHook {
    /// Declaration coordinates, not an enclosing test's coordinates.
    pub span: Span,
    /// The dialect's lifecycle name.
    pub kind: &'static str,
    /// Owned body views; absent for an unread callback or a truncated exact form.
    pub views: Option<Box<Views>>,
    /// The actual effect key, only when this declaration participates in a proved chain.
    pub effect_key: Option<u64>,
}

impl HeapBytes for DiagnosticHook {
    /// Only the optional view box allocates; span, kind and key are inline.
    fn heap_bytes(&self) -> usize {
        self.views.as_ref().map_or(0, |_| size_of::<Views>())
    }
}

/// Project inventory declarations without widening the hook planner's admitted population.
pub(super) fn hooks<'a>(
    parsed: &'a Parsed,
    found: &Inventory,
    proved: &Hooks,
    normalizer: &mut Normalizer<'a>,
    dialect: &Dialect,
    source: &str,
) -> Vec<DiagnosticHook> {
    let lines = Lines::new(source);
    found
        .sites
        .iter()
        .filter_map(|site| {
            let SiteKind::Registers(Registers::Hook(_)) = site.kind else {
                return None;
            };
            let span = super::at_span(&lines, source, site.span.start, site.span.end);
            let callback = parsed
                .tree
                .list(site.node, "arguments")
                .first()
                .copied()
                .flatten()
                .map(|node| parsed.tree.inner(node))
                .filter(|&node| parsed.tree.is_function(node));
            let views = callback.and_then(|callback| {
                normalizer.take_truncated();
                let body = digest(&normalizer.of(callback));
                if normalizer.take_truncated() {
                    return None;
                }
                let shape = digest(&normalizer.shape(callback, HashMap::new(), false));
                let shape = (!normalizer.take_truncated()).then_some(shape);
                let (trace, _) =
                    crate::behavior::trace(normalizer, parsed, callback, HashMap::new(), false);
                Some(Box::new(Views {
                    body,
                    shape,
                    trace: trace
                        .complete
                        .then(|| digest(&format!("{:?}", trace.events))),
                    context: 0,
                    modifiers: 0,
                }))
            });
            let effect_key = proved
                .declarations
                .iter()
                .enumerate()
                .find_map(|(index, hook)| {
                    (hook.span == span && proved.chains.iter().any(|chain| chain.contains(&index)))
                        .then(|| hook.key.as_deref().map(digest))
                        .flatten()
                });
            Some(DiagnosticHook {
                span,
                kind: super::site_kind(dialect, site.kind),
                views,
                effect_key,
            })
        })
        .collect()
}
