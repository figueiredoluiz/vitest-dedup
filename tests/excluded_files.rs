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

/// Read one JSON report without forcing JavaScript title code units into Rust Unicode strings.
fn json_report(root: &Path, flags: &[&str]) -> String {
    let output = tempfile::tempdir().unwrap();
    let prefix = output.path().join("report");
    let _json_run = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(root)
        .args(["--reporters", "json", "--no-metrics", "--output"])
        .arg(&prefix)
        .args(flags)
        .output()
        .unwrap();
    std::fs::read_to_string(prefix.with_extension("json")).unwrap()
}

/// Runs the binary over `root` with the JSON reporter, then again with JSONL.
fn analyze(root: &Path, flags: &[&str]) -> Run {
    let json_text = json_report(root, flags);
    let jsonl_run = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(root)
        .args(["--reporters", "jsonl", "--no-metrics"])
        .args(flags)
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

/// The entry for a file that a nested runner keeps from being compared: the one test the runner
/// registers is left out as `registered-elsewhere`.
fn nested_runner(path: &'static str) -> Expect {
    Expect {
        path,
        reason: "registered-elsewhere",
        others: &[],
        excluded: 1,
        total: 1,
    }
}

/// A scenario: files to write, flags, and the expected listing.
fn scenario(files: &[(&str, &str)], flags: &[&str], expected: &[Expect]) {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    for (name, contents) in files {
        write(directory.path(), name, contents);
    }
    let run = analyze(directory.path(), flags);
    assert_listed(&run, expected);
}

const OPTS_HELPER: &str = "import { test, expect } from 'vitest';\nexport function run() { test('a', { timeout: 5 }, () => { expect(1).toBe(1); }); }\n";
const WITH: &str = "import { test } from 'vitest';\nwith ({}) { test('w', () => {}); }\n";
const DYNAMIC: &str = "import { test, expect } from 'vitest';\nconst n = Math.random();\ntest(`d ${n}`, () => { expect(1).toBe(1); });\n";

/// A runner the analyzer cannot expand leaves the one test its call registers out as
/// `registered-elsewhere`, whatever stopped the expansion: a refused option, a second missing
/// helper, recursion, effects in two nested helpers, a default export, module setup or a mock.
#[test]
fn a_runner_that_is_not_expanded_leaves_its_test_registered_elsewhere() {
    let runner = |helper: &str| format!("import {{ test, expect }} from 'vitest';\n{helper}");
    let shapes: [Vec<(&str, String)>; 6] = [
        vec![
            ("helpers/opts.ts", OPTS_HELPER.to_owned()),
            ("case.test.ts", "import { run } from './helpers/opts';\nimport { missing } from './helpers/missing';\nrun();\nmissing();\n".to_owned()),
        ],
        vec![
            ("helpers/rec.ts", runner("export function run() { run(); test('x', () => { expect(1).toBe(1); }); }\n")),
            ("case.test.ts", "import { run } from './helpers/rec';\nrun();\n".to_owned()),
        ],
        vec![
            ("helpers/a.ts", "import { test } from 'vitest';\nexport async function a() { test('a', () => {}); }\n".to_owned()),
            ("helpers/b.ts", "import { test } from 'vitest';\nexport function b(x) { x = 1; test('b', () => {}); }\n".to_owned()),
            ("helpers/wrap.ts", "import { a } from './a';\nimport { b } from './b';\nexport function run() { a(); b(0); }\n".to_owned()),
            ("case.test.ts", "import { run } from './helpers/wrap';\nrun();\n".to_owned()),
        ],
        vec![
            ("helpers/defexp.ts", runner("let run = () => { test('inside', () => { expect(1).toBe(1); }); };\nexport default run;\n")),
            ("case.test.ts", "import run from './helpers/defexp';\nrun();\n".to_owned()),
        ],
        vec![
            ("helpers/setup.ts", runner("globalThing();\nexport function run() { test('inside', () => { expect(1).toBe(1); }); }\n")),
            ("case.test.ts", "import { run } from './helpers/setup';\nrun();\n".to_owned()),
        ],
        vec![
            ("helpers/opts.ts", OPTS_HELPER.to_owned()),
            ("case.test.ts", "import { vi } from 'vitest';\nimport { run } from './helpers/opts';\nvi.mock('./helpers/opts');\nrun();\n".to_owned()),
        ],
    ];
    for files in shapes {
        let files: Vec<(&str, &str)> = files
            .iter()
            .map(|(path, text)| (*path, text.as_str()))
            .collect();
        scenario(&files, &[], &[nested_runner("case.test.ts")]);
    }
}

/// A file the analyzer refuses as a whole, or whose imported helper does not parse, is listed
/// with the refusal's own code.
#[test]
fn file_level_refusals_are_listed_with_their_own_code() {
    let big = format!(
        "import {{ test }} from 'vitest';\n// {}\ntest('b', () => {{}});\n",
        "0".repeat(8 * 1024 * 1024 + 10)
    );
    for (files, expected) in [
        (
            vec![
                ("helpers/bad.ts", "export const = ;\n"),
                ("case.test.ts", "import { test, expect } from 'vitest';\nimport { x } from './helpers/bad';\ntest('a', () => { expect(x).toBe(1); });\n"),
            ],
            Expect { path: "case.test.ts", reason: "helper-export-unreadable", others: &[], excluded: 0, total: 1 },
        ),
        (
            vec![("case.test.ts", "import { test } from 'vitest';\ntest('a', () => { let = ; });\n")],
            Expect { path: "case.test.ts", reason: "parse-error", others: &[], excluded: 0, total: 0 },
        ),
        (
            vec![("case.test.ts", big.as_str())],
            Expect { path: "case.test.ts", reason: "file-too-large", others: &[], excluded: 0, total: 0 },
        ),
    ] {
        scenario(&files, &[], &[expected]);
    }
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
    let expected = Expect {
        path: "mixed.test.ts",
        reason: "mock-factory-unreadable",
        others: &["registered-elsewhere"],
        excluded: 2,
        total: 2,
    };
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
        &[expected],
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
    let run = analyze(&root, &[]);
    if cfg!(windows) {
        // The Windows resolver cannot follow this alias from its canonical importer. The next
        // core consequently has no local registration to attach the confinement fact to; the
        // resolver's platform-specific tests pin that behavior.
        assert!(run.json["excludedFiles"].as_array().unwrap().is_empty());
        return;
    }
    let (reason, others): (&str, &[&str]) = ("import-outside-root", &[]);
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

/// The analyzer lists an undetermined `globals` too, after its own reasons for the file.
#[test]
fn the_analyzer_keeps_an_undetermined_globals_setting() {
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
    let run = analyze(directory.path(), &[]);
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
    let run = analyze(directory.path(), &[]);
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
    let run = analyze(directory.path(), &["--max-memory", "1"]);
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

/// The analyzer stops at the memory budget. Without HTML output it charges no retained bytes,
/// so the RSS backstop, below the process's startup footprint at one megabyte, stops the run.
#[test]
fn the_analyzer_stops_at_the_memory_budget() {
    let directory = tempfile::tempdir().unwrap();
    for file in 0..3 {
        write(
            directory.path(),
            &format!("f{file}.test.ts"),
            format!("import {{ test, expect }} from 'vitest';\ntest('t{file}', () => {{ expect({file}).toBe({file}); }});\n"),
        );
    }
    let run = analyze(directory.path(), &["--max-memory", "1"]);
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
    // The oversized manifest is refused and only the dependent file's tests are left out.
    {
        let result = analyze(root, &[]);
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
}

/// A refused manifest a shared helper needs reaches every file that imports the helper, and a file
/// with no test to leave out is still incomplete.
#[test]
fn a_refused_manifest_behind_a_shared_helper_reaches_every_importer() {
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
    write(root, "helper.ts", "import { value } from 'owned';\nexport const read = () => value;\nexport const unused = 1;\n");
    let pair = "import { test, expect } from 'vitest';\nimport { read } from './helper';\ntest('first', () => expect(read()).toBe(1));\ntest('second', () => expect(read()).toBe(1));\n";
    write(root, "a.test.ts", pair);
    write(root, "b.test.ts", pair);
    let result = analyze(root, &[]);
    assert_eq!(result.json["testsAnalyzed"], 5);
    assert_eq!(
        result.json["unresolvedTests"], 4,
        "{:#}",
        result.json["excludedFiles"]
    );
    assert!(result.json["findings"].as_array().unwrap().is_empty());
    let listed: Vec<_> = result.json["excludedFiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().unwrap().to_owned(),
                entry["reason"].clone(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            (
                "a.test.ts".to_owned(),
                serde_json::json!("package-manifest-unreadable")
            ),
            (
                "b.test.ts".to_owned(),
                serde_json::json!("package-manifest-unreadable")
            ),
        ]
    );
    // A file whose only import is the helper, with no test of its own, still reads in part.
    let lone = tempfile::tempdir().unwrap();
    write(lone.path(), "clean.test.ts", CLEAN);
    write(
        lone.path(),
        "node_modules/owned/index.js",
        "export const value = 1;",
    );
    write(
        lone.path(),
        "node_modules/owned/package.json",
        format!("{{\"name\":\"owned\"}}{}", " ".repeat(1024 * 1024)),
    );
    write(
        lone.path(),
        "helper.ts",
        "import { value } from 'owned';\nexport const unused = value;\n",
    );
    write(
        lone.path(),
        "only.test.ts",
        "import { unused } from './helper';\n",
    );
    let result = analyze(lone.path(), &[]);
    assert_eq!(result.json["incomplete"], true, "{:#}", result.json);
    assert!(result.json["excludedFiles"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "only.test.ts"
            && entry["reason"] == "package-manifest-unreadable"));
}

/// The analyzer's reasons for a test it leaves unresolved, one file each, beside a clean file
/// and files whose tests have nothing to compare, which are counted but not listed.
#[test]
fn the_analyzer_lists_why_each_test_is_not_compared() {
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
    // A `let` helper may be reassigned before the call, so its effects stay unproved; a
    // `function` or `const` helper called here would be expanded instead.
    write(
        directory.path(),
        "wrapped.test.ts",
        "import { test, expect } from 'vitest';
let suite = () => { test('inner', () => { expect(1).toBe(1); }); };
suite();
test('kept', () => { expect(1).toBe(1); });
",
    );
    let run = analyze(directory.path(), &[]);
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
                "registered-elsewhere",
                &["helper-effects-unproved"],
                3,
                3,
            ),
        ],
    );
    assert_eq!(run.json["testsAnalyzed"], 15);
    assert_eq!(run.json["unresolvedTests"], 7);
    assert_eq!(run.json["incompleteCauses"]["unresolvedContexts"], 7);
    // `named.test.ts` and `wrapped.test.ts` have every test unresolved; `assert_listed` ties each
    // count to its entry. `unprovable.test.ts` holds a class the trace cannot read: reported, and
    // it excludes nothing.
    assert_eq!(run.json["blindFiles"], 2);
    // The one relationship the ported rules claim: three files spell the identical `kept` test,
    // reported as the spanning set of pairs. Every excluded test above stays out of every group;
    // `wrapped.test.ts` is excluded whole, its unproved runner invocation withholding the scope,
    // so no finding reaches it.
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
}

/// A rule-local trace gap is reported without turning an otherwise complete run into an
/// incomplete one.
#[test]
fn an_incomplete_trace_alone_keeps_the_run_complete() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "trace.test.ts",
        "import { test, expect } from 'vitest';
test('partial', () => {
  class C { [key]() {} }
  new C();
});
test('complete', () => { expect(1).toBe(1); });
",
    );
    let run = analyze(directory.path(), &[]);
    assert_eq!(run.json["testsAnalyzed"], 2);
    assert_eq!(run.json["unresolvedTests"], 0);
    assert_eq!(run.json["incomplete"], false, "{:#}", run.json);
    assert_eq!(run.json["incompleteCauses"]["unresolvedContexts"], 0);
    assert_eq!(run.json["blindFiles"], 0);
    assert_eq!(run.json["excludedFiles"].as_array().unwrap().len(), 1);
    let entry = &run.json["excludedFiles"][0];
    assert_eq!(entry["path"], "trace.test.ts");
    assert_eq!(entry["reason"], "incomplete-trace");
    assert_eq!(entry["testsExcluded"], 0);
    assert_eq!(entry["testsTotal"], 2);
    assert_eq!(entry["everyTestExcluded"], false);
    let summary = run.jsonl.last().unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["testsAnalyzed"], 2);
    assert_eq!(summary["unresolvedTests"], 0);
    assert_eq!(summary["incomplete"], false);
    assert_eq!(summary["incompleteCauses"]["unresolvedContexts"], 0);
}

/// Non-hoisted mocks affect dynamic imports at the point they run, so every test in their file is
/// visibly excluded instead of sharing a context the analyzer cannot model.
#[test]
fn the_analyzer_excludes_non_hoisted_mock_files_and_fails_incomplete_gates() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    write(
        directory.path(),
        "scoped.test.ts",
        "import { test, expect, vi } from 'vitest';\n\
         test('first', async () => {\n\
           vi.doMock('./dep', () => ({ value: 1 }));\n\
           const { value } = await import('./dep');\n\
           expect(value).toBe(1);\n\
         });\n\
         test('second', () => { expect(1).toBe(1); });\n",
    );
    write(directory.path(), "dep.ts", "export const value = 0;\n");
    let run = analyze(directory.path(), &[]);
    assert_eq!(run.json["testsAnalyzed"], 3);
    assert_eq!(run.json["unresolvedTests"], 2);
    assert!(run.json["findings"].as_array().unwrap().is_empty());
    assert_listed(
        &run,
        &[Expect {
            path: "scoped.test.ts",
            reason: "unresolved-context",
            others: &[],
            excluded: 2,
            total: 2,
        }],
    );

    let mut command = assert_cmd::Command::cargo_bin("vitest-dedup").unwrap();
    command
        .arg(directory.path())
        .arg("--fail-on-incomplete=reads")
        .assert()
        .code(2)
        .stderr(predicates::str::contains("could not be fully analyzed"));
}

/// Dynamic test names are unresolved context, not a clean pair with equal callback text.
#[test]
fn the_analyzer_excludes_dynamic_titles() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    write(
        directory.path(),
        "dynamic.test.ts",
        "import { test, expect } from 'vitest';\n\
         const suffix = Math.random();\n\
         test(`first ${suffix}`, () => { expect(1).toBe(1); });\n\
         test(`second ${suffix}`, () => { expect(1).toBe(1); });\n",
    );
    let run = analyze(directory.path(), &[]);
    assert_eq!(run.json["testsAnalyzed"], 3);
    assert_eq!(run.json["unresolvedTests"], 2);
    assert!(run.json["findings"].as_array().unwrap().is_empty());
    assert_listed(
        &run,
        &[Expect {
            path: "dynamic.test.ts",
            reason: "unresolved-context",
            others: &[],
            excluded: 2,
            total: 2,
        }],
    );
}

/// Static signed fixture values and unrelated helper imports retain ordinary comparison eligibility.
#[test]
fn the_analyzer_keeps_signed_fixtures_and_unrelated_helper_imports_comparable() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "signed.test.ts",
        "import { test as base, expect } from 'vitest';\n\
         const test = base.extend({ count: -1, nested: [-2] });\n\
         test('first', () => { expect(1).toBe(1); });\n\
         test('second', () => { expect(1).toBe(1); });\n",
    );
    write(
        directory.path(),
        "fixture.ts",
        "import { test as base } from 'vitest';\n\
         export const extended = base.extend({ db: makeDb() });\n\
         export const test = base;\n",
    );
    write(
        directory.path(),
        "token.test.ts",
        "import { test, expect } from 'vitest';\n\
         import { test as helperTest } from './fixture';\n\
         void helperTest;\n\
         test('third', () => { expect(1).toBe(1); });\n\
         test('fourth', () => { expect(1).toBe(1); });\n",
    );
    write(
        directory.path(),
        "destructured.test.ts",
        "import { test, expect, vi } from 'vitest';\n\
         const { helper = { doMock() {} } } = vi;\n\
         helper.doMock('./dep');\n\
         function unused() { return; test.extend({ db: makeDb() }); }\n\
         test('fifth', () => { expect(1).toBe(1); });\n\
         test('sixth', () => { expect(1).toBe(1); });\n",
    );
    let run = analyze(directory.path(), &[]);
    assert_eq!(run.json["testsAnalyzed"], 6, "{}", run.json_text);
    assert_eq!(run.json["unresolvedTests"], 0, "{}", run.json_text);
    assert_eq!(run.json["excludedFiles"], serde_json::json!([]));
    assert_eq!(
        run.json["findings"].as_array().unwrap().len(),
        3,
        "{}",
        run.json_text
    );
}

/// Benchmarks are counted and listed as not compared, never dropped.
#[test]
fn the_analyzer_counts_benchmarks_as_not_compared() {
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
    let run = analyze(directory.path(), &[]);
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

/// Tests with nothing to compare leave the run complete by the analyzer.
#[test]
fn the_analyzer_counts_tests_with_nothing_to_compare_without_listing_them() {
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
    let run = analyze(directory.path(), &[]);
    assert_eq!(run.json["testsAnalyzed"], 3);
    assert_eq!(run.json["unresolvedTests"], 0);
    assert_eq!(run.json["incomplete"], false, "{:#}", run.json);
    assert_eq!(run.json["excludedFiles"], serde_json::json!([]));
    assert_eq!(run.json["blindFiles"], 0);
}

/// The config's severities rule the findings: a
/// rule turned off reports nothing and exits clean, an override reaches the findings that stay,
/// and an error-severity duplicate fails the quality gate.
#[test]
fn the_analyzer_honors_the_configured_severities() {
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
fn the_analyzer_marks_inert_participants() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "s.test.ts",
        "import { test, expect } from 'vitest';\ntest.skip('lazy', () => { expect(1).toBe(1); });\ntest('keen', () => { expect(1).toBe(1); });\n",
    );
    let run = analyze(directory.path(), &[]);
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

/// An inline suppression directive suppresses a finding like any other: it is a
/// claim about a file's findings, not about how its tests were read.
#[test]
fn an_inline_suppression_suppresses_a_finding() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "s.test.ts",
        "import { test, expect } from 'vitest';\n// vitest-dedup-disable-next-line the pair is intentional\ntest('one', () => { expect(1).toBe(1); });\ntest('two', () => { expect(1).toBe(1); });\n",
    );
    let run = analyze(directory.path(), &[]);
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
fn an_inline_suppression_above_a_multi_line_table_suppresses_a_finding() {
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
    let run = analyze(directory.path(), &[]);
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
    let run = analyze(directory.path(), &[]);
    assert_eq!(
        run.json["findings"],
        serde_json::json!([]),
        "{}",
        run.json_text
    );
    assert_eq!(run.json["modes"]["suppressionsApplied"], 1);
}

/// Reference baselines require a repository before any analysis or report is emitted.
#[test]
fn the_analyzer_requires_git_for_reference_baselines() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "clean.test.ts", CLEAN);
    let flags = ["--baseline-from-ref", "HEAD"];
    let output = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .arg(directory.path())
        .args(["--reporters", "json,jsonl", "--no-metrics"])
        .args(flags)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{flags:?}: {stderr}");
    assert!(
        stderr.contains("requires a Git repository"),
        "{flags:?}: {stderr}"
    );
    assert!(output.stdout.is_empty(), "{flags:?}");
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
        "helper-effects-unproved",
        "parse-error",
        "source-limit",
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
    // outside the classified set, a recovered panic); `src/corpus/tests.rs` asserts both.
    const BY_UNIT_TEST: &[&str] = &["unsupported-source", "extraction-failed"];
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
    let run = analyze(directory.path(), &[]);
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
    let run = analyze(directory.path(), &[]);
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
    let first = analyze(directory.path(), &[]);
    let second = analyze(directory.path(), &[]);
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
    let run = analyze(directory.path(), &[]);
    assert_eq!(run.json["excludedFiles"][0]["path"], "evilgnp.test.ts");
    assert!(!run.jsonl_text.contains('\u{202e}'));
    assert!(!run.stderr.contains('\u{202e}'));
}

/// How one owned fixture is excluded: what `tests/fixtures/owned-exclusions.json` pins for it.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Pinned {
    reason: String,
    other_reasons: Vec<String>,
    tests_excluded: u64,
}

/// The analyzer over the owned fixtures: floors on the work it did, and every file it excludes,
/// with its reasons and test count, pinned in `tests/fixtures/owned-exclusions.json`, so neither an
/// empty read nor an unexpected exclusion passes.
#[test]
fn the_analyzer_reads_the_owned_fixtures() {
    // The corpus includes lone-surrogate titles. Deserialize only the population this check
    // verifies: serde_json::Value cannot represent every JavaScript string in the findings.
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Excluded {
        path: String,
        #[serde(flatten)]
        pinned: Pinned,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Population {
        files_analyzed: u64,
        tests_analyzed: u64,
        unresolved_tests: u64,
        excluded_files: Vec<Excluded>,
    }
    #[derive(serde::Deserialize)]
    struct Pins {
        files: std::collections::BTreeMap<String, Pinned>,
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let run: Population = serde_json::from_str(&json_report(&root.join("fixtures"), &[])).unwrap();
    assert!(run.files_analyzed >= 580, "{}", run.files_analyzed);
    assert!(run.tests_analyzed >= 1250, "{}", run.tests_analyzed);
    assert!(run.unresolved_tests > 0);
    let mut actual = std::collections::BTreeMap::new();
    for Excluded { path, mut pinned } in run.excluded_files {
        assert!(
            !["extraction-failed", "source-limit"].contains(&pinned.reason.as_str()),
            "{path}: {}",
            pinned.reason
        );
        pinned.other_reasons.sort();
        assert!(actual.insert(path.replace('\\', "/"), pinned).is_none());
    }
    // Every unresolved test belongs to an excluded file.
    let excluded: u64 = actual.values().map(|pinned| pinned.tests_excluded).sum();
    assert_eq!(excluded, run.unresolved_tests);
    let expected: Pins = serde_json::from_str(
        &std::fs::read_to_string(root.join("tests/fixtures/owned-exclusions.json")).unwrap(),
    )
    .unwrap();
    let entry = |pinned: &Pinned| serde_json::to_string(pinned).unwrap();
    let mut differences = Vec::new();
    let paths: std::collections::BTreeSet<_> = actual.keys().chain(expected.files.keys()).collect();
    for path in paths {
        match (actual.get(path), expected.files.get(path)) {
            (Some(now), None) => differences.push(format!("added {path:?}: {}", entry(now))),
            (None, Some(was)) => differences.push(format!("removed {path:?}: {}", entry(was))),
            (Some(now), Some(was)) if now != was => differences.push(format!(
                "changed {path:?}: {} (pinned {})",
                entry(now),
                entry(was)
            )),
            _ => {}
        }
    }
    assert!(
        differences.is_empty(),
        "owned-fixture exclusions differ from tests/fixtures/owned-exclusions.json:\n{}",
        differences.join("\n")
    );
}

/// A todo a name finding names is marked inert, and stays out of the quality gate's numerator as it
/// is out of its denominator; an empty test is counted in both. Over the owned recall source.
#[test]
fn name_findings_on_todos_mark_them_inert_and_leave_the_gate_count() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/recall/duplicate-test-name-empty-and-todo");
    let run = analyze(&fixture, &["--rule", "duplicate-test-name=error"]);
    assert_eq!(run.json["testsAnalyzed"], 10);
    let findings = run.json["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 5, "{}", run.json_text);
    let inert = |title: &str| {
        findings
            .iter()
            .find(|finding| finding["titles"][0] == title)
            .unwrap_or_else(|| panic!("no finding {title}"))["inert"]
            .clone()
    };
    assert_eq!(inert("saves the cart"), serde_json::json!([true, true]));
    assert_eq!(inert("clears the cart"), serde_json::json!([true, true]));
    assert_eq!(
        inert("checkout > applies a coupon"),
        serde_json::json!([true, false])
    );
    assert_eq!(inert("loads the cart"), serde_json::json!([false, false]));
    // Five todos are not eligible; the two empty tests, the empty coupon and the totals pair are.
    let gate = &run.json["qualityGate"];
    assert_eq!(gate["eligibleCases"], 5);
    assert_eq!(gate["errorCases"], 5);
}

/// The name port reaches reporters, severity gates and line suppressions over the owned source.
#[test]
fn name_findings_preserve_titles_severity_and_suppression() {
    let directory = tempfile::tempdir().unwrap();
    let source = include_str!("../fixtures/oracle/name-collisions/cases.test.ts");
    write(directory.path(), "cases.test.ts", source);
    let result = analyze(
        directory.path(),
        &[
            "--rule",
            "duplicate-test=off",
            "--rule",
            "cross-file-duplicate=off",
            "--rule",
            "parameterization-candidate=off",
        ],
    );
    assert_eq!(result.json["testsAnalyzed"], 18);
    let findings = result.json["findings"].as_array().unwrap();
    // Four compared collisions, and the empty and todo pairs, which collide by name only.
    assert_eq!(findings.len(), 6, "{}", result.json_text);
    assert!(findings.iter().all(
        |finding| finding["rule"] == "duplicate-test-name" && finding["severity"] == "warning"
    ));
    let streamed: Vec<_> = result
        .jsonl
        .iter()
        .filter(|record| record["type"] == "finding")
        .collect();
    assert_eq!(streamed.len(), 6);
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
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(expected_exit));
    }
    let suppressed = source.replace(
        "test('collision'",
        "// vitest-dedup-disable-next-line the name is intentional\ntest('collision'",
    );
    write(directory.path(), "cases.test.ts", suppressed);
    let result = analyze(
        directory.path(),
        &[
            "--rule",
            "duplicate-test=off",
            "--rule",
            "cross-file-duplicate=off",
            "--rule",
            "parameterization-candidate=off",
        ],
    );
    assert_eq!(
        result.json["findings"].as_array().unwrap().len(),
        5,
        "{}",
        result.json_text
    );
}
