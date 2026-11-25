//! Fresh array selection keeps the existing registration boundary.

use super::*;
use crate::inventory::Site;

/// Inspect the final sites for a route beside an independent registration.
fn sites(imports: &str, expression: &str) -> Vec<Site> {
    let source = format!(
        "{imports} import {{test as control}} from 'vitest'; {expression}; control('retained', () => {{}});"
    );
    let (found, parsed) = inventory_with_tree(&source, &[], false);
    assert_eq!(parsed.parse_errors, 0, "{source}");
    assert!(
        found.sites.iter().any(|site| {
            site.title.as_deref() == Some("retained")
                && site.kind == SiteKind::Registers(Registers::Test)
        }),
        "{source}: {found:?}"
    );
    found.sites
}

/// Every occupied own slot with the fixture-extension result gets one conservative site.
#[test]
fn own_array_slots_account_for_immediate_fixture_calls() {
    let imports = "import {test as api} from 'vitest'; import * as v from 'vitest';";
    for builder in ["api.extend({})", "v.test.extend({})", "api['extend']({})"] {
        for (before, after, index, prefix) in [
            ("", "", "0", false),
            ("", "", "'0'", false),
            ("", ", ...[]", "0x0", false),
            ("0, ", "", "1.0", false),
            (", ", "", "'1'", false),
            ("control('prefix', () => {}), ", "", "1e0", true),
        ] {
            for wrap in ["", " as typeof api"] {
                let expression = format!(
                    "([{before}({builder}{wrap}){after}][{index}])('selected', () => {{}})"
                );
                let found = sites(imports, &expression);
                assert_eq!(
                    found.len(),
                    if prefix { 3 } else { 2 },
                    "{expression}: {found:?}"
                );
                assert_eq!(
                    found
                        .iter()
                        .filter(|site| site.title.as_deref() == Some("prefix"))
                        .count(),
                    usize::from(prefix),
                    "{expression}: {found:?}"
                );
                assert!(
                    found.iter().any(|site| {
                        site.title.as_deref() == Some("selected")
                            && site.kind == SiteKind::Wrapper(Reach::Tasks)
                    }),
                    "{expression}: {found:?}"
                );
            }
        }
    }
}

/// Other array values or uncertain slots keep their existing accounting.
#[test]
fn unsupported_array_routes_do_not_become_wrapper_sites() {
    let imported = "import {test as api} from 'vitest';";
    let type_only = "import type {test as api} from 'vitest';";
    for (imports, expression) in [
        (imported, "[api.extend({})][1]('other', () => {})"),
        (imported, "[,api.extend({})][0]('other', () => {})"),
        (imported, "[...[],api.extend({})][0]('other', () => {})"),
        (imported, "[api.extend({})]['00']('other', () => {})"),
        (imported, "[api.extend({})]['+0']('other', () => {})"),
        (imported, "[api.extend({})]['0.0']('other', () => {})"),
        (imported, "[api.extend({})][0.5]('other', () => {})"),
        (
            imported,
            "const key = 0; [api.extend({})][key]('other', () => {})",
        ),
        (
            imported,
            "const stored = [api.extend({})]; stored[0]('other', () => {})",
        ),
        (
            imported,
            "const stored = [api.extend({})][0]; stored('other', () => {})",
        ),
        (imported, "[api.extend({})][0].unknown('other', () => {})"),
        (imported, "[api.extend][0]('other', () => {})"),
        (imported, "[api][0]('other', () => {})"),
        (imported, "[() => {}, api.extend({})][0]('other', () => {})"),
        (
            imported,
            "{ const api = {extend: () => () => {}}; [api.extend({})][0]('other', () => {}) }",
        ),
        (type_only, "[api.extend({})][0]('other', () => {})"),
    ] {
        let found = sites(imports, expression);
        assert_eq!(found.len(), 1, "{expression}: {found:?}");
    }
}

/// A surrounding chain must not hide an independently executed slot call.
#[test]
fn longer_chain_preserves_the_immediate_slot_call() {
    let imports = "import {test as api} from 'vitest';";
    for suffix in [".unknown", "?.('other', () => {})"] {
        let expression = format!("([api.extend({{}})][0]('selected', () => {{}})){suffix}");
        let found = sites(imports, &expression);
        assert_eq!(found.len(), 2, "{expression}: {found:?}");
        assert!(
            found.iter().any(|site| {
                site.title.as_deref() == Some("selected")
                    && site.kind == SiteKind::Wrapper(Reach::Tasks)
            }),
            "{expression}: {found:?}"
        );
    }
}
