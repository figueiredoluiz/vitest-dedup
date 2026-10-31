//! Whole-suite outcome controls over parsed source.

use super::*;
use crate::model::Rule;

const IMPORTS: &str = "import { describe, test, expect, beforeEach } from 'vitest';";
const CHILD: &str = "test('child', () => { expect(1).toBe(1) });";

/// Parse one owned module and require its executable population before inspecting findings.
fn read(source: &str, count: usize) -> FileCorpus {
    let mut files = corpora(&[("a.test.ts", source)], &["a.test.ts"]);
    let corpus = files.remove(0);
    assert_eq!(corpus.tests.len(), count, "{source}");
    corpus
}

/// Count suite claims over one parsed file.
fn suite_count(source: &str, count: usize) -> usize {
    owned_findings("a.test.ts", read(source, count))
        .findings
        .iter()
        .filter(|finding| finding.rule == Rule::DuplicateSuite)
        .count()
}

/// The real Vitest oracle source reports its copied block once without counting suites as tests.
#[test]
fn owned_suites_replace_child_findings() {
    let source = include_str!("../../../fixtures/oracle/duplicate-suites/cases.test.ts");
    let corpus = read(source, 4);
    assert!(corpus.tests.iter().all(|case| case.exclusion.is_none()));
    assert_eq!(corpus.suites.len(), 2);
    let outcome = owned_findings("a.test.ts", corpus);
    assert_eq!(outcome.findings.len(), 1, "{outcome:?}");
    assert_eq!(outcome.findings[0].rule, Rule::DuplicateSuite);
    assert_eq!(outcome.findings[0].titles, ["copy", "copy"]);
    assert_eq!(
        outcome.findings[0]
            .locations
            .iter()
            .map(|l| l.span.start.line)
            .collect::<Vec<_>>(),
        [3, 7]
    );
}

/// Every whole-block dimension is varied independently against a retained copied-suite control.
#[test]
fn suite_keys_preserve_children_names_hooks_and_modifiers() {
    let ordinary = format!("describe('same', () => {{ {CHILD} }})");
    assert_eq!(
        suite_count(&format!("{IMPORTS}{ordinary};{ordinary}"), 2),
        1
    );
    for different in [
        format!("describe('other', () => {{ {CHILD} }})"),
        ordinary.replace("expect(1)", "expect(2)"),
        ordinary.replace("'child'", "'different'"),
        ordinary.replace(
            "{ test",
            "{ beforeEach(() => { console.log('setup') }); test",
        ),
        ordinary.replacen("describe(", "describe.concurrent(", 1),
        ordinary.replacen("describe(", "describe.runIf(true)(", 1),
        ordinary.replace("'same',", "'same', { timeout: 100 },"),
    ] {
        assert_eq!(
            suite_count(&format!("{IMPORTS}{ordinary};{different}"), 2),
            0,
            "{different}"
        );
    }
    for chain in [
        "describe.concurrent",
        "describe.runIf(true)",
        "describe.skipIf(false)",
    ] {
        let identical = ordinary.replacen("describe", chain, 1);
        assert_eq!(
            suite_count(&format!("{IMPORTS}{identical};{identical}"), 2),
            1,
            "{chain}"
        );
    }
}

/// Uncertain registration, inert blocks and excluded children never become whole-suite claims.
#[test]
fn suite_uncertainty_never_bypasses_test_exclusions() {
    for source in [
        format!("describe.skip('same', () => {{ {CHILD} }}); describe.skip('same', () => {{ {CHILD} }})"),
        format!("if (true) {{ describe('same', () => {{ {CHILD} }}) }} else {{ describe('same', () => {{ {CHILD} }}) }}"),
        format!("describe.each([1])('same', () => {{ {CHILD} }}); describe.each([1])('same', () => {{ {CHILD} }})"),
        format!("const suite = describe; suite('same', () => {{ {CHILD} }}); suite('same', () => {{ {CHILD} }})"),
        "describe('same', () => { test('child', () => {}) }); describe('same', () => { test('child', () => {}) })".into(),
        format!("describe('same', {{ skip: true }}, () => {{ {CHILD} }}); describe('same', {{ skip: true }}, () => {{ {CHILD} }})"),
    ] { assert_eq!(suite_count(&format!("{IMPORTS}{source}"), 2), 0, "{source}"); }
}

/// Enclosing paths and setup participate; each copied suite level remains an independent claim.
#[test]
fn suite_ancestry_is_keyed_and_nested_copies_report_each_level() {
    let inner = format!("describe('inner', () => {{ {CHILD} }})");
    let outer = format!("describe('outer', () => {{ {inner} }})");
    assert_eq!(suite_count(&format!("{IMPORTS}{outer};{outer}"), 2), 2);
    let changed = outer.replace("'outer'", "'other'");
    assert_eq!(suite_count(&format!("{IMPORTS}{outer};{changed}"), 2), 0);
    let changed = outer.replacen("describe(", "describe.concurrent(", 1);
    assert_eq!(suite_count(&format!("{IMPORTS}{outer};{changed}"), 2), 0);
    let changed = outer.replacen(
        "() => {",
        "() => { beforeEach(() => { console.log('outer') });",
        1,
    );
    assert_eq!(suite_count(&format!("{IMPORTS}{outer};{changed}"), 2), 0);
    assert_eq!(
        suite_count(
            &format!("{IMPORTS}describe('outer', () => {{ {inner}; {inner} }})"),
            2
        ),
        1
    );
}

/// Real runtime conditions and option-based skips use one predicate for suite admission.
#[test]
fn owned_suite_conditions_and_options() {
    let source = include_str!("../../../fixtures/oracle/duplicate-suites/conditions.test.ts");
    let corpus = read(source, 16);
    assert!(corpus.tests.iter().all(|test| test.exclusion.is_none()));
    let result = owned_findings("a.test.ts", corpus);
    let mut titles: Vec<_> = result
        .findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateSuite)
        .map(|f| f.titles[0].as_str())
        .collect();
    titles.sort();
    assert_eq!(titles, ["option false", "run active", "skip active"]);
}

/// Renaming suite-local bindings preserves equality while unknown options and conditions refuse it.
#[test]
fn suite_local_names_and_unknown_activation() {
    let block = "describe('same', () => { const value = 1; test('child', () => { expect(value).toBe(1) }) })";
    assert_eq!(
        suite_count(
            &format!("{IMPORTS}{block};{}", block.replace("value", "renamed")),
            2
        ),
        1
    );
    let one = "test('one', () => { expect(1).toBe(1) });";
    let two = "test('two', () => { expect(2).toBe(2) });";
    let reordered = format!("{IMPORTS}describe('same', () => {{ {one} {two} }}); describe('same', () => {{ {two} {one} }})");
    assert_eq!(
        suite_count(&reordered, 4),
        0,
        "child order is part of the complete block"
    );
    for prefix in [
        "describe.runIf(false)",
        "describe.skipIf(true)",
        "describe['runIf'](false)",
        "describe['skipIf'](true)",
    ] {
        let changed = block.replacen("describe", prefix, 1);
        assert_eq!(suite_count(&format!("{IMPORTS}{changed};{changed}"), 2), 0);
    }
    for options in [
        "{ skip: unknown }",
        "{ ...unknown }",
        "unknown",
        "{ [unknown]: false }",
    ] {
        let changed = block.replace("'same',", &format!("'same', {options},"));
        assert_eq!(suite_count(&format!("{IMPORTS}{changed};{changed}"), 2), 0);
    }
}
