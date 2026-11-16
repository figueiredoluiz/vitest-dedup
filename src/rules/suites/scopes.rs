//! Source intervals used to match findings to enclosing suite copies.

use crate::model::Location;
use crate::pairs::PairBudget;
use std::collections::BTreeMap;
use std::path::Path;

/// A path's suites are ordered by start, with the greatest end in each subtree at its midpoint.
pub(super) struct Scopes<'a, T>(BTreeMap<&'a Path, Vec<Scope<T>>>);

struct Scope<T> {
    start: u32,
    end: u32,
    max_end: u32,
    value: T,
}

impl<'a, T: Copy> Scopes<'a, T> {
    /// Builds the interval index without assuming suite spans form a proper nesting tree.
    pub(super) fn new(locations: impl IntoIterator<Item = (&'a Location, T)>) -> Self {
        let mut files = BTreeMap::<_, Vec<Scope<T>>>::new();
        for (location, value) in locations {
            files
                .entry(location.path.as_path())
                .or_default()
                .push(Scope {
                    start: location.span.start_byte,
                    end: location.span.end_byte,
                    max_end: 0,
                    value,
                });
        }
        for scopes in files.values_mut() {
            scopes.sort_unstable_by_key(|scope| scope.start);
            index_ends(scopes);
        }
        Self(files)
    }

    /// Visits only suites that start earlier and fully enclose the requested location.
    pub(super) fn enclosing(
        &self,
        location: &Location,
        work: &mut PairBudget,
        mut visit: impl FnMut(T),
    ) -> Result<(), ()> {
        if let Some(scopes) = self.0.get(location.path.as_path()) {
            enclosing(
                scopes,
                location.span.start_byte,
                location.span.end_byte,
                work,
                &mut visit,
            )?;
        }
        Ok(())
    }
}

/// Fills the maximum end of each balanced subtree, whose depth is logarithmic in its size.
fn index_ends<T>(scopes: &mut [Scope<T>]) -> u32 {
    let (left, rest) = scopes.split_at_mut(scopes.len() / 2);
    let Some((middle, right)) = rest.split_first_mut() else {
        return 0;
    };
    middle.max_end = middle.end.max(index_ends(left)).max(index_ends(right));
    middle.max_end
}

/// Prunes a subtree when none of its ends reaches the requested end or its starts are too late.
fn enclosing<T: Copy>(
    scopes: &[Scope<T>],
    start: u32,
    end: u32,
    work: &mut PairBudget,
    visit: &mut impl FnMut(T),
) -> Result<(), ()> {
    let (left, rest) = scopes.split_at(scopes.len() / 2);
    let Some((middle, right)) = rest.split_first() else {
        return Ok(());
    };
    work.charge(1).then_some(()).ok_or(())?;
    if middle.max_end < end {
        return Ok(());
    }
    enclosing(left, start, end, work, visit)?;
    if middle.start < start {
        if end <= middle.end {
            visit(middle.value);
        }
        enclosing(right, start, end, work, visit)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Position, Span};

    /// A span with independent byte endpoints and a file identity.
    fn site(path: &str, start: u32, end: u32) -> Location {
        Location {
            path: path.into(),
            span: Span {
                start: Position { line: 1, column: 1 },
                end: Position { line: 1, column: 2 },
                start_byte: start,
                end_byte: end,
            },
        }
    }

    /// Crossed ranges, equal boundaries and a different path must follow exact containment.
    #[test]
    fn enclosing_matches_a_direct_interval_filter() {
        let copies = [
            site("a.ts", 3, 20),
            site("a.ts", 5, 14),
            site("a.ts", 5, 22),
            site("b.ts", 3, 20),
        ];
        let index = Scopes::new(copies.iter().zip(0..));
        for path in ["a.ts", "b.ts", "c.ts"] {
            for start in 0..24 {
                for end in start..24 {
                    let request = site(path, start, end);
                    let mut actual = Vec::new();
                    index
                        .enclosing(&request, &mut PairBudget::new(8), |id| actual.push(id))
                        .unwrap();
                    actual.sort_unstable();
                    let expected: Vec<_> = copies
                        .iter()
                        .enumerate()
                        .filter_map(|(id, copy)| {
                            (copy.path == request.path
                                && copy.span.start_byte < start
                                && end <= copy.span.end_byte)
                                .then_some(id)
                        })
                        .collect();
                    assert_eq!(actual, expected, "{path} {start}..{end}");
                }
            }
        }
    }

    /// Looking up one late suite copy fits a small budget even with many earlier declarations.
    #[test]
    fn lookups_prune_finished_intervals() {
        let copies: Vec<_> = (0..512).map(|i| site("a.ts", i * 10, i * 10 + 9)).collect();
        let index = Scopes::new(copies.iter().zip(0..));
        let request = site("a.ts", 5101, 5109);
        let mut matched = Vec::new();
        let mut visit = |id| matched.push(id);
        assert_eq!(
            index.enclosing(&request, &mut PairBudget::new(0), &mut visit),
            Err(())
        );
        index
            .enclosing(&request, &mut PairBudget::new(32), visit)
            .unwrap();
        assert_eq!(matched, [510]);
    }
}
