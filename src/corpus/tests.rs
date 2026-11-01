use super::{file, Engine, Exclusion, Failure, FileCorpus, Run, Setting, TestCase};
use crate::framework::vitest::DIALECT;
use crate::model::ExclusionReason;
use std::collections::HashMap;
use std::fs;

/// The corpus of each of `tests` (paths among `files`), read in one run.
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

/// Runs an owned oracle source through the corpus and asserts counts before name findings.
fn assert_owned_names(source: &str, path: &str, tests: usize, excluded: usize, expected: &[&str]) {
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

/// The same owned source the collection oracle runs: names, rows, scope and deferred exclusions.
#[test]
fn owned_name_collisions_keep_distinct_rows_and_exclusions() {
    assert_owned_names(
        include_str!("../../fixtures/oracle/name-collisions/cases.test.ts"),
        "cases.test.ts",
        18,
        4,
        &["collision", "round 1", "round 2", "object [object Object]"],
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
             test('{name}', () => {{ expect(x() + local).toBe(1) }})\n\
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
    // Nothing but the setup file differs between directories.
    let setup = "import { test, expect } from 'vitest'\nimport './setup.js'\ntest('t', () => { expect(globalThis.mode).toBe('a') })".to_owned();
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
        ],
        &[
            "one/a.test.ts",
            "one/b.test.ts",
            "one/c.test.ts",
            "two/a.test.ts",
            "one/d.test.ts",
            "one/e.test.ts",
            "two/d.test.ts",
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
        0,
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

/// The owned runtime source pins input equality, observability and rendering as final findings.
#[test]
fn owned_duplicate_rows_require_proven_inputs_and_keep_every_group() {
    let source = include_str!("../../fixtures/oracle/duplicate-rows/cases.test.ts");
    let corpus = corpora(&[("cases.test.ts", source)], &["cases.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 75);
    assert_eq!(
        corpus
            .tests
            .iter()
            .filter(|case| case.exclusion.is_some())
            .count(),
        2
    );
    assert_eq!(
        corpus
            .tests
            .iter()
            .filter(|case| case.row.is_some())
            .count(),
        75
    );
    let proven_rows = corpus
        .tests
        .iter()
        .filter(|case| case.row_class.is_some())
        .count();
    let outcome = owned_findings("cases.test.ts", corpus);
    let rows: Vec<_> = outcome
        .findings
        .iter()
        .filter(|finding| finding.rule == crate::model::Rule::DuplicateEachRow)
        .collect();
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
        ("whole const 1", 2),
    ];
    assert_eq!(rows.len(), expected.len(), "{rows:#?}");
    assert_eq!(proven_rows, 51);
    for (finding, (title, count)) in rows.iter().zip(expected) {
        assert_eq!(finding.locations.len(), count);
        assert_eq!(finding.titles, vec![title; count]);
        assert!(finding
            .locations
            .iter()
            .all(|location| location == &finding.locations[0]));
    }
}

/// Run rules over already-counted owned cases with the built-in severities.
fn owned_findings(path: &str, corpus: FileCorpus) -> crate::rules::Outcome {
    let config = crate::config::Config {
        root: ".".into(),
        include: Vec::new(),
        include_is_explicit: false,
        exclude: Vec::new(),
        rules: Default::default(),
        threshold: 100,
        source: "test".to_owned(),
    };
    crate::rules::run_with_hooks(
        &[(path.into(), corpus.tests)],
        &[(path.into(), corpus.suites)],
        &[(path.into(), corpus.hooks)],
        &config,
        &[],
    )
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
