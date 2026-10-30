//! Grouping and bounded pair/cluster emission shared by every duplication rule.
//!
//! Mirrors CukeDedup's `analysis::pairs` (see
//! `cuke-dedup/src/analysis/pairs.rs`): group test cases by an equality key, then report a
//! *spanning* set of pairs for small groups instead of the full quadratic cross-product, and
//! collapse larger groups into a single cluster finding.
//!
//! One deliberate simplification versus CukeDedup: every rule here groups by an *exact* equality
//! key (`duplicate-test` on alpha-body + context, `duplicate-test-name` on file + full title),
//! never a fuzzy similarity score. A group is therefore already one connected component by
//! construction, so there is no need for CukeDedup's union-find step to merge overlapping pair
//! findings that came from different candidate sources into clusters -- one equality group *is*
//! one cluster.

/// Groups larger than this are collapsed into a single cluster finding instead of a full linear
/// spanning set of pair findings.
///
/// Chosen to match CukeDedup's `MIN_CLUSTER_DEFINITIONS` threshold of 5: at four members, three
/// pair findings are still more informative to a reader than one cluster summary (each pair names
/// two concrete locations); at five, the pair list starts to dominate the report without adding
/// information a cluster summary ("5 tests share this fingerprint") does not already convey.
pub(crate) const SPANNING_GROUP_MAX: usize = 4;

/// How one equality group of 2+ members should be turned into findings.
pub(crate) enum GroupOutcome {
    /// `(left, right)` index pairs forming a spanning set: consecutive members (in the group's
    /// own, already-deterministic order) are linked pairwise, so every member appears in at
    /// least one finding without emitting the full `n * (n - 1) / 2` cross product.
    Spanning(Vec<(usize, usize)>),
    /// A group too large to enumerate pairwise; carries every member's index so the caller can
    /// build one cluster finding naming all of them.
    Cluster(Vec<usize>),
}

/// Decides how `group` (already known to share an equality key, and already sorted into
/// deterministic order by the caller) should be reported.
pub(crate) fn group_outcome(group: &[usize]) -> GroupOutcome {
    if group.len() > SPANNING_GROUP_MAX {
        GroupOutcome::Cluster(group.to_vec())
    } else {
        GroupOutcome::Spanning(group.windows(2).map(|pair| (pair[0], pair[1])).collect())
    }
}

/// Groups `items` by `key_fn`, keeping only groups with 2 or more members, and returns them in an
/// order that depends only on the *content* of `items` -- never on the order the caller happened
/// to supply them in.
///
/// This is what makes [`crate::analysis::analyze`] deterministic under a shuffled input: grouping itself
/// uses a `BTreeMap` (ordered by `K`, not by insertion/hash order), each group's members are then
/// sorted by `T`'s own `Ord`, and the groups themselves are finally ordered by their first
/// (smallest) member. For every `TestCase` key used in this module, `T::cmp` sorts by location
/// (path, then line, then column) first, because `location` is `TestCase`'s first field and
/// derived `Ord` compares fields in declaration order -- so this one sort satisfies the "sort by
/// path, then line, then column" requirement for free.
///
/// The key's lifetime is tied to `items` rather than left higher-ranked, so a key may *borrow*
/// the fields it compares instead of cloning them. Every caller here does: a key is built once
/// per item, compared inside this function's map, and dropped with the map before the next rule
/// runs, while `items` outlives all of them -- so an owning key only ever bought a second copy of
/// each test's body, hook chain, and import list, once per rule.
///
/// `key_fn` returns `None` to exclude an item from grouping entirely (for example, a test with no
/// body cannot participate in `duplicate-test`); excluded items never appear in any group and are
/// therefore invisible to every rule that calls this function with that predicate.
pub(crate) fn group_and_order<'a, K, T, KeyFn>(items: &'a [T], key_fn: KeyFn) -> Vec<Vec<usize>>
where
    K: Ord,
    T: Ord + 'a,
    KeyFn: Fn(&'a T) -> Option<K>,
{
    let mut groups: std::collections::BTreeMap<K, Vec<usize>> = std::collections::BTreeMap::new();
    for (index, item) in items.iter().enumerate() {
        if let Some(key) = key_fn(item) {
            groups.entry(key).or_default().push(index);
        }
    }

    let mut groups: Vec<Vec<usize>> = groups
        .into_values()
        .filter(|group| group.len() >= 2)
        .map(|mut group| {
            group.sort_by(|&left, &right| items[left].cmp(&items[right]));
            group
        })
        .collect();
    groups.sort_by(|left, right| items[left[0]].cmp(&items[right[0]]));
    groups
}

/// Tracks pairwise-comparison "work" spent turning equality groups into findings, so a
/// pathological input (very many, very large groups) cannot make one `analyze` call run
/// unbounded.
///
/// The caller charges one unit per spanning edge it is ready to report, or `group.len() - 1` for a
/// cluster: exactly the number of equivalences it is about to certify. Relationships suppressed
/// before reporting are not charged. This keeps the budget meaningful (it reflects real reported
/// relationships) rather than an arbitrary per-group flat fee that a report full of tiny groups
/// could dodge.
pub(crate) struct PairBudget {
    remaining: u64,
}

impl PairBudget {
    /// Creates a budget starting with `limit` units of work available.
    pub(crate) fn new(limit: u64) -> Self {
        Self { remaining: limit }
    }

    /// Attempts to charge `amount` units of work against the budget.
    ///
    /// Returns `false` once the budget is exhausted; the caller must then stop processing further
    /// groups (not just the current one) and mark the analysis result incomplete, per the
    /// project rule that degraded analysis always stays visible rather than silently truncating.
    pub(crate) fn charge(&mut self, amount: u64) -> bool {
        match self.remaining.checked_sub(amount) {
            Some(next) => {
                self.remaining = next;
                true
            }
            None => {
                self.remaining = 0;
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression fixture for pair emission: group outcome spans up to the max and clusters
    /// above it.
    #[test]
    fn group_outcome_spans_up_to_the_max_and_clusters_above_it() {
        assert!(
            matches!(group_outcome(&[0, 1]), GroupOutcome::Spanning(pairs) if pairs == vec![(0, 1)])
        );
        assert!(
            matches!(group_outcome(&[0, 1, 2, 3]), GroupOutcome::Spanning(pairs) if pairs == vec![(0, 1), (1, 2), (2, 3)])
        );
        assert!(matches!(
            group_outcome(&[0, 1, 2, 3, 4]),
            GroupOutcome::Cluster(members) if members == vec![0, 1, 2, 3, 4]
        ));
    }

    /// Regression fixture for pair emission: group and order drops singletons and excluded
    /// items.
    #[test]
    fn group_and_order_drops_singletons_and_excluded_items() {
        let items = vec![1, 2, 1, 3];
        let groups = group_and_order(&items, |value| (*value != 3).then_some(*value));
        // `3` is excluded by the predicate; `2` has no duplicate, so only `1`'s pair remains.
        assert_eq!(groups, vec![vec![0, 2]]);
    }

    #[test]
    /// A budget already spent refuses further charges.
    fn pair_budget_refuses_once_exhausted() {
        let mut budget = PairBudget::new(3);
        assert!(budget.charge(2));
        assert!(!budget.charge(2));
        assert!(budget.charge(0));
    }
}
