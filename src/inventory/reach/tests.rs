use super::*;
use crate::framework::HookTiming;

/// Index queries retain the full containment predicate across order, overlap and every site kind.
#[test]
fn indexed_reach_matches_containment() {
    let kinds = [
        SiteKind::Registers(Registers::Test),
        SiteKind::Registers(Registers::Suite),
        SiteKind::Registers(Registers::Bench),
        SiteKind::Registers(Registers::Hook(HookTiming::BeforeEach)),
        SiteKind::Wrapper(Reach::Hook),
        SiteKind::Wrapper(Reach::Hooks),
        SiteKind::Wrapper(Reach::Tasks),
    ];
    let spans = [(0, 12), (2, 9), (2, 4), (5, 8), (8, 8), (10, 11)];
    for kind in kinds {
        let mut sites: Vec<_> = spans
            .iter()
            .map(|&(start, end)| (Span { start, end }, kind))
            .collect();
        sites.push((Span { start: 4, end: 5 }, SiteKind::Wrapper(Reach::Tasks)));
        for _ in 0..sites.len() {
            sites.rotate_left(1);
            let index = ReachIndex::new(sites.iter().copied());
            for start in 0..=13 {
                for end in start..=13 {
                    let body = Span { start, end };
                    let expected = sites
                        .iter()
                        .filter(|(span, _)| body.contains(*span))
                        .map(|(_, kind)| match kind {
                            SiteKind::Registers(Registers::Hook(_))
                            | SiteKind::Wrapper(Reach::Hooks | Reach::Hook) => Reach::Hooks,
                            _ => Reach::Tasks,
                        })
                        .max();
                    assert_eq!(index.within(body), expected, "{body:?} {sites:?}");
                }
            }
        }
    }
    let last = Span {
        start: u32::MAX,
        end: u32::MAX,
    };
    assert_eq!(ReachIndex::new(std::iter::empty()).within(last), None);
    assert_eq!(
        ReachIndex::new([(last, SiteKind::Wrapper(Reach::Tasks))].into_iter()).within(last),
        Some(Reach::Tasks)
    );
}
