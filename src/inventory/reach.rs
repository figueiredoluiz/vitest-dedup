//! Indexed containment queries for one immutable registration scan.

use super::{Reach, SiteKind};
use crate::framework::Registers;
use crate::lang::Span;

/// Starts paired with suffix-minimum ends, separately for each possible body reach.
pub(super) struct ReachIndex {
    hooks: Vec<Span>,
    tasks: Vec<Span>,
}

impl ReachIndex {
    /// Record the scan's classifications without changing its sites or their order.
    pub(super) fn new(sites: impl Iterator<Item = (Span, SiteKind)>) -> Self {
        let mut hooks = Vec::new();
        let mut tasks = Vec::new();
        for (span, kind) in sites {
            match kind {
                SiteKind::Registers(Registers::Hook(_))
                | SiteKind::Wrapper(Reach::Hooks | Reach::Hook) => hooks.push(span),
                SiteKind::Registers(_) | SiteKind::Wrapper(Reach::Tasks) => tasks.push(span),
            }
        }
        Self {
            hooks: suffix_ends(hooks),
            tasks: suffix_ends(tasks),
        }
    }

    /// The widest reach of sites wholly contained in `body`, or none when it contains no site.
    pub(super) fn within(&self, body: Span) -> Option<Reach> {
        if contains(&self.tasks, body) {
            Some(Reach::Tasks)
        } else if contains(&self.hooks, body) {
            Some(Reach::Hooks)
        } else {
            None
        }
    }
}

/// Preserve sorted starts while replacing each end with the smallest end at or after that start.
fn suffix_ends(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_unstable_by_key(|span| span.start);
    let mut end = u32::MAX;
    for span in spans.iter_mut().rev() {
        end = end.min(span.end);
        span.end = end;
    }
    spans
}

/// A qualifying start has a contained site exactly when its suffix's minimum end fits the body.
fn contains(spans: &[Span], body: Span) -> bool {
    let first = spans.partition_point(|span| {
        #[cfg(test)]
        CHECKS.with(|checks| checks.set(checks.get() + 1));
        span.start < body.start
    });
    spans.get(first).is_some_and(|span| span.end <= body.end)
}

#[cfg(test)]
thread_local! {
    static CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Reset and return this thread's query comparisons for deterministic scaling regressions.
#[cfg(test)]
pub(super) fn take_checks() -> usize {
    CHECKS.with(|checks| checks.replace(0))
}

#[cfg(test)]
mod tests;
