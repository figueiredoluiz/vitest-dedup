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

/// Walk steps reading an exported conditional chain of `arms` arms, none a registration function.
/// The conditions are unknown, so every arm is read.
fn chain_walk_steps(arms: usize) -> usize {
    let chain: Vec<String> = (0..arms).map(|arm| format!("on{} ? f", arm % 2)).collect();
    let helper = format!(
        "const noop = () => {{}}\nconst f = noop, on0 = process.env.A, on1 = process.env.B\n\
         export const run: any = {} : noop",
        chain.join(" : ")
    );
    super::super::module::take_walk_steps();
    let (found, _) = inventory_of(
        "import { run } from './helper'\nrun('a', () => {})",
        &[("helper.ts", helper.as_str())],
        false,
    );
    assert!(found.sites.is_empty(), "{:?}", found.sites);
    super::super::module::take_walk_steps()
}

/// Asking whether a conditional chain may hold a registration function reads each arm a bounded
/// number of times: doubling the arms at most doubles the work, plus slack for constant terms.
/// Both lengths stay under the depth limit, so neither is cut short.
#[test]
fn a_conditional_chain_is_read_in_linear_work() {
    let small = chain_walk_steps(16);
    let large = chain_walk_steps(32);
    assert!(
        small > 0 && large > small,
        "{small} steps for 16 arms, {large} for 32"
    );
    assert!(
        large <= 2 * small + 64,
        "{small} steps for 16 arms, {large} for 32"
    );
}

/// Escape-climb steps reading an exported value built from `parts` parts by `shape`.
fn value_steps(parts: usize, shape: fn(usize) -> String) -> usize {
    let helper = format!(
        "import {{ test }} from 'vitest'\nconst noop = (..._: unknown[]) => {{}}\n\
         const on0 = 1, on1 = 0\nexport const run: any = {}",
        shape(parts)
    );
    super::super::module::take_climb_steps();
    inventory_of(
        "import { run } from './helper'\nrun('a', () => {})",
        &[("helper.ts", helper.as_str())],
        false,
    );
    super::super::module::take_climb_steps()
}

/// Escaped reads are climbed in linear work, where each fails only at the top: many arguments of
/// one call, each a nested member read through casts, and a chain reading the API in every arm,
/// both in a condition.
#[test]
fn escaped_reads_are_climbed_in_linear_work() {
    type Shape = (&'static str, fn(usize) -> String);
    let shapes: [Shape; 2] = [
        ("arguments", |n| {
            let read = "((test as any).each as any).x";
            format!("noop({}) ? noop : noop", vec![read; n].join(", "))
        }),
        ("chain", |n| {
            let arms: Vec<String> = (0..n).map(|arm| format!("on{} ? test", arm % 2)).collect();
            format!("noop({} : noop) ? noop : noop", arms.join(" : "))
        }),
    ];
    for (name, shape) in shapes {
        let small = value_steps(400, shape);
        let large = value_steps(800, shape);
        assert!(
            small > 0 && large > small,
            "{name}: {small} steps for 400, {large} for 800"
        );
        assert!(
            large <= 2 * small + 64,
            "{name}: {small} steps for 400, {large} for 800"
        );
    }
}

/// Walk steps of a module binding each of `n` names to one element of an `n`-element table,
/// each element `element` or, if empty, its index.
fn table_read_steps(n: usize, element: &str) -> usize {
    let elements: Vec<String> = (0..n)
        .map(|i| match element {
            "" => i.to_string(),
            element => element.to_owned(),
        })
        .collect();
    let reads: Vec<String> = (0..n).map(|i| format!("const a{i} = table[{i}]")).collect();
    let source = format!(
        "import {{ test }} from 'vitest'\nconst table = [{}]\n{}\ntest('t', () => {{}})",
        elements.join(", "),
        reads.join("\n")
    );
    super::super::module::take_walk_steps();
    let (found, _) = inventory_of(&source, &[], false);
    assert!(!found.sites.is_empty());
    super::super::module::take_walk_steps()
}

/// Bindings reading one shared table are evaluated in linear work: the table and its writes are
/// read once per pass, not once per binding, also when each binding read adds an alias.
#[test]
fn reads_of_a_shared_table_are_linear() {
    for element in ["", "test"] {
        let small = table_read_steps(400, element);
        let large = table_read_steps(800, element);
        assert!(
            small > 0 && large > small,
            "{element:?}: {small} steps for 400, {large} for 800"
        );
        assert!(
            large <= 2 * small + 64,
            "{element:?}: {small} steps for 400, {large} for 800"
        );
    }
}
