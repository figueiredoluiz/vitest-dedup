use super::{inventory_of, Reach, Registers, SiteKind};

/// The permanent runtime/recall chain retains every site without scanning the whole index per body.
#[test]
fn owned_wrapper_chain_keeps_reach_with_bounded_query_work() {
    let source = include_str!("../../../fixtures/recall/wrapper-reach-chain/chain.test.ts");
    super::super::reach::take_checks();
    let (found, _) = inventory_of(source, &[], false);
    let checks = super::super::reach::take_checks();
    let wrappers = source
        .lines()
        .filter(|line| line.starts_with("function f"))
        .count();
    assert_eq!(wrappers, 24);
    assert_eq!(found.sites.len(), 2 * wrappers + 1);
    assert_eq!(
        found
            .sites
            .iter()
            .filter(|site| site.kind == SiteKind::Wrapper(Reach::Tasks))
            .count(),
        wrappers
    );
    assert_eq!(
        found
            .sites
            .iter()
            .filter(|site| matches!(site.kind, SiteKind::Registers(Registers::Hook(_))))
            .count(),
        wrappers
    );
    assert_eq!(
        found
            .sites
            .iter()
            .filter(|site| site.kind == SiteKind::Registers(Registers::Test))
            .count(),
        1
    );
    // At most two searches for each body in each propagation round. This source has one
    // named function and one hook callback per link, plus the terminal test callback.
    let functions = 2 * wrappers + 1;
    let search_steps = (found.sites.len() + 1).ilog2() as usize + 2;
    let bound = 2 * functions * (wrappers + 2) * search_steps;
    assert!(
        checks > 0 && checks <= bound,
        "{checks} query comparisons exceed {bound}"
    );
}
