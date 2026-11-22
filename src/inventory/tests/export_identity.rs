//! Registration-site controls for terminal API binding identity and explicit precedence.
use super::*;

/// A callable can cross initialized lexical bindings before becoming a mixed result.
#[test]
fn registration_value_aliases_reach_result_arms() {
    for api in ["test", "it"] {
        for value in ["held", "renamed", "(renamed as typeof base)"] {
            for result in [
                format!("true ? {value} : noop"),
                format!("true && {value}"),
                format!("false || {value}"),
                format!("null ?? {value}"),
            ] {
                for (export, import) in [
                    ("export const run =", "import { run }"),
                    ("export default", "import run"),
                ] {
                    let helper = format!(
                        "import {{ {api} as base }} from 'vitest';\
                         const held = base; const renamed = held; const noop = () => {{}};\
                         {export} ({result});"
                    );
                    let source = format!("{import} from './helper'; run('case', () => {{}});");
                    let (inventory, parsed) =
                        inventory_with_tree(&source, &[("helper.ts", &helper)], false);
                    assert_eq!(parsed.parse_errors, 0);
                    assert_eq!(
                        inventory
                            .sites
                            .iter()
                            .map(|site| site.kind)
                            .collect::<Vec<_>>(),
                        [SiteKind::Wrapper(Reach::Tasks)],
                        "{helper}"
                    );
                    assert!(inventory.loads.is_empty(), "{helper}");
                }
            }
        }
    }
}

/// The origin of a scalar, a shadowed read, or a consumed operand does not prove a callable.
#[test]
fn registration_value_aliases_require_an_initialized_callable_result() {
    for definitions in [
        "const held = base.name; export const run = false ? held : noop;",
        "const held = base.length; export default (false ? held : noop);",
        "const held = base; const label = held.name; export default (false ? label : noop);",
        "declare const held: typeof base; export const run = false ? held : noop;",
        "export const run = false ? held : noop; const held = base;",
        "const held = base; export const run = held && noop;",
        "const held = base; { const held = noop; const run = false ? held : noop; run(); }",
    ] {
        let helper = format!(
            "import {{ test as base }} from 'vitest'; const noop = () => {{}}; {definitions}"
        );
        let (inventory, _) = inventory_of(
            "import missing, { run } from './helper'; missing(); run();",
            &[("helper.ts", &helper)],
            false,
        );
        assert!(
            inventory.sites.is_empty() && inventory.loads.is_empty(),
            "{helper}: {inventory:?}"
        );
    }
}

/// Both API origins must settle before a mixed result can open an exact alias.
#[test]
fn registration_value_aliases_keep_exact_results() {
    let source = r"import { expect, test } from 'vitest'
const held = test
const selected = true ? held : later
const later = test
selected('retained left', () => expect(5).toBe(5))
selected('retained right', () => expect(5).toBe(5))
";
    let (inventory, parsed) = inventory_with_tree(source, &[], false);
    assert_eq!(parsed.parse_errors, 0);
    assert_eq!(inventory.sites.len(), 2, "{inventory:?}");
    assert!(
        inventory
            .sites
            .iter()
            .all(|site| site.kind == SiteKind::Registers(Registers::Test)),
        "{inventory:?}"
    );
}

/// Renamed paths preserve identity while same-category but distinct exports remain uncertain.
#[test]
fn repeated_star_bindings_require_the_same_terminal_export() {
    for (right, expected) in [
        ("test", SiteKind::Registers(Registers::Test)),
        ("it", SiteKind::Wrapper(Reach::Tasks)),
        ("describe", SiteKind::Wrapper(Reach::Tasks)),
    ] {
        let right = format!("export {{ {right} as run }} from 'vitest'");
        let files = [
            ("left.ts", "export { test as original } from 'vitest'"),
            ("renamed.ts", "export { original as run } from './left'"),
            ("right.ts", &right),
        ];
        for barrel in [
            "export * from './renamed'; export * from './right';",
            "export * from './right'; export * from './renamed';",
        ] {
            let mut files = files.to_vec();
            files.push(("barrel.ts", barrel));
            let (inventory, _) = inventory_of(
                "import { run } from './barrel'; run('case', () => {});",
                &files,
                false,
            );
            assert_eq!(inventory.sites.len(), 1);
            assert_eq!(inventory.sites[0].kind, expected, "{right}: {barrel}");
        }
    }
}

/// Explicit API selection overrides stars without confusing suite and test registration kinds.
#[test]
fn explicit_reexports_win_before_and_after_stars() {
    for kind in ["test", "describe"] {
        for barrel in [
            format!("export * from './opaque'; export {{ {kind} as run }} from 'vitest';"),
            format!("export {{ {kind} as run }} from 'vitest'; export * from './opaque';"),
            format!("export * from './api'; export {{ {kind} as run }} from 'vitest';"),
        ] {
            let files = [
                ("opaque.ts", "export function run() {}"),
                ("api.ts", "export { test as run } from 'vitest'"),
                ("barrel.ts", &barrel),
            ];
            let (inventory, _) = inventory_of(
                "import { run } from './barrel'; run('case', () => {});",
                &files,
                false,
            );
            assert_eq!(inventory.sites.len(), 1);
            assert_eq!(
                inventory.sites[0].kind,
                SiteKind::Registers(if kind == "test" {
                    Registers::Test
                } else {
                    Registers::Suite
                }),
                "{barrel}"
            );
        }
    }
}

/// Distinct dialect API modules cannot be equated merely because their names register tests.
#[test]
fn repeated_star_bindings_require_the_same_terminal_module() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.test.ts");
    let source = "import { test } from './barrel'; test('case', () => {});";
    fs::write(
        dir.path().join("barrel.ts"),
        "export * from 'first-api'; export * from 'second-api';",
    )
    .unwrap();
    fs::write(&path, source).unwrap();
    let parsed = parse(&path, source).unwrap();
    let dialect = crate::framework::Dialect {
        api_modules: &["first-api", "second-api"],
        ..DIALECT
    };
    let inventory = inventory(
        &path,
        source,
        &parsed,
        Setting {
            dialect: &dialect,
            globals: false,
            root: dir.path(),
            major: crate::framework::Formatting::Unknown,
        },
        &mut Modules::default(),
    );
    assert_eq!(inventory.sites.len(), 1);
    assert_eq!(inventory.sites[0].kind, SiteKind::Wrapper(Reach::Tasks));
}
