//! Complete site records for the existing inventory contract, including source identity.

use super::{inventory_with_tree, Inventory, Label, Provenance, Reach, Registers, SiteKind};
use crate::lang::{Parsed, Span};

const TEST: SiteKind = SiteKind::Registers(Registers::Test);

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
