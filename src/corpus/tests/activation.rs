//! A suite's `skip` or `todo` reaches every test declared inside it, table or not.

use super::*;
use crate::model::{Finding, Rule};

const IMPORTS: &str =
    "import { describe, expect, test } from 'vitest';\nconst off = true;\nlet value = 0;\n";

/// The findings of `rule` over `source`, after asserting how many tests it registers.
fn findings(source: &str, tests: usize, rule: Rule) -> Vec<Finding> {
    let corpus = corpora(&[("a.test.ts", source)], &["a.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), tests, "{source}");
    owned_findings("a.test.ts", corpus)
        .findings
        .into_iter()
        .filter(|finding| finding.rule == rule)
        .collect()
}

/// Every route Vitest skips or todos a child by, at any depth, and the ones it does not.
#[test]
fn a_skip_or_todo_suite_makes_every_test_inside_it_inert() {
    let body = "() => { expect(1).toBe(1) }";
    let tests = by_title(&format!(
        "{IMPORTS}\
         describe.skip('a', () => {{ test('chain skip', {body}) }});\n\
         describe.todo('b', () => {{ test('chain todo', {body}) }});\n\
         describe('c', {{ skip: true }}, () => {{ test('option skip', {body}) }});\n\
         describe('d', {{ todo: true }}, () => {{ test('option todo', {body}) }});\n\
         describe('e', () => {{ describe.skip('f', () => {{ test('inner skip', {body}) }}) }});\n\
         describe.skip('g', () => {{ describe('h', () => {{ test('outer skip', {body}) }}) }});\n\
         describe.skip.each([[1]])('i %s', () => {{ test('table skip', {body}) }});\n\
         describe('j', () => {{ test('plain', {body}) }});\n\
         describe('k', {{ skip: false }}, () => {{ test('off', {body}) }});\n\
         describe('l', {{ skip: off }}, () => {{ test('bound', {body}) }});\n\
         describe.each([[1]])('m %s', () => {{ test('table', {body}) }});\n"
    ));
    assert_eq!(tests.len(), 11, "{:?}", tests.keys());
    for title in [
        "chain skip",
        "chain todo",
        "option skip",
        "option todo",
        "inner skip",
        "outer skip",
        "table skip",
    ] {
        assert!(tests[title].inert && tests[title].may_skip, "{title}");
    }
    for title in ["plain", "off", "table"] {
        assert!(!tests[title].inert && !tests[title].may_skip, "{title}");
    }
    // A skip read from a value may skip the test, so it is not proved to run, nor proved inert.
    assert!(!tests["bound"].inert && tests["bound"].may_skip);
}

/// Each of `skip` and `todo` is its last write in property order, as Vitest reads the object; a
/// spread or an unnamed key may write either, so it never proves a skip. Such options also leave
/// the repeat count unproved, so the test is excluded before it can be compared.
#[test]
fn options_are_read_as_their_last_write() {
    let body = "() => { expect(1).toBe(1) }";
    // (options, inert, may_skip) from the runtime table Vitest 3, 4 and 5 agree on; `None` is a
    // test excluded for unreadable options, which must still not be inert.
    let rows = [
        ("{ skip: true, ...{ skip: false } }", None),
        ("{ skip: false, ...{ skip: true } }", None),
        ("{ skip: true, ...open }", None),
        ("{ ...shut }", None),
        ("{ skip: true, [key]: false }", None),
        ("{ todo: true, ...{ todo: false } }", None),
        ("{ todo: true, skip: false }", Some((true, true))),
        ("{ skip: true, skip: false }", Some((false, false))),
        ("{ skip: false, skip: true }", Some((true, true))),
        ("{ todo: true, todo: false }", Some((false, false))),
        ("{ skip: true, skip: flag }", Some((false, true))),
        ("{ retry: 1, skip: false }", Some((false, false))),
    ];
    let mut source = format!(
        "{IMPORTS}const open = {{ skip: false }}; const shut = {{ skip: true }};\n\
         const key = 'skip'; const flag = false;\n"
    );
    for (index, (options, _)) in rows.iter().enumerate() {
        source.push_str(&format!(
            "describe('s{index}', {options}, () => {{ test('in suite {index}', {body}) }});\n\
             test('own {index}', {options}, {body});\n"
        ));
    }
    let tests = by_title(&source);
    assert_eq!(tests.len(), rows.len() * 2, "{:?}", tests.keys());
    for (index, (options, expected)) in rows.into_iter().enumerate() {
        for title in [format!("in suite {index}"), format!("own {index}")] {
            let test = &tests[title.as_str()];
            match expected {
                Some(flags) => {
                    assert!(test.exclusion.is_none(), "{title}: {options}");
                    assert_eq!((test.inert, test.may_skip), flags, "{title}: {options}");
                }
                None => assert!(
                    test.exclusion.is_some() && !test.inert,
                    "{title}: {options}"
                ),
            }
        }
    }
}

/// A duplicate whose copy sits in a skipped suite is still reported, and that copy is marked.
#[test]
fn a_duplicate_inside_a_skipped_suite_is_an_inert_participant() {
    let test = "test('copy', () => { expect(value + 1).toBe(1) });";
    for (suite, inert) in [
        ("describe.skip('s', () => { TEST })", true),
        ("describe.todo('s', () => { TEST })", true),
        ("describe('s', { todo: true }, () => { TEST })", true),
        ("describe('s', () => { TEST })", false),
        ("describe('s', { skip: off }, () => { TEST })", false),
    ] {
        let source = format!(
            "{IMPORTS}test('outside', () => {{ expect(value + 1).toBe(1) }});\n{}\n",
            suite.replace("TEST", test)
        );
        let found = findings(&source, 2, Rule::DuplicateTest);
        assert_eq!(found.len(), 1, "{suite}");
        assert_eq!(
            found[0].inert.iter().filter(|flag| **flag).count(),
            usize::from(inert),
            "{suite}"
        );
        assert_eq!(
            found[0].message.contains("inert (skip/todo) participant"),
            inert,
            "{suite}"
        );
    }
}
