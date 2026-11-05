//! Suite and module statements whose control flow runs nothing leave the context; each refusal is
//! one edit from an admitted statement.

use super::*;

/// Whether a suite holding `statement` after its test keys like a suite holding nothing.
fn leaves_context(statement: &str) -> bool {
    let body = "() => { expect(1).toBe(1) }";
    let source = format!(
        "{HEADER}const on = true\nconst o = {{ get go() {{ return true }} }}\nconst items = [1]\nlet flag = true\n\
         describe('a', () => {{ test('with', {body})\n{statement} }})\n\
         describe('b', () => {{ test('without', {body}) }})\nconst late = true\n"
    );
    let tests = by_title(&source);
    assert_eq!(tests.len(), 2, "{statement}");
    assert!(
        tests.values().all(|test| test.exclusion.is_none()),
        "{statement}"
    );
    tests["with"].key == tests["without"].key
}

/// Statements whose every `return`, `break` and `continue` is consumed inside them, over values
/// that call no code, set nothing up.
#[test]
fn control_flow_that_ends_where_it_started_leaves_the_context() {
    for statement in [
        ";",
        "0",
        "out: { break out; return }",
        "exit: { try { return } finally { break exit } }",
        "stop: { if (on) break stop }",
        "stop: { if (on) break stop; else {} }",
        "switch (true) { case true: if (on) break; break }",
        "switch (1) { case 2: break; default: }",
        "for (const v of [1]) { if (on) continue; break }",
        "for (const n of [1, , 2]) continue",
        "while (on) break",
        "a: for (const x of [1]) { b: for (const y of [1]) { continue b } for (const z of [1]) { if (on) break a } }",
        "a: b: { break a }",
        "a: while (on) { if (on) break a; break }",
    ] {
        assert!(leaves_context(statement), "{statement} should be inert");
    }
}

/// A completion that leaves the suite, a value that may run code, a loop that may not end, or a
/// declaration keeps the statement in the context. Both branches count whatever the condition
/// holds, so a `return` behind a constant condition still refuses.
#[test]
fn control_flow_that_may_act_or_exit_stays_in_the_context() {
    for statement in [
        "exit: { try { break exit } finally { return } }",
        "if (on) return",
        "stop: { if (o.go) break stop }",
        "stop: { if (flag) break stop }",
        "stop: { if (late) break stop }",
        "stop: { if (on) break stop; let x = 1 }",
        "out: { break out; var x = 1 }",
        "out: { break out; { var x = 1 } }",
        "switch (on) { case o.go: break }",
        "switch (o.go) { case true: break }",
        "for (const v of items) break",
        "for (const v of [o.go]) break",
        "for (const v of [...items]) break",
        "for (var v of [1]) break",
        "for (const v in [1]) break",
        "while (on) { if (on) break }",
        "try { } catch { }",
        "try { } finally { o.go }",
        "o.go",
        "for (const [v] of [1]) break",
        "a: { switch (on) { case false: break a } return }",
        "a: { if (on) break a; return }",
    ] {
        assert!(!leaves_context(statement), "{statement} should act");
    }
}

/// The module context reads the same predicate: an inert top-level statement keys a file like
/// one without it.
#[test]
fn an_inert_module_statement_leaves_the_module_context() {
    let test = "test('t', () => { expect(1).toBe(1) })\n";
    let module_key = |statement: &str| {
        let files = [
            (
                "a.test.ts",
                format!("{HEADER}const on = true\n{statement}\n{test}"),
            ),
            ("b.test.ts", format!("{HEADER}const on = true\n{test}")),
        ];
        let files: Vec<(&str, &str)> = files.iter().map(|(p, s)| (*p, s.as_str())).collect();
        let corpora = corpora(&files, &["a.test.ts", "b.test.ts"]);
        assert!(corpora.iter().all(|corpus| corpus.tests.len() == 1));
        corpora[0].tests[0].module_key == corpora[1].tests[0].module_key
    };
    assert!(module_key("stop: { if (on) break stop }"));
    assert!(!module_key("stop: { if (on.valueOf()) break stop }"));
    assert!(!module_key("for await (const v of [1]) break"));
}

/// Each switch case is read once: thirty nested switches with three entries each would take
/// three to the thirtieth evaluations if every entry re-read the cases after it.
#[test]
fn nested_switches_are_read_in_linear_time() {
    let mut statement = "break".to_owned();
    for _ in 0..30 {
        statement = format!("switch (0) {{ case 0: case 1: case 2: {statement} }}");
    }
    assert!(leaves_context(&statement));
}

/// A `const` declared in a `switch` case may be skipped by entering at a later case, so a read
/// there is not proven initialized; declared before the `switch`, or read in its own case, it is.
#[test]
fn a_constant_a_switch_case_may_skip_is_not_proven() {
    let suites = "describe('a', () => { test('with', () => { expect(1).toBe(1) })\nif (on) {} })\n\
                  describe('b', () => { test('without', () => { expect(1).toBe(1) }) })";
    let skipped =
        format!("{HEADER}switch (true) {{ case false: const on = true\ncase true: {suites} }}\n");
    let tests = by_title(&skipped);
    assert_eq!(tests.len(), 2);
    assert_ne!(tests["with"].key, tests["without"].key);
    let before =
        format!("{HEADER}const on = true\nswitch (true) {{ case false: case true: {suites} }}\n");
    let tests = by_title(&before);
    assert_eq!(tests.len(), 2);
    assert_eq!(tests["with"].key, tests["without"].key);
    let same_case = format!("{HEADER}switch (true) {{ case true: const on = true\n{suites} }}\n");
    let tests = by_title(&same_case);
    assert_eq!(tests.len(), 2);
    assert_eq!(tests["with"].key, tests["without"].key);
}

/// `declare const` is erased, so reading it reads whatever global holds the name, which may run a
/// getter; only a runtime `const` is proven.
#[test]
fn an_ambient_constant_is_not_proven() {
    let suites = "describe('a', () => { test('with', () => { expect(1).toBe(1) })\nstop: { if (on) break stop } })\n\
                  describe('b', () => { test('without', () => { expect(1).toBe(1) }) })";
    for (declaration, inert) in [
        ("declare const on = true", false),
        ("const on = true", true),
    ] {
        let tests = by_title(&format!("{HEADER}{declaration}\n{suites}\n"));
        assert_eq!(tests.len(), 2);
        assert_eq!(
            tests["with"].key == tests["without"].key,
            inert,
            "{declaration}"
        );
    }
}
