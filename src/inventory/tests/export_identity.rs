//! Registration-site controls for terminal API binding identity and explicit precedence.
use super::*;

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
        },
        &mut Modules::default(),
    );
    assert_eq!(inventory.sites.len(), 1);
    assert_eq!(inventory.sites[0].kind, SiteKind::Wrapper(Reach::Tasks));
}
