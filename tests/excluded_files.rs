//! Every file excluded from comparison is listable by machine and summarized by reason.
//!
//! Each scenario builds a small project at run time, runs the binary, and compares the `json`
//! report, the `jsonl` stream and the terminal histogram. A clean control file sits in every
//! project, so a reason that fired on the wrong file fails the listing.

use serde_json::Value;
use std::path::Path;
use vitest_dedup::model::ExclusionReason;

const CLEAN: &str =
    "import { test, expect } from 'vitest';\ntest('clean', () => { expect(3).toBe(3); });\n";

/// One run of the binary: the JSON document, the JSONL records and stderr.
struct Run {
    /// The analyzed directory as given and as canonicalized: neither may appear in a report.
    roots: Vec<String>,
    json: Value,
    json_text: String,
    jsonl: Vec<Value>,
    jsonl_text: String,
    stderr: String,
}

/// Writes `contents` to `name` under `root`, creating parents.
fn write(root: &Path, name: &str, contents: impl AsRef<[u8]>) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Runs the binary over `root` with the JSON reporter, then again with JSONL.
fn run(root: &Path, flags: &[&str]) -> Run {
    run_with(root, flags, &[])
}

/// [`run`] with `environment` set for both runs.
fn run_with(root: &Path, flags: &[&str], environment: &[(&str, &str)]) -> Run {
    let output = tempfile::tempdir().unwrap();
    let prefix = output.path().join("report");
    let _json_run = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(root)
        .args(["--reporters", "json", "--no-metrics", "--output"])
        .arg(&prefix)
        .args(flags)
        .envs(environment.iter().copied())
        .output()
        .unwrap();
    let json_text = std::fs::read_to_string(prefix.with_extension("json")).unwrap();
    let jsonl_run = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(root)
        .args(["--reporters", "jsonl", "--no-metrics"])
        .args(flags)
        .envs(environment.iter().copied())
        .output()
        .unwrap();
    let jsonl_text = String::from_utf8(jsonl_run.stdout).unwrap();
    let mut roots = vec![root.to_string_lossy().into_owned()];
    roots.push(
        std::fs::canonicalize(root)
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    );
    Run {
        roots,
        json: serde_json::from_str(&json_text).unwrap(),
        json_text,
        jsonl: jsonl_text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect(),
        jsonl_text,
        stderr: String::from_utf8_lossy(&jsonl_run.stderr).into_owned(),
    }
}

/// What one excluded file must look like.
struct Expect {
    path: &'static str,
    reason: &'static str,
    others: &'static [&'static str],
    excluded: u64,
    total: u64,
}

/// Asserts the JSON list, the JSONL records, the summary and the histogram all agree with
/// `expected`, and that the run is incomplete.
fn assert_listed(run: &Run, expected: &[Expect]) {
    let listed = run.json["excludedFiles"].as_array().unwrap();
    assert_eq!(
        listed.len(),
        expected.len(),
        "{:#}",
        run.json["excludedFiles"]
    );
    for (entry, want) in listed.iter().zip(expected) {
        // No entry quotes where the project lives: the report must not depend on the checkout.
        for root in &run.roots {
            // The decoded text, not its JSON spelling, where a Windows backslash is doubled.
            for field in ["path", "detail"] {
                assert!(
                    !entry[field].as_str().unwrap().contains(root.as_str()),
                    "{root} leaked into {entry}"
                );
            }
        }
        assert_eq!(entry["path"], want.path, "{entry}");
        assert_eq!(entry["reason"], want.reason, "{entry}");
        assert_eq!(
            entry["otherReasons"],
            serde_json::json!(want.others),
            "{entry}"
        );
        assert_eq!(entry["testsExcluded"], want.excluded, "{entry}");
        assert_eq!(entry["testsTotal"], want.total, "{entry}");
        assert_eq!(
            entry["everyTestExcluded"],
            want.total > 0 && want.excluded == want.total,
            "{entry}"
        );
        assert!(
            !entry["detail"].as_str().unwrap().is_empty(),
            "a reason always carries its sentence: {entry}"
        );
    }
    // JSONL carries the same entries as records, after the findings and before the summary.
    let records: Vec<&Value> = run
        .jsonl
        .iter()
        .filter(|record| record["type"] == "excluded-file")
        .collect();
    assert_eq!(records.len(), expected.len(), "{}", run.jsonl_text);
    for (record, entry) in records.iter().zip(listed) {
        let mut record = (*record).clone();
        let object = record.as_object_mut().unwrap();
        assert_eq!(object.remove("type").unwrap(), "excluded-file");
        assert_eq!(object.remove("schemaVersion").unwrap(), "0.1");
        assert!(object.remove("contentNotice").unwrap().is_string());
        assert_eq!(&record, entry);
    }
    let types: Vec<&str> = run
        .jsonl
        .iter()
        .map(|record| record["type"].as_str().unwrap())
        .collect();
    let last = types.len() - 1;
    assert_eq!(types[last], "summary", "{types:?}");
    let first_excluded = types.iter().position(|t| *t == "excluded-file").unwrap();
    assert!(
        types[first_excluded..last]
            .iter()
            .all(|t| *t == "excluded-file"),
        "excluded-file records are contiguous and precede the summary: {types:?}"
    );
    // Summary: exact totals, and the JSON and JSONL summaries agree.
    let summary = &run.json["exclusions"];
    assert_eq!(summary["files"], expected.len(), "{summary}");
    assert_eq!(
        summary["tests"],
        expected.iter().map(|e| e.excluded).sum::<u64>(),
        "{summary}"
    );
    assert_eq!(summary["omitted"], 0);
    assert_eq!(&run.jsonl[last]["exclusions"], summary);
    // A listed file always implies an incomplete run.
    assert_eq!(run.json["incomplete"], true, "{}", run.stderr);
    // The histogram names each primary reason with its file and test counts.
    for want in expected {
        let tally = summary["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["reason"] == want.reason)
            .unwrap_or_else(|| panic!("{} missing from {summary}", want.reason));
        let files = tally["files"].as_u64().unwrap();
        let tests = tally["tests"].as_u64().unwrap();
        // It excludes no test, so it has a line of its own rather than a histogram row.
        if want.reason == "incomplete-trace" {
            assert_eq!(tests, 0);
            assert!(
                run.stderr.contains(&format!(
                    "{files} file(s) hold a test with an incomplete trace"
                )),
                "{}",
                run.stderr
            );
            continue;
        }
        let line = run
            .stderr
            .lines()
            .find(|line| line.trim_start().starts_with(want.reason))
            .unwrap_or_else(|| panic!("{} not in the histogram:\n{}", want.reason, run.stderr));
        assert!(
            line.contains(&format!("{files} file(s)"))
                && line.contains(&format!("{tests} test(s)")),
            "{line}"
        );
    }
    assert!(
        run.stderr.contains("--reporters json") && run.stderr.contains("--reporters jsonl"),
        "the histogram says where the full list is:\n{}",
        run.stderr
    );
    // `blindFiles` and `everyTestExcluded` are one definition of the same fact.
    let every = listed
        .iter()
        .filter(|e| e["everyTestExcluded"] == true)
        .count();
    assert_eq!(run.json["blindFiles"], every);
}

/// A scenario: files to write, flags, and the expected listing.
fn scenario(files: &[(&str, &str)], flags: &[&str], expected: &[Expect]) {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    for (name, contents) in files {
        write(directory.path(), name, contents);
    }
    let run = run(directory.path(), flags);
    assert_listed(&run, expected);
}

const OPTS_HELPER: &str = "import { test, expect } from 'vitest';\nexport function run() { test('a', { timeout: 5 }, () => { expect(1).toBe(1); }); }\n";
const RUN_OPTS: &str = "import { run } from './helpers/opts';\nrun();\n";
const WITH: &str = "import { test } from 'vitest';\nwith ({}) { test('w', () => {}); }\n";
const DYNAMIC: &str = "import { test, expect } from 'vitest';\nconst n = Math.random();\ntest(`d ${n}`, () => { expect(1).toBe(1); });\n";
const RECURSIVE: &str = "import { test, expect } from 'vitest';\nfunction f() { f(); test('x', () => { expect(1).toBe(1); }); }\nf();\n";
const HELPER_PREFIX: &str = "import { test, expect } from 'vitest';\n";

/// One reason code reachable from the CLI: the files that produce it and what is listed.
struct Case {
    reason: &'static str,
    files: Vec<(&'static str, String)>,
    path: &'static str,
    excluded: u64,
    total: u64,
}

/// Builds one [`Case`] for the table of CLI-reachable reasons.
fn case(
    reason: &'static str,
    files: Vec<(&'static str, String)>,
    path: &'static str,
    (excluded, total): (u64, u64),
) -> Case {
    Case {
        reason,
        files,
        path,
        excluded,
        total,
    }
}

/// Owned copy of a fixture source.
fn text(source: &str) -> String {
    source.to_owned()
}

/// A runner helper at `helpers/<name>.ts` and the caller `<name>.test.ts` that runs it.
fn runner(name: &'static str, helper_body: &str, caller: &str) -> Vec<(&'static str, String)> {
    let helper: &'static str = Box::leak(format!("helpers/{name}.ts").into_boxed_str());
    let test: &'static str = Box::leak(format!("{name}.test.ts").into_boxed_str());
    vec![
        (helper, format!("{HELPER_PREFIX}{helper_body}")),
        (test, caller.to_owned()),
    ]
}

/// Each reason reachable from the CLI is listed once with its counts in every report.
#[test]
fn each_reason_reachable_from_the_cli_is_listed_with_its_counts() {
    let cases = vec![
        case(
            "unresolved-context",
            vec![("ctx.test.ts", format!("{DYNAMIC}test('ok', () => {{ expect(2).toBe(2); }});\n"))],
            "ctx.test.ts",
            (1, 2),
        ),
        case(
            "mock-factory-unreadable",
            vec![
                ("helpers/factory.ts", text("export const factory = () => ({});\n")),
                ("mockfac.test.ts", text("import { test, expect, vi } from 'vitest';\nimport { factory } from './helpers/factory';\nvi.mock('./helpers/dep', factory);\ntest('a', () => { expect(1).toBe(1); });\n")),
            ],
            "mockfac.test.ts",
            (1, 1),
        ),
        case(
            "fixture-unreadable",
            vec![
                ("helpers/db.ts", text("export const makeDb = () => 1;\n")),
                ("fixture.test.ts", text("import { test as base, expect } from 'vitest';\nimport { makeDb } from './helpers/db';\nconst test = base.extend({ db: makeDb() });\ntest('a', ({ db }) => { expect(db).toBe(1); });\n")),
            ],
            "fixture.test.ts",
            (1, 1),
        ),
        case(
            "helper-export-unreadable",
            runner("defexp", "let run = () => { test('inside', () => { expect(1).toBe(1); }); };\nexport default run;\n", "import run from './helpers/defexp';\nrun();\n"),
            "defexp.test.ts",
            (0, 0),
        ),
        case(
            "helper-setup-effects",
            runner("setup", "globalThing();\nexport function run() { test('inside', () => { expect(1).toBe(1); }); }\n", "import { run } from './helpers/setup';\nrun();\n"),
            "setup.test.ts",
            (0, 0),
        ),
        case(
            "helper-effects-unproved",
            runner("eff", "export function run(name: string) { prep(); test(name, () => { expect(1).toBe(1); }); }\n", "import { run } from './helpers/eff';\nrun('x');\n"),
            "eff.test.ts",
            (0, 0),
        ),
        case("helper-limit", vec![("recur.test.ts", text(RECURSIVE))], "recur.test.ts", (1, 1)),
        case(
            "runner-mocked",
            vec![
                ("helpers/opts.ts", text(OPTS_HELPER)),
                ("mocked.test.ts", text("import { vi } from 'vitest';\nimport { run } from './helpers/opts';\nvi.mock('./helpers/opts');\nrun();\n")),
            ],
            "mocked.test.ts",
            (0, 0),
        ),
        case(
            "unsupported-expression",
            vec![("helpers/opts.ts", text(OPTS_HELPER)), ("opts.test.ts", text(RUN_OPTS))],
            "opts.test.ts",
            (0, 0),
        ),
        // Tests already extracted, directly and from a good runner, are excluded with the file.
        case(
            "unsupported-expression",
            vec![
                ("helpers/ok.ts", format!("{HELPER_PREFIX}export function run() {{ test('from helper', () => {{ expect(1).toBe(1); }}); }}\n")),
                ("helpers/opts.ts", text(OPTS_HELPER)),
                ("mix.test.ts", text("import { test, expect } from 'vitest';\nimport { run } from './helpers/ok';\nimport { run as bad } from './helpers/opts';\ntest('direct', () => { expect(1).toBe(1); });\nrun();\nbad();\n")),
            ],
            "mix.test.ts",
            (2, 2),
        ),
        case("unsupported-statement", vec![("with.test.ts", text(WITH))], "with.test.ts", (0, 0)),
        case(
            "parse-error",
            vec![("parse.test.ts", text("import { test } from 'vitest';\ntest('a', () => { let = ; });\n"))],
            "parse.test.ts",
            (0, 0),
        ),
        case(
            "source-limit",
            vec![("nest.test.ts", format!("import {{ test }} from 'vitest';\ntest('n', () => {{ const x = {}{}; }});\n", "[".repeat(300), "]".repeat(300)))],
            "nest.test.ts",
            (0, 0),
        ),
        case(
            "walk-limit",
            vec![("many.test.ts", format!("import {{ test }} from 'vitest';\ntest.each([{}])('t %d', () => {{}});\n", vec!["1"; 10_001].join(",")))],
            "many.test.ts",
            (0, 0),
        ),
        case(
            "file-too-large",
            vec![("big.test.ts", format!("import {{ test }} from 'vitest';\n// {}\ntest('b', () => {{}});\n", "x".repeat(8 * 1024 * 1024 + 10)))],
            "big.test.ts",
            (0, 0),
        ),
    ];
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "clean.test.ts", CLEAN);
        for (name, contents) in &case.files {
            write(directory.path(), name, contents);
        }
        let run = run(directory.path(), &[]);
        assert_listed(
            &run,
            &[Expect {
                path: case.path,
                reason: case.reason,
                others: &[],
                excluded: case.excluded,
                total: case.total,
            }],
        );
    }
}

/// A second distinct helper refusal survives the one-diagnostic cap as an other reason.
#[test]
fn a_second_distinct_helper_refusal_is_kept_beside_the_first() {
    scenario(
        &[
            ("helpers/opts.ts", OPTS_HELPER),
            (
                "two.test.ts",
                "import { run } from './helpers/opts';\nimport { missing } from './helpers/missing';\nrun();\nmissing();\n",
            ),
        ],
        &[],
        &[Expect {
            path: "two.test.ts",
            reason: "unsupported-expression",
            others: &["helper-export-unreadable"],
            excluded: 0,
            total: 0,
        }],
    );
}

/// A runner refused inside another runner keeps the inner refusal code.
#[test]
fn a_nested_runner_refusal_keeps_its_own_code() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    write(directory.path(), "helpers/rec.ts", "import { test, expect } from 'vitest';\nexport function run() { run(); test('x', () => { expect(1).toBe(1); }); }\n");
    write(
        directory.path(),
        "rec.test.ts",
        "import { run } from './helpers/rec';\nrun();\n",
    );
    let run = run(directory.path(), &[]);
    assert_listed(
        &run,
        &[Expect {
            path: "rec.test.ts",
            reason: "helper-limit",
            others: &[],
            excluded: 0,
            total: 0,
        }],
    );
    // The outer sentence carries the nested cause, not only the fact that extraction failed.
    let detail = run.json["excludedFiles"][0]["detail"].as_str().unwrap();
    assert!(
        detail.contains("could not be fully extracted:") && detail.contains("calls itself"),
        "{detail}"
    );
}

/// A wrapper runner that calls two refused helpers keeps both reasons: the first leads, the other
/// is listed beside it.
#[test]
fn a_nested_runner_keeps_every_distinct_refusal_it_hit() {
    scenario(
        &[
            ("helpers/a.ts", "import { test } from 'vitest';\nexport async function a() { test('a', () => {}); }\n"),
            ("helpers/b.ts", "import { test } from 'vitest';\nexport function b(x) { x = 1; test('b', () => {}); }\n"),
            ("helpers/wrap.ts", "import { a } from './a';\nimport { b } from './b';\nexport function run() { a(); b(0); }\n"),
            ("wrap.test.ts", "import { run } from './helpers/wrap';\nrun();\n"),
        ],
        &[],
        &[Expect {
            path: "wrap.test.ts",
            reason: "unsupported-expression",
            others: &["helper-effects-unproved"],
            excluded: 0,
            total: 0,
        }],
    );
}

/// A test can have an unreadable mock factory and an unreadable fixture at once: both are listed.
#[test]
fn independent_context_reasons_are_both_listed() {
    scenario(
        &[
            ("helpers/factory.ts", "export const factory = () => ({});\nexport const makeDb = () => 1;\n"),
            (
                "both.test.ts",
                "import { test as base, expect, vi } from 'vitest';\nimport { factory, makeDb } from './helpers/factory';\nvi.mock('./helpers/dep', factory);\nconst test = base.extend({ db: makeDb() });\ntest('a', ({ db }) => { expect(db).toBe(1); });\n",
            ),
        ],
        &[],
        &[Expect { path: "both.test.ts", reason: "mock-factory-unreadable", others: &["fixture-unreadable"], excluded: 1, total: 1 }],
    );
}

/// A helper refusal explains the generic fallback, but not a test's own unreadable mock factory.
#[test]
fn an_unreadable_mock_factory_is_listed_beside_a_helper_refusal() {
    scenario(
        &[
            ("helpers/factory.ts", "export const factory = () => ({});\n"),
            ("helpers/rec.ts", "import { test, expect } from 'vitest';\nexport function run() { run(); test('x', () => { expect(1).toBe(1); }); }\n"),
            (
                "mixed.test.ts",
                "import { test, expect, vi } from 'vitest';\nimport { factory } from './helpers/factory';\nimport { run } from './helpers/rec';\nvi.mock('./helpers/dep', factory);\nrun();\ntest('a', () => { expect(1).toBe(1); });\n",
            ),
        ],
        &[],
        &[Expect { path: "mixed.test.ts", reason: "runner-mocked", others: &["mock-factory-unreadable"], excluded: 1, total: 1 }],
    );
}

/// A skip that stands on unreadable project configuration is listed, unlike a deliberate one.
#[test]
fn a_file_skipped_because_project_config_is_unreadable_is_listed() {
    scenario(
        &[
            (".vitest-dedup.json", r#"{"include":["src/**/*.ts"]}"#),
            (
                "vitest.config.ts",
                "export default { test: { globals: false, include: chooseIncludes() } };\n",
            ),
            (
                "src/a.ts",
                "import { test } from 'vitest';\ntest('a', () => {});\n",
            ),
        ],
        &[],
        &[Expect {
            path: "src/a.ts",
            reason: "project-config-unreadable",
            others: &[],
            excluded: 0,
            total: 0,
        }],
    );
}

/// An import that left the root is the primary reason, ahead of the helper refusal it causes.
#[test]
fn an_import_leaving_the_root_leads_the_helper_refusal_it_causes() {
    let guard = tempfile::tempdir().unwrap();
    let root = guard.path().join("root");
    write(&root, "clean.test.ts", CLEAN);
    write(
        &root,
        "tsconfig.json",
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@out/*":["../outside/*"]}}}"#,
    );
    write(
        guard.path(),
        "outside/target.ts",
        "import { test, expect } from 'vitest';\nexport function run() { test('a', () => { expect(1).toBe(1); }); }\n",
    );
    write(
        &root,
        "outside.test.ts",
        "import { run } from '@out/target';\nrun();\n",
    );
    let run = run(&root, &[]);
    // On Windows the resolver does not apply a tsconfig `paths` alias from a canonical (`\\?\`)
    // importer at all, so the alias never resolves to a file outside the root and only the helper
    // refusal is recorded; see the matching branch in the resolver's own tests.
    let (reason, others): (&str, &[&str]) = if cfg!(windows) {
        ("helper-export-unreadable", &[])
    } else {
        ("import-outside-root", &["helper-export-unreadable"])
    };
    assert_listed(
        &run,
        &[Expect {
            path: "outside.test.ts",
            reason,
            others,
            excluded: 0,
            total: 0,
        }],
    );
}

/// A project whose `globals` cannot be read lists its files as globals-undetermined.
#[test]
fn an_undetermined_globals_setting_is_globals_undetermined() {
    scenario(
        &[
            (
                "vitest.config.ts",
                "export default { test: { projects: [ { test: { name: 'a', include: ['a/**/*.test.ts'], globals: true } }, { test: { name: 'b', include: ['b/**/*.test.ts'], globals: computeIt() } } ] } };\n",
            ),
            ("b/g.test.ts", CLEAN),
        ],
        &[],
        &[
            // The clean control at the root is governed by no project here, so it is skipped by
            // classification, not excluded; only the file in the undetermined project is listed.
            Expect { path: "b/g.test.ts", reason: "globals-undetermined", others: &[], excluded: 0, total: 1 },
        ],
    );
}

/// The next engine lists an undetermined `globals` too, after its own reasons for the file.
#[test]
fn the_next_engine_keeps_an_undetermined_globals_setting() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vitest.config.ts",
        "export default { test: { globals: computeIt() } };\n",
    );
    write(directory.path(), "g.test.ts", CLEAN);
    write(
        directory.path(),
        "o.test.ts",
        "import { test, expect } from 'vitest';
let count = 0;
test('writes', () => { count = 1; expect(count).toBe(1); });
test('reads', () => { expect(count).toBe(0); });
",
    );
    let run = run_with(directory.path(), &[], &[("VITEST_DEDUP_ENGINE", "next")]);
    assert_listed(
        &run,
        &[
            Expect {
                path: "g.test.ts",
                reason: "globals-undetermined",
                others: &[],
                excluded: 0,
                total: 1,
            },
            Expect {
                path: "o.test.ts",
                reason: "order-dependent-state",
                others: &["globals-undetermined"],
                excluded: 1,
                total: 2,
            },
        ],
    );
}

/// A file that is not UTF-8 is listed as unreadable.
#[test]
fn a_file_that_is_not_utf8_is_unreadable() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    write(
        directory.path(),
        "binary.test.ts",
        b"\xff\xfe\x00bad".as_slice(),
    );
    let run = run(directory.path(), &[]);
    assert_listed(
        &run,
        &[Expect {
            path: "binary.test.ts",
            reason: "unreadable",
            others: &[],
            excluded: 0,
            total: 0,
        }],
    );
}

/// Files left unread by the memory budget are listed in discovery order.
#[test]
fn files_left_unanalyzed_by_the_memory_budget_are_memory_budget() {
    let directory = tempfile::tempdir().unwrap();
    for file in 0..10 {
        let tests: String = (0..400)
            .map(|test| {
                format!(
                    "test('case {file} {test}', () => {{ expect(add({}, 1)).toBe({}); }});\n",
                    test % 10,
                    test % 10 + 1
                )
            })
            .collect();
        write(
            directory.path(),
            &format!("f{file}.test.ts"),
            format!(
                "import {{ test, expect }} from 'vitest';\nimport {{ add }} from './add';\n{tests}"
            ),
        );
    }
    let run = run(directory.path(), &["--max-memory", "1"]);
    let skipped = run.json["incompleteCauses"]["filesOverMemoryBudget"]
        .as_u64()
        .unwrap();
    assert!(skipped > 0 && skipped < 10, "{skipped}");
    let listed = run.json["excludedFiles"].as_array().unwrap();
    assert_eq!(listed.len() as u64, skipped);
    for (offset, entry) in listed.iter().enumerate() {
        // The unread tail, in the order discovery read the files.
        assert_eq!(
            entry["path"],
            format!("f{}.test.ts", 10 - skipped as usize + offset)
        );
        assert_eq!(entry["reason"], "memory-budget");
        assert_eq!(entry["testsTotal"], 0);
    }
    assert_eq!(run.json["exclusions"]["reasons"][0]["files"], skipped);
}

/// The next engine stops at the memory budget. Without HTML output it charges no retained bytes,
/// so the RSS backstop, below the process's startup footprint at one megabyte, stops the run.
#[test]
fn the_next_engine_stops_at_the_memory_budget() {
    let directory = tempfile::tempdir().unwrap();
    for file in 0..3 {
        write(
            directory.path(),
            &format!("f{file}.test.ts"),
            format!("import {{ test, expect }} from 'vitest';\ntest('t{file}', () => {{ expect({file}).toBe({file}); }});\n"),
        );
    }
    let run = run_with(
        directory.path(),
        &["--max-memory", "1"],
        &[("VITEST_DEDUP_ENGINE", "next")],
    );
    assert_eq!(run.json["incompleteCauses"]["filesOverMemoryBudget"], 2);
    let reasons: Vec<_> = run.json["excludedFiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["reason"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(reasons, ["memory-budget", "memory-budget"]);
}

/// A required manifest refusal has one stable code across JSON, JSONL and the terminal histogram.
#[test]
fn refused_package_metadata_is_reported_without_tainting_independent_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "clean.test.ts", CLEAN);
    write(
        root,
        "node_modules/owned/index.js",
        "export const value = 1;",
    );
    write(
        root,
        "node_modules/owned/package.json",
        format!("{{\"name\":\"owned\"}}{}", " ".repeat(1024 * 1024)),
    );
    write(root, "dependent.test.ts", "import { test, expect } from 'vitest';\nimport { value } from 'owned';\ntest('first', () => expect(value).toBe(1));\ntest('second', () => expect(value).toBe(1));\n");
    let result = run(root, &[]);
    assert_eq!(result.json["testsAnalyzed"], 3);
    assert_eq!(result.json["unresolvedTests"], 2);
    assert!(result.json["findings"].as_array().unwrap().is_empty());
    assert_listed(
        &result,
        &[Expect {
            path: "dependent.test.ts",
            reason: "package-manifest-unreadable",
            others: &[],
            excluded: 2,
            total: 2,
        }],
    );
}

/// The next engine's reasons for a test it leaves unresolved, one file each, beside a clean file
/// and files whose tests have nothing to compare, which are counted but not listed.
#[test]
fn the_next_engine_lists_why_each_test_is_not_compared() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    write(
        directory.path(),
        "empty.test.ts",
        "import { test, expect } from 'vitest';
test('empty', () => {});
test('kept', () => { expect(1).toBe(1); });
",
    );
    write(
        directory.path(),
        "missing.test.ts",
        "import { test } from 'vitest';
test('no callback');
",
    );
    write(
        directory.path(),
        "inside.test.ts",
        "import { test, expect } from 'vitest';
export function later() { test('inside', () => { expect(1).toBe(1); }); }
test('kept', () => { expect(1).toBe(1); });
",
    );
    write(
        directory.path(),
        "named.test.ts",
        "import { test, expect } from 'vitest';
function check() { expect(1).toBe(1); }
test('named', check);
",
    );
    write(
        directory.path(),
        "order.test.ts",
        "import { test, expect } from 'vitest';
let count = 0;
test('writes', () => { count = 1; expect(count).toBe(1); });
test('reads', () => { expect(count).toBe(0); });
",
    );
    write(
        directory.path(),
        "unprovable.test.ts",
        "import { test } from 'vitest';
test('unprovable', () => {
  class C { [key]() {} }
  new C();
});
",
    );
    write(
        directory.path(),
        "spread.test.ts",
        "import { test, expect } from 'vitest';
test('spread', ...[() => { expect(1).toBe(1); }]);
test('kept', () => { expect(1).toBe(1); });
",
    );
    write(
        directory.path(),
        "wrapped.test.ts",
        "import { test, expect } from 'vitest';
function suite() { test('inner', () => { expect(1).toBe(1); }); }
suite();
test('kept', () => { expect(1).toBe(1); });
",
    );
    let run = run_with(directory.path(), &[], &[("VITEST_DEDUP_ENGINE", "next")]);
    let unresolved = |path, reason, others, excluded, total| Expect {
        path,
        reason,
        others,
        excluded,
        total,
    };
    assert_listed(
        &run,
        &[
            unresolved("inside.test.ts", "registered-inside-function", &[], 1, 2),
            unresolved("named.test.ts", "named-callback", &[], 1, 1),
            unresolved("order.test.ts", "order-dependent-state", &[], 1, 2),
            unresolved("spread.test.ts", "unreadable-callback", &[], 1, 2),
            unresolved("unprovable.test.ts", "incomplete-trace", &[], 0, 1),
            unresolved(
                "wrapped.test.ts",
                "registered-inside-function",
                &["registered-elsewhere"],
                2,
                3,
            ),
        ],
    );
    assert_eq!(run.json["testsAnalyzed"], 15);
    assert_eq!(run.json["unresolvedTests"], 6);
    assert_eq!(run.json["incompleteCauses"]["unresolvedContexts"], 6);
    // `named.test.ts` has every test unresolved; `assert_listed` ties each count to its entry.
    // `unprovable.test.ts` holds a class the trace cannot read: reported, and it excludes nothing.
    assert_eq!(run.json["blindFiles"], 1);
    // The one relationship the ported rules claim: three files spell the identical `kept` test,
    // reported as the spanning set of pairs. Every excluded test above stays out of every group,
    // and `wrapped.test.ts`'s `kept` keys apart (its wrapper call is module context), so no
    // finding reaches it.
    let findings = run.json["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 2, "{}", run.jsonl_text);
    let spelling: Vec<(String, Vec<String>)> = findings
        .iter()
        .map(|finding| {
            assert_eq!(finding["rule"], "cross-file-duplicate");
            assert_eq!(finding["severity"], "warning");
            (
                finding["message"].as_str().unwrap().to_owned(),
                finding["locations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|location| location["path"].as_str().unwrap().to_owned())
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        spelling,
        vec![
            (
                "2 tests across 2 files have identical bodies under an equivalent context"
                    .to_owned(),
                vec!["empty.test.ts".to_owned(), "inside.test.ts".to_owned()]
            ),
            (
                "2 tests across 2 files have identical bodies under an equivalent context"
                    .to_owned(),
                vec!["inside.test.ts".to_owned(), "spread.test.ts".to_owned()]
            ),
        ]
    );
    assert!(
        run.stderr
            .contains("near-duplicate-test, subsumed-test, redundant-hook"),
        "{}",
        run.stderr
    );
}

/// Benchmarks are counted under the next engine and listed as not compared, never dropped.
#[test]
fn the_next_engine_counts_benchmarks_as_not_compared() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "work.bench.ts",
        "import { bench } from 'vitest';
bench('sqrt', () => { Math.sqrt(5); });
bench('cbrt', () => { Math.cbrt(5); });
",
    );
    write(directory.path(), "clean.test.ts", CLEAN);
    let run = run_with(directory.path(), &[], &[("VITEST_DEDUP_ENGINE", "next")]);
    assert_listed(
        &run,
        &[Expect {
            path: "work.bench.ts",
            reason: "unsupported-benchmark",
            others: &[],
            excluded: 2,
            total: 2,
        }],
    );
    assert_eq!(run.json["testsAnalyzed"], 3);
    assert_eq!(run.json["unresolvedTests"], 2);
}

/// Tests with nothing to compare leave the run complete under the next engine.
#[test]
fn the_next_engine_counts_tests_with_nothing_to_compare_without_listing_them() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "trivial.test.ts",
        "import { test } from 'vitest';
test('empty', () => {});
test('no callback');
test('options only', { timeout: 1000 });
",
    );
    let run = run_with(directory.path(), &[], &[("VITEST_DEDUP_ENGINE", "next")]);
    assert_eq!(run.json["testsAnalyzed"], 3);
    assert_eq!(run.json["unresolvedTests"], 0);
    assert_eq!(run.json["incomplete"], false, "{:#}", run.json);
    assert_eq!(run.json["excludedFiles"], serde_json::json!([]));
    assert_eq!(run.json["blindFiles"], 0);
}

/// The config's severities rule the next engine's findings the way they rule the default's: a
/// rule turned off reports nothing and exits clean, an override reaches the findings that stay,
/// and an error-severity duplicate fails the quality gate.
#[test]
fn the_next_engine_honors_the_configured_severities() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "d.test.ts",
        "import { test, expect } from 'vitest';\ntest('one', () => { expect(1).toBe(1); });\ntest('two', () => { expect(1).toBe(1); });\n",
    );
    let run = |flags: &[&str]| {
        let output_dir = tempfile::tempdir().unwrap();
        let prefix = output_dir.path().join("report");
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(directory.path())
            .args(["--reporters", "json", "--no-metrics", "--output"])
            .arg(&prefix)
            .args(flags)
            .env("VITEST_DEDUP_ENGINE", "next")
            .output()
            .unwrap();
        let json = serde_json::from_str::<Value>(
            &std::fs::read_to_string(prefix.with_extension("json")).unwrap(),
        )
        .unwrap();
        (output.status.code(), json)
    };
    let (code, json) = run(&[]);
    assert_eq!(code, Some(1), "an error-severity duplicate fails the gate");
    assert_eq!(json["findings"][0]["rule"], "duplicate-test");
    assert_eq!(json["findings"][0]["severity"], "error");
    let (code, json) = run(&["--rule", "duplicate-test=off"]);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(json["findings"], serde_json::json!([]));
    let (code, json) = run(&["--rule", "duplicate-test=warning"]);
    assert_eq!(code, Some(0), "a warning stays under the default threshold");
    assert_eq!(json["findings"][0]["severity"], "warning");
}

/// An inert (`.skip`/`.todo`) participant is marked and named the way the default rule names it.
#[test]
fn the_next_engine_marks_inert_participants() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "s.test.ts",
        "import { test, expect } from 'vitest';\ntest.skip('lazy', () => { expect(1).toBe(1); });\ntest('keen', () => { expect(1).toBe(1); });\n",
    );
    let run = run_with(directory.path(), &[], &[("VITEST_DEDUP_ENGINE", "next")]);
    let findings = run.json["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{}", run.jsonl_text);
    assert_eq!(findings[0]["inert"], serde_json::json!([true, false]));
    assert!(
        findings[0]["message"]
            .as_str()
            .unwrap()
            .contains("inert (skip/todo) participant(s), still counted as duplicates: lazy"),
        "{}",
        findings[0]["message"]
    );
}

/// An inline suppression directive suppresses a next-engine finding like any other: it is a
/// claim about a file's findings, not about how its tests were read.
#[test]
fn an_inline_suppression_suppresses_a_next_engine_finding() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "s.test.ts",
        "import { test, expect } from 'vitest';\n// vitest-dedup-disable-next-line the pair is intentional\ntest('one', () => { expect(1).toBe(1); });\ntest('two', () => { expect(1).toBe(1); });\n",
    );
    let run = run_with(directory.path(), &[], &[("VITEST_DEDUP_ENGINE", "next")]);
    assert_eq!(
        run.json["findings"],
        serde_json::json!([]),
        "{}",
        run.json_text
    );
    assert_eq!(run.json["modes"]["suppressionsApplied"], 1);
}

/// A suppression sits above where the declaration starts, also when Vitest places the test on a
/// later line: a multi-line table's location is the call, the way the default engine reports it.
#[test]
fn an_inline_suppression_above_a_multi_line_table_suppresses_a_next_engine_finding() {
    let directory = tempfile::tempdir().unwrap();
    let table = |title: &str| {
        format!("test.each([\n  [1, 2],\n  [3, 4],\n])('{title} %i %i', (a, b) => {{ expect(a + b).toBeGreaterThan(0); }});\n")
    };
    let source = |comment: &str| {
        format!(
            "import {{ test, expect }} from 'vitest';\n{comment}{}{}",
            table("one"),
            table("two")
        )
    };
    write(directory.path(), "s.test.ts", source(""));
    let run = run_with(directory.path(), &[], &[("VITEST_DEDUP_ENGINE", "next")]);
    let findings = run.json["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{}", run.json_text);
    assert_eq!(
        findings[0]["locations"][0]["line"], 2,
        "the location is the declaration's first line: {}",
        run.json_text
    );
    write(
        directory.path(),
        "s.test.ts",
        source("// vitest-dedup-disable-next-line the tables are intentional\n"),
    );
    let run = run_with(directory.path(), &[], &[("VITEST_DEDUP_ENGINE", "next")]);
    assert_eq!(
        run.json["findings"],
        serde_json::json!([]),
        "{}",
        run.json_text
    );
    assert_eq!(run.json["modes"]["suppressionsApplied"], 1);
}

/// Modes that filter or record findings against recorded ones are refused under the next engine,
/// which carries no baseline of its match keys and measures no engine memory (`--fail-on-new`
/// needs `--baseline`); another value of the variable is the default engine.
#[test]
fn the_next_engine_refuses_modes_that_need_findings() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    let baseline = directory.path().join("baseline.json");
    for flags in [
        vec!["--baseline", baseline.to_str().unwrap()],
        vec![
            "--baseline",
            baseline.to_str().unwrap(),
            "--update-baseline",
        ],
        vec!["--baseline-from-ref", "HEAD"],
        vec!["--changed-since", "HEAD"],
        vec!["--debug-memory"],
    ] {
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(directory.path())
            .args(["--reporters", "json,jsonl", "--no-metrics"])
            .args(&flags)
            .env("VITEST_DEDUP_ENGINE", "next")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{flags:?}: {stderr}");
        assert!(
            stderr.contains("is not available under VITEST_DEDUP_ENGINE=next"),
            "{flags:?}: {stderr}"
        );
        assert!(output.stdout.is_empty(), "{flags:?}");
    }
    let default = run_with(directory.path(), &[], &[("VITEST_DEDUP_ENGINE", "current")]);
    assert!(
        !default.stderr.contains("VITEST_DEDUP_ENGINE"),
        "{}",
        default.stderr
    );
    assert_eq!(default.json["testsAnalyzed"], 1);
}

/// Every code has a scenario here or a named unit test elsewhere.
#[test]
fn every_closed_code_has_a_scenario_or_a_named_unit_test() {
    // Reachable through the CLI above; the rest are covered where they can be produced.
    const BY_SCENARIO: &[&str] = &[
        "unresolved-context",
        "mock-factory-unreadable",
        "fixture-unreadable",
        "helper-export-unreadable",
        "helper-setup-effects",
        "helper-effects-unproved",
        "helper-limit",
        "runner-mocked",
        "unsupported-expression",
        "unsupported-statement",
        "parse-error",
        "source-limit",
        "walk-limit",
        "import-outside-root",
        "package-manifest-unreadable",
        "globals-undetermined",
        "file-too-large",
        "unreadable",
        "memory-budget",
        "project-config-unreadable",
        "registered-inside-function",
        "order-dependent-state",
        "named-callback",
        "unreadable-callback",
        "unsupported-benchmark",
        "incomplete-trace",
        "registered-elsewhere",
    ];
    // `unsupported-source` and `extraction-failed` need a path the CLI cannot reach (an extension
    // outside the classified set, a recovered panic); `helper-sources-unavailable` needs a
    // revision baseline. Each is asserted in `src/frontend/extract/tests/imported.rs` or
    // `src/exclusions.rs`.
    const BY_UNIT_TEST: &[&str] = &[
        "unsupported-source",
        "extraction-failed",
        "helper-sources-unavailable",
    ];
    // A listed scenario code must still appear in an expectation of this file besides the list
    // above, so deleting its scenario fails here.
    let source = include_str!("excluded_files.rs");
    for code in BY_SCENARIO {
        assert!(
            source.matches(&format!("\"{code}\"")).count() >= 2,
            "no scenario expects {code}"
        );
    }
    let mut covered: Vec<&str> = BY_SCENARIO.iter().chain(BY_UNIT_TEST).copied().collect();
    covered.sort_unstable();
    let mut all: Vec<&str> = ExclusionReason::ALL.iter().map(|r| r.code()).collect();
    all.sort_unstable();
    assert_eq!(covered, all);
}

/// A clean project lists nothing and writes no excluded-file record.
#[test]
fn a_clean_project_lists_nothing_and_emits_no_record() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    let run = run(directory.path(), &[]);
    assert_eq!(run.json["excludedFiles"], serde_json::json!([]));
    assert_eq!(
        run.json["exclusions"],
        serde_json::json!({"files": 0, "tests": 0, "omitted": 0, "reasons": []})
    );
    assert!(run.jsonl.iter().all(|r| r["type"] != "excluded-file"));
    assert!(
        !run.stderr
            .contains("excluded from comparison in whole or in part"),
        "{}",
        run.stderr
    );
}

/// More excluded files than the terminal cap are all listed in json and jsonl.
#[test]
fn more_excluded_files_than_the_terminal_cap_are_all_listed_in_the_reports() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    for index in 0..25 {
        write(directory.path(), &format!("dyn{index:02}.test.ts"), DYNAMIC);
    }
    let run = run(directory.path(), &[]);
    let listed = run.json["excludedFiles"].as_array().unwrap();
    assert_eq!(listed.len(), 25);
    assert_eq!(
        run.jsonl
            .iter()
            .filter(|r| r["type"] == "excluded-file")
            .count(),
        25
    );
    // Sorted by path.
    let paths: Vec<&str> = listed.iter().map(|e| e["path"].as_str().unwrap()).collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted);
    // The terminal keeps its cap, says how many it left out, and counts every file in the
    // histogram.
    assert!(
        run.stderr.contains("25 file(s) had EVERY test excluded"),
        "{}",
        run.stderr
    );
    assert_eq!(
        run.stderr
            .lines()
            .filter(|l| l.starts_with("  dyn"))
            .count(),
        10,
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("... and 15 more"), "{}", run.stderr);
    let histogram = run
        .stderr
        .lines()
        .find(|l| l.trim_start().starts_with("unresolved-context"))
        .unwrap();
    assert!(
        histogram.contains("25 file(s)") && histogram.contains("25 test(s)"),
        "{histogram}"
    );
}

/// The histogram names five kinds and folds the rest into a count.
#[test]
fn the_histogram_names_the_top_kinds_then_folds_the_rest() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    write(directory.path(), "helpers/opts.ts", OPTS_HELPER);
    write(directory.path(), "opts.test.ts", RUN_OPTS);
    write(directory.path(), "with.test.ts", WITH);
    write(
        directory.path(),
        "parse.test.ts",
        "import { test } from 'vitest';\ntest('a', () => { let = ; });\n",
    );
    write(
        directory.path(),
        "binary.test.ts",
        b"\xff\xfe\x00bad".as_slice(),
    );
    write(directory.path(), "recur.test.ts", RECURSIVE);
    write(directory.path(), "ctx.test.ts", DYNAMIC);
    let run = run(directory.path(), &[]);
    assert_eq!(
        run.json["exclusions"]["reasons"].as_array().unwrap().len(),
        6
    );
    let named = run
        .stderr
        .lines()
        .filter(|l| {
            ExclusionReason::ALL.iter().any(|r| {
                l.trim_start().starts_with(r.code())
                    && l.contains("file(s)")
                    && l.contains("test(s)")
            })
        })
        .count();
    assert_eq!(named, 5, "{}", run.stderr);
    assert!(
        run.stderr.contains("... and 1 more kind(s)"),
        "{}",
        run.stderr
    );
}

/// Two runs over one project produce byte-identical reports and stderr.
#[test]
fn two_runs_are_byte_identical() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    for index in 0..8 {
        write(directory.path(), &format!("d{index}.test.ts"), DYNAMIC);
    }
    write(directory.path(), "with.test.ts", WITH);
    write(
        directory.path(),
        "binary.test.ts",
        b"\xff\xfe\x00bad".as_slice(),
    );
    let first = run(directory.path(), &[]);
    let second = run(directory.path(), &[]);
    assert_eq!(first.json_text, second.json_text);
    assert_eq!(first.jsonl_text, second.jsonl_text);
    assert_eq!(first.stderr, second.stderr);
}

/// A bidi character in a file name is stripped from every output.
#[test]
fn a_hostile_file_name_is_sanitized_in_every_report() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    write(
        directory.path(),
        "evil\u{202e}gnp.test.ts",
        b"\xff\xfe\x00bad".as_slice(),
    );
    let run = run(directory.path(), &[]);
    assert_eq!(run.json["excludedFiles"][0]["path"], "evilgnp.test.ts");
    assert!(!run.jsonl_text.contains('\u{202e}'));
    assert!(!run.stderr.contains('\u{202e}'));
}

/// The next engine over the owned fixtures: floors on the work it did, and a ceiling on what it
/// leaves unresolved, so neither an empty read nor a mass exclusion passes.
#[test]
fn the_next_engine_reads_the_owned_fixtures() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let run = run_with(&fixtures, &[], &[("VITEST_DEDUP_ENGINE", "next")]);
    let count = |field: &str| run.json[field].as_u64().unwrap();
    assert!(count("filesAnalyzed") >= 580, "{}", count("filesAnalyzed"));
    assert!(count("testsAnalyzed") >= 1250, "{}", count("testsAnalyzed"));
    let unresolved = count("unresolvedTests");
    assert!(
        unresolved > 0 && unresolved * 5 <= count("testsAnalyzed"),
        "{unresolved}"
    );
    let failed: Vec<_> = run.json["excludedFiles"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| {
            ["extraction-failed", "source-limit"].contains(&entry["reason"].as_str().unwrap())
        })
        .collect();
    assert!(failed.is_empty(), "{failed:#?}");
}

/// The name port reaches reporters, severity gates and line suppressions over the owned source.
#[test]
fn next_name_findings_preserve_titles_severity_and_suppression() {
    let directory = tempfile::tempdir().unwrap();
    let source = include_str!("../fixtures/oracle/name-collisions/cases.test.ts");
    write(directory.path(), "cases.test.ts", source);
    let environment = [("VITEST_DEDUP_ENGINE", "next")];
    let result = run_with(
        directory.path(),
        &[
            "--rule",
            "duplicate-test=off",
            "--rule",
            "cross-file-duplicate=off",
            "--rule",
            "parameterization-candidate=off",
        ],
        &environment,
    );
    assert_eq!(result.json["testsAnalyzed"], 18);
    let findings = result.json["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 4, "{}", result.json_text);
    assert!(findings.iter().all(
        |finding| finding["rule"] == "duplicate-test-name" && finding["severity"] == "warning"
    ));
    let streamed: Vec<_> = result
        .jsonl
        .iter()
        .filter(|record| record["type"] == "finding")
        .collect();
    assert_eq!(streamed.len(), 4);
    assert_eq!(
        streamed
            .iter()
            .map(|finding| &finding["titles"])
            .collect::<Vec<_>>(),
        findings
            .iter()
            .map(|finding| &finding["titles"])
            .collect::<Vec<_>>()
    );
    for (severity, expected_exit) in [("off", 0), ("warning", 0), ("error", 1)] {
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(directory.path())
            .args([
                "--rule",
                "duplicate-test=off",
                "--rule",
                "cross-file-duplicate=off",
                "--rule",
                "parameterization-candidate=off",
                "--rule",
                &format!("duplicate-test-name={severity}"),
                "--reporters",
                "jsonl",
            ])
            .envs(environment)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(expected_exit));
    }
    let suppressed = source.replace(
        "test('collision'",
        "// vitest-dedup-disable-next-line the name is intentional\ntest('collision'",
    );
    write(directory.path(), "cases.test.ts", suppressed);
    let result = run_with(
        directory.path(),
        &[
            "--rule",
            "duplicate-test=off",
            "--rule",
            "cross-file-duplicate=off",
            "--rule",
            "parameterization-candidate=off",
        ],
        &environment,
    );
    assert_eq!(
        result.json["findings"].as_array().unwrap().len(),
        3,
        "{}",
        result.json_text
    );
}
