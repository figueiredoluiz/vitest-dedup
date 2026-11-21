//! Complete site records for the existing inventory contract, including source identity.

use super::{inventory_with_tree, Inventory, Label, Provenance, Reach, Registers, SiteKind};
use crate::lang::{Parsed, Span};

const TEST: SiteKind = SiteKind::Registers(Registers::Test);

/// Scalar builder arguments cannot multiply resolution work, or erase known registration sites.
#[test]
fn logical_builder_arguments_have_bounded_resolution_work() {
    for member in ["name", "length"] {
        for guard in ["false ||", "null ??", "true &&"] {
            for depth in [4, 8, 12, 16] {
                let mut expression = "0".to_owned();
                for _ in 0..depth {
                    expression = format!("{guard} api.skipIf({expression}).{member}");
                }
                let source = format!(
                    "import {{test as api}} from 'vitest'; const scalar = {expression};\n\
                     const chosen = true && api; chosen('uncertain', () => {{}});\n\
                     api('retained', () => {{}});"
                );
                super::super::module::take_resolutions();
                let (found, parsed) = inventory_with_tree(&source, &[], false);
                let work = super::super::module::take_resolutions();
                assert_eq!(parsed.parse_errors, 0);
                assert_eq!(found.sites.len(), 2, "{found:?}");
                assert_eq!(found.sites[0].kind, SiteKind::Wrapper(Reach::Tasks));
                assert_eq!(found.sites[1].kind, TEST);
                assert_eq!(found.sites[1].title.as_deref(), Some("retained"));
                let nodes = parsed.tree.nodes().count();
                let bound = 32 * nodes * nodes;
                assert!(work > 0 && work <= bound, "{guard} {member} depth{depth}: {work} resolutions exceed {bound} for {nodes} nodes");
            }
        }
    }
}

/// A resolved wrapper keeps its reach through a default expression just as through a binding.
#[test]
fn default_expression_wrappers_match_binding_exports() {
    let mut variants = 0;
    for expression in [
        "flag ? api : noop",
        "flag ? noop : api",
        "flag ? api.skip : noop",
        "flag ? api.extend({}) : noop",
        "flag ? noop : (flag ? api : noop)",
        "flag ? api : suite",
        "flag ? hook : noop",
    ] {
        for wrapped in [
            expression.to_owned(),
            format!("({expression}) as any"),
            format!("({expression}) satisfies unknown"),
        ] {
            for binding in [true, false] {
                let export = if binding {
                    format!("const chosen = {wrapped}; export default chosen;")
                } else {
                    format!("export default ({wrapped});")
                };
                let helper = format!("import {{ test as api, describe as suite, beforeEach as hook }} from 'vitest';\nconst flag = true; const noop = () => {{}};\n{export}");
                for import in [
                    "import held from './values';",
                    "import held from './default-barrel';",
                    "import { forwarded as held } from './named-barrel';",
                ] {
                    for controls in [false, true] {
                        let call = if expression.contains("hook") {
                            "held(() => {})"
                        } else {
                            "held('uncertain', () => {})"
                        };
                        let source = format!(
                            "{import}\nimport {{ test as direct }} from 'vitest';\n{call};\n{}",
                            if controls {
                                "direct('retained', () => {});"
                            } else {
                                ""
                            }
                        );
                        let (found, parsed) = inventory_with_tree(
                            &source,
                            &[
                                ("values.ts", &helper),
                                ("default-barrel.ts", "export { default } from './values';"),
                                (
                                    "named-barrel.ts",
                                    "export { default as forwarded } from './values';",
                                ),
                            ],
                            false,
                        );
                        assert_eq!(parsed.parse_errors, 0);
                        assert_eq!(
                            found.sites.len(),
                            1 + usize::from(controls),
                            "{helper}\n{source}: {found:?}"
                        );
                        assert_eq!(found.sites[0].kind, SiteKind::Wrapper(Reach::Tasks));
                        assert_eq!(found.sites[0].provenance, Provenance::Wrapper);
                        assert!(
                            found.loads.is_empty(),
                            "exporting the value must not call it"
                        );
                        if controls {
                            assert_eq!(found.sites[1].kind, TEST);
                        }
                        variants += 1;
                    }
                }
            }
        }
    }
    assert_eq!(variants, 252);
}

/// Scalar API properties, namespace values and unrelated expressions gain no wrapper export.
#[test]
fn default_expression_controls_preserve_absent_and_existing_results() {
    for expression in [
        "flag ? noop : noop",
        "flag ? api.name : noop",
        "flag ? api.name : api.name",
        "flag ? api.length : api.length",
        "flag ? scalar : noop",
        "flag ? namespace : noop",
        "flag ? api.unknownMember : noop",
        "flag ? unknown : noop",
        "api.name",
        "api.length",
    ] {
        let helper = format!("import {{ test as api }} from 'vitest'; import * as namespace from 'vitest';\nconst flag = true; const noop = () => {{}}; const scalar = api.name;\nexport default ({expression});");
        let (found, parsed) = inventory_with_tree(
            "import held from './values'; held('absent', () => {});",
            &[("values.ts", &helper)],
            false,
        );
        assert_eq!(parsed.parse_errors, 0);
        assert_eq!(found.sites.len(), 0, "{helper}");
        assert!(found.loads.is_empty());
    }
    for declaration in [
        "export default api;",
        "const held = api; export default held;",
        "export default function () { api('inside', () => {}); }",
    ] {
        let helper = format!("import {{ test as api }} from 'vitest'; {declaration}");
        let (found, _) = inventory_with_tree(
            "import held from './values'; held('kept', () => {});",
            &[("values.ts", &helper)],
            false,
        );
        assert_eq!(found.sites.len(), 1, "{helper}");
        assert_eq!(found.sites[0].kind, SiteKind::Wrapper(Reach::Tasks));
    }
}

/// Exact API joins survive every insertion point in a dependency-ordered alias chain.
#[test]
fn conditional_exact_aliases_do_not_depend_on_declaration_order() {
    let mut variants = 0;
    for depth in 1..=3 {
        let declarations: Vec<_> = (0..depth)
            .map(|index| {
                let value = if index == 0 {
                    "api".to_owned()
                } else {
                    format!("arm{}", index - 1)
                };
                format!("const arm{index} = {value};")
            })
            .collect();
        for position in 0..=depth {
            for reverse in [false, true] {
                for nested in [false, true] {
                    for callee in ["chosen", "held"] {
                        let later = format!("arm{}", depth - 1);
                        let (left, right) = if reverse {
                            (later, "api".to_owned())
                        } else {
                            ("api".to_owned(), later)
                        };
                        let expression = format!("flag ? {left} : {right}");
                        let expression = if nested {
                            format!("flag ? ({expression}) : ({expression})")
                        } else {
                            expression
                        };
                        let mut statements = declarations.clone();
                        statements.insert(position, format!("const chosen = {expression};"));
                        let source = format!(
                            "import {{ test as api }} from 'vitest';\nconst flag = {};\n{}\n\
                             const held = chosen; {callee}('kept', () => {{}});",
                            !reverse,
                            statements.join("\n")
                        );
                        let (found, parsed) = inventory_with_tree(&source, &[], false);
                        assert_eq!(parsed.parse_errors, 0);
                        assert_eq!(found.sites.len(), 1, "{source}: {found:?}");
                        assert_eq!(found.sites[0].kind, TEST, "{source}");
                        assert_eq!(found.sites[0].provenance, Provenance::Direct, "{source}");
                        assert_eq!(found.sites[0].title.as_deref(), Some("kept"));
                        assert!(found.loads.is_empty());
                        variants += 1;
                    }
                }
            }
        }
    }
    assert_eq!(variants, 72);
}

/// An unproved result arm cannot erase a registration capability at any supported binding route.
#[test]
fn branch_capabilities_survive_binding_and_export_routes() {
    let mut variants = 0;
    for expression in [
        "flag ? api : noop",
        "flag ? noop : api",
        "flag ? api : undefined",
        "flag ? 0 : api",
        "flag ? api.skip : noop",
        "flag ? api.extend({}) : noop",
        "flag ? noop : (flag ? api : noop)",
        "flag ? api : suite",
        "flag ? hook : noop",
        "flag && api",
        "false || api",
        "undefined ?? api",
        "api || noop",
        "api ?? noop",
        "(flag && api) || noop",
        "api && api",
        "flag && api.skip",
        "flag && api.extend({})",
        "flag && hook",
        "flag && (flag ? api : noop)",
        "flag && (flag ? api : api)",
        "(flag ? api : api) || noop",
        "(flag ? api : api) ?? noop",
        "flag && (flag ? api.name : api)",
        "flag && (flag ? noop : (flag ? api : api))",
        "flag && ((flag ? api : api) as any)",
        "(flag && api) as any",
        "(null ?? api) satisfies unknown",
        "(api || noop)!",
        "flag && import.meta.vitest.test",
        "false || import.meta.vitest.it",
        "null ?? import.meta.vitest.test.skipIf(false)",
        "flag && (flag ? import.meta.vitest.test : import.meta.vitest.test)",
        "flag ? import.meta.vitest.test : noop",
        "flag ? noop : import.meta.vitest.test",
    ] {
        let header =
            "import { test as api, describe as suite, beforeEach as hook } from 'vitest';\n\
                      const flag = true; const noop = () => {};";
        for route in 0..7 {
            let helper = if route == 6 {
                format!("{header}\nexport default ({expression});")
            } else {
                format!("{header}\nconst chosen = {expression};\nexport {{ chosen }}; export default chosen;")
            };
            let barrel = "export { chosen as forwarded } from './values';";
            let (setup, callee) = match route {
                0 => (header.to_owned(), format!("({expression})")),
                1 => (
                    format!("{header}\nconst chosen = {expression}; const held = chosen;"),
                    "held".to_owned(),
                ),
                2 => (
                    "import { chosen as held } from './values';".to_owned(),
                    "held".to_owned(),
                ),
                3 | 6 => ("import held from './values';".to_owned(), "held".to_owned()),
                4 => (
                    "import { forwarded as held } from './barrel';".to_owned(),
                    "held".to_owned(),
                ),
                _ => (
                    "import { chosen as held } from './star';".to_owned(),
                    "held".to_owned(),
                ),
            };
            for controls in [false, true] {
                let hook = expression.contains("hook");
                let call = if hook {
                    format!("{callee}(() => {{}})")
                } else {
                    format!("{callee}('uncertain', () => {{}})")
                };
                let source = format!(
                    "{setup}\nimport {{ test as direct }} from 'vitest';\n{call};\n{}",
                    if controls {
                        "direct('kept', () => {});"
                    } else {
                        ""
                    }
                );
                let (found, parsed) = inventory_with_tree(
                    &source,
                    &[
                        ("values.ts", &helper),
                        ("barrel.ts", barrel),
                        ("star.ts", "export * from './values';"),
                    ],
                    false,
                );
                assert_eq!(parsed.parse_errors, 0, "{source}");
                assert_eq!(
                    found.sites.len(),
                    1 + usize::from(controls),
                    "{source}: {found:?}"
                );
                let site = &found.sites[0];
                assert_eq!(site.kind, SiteKind::Wrapper(Reach::Tasks), "{source}");
                assert_eq!(site.provenance, Provenance::Wrapper, "{source}");
                assert_eq!(
                    &source[site.span.start as usize..site.span.end as usize],
                    call
                );
                assert_eq!(
                    site.title.as_deref(),
                    if hook { None } else { Some("uncertain") }
                );
                assert!(site.member.is_none());
                assert!(
                    found.loads.is_empty(),
                    "exporting a capability must not register: {source}"
                );
                if controls {
                    assert_eq!(found.sites[1].kind, TEST);
                    assert_eq!(found.sites[1].title.as_deref(), Some("kept"));
                }
                variants += 1;
            }
        }
    }
    assert_eq!(variants, 490);
}

/// A consumed AND-left value, scalar alias or namespace supplies no callable result proof.
#[test]
fn logical_non_callable_candidates_preserve_direct_controls() {
    for expression in [
        "flag && import.meta.vitest",
        "false || import.meta.vitest.name",
        "null ?? import.meta.vitest.test.name",
        "flag && import.meta.vitest.test.length",
        "flag && import.meta.vitest.test.unknownMember",
        "import.meta.vitest.test && noop",
        "flag && import.meta.unrelated.test",
        "flag ? import.meta.vitest.test.name : noop",
        "api && noop",
        "api && 0",
        "api && api.name",
        "api && namespace",
        "false || api.name",
        "null ?? api.length",
        "flag && scalar",
        "scalar || noop",
        "scalar ?? noop",
        "flag && namespace",
        "flag && api.unknownMember",
        "flag && unknown",
        "flag && noop",
        "flag && (flag ? api.name : api.length)",
        "(flag ? api.name : api.length) || noop",
        "(flag ? api.name : api.length) ?? noop",
        "(flag ? api : api) && noop",
        "flag && (flag ? namespace : namespace)",
        "flag && (flag ? unknown : unknown)",
    ] {
        let helper = format!("import {{ test as api }} from 'vitest'; import * as namespace from 'vitest';\nconst flag = true; const noop = () => {{}}; const scalar = api.name;\nexport default ({expression});");
        let (found, parsed) = inventory_with_tree(
            "import chosen from './values'; import { test } from 'vitest'; chosen('absent', () => {}); test('retained', () => {});",
            &[("values.ts", &helper)], false,
        );
        assert_eq!(parsed.parse_errors, 0);
        assert_eq!(found.sites.len(), 1, "{helper}: {found:?}");
        assert_eq!(found.sites[0].kind, TEST);
        assert_eq!(found.sites[0].title.as_deref(), Some("retained"));
        assert!(found.loads.is_empty());
    }
    for header in [
        "import type { test as api } from 'vitest';",
        "declare const api: any;",
        "const api = () => {};",
    ] {
        let helper = format!("{header} export const chosen = true && api;");
        let (found, _) = inventory_with_tree(
            "import { chosen } from './values'; chosen('absent', () => {});",
            &[("values.ts", &helper)],
            false,
        );
        assert!(found.sites.is_empty(), "{helper}: {found:?}");
        assert!(found.loads.is_empty());
    }
}

/// Equal API arms remain exact; non-result references and unrelated values establish no root.
#[test]
fn conditional_capability_controls_keep_exact_and_absent_results() {
    for (expression, expected) in [
        ("flag ? api : api.skip", Some(TEST)),
        ("flag ? noop : noop", None),
        ("api ? noop : noop", None),
        ("flag ? api.name : noop", None),
        ("flag ? api.length : 0", None),
        ("flag ? scalar : noop", None),
        ("flag ? namespace : noop", None),
        ("flag ? api.unknownMember : noop", None),
        ("flag ? unknown : noop", None),
    ] {
        let source = format!(
            "import {{ test as api }} from 'vitest'; import * as namespace from 'vitest';\n\
            const flag = true; const noop = () => {{}}; const scalar = api.name;\n\
            const chosen = {expression};\nchosen('case', () => {{}});"
        );
        let (found, parsed) = inventory_with_tree(&source, &[], false);
        assert_eq!(parsed.parse_errors, 0);
        assert_eq!(
            found.sites.len(),
            usize::from(expected.is_some()),
            "{source}"
        );
        if let Some(kind) = expected {
            assert_eq!(found.sites[0].kind, kind);
            assert_eq!(found.sites[0].provenance, Provenance::Direct);
        }
    }
}

/// Escape policy keeps exact anchors across API bindings, syntax and direct retention controls.
#[test]
fn escape_roles_preserve_records_and_direct_controls() {
    for (statement, escaped, calls) in [
        ("VALUE;", Some("VALUE"), 0),
        ("VALUE.skip;", Some("VALUE.skip"), 0),
        ("VALUE[key];", Some("VALUE[key]"), 0),
        ("sink(VALUE.skip);", Some("VALUE.skip"), 0),
        ("sink(VALUE as any);", Some("VALUE as any"), 0),
        ("VALUE satisfies any;", Some("VALUE satisfies any"), 0),
        ("[VALUE];", Some("VALUE"), 0),
        ("({ value: VALUE });", Some("VALUE"), 0),
        ("true ? VALUE : 0;", Some("VALUE"), 0),
        ("VALUE === 0;", Some("VALUE"), 0),
        ("`${VALUE}`;", Some("VALUE"), 0),
        ("target = VALUE;", Some("VALUE"), 0),
        ("(VALUE, 0);", Some("VALUE"), 0),
        ("(0, VALUE)('case', () => {});", Some("VALUE"), 0),
        ("(<any>VALUE)('case', () => {});", Some("VALUE"), 1),
        ("(VALUE<any>)('case', () => {});", Some("VALUE"), 1),
        ("VALUE<any>('case', () => {});", None, 1),
        ("typeof VALUE;", None, 0),
        ("const held = VALUE;", None, 0),
        ("export default VALUE;", None, 0),
        ("VALUE('case', () => {});", None, 1),
        ("(VALUE)('case', () => {});", None, 1),
        ("(VALUE as any)('case', () => {});", None, 1),
        ("VALUE!('case', () => {});", None, 1),
        ("VALUE.each`n\n${1}`('case', () => {});", None, 1),
    ] {
        for (import, value) in [
            ("import { test as api } from 'vitest';", "api"),
            ("import * as v from 'vitest';", "v.test"),
        ] {
            for control in [false, true] {
                let statement = statement.replace("VALUE", value);
                let source = format!(
                    "{import}\nimport {{ test as direct }} from 'vitest';\nlet target;\n{statement}\n{}",
                    if control { "direct('kept', () => {});" } else { "" }
                );
                let (found, parsed) = inventory_with_tree(&source, &[], false);
                assert_eq!(parsed.parse_errors, 0, "{source}");
                assert_eq!(
                    found.sites.len(),
                    calls + usize::from(control),
                    "{source}: {found:?}"
                );
                assert!(found.sites.iter().all(|site| site.kind == TEST), "{source}");
                if control {
                    assert_eq!(found.sites.last().unwrap().title.as_deref(), Some("kept"));
                }
                assert_eq!(
                    found.escapes.len(),
                    usize::from(escaped.is_some()),
                    "{source}: {found:?}"
                );
                if let Some(anchor) = escaped {
                    let anchor = anchor.replace("VALUE", value);
                    let escape = &found.escapes[0];
                    assert_eq!(escape.text, anchor, "{source}");
                    assert_eq!(
                        escape.offset as usize,
                        source.find(&statement).unwrap() + statement.find(&anchor).unwrap(),
                        "{source}"
                    );
                }
            }
        }
    }
    for statement in ["export { api };", "export type { api };"] {
        let source = format!("import {{ test as api }} from 'vitest';\n{statement}");
        let (found, parsed) = inventory_with_tree(&source, &[], false);
        assert_eq!(parsed.parse_errors, 0);
        assert!(found.sites.is_empty());
        assert!(found.escapes.is_empty(), "{source}: {found:?}");
    }
}

/// Result positions keep factory accounting across declaration forms and module boundaries.
#[test]
fn return_positions_preserve_factory_records_and_direct_controls() {
    for (expression, factory) in [
        ("base", true),
        ("base.skip", true),
        ("base.extend({})", true),
        ("(base as any)", true),
        ("<any>base", true),
        ("base<any>", true),
        ("flag ? base : 0", true),
        ("base ? 1 : 0", false),
        ("0 && base", true),
        ("base && 0", false),
        ("base || 0", true),
        ("base ?? 0", true),
        ("0, base", true),
        ("base, 0", false),
        ("[base]", true),
        ("{ value: base }", true),
        ("target ??= base", true),
        ("target += base", false),
        ("typeof base", false),
        ("base === 0", false),
        ("`${base}`", false),
        ("String(base)", false),
        ("null as unknown as typeof base", false),
        ("alias", false),
        ("() => base", false),
        ("base.name", false),
    ] {
        for form in 0..3 {
            let declaration = match form {
                0 => format!("function make() {{ return ({expression}); }}"),
                1 => format!("const make = () => {{ return ({expression}); }};"),
                _ => format!("const make = () => ({expression});"),
            };
            for imported in [false, true] {
                let helper = format!(
                    "import {{ test as base }} from 'vitest';\n\
                     const alias = base; let target; const flag = true;\n{}\n{declaration}",
                    if imported { "export { make };" } else { "" }
                );
                let source = format!(
                    "import {{ test as control }} from 'vitest';\n{}\n\
                     make(0);\ncontrol('kept', () => {{}})",
                    if imported {
                        "import { make } from './factory';"
                    } else {
                        &helper
                    }
                );
                let mut expected = Vec::new();
                if factory {
                    expected.push(Record {
                        provenance: Provenance::Wrapper,
                        ..Record::call("make(0)", "make(0)", SiteKind::Wrapper(Reach::Tasks))
                    });
                }
                expected.push(Record {
                    title: Some("kept"),
                    ..Record::call("control('kept', () => {})", "control(", TEST)
                });
                records(&source, &[("factory.ts", &helper)], false, &expected);
            }
        }
    }
}

/// The same owned sources used by the collection oracle keep their direct and wrapper controls.
#[test]
fn owned_return_sources_keep_their_registration_kinds() {
    let helper = include_str!("../../../fixtures/oracle/factory-returned-api/factories.ts");
    for (source, kinds) in [
        (
            include_str!("../../../fixtures/oracle/factory-returned-api/control.test.ts"),
            &[TEST, TEST, TEST][..],
        ),
        (
            include_str!("../../../fixtures/oracle/factory-returned-api/plain.test.ts"),
            &[SiteKind::Wrapper(Reach::Tasks); 2][..],
        ),
    ] {
        let source = source.replace("\r\n", "\n");
        let (found, parsed) = inventory_with_tree(&source, &[("factories.ts", helper)], false);
        assert_eq!(parsed.parse_errors, 0);
        assert_eq!(found.sites.len(), kinds.len(), "{source}: {found:?}");
        assert_eq!(
            found.sites.iter().map(|site| site.kind).collect::<Vec<_>>(),
            kinds
        );
    }
}

/// A type-only namespace never becomes a runtime alias; a runtime import remains a control.
#[test]
fn binding_transport_does_not_create_values_for_type_only_imports() {
    let source = "import type * as erased from 'vitest';\n\
                  import { test } from 'vitest';\n\
                  const namespace = erased;\nconst { test: held } = namespace;\n\
                  held('erased', () => {});\ntest('control', () => {})";
    let (found, _) = records(
        source,
        &[],
        false,
        &[Record {
            title: Some("control"),
            ..Record::call("test('control', () => {})", "test('control'", TEST)
        }],
    );
    assert!(found.registration_values.is_empty());
    assert!(found.escapes.is_empty());
}

/// Alias depth and declaration order do not confuse separately resolved same-spelling bindings.
#[test]
fn binding_transport_keeps_scope_identity_across_alias_fixpoints() {
    for expression in ["base", "(base as any)", "api.test"] {
        for depth in 1..=3 {
            for reverse in [false, true] {
                let mut bindings: Vec<_> = (0..depth)
                    .map(|index| {
                        let input = if index == 0 {
                            expression.to_owned()
                        } else {
                            format!("held{}", index - 1)
                        };
                        format!("const held{index} = {input};")
                    })
                    .collect();
                if reverse {
                    bindings.reverse();
                }
                let name = format!("held{}", depth - 1);
                let call = format!("{name}('case', () => {{}})");
                let source = format!(
                    "import {{ test as base }} from 'vitest';\n\
                     import * as api from 'vitest';\n{}\n\
                     {{ const {name} = () => {{}}; {name}('shadow', () => {{}}); }}\n{call}",
                    bindings.join("\n")
                );
                let (found, _) = records(
                    &source,
                    &[],
                    false,
                    &[Record {
                        title: Some("case"),
                        ..Record::call(&call, &call, TEST)
                    }],
                );
                assert_eq!(found.registration_values.len(), depth, "{source}");
                assert!(found.escapes.is_empty(), "{source}: {found:?}");
            }
        }
    }
}

/// Namespace projections transport the recorded uncertainty through subsequent aliases.
#[test]
fn binding_transport_preserves_write_and_redeclaration_uncertainty() {
    for (binding, uncertain, escaped) in [
        ("const namespace = api;", false, None),
        (
            "type namespace = number; const namespace = api;",
            false,
            None,
        ),
        ("const namespace = api; interface namespace {}", false, None),
        (
            "let namespace = api; namespace = other;",
            true,
            Some("namespace = other"),
        ),
        (
            "let namespace = api; namespace ||= other;",
            true,
            Some("namespace ||= other"),
        ),
        ("var namespace = api; var namespace = other;", true, None),
    ] {
        let source = format!(
            "import * as api from 'vitest';\n\
             import {{ test as control }} from 'vitest';\n{binding}\n\
             const projected = namespace.test;\nconst held = projected;\n\
             held('case', () => {{}});\ncontrol('control', () => {{}})"
        );
        let (found, _) = records(
            &source,
            &[],
            false,
            &[
                Record {
                    title: Some("case"),
                    provenance: if uncertain {
                        Provenance::Wrapper
                    } else {
                        Provenance::Direct
                    },
                    ..Record::call(
                        "held('case', () => {})",
                        "held(",
                        if uncertain {
                            SiteKind::Wrapper(Reach::Tasks)
                        } else {
                            TEST
                        },
                    )
                },
                Record {
                    title: Some("control"),
                    ..Record::call("control('control', () => {})", "control(", TEST)
                },
            ],
        );
        assert_eq!(
            found.registration_values.len(),
            if uncertain { 0 } else { 2 },
            "{source}"
        );
        assert_eq!(
            found.escapes.len(),
            usize::from(escaped.is_some()),
            "{source}"
        );
        if let Some(anchor) = escaped {
            assert_eq!(found.escapes[0].text, "namespace", "{source}");
            assert_eq!(found.escapes[0].offset, offset(&source, anchor), "{source}");
        }
    }
}

/// Pattern bindings retain conservative wrapper reach without widening namespace projection.
#[test]
fn binding_transport_distinguishes_wrapper_patterns_from_namespace_patterns() {
    let helper = "import { beforeEach } from 'vitest';\n\
                  export function setup() { beforeEach(() => {}) }";
    for pattern in [
        "{ test: held }",
        "[held]",
        "{ nested: { test: held } }",
        "{ test: held = fallback }",
        "{ ...held }",
        "[, ...held]",
    ] {
        let source = format!(
            "import {{ setup }} from './setup';\n\
             const bare = setup;\nconst {pattern} = setup;\n\
             bare();\nheld('case', () => {{}})"
        );
        let (found, _) = records(
            &source,
            &[("setup.ts", helper)],
            false,
            &[
                Record {
                    provenance: Provenance::Wrapper,
                    ..Record::call("bare()", "bare()", SiteKind::Wrapper(Reach::Hooks))
                },
                Record {
                    title: Some("case"),
                    provenance: Provenance::Wrapper,
                    ..Record::call(
                        "held('case', () => {})",
                        "held(",
                        SiteKind::Wrapper(Reach::Tasks),
                    )
                },
            ],
        );
        assert!(found.registration_values.is_empty());
        assert!(found.escapes.is_empty(), "{source}: {found:?}");

        // Only a shallow, non-defaulted static property transports a namespace API member.
        let source = format!(
            "import * as api from 'vitest';\n\
             const {pattern} = api;\nheld('case', () => {{}});\n\
             api.test('control', () => {{}})"
        );
        let mut expected = Vec::new();
        if pattern == "{ test: held }" {
            expected.push(Record {
                title: Some("case"),
                ..Record::call("held('case', () => {})", "held(", TEST)
            });
        }
        expected.push(Record {
            title: Some("control"),
            chain: &["test"],
            ..Record::call("api.test('control', () => {})", "test('control'", TEST)
        });
        let (found, _) = records(&source, &[], false, &expected);
        assert!(found.registration_values.is_empty());
        assert!(found.escapes.is_empty(), "{source}: {found:?}");
    }
}

/// Direct and aliased values keep one registration kind but retain their distinct chain boundary.
#[test]
fn callee_value_projections_preserve_direct_and_alias_records() {
    let suite = SiteKind::Registers(Registers::Suite);
    let wrapper = SiteKind::Wrapper(Reach::Tasks);
    for (expression, kind, chain, member) in [
        ("test", TEST, &[][..], None),
        ("test.skip", TEST, &["skip"][..], None),
        ("test.describe", suite, &[][..], Some("test.describe")),
        (
            "test.skip.describe",
            suite,
            &[][..],
            Some("test.skip.describe"),
        ),
        (
            "test.skip['describe']",
            suite,
            &[][..],
            Some("test.skip['describe']"),
        ),
        (
            "(test as any).describe",
            suite,
            &[][..],
            Some("(test as any).describe"),
        ),
        ("test.each([[1]])", TEST, &["each", "()"][..], None),
        (
            "test.describe.each([[1]])",
            suite,
            &["each", "()"][..],
            Some("test.describe"),
        ),
        (
            "test.describe.each`n\n${1}`",
            suite,
            &["each", "()"][..],
            Some("test.describe"),
        ),
        (
            "test['skip'].describe",
            wrapper,
            &["skip", "describe"][..],
            None,
        ),
    ] {
        for alias in [false, true] {
            let call = format!(
                "{}('case', () => {{}})",
                if alias { "held" } else { expression }
            );
            let binding = if alias {
                format!("const held = {expression};\n")
            } else {
                String::new()
            };
            let source = format!("import {{ test }} from 'vitest';\n{binding}{call}");
            let (found, parsed) = inventory_with_tree(&source, &[], false);
            assert_eq!(found.sites.len(), 1, "{source}: {found:?}");
            let site = &found.sites[0];
            assert_eq!(site.kind, kind, "{source}");
            assert_eq!(site.chain, if alias { &[][..] } else { chain }, "{source}");
            assert_eq!(
                site.provenance,
                if kind == wrapper {
                    Provenance::Wrapper
                } else {
                    Provenance::Direct
                },
                "{source}"
            );
            assert_eq!(site.title.as_deref(), Some("case"), "{source}");
            assert_eq!(site.span, span(&source, &call), "{source}");
            assert_eq!(
                site.member.map(|id| parsed.tree.span(id)),
                if alias {
                    None
                } else {
                    member.map(|text| span(&source, text))
                },
                "{source}"
            );
            assert_eq!(
                found.registration_values.len(),
                usize::from(alias && kind != wrapper),
                "{source}"
            );
            assert!(found.escapes.is_empty(), "{source}: {found:?}");
        }
    }
}

/// Equivalent spellings preserve the superseding member's exact anchor and the applied chain.
#[test]
fn structural_chain_spellings_keep_registration_records() {
    for prefix in [
        "test.skip.describe",
        "test.skip['describe']",
        "(test.skip as any).describe",
        "((test.skip)!).describe",
    ] {
        for (suffix, chain) in [
            ("", &[][..]),
            (".each([[1]])", &["each", "()"] as &[_]),
            (".each`n\n${1}`", &["each", "()"] as &[_]),
        ] {
            let call = format!("{prefix}{suffix}('case', () => {{}})");
            let source = format!("import {{ test }} from 'vitest'\n{call}");
            let (found, parsed) = inventory_with_tree(&source, &[], false);
            assert_eq!(found.sites.len(), 1, "{source}: {found:?}");
            let site = &found.sites[0];
            assert_eq!(site.kind, SiteKind::Registers(Registers::Suite), "{source}");
            assert_eq!(site.chain, chain, "{source}");
            assert_eq!(site.provenance, Provenance::Direct, "{source}");
            assert_eq!(site.title.as_deref(), Some("case"), "{source}");
            assert_eq!(site.span, span(&source, &call), "{source}");
            assert_eq!(
                site.member.map(|id| parsed.tree.span(id)),
                Some(span(&source, prefix)),
                "{source}"
            );
            assert!(site.labels.is_empty());
            assert!(found.escapes.is_empty(), "{source}");
            assert!(found.registration_values.is_empty(), "{source}");
        }
    }
    let (evaluated, _) = inventory_with_tree(
        "import { test } from 'vitest'; test['skip'].describe('case', () => {})",
        &[],
        false,
    );
    assert_eq!(evaluated.sites.len(), 1);
    assert_eq!(evaluated.sites[0].kind, SiteKind::Wrapper(Reach::Tasks));
    assert_eq!(evaluated.sites[0].provenance, Provenance::Wrapper);
    assert_eq!(evaluated.sites[0].chain, ["skip", "describe"]);
    assert!(evaluated.sites[0].member.is_none());
    let (unknown, _) = inventory_with_tree(
        "import { test } from 'vitest'; test[key]('case', () => {})",
        &[],
        false,
    );
    assert!(unknown.sites.is_empty(), "{unknown:?}");
}

/// The written call and every public field downstream consumers read.
struct Record<'a> {
    call: &'a str,
    kind: SiteKind,
    chain: &'a [&'a str],
    member: Option<&'a str>,
    provenance: Provenance,
    title: Option<&'a str>,
    position: &'a str,
    from: &'a str,
    labels: &'a [Label],
}

impl<'a> Record<'a> {
    /// A direct call with no chain, member, title or surrounding lexical labels.
    fn call(call: &'a str, position: &'a str, kind: SiteKind) -> Self {
        Self {
            call,
            kind,
            chain: &[],
            member: None,
            provenance: Provenance::Direct,
            title: None,
            position,
            from: position,
            labels: &[],
        }
    }
}

/// The unique written anchor; repeated fragments cannot silently select the wrong node.
fn offset(source: &str, anchor: &str) -> u32 {
    assert_eq!(
        source.matches(anchor).count(),
        1,
        "ambiguous anchor: {anchor}"
    );
    source.find(anchor).unwrap().try_into().unwrap()
}

/// A written fragment's exact source range.
fn span(source: &str, fragment: &str) -> Span {
    let start = offset(source, fragment);
    Span {
        start,
        end: start + u32::try_from(fragment.len()).unwrap(),
    }
}

/// Counts first, then compares each record against independently written source anchors.
fn records(
    source: &str,
    files: &[(&str, &str)],
    globals: bool,
    expected: &[Record<'_>],
) -> (Inventory, Parsed) {
    let (found, parsed) = inventory_with_tree(source, files, globals);
    assert_eq!(parsed.parse_errors, 0, "{source}");
    assert_eq!(found.sites.len(), expected.len(), "{:#?}", found.sites);
    for (site, want) in found.sites.iter().zip(expected) {
        assert_eq!(site.span, span(source, want.call), "{}", want.call);
        assert_eq!(parsed.tree.span(site.node), site.span, "{}", want.call);
        assert_eq!(
            parsed.tree.kind(site.node),
            "CallExpression",
            "{}",
            want.call
        );
        assert_eq!(site.kind, want.kind, "{}", want.call);
        assert_eq!(site.chain, want.chain, "{}", want.call);
        assert_eq!(site.provenance, want.provenance, "{}", want.call);
        assert_eq!(site.title.as_deref(), want.title, "{}", want.call);
        assert_eq!(
            site.position,
            offset(source, want.position),
            "{}",
            want.call
        );
        assert_eq!(site.from, offset(source, want.from), "{}", want.call);
        assert_eq!(site.labels, want.labels, "{}", want.call);
        assert_eq!(
            site.member.map(|id| parsed.tree.span(id)),
            want.member.map(|text| span(source, text)),
            "{}",
            want.call
        );
    }
    (found, parsed)
}

/// Calls retain their chain, registration member, mapped position and exact alias-value node.
#[test]
fn direct_namespace_alias_and_table_records_keep_their_dimensions() {
    let source = "import { test as base } from 'vitest'\n\
        import * as v from 'vitest'\n\
        const local = base\n\
        local('alias', () => {})\n\
        base('direct', () => {})\n\
        base.skip('skipped', () => {})\n\
        v.it('namespace', () => {})\n\
        base.each([[1]])('row %s', (n) => {})\n\
        base.describe('suite', () => {})";
    let (found, parsed) = records(
        source,
        &[],
        false,
        &[
            Record {
                title: Some("alias"),
                ..Record::call("local('alias', () => {})", "local(", TEST)
            },
            Record {
                title: Some("direct"),
                ..Record::call("base('direct', () => {})", "base('direct'", TEST)
            },
            Record {
                title: Some("skipped"),
                chain: &["skip"],
                ..Record::call("base.skip('skipped', () => {})", "skip(", TEST)
            },
            Record {
                title: Some("namespace"),
                chain: &["it"],
                ..Record::call("v.it('namespace', () => {})", "it(", TEST)
            },
            Record {
                title: Some("row %s"),
                chain: &["each", "()"],
                from: "base.each",
                ..Record::call("base.each([[1]])('row %s', (n) => {})", "('row %s'", TEST)
            },
            Record {
                title: Some("suite"),
                member: Some("base.describe"),
                ..Record::call(
                    "base.describe('suite', () => {})",
                    "describe(",
                    SiteKind::Registers(Registers::Suite),
                )
            },
        ],
    );
    assert!(found.escapes.is_empty());
    assert_eq!(found.registration_values.len(), 1);
    let value = *found.registration_values.iter().next().unwrap();
    let start = offset(source, "base\nlocal");
    assert_eq!(
        parsed.tree.span(value),
        Span {
            start,
            end: start + 4
        }
    );
}

/// Imported API values and user helpers must not collapse Derived into Wrapper provenance.
#[test]
fn imported_values_and_helpers_keep_provenance_and_hook_reach() {
    let source = "import { derived, hook, setup, register } from './helpers'\n\
        derived('derived', () => {})\n\
        hook(() => {})\n\
        setup()\n\
        register('wrapped', () => {})";
    let helpers = "import { test, beforeEach } from 'vitest'\n\
        export const derived = test\n\
        export const hook = beforeEach\n\
        export function setup() { beforeEach(() => {}) }\n\
        export function register(name, fn) { test(name, fn) }";
    let (found, _) = records(
        source,
        &[("helpers.ts", helpers)],
        false,
        &[
            Record {
                title: Some("derived"),
                provenance: Provenance::Derived,
                ..Record::call(
                    "derived('derived', () => {})",
                    "derived(",
                    SiteKind::Wrapper(Reach::Tasks),
                )
            },
            Record {
                provenance: Provenance::Derived,
                ..Record::call("hook(() => {})", "hook(", SiteKind::Wrapper(Reach::Hook))
            },
            Record {
                provenance: Provenance::Wrapper,
                ..Record::call("setup()", "setup(", SiteKind::Wrapper(Reach::Hooks))
            },
            Record {
                title: Some("wrapped"),
                provenance: Provenance::Wrapper,
                ..Record::call(
                    "register('wrapped', () => {})",
                    "register(",
                    SiteKind::Wrapper(Reach::Tasks),
                )
            },
        ],
    );
    assert!(found.escapes.is_empty());
    assert!(found.registration_values.is_empty());
}

/// Compatibility baseline only: main accounts for both the factory and its returned callable.
/// Changing this classification requires the separate returned-registration contract.
#[test]
fn returned_factory_records_describe_current_wrapper_accounting() {
    let source =
        "import { make } from './factory'\nconst held = make()\nheld('returned', () => {})";
    let factory = "import { test } from 'vitest'; export function make() { return test }";
    let (found, _) = records(
        source,
        &[("factory.ts", factory)],
        false,
        &[
            Record {
                provenance: Provenance::Wrapper,
                ..Record::call("make()", "make(", SiteKind::Wrapper(Reach::Tasks))
            },
            Record {
                title: Some("returned"),
                provenance: Provenance::Wrapper,
                ..Record::call(
                    "held('returned', () => {})",
                    "held(",
                    SiteKind::Wrapper(Reach::Tasks),
                )
            },
        ],
    );
    assert!(found.escapes.is_empty());
    assert!(found.registration_values.is_empty());
}

/// The same owned oracle source retains lexical labels and hooks-only wrapper records.
#[test]
fn owned_hook_bindings_keep_their_enclosing_registration_labels() {
    let source =
        include_str!("../../../fixtures/oracle/binding-stability/annotated-binding.test.ts")
            .replace("\r\n", "\n");
    let source = source.as_str();
    let helper = include_str!("../../../fixtures/oracle/binding-stability/annotated-binding.ts");
    let (found, _) = records(source, &[("annotated-binding.ts", helper)], false, &[
        Record { title: Some("annotated-binding"), ..Record::call("describe('annotated-binding', () => {\n  closed()\n  annotated()\n  test('stays comparable', () => {\n    expect(1).toBe(1)\n  })\n})", "describe(", SiteKind::Registers(Registers::Suite)) },
        Record { provenance: Provenance::Wrapper, labels: &[Label::RegistrationBody], ..Record::call("closed()", "closed(", SiteKind::Wrapper(Reach::Hooks)) },
        Record { provenance: Provenance::Wrapper, labels: &[Label::RegistrationBody], ..Record::call("annotated()", "annotated(", SiteKind::Wrapper(Reach::Hooks)) },
        Record { title: Some("stays comparable"), labels: &[Label::RegistrationBody], ..Record::call("test('stays comparable', () => {\n    expect(1).toBe(1)\n  })", "test(", TEST) },
    ]);
    assert!(found.escapes.is_empty());
    assert!(found.registration_values.is_empty());
}

/// A consumed read is inert, while a hand-off retains the exact escaped expression and offset.
#[test]
fn consumed_and_escaped_registration_reads_remain_distinct() {
    let source =
        "import { test as api } from 'vitest'\napi('kept', () => {})\ntypeof api\nrun(api.skip)";
    let (found, _) = records(
        source,
        &[],
        false,
        &[Record {
            title: Some("kept"),
            ..Record::call("api('kept', () => {})", "api(", TEST)
        }],
    );
    assert_eq!(found.escapes.len(), 1);
    assert_eq!(found.escapes[0].text, "api.skip");
    assert_eq!(found.escapes[0].offset, offset(source, "api.skip"));
    assert!(found.registration_values.is_empty());
}

/// Free framework names keep their explicit global provenance.
#[test]
fn free_registration_names_keep_global_provenance() {
    let source = "test('global', () => {})";
    let (found, _) = records(
        source,
        &[],
        true,
        &[Record {
            title: Some("global"),
            provenance: Provenance::Global,
            ..Record::call(source, "test(", TEST)
        }],
    );
    assert!(found.escapes.is_empty());
    assert!(found.registration_values.is_empty());
}
