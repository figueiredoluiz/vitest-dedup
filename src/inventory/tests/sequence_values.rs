//! Sequence capabilities preserve existing wrapper routes and their independent prefix effects.
use super::*;

/// The independent direct control remains a test registration.
fn retained(found: &Inventory) -> bool {
    found.sites.iter().any(|site| {
        site.title.as_deref() == Some("retained")
            && site.kind == SiteKind::Registers(Registers::Test)
    })
}

/// Measure resolver work only across extraction, retaining semantic controls for every input.
fn resolution_case(expression: &str, conditional: bool) -> (Inventory, usize) {
    let flag = if conditional {
        "const flag = true; "
    } else {
        ""
    };
    let source = format!("import {{test}} from 'vitest'; {flag}function run() {{test('inner', () => {{}})}} {expression}(); test('retained', () => {{}});");
    crate::inventory::module::take_resolutions();
    let (found, parsed) = inventory_with_tree(&source, &[], false);
    let work = crate::inventory::module::take_resolutions();
    assert_eq!(parsed.parse_errors, 0);
    assert_eq!(found.sites.len(), 3, "{source}: {found:?}");
    assert_eq!(
        found
            .sites
            .iter()
            .filter(|site| matches!(site.kind, SiteKind::Wrapper(_)))
            .count(),
        1
    );
    assert!(retained(&found));
    (found, work)
}

/// A sequence transports only the recorded wrapper value, across independently supported routes.
#[test]
fn sequence_wrappers_match_existing_routes() {
    let records = |found: &Inventory| {
        found
            .sites
            .iter()
            .map(|s| (s.kind, s.title.clone(), s.chain.clone(), s.provenance))
            .collect::<Vec<_>>()
    };
    let escaped = |found: &Inventory| {
        found
            .escapes
            .iter()
            .map(|escape| escape.text.clone())
            .collect::<Vec<_>>()
    };
    for (name, api, call) in [
        ("run", "test", "api(title, callback)"),
        ("collect", "test", "api(title, callback)"),
        ("setup", "beforeEach", "api(callback)"),
    ] {
        let declaration = format!(
            "import {{{api} as api}} from 'vitest'; function {name}(title, callback) {{ {call} }}"
        );
        let collect = |prefix: &str, final_value: &str, route: &str, sequence| {
            let value = if sequence {
                format!("({prefix}, {final_value})")
            } else {
                format!("({final_value})")
            };
            let earlier = if sequence {
                String::new()
            } else {
                format!("{prefix};")
            };
            let tail = match route {
                "direct" => format!("{value}('selected', () => {{}});"),
                "local" => format!("const held = {value}; held('selected', () => {{}});"),
                "named" => format!("export const held = {value};"),
                _ => format!("export default {value};"),
            };
            let helper = format!("{declaration} {earlier}{tail}");
            let body = match route {
                "named" => "import {held} from './helper'; held('selected', () => {});",
                "default" => "import held from './helper'; held('selected', () => {});",
                _ => &helper,
            };
            let source = format!(
                "{body} import {{test as control}} from 'vitest'; control('retained', () => {{}});"
            );
            let (found, parsed) = inventory_with_tree(&source, &[("helper.ts", &helper)], false);
            assert_eq!(parsed.parse_errors, 0, "{source}");
            found
        };
        for prefix in ["0", "false", "null", &format!("{name}.unmodeled")] {
            for final_value in [
                name.to_owned(),
                format!("({name}) as any"),
                format!("({name}) satisfies unknown"),
                format!("({name})!"),
            ] {
                for route in ["direct", "local", "named", "default"] {
                    let [expected, actual] = [false, true]
                        .map(|sequence| collect(prefix, &final_value, route, sequence));
                    let context = format!("{name} {prefix} {final_value} {route}");
                    assert!(expected.sites.len() >= 2, "{context}");
                    let counts =
                        |f: &Inventory| (f.sites.len(), f.loads.len(), f.registration_values.len());
                    assert_eq!(counts(&actual), counts(&expected), "{context}");
                    assert_eq!(records(&actual), records(&expected), "{context}");
                    assert_eq!(escaped(&actual), escaped(&expected), "{context}");
                    assert!(retained(&actual), "{context}");
                }
            }
        }
    }
}

/// Other final-value categories keep their current refusal rather than becoming wrappers.
#[test]
fn sequence_excluded_final_categories_add_no_invocation() {
    let collect = |header: &str, invoke| {
        let suffix = if invoke { "('excluded', () => {})" } else { "" };
        let source = format!("{header} (0, held){suffix}; import {{test as control}} from 'vitest'; control('retained', () => {{}});");
        inventory_with_tree(
            &source,
            &[(
                "derived.ts",
                "import {test} from 'vitest'; export const derived = test;",
            )],
            false,
        )
        .0
    };
    for header in [
        "import {test as held} from 'vitest';",
        "import * as held from 'vitest';",
        "import {derived as held} from './derived';",
    ] {
        let baseline = collect(header, false);
        let actual = collect(header, true);
        assert_eq!(actual.sites.len(), baseline.sites.len(), "{header}");
        assert!(
            actual
                .sites
                .iter()
                .all(|site| site.title.as_deref() != Some("excluded")),
            "{header}"
        );
    }
}

/// Only the directly invoked fixture-extension result adds a conservative registration.
#[test]
fn sequence_fixture_builder_accounts_for_the_invocation() {
    for (expression, direct_tests, wrappers) in [
        ("(0, api.extend({}))('selected', () => {})", 1, 1),
        ("(0, v.test.extend({}))('selected', () => {})", 1, 1),
        ("(0, v['test']['extend']({}))('selected', () => {})", 1, 1),
        (
            "(control('prefix', () => {}), v.test.extend({}))('selected', () => {})",
            2,
            1,
        ),
        ("(0, (false, api.extend({})))('selected', () => {})", 1, 1),
        ("(0, api['extend']({}))('selected', () => {})", 1, 1),
        ("((0, api.extend({})) as any)('selected', () => {})", 1, 1),
        (
            "(control('prefix', () => {}), api.extend({}))('selected', () => {})",
            2,
            1,
        ),
        (
            "((0, api.extend({}))('selected', () => {}))?.('ignored', () => {})",
            1,
            1,
        ),
        ("(0, api.extend({}))", 1, 0),
        ("(0, v.test.extend({}))", 1, 0),
        ("(0, api.test.extend({}))('other', () => {})", 1, 0),
        (
            "{const v = {test: {extend: () => () => {}}}; (0, v.test.extend({}))()}",
            1,
            0,
        ),
        ("(api.extend({}), () => {})('inert', () => {})", 1, 0),
        ("(0, api.extend)", 1, 0),
        ("(0, api.name)", 1, 0),
        (
            "const alias = api; (0, alias.extend({}))('other', () => {})",
            1,
            0,
        ),
        (
            "{const api = {extend: () => () => {}}; (0, api.extend({}))()}",
            1,
            0,
        ),
        ("{const api = {extend: () => 0}; (0, api.extend({}))}", 1, 0),
    ] {
        let source = format!("import {{test as api, test as control}} from 'vitest'; import * as v from 'vitest'; {expression}; control('retained', () => {{}});");
        let (found, parsed) = inventory_with_tree(&source, &[], false);
        assert_eq!(parsed.parse_errors, 0, "{source}");
        assert_eq!(
            found.sites.len(),
            direct_tests + wrappers,
            "{source}: {found:?}"
        );
        assert!(retained(&found), "{source}: {found:?}");
        assert_eq!(
            found
                .sites
                .iter()
                .filter(|site| site.kind == SiteKind::Wrapper(Reach::Tasks))
                .count(),
            wrappers,
            "{source}: {found:?}"
        );
        if wrappers > 0 {
            assert!(found.escapes.is_empty(), "{source}: {found:?}");
        }
    }
}

/// Await assimilation and property handoffs retain uncertainty independently of the final call.
#[test]
fn sequence_effect_boundaries_keep_their_escape() {
    for (expression, marker) in [
        ("(0, await run)", "await run"),
        ("(0, true ? holder.value = run : run)", "value = run"),
    ] {
        let source = format!("import {{test}} from 'vitest'; function run(title, callback) {{test(title, callback)}} const holder = {{set value(fn) {{}}}}; async function collect() {{{expression}('selected', () => {{}})}} collect();");
        let offset = source.find(marker).unwrap() + marker.len() - "run".len();
        let (found, parsed) = inventory_with_tree(&source, &[], false);
        assert_eq!(parsed.parse_errors, 0);
        assert!(
            found
                .escapes
                .iter()
                .any(|escape| escape.offset as usize == offset && escape.text == "run"),
            "{source}: {found:?}"
        );
    }
}

/// Balanced result trees add linear resolver work rather than resolving once per source read.
#[test]
fn balanced_sequence_resolution_adds_linear_work() {
    let mut observations = Vec::new();
    for leaves in [4_usize, 8, 16, 32, 64] {
        let mut conditional = "run".to_owned();
        for _ in 0..leaves.ilog2() {
            conditional = format!("(flag ? {conditional} : {conditional})");
        }
        let mut work = Vec::new();
        for sequence in [false, true] {
            let callee = if sequence {
                format!("(0, {conditional})")
            } else {
                conditional.clone()
            };
            let (found, entries) = resolution_case(&callee, true);
            work.push(entries);
            if sequence {
                assert!(found.escapes.is_empty(), "leaves{leaves}: {found:?}");
            }
        }
        observations.push((leaves, work[0], work[1], work[1].saturating_sub(work[0])));
    }
    eprintln!("sequence resolver entries (leaves,direct,sequence,extra): {observations:?}");
    for (leaves, direct, sequence, extra) in observations {
        assert!(direct > 0 && sequence > 0);
        assert!(
            extra <= 8 * leaves,
            "leaves{leaves}: direct{direct}, sequence{sequence}, extra{extra}"
        );
    }
}

/// A repeatedly inspected inert sequence argument reuses its failed resolution.
#[test]
fn failed_sequence_arguments_reuse_scan_results() {
    let mut observations = Vec::new();
    for calls in [8, 16, 32, 48] {
        let mut entries = Vec::new();
        for argument in ["undefined", "(0, undefined)"] {
            let tail = "()".repeat(calls - 1);
            let source = format!(
                "import {{test}} from 'vitest';
                 function run() {{ test('inner', () => {{}}); return run; }}
                 run({argument}){tail};
                 test('retained', () => {{}});"
            );
            crate::inventory::module::take_resolutions();
            let (found, parsed) = inventory_with_tree(&source, &[], false);
            entries.push(crate::inventory::module::take_resolutions());
            assert_eq!(parsed.parse_errors, 0);
            assert_eq!(found.sites.len(), calls + 2, "{source}: {found:?}");
            assert_eq!(
                found
                    .sites
                    .iter()
                    .filter(|site| site.kind == SiteKind::Wrapper(Reach::Tasks))
                    .count(),
                calls,
                "{source}: {found:?}"
            );
            assert!(retained(&found));
        }
        observations.push((calls, entries[0], entries[1]));
    }
    eprintln!("failed sequence argument work (calls, direct, sequence): {observations:?}");
    for (calls, direct, sequence) in observations {
        assert!(direct > 0 && sequence > 0);
        assert!(
            sequence <= direct + 12,
            "calls{calls}: direct{direct}, sequence{sequence}"
        );
    }
}

/// Nested final sequences add no recursive resolution frames; wide prefixes preserve the call.
#[test]
fn sequence_resolution_work_stays_bounded() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut counts = Vec::new();
            for size in [4, 8, 12, 16] {
                let mut nested = "run".to_owned();
                for _ in 0..size {
                    nested = format!("(0, {nested})");
                }
                let wide = format!("({}, run)", vec!["0"; size * 8].join(","));
                for expression in [nested, wide] {
                    let (_, work) = resolution_case(&expression, false);
                    counts.push(work);
                }
            }
            assert!(counts.iter().all(|&count| count > 0));
            assert!(
                counts.iter().max().unwrap() - counts.iter().min().unwrap() <= 16,
                "{counts:?}"
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

/// Only recorded consumers discharge selected reads; absent calls retain uncertainty.
#[test]
fn selected_sequence_reads_use_recorded_consumers() {
    for tail in [
        "(VALUE)('selected', () => {});",
        "const held = VALUE; held('selected', () => {});",
        "export default (VALUE);",
    ] {
        let collect = |value: &str| {
            let source = format!("import {{test as api}} from 'vitest'; const flag = true; function run() {{ api('inner', () => {{}}) }} {} api('retained', () => {{}});", tail.replace("VALUE", value));
            let (found, parsed) = inventory_with_tree(&source, &[], false);
            assert_eq!(parsed.parse_errors, 0, "{source}");
            (found, source)
        };
        let (baseline, _) = collect("flag ? api : run");
        let (actual, source) = collect("flag ? (0, api) : run");
        assert!(baseline.sites.len() >= 2);
        assert_eq!(actual.sites.len(), baseline.sites.len(), "{source}");
        assert!(retained(&actual));
        let selected = source.find("(0, api)").unwrap() + "(0, ".len();
        assert_eq!(
            actual
                .escapes
                .iter()
                .filter(|escape| escape.offset as usize == selected)
                .count(),
            0,
            "{source}: {actual:?}"
        );
    }
    let source = "import {test} from 'vitest'; function run() {test('inner', () => {})} function unused() {return; (0, run)('unrecorded', () => {})} test('retained', () => {});";
    let (actual, parsed) = inventory_with_tree(source, &[], false);
    assert_eq!(parsed.parse_errors, 0);
    assert_eq!(actual.sites.len(), 2);
    assert!(retained(&actual));
    let offset = source.find("(0, run)").unwrap() + "(0, ".len();
    assert!(!actual
        .sites
        .iter()
        .any(|site| site.title.as_deref() == Some("unrecorded")));
    assert!(
        actual
            .escapes
            .iter()
            .any(|escape| escape.offset as usize == offset && escape.text == "run"),
        "{actual:?}"
    );
}
