use super::{file, Engine, Exclusion, Failure, FileCorpus, Run, Setting, TestCase};
use crate::framework::vitest::DIALECT;
use crate::model::ExclusionReason;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

mod completion;
mod dynamic_import;
mod export_identity;
mod import_keys;
mod import_limits;
mod local_helpers;
mod module_meta;
mod nc64;
mod nc65;
mod normal_properties;
mod runners;

/// Require the requested rule to stay silent while retaining diagnostics on failure.
fn assert_rule_absent(outcome: crate::rules::Outcome, rule: crate::model::Rule) {
    assert!(
        outcome.findings.iter().all(|finding| finding.rule != rule),
        "{:?}",
        outcome.findings
    );
}

/// Read one caller together with its imported factory.
fn factory_corpus(factory: &str, caller: &str) -> FileCorpus {
    corpora(
        &[("case.test.ts", caller), ("factory.ts", factory)],
        &["case.test.ts"],
    )
    .remove(0)
}

/// Assert the writer/read batch refuses its reader while an isolated reader stays eligible.
fn assert_mutation_reader(results: &[FileCorpus]) {
    assert_eq!(results[0].tests.len(), 2);
    assert_eq!(results[1].tests.len(), 1);
    assert_eq!(
        results[0].tests[1].exclusion,
        Some(Exclusion::OrderDependentState)
    );
    assert_eq!(results[1].tests[0].exclusion, None);
    assert_rule_absent(
        owned_outcome(vec![
            ("a.test.ts", results[0].clone()),
            ("b.test.ts", results[1].clone()),
        ]),
        crate::model::Rule::CrossFileDuplicate,
    );
}

/// Retain the explicit wrapper-refusal expectation across independently read callers.
fn assert_all_wrappers_refused(results: &[FileCorpus]) {
    assert!(
        results
            .iter()
            .flat_map(|corpus| &corpus.tests)
            .all(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)),
        "{results:?}"
    );
}

/// Read selected files under one shared project authority.
fn corpora(files: &[(&str, &str)], tests: &[&str]) -> Vec<FileCorpus> {
    runs(files, &[tests]).remove(0)
}

/// The corpus of the test files each of `orders` names, read in that order in a run of its own,
/// all over one project holding `files`.
fn runs(files: &[(&str, &str)], orders: &[&[&str]]) -> Vec<Vec<FileCorpus>> {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    let setting = Setting {
        dialect: &DIALECT,
        globals: false,
        root: dir.path(),
        benchmarks: false,
    };
    orders
        .iter()
        .map(|order| {
            let mut run = Run::default();
            order
                .iter()
                .map(|test| {
                    let path = dir.path().join(test);
                    let source = fs::read_to_string(&path).unwrap();
                    file(&path, &source, setting, &mut run).unwrap()
                })
                .collect()
        })
        .collect()
}

/// The tests of one file, by title (a row's title gains `#row`).
fn by_title(source: &str) -> HashMap<String, TestCase> {
    let corpus = corpora(&[("a.test.ts", source)], &["a.test.ts"]).remove(0);
    corpus
        .tests
        .into_iter()
        .map(|test| {
            let title = match test.row {
                Some(row) => format!("{}#{row}", test.title.clone().unwrap_or_default()),
                None => test.title.clone().unwrap_or_default(),
            };
            (title, test)
        })
        .collect()
}

const HEADER: &str =
    "import { test, describe, beforeEach, aroundEach, expect, vi } from 'vitest'\n";

/// Counts extracted and excluded cases from an owned source before any finding assertion.
fn counted_corpus(source: &str, path: &str, tests: usize, excluded: usize) -> FileCorpus {
    let corpus = corpora(&[(path, source)], &[path]).remove(0);
    assert_eq!(corpus.tests.len(), tests);
    assert_eq!(
        corpus
            .tests
            .iter()
            .filter(|case| case.exclusion.is_some())
            .count(),
        excluded
    );
    corpus
}

/// Runs an owned oracle source through the corpus and asserts counts before name findings.
fn assert_owned_names(source: &str, path: &str, tests: usize, excluded: usize, expected: &[&str]) {
    let corpus = counted_corpus(source, path, tests, excluded);
    let outcome = owned_findings(path, corpus);
    let names: Vec<_> = outcome
        .findings
        .iter()
        .filter(|finding| finding.rule == crate::model::Rule::DuplicateTestName)
        .collect();
    assert_eq!(names.len(), expected.len(), "{names:#?}");
    for (finding, title) in names.iter().zip(expected) {
        assert_eq!(finding.titles, [*title, *title]);
        assert_eq!(finding.locations.len(), 2);
    }
}

/// The same owned source the collection oracle runs: names, rows and scope; empty tests and todos
/// stay excluded from comparison and still collide by name.
#[test]
fn owned_name_collisions_keep_distinct_rows_and_exclusions() {
    assert_owned_names(
        include_str!("../../fixtures/oracle/name-collisions/cases.test.ts"),
        "cases.test.ts",
        18,
        4,
        &[
            "collision",
            "round 1",
            "round 2",
            "object [object Object]",
            "held",
            "empty",
        ],
    );
}

/// Asserts which titled tests share a same-file key.
fn keys(source: &str, equal: &[(&str, &str)], differ: &[(&str, &str)]) {
    let tests = by_title(&format!("{HEADER}{source}"));
    let test = |title: &str| {
        tests
            .get(title)
            .unwrap_or_else(|| panic!("no test {title}"))
    };
    for (left, right) in equal {
        assert_eq!(test(left).exclusion, None, "{left} is compared");
        assert_eq!(
            test(left).key,
            test(right).key,
            "{left} and {right} should be equal"
        );
    }
    for (left, right) in differ {
        assert_eq!(test(left).exclusion, None, "{left} is compared");
        assert_eq!(test(right).exclusion, None, "{right} is compared");
        assert_ne!(
            test(left).key,
            test(right).key,
            "{left} and {right} should differ"
        );
    }
}

/// Each exclusion comes from one cause, and its one-edit neighbour is compared.
#[test]
fn each_reason_has_one_cause() {
    let tests = by_title(&format!(
        "{HEADER}test('callback', () => {{ expect(1).toBe(1) }})\n\
         test('empty', () => {{}})\n\
         test('no callback')\n\
         test.todo('todo')\n\
         function later() {{ test('inside', () => {{ expect(1).toBe(1) }}) }}\n\
         [1].forEach(() => test('iterated', () => {{ expect(1).toBe(1) }}))\n\
         let shared = 0\n\
         let reset = 0\n\
         beforeEach(() => {{ reset = 0 }})\n\
         test('writer', () => {{ shared = 1; expect(1).toBe(1) }})\n\
         test('reader', () => {{ expect(shared).toBe(0) }})\n\
         test('reset', () => {{ expect(reset).toBe(0) }})\n\
         test('timed empty', () => {{}}, 1000)\n\
         test('options only', {{ retry: 2 }})\n\
         function blank() {{}}\n\
         const reads = () => {{ expect(shared).toBe(0) }}\n\
         test('named empty', blank)\n\
         test('named reader', reads, 1000)\n\
         test('timed', () => {{ expect(1).toBe(1) }}, 1000)\n\
         test('options first', {{ retry: 2 }}, () => {{ expect(1).toBe(1) }})\n\
         import {{ check }} from './check'\n\
         test('imported', check)"
    ));
    let exclusion = |title: &str| tests[title].exclusion;
    assert_eq!(exclusion("callback"), None);
    assert_eq!(exclusion("empty"), Some(Exclusion::EmptyBody));
    assert_eq!(exclusion("no callback"), Some(Exclusion::NoCallback));
    assert_eq!(exclusion("todo"), Some(Exclusion::NoCallback));
    assert_eq!(
        exclusion("inside"),
        Some(Exclusion::RegisteredInsideFunction)
    );
    assert_eq!(
        exclusion("iterated"),
        None,
        "an iteration callback runs while the file loads"
    );
    assert_eq!(exclusion("reader"), Some(Exclusion::OrderDependentState));
    assert_eq!(
        exclusion("writer"),
        None,
        "only the writer reads its own write"
    );
    assert_eq!(
        exclusion("reset"),
        None,
        "a hook that resets is idempotent per test"
    );
    // The callback is the function, wherever a timeout or options put it.
    assert_eq!(exclusion("timed empty"), Some(Exclusion::EmptyBody));
    assert_eq!(exclusion("options only"), Some(Exclusion::NoCallback));
    // A callback passed by name is refused, whatever it is bound to.
    assert_eq!(exclusion("named empty"), Some(Exclusion::NamedCallback));
    assert_eq!(exclusion("named reader"), Some(Exclusion::NamedCallback));
    assert_eq!(exclusion("timed"), None);
    assert_eq!(exclusion("options first"), None);
    assert_eq!(tests["options first"].key, tests["callback"].key);
    assert_eq!(exclusion("imported"), Some(Exclusion::NamedCallback));
}

/// A test refused only for its callback keeps the name the compared path gives; a registration a
/// function places, or a callback a spread may hide, has none.
#[test]
fn callback_exclusions_keep_their_names() {
    let tests = by_title(
        "import { describe, expect, test } from 'vitest'\n\
         describe('suite', () => { test('empty', () => {}); test.todo('todo') })\n\
         test(function titled() { throw Error('never runs') })\n\
         test.each([1])('row %i', () => {})\n\
         test('compared', () => { expect(1).toBe(1) })\n\
         Promise.resolve().then(() => test('deferred'))\n\
         test('spread', ...[])\n",
    );
    assert_eq!(tests.len(), 7, "{:?}", tests.keys());
    let named = |title: &str| {
        let test = &tests[title];
        (
            test.exclusion,
            test.name.as_ref().map(|name| name.join(" > ")),
        )
    };
    let name = |text: &str| Some(text.to_owned());
    assert_eq!(
        named("empty"),
        (Some(Exclusion::EmptyBody), name("suite > empty"))
    );
    assert_eq!(
        named("todo"),
        (Some(Exclusion::NoCallback), name("suite > todo"))
    );
    assert_eq!(
        named("titled"),
        (Some(Exclusion::NoCallback), name("titled"))
    );
    assert_eq!(
        named("row %i#0"),
        (Some(Exclusion::EmptyBody), name("row 1"))
    );
    assert_eq!(named("compared"), (None, name("compared")));
    assert_eq!(named("deferred"), (Some(Exclusion::NoCallback), None));
    assert_eq!(named("spread"), (Some(Exclusion::UnreadableCallback), None));
    // Vitest marks a test with no callback `todo`, written as `.todo` or not; an empty one runs.
    let inert = |title: &str| tests[title].inert;
    assert!(inert("titled") && inert("todo") && inert("deferred"));
    assert!(!inert("empty") && !inert("compared"));
}

/// A scoped mock changes dynamic imports at runtime, so no test in its file is comparable.
#[test]
fn non_hoisted_mock_excludes_the_file_but_hoisted_mock_controls_remain_comparable() {
    let scoped = counted_corpus(
        "import { test, expect, vi } from 'vitest';\n\
         test('first', async () => { vi.doMock('./dep', () => ({ value: 1 })); expect(1).toBe(1) });\n\
         test('second', () => { expect(1).toBe(1) });\n",
        "scoped.test.ts",
        2,
        2,
    );
    assert!(scoped
        .tests
        .iter()
        .all(|test| test.exclusion == Some(Exclusion::NonHoistedMock)));
    assert_eq!(scoped.context_refusals, [Exclusion::NonHoistedMock]);

    let hoisted = counted_corpus(
        "import { test, expect, vi } from 'vitest';\n\
         vi.mock('./dep', () => ({ value: 1 }));\n\
         test('first', () => { expect(1).toBe(1) });\n\
         test('second', () => { expect(1).toBe(1) });\n",
        "hoisted.test.ts",
        2,
        0,
    );
    assert!(hoisted.context_refusals.is_empty());
}

/// Context values that need evaluation are visibly refused instead of sharing a made-up context.
#[test]
fn unreadable_mock_factories_and_fixtures_exclude_their_file() {
    let mock = counted_corpus(
        "import { test, vi } from 'vitest'; import { factory } from './factory';\n\
         vi.mock('./dep', factory); test('a', () => {});",
        "mock.test.ts",
        1,
        1,
    );
    assert_eq!(
        mock.tests[0].exclusion,
        Some(Exclusion::UnreadableMockFactory)
    );
    assert_eq!(mock.context_refusals, [Exclusion::UnreadableMockFactory]);

    let fixture = counted_corpus(
        "import { test as base } from 'vitest'; import { makeDb } from './db';\n\
         const test = base.extend({ db: makeDb() }); test('a', () => {});",
        "fixture.test.ts",
        1,
        1,
    );
    assert_eq!(
        fixture.tests[0].exclusion,
        Some(Exclusion::UnreadableFixture)
    );
    assert_eq!(fixture.context_refusals, [Exclusion::UnreadableFixture]);
}

/// A title the analyzer cannot resolve cannot safely take part in either name or body comparison.
#[test]
fn dynamic_titles_are_excluded_but_literal_titles_remain_comparable() {
    let dynamic = counted_corpus(
        "import { test, expect } from 'vitest';\n\
         const suffix = Math.random();\n\
         test(`first ${suffix}`, () => { expect(1).toBe(1) });\n\
         test(`second ${suffix}`, () => { expect(1).toBe(1) });\n",
        "dynamic.test.ts",
        2,
        2,
    );
    assert!(dynamic
        .tests
        .iter()
        .all(|test| test.exclusion == Some(Exclusion::UnresolvedTitle)));
    assert!(dynamic.context_refusals.is_empty());

    let literal = counted_corpus(
        "import { test, expect } from 'vitest';\n\
         test('first', () => { expect(1).toBe(1) });\n\
         test('second', () => { expect(1).toBe(1) });\n",
        "literal.test.ts",
        2,
        0,
    );
    assert_eq!(literal.tests[0].key, literal.tests[1].key);
}

/// A wrapper call, a module load and a registration function read as a value each register tests
/// this file does not spell: one excluded case each.
#[test]
fn what_registers_elsewhere_is_one_excluded_case_per_site() {
    let own = "test('own', () => { expect(1).toBe(1) })\n";
    let elsewhere = format!(
        "import {{ test, expect }} from 'vitest'\nimport {{ suite }} from './suite'\n\
         import './loads'\nsuite()\nrun(test)\n{own}"
    );
    let neighbour = format!(
        "import {{ test, expect }} from 'vitest'\nimport {{ quiet }} from './suite'\n\
         import './quiet'\nquiet()\nrun(1)\n{own}"
    );
    let corpora = corpora(
        &[
            (
                "suite.ts",
                "import { test } from 'vitest'\n\
                 export const suite = () => { test('x', () => {}) }\nexport const quiet = () => 1\n",
            ),
            ("loads.ts", "import { test } from 'vitest'\ntest('on load', () => {})\n"),
            ("quiet.ts", "export const value = 1\n"),
            ("a.test.ts", &elsewhere),
            ("b.test.ts", &neighbour),
        ],
        &["a.test.ts", "b.test.ts"],
    );
    let elsewhere = |corpus: &FileCorpus| {
        corpus
            .exclusions()
            .get(&Exclusion::RegisteredElsewhere)
            .copied()
    };
    assert_eq!(elsewhere(&corpora[0]), Some(3));
    assert_eq!(corpora[0].tests.len(), 4);
    assert_eq!(elsewhere(&corpora[1]), None);
    assert_eq!(corpora[1].tests.len(), 1);
}

/// Every key of a file is the same whichever files the run read before it.
#[test]
fn keys_do_not_depend_on_the_order_files_are_read() {
    let test_file = |name: &str| {
        format!(
            "import {{ test, expect, beforeEach }} from 'vitest'\n\
             import {{ helper }} from './m'\nimport {{ x }} from './n'\nimport './setup'\n\
             const local = {}1\nbeforeEach(() => {{ helper() }})\n\
             test('{name}', async () => {{ const n = await import('./n'); expect(x() + local + n.x()).toBe(1) }})\n\
             test.each([[1], [2]])('{name} row', (n) => {{ expect(helper(n)).toBe(n) }})\n",
            "!".repeat(240)
        )
    };
    let (a, b) = (test_file("a"), test_file("b"));
    let files = [
        (
            "m.ts",
            "import { x } from './n'\nexport const helper = (n) => x() + n\n",
        ),
        (
            "n.ts",
            "import { helper } from './m'\nexport const x = () => helper(0)\n",
        ),
        ("setup.ts", "globalThis.ready = true\n"),
        ("a.test.ts", a.as_str()),
        ("b.test.ts", b.as_str()),
    ];
    let runs = runs(
        &files,
        &[&["a.test.ts", "b.test.ts"], &["b.test.ts", "a.test.ts"]],
    );
    let keys = |corpora: &[FileCorpus]| -> Vec<Vec<(Option<usize>, u64, u64)>> {
        corpora
            .iter()
            .map(|corpus| {
                corpus
                    .tests
                    .iter()
                    .map(|test| (test.row, test.key, test.module_key))
                    .collect()
            })
            .collect()
    };
    let forward = keys(&runs[0]);
    let mut backward = keys(&runs[1]);
    backward.reverse();
    assert_eq!(forward[0].len(), 3);
    assert_eq!(forward, backward);
}

/// A hook that builds on the previous value makes what reads it depend on test order; one that
/// sets a value does not.
#[test]
fn an_accumulating_hook_makes_its_readers_order_dependent() {
    for (hook, expected) in [
        ("count++", Some(Exclusion::OrderDependentState)),
        ("count += 1", Some(Exclusion::OrderDependentState)),
        ("count = count + 1", Some(Exclusion::OrderDependentState)),
        ("count = 1", None),
        ("other = count + 1", None),
    ] {
        let tests = by_title(&format!(
            "{HEADER}let count = 0\nlet other = 0\n\
             beforeEach(() => {{ {hook} }})\n\
             test('reads', () => {{ expect(count).toBe(1) }})"
        ));
        assert_eq!(tests["reads"].exclusion, expected, "{hook}");
    }
}

/// Every test site is compared or excluded, and each row of a table is a case of its own.
#[test]
fn every_test_site_is_compared_or_excluded_and_rows_are_cases() {
    let corpus = corpora(
        &[(
            "a.test.ts",
            "import { test, describe } from 'vitest'\n\
             describe('s', () => {\n\
               test.each([[1], [2], [3]])('row %s', (n) => { expect(n).toBeTruthy() })\n\
               test('plain', () => { expect(1).toBe(1) })\n\
               test('empty', () => {})\n\
               test.each([[1], [2]])('blank %s', () => {})\n\
             })",
        )],
        &["a.test.ts"],
    )
    .remove(0);
    let rows: Vec<_> = corpus.tests.iter().map(|test| test.row).collect();
    // An excluded table is still a case per row.
    assert_eq!(
        rows,
        [Some(0), Some(1), Some(2), None, None, Some(0), Some(1)]
    );
    assert_eq!(corpus.exclusions().get(&Exclusion::EmptyBody), Some(&3));
    let test_sites = corpus
        .sites
        .iter()
        .filter(|site| site.kind == "test")
        .count();
    assert_eq!(test_sites, 4);
    assert_eq!(corpus.sites[0].kind, "describe");
    assert_eq!((corpus.sites[1].line, corpus.sites[1].column), (3, 27));
    assert_eq!(corpus.sites[1].chain, "each.()");
}

/// A row whose every parameter is bound keys like the test that spells its values.
#[test]
fn a_bound_row_is_the_test_it_spells() {
    keys(
        "test.each([[1], [2]])('row', (n) => { expect(n).toBe(1) })\n\
         test('spelled', () => { expect(1).toBe(1) })\n\
         test.each([[1]])('unbound', (n, ...rest) => { expect(n).toBe(1) })\n\
         test.each([[1]])('timed row', (n) => { expect(n).toBe(1) }, 1000)\n\
         test('timed spelled', () => { expect(1).toBe(1) }, 1000)",
        &[("row#0", "spelled"), ("timed row#0", "timed spelled")],
        &[("row#1", "spelled"), ("unbound#0", "spelled")],
    );
}

/// A suite adds its hooks, statements and modifiers to its tests' keys; how deep it nests adds
/// nothing.
#[test]
fn suite_context_is_what_a_suite_adds_not_how_deep_it_is() {
    keys(
        "describe('one', () => { test('a', () => { expect(x()).toBe(1) }) })\n\
         describe('two', () => { describe('deeper', () => { test('b', () => { expect(x()).toBe(1) }) }) })\n\
         describe('hooked', () => { beforeEach(() => { reset() }); test('c', () => { expect(x()).toBe(1) }) })\n\
         describe('around', () => { aroundEach(async (run) => { await run() }); test('d', () => { expect(x()).toBe(1) }) })\n\
         describe('env', () => { process.env.MODE = 'x'; test('e', () => { expect(x()).toBe(1) }) })\n\
         describe('stub', () => { vi.stubEnv('MODE', 'x'); test('f', () => { expect(x()).toBe(1) }) })\n\
         describe('scoped', () => { test.scoped({ mode: 'x' }); test('g', () => { expect(x()).toBe(1) }) })\n\
         describe('declares', () => { const local = 1; test('h', () => { expect(x()).toBe(1) }) })\n\
         describe.skipIf(ci)('conditional', () => { test('i', () => { expect(x()).toBe(1) }) })\n\
         describe.fails('inverted', () => { test('j', () => { expect(x()).toBe(1) }) })",
        &[("a", "b"), ("a", "h")],
        &[
            ("a", "c"),
            ("a", "d"),
            ("a", "e"),
            ("a", "f"),
            ("a", "g"),
            ("e", "f"),
            ("a", "i"),
            ("a", "j"),
        ],
    );
}

/// Modifiers and options key a test when they change its outcome, not when they only schedule it.
#[test]
fn modifiers_and_options_key_what_changes_the_outcome() {
    keys(
        "test('a', () => { expect(x()).toBe(1) })\n\
         test.skip('b', () => { expect(x()).toBe(1) })\n\
         test('c', { timeout: 10 }, () => { expect(x()).toBe(1) })\n\
         test.fails('d', () => { expect(x()).toBe(1) })\n\
         test('e', { fails: true }, () => { expect(x()).toBe(1) })",
        &[("a", "b"), ("a", "c"), ("d", "e")],
        &[("a", "d")],
    );
}

/// A cross-file key carries what its file runs while loading; a same-file key does not.
#[test]
fn cross_file_keys_carry_what_each_file_does_while_loading() {
    let body = "test('t', () => { expect(data).toBeTruthy() })";
    let plain =
        format!("import {{ test, expect }} from 'vitest'\nimport data from './data.json'\n{body}");
    let mocked = format!("import {{ test, expect, vi }} from 'vitest'\nimport data from './data.json'\nvi.mock('./x')\n{body}");
    // Vitest hoists only `vi.mock` as spelled: through another binding the call runs in place.
    let renamed = format!("import {{ test, expect, vi as tools }} from 'vitest'\nimport data from './data.json'\ntools.mock('./x')\n{body}");
    // The same mock from another folder, loading the same data file: only the target differs.
    let elsewhere = renamed.replace("'./data.json'", "'../one/data.json'");
    let alias = format!("import {{ test, expect, vitest as tools }} from 'vitest'\nimport data from './data.json'\ntools.mock('./x')\n{body}");
    let namespace = format!("import {{ test, expect }} from 'vitest'\nimport * as framework from 'vitest'\nimport data from './data.json'\nframework.vi.mock('./x')\n{body}");
    // Nothing but the setup file differs between directories.
    let setup = "import { test, expect } from 'vitest'\nimport './setup.js'\ntest('t', () => { expect(globalThis.mode).toBe('a') })".to_owned();
    // What a dynamic import loads, likewise.
    let dynamic = "import { test, expect } from 'vitest'\ntest('t', async () => { expect(await import('./data.json')).toBeTruthy() })".to_owned();
    let corpora = corpora(
        &[
            ("one/a.test.ts", &plain),
            ("one/b.test.ts", &plain),
            ("one/c.test.ts", &mocked),
            ("two/a.test.ts", &plain),
            ("one/data.json", "{}"),
            ("two/data.json", "{}"),
            ("one/d.test.ts", &setup),
            ("one/e.test.ts", &setup),
            ("two/d.test.ts", &setup),
            ("one/setup.js", "globalThis.mode = 'a'"),
            ("two/setup.js", "globalThis.mode = 'b'"),
            ("one/f.test.ts", &dynamic),
            ("one/g.test.ts", &dynamic),
            ("two/f.test.ts", &dynamic),
            ("one/m.test.ts", &renamed),
            ("one/n.test.ts", &alias),
            ("one/o.test.ts", &namespace),
            ("two/m.test.ts", &elsewhere),
        ],
        &[
            "one/a.test.ts",
            "one/b.test.ts",
            "one/c.test.ts",
            "two/a.test.ts",
            "one/d.test.ts",
            "one/e.test.ts",
            "two/d.test.ts",
            "one/f.test.ts",
            "one/g.test.ts",
            "two/f.test.ts",
            "one/m.test.ts",
            "one/n.test.ts",
            "one/o.test.ts",
            "two/m.test.ts",
        ],
    );
    let key = |index: usize| corpora[index].tests[0].module_key;
    assert_eq!(key(0), key(1), "equal files importing one data file");
    assert_ne!(key(0), key(2), "a file that mocks differs");
    assert_ne!(key(0), key(3), "the same spelling names another file");
    assert_eq!(key(4), key(5), "one setup file imported for its effects");
    assert_ne!(
        key(4),
        key(6),
        "a setup file of the same spelling is another file"
    );
    assert_eq!(key(7), key(8), "one data file loaded dynamically");
    assert_ne!(
        key(7),
        key(9),
        "a dynamic import of the same spelling is another file"
    );
    assert!(
        corpora[10..]
            .iter()
            .all(|corpus| corpus.tests[0].exclusion.is_none()),
        "a top-level mock run in place is compared"
    );
    assert_eq!(
        key(10),
        key(11),
        "renamed `vi` and `vitest` mock in place alike"
    );
    assert_eq!(key(10), key(12), "a namespace member mocks in place too");
    assert_ne!(key(10), key(2), "`vi.mock` runs before the imports");
    assert_ne!(key(10), key(0), "a renamed mock still mocks");
    assert_ne!(
        key(10),
        key(13),
        "the same mock spelling in another folder targets another file"
    );
    assert_eq!(
        corpora[0].tests[0].key, corpora[2].tests[0].key,
        "same-file keys ignore module effects"
    );
}

/// A file the parser refuses is an error for the caller, not an empty corpus.
#[test]
fn refusals_reach_the_caller() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.test.ts");
    let setting = Setting {
        dialect: &DIALECT,
        globals: false,
        root: dir.path(),
        benchmarks: false,
    };
    assert_eq!(
        file(&path, "(((", setting, &mut Run::default()).err(),
        Some(crate::lang::Refusal::Fatal)
    );
}

/// State every test starts from the same value of is not order dependent.
#[test]
fn state_every_test_starts_from_the_same_value_of_is_not_order_dependent() {
    let case = |setup: &str, reader: &str| {
        let tests = by_title(&format!(
            "{HEADER}import {{ beforeAll }} from 'vitest'\n\
             let wrapper = null\n\
             {setup}\n\
             test('writer', () => {{ wrapper = mount(); expect(wrapper).toBeTruthy() }})\n\
             describe('readers', () => {{ test('reader', () => {{ {reader} }}) }})"
        ));
        tests["reader"].exclusion
    };
    let reads = "expect(wrapper).toBeTruthy()";
    let order = Some(Exclusion::OrderDependentState);
    // A before-each reset around the reader, at module level or in its suite.
    assert_eq!(case("beforeEach(() => { wrapper = mount() })", reads), None);
    assert_eq!(
        case(
            "aroundEach(async (run) => { wrapper = mount(); await run() })",
            reads
        ),
        None
    );
    // Not a reset: it runs once, it reads what it replaces, or it is in another suite.
    assert_eq!(case("beforeAll(() => { wrapper = mount() })", reads), order);
    assert_eq!(
        case("beforeEach(() => { wrapper = wrapper || mount() })", reads),
        order
    );
    // A read inside a function the value holds runs later, on the new value.
    assert_eq!(
        case(
            "beforeEach(() => { wrapper = { get: () => wrapper } })",
            reads
        ),
        None
    );
    assert_eq!(
        case(
            "describe('other', () => { beforeEach(() => { wrapper = mount() }) })",
            reads
        ),
        order
    );
    // Nor one that may not run: under a condition, or after a way out of the hook.
    assert_eq!(
        case("beforeEach(() => { if (ready) wrapper = mount() })", reads),
        order
    );
    assert_eq!(
        case(
            "beforeEach(() => { if (ready) return; wrapper = mount() })",
            reads
        ),
        order
    );
    // Nor one a helper registers for whichever suite calls it.
    assert_eq!(
        case(
            "function setup() { beforeEach(() => { wrapper = mount() }) }",
            reads
        ),
        order
    );
    // A concise hook body runs whenever the hook does.
    assert_eq!(case("beforeEach(() => (wrapper = mount()))", reads), None);
    // The reader's own assignment first, directly in its body.
    assert_eq!(
        case("", "wrapper = mount(); expect(wrapper).toBeTruthy()"),
        None
    );
    assert_eq!(
        case("", "expect(wrapper).toBeNull(); wrapper = mount()"),
        order
    );
    assert_eq!(
        case(
            "",
            "if (flag) wrapper = mount(); expect(wrapper).toBeTruthy()"
        ),
        order
    );
}

/// What a file runs while it loads is part of a cross-file key whatever statement holds it: a
/// block or a declaration that calls; a declaration that runs nothing is not.
#[test]
fn every_statement_that_runs_while_the_file_loads_is_module_context() {
    let file = |setup: &str| {
        format!(
            "import {{ test, expect, vi }} from 'vitest'\n{setup}\n\
             test('mode', () => {{ expect(process.env.MODE).toBe('a') }})\n"
        )
    };
    let pairs = [
        (
            "if (true) { vi.stubEnv('MODE', 'a') }",
            "if (true) { vi.stubEnv('MODE', 'b') }",
            false,
        ),
        (
            "if (true) { vi.stubEnv('MODE', 'a') }",
            "if (true) { vi.stubEnv('MODE', 'a') }",
            true,
        ),
        (
            "const env = vi.stubEnv('MODE', 'a')",
            "const env = vi.stubEnv('MODE', 'b')",
            false,
        ),
        ("const unused = 1", "const unused = 2", true),
        (
            "export const env = vi.stubEnv('MODE', 'a')",
            "export const env = vi.stubEnv('MODE', 'b')",
            false,
        ),
        ("export const unused = 1", "export const unused = 2", true),
    ];
    for (left, right, equal) in pairs {
        let (left_file, right_file) = (file(left), file(right));
        let corpora = corpora(
            &[("a.test.ts", &left_file), ("b.test.ts", &right_file)],
            &["a.test.ts", "b.test.ts"],
        );
        let key = |index: usize| corpora[index].tests[0].module_key;
        assert_eq!(key(0) == key(1), equal, "{left} / {right}");
    }
}

/// A test or hook callback passed by name runs at test time: what it writes makes readers order
/// dependent, as an inline one does.
#[test]
fn a_named_callback_writes_at_run_time() {
    let tests = |writer: &str, registration: &str| {
        by_title(&format!(
            "{HEADER}let count = 0\nlet other = 0\nfunction writer() {{ {writer} }}\n\
             test('first', () => {{ expect(count).toBe(0) }})\n{registration}\n\
             test('second', () => {{ expect(count).toBe(0) }})"
        ))
    };
    for registration in ["test('writer', writer)", "beforeEach(writer)"] {
        let writes = tests("count++", registration);
        assert_eq!(
            writes["second"].exclusion,
            Some(Exclusion::OrderDependentState),
            "{registration}"
        );
        let elsewhere = tests("other++", registration);
        assert_eq!(elsewhere["second"].exclusion, None, "{registration}");
    }
}

/// A function in the title position is the title: the callback comes after it.
#[test]
fn a_function_title_is_not_the_callback() {
    let source = format!(
        "{HEADER}const handler = () => {{ expect(1).toBe(1) }}\n\
         test(function title() {{}}, handler)\n\
         test(function title() {{}}, () => {{ expect(1).toBe(1) }})"
    );
    let corpus = corpora(&[("a.test.ts", &source)], &["a.test.ts"]).remove(0);
    let exclusions: Vec<_> = corpus.tests.iter().map(|test| test.exclusion).collect();
    assert_eq!(exclusions, [Some(Exclusion::NamedCallback), None]);
}

/// A callback passed by name is refused however it is bound: reassigned, or shared by two tests.
/// What it writes is still seen, in any argument position, and a named suite body is no suite.
#[test]
fn named_callbacks_are_refused_and_their_writes_seen() {
    let tests = by_title(&format!(
        "{HEADER}function body() {{}}\nbody = () => {{ expect(1).toBe(1) }}\n\
         test('reassigned', body)\n\
         let n = 0\nfunction shared() {{ expect(n++).toBe(0) }}\n\
         test('shared a', shared)\ntest('shared b', shared)\n\
         const timeout = 1000\nlet count = 0\nfunction writer() {{ count++ }}\n\
         test('first', () => {{ expect(count).toBe(0) }})\n\
         test('writer', writer, timeout)\n\
         test('second', () => {{ expect(count).toBe(0) }})\n\
         let state = 0\n\
         function suite() {{ beforeEach(() => {{ state = 0 }}); test('inner', () => {{ expect(state).toBe(0) }}) }}\n\
         describe('s', suite)"
    ));
    let exclusion = |title: &str| tests[title].exclusion;
    for title in ["reassigned", "shared a", "shared b", "writer"] {
        assert_eq!(exclusion(title), Some(Exclusion::NamedCallback), "{title}");
    }
    assert_eq!(exclusion("second"), Some(Exclusion::OrderDependentState));
    assert_eq!(
        exclusion("inner"),
        Some(Exclusion::RegisteredInsideFunction)
    );
}

/// A suite's options are part of its tests' keys when they change the outcome.
#[test]
fn suite_options_key_their_tests() {
    keys(
        "describe('a', { fails: true }, () => { test('expected to fail', () => { expect(1).toBe(1) }) })\n\
         describe('b', () => { test('plain', () => { expect(1).toBe(1) }) })\n\
         describe('c', { timeout: 10 }, () => { test('timed', () => { expect(1).toBe(1) }) })",
        &[("plain", "timed")],
        &[("expected to fail", "plain")],
    );
}

/// An enclosing suite that is expected to fail inverts its tests' outcome; a whole row that cannot
/// be bound is keyed by its cells as written.
#[test]
fn an_expected_failure_suite_and_unbound_rows_key_their_tests() {
    keys(
        "describe.fails('f', () => { test('in fails', () => { expect(1).toBe(1) }) })\n\
         describe('p', () => { test('in plain', () => { expect(1).toBe(1) }) })\n\
         const base = {}\n\
         test.for([{ __proto__: base, n: 1 }, { __proto__: base, n: 2 }, { __proto__: base, n: 1 }])\
         ('proto', ({ n }) => { expect(n).toBe(1) })",
        &[("proto#0", "proto#2")],
        &[("in fails", "in plain"), ("proto#0", "proto#1")],
    );
}

/// A statement that does more than register is context: setup sequenced with a registration counts.
#[test]
fn setup_sequenced_with_a_registration_is_module_context() {
    let file = |mode: &str| {
        format!(
            "import {{ test, expect }} from 'vitest'\n\
             ;(globalThis.mode = '{mode}', test('mode', () => {{ expect(globalThis.mode).toBe('a') }}))\n"
        )
    };
    let (a, b, c) = (file("a"), file("b"), file("a"));
    let corpora = corpora(
        &[("a.test.ts", &a), ("b.test.ts", &b), ("c.test.ts", &c)],
        &["a.test.ts", "b.test.ts", "c.test.ts"],
    );
    let key = |index: usize| corpora[index].tests[0].module_key;
    assert_ne!(key(0), key(1));
    assert_eq!(key(0), key(2));
}

/// A suite body passed by name runs while the file registers: what it writes is part of what the
/// bindings it writes are, not a test's write.
#[test]
fn a_named_suite_body_writes_at_registration_time() {
    keys(
        "const a = { value: 0 }\nconst b = { value: 0 }\n\
         function setup() { a.value = 1 }\ndescribe('s', setup)\n\
         test('reads a', () => { expect(a.value).toBe(0) })\n\
         test('reads b', () => { expect(b.value).toBe(0) })",
        &[],
        &[("reads a", "reads b")],
    );
}

/// The engine reads a file into one excluded-file entry: unresolved reasons with counts, parse
/// errors as a partial read, refusals as unparsed, and sites only when asked for.
#[test]
fn the_engine_reports_each_file_once() {
    let dir = tempfile::tempdir().unwrap();
    let setting = Setting {
        dialect: &DIALECT,
        globals: false,
        root: dir.path(),
        benchmarks: false,
    };
    let mut engine = Engine::default();
    let mut read = |name: &str, source: &str, keep_sites| {
        let path = dir.path().join(name);
        fs::write(&path, source).unwrap();
        engine.read(
            &path,
            name.as_ref(),
            source,
            setting,
            keep_sites,
            Vec::new(),
        )
    };
    let (entry, partial) = read(
        "a.test.ts",
        "import { test, expect } from 'vitest';
function check() { expect(1).toBe(1); }
test('named', check);
test('empty', () => {});
test('kept', () => { expect(1).toBe(1); });
",
        true,
    );
    let entry = entry.unwrap();
    assert_eq!(entry.reason, ExclusionReason::NamedCallback);
    assert_eq!((entry.tests_excluded, entry.tests_total), (1, 3));
    assert!(!partial);
    let (entry, partial) = read(
        "b.test.ts",
        "import { test } from 'vitest';\nx = a ?? b || c;\ntest('e', () => {});\n",
        false,
    );
    assert_eq!(entry.unwrap().reason, ExclusionReason::ParseError);
    assert!(partial);
    let (entry, partial) = read("c.test.txt", "test('t', () => {});\n", true);
    assert_eq!(entry.unwrap().reason, ExclusionReason::UnsupportedSource);
    assert!(partial);
    let (entry, partial) = read(
        "d.test.ts",
        "import { test } from 'vitest';\ntest('e', () => {});\n",
        true,
    );
    assert!(entry.is_none());
    assert!(!partial);
    let deep = format!("x = {}1{};\n", "[".repeat(300), "]".repeat(300));
    let (entry, _) = read("f.test.ts", &deep, false);
    assert_eq!(entry.unwrap().reason, ExclusionReason::SourceLimit);
    // An unterminated file is refused, not read in part.
    let (entry, _) = read("e.test.ts", "test('t', () => {\n", false);
    assert_eq!(entry.unwrap().reason, ExclusionReason::ParseError);
    assert_eq!((engine.tests, engine.unresolved), (5, 1));
    let sites: Vec<_> = engine
        .sites
        .iter()
        .map(|(path, site)| (path.to_str().unwrap(), site.line))
        .collect();
    assert_eq!(
        sites,
        [
            ("a.test.ts", 3),
            ("a.test.ts", 4),
            ("a.test.ts", 5),
            ("d.test.ts", 2)
        ]
    );
    assert!(engine.blind_files().is_empty());
}

/// A file whose every test is unresolved is blind; the files are listed most tests first.
#[test]
fn the_engine_lists_blind_files_most_tests_first() {
    let dir = tempfile::tempdir().unwrap();
    let setting = Setting {
        dialect: &DIALECT,
        globals: false,
        root: dir.path(),
        benchmarks: false,
    };
    let mut engine = Engine::default();
    for (name, count) in [("a.test.ts", 1), ("b.test.ts", 2)] {
        let source = format!(
            "import {{ test, expect }} from 'vitest';\nfunction check() {{ expect(1).toBe(1); }}\n{}",
            "test('named', check);\n".repeat(count)
        );
        let path = dir.path().join(name);
        engine.read(&path, name.as_ref(), &source, setting, false, Vec::new());
    }
    let blind: Vec<_> = engine
        .blind_files()
        .into_iter()
        .map(|file| (file.path.to_str().unwrap().to_owned(), file.tests))
        .collect();
    assert_eq!(
        blind,
        [("b.test.ts".to_owned(), 2), ("a.test.ts".to_owned(), 1)]
    );
}

/// An incomplete trace is listed behind any real cause, and excludes no test from the run.
#[test]
fn an_incomplete_trace_is_listed_last_and_excludes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let setting = Setting {
        dialect: &DIALECT,
        globals: false,
        root: dir.path(),
        benchmarks: false,
    };
    let source = "import { test, expect } from 'vitest';\n\
                  test('t', () => { if (x()) { expect(1).toBe(1); } });\n";
    let path = dir.path().join("a.test.ts");
    let read = |further| {
        let mut engine = Engine::default();
        let (entry, partial) =
            engine.read(&path, "a.test.ts".as_ref(), source, setting, false, further);
        assert!(!partial);
        assert_eq!(engine.unresolved, 0);
        entry.unwrap()
    };
    let alone = read(Vec::new());
    assert_eq!(alone.reason, ExclusionReason::IncompleteTrace);
    assert_eq!(alone.tests_excluded, 0);
    let real = (ExclusionReason::GlobalsUndetermined, "unknown".to_owned());
    let behind = read(vec![real]);
    assert_eq!(behind.reason, ExclusionReason::GlobalsUndetermined);
    assert_eq!(behind.other_reasons, [ExclusionReason::IncompleteTrace]);
}

/// A recovered panic excludes its file and drops the run's caches, which it may have left
/// half-written; a thread that could not start excludes its file and keeps them.
#[test]
fn a_recovered_panic_drops_the_run_caches() {
    let dir = tempfile::tempdir().unwrap();
    let setting = Setting {
        dialect: &DIALECT,
        globals: false,
        root: dir.path(),
        benchmarks: false,
    };
    let fresh = format!("{:?}", Run::default());
    let mut engine = Engine::default();
    fs::write(dir.path().join("b.ts"), "export const b = 1;\n").unwrap();
    let source = "import { test, expect } from 'vitest';\nimport { b } from './b';\ntest('t', () => { expect(b).toBe(1); });\n";
    let path = dir.path().join("a.test.ts");
    engine.read(
        &path,
        "a.test.ts".as_ref(),
        source,
        setting,
        false,
        Vec::new(),
    );
    assert_ne!(format!("{:?}", engine.run), fresh);
    // A thread that never started touched nothing: the caches stay.
    let (entry, partial) = engine.record(
        "s.test.ts".as_ref(),
        Err(Failure::Start("refused".to_owned())),
        false,
        Vec::new(),
        false,
        None,
    );
    let entry = entry.unwrap();
    assert_eq!(entry.reason, ExclusionReason::ExtractionFailed);
    assert!(entry.detail.contains("could not start"), "{}", entry.detail);
    assert!(partial);
    assert_ne!(format!("{:?}", engine.run), fresh);
    let (entry, partial) = engine.record(
        "p.test.ts".as_ref(),
        Err(Failure::Panicked),
        false,
        Vec::new(),
        false,
        None,
    );
    assert_eq!(entry.unwrap().reason, ExclusionReason::ExtractionFailed);
    assert!(partial);
    assert_eq!(format!("{:?}", engine.run), fresh);
}

/// A call of a helper that registers only hooks is no excluded case: it registers no test, and
/// its hooks key the tests of its scope. One edit away, a test in the helper makes the call one
/// `registered-elsewhere` case.
#[test]
fn a_hooks_only_helper_call_is_context_not_an_excluded_test() {
    let server = |extra: &str| {
        format!(
            "import {{ afterEach, beforeEach, test }} from 'vitest'\n\
             export function setupServer() {{ beforeEach(() => {{}}); afterEach(() => {{}}); {extra} }}\n"
        )
    };
    let main = "import { describe, expect, test } from 'vitest'
import { setupServer } from './server'
describe('with', () => {
  setupServer()
  test('t', () => { expect(1).toBe(1) })
})
describe('without', () => {
  test('t', () => { expect(1).toBe(1) })
})
";
    let hooks_only = server("");
    let corpus = corpora(
        &[("server.ts", &hooks_only), ("a.test.ts", main)],
        &["a.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 2, "{:?}", corpus.tests);
    assert!(corpus.tests.iter().all(|test| test.exclusion.is_none()));
    assert_ne!(
        corpus.tests[0].key, corpus.tests[1].key,
        "the helper's hooks key its scope"
    );
    assert!(corpus.sites.iter().any(|site| site.kind == "hook-wrapper"));
    let registers = server("test('inside', () => {})");
    let corpus = corpora(
        &[("server.ts", &registers), ("a.test.ts", main)],
        &["a.test.ts"],
    )
    .remove(0);
    assert_eq!(
        corpus.exclusions().get(&Exclusion::RegisteredElsewhere),
        Some(&1),
        "{:?}",
        corpus.tests
    );
}

/// A static returned-object leaf and an independent proved helper expand in one atomic batch.
#[test]
fn an_inert_factory_result_can_coexist_with_a_proved_helper() {
    let runner = include_str!("../../fixtures/oracle/factory-result-binding/runner.ts");
    let factory = include_str!("../../fixtures/oracle/factory-result-binding/factory.ts");
    for (path, source) in [
        (
            "direct.test.ts",
            include_str!("../../fixtures/oracle/factory-result-binding/direct.test.ts"),
        ),
        (
            "destructured.test.ts",
            include_str!("../../fixtures/oracle/factory-result-binding/destructured.test.ts"),
        ),
    ] {
        let corpus = corpora(
            &[
                (path, source),
                ("factory.ts", factory),
                ("runner.ts", runner),
            ],
            &[path],
        )
        .remove(0);
        assert_eq!(corpus.tests.len(), 2, "{path}: {:?}", corpus.tests);
        assert!(
            corpus.tests.iter().all(|case| case.exclusion.is_none()),
            "{path}: {:?}",
            corpus.tests
        );
    }

    let source = "import {make} from './factory';import {run} from './runner';\
        run();const result=make('shared');result.cases.nested.read(1)";
    let ordered = corpora(
        &[
            ("ordered.test.ts", source),
            ("factory.ts", factory),
            ("runner.ts", runner),
        ],
        &["ordered.test.ts"],
    )
    .remove(0);
    assert_eq!(
        ordered
            .tests
            .iter()
            .map(|case| case.title.as_deref())
            .collect::<Vec<_>>(),
        [Some("inside"), Some("shared 1")],
        "{:?}",
        ordered.tests
    );

    let unused_object = corpora(
        &[
            (
                "unused-object.test.ts",
                "import {make} from './factory';const r=make();r.cases.read()",
            ),
            (
                "factory.ts",
                "import {test,expect} from 'vitest';export function make(){const defaults={timeout:5};\
                 return {cases:{read:()=>test('unused object',()=>expect(1).toBe(1))}}}",
            ),
        ],
        &["unused-object.test.ts"],
    )
    .remove(0);
    assert_eq!(unused_object.tests.len(), 1, "{:?}", unused_object.tests);
    assert_eq!(
        unused_object.tests[0].exclusion, None,
        "{:?}",
        unused_object.tests
    );

    let derived_factory = "import {test} from 'vitest';export function make({enabled=true}){\
        const identity=fn=>fn;const conditional=fn=>fn.runIf(enabled);return {cases:{\
        plain:value=>identity(test)('plain',()=>value),\
        conditional:value=>conditional(test)('conditional',()=>value)}}}";
    let derived_caller = "import {make} from './factory';const {cases}=make({});\
        cases.plain(1);cases.conditional(1)";
    let derived = corpora(
        &[
            ("derived.test.ts", derived_caller),
            ("factory.ts", derived_factory),
        ],
        &["derived.test.ts"],
    )
    .remove(0);
    assert_eq!(derived.tests.len(), 2, "{:?}", derived.tests);
    assert!(
        derived.tests.iter().all(|case| case.exclusion.is_none()),
        "{:?}",
        derived.tests
    );
    assert_ne!(derived.tests[0].key, derived.tests[1].key);

    let imported_transform = corpora(
        &[
            (
                "imported.test.ts",
                "import {make} from './factory';const result=make();result.cases.read(1)",
            ),
            (
                "factory.ts",
                "import {test} from 'vitest';import {only} from './only';export function make(){\
                 const r=only;return {cases:{read:value=>r(test)('imported',()=>value)}}}",
            ),
            ("only.ts", "export function only(fn){return fn.runIf(true)}"),
        ],
        &["imported.test.ts"],
    )
    .remove(0);
    assert_eq!(
        imported_transform.tests.len(),
        1,
        "{:?}",
        imported_transform.tests
    );
    assert!(
        imported_transform.tests[0].exclusion.is_none(),
        "{:?}",
        imported_transform.tests
    );

    let imported_data = corpora(
        &[
            (
                "data.test.ts",
                "import {make} from './factory';const result=make();result.cases.read()",
            ),
            (
                "factory.ts",
                "import {test,expect} from 'vitest';import {data} from './data';export function make(){\
                 const captured=data;return {cases:{read:()=>test('data',()=>expect(captured).toBe(1))}}}",
            ),
            ("data.ts", "export const data=1"),
        ],
        &["data.test.ts"],
    )
    .remove(0);
    assert!(
        imported_data
            .tests
            .iter()
            .any(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)),
        "{:?}",
        imported_data.tests
    );

    let uncertain_transform = corpora(
        &[
            (
                "uncertain.test.ts",
                "import {make} from './factory';const result=make();result.cases.read(1)",
            ),
            (
                "factory.ts",
                "import {test} from 'vitest';import {only} from './only';export function make(){\
                 const r=flag?only:fn=>fn;return {cases:{read:value=>r(test)('uncertain',()=>value)}}}",
            ),
            ("only.ts", "export function only(fn){return fn.runIf(true)}"),
        ],
        &["uncertain.test.ts"],
    )
    .remove(0);
    assert!(
        uncertain_transform
            .tests
            .iter()
            .any(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)),
        "{:?}",
        uncertain_transform.tests
    );

    let caller = |binding: &str| {
        format!("import {{make}} from './factory';import {{run}} from './runner';{binding};run();")
    };
    for (factory, binding) in [
        (
            "export function make(){effect();return {cases:{}}}",
            "const cases=make()",
        ),
        (
            "export function make(flag){if(flag)return {cases:{}};return {cases:{}}}",
            "const cases=make(true)",
        ),
        (
            "export function make(){return {...other}}",
            "const cases=make()",
        ),
        (
            "export function make(){return {cases:{run(){}}}}",
            "const cases=make();cases[key]()",
        ),
    ] {
        let source = caller(binding);
        let corpus = corpora(
            &[
                ("a.test.ts", &source),
                ("factory.ts", factory),
                ("runner.ts", runner),
            ],
            &["a.test.ts"],
        )
        .remove(0);
        assert!(
            corpus
                .tests
                .iter()
                .any(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)),
            "{binding}: {:?}",
            corpus.tests
        );
    }
}

/// Factory and leaf frames both participate in the final comparison outcome.
#[test]
fn returned_closure_frames_preserve_equivalence_and_conflicts() {
    let factory = include_str!("../../fixtures/oracle/factory-result-binding/factory.ts");
    let callers = [
        (
            "same-a.test.ts",
            "import {make} from './factory';const r=make('same');r.cases.nested.read(1)",
        ),
        (
            "same-b.test.ts",
            "import {make} from './factory';const r=make('same');r.cases.nested.read(1)",
        ),
        (
            "factory-diff.test.ts",
            "import {make} from './factory';const r=make('other');r.cases.nested.read(1)",
        ),
        (
            "leaf-diff.test.ts",
            "import {make} from './factory';const r=make('same');r.cases.nested.read(2)",
        ),
    ];
    let mut files = callers.to_vec();
    files.push(("factory.ts", factory));
    let paths: Vec<_> = callers.iter().map(|(path, _)| *path).collect();
    let corpora = corpora(&files, &paths);
    let keys: Vec<_> = corpora
        .iter()
        .map(|corpus| {
            assert_eq!(corpus.tests.len(), 1, "{:?}", corpus.tests);
            assert!(corpus.tests[0].exclusion.is_none(), "{:?}", corpus.tests);
            corpus.tests[0].key
        })
        .collect();
    assert_eq!(keys[0], keys[1]);
    assert_ne!(keys[0], keys[2]);
    assert_ne!(keys[0], keys[3]);
}

/// Invocation substitution refuses runtime identity and array-hole distinctions JSON erases.
#[test]
fn invocation_frames_refuse_erased_reference_semantics() {
    for (name, helper, first, second) in [
        (
            "identity",
            "import {test,expect} from 'vitest';function inner(x,y,title){test(title,()=>expect(x===y).toBe(true))}\
             export function same(a){inner(a,a,'same')}export function distinct(a){inner(a,{},'distinct')}",
            "import {same} from './helper';same({})",
            "import {distinct} from './helper';distinct({})",
        ),
        (
            "holes",
            "import {test,expect} from 'vitest';export function run(a,title){test(title,()=>expect(a[0]).toBe(null))}",
            "import {run} from './helper';run([,],'hole')",
            "import {run} from './helper';run([null],'null')",
        ),
    ] {
        let paths = [format!("{name}-a.test.ts"), format!("{name}-b.test.ts")];
        let files = [
            (paths[0].as_str(), first),
            (paths[1].as_str(), second),
            ("helper.ts", helper),
        ];
        let corpora = corpora(&files, &[paths[0].as_str(), paths[1].as_str()]);
        assert_eq!(
            corpora.iter().map(|corpus| corpus.tests.len()).sum::<usize>(),
            2,
            "{name}: {corpora:?}"
        );
        let outcome = owned_outcome(
            paths
                .iter()
                .zip(corpora)
                .map(|(path, corpus)| (path.as_str(), corpus))
                .collect(),
        );
        assert!(
            outcome
                .findings
                .iter()
                .all(|finding| finding.rule != crate::model::Rule::CrossFileDuplicate),
            "{name}: {:?}",
            outcome.findings
        );
    }
}

/// Returned factories refuse defaults and transforms whose actual JavaScript result is unmodeled.
#[test]
fn returned_factories_refuse_inherited_defaults_and_async_transforms() {
    let inherited_factory =
        "import {test,expect} from 'vitest';export function make({constructor=1}){\
        return {cases:{read:()=>test('case',()=>expect(constructor).toBe(1))}}}";
    let paths = ["inherited.test.ts", "own.test.ts"];
    let files = [
        (
            paths[0],
            "import {make} from './factory';const r=make({});r.cases.read()",
        ),
        (
            paths[1],
            "import {make} from './factory';const r=make({constructor:1});r.cases.read()",
        ),
        ("factory.ts", inherited_factory),
    ];
    let results: Vec<_> = runs(&files, &[&[paths[0]], &[paths[1]]])
        .into_iter()
        .map(|mut run| run.remove(0))
        .collect();
    assert!(
        results[0]
            .tests
            .iter()
            .all(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)),
        "{:?}",
        results[0].tests
    );
    assert_eq!(results[1].tests.len(), 1, "{:?}", results[1].tests);
    assert!(
        results[1].tests[0].exclusion.is_none(),
        "{:?}",
        results[1].tests
    );
    let outcome = owned_outcome(paths.iter().copied().zip(results).collect());
    assert_rule_absent(outcome, crate::model::Rule::CrossFileDuplicate);

    let nested_factory =
        "import {test,expect} from 'vitest';export function make({ignored:{},value}){\
        return {cases:{read:()=>test('case',()=>expect(value).toBe(1))}}}";
    let paths = ["missing-nested.test.ts", "present-nested.test.ts"];
    let files = [
        (
            paths[0],
            "import {make} from './factory';const r=make({value:1});r.cases.read()",
        ),
        (
            paths[1],
            "import {make} from './factory';const r=make({ignored:{},value:1});r.cases.read()",
        ),
        ("factory.ts", nested_factory),
    ];
    let nested_results: Vec<_> = runs(&files, &[&[paths[0]], &[paths[1]]])
        .into_iter()
        .map(|mut run| run.remove(0))
        .collect();
    assert_all_wrappers_refused(&nested_results);
    let outcome = owned_outcome(paths.iter().copied().zip(nested_results).collect());
    assert_rule_absent(outcome, crate::model::Rule::CrossFileDuplicate);

    for transform in ["async fn=>fn", "function*(fn){return fn}"] {
        let factory = format!(
            "import {{test}} from 'vitest';export function make(){{const identity={transform};\
             return {{cases:{{read:()=>identity(test)('case',()=>{{}})}}}}}}"
        );
        let corpus = factory_corpus(
            &factory,
            "import {make} from './factory';const r=make();r.cases.read()",
        );
        assert!(
            corpus
                .tests
                .iter()
                .any(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)),
            "{transform}: {:?}",
            corpus.tests
        );
    }

    for (factory, call) in [
        (
            "import {test} from 'vitest';export function make(unused=test('hidden',()=>{})){\
             return {cases:{read:()=>test('case',()=>{})}}}",
            "make()",
        ),
        (
            "import {test} from 'vitest';export function make({unused=boom()}){\
             return {cases:{read:()=>test('case',()=>{})}}}",
            "make({})",
        ),
    ] {
        let caller = format!("import {{make}} from './factory';const r={call};r.cases.read()");
        let corpus = corpora(
            &[("case.test.ts", &caller), ("factory.ts", factory)],
            &["case.test.ts"],
        )
        .remove(0);
        assert!(
            corpus
                .tests
                .iter()
                .any(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)),
            "{factory}: {:?}",
            corpus.tests
        );
    }

    for condition in ["test('hidden',()=>{})", "boom()"] {
        let factory = format!(
            "import {{test}} from 'vitest';export function make(){{\
             const conditional=fn=>fn.runIf({condition});\
             return {{cases:{{read:()=>conditional(test)('case',()=>{{}})}}}}}}"
        );
        let corpus = factory_corpus(
            &factory,
            "import {make} from './factory';const r=make();r.cases.read()",
        );
        assert!(
            corpus
                .tests
                .iter()
                .any(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)),
            "{condition}: {:?}",
            corpus.tests
        );
    }

    let mutable_factory = "import {test,expect} from 'vitest';export function make(){\
        const id=fn=>fn;return {cases:{\
        left:(x,y)=>id(test)('left',()=>{x=2;expect(y).toBe(1)}),\
        right:(x,y)=>id(test)('right',()=>{y=2;expect(y).toBe(1)})}}}";
    let mutable = corpora(
        &[
            (
                "mutable.test.ts",
                "import {make} from './factory';const r=make();r.cases.left(1,1);r.cases.right(1,1)",
            ),
            ("factory.ts", mutable_factory),
        ],
        &["mutable.test.ts"],
    )
    .remove(0);
    assert_eq!(mutable.tests.len(), 3, "{:?}", mutable.tests);
    assert!(
        mutable
            .tests
            .iter()
            .all(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)),
        "{:?}",
        mutable.tests
    );
    let outcome = owned_findings("mutable.test.ts", mutable);
    assert_rule_absent(outcome, crate::model::Rule::DuplicateTest);

    let captured_factory = "import {test,expect} from 'vitest';export function make(mode){\
        function get(){return mode}return {cases:{read:()=>test('case',()=>expect(get()).toBe(1))}}}";
    let paths = ["captured-a.test.ts", "captured-b.test.ts"];
    let files = [
        (
            paths[0],
            "import {make} from './factory';const r=make(1);r.cases.read()",
        ),
        (
            paths[1],
            "import {make} from './factory';const r=make(2);r.cases.read()",
        ),
        ("factory.ts", captured_factory),
    ];
    let captured_results: Vec<_> = runs(&files, &[&[paths[0]], &[paths[1]]])
        .into_iter()
        .map(|mut run| run.remove(0))
        .collect();
    assert_all_wrappers_refused(&captured_results);
    let outcome = owned_outcome(paths.iter().copied().zip(captured_results).collect());
    assert_rule_absent(outcome, crate::model::Rule::CrossFileDuplicate);

    let nested_callable_factory = "import {test,expect} from 'vitest';export function make(mode){\
        function get(){return ()=>mode}return {cases:{\
        read:()=>test('case',()=>expect(get()()).toBe(1))}}}";
    let files = [
        (
            paths[0],
            "import {make} from './factory';const r=make(1);r.cases.read()",
        ),
        (
            paths[1],
            "import {make} from './factory';const r=make(2);r.cases.read()",
        ),
        ("factory.ts", nested_callable_factory),
    ];
    let nested_results = corpora(&files, &paths);
    assert_all_wrappers_refused(&nested_results);
    let outcome = owned_outcome(paths.iter().copied().zip(nested_results).collect());
    assert_rule_absent(outcome, crate::model::Rule::CrossFileDuplicate);

    let sibling_factory = "import {test,expect} from 'vitest';let state=0;const id=fn=>fn;\
        export function make(){return {cases:{\
        write:()=>id(test)('write',()=>{state=1}),\
        read:()=>id(test)('read',()=>expect(state).toBe(0))}}}";
    let files = [
        (
            paths[0],
            "import {make} from './factory';const r=make();r.cases.write();r.cases.read()",
        ),
        (
            paths[1],
            "import {make} from './factory';const r=make();r.cases.read()",
        ),
        ("factory.ts", sibling_factory),
    ];
    let sibling_results = corpora(&files, &paths);
    let read_cases: Vec<_> = sibling_results
        .iter()
        .flat_map(|corpus| &corpus.tests)
        .filter(|case| case.title.as_deref() == Some("read"))
        .collect();
    assert_eq!(read_cases.len(), 2, "{sibling_results:?}");
    assert_eq!(
        read_cases[0].exclusion,
        Some(Exclusion::OrderDependentState),
        "{sibling_results:?}"
    );
    assert_eq!(read_cases[1].exclusion, None, "{sibling_results:?}");
    let outcome = owned_outcome(paths.iter().copied().zip(sibling_results).collect());
    assert_rule_absent(outcome, crate::model::Rule::CrossFileDuplicate);

    let redeclared_transform = "import {test} from 'vitest';export function make(){\
        function id(fn){return fn}function id(fn){return fn.fails}\
        return {cases:{read:()=>id(test)('derived',()=>{})}}}";
    let source = "import {test} from 'vitest';import {make} from './factory';\
        const r=make();r.cases.read();test('ordinary',()=>{})";
    let corpus = corpora(
        &[
            ("redeclared-transform.test.ts", source),
            ("factory.ts", redeclared_transform),
        ],
        &["redeclared-transform.test.ts"],
    )
    .remove(0);
    assert!(
        corpus
            .tests
            .iter()
            .any(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)),
        "{:?}",
        corpus.tests
    );
    let outcome = owned_findings("redeclared-transform.test.ts", corpus);
    assert_rule_absent(outcome, crate::model::Rule::DuplicateTest);
}

/// Direct runner expansion retains JavaScript's harmless arity differences and refuses a
/// parameter whose runtime binding is replaced by a hoisted declaration.
#[test]
fn returned_transform_aliases_require_initialized_bindings() {
    for (setup, admitted) in [
        ("const second=only;", true),
        ("const second=first;const first=only;", false),
    ] {
        let factory = format!(
            "import {{test,expect}} from 'vitest';import {{only}} from './only';\
             export function make(){{{setup}return {{cases:{{read:()=>\
             second(test)('alias',()=>expect(1).toBe(1))}}}}}}"
        );
        let corpus = corpora(
            &[
                (
                    "alias.test.ts",
                    "import {make} from './factory';const r=make();r.cases.read()",
                ),
                ("factory.ts", &factory),
                ("only.ts", "export function only(fn){return fn}"),
            ],
            &["alias.test.ts"],
        )
        .remove(0);
        if admitted {
            assert_eq!(corpus.tests.len(), 1, "{:?}", corpus.tests);
            assert_eq!(corpus.tests[0].exclusion, None);
        } else {
            assert_eq!(corpus.tests.len(), 2, "{:?}", corpus.tests);
            assert!(corpus
                .tests
                .iter()
                .all(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere)));
        }
    }
}

/// Mutation facts cross factory instances and empty local factories still execute their setup.
#[test]
fn returned_batches_share_mutation_and_require_initialized_factories() {
    let factory = include_str!("../../fixtures/oracle/factory-batch-mutation/factory.ts");
    let results = corpora(
        &[
            (
                "a.test.ts",
                include_str!("../../fixtures/oracle/factory-batch-mutation/a.test.ts"),
            ),
            (
                "b.test.ts",
                include_str!("../../fixtures/oracle/factory-batch-mutation/b.test.ts"),
            ),
            ("factory.ts", factory),
        ],
        &["a.test.ts", "b.test.ts"],
    );
    assert_mutation_reader(&results);
    let reversed = corpora(
        &[
            (
                "reversed.test.ts",
                include_str!("../../fixtures/oracle/factory-batch-mutation/reversed.test.ts"),
            ),
            ("factory.ts", factory),
        ],
        &["reversed.test.ts"],
    )
    .remove(0);
    assert_eq!(reversed.tests.len(), 2);
    assert_eq!(
        reversed.tests[0].exclusion,
        Some(Exclusion::OrderDependentState)
    );
    for (body, admitted) in [
        (
            include_str!("../../fixtures/oracle/factory-initialization/late-call.test.ts"),
            false,
        ),
        (
            include_str!("../../fixtures/oracle/factory-initialization/late-read.test.ts"),
            false,
        ),
        (
            include_str!("../../fixtures/oracle/factory-initialization/control.test.ts"),
            true,
        ),
    ] {
        let result = corpora(
            &[
                ("case.test.ts", body),
                (
                    "runner.ts",
                    include_str!("../../fixtures/oracle/factory-initialization/runner.ts"),
                ),
            ],
            &["case.test.ts"],
        )
        .remove(0);
        assert!(!result.tests.is_empty());
        assert_eq!(
            result.tests.iter().all(|test| test.exclusion.is_none()),
            admitted,
            "{body}: {:?}",
            result.tests
        );
    }
}

/// Captured setup strings must pay for every leaf's rendered substitutions.
#[test]
fn returned_factory_capture_work_refuses_amplification() {
    let factory = include_str!("../../fixtures/oracle/factory-resource-work/copies.ts");
    let caller = include_str!("../../fixtures/oracle/factory-resource-work/copies.test.ts");
    let analyze = |factory: &str, caller: &str| {
        corpora(
            &[("copies.ts", factory), ("case.test.ts", caller)],
            &["case.test.ts"],
        )
        .remove(0)
    };
    let refused = analyze(factory, caller);
    assert_eq!(refused.tests.len(), 2, "{:?}", refused.tests);
    assert!(refused.tests.iter().all(|case| case.exclusion.is_some()));
    let caller = include_str!("../../fixtures/oracle/factory-resource-work/copies-control.test.ts");
    let retained = analyze(factory, caller);
    assert_eq!(retained.tests.len(), 2, "{:?}", retained.tests);
    assert!(retained.tests.iter().all(|case| case.exclusion.is_none()));
    assert_eq!(retained.tests[0].key, retained.tests[1].key);
    let outcome = owned_findings("case.test.ts", retained);
    assert!(outcome
        .findings
        .iter()
        .any(|finding| finding.rule == crate::model::Rule::DuplicateTest));
}

/// Proved local returned leaves use their invocation frames exactly once.
#[test]
fn returned_local_leaves_preserve_equal_and_conflicting_frames() {
    let source = include_str!("../../fixtures/oracle/factory-local-leaves/case.test.ts");
    for source in [
        source,
        include_str!("../../fixtures/oracle/factory-local-leaves/const.test.ts"),
        include_str!("../../fixtures/oracle/factory-local-leaves/overload.test.ts"),
        include_str!("../../fixtures/oracle/factory-local-leaves/unused.test.ts"),
    ] {
        let corpus = corpora(&[("case.test.ts", source)], &["case.test.ts"]).remove(0);
        assert_eq!(corpus.tests.len(), 3, "{:?}", corpus.tests);
        assert!(corpus.tests.iter().all(|case| case.exclusion.is_none()));
        assert_eq!(corpus.tests[0].key, corpus.tests[1].key);
        assert_ne!(corpus.tests[0].key, corpus.tests[2].key);
        let outcome = owned_findings("case.test.ts", corpus);
        let duplicates: Vec<_> = outcome
            .findings
            .iter()
            .filter(|finding| finding.rule == crate::model::Rule::DuplicateTest)
            .collect();
        assert_eq!(duplicates.len(), 1, "{:?}", outcome.findings);
    }
    for source in [
        format!("{source}\nconst escaped=make"),
        source.replace("a.read()", "make=()=>({read:()=>{}});a.read()"),
    ] {
        let corpus = corpora(&[("case.test.ts", &source)], &["case.test.ts"]).remove(0);
        assert!(!corpus.tests.is_empty());
        assert!(corpus.tests.iter().any(|case| case.exclusion.is_some()));
        assert_rule_absent(
            owned_findings("case.test.ts", corpus),
            crate::model::Rule::DuplicateTest,
        );
    }
}

/// Derived conditions retain discovery and identity while controlling hook activation.
#[test]
fn derived_conditional_registration_preserves_activation() {
    let factory = include_str!("../../fixtures/oracle/factory-conditional-activation/factory.ts");
    let source = include_str!("../../fixtures/oracle/factory-conditional-activation/case.test.ts");
    for (input, truthy) in [
        ("false", false),
        ("true", true),
        ("0", false),
        ("-0", false),
        ("null", false),
        ("''", false),
        ("1", true),
        ("-2", true),
        ("'x'", true),
    ] {
        let caller = source.replace("make(false)", &format!("make({input})"));
        let corpus = factory_corpus(factory, &caller);
        assert_eq!(corpus.tests.len(), 4, "{input}: {:?}", corpus.tests);
        assert!(corpus.tests.iter().all(|case| case.exclusion.is_none()));
        for (case, inert) in corpus.tests.iter().zip([!truthy, truthy, false, true]) {
            assert_eq!(case.inert, inert, "{input}: {:?}", corpus.tests);
            assert_eq!(case.witnesses_hooks(), !inert, "{input}");
        }
        if !truthy {
            assert_ne!(corpus.tests[0].key, corpus.tests[2].key, "{input}");
        }
    }
}

/// An ordinary imported reader observes writes registered by a factory in the same module.
#[test]
fn ordinary_runner_frames_share_factory_mutation_facts() {
    let results = corpora(
        &[
            (
                "a.test.ts",
                include_str!("../../fixtures/oracle/factory-mixed-mutation/a.test.ts"),
            ),
            (
                "b.test.ts",
                include_str!("../../fixtures/oracle/factory-mixed-mutation/b.test.ts"),
            ),
            (
                "factory.ts",
                include_str!("../../fixtures/oracle/factory-mixed-mutation/factory.ts"),
            ),
        ],
        &["a.test.ts", "b.test.ts"],
    );
    assert_mutation_reader(&results);
}

/// Direct helpers retain harmless arity differences while refusing replaced parameter bindings.
#[test]
fn runner_arity_and_parameter_redeclaration_keep_javascript_semantics() {
    let helper = "import {test,expect} from 'vitest';export function run(unused){test('constant',()=>expect(1).toBe(1))}";
    let paths = ["missing.test.ts", "extra.test.ts"];
    let files = [
        (paths[0], "import {run} from './helper';run()"),
        (paths[1], "import {run} from './helper';run(1,2)"),
        ("helper.ts", helper),
    ];
    let arity_results = corpora(&files, &paths);
    assert!(
        arity_results
            .iter()
            .flat_map(|corpus| &corpus.tests)
            .all(|case| case.exclusion.is_none()),
        "{arity_results:?}"
    );
    let outcome = owned_outcome(paths.iter().copied().zip(arity_results).collect());
    assert_eq!(
        outcome
            .findings
            .iter()
            .filter(|finding| finding.rule == crate::model::Rule::CrossFileDuplicate)
            .count(),
        1,
        "{:?}",
        outcome.findings
    );

    let redeclared = "import {test,expect} from 'vitest';export function run(a){function a(){};\
        test('redeclared',()=>expect(a).toBe(1))}";
    let ordinary = "import {test,expect} from 'vitest';export function run(a){\
        test('ordinary',()=>expect(a).toBe(1))}";
    let paths = ["redeclared.test.ts", "ordinary.test.ts"];
    let files = [
        (paths[0], "import {run} from './redeclared';run(1)"),
        (paths[1], "import {run} from './ordinary';run(1)"),
        ("redeclared.ts", redeclared),
        ("ordinary.ts", ordinary),
    ];
    let redeclared_results = corpora(&files, &paths);
    assert_eq!(
        redeclared_results
            .iter()
            .map(|corpus| corpus.tests.len())
            .sum::<usize>(),
        2,
        "{redeclared_results:?}"
    );
    let outcome = owned_outcome(paths.iter().copied().zip(redeclared_results).collect());
    assert_rule_absent(outcome, crate::model::Rule::CrossFileDuplicate);
}

/// A participant is named the way a report spells it: the suite path, then the title, when every
/// segment is spelled; one dynamic segment leaves the whole path unnamed.
#[test]
fn full_title_joins_the_suite_path_with_the_title_when_every_segment_is_spelled() {
    let corpus = corpora(
        &[(
            "a.test.ts",
            "import { test, describe } from 'vitest'
describe('outer', () => {
  test('inner', () => { expect(1).toBe(1) })
  const name = dynamic();
  describe(name, () => {
    test('under dynamic', () => { expect(1).toBe(1) })
  })
})
test('plain', () => { expect(1).toBe(1) })
",
        )],
        &["a.test.ts"],
    )
    .remove(0);
    let titles: HashMap<String, Option<String>> = corpus
        .tests
        .into_iter()
        .map(|test| (test.title.unwrap_or_default(), test.full_title))
        .collect();
    assert_eq!(titles["inner"].as_deref(), Some("outer > inner"));
    assert_eq!(
        titles["under dynamic"], None,
        "a dynamic suite title leaves the participant unnamed"
    );
    assert_eq!(titles["plain"].as_deref(), Some("plain"));
}

/// An options object marks a case inert the way the chain does, only for a literal `true`.
#[test]
fn a_literal_skip_or_todo_option_marks_a_case_inert() {
    let source = "import { test, expect } from 'vitest'\n\
                  test('skipped', { skip: true }, () => { expect(1).toBe(1) })\n\
                  test('planned', { todo: true }, () => { expect(1).toBe(1) })\n\
                  test('off', { skip: false }, () => { expect(1).toBe(1) })\n\
                  test('computed', { skip: process.env.CI }, () => { expect(1).toBe(1) })\n\
                  test('timed', { timeout: 5 }, () => { expect(1).toBe(1) })\n";
    let corpus = corpora(&[("a.test.ts", source)], &["a.test.ts"]).remove(0);
    let inert: HashMap<Option<&str>, bool> = corpus
        .tests
        .iter()
        .map(|test| (test.title.as_deref(), test.inert))
        .collect();
    assert!(inert[&Some("skipped")] && inert[&Some("planned")]);
    assert!(!inert[&Some("off")] && !inert[&Some("computed")] && !inert[&Some("timed")]);
}

/// `skip` and `todo` change nothing a test checks, so they are not keyed -- but a report marks
/// the participant as inert, so the corpus carries that beside the key.
#[test]
fn skip_marks_a_case_inert_without_changing_its_key() {
    let body = "import { test } from 'vitest'\ntest.skip('same', () => { expect(1).toBe(1) })\ntest.todo('planned')\ntest('eager', () => { expect(1).toBe(1) })\n";
    let corpus = corpora(&[("a.test.ts", body)], &["a.test.ts"]).remove(0);
    let inert: HashMap<Option<&str>, bool> = corpus
        .tests
        .iter()
        .map(|test| (test.title.as_deref(), test.inert))
        .collect();
    assert!(inert[&Some("same")]);
    assert!(inert[&Some("planned")]);
    assert!(!inert[&Some("eager")]);
    let skipped = corpora(&[("a.test.ts", body)], &["a.test.ts"])
        .remove(0)
        .tests
        .remove(0);
    let plain = corpora(
        &[(
            "a.test.ts",
            "import { test } from 'vitest'\ntest('same', () => { expect(1).toBe(1) })\n",
        )],
        &["a.test.ts"],
    )
    .remove(0)
    .tests
    .remove(0);
    assert_eq!(
        skipped.key, plain.key,
        "inertness is recorded beside the key, never inside it: skip is not what the test checks"
    );
    assert_ne!(skipped.inert, plain.inert);
}

/// Exact runtime names from the owned formatter controls; uncertain coercions must stay silent.
#[test]
fn owned_name_formats_match_runtime_without_widening_unknowns() {
    assert_owned_names(
        include_str!("../../fixtures/oracle/name-collisions/formats.test.ts"),
        "formats.test.ts",
        32,
        1,
        &[
            "constant",
            "array 1,,2,3",
            "value 2",
            "for A 1",
            "for B 2",
            "index 0/1",
            "index 1/2",
            "namedTitle",
        ],
    );
}

/// A title Vitest renders differently from a literal never collides with it; `verify-titles` pins
/// every collision here to the runtime's own titles.
#[test]
fn owned_rendering_matches_runtime_or_refuses() {
    assert_owned_names(
        include_str!("../../fixtures/oracle/name-collisions/rendering.test.ts"),
        "rendering.test.ts",
        34,
        0,
        &[
            "position y",
            "scalar first q",
            "index x",
            "next position y",
            "truncated 1",
            "missing undefined",
            "each null null",
            "each number 0",
        ],
    );
}

/// Checks that every collected case retains its row identity before comparing row findings.
fn owned_row_corpus(source: &str, tests: usize, excluded: usize) -> FileCorpus {
    let corpus = counted_corpus(source, "cases.test.ts", tests, excluded);
    assert!(corpus.tests.iter().all(|case| case.row.is_some()));
    corpus
}

/// Dynamic table titles refuse before row proofs, retaining every case and readable-row findings.
#[test]
fn dynamic_row_titles_refuse_before_identity_and_retain_readable_rows() {
    let source = include_str!("../../fixtures/oracle/row-identity-prerequisites/cases.test.ts");
    let corpus = owned_row_corpus(source, 12, 3);
    assert!(corpus.tests[..3].iter().all(|case| {
        case.exclusion == Some(Exclusion::UnresolvedTitle)
            && case.row_class.is_none()
            && case.name.is_none()
    }));
    assert!(corpus.tests[3..6]
        .iter()
        .all(|case| case.exclusion.is_none() && case.row_class.is_some()));
    assert_eq!(corpus.tests[3].row_class, corpus.tests[4].row_class);
    assert_ne!(corpus.tests[3].row_class, corpus.tests[5].row_class);
    assert_owned_row_findings(corpus, &[("positive 1", 2)]);
}

/// The comparison names of a file's cases, sorted, with unnamed cases left out.
fn sorted_names(corpus: &FileCorpus) -> Vec<String> {
    let mut names: Vec<_> = corpus
        .tests
        .iter()
        .filter_map(|case| case.name.as_ref().map(|name| name.join(" > ")))
        .collect();
    names.sort();
    names
}

/// A title a `let` computes from the enclosing suite row is the one Vitest gives each strategy, so
/// a repeated row is reported within its strategy and never across strategies.
#[test]
fn row_computed_titles_follow_each_strategy() {
    let source = include_str!("../../fixtures/oracle/row-computed-titles/strategies.test.ts");
    let corpus = counted_corpus(source, "cases.test.ts", 18, 0);
    let mut expected = Vec::new();
    for (strategy, format) in [
        ("default", "{}"),
        ("with-variant", "focus:{}"),
        ("important", "{}!"),
        ("prefix", "tw:{}"),
    ] {
        let spell = |candidate: &str| format.replace("{}", candidate);
        for (index, (input, output)) in [("flex", "flex"), ("grid", "block"), ("flex", "flex")]
            .into_iter()
            .enumerate()
        {
            expected.push(format!(
                "{strategy} > {} → {} ({index})",
                spell(input),
                spell(output)
            ));
        }
    }
    // A title without `%#`, so its repeated row is reported.
    for strategy in ["plain", "important"] {
        let mark = if strategy == "plain" { "" } else { "!" };
        for (input, output) in [("flex", "flex"), ("grid", "block"), ("flex", "flex")] {
            expected.push(format!(
                "repeat {strategy} > {input}{mark} → {output}{mark}"
            ));
        }
    }
    expected.sort();
    assert_eq!(sorted_names(&corpus), expected);
    let outcome = owned_findings("cases.test.ts", corpus);
    let mut groups: Vec<Vec<String>> = outcome
        .findings
        .iter()
        .filter(|finding| finding.rule == crate::model::Rule::DuplicateEachRow)
        .map(|finding| finding.titles.clone())
        .collect();
    groups.sort();
    assert_eq!(
        groups,
        [
            [
                "repeat important > flex! → flex!",
                "repeat important > flex! → flex!"
            ],
            ["repeat plain > flex → flex", "repeat plain > flex → flex"],
        ]
    );
}

/// A `let` title is read only when its writes are `=` statements the declaring body runs before
/// the read, or, inside its suite callbacks, by the time it returns, under decided branches. The
/// suite row values reach titles a test spells from them.
#[test]
fn let_titles_replay_direct_writes_and_refuse_the_rest() {
    let admitted = include_str!("../../fixtures/oracle/row-computed-titles/admitted.test.ts");
    let corpus = counted_corpus(admitted, "cases.test.ts", 11, 0);
    assert_eq!(
        sorted_names(&corpus),
        [
            "module > module after",
            "outer > inner > nested after",
            "plain let",
            "second",
            "x > base-x",
            "x > x",
            "x > x as template",
            "y > base-other",
            "y > y",
            "y > y as template",
            "é-é𝒳",
        ]
    );
    let refused = include_str!("../../fixtures/oracle/row-computed-titles/refusals.test.ts");
    let corpus = counted_corpus(refused, "cases.test.ts", 11, 11);
    assert!(corpus
        .tests
        .iter()
        .all(|case| case.exclusion == Some(Exclusion::UnresolvedTitle) && case.name.is_none()));
    // A module that may replace `replaceAll` keeps its runtime title unknown.
    let patched = include_str!("../../fixtures/oracle/row-computed-titles/patched.test.ts");
    let corpus = counted_corpus(patched, "cases.test.ts", 1, 1);
    assert_eq!(corpus.tests[0].exclusion, Some(Exclusion::UnresolvedTitle));
    // A suite callback a sibling writes in, a body that may return early or that awaits, and a
    // callback that runs while the body does.
    let deferred = include_str!("../../fixtures/oracle/row-computed-titles/deferred.test.ts");
    let corpus = counted_corpus(deferred, "cases.test.ts", 6, 5);
    assert_eq!(
        corpus
            .tests
            .iter()
            .filter(|case| case.exclusion == Some(Exclusion::UnresolvedTitle))
            .count(),
        5
    );
}

/// Pins the exact duplicate-row groups and their common source locations.
fn assert_owned_row_findings(corpus: FileCorpus, expected: &[(&str, usize)]) {
    let outcome = owned_findings("cases.test.ts", corpus);
    let rows: Vec<_> = outcome
        .findings
        .iter()
        .filter(|finding| finding.rule == crate::model::Rule::DuplicateEachRow)
        .collect();
    assert_eq!(rows.len(), expected.len(), "{rows:#?}");
    for (finding, &(title, count)) in rows.iter().zip(expected) {
        assert_eq!(finding.locations.len(), count);
        assert_eq!(finding.titles, vec![title; count]);
        assert!(finding
            .locations
            .iter()
            .all(|location| location == &finding.locations[0]));
    }
}

/// The owned runtime source pins input equality, observability and rendering as final findings.
#[test]
fn owned_duplicate_rows_require_proven_inputs_and_keep_every_group() {
    let source = include_str!("../../fixtures/oracle/duplicate-rows/cases.test.ts");
    let corpus = owned_row_corpus(source, 75, 2);
    assert_eq!(
        corpus
            .tests
            .iter()
            .filter(|case| case.row_class.is_some())
            .count(),
        57
    );
    let expected = [
        ("scalar 1", 2),
        ("ignored 1", 2),
        ("rest repeat", 2),
        ("whole 3 4", 2),
        ("object", 2),
        ("groups 1", 2),
        ("groups 2", 2),
        ("spanning", 2),
        ("spanning", 2),
        ("cluster", 5),
        ("primitive 7", 2),
        ("literal array", 2),
        ("empty row", 2),
        ("unicode é😀", 2),
        ("encoded title \u{fffd}d800", 2),
        ("whole const 1", 2),
    ];
    assert_owned_row_findings(corpus, &expected);
}

/// Erased wrappers and outer holes preserve the runtime's rows, names and input distinctions.
#[test]
fn owned_wrapped_sparse_tables_keep_rows_and_original_indexes() {
    let source = include_str!("../../fixtures/oracle/wrapped-sparse-tables/cases.test.ts");
    let corpus = owned_row_corpus(source, 40, 2);
    let indexes = |line| {
        corpus
            .tests
            .iter()
            .filter(|case| case.span.start.line == line)
            .map(|case| case.row.unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(indexes(14), [1, 3, 4]);
    assert_eq!(indexes(15), [1, 3, 4]);
    assert_eq!(indexes(19), [0, 2], "undefined is not an elision");
    assert_eq!(indexes(25), [1, 3], "exclusions keep physical indexes too");
    let titles: Vec<_> = corpus
        .tests
        .iter()
        .filter(|case| matches!(case.span.start.line, 16 | 17))
        .map(|case| case.name.as_ref().unwrap().join(" > "))
        .collect();
    assert_eq!(
        titles,
        [
            "each index 1 position 2",
            "each index 3 position 4",
            "for index 1 position 2",
            "for index 3 position 4"
        ]
    );
    let expected = [
        "plain 1",
        "const 3",
        "satisfies 5",
        "asserted 7",
        "nonnull 9",
        "nested 11",
        "whole 13",
        "sparse scalar 15",
        "sparse spread 17",
        "sparse whole 19",
    ];
    assert_owned_row_findings(corpus, &expected.map(|title| (title, 2)));
}

/// Run rules over already-counted owned cases with the built-in severities.
fn owned_findings(path: &str, corpus: FileCorpus) -> crate::rules::Outcome {
    owned_outcome(vec![(path, corpus)])
}

/// Run rules over the already-counted owned cases of several files, as one run reports them.
fn owned_outcome(corpora: Vec<(&str, FileCorpus)>) -> crate::rules::Outcome {
    let config = crate::config::Config {
        root: ".".into(),
        include: Vec::new(),
        include_is_explicit: false,
        exclude: Vec::new(),
        rules: Default::default(),
        threshold: 100,
        source: "test".to_owned(),
    };
    let (mut tests, mut suites, mut hooks) = (Vec::new(), Vec::new(), Vec::new());
    for (path, corpus) in corpora {
        let path = std::path::PathBuf::from(path);
        tests.push((path.clone(), corpus.tests));
        suites.push((path.clone(), corpus.suites));
        hooks.push((path, corpus.hooks));
    }
    crate::rules::run_with_hooks(&tests, &suites, &hooks, &config, &[])
}

/// A derived suite table has no proven prefix yet; a direct suite retains row detection.
#[test]
fn row_names_refuse_derived_suite_prefixes() {
    let source = format!("{HEADER}\nconst suite = describe.each(['a', 'b']);\n\
        suite('scope %s', () => {{ test.each([[1], [1]])('row', value => expect(value).toBe(1)) }});\n\
        describe('scope', () => {{ test.each([[1], [1]])('row', value => expect(value).toBe(1)) }});\n\
        import {{ suite as imported }} from './suite';\n\
        imported('imported %s', () => {{ test.each([[1], [1]])('row', value => expect(value).toBe(1)) }});");
    let corpus = corpora(
        &[
            ("cases.test.ts", &source),
            (
                "suite.ts",
                "import { describe } from 'vitest'; export const suite = describe.each(['a', 'b'])",
            ),
        ],
        &["cases.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 7);
    assert_eq!(
        corpus
            .tests
            .iter()
            .filter(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere))
            .count(),
        1
    );
    assert_eq!(
        corpus
            .tests
            .iter()
            .filter(|case| case.row.is_some())
            .count(),
        6
    );
    let findings = owned_findings("cases.test.ts", corpus).findings;
    let rows: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == crate::model::Rule::DuplicateEachRow)
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].titles, ["scope > row", "scope > row"]);
}

mod suites;

mod bench_admission;
mod bindings;
mod hooks;
mod parameters;

/// The permanent owned positive is extracted before its directional finding is inspected.
#[test]
fn subsumption_owned_positive_reaches_final_findings() {
    let source = include_str!("../../fixtures/duplicates/subsumed-smoke-subset.test.ts");
    let corpus = corpora(&[("a.test.ts", source)], &["a.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 2);
    assert!(corpus
        .tests
        .iter()
        .all(|case| case.exclusion.is_none() && case.trace_complete));
    let config = subsumption_config();
    let result = crate::rules::run(&[("a.test.ts".into(), corpus.tests)], &config);
    let findings: Vec<_> = result
        .findings
        .iter()
        .filter(|f| f.rule == crate::model::Rule::SubsumedTest)
        .collect();
    assert_eq!(findings.len(), 1);
    assert_eq!(
        findings[0].titles,
        [
            "cart > applies the coupon",
            "cart > applies the coupon and recalculates"
        ]
    );
}

/// The permanent placeholder regression retains extraction while refusing the new rule.
#[test]
fn subsumption_placeholder_is_not_evidence() {
    let source = include_str!("../../fixtures/duplicates/not-subsumed-placeholder.test.ts");
    let corpus = corpora(&[("a.test.ts", source)], &["a.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 2);
    assert!(corpus
        .tests
        .iter()
        .all(|case| case.exclusion.is_none() && case.trace_complete));
    assert!(corpus.tests[0].subsumption_key.is_none());
    assert!(corpus.tests[1].subsumption_key.is_some());
    let config = subsumption_config();
    let result = crate::rules::run(&[("a.test.ts".into(), corpus.tests)], &config);
    assert!(result
        .findings
        .iter()
        .all(|f| f.rule != crate::model::Rule::SubsumedTest));
}

/// Placeholder spellings are refused while a one-event assertion about real data is retained.
#[test]
fn subsumption_placeholder_family_and_retention() {
    for body in [
        "() => null",
        "() => undefined",
        "() => void 0",
        "() => todo()",
        "() => pending()",
        "() => Promise.resolve()",
        "async () => await pending()",
        "() => { return expect(true).toBe(true) }",
        "function () { expect(true).toBe(true) }",
    ] {
        let source = format!("import {{test, expect}} from 'vitest'; test('stub', {body});");
        let corpus = corpora(&[("a.test.ts", &source)], &["a.test.ts"]).remove(0);
        assert_eq!(corpus.tests.len(), 1, "{body}");
        assert!(corpus.tests[0].subsumption_key.is_none(), "{body}");
    }
    let source = "import {test,expect} from 'vitest'; test('real',()=>expect(value).toBe(true));";
    let corpus = corpora(&[("a.test.ts", source)], &["a.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 1);
    assert!(corpus.tests[0].subsumption_key.is_some());
}

/// Checks final subsumption claims only after the owned source's extraction is accounted for.
fn subsumption_claim_count(source: &str, count: usize) -> usize {
    let corpus = corpora(&[("a.test.ts", source)], &["a.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), count);
    assert!(corpus.tests.iter().all(|case| case.exclusion.is_none()));
    let config = subsumption_config();
    crate::rules::run(&[("a.test.ts".into(), corpus.tests)], &config)
        .findings
        .iter()
        .filter(|f| f.rule == crate::model::Rule::SubsumedTest)
        .count()
}

/// A larger trace can subsume a smaller one only on the same invocation schedule.
#[test]
fn subsumption_requires_equal_repeat_counts() {
    let source = "import {test, expect} from 'vitest';
        test('short', {repeats: 1}, () => {expect(value).toBe(1)});
        test('long', {repeats: 2}, () => {expect(value).toBe(1);expect(other).toBe(2)});";
    assert_eq!(subsumption_claim_count(source, 2), 0);
    assert_eq!(
        subsumption_claim_count(&source.replace("repeats: 1", "repeats: 2"), 2),
        1
    );
}

/// A wrapped skip marks its test inert and a wrapped suite withholds subsumption for its child.
#[test]
fn wrapped_options_mark_inert_cases() {
    let tests = by_title(include_str!(
        "../../fixtures/recall/subsumed-test-wrapped-options/run.test.ts"
    ));
    assert_eq!(tests.len(), 8);
    assert!(tests.values().all(|case| case.exclusion.is_none()));
    assert!(tests["skipped larger"].inert);
    assert!(tests["pending larger"].subsumption_key.is_none());
    assert!(!tests["running larger"].inert);
}

/// An undefined override under an explicit suite count depends on runner merge semantics.
#[test]
fn repeat_overrides_need_proven_counts() {
    let tests = by_title(
        "import {test, describe, expect} from 'vitest';
        const count = 2 as const;
        describe('outer', {repeats: count}, () => {
            test('inherited', () => expect(1).toBe(1));
            test('fixed', {repeats: 2}, () => expect(1).toBe(1));
            test('undefined', {repeats: undefined}, () => expect(1).toBe(1));
            test('invalid', {repeats: -1}, () => expect(1).toBe(1));
            test('timed', () => expect(1).toBe(1), 120_000 * 3);
        });",
    );
    assert_eq!(tests.len(), 5);
    for title in ["inherited", "fixed", "timed"] {
        assert_eq!(tests[title].exclusion, None);
    }
    assert_eq!(tests["inherited"].key, tests["fixed"].key);
    for title in ["undefined", "invalid"] {
        assert_eq!(tests[title].exclusion, Some(Exclusion::UnprovedRepeatCount));
    }
}

/// Fixture-provider identity must participate even when the callback only reads a fixture name.
#[test]
fn subsumption_fixture_provider_identity() {
    let source = include_str!("../../fixtures/duplicates/not-subsumed-different-fixtures.test.ts");
    assert_eq!(subsumption_claim_count(source, 2), 0);
    let same_provider = source.replace("pro('b'", "free('b'");
    assert_eq!(subsumption_claim_count(&same_provider, 2), 1);
}

/// Unknown row populations never anchor this rule, including identical sources and both APIs.
#[test]
fn subsumption_unknown_tables_are_refused() {
    let source = include_str!("../../fixtures/duplicates/not-subsumed-unresolved-tables.test.ts");
    assert_eq!(subsumption_claim_count(source, 3), 0);
    let tagged = "import {test,expect} from 'vitest'; test.each`n\n${1}`('a',({n})=>{expect(n).toBe(1)}); test.each`n\n${2}`('b',({n})=>{expect(n).toBe(1);expect(n).toBe(2)});";
    assert_eq!(subsumption_claim_count(tagged, 2), 0);
    for api in ["each", "for"] {
        let same_source = source
            .replace("loadB())", "loadA())")
            .replace("test.each", &format!("test.{api}"));
        assert_eq!(subsumption_claim_count(&same_source, 3), 0);
    }
}

/// Fully resolved rows retain manual comparisons while different conditions remain separate.
#[test]
fn subsumption_bound_rows_and_conditions() {
    let source = "import {test,expect} from 'vitest'; test.each([1])('a',()=>{expect(value).toBe(1)}); test('b',()=>{expect(value).toBe(1);expect(other).toBe(2)});";
    assert_eq!(subsumption_claim_count(source, 2), 1);
    let conditional = "import {test,expect} from 'vitest'; test.skipIf(first)('a',()=>{expect(value).toBe(1)});test.skipIf(second)('b',()=>{expect(value).toBe(1);expect(other).toBe(2)});";
    assert_eq!(subsumption_claim_count(conditional, 2), 0);
}

/// The owned source stays silent, and removing only its failure markers makes every pair fire.
#[test]
fn subsumption_refuses_expected_failure_in_either_direction() {
    let source = include_str!("../../fixtures/duplicates/not-subsumed-expected-failure.test.ts");
    assert_eq!(subsumption_claim_count(source, 10), 0);
    let plain = source
        .replace("test.fails(", "test(")
        .replace(", { fails: true }", "");
    assert_eq!(subsumption_claim_count(&plain, 10), 5);
}

/// Every spelling of an expected failure reaches the refusal; a one-edit plain twin fires.
#[test]
fn subsumption_expected_failure_spellings() {
    for (large, plain) in [
        ("test.fails.each([[1]])", "test.each([[1]])"),
        ("test.concurrent.fails", "test.concurrent"),
        ("my.fails", "my"),
    ] {
        let source = format!(
            "import {{test,expect}} from 'vitest'; const my = test.extend({{}}); \
             {}('a',()=>{{expect(value).toBe(1)}}); \
             {large}('b',()=>{{expect(value).toBe(1);expect(other).toBe(2)}});",
            large.split('.').next().unwrap()
        );
        assert_eq!(subsumption_claim_count(&source, 2), 0, "{large}");
        let twin = source.replace(large, plain);
        assert_eq!(subsumption_claim_count(&twin, 2), 1, "{plain}");
    }
    for suite in [
        "describe('o',{fails:true},()=>{describe('i',()=>{BODY})})",
        "describe.each([[1]])('o',{fails:true},()=>{BODY})",
    ] {
        let body = "test('a',()=>{expect(value).toBe(1)});\
                    test('b',()=>{expect(value).toBe(1);expect(other).toBe(2)});";
        let source = format!(
            "import {{test,describe,expect}} from 'vitest'; {}",
            suite.replace("BODY", body)
        );
        assert_eq!(subsumption_claim_count(&source, 2), 0, "{suite}");
        let twin = source.replace("fails:true", "fails:false");
        assert_eq!(subsumption_claim_count(&twin, 2), 1, "{suite}");
    }
}

/// Options that keep the expected outcome leave the pair comparable.
#[test]
fn subsumption_keeps_options_that_expect_to_pass() {
    for options in ["{ fails: false }", "{ retry: 2 }"] {
        let source = format!(
            "import {{test,expect}} from 'vitest'; test('a',()=>{{expect(value).toBe(1)}}); \
             test('b',{options},()=>{{expect(value).toBe(1);expect(other).toBe(2)}});"
        );
        assert_eq!(subsumption_claim_count(&source, 2), 1, "{options}");
    }
}

/// Computed callees and awaited assertions are not additional placeholder admissions.
#[test]
fn subsumption_placeholder_static_boundary() {
    for body in [
        "() => handlers[pending]()",
        "async () => await expect(true).toBe(true)",
    ] {
        let source = format!("import {{test,expect}} from 'vitest'; test('a', {body});");
        let corpus = corpora(&[("a.test.ts", &source)], &["a.test.ts"]).remove(0);
        assert_eq!(corpus.tests.len(), 1);
        assert!(corpus.tests[0].subsumption_key.is_some(), "{body}");
    }
}

/// Enables the opt-in rule consistently for owned-source outcome checks.
fn subsumption_config() -> crate::config::Config {
    crate::config::Config {
        rules: [(
            crate::model::Rule::SubsumedTest,
            crate::model::Severity::Warning,
        )]
        .into(),
        root: ".".into(),
        include: Vec::new(),
        include_is_explicit: false,
        exclude: Vec::new(),
        threshold: 100,
        source: "test".into(),
    }
}

mod memory;

/// Panic recovery clears cached answers while preserving committed authority and charging it once.
#[test]
fn historical_source_authority_survives_cache_recovery() {
    use crate::lang::{SourceTree, Sources};
    use crate::memory::HeapBytes;
    let dir =
        crate::modes::tests::repository_with(&[("helper.ts", b"export function run(){return 1}")]);
    let root = dir.path().canonicalize().unwrap();
    let view = std::sync::Arc::new(crate::modes::RevisionFiles::load(&root, "HEAD", None).unwrap());
    let mut engine = Engine::with_sources(Sources::committed(view.clone()));
    assert_eq!(engine.memory_fields().modules, view.retained_bytes());
    let setting = Setting {
        dialect: &DIALECT,
        globals: false,
        root: &root,
        benchmarks: false,
    };
    let source="import {test,expect} from 'vitest'; import {run} from './helper'; test('a',()=>expect(run()).toBe(1));";
    let path = root.join("run.test.ts");
    engine.read(
        &path,
        "run.test.ts".as_ref(),
        source,
        setting,
        false,
        Vec::new(),
    );
    assert_eq!(engine.tests, 1);
    let first = engine.files[0].1[0].key;
    assert!(engine.files[0].1[0].exclusion.is_none());
    assert_eq!(
        engine.memory_fields().modules,
        engine.run.modules.heap_bytes() + view.retained_bytes()
    );
    std::fs::write(root.join("helper.ts"), "export function run(){return 2}").unwrap();
    engine.record(
        "panic.test.ts".as_ref(),
        Err(Failure::Panicked),
        false,
        Vec::new(),
        false,
        None,
    );
    engine.read(
        &path,
        "run.test.ts".as_ref(),
        source,
        setting,
        false,
        Vec::new(),
    );
    assert_eq!(engine.tests, 2);
    assert_eq!(engine.files[1].1[0].key, first);
    view.ensure_readable().unwrap();
    let mut live = Engine::default();
    live.read(
        &path,
        "run.test.ts".as_ref(),
        source,
        setting,
        false,
        Vec::new(),
    );
    assert_eq!(live.tests, 1);
    assert_ne!(live.files[0].1[0].key, first);
}

/// Empty-table diagnostics use the normalizer's recorded row result, never a syntax guess; a suite
/// table proved empty is an empty table, and only another suite declaration is a container.
#[test]
fn empty_table_counts_require_proven_zero_rows() {
    for (declaration, empty, cases, containers) in [
        ("test.each([])('empty', n => expect(n).toBe(1));", 1, 0, 0),
        ("test.for([])('empty', n => expect(n).toBe(1));", 1, 0, 0),
        ("test.each([1, 2])('row', n => expect(n).toBe(1));", 0, 2, 0),
        (
            "test.each(unknownRows)('unknown', n => expect(n).toBe(1));",
            0,
            1,
            0,
        ),
        ("describe.each([])('empty %s', () => {});", 1, 0, 0),
        ("describe.each([1])('row %s', () => {});", 0, 0, 1),
        ("describe('plain', () => {});", 0, 0, 1),
    ] {
        let source = format!("import {{describe, test, expect}} from 'vitest'; {declaration}");
        let result = corpora(&[("a.test.ts", &source)], &["a.test.ts"]).remove(0);
        assert_eq!(result.tests.len(), cases, "{declaration}");
        assert_eq!(result.empty_tables, empty, "{declaration}");
        assert_eq!(result.containers, containers, "{declaration}");
    }
}

/// Nothing declared inside a suite table proved to have no rows registers, at any depth; a table
/// with rows, or one whose rows are unread, keeps its tests (counts as Vitest 5.0.5 collects them).
#[test]
fn a_suite_table_with_no_rows_registers_nothing_inside_it() {
    let control = "test('control', () => { expect(1).toBe(1); });";
    for (body, cases, containers) in [
        ("describe.each([])('s %s', () => { test('t', () => { expect(1).toBe(1); }); });", 1, 0),
        ("describe.for([])('s %s', () => { test('t', () => { expect(1).toBe(1); }); });", 1, 0),
        (
            "describe('outer', () => { describe.each([])('inner %s', () => { \
             test('t', () => { expect(1).toBe(1); }); }); });",
            1,
            1,
        ),
        (
            "describe.each([])('outer %s', () => { describe('inner', () => { \
             test('deep', () => { expect(1).toBe(1); }); }); });",
            1,
            0,
        ),
        (
            "describe.each(unknownRows)('s %s', () => { test('t', () => { expect(1).toBe(1); }); });",
            2,
            1,
        ),
        ("describe.each([])('s %s', () => { bench('b', () => {}); });", 1, 0),
    ] {
        let source =
            format!("import {{bench, describe, test, expect}} from 'vitest'; {body}\n{control}");
        let result = corpora(&[("a.test.ts", &source)], &["a.test.ts"]).remove(0);
        assert_eq!(result.tests.len(), cases, "{body}");
        assert_eq!(result.containers, containers, "{body}");
    }
}

/// A wrapper called inside a suite table with no rows is never called, so it leaves no
/// `registered-elsewhere` placeholder; with a row, the placeholder stays.
#[test]
fn a_wrapper_inside_an_empty_suite_table_leaves_no_placeholder() {
    let runner = "import { test, expect } from 'vitest';\n\
        export function run() { test('first', () => { expect(1).toBe(1); }); }\n";
    for (rows, cases) in [("[]", 1), ("[1]", 2)] {
        let source = format!(
            "import {{describe, test, expect}} from 'vitest'; import {{ run }} from './runner';\n\
             describe.each({rows})('s %s', () => {{ run(); }});\n\
             test('control', () => {{ expect(2).toBe(2); }});"
        );
        let result = corpora(
            &[("a.test.ts", &source), ("runner.ts", runner)],
            &["a.test.ts"],
        )
        .remove(0);
        assert_eq!(result.tests.len(), cases, "{rows}");
        assert_eq!(
            result
                .tests
                .iter()
                .filter(|case| case.exclusion == Some(Exclusion::RegisteredElsewhere))
                .count(),
            cases - 1,
            "{rows}"
        );
    }
}

/// A module no reader can parse, behind a helper two files import, marks both files' reads
/// partial: the helper is read again for each, not answered from its cache.
#[test]
fn an_unparsed_module_behind_a_shared_helper_reaches_every_importer() {
    let deep = format!(
        "export const x = {}1{};",
        "(".repeat(40_000),
        ")".repeat(40_000)
    );
    let helper = "import { x } from './deep';\nexport function register() { return x; }\n";
    let caller = "import { register } from './helper';\nregister();\n";
    let result = corpora(
        &[
            ("a.test.ts", caller),
            ("b.test.ts", caller),
            ("helper.ts", helper),
            ("deep.ts", &deep),
        ],
        &["a.test.ts", "b.test.ts"],
    );
    for corpus in &result {
        assert!(
            corpus
                .context_refusals
                .contains(&Exclusion::UnreadableImport),
            "{:?}",
            corpus.context_refusals
        );
    }
}

/// A confinement refusal travels with the module that traverses it: the module's analysis is
/// not cached, so every importer re-derives the refusal and marks its own read partial instead
/// of inheriting silence from a cache hit.
#[test]
fn a_confined_module_is_re_analyzed_per_file_so_every_importer_records_the_refusal() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("tsconfig.json"),
        format!(
            "{{ \"compilerOptions\": {{ \"paths\": {{ \"~/*\": [\"../{}/*\"] }} }} }}",
            outside.path().file_name().unwrap().to_str().unwrap()
        ),
    )
    .unwrap();
    std::fs::write(
        outside.path().join("helper.ts"),
        "export const check = (v: string) => v;\n",
    )
    .unwrap();
    let module = "import { check } from '~/helper';\nexport const use = (v: string) => check(v);\n";
    let importer = "import { test, expect } from 'vitest';\nimport { use } from './m';\ntest('importer', () => { expect(use('x')).toBe('x'); });\n";
    std::fs::write(project.path().join("m.ts"), module).unwrap();
    std::fs::write(project.path().join("a.test.ts"), importer).unwrap();
    std::fs::write(project.path().join("b.test.ts"), importer).unwrap();

    let mut engine = Engine::default();
    for name in ["a.test.ts", "b.test.ts"] {
        let source = std::fs::read_to_string(project.path().join(name)).unwrap();
        let (entry, partial) = engine.read(
            &project.path().join(name),
            std::path::Path::new(name),
            &source,
            Setting {
                dialect: &DIALECT,
                globals: false,
                root: project.path(),
                benchmarks: false,
            },
            false,
            Vec::new(),
        );
        let entry = entry.expect(name);
        assert!(
            partial,
            "{name}: the confined import must mark the read partial"
        );
        assert_eq!(entry.reason, ExclusionReason::ImportOutsideRoot, "{name}");
        assert!(
            entry.detail.contains("outside the analysis root"),
            "{entry:?}"
        );
    }
}

/// Diagnostic row views reflect actual owned bodies and leave findings and populations intact.
#[test]
fn optional_record_views_preserve_rows_and_charge_their_owned_storage() {
    use crate::memory::HeapBytes;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rows.test.ts");
    let source = "import { test, expect } from 'vitest'; test.each([1, 1, 2])('row', value => expect(value).toBeGreaterThan(0));";
    let setting = Setting {
        dialect: crate::framework::current().dialect(),
        globals: false,
        root: dir.path(),
        benchmarks: false,
    };
    let mut ordinary = Engine::default();
    let mut instrumented = Engine::default();
    ordinary.read(
        &path,
        Path::new("rows.test.ts"),
        source,
        setting,
        false,
        vec![],
    );
    instrumented.read(
        &path,
        Path::new("rows.test.ts"),
        source,
        setting,
        true,
        vec![],
    );
    assert_eq!(ordinary.tests, 3);
    assert_eq!(instrumented.tests, 3);
    assert!(instrumented.heap_bytes() > ordinary.heap_bytes());
    let plain = ordinary.take_files();
    let files = instrumented.take_files();
    assert_eq!(files[0].1.len(), 3);
    let rows = &files[0].1;
    let a = rows[0].views.as_ref().unwrap();
    let b = rows[1].views.as_ref().unwrap();
    let c = rows[2].views.as_ref().unwrap();
    assert_eq!(a.body, b.body);
    assert_ne!(a.body, c.body);
    assert_eq!(a.shape, c.shape);
    assert!(a.shape.is_some());
    assert!(plain[0].1.iter().all(|case| case.views.is_none()));
    let config =
        crate::config::Config::load(dir.path(), crate::config::ConfigOverrides::default()).unwrap();
    let off =
        crate::rules::run_with_hooks(&plain, ordinary.suites(), ordinary.hooks(), &config, &[]);
    let on = crate::rules::run_with_hooks(
        &files,
        instrumented.suites(),
        instrumented.hooks(),
        &config,
        &[],
    );
    assert!(on
        .findings
        .iter()
        .any(|finding| finding.rule == crate::model::Rule::DuplicateEachRow));
    assert_eq!(
        serde_json::to_value(off.findings).unwrap(),
        serde_json::to_value(on.findings).unwrap()
    );
}

/// Known idempotent hook proofs reach records; an unread callback still carries no invented view.
#[test]
fn optional_hook_records_read_proved_chains_and_withhold_unread_callbacks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hooks.test.ts");
    let setting = Setting {
        dialect: crate::framework::current().dialect(),
        globals: false,
        root: dir.path(),
        benchmarks: false,
    };
    let mut engine = Engine::default();
    let source = "import { beforeEach, describe, test, expect } from 'vitest'; let cell = 1; beforeEach(() => { cell = 0; }); describe('nested', () => { beforeEach(() => { cell = 0; }); test('reads', () => expect(cell).toBe(0)); });";
    engine.read(
        &path,
        Path::new("hooks.test.ts"),
        source,
        setting,
        true,
        vec![],
    );
    assert_eq!(engine.tests, 1);
    let hooks = engine.take_diagnostic_hooks();
    assert_eq!(hooks.len(), 2);
    assert!(hooks.iter().all(|(_, hook)| hook.effect_key.is_some()));
    assert_eq!(hooks[0].1.effect_key, hooks[1].1.effect_key);
    let mut unknown = Engine::default();
    unknown.read(
        &path,
        Path::new("hooks.test.ts"),
        "import { beforeEach, test } from 'vitest'; beforeEach(missing); test('empty', () => {});",
        setting,
        true,
        vec![],
    );
    assert_eq!(unknown.tests, 1);
    let hooks = unknown.take_diagnostic_hooks();
    assert_eq!(hooks.len(), 1);
    assert!(hooks[0].1.views.is_none());
    assert!(hooks[0].1.effect_key.is_none());
}

/// A suite prefix is read from the inventory's provenance, not the import's spelling: rows under
/// a suite imported through a re-export of the API are compared exactly as a direct import, while
/// rows under a user wrapper keep their scope withheld and no claim is manufactured.
#[test]
fn row_claims_follow_the_inventoried_provenance_not_the_import_spelling() {
    let caller = "import { describe, test, expect } from './u'
describe('s', () => {
  test.each([1, 1])('row %i', (n) => { expect(n).toBe(1) })
})
";
    let reexport = "export * from 'vitest'";
    let api_corpus =
        corpora(&[("u.ts", reexport), ("a.test.ts", caller)], &["a.test.ts"]).remove(0);
    assert_eq!(api_corpus.tests.len(), 2);
    assert!(api_corpus.tests.iter().all(|case| case.exclusion.is_none()));
    let outcome = owned_findings("a.test.ts", api_corpus);
    assert_eq!(outcome.findings.len(), 1, "{outcome:?}");
    assert_eq!(
        outcome.findings[0].rule,
        crate::model::Rule::DuplicateEachRow
    );

    let wrapper = "import { describe as d } from 'vitest'
export const describe = (name: string, fn: () => void) => d(name, fn)
export * from 'vitest'
";
    let wrapper_corpus =
        corpora(&[("u.ts", wrapper), ("a.test.ts", caller)], &["a.test.ts"]).remove(0);
    assert_eq!(wrapper_corpus.tests.len(), 3);
    assert!(wrapper_corpus
        .tests
        .iter()
        .all(|case| case.exclusion.is_some()));
    assert!(owned_findings("a.test.ts", wrapper_corpus)
        .findings
        .is_empty());

    // The whole-suite claim reads the same provenance.
    let copies = "import { describe, test, expect } from './u'
describe('same', () => { test('child', () => { expect(1).toBe(1) }) })
describe('same', () => { test('child', () => { expect(1).toBe(1) }) })
";
    let suites = |helper: &str, tests: usize| {
        let corpus = corpora(&[("u.ts", helper), ("a.test.ts", copies)], &["a.test.ts"]).remove(0);
        assert_eq!(corpus.tests.len(), tests);
        owned_findings("a.test.ts", corpus)
            .findings
            .iter()
            .filter(|finding| finding.rule == crate::model::Rule::DuplicateSuite)
            .count()
    };
    assert_eq!(suites(reexport, 2), 1);
    assert_eq!(suites(wrapper, 4), 0);
}

/// An object row's fields are read one property at a time: an unreadable value leaves only its
/// own field unknown, the last of a duplicated key decides, and a spread refuses every field. A
/// whole-row read that spent its budget leaves the per-property read its own.
#[test]
fn row_titles_read_object_fields_one_property_at_a_time() {
    let source = format!(
        "{HEADER}const make = () => 1;\n\
         describe('s', () => {{\n\
         test.each([{{ name: 'kept', other: make() }}])('$name', () => {{}});\n\
         test.each([{{ name: 'kept', other: make() }}])('$other', () => {{}});\n\
         test.each([{{ name: 'first', name: make() }}])('$name', () => {{}});\n\
         test.each([{{ name: make(), name: 'last' }}])('$name', () => {{}});\n\
         const base = {{ name: 'base' }};\n\
         test.each([{{ ...base, name: 'spread' }}])('$name', () => {{}});\n\
         test.each([{{ name: 'spent', data: [{literals}], other: make() }}])('$name', () => {{}});\n\
         }});\n",
        literals = vec!["0"; 252].join(", ")
    );
    let corpus = corpora(&[("a.test.ts", &source)], &["a.test.ts"]).remove(0);
    let shown: Vec<Option<String>> = corpus
        .tests
        .into_iter()
        .map(|test| test.full_title)
        .collect();
    let shown: Vec<&str> = shown
        .iter()
        .map(|title| title.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(
        shown,
        [
            "s > kept",
            "s > $other",
            "s > $name",
            "s > last",
            "s > $name",
            "s > spent"
        ]
    );
}
