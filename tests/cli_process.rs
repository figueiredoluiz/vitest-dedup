//! Process-level CLI contracts use Cargo's executable path for integration tests.

mod tests {
    use vitest_dedup::cli::ExitCode;
    use vitest_dedup::discovery;

    // -- End-to-end: carrying an `includeSource` project's `globals` through to analysis -----
    //
    // Regression coverage for project-specific globals: `discovery::discover`
    // resolves in-source-testing candidates (`test.includeSource`) against a project's globs,
    // but until this fix that governing project was never carried forward, so `main.rs` resolved
    // `globals` for such a file from path globs alone (`ProjectSet::globals_for`), found no
    // governing project, and silently fell back to the repository-wide answer -- `Disabled` in a
    // `test.projects` monorepo. These tests run the actual binary end to end (not just
    // `discovery::discover`) so a regression in *either* half of the fix (discovery carrying the
    // value, or `main.rs` consuming it) is caught.

    /// An ambient `test(...)` call -- no import, no `import.meta.vitest` destructure -- inside an
    /// `if (import.meta.vitest)` guard. This is deliberately *not*
    /// `fixtures/insource/in-source-basic.ts`'s destructured form: that form binds `test`
    /// explicitly and is recognized as Vitest regardless of `globals`, so it cannot exercise the
    /// defect here. Only an ambient binding is statically indistinguishable from Mocha/Jasmine
    /// without knowing the project's `globals` setting -- which is exactly the fact that was
    /// getting lost.
    const AMBIENT_IN_SOURCE_TEST: &str = r#"
        export function add(a: number, b: number) {
            return a + b;
        }

        if (import.meta.vitest) {
            test('add works', () => {
                expect(add(1, 1)).toBe(2);
            });
        }
    "#;

    /// An ordinary, always-recognized Vitest test (explicit import, no ambient binding). Every
    /// fixture also gets this file so `run()`'s zero-extraction guard does not mark the whole
    /// run incomplete when the in-source file yields no tests. This isolates the
    /// `project_globals` behavior each fixture exercises.
    const ORDINARY_TEST: &str = r#"
        import { test, expect } from 'vitest';

        test('ordinary works', () => {
            expect(1 + 1).toBe(2);
        });
    "#;

    /// Write a Vitest config and companion ordinary/in-source test fixtures beneath root.
    fn write_project(root: &std::path::Path, config_test_body: &str) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("vitest.config.ts"),
            format!("export default defineConfig({{ test: {{ {config_test_body} }} }});"),
        )
        .unwrap();
        std::fs::write(root.join("src/thing.ts"), AMBIENT_IN_SOURCE_TEST).unwrap();
        std::fs::write(root.join("src/companion.test.ts"), ORDINARY_TEST).unwrap();
    }

    /// A copied block fails the gate its tests fail written out: the owned copy exits 1, where
    /// reporting it as one Warning suite finding once let it pass.
    #[test]
    fn a_copied_block_fails_the_gate() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("cases.test.ts"),
            include_str!("../fixtures/oracle/duplicate-suites/cases.test.ts"),
        )
        .unwrap();
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(directory.path())
            .args(["--reporters", "jsonl"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let suite: serde_json::Value = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .find(|record| record["rule"] == "duplicate-suite")
            .expect("the copy is reported as a suite");
        assert_eq!(suite["severity"], "warning");
        assert_eq!(suite["covers"]["severity"], "error");
        assert_eq!(suite["covers"]["locations"].as_array().unwrap().len(), 4);
    }

    /// Runs the analyzer over `files` and returns the exit code and stderr.
    fn run_files(files: &[(&str, &str)]) -> (Option<i32>, String) {
        let directory = tempfile::tempdir().unwrap();
        for (name, contents) in files {
            let path = directory.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(directory.path())
            .args(["--reporters", "json", "--no-metrics", "--output"])
            .arg(directory.path().join("report"))
            .output()
            .unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    const EMPTY_TABLE: &str = "import { test, expect } from 'vitest';\n\
        test.each([])('never %s', (v) => { expect(v).toBe(1); });\n";

    /// A run whose every file holds only statically empty tables says so, and does not send the
    /// reader to `--globals`.
    #[test]
    fn the_analyzer_names_statically_empty_tables_when_nothing_was_compared() {
        let (code, stderr) = run_files(&[("empty.test.ts", EMPTY_TABLE)]);
        assert_eq!(code, Some(ExitCode::OperationalError.code()), "{stderr}");
        assert!(stderr.contains("statically empty"), "{stderr}");
        assert!(stderr.contains("empty.test.ts"), "{stderr}");
        assert!(!stderr.contains("--globals"), "{stderr}");
        // A suite table proved empty is an empty table, not a suite container.
        let (_, suites) = run_files(&[(
            "empty.test.ts",
            "import { describe } from 'vitest';\ndescribe.each([])('s %s', () => {});\n",
        )]);
        assert!(suites.contains("statically empty"), "{suites}");
        assert!(!suites.contains("only suite containers"), "{suites}");
        // A test inside that table never registers, so the file is still only an empty table.
        let (code, nested) = run_files(&[(
            "empty.test.ts",
            "import { describe, test, expect } from 'vitest';\n\
             describe.each([])('s %s', () => { test('t', () => { expect(1).toBe(1); }); });\n",
        )]);
        assert_eq!(code, Some(ExitCode::OperationalError.code()), "{nested}");
        assert!(nested.contains("statically empty"), "{nested}");
    }

    /// One ordinary file with nothing extracted keeps the `globals` advice: the empty table does not
    /// explain it. The same holds when the empty table's file also failed to parse.
    #[test]
    fn the_analyzer_keeps_the_globals_advice_unless_every_file_is_proven_empty() {
        let (_, mixed) = run_files(&[
            ("empty.test.ts", EMPTY_TABLE),
            ("plain.test.ts", "import { test } from 'vitest';\nfoo();\n"),
        ]);
        assert!(
            mixed.contains("--globals") && !mixed.contains("statically empty"),
            "{mixed}"
        );
        let (_, broken) = run_files(&[(
            "bad.test.ts",
            "import { test, expect } from 'vitest';\n\
             test.each([])('x %s', (v) => { expect(v).toBe(1); });\nconst = ;\n",
        )]);
        assert!(
            broken.contains("--globals") && !broken.contains("statically empty"),
            "{broken}"
        );
        // A suite whose children are not read (an ambient `it` without `globals`) is a declaration
        // the table does not explain: the run is a suite-only one.
        let (_, ambient) = run_files(&[(
            "ambient.test.ts",
            "import { test, describe } from 'vitest';\n\
             test.each([])('never %s', () => {});\n\
             describe('d', () => { it('x', () => { expect(1).toBe(1); }); });\n",
        )]);
        assert!(
            ambient.contains("only suite containers") && !ambient.contains("statically empty"),
            "{ambient}"
        );
        // A direct `eval` is a refusal of its own, so the empty table is not all the file holds.
        let (_, refused) = run_files(&[(
            "eval.test.ts",
            "import { test, expect } from 'vitest';\n\
             test.each([])('x %s', (v) => { expect(v).toBe(1); });\neval('1');\n",
        )]);
        assert!(
            refused.contains("--globals") && !refused.contains("statically empty"),
            "{refused}"
        );
    }

    /// A file that only registers suites is a retained container, not a test that runs; a suite
    /// with a test inside gives no zero-work message at all.
    #[test]
    fn the_analyzer_tells_a_suite_only_run_from_an_empty_one() {
        let (code, stderr) = run_files(&[(
            "suite.test.ts",
            "import { describe } from 'vitest';\ndescribe('container', () => {});\n",
        )]);
        assert_eq!(code, Some(ExitCode::OperationalError.code()), "{stderr}");
        assert!(stderr.contains("only suite containers"), "{stderr}");
        let (code, stderr) = run_files(&[(
            "suite.test.ts",
            "import { describe, test, expect } from 'vitest';\n\
             describe('s', () => { test('t', () => { expect(1).toBe(1); }); });\n",
        )]);
        assert_eq!(code, Some(ExitCode::Success.code()), "{stderr}");
        assert!(!stderr.contains("extracted"), "{stderr}");
    }

    /// A file read only in part is listed under the diagnostics beside its root-relative path; a
    /// clean file is not.
    #[test]
    fn the_analyzer_lists_a_partly_read_file_among_the_diagnostics() {
        let (_, stderr) = run_files(&[
            ("src/broken.test.ts", "test(: broken {\n"),
            (
                "src/clean.test.ts",
                "import { test, expect } from 'vitest';\ntest('c', () => { expect(1).toBe(1); });\n",
            ),
        ]);
        assert!(stderr.contains("could not be fully analyzed"), "{stderr}");
        assert!(stderr.contains("src/broken.test.ts: "), "{stderr}");
        assert!(!stderr.contains("src/clean.test.ts"), "{stderr}");
    }

    /// The rule preserves its consolidation verdict through the CLI and JSONL.
    #[test]
    fn parameterization_reports_cases_to_keep() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("cases.test.ts"),
            include_str!("../fixtures/duplicates/parameterization-contract.test.ts"),
        )
        .unwrap();
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(directory.path())
            .args([
                "--reporters",
                "jsonl",
                "--rule",
                "parameterization-candidate=error",
                "--rule",
                "duplicate-test=off",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let records: Vec<serde_json::Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let summary = records.iter().find(|r| r["type"] == "summary").unwrap();
        assert_eq!(summary["testsAnalyzed"], 10);
        assert_eq!(summary["unresolvedTests"], 0);
        assert_eq!(summary["errors"], 2);
        let findings: Vec<_> = records.iter().filter(|r| r["type"] == "finding").collect();
        assert_eq!(findings.len(), 2);
        for finding in findings {
            assert_eq!(finding["rule"], "parameterization-candidate");
            assert_eq!(finding["severity"], "error");
            assert_eq!(finding["verdict"], "consolidatable");
            assert!(finding["message"]
                .as_str()
                .unwrap()
                .contains("keeps every case as its own row"));
        }
    }

    /// The `--fail-on-incomplete` re-run the unresolved-globals tests end with: the same
    /// repository, reported to a throwaway prefix, must fail with `OperationalError` -- not the
    /// duplicate-test exit and not a crash.
    fn assert_fail_on_incomplete_is_operational_error(root: &std::path::Path) {
        assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(root)
            .args(["--reporters", "json", "--output"])
            .arg(root.join("report2"))
            .arg("--fail-on-incomplete")
            .assert()
            .failure()
            .code(ExitCode::OperationalError.code());
    }

    /// Import cutoffs remain visible in reports and strict mode, even without local tests.
    #[test]
    fn import_cutoff_is_incomplete_with_or_without_local_tests() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/recall/import-depth-cut");
        for entry in std::fs::read_dir(fixture).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                std::fs::copy(entry.path(), directory.path().join(entry.file_name())).unwrap();
            }
        }
        for expected_tests in [4, 2] {
            let (_, report) = run_json_with_stderr(directory.path(), ExitCode::QualityGateFailed);
            assert_eq!(report["testsAnalyzed"], expected_tests);
            assert_eq!(report["unresolvedTests"], expected_tests - 2);
            assert_eq!(report["incomplete"], true);
            assert_eq!(report["findings"].as_array().unwrap().len(), 1);
            assert!(report.to_string().contains("helper-export-unreadable"));
            assert_fail_on_incomplete_is_operational_error(directory.path());
            std::fs::write(directory.path().join("deep.test.ts"), "import './m0';").unwrap();
        }
    }

    /// [`run_json`] for the tests that also assert on the diagnostic text: the same
    /// JSON-to-file report, returning stderr alongside the parsed report.
    fn run_json_with_stderr(
        root: &std::path::Path,
        expected: ExitCode,
    ) -> (String, serde_json::Value) {
        let prefix = root.join("report").into_os_string().into_string().unwrap();
        let (code, _, stderr) = run_cli(root, &["--reporters", "json", "--output", &prefix]);
        assert_eq!(
            code,
            Some(expected.code()),
            "the JSON report run must have the expected outcome: {stderr}"
        );
        (stderr, read_json_report(&root.join("report")))
    }

    /// Run the CLI successfully with a JSON output prefix, then read and parse the generated
    /// report.
    fn run_json(root: &std::path::Path, extra_args: &[&str]) -> serde_json::Value {
        let output_prefix = root.join("report");
        // The process outcome is part of what this helper verifies, not just the file it leaves
        // behind: a non-zero exit here would mean the report below is stale (or missing), and the
        // downstream assertions on it would either fail confusingly or -- worse -- pass against
        // leftover state from an earlier run.
        assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(root)
            .args(["--reporters", "json", "--output"])
            .arg(&output_prefix)
            .args(extra_args)
            .assert()
            .success();
        let report = std::fs::read_to_string(root.join("report.json")).unwrap_or_else(|error| {
            panic!(
                "expected {}.json to be written: {error}",
                output_prefix.display()
            )
        });
        serde_json::from_str(&report).unwrap()
    }

    /// Regression fixture for CLI reporting: in source file in a globals true project yields
    /// its ambient tests.
    #[test]
    fn in_source_file_in_a_globals_true_project_yields_its_ambient_tests() {
        let directory = tempfile::tempdir().unwrap();
        write_project(
            directory.path(),
            "globals: true, includeSource: ['src/**/*.ts']",
        );

        let report = run_json(directory.path(), &[]);

        // The ambient in-source test plus the always-recognized companion test.
        assert_eq!(
            report["testsAnalyzed"], 2,
            "the ambient in-source test must be extracted under `globals: true`: {report:#?}"
        );
        assert_eq!(report["incomplete"], false);
    }

    /// Regression fixture for CLI reporting: in source file in a globals false project yields
    /// no ambient test.
    #[test]
    fn in_source_file_in_a_globals_false_project_yields_no_ambient_test() {
        // Same file, same `includeSource`, only `globals` differs: the ambient `test(...)` call
        // is correctly *not* recognized as Vitest here, so only the companion test is counted --
        // confirming the fix carries the project's real answer through rather than defaulting
        // every in-source file to `Enabled`.
        let directory = tempfile::tempdir().unwrap();
        write_project(
            directory.path(),
            "globals: false, includeSource: ['src/**/*.ts']",
        );

        let report = run_json(directory.path(), &[]);

        assert_eq!(
            report["testsAnalyzed"], 1,
            "expected only the companion test: {report:#?}"
        );
        assert_eq!(report["incomplete"], false);
    }

    /// Regression fixture for CLI reporting: in source file whose projects globals is unknown
    /// marks the run incomplete.
    #[test]
    fn in_source_file_whose_projects_globals_is_unknown_marks_the_run_incomplete() {
        // The admitting project's own `globals` cannot be read. Falling back to the
        // repository-wide answer here would recreate the silent-omission bug this fix closes --
        // this must surface as incompleteness instead, both in the report and in the exit code
        // under `--fail-on-incomplete`. The companion test keeps `tests` non-empty, so this
        // incompleteness is provably coming from the `project_globals` handling under test, not
        // from `run()`'s generic "zero tests extracted" guard.
        let directory = tempfile::tempdir().unwrap();
        write_project(
            directory.path(),
            "globals: someHelper(), includeSource: ['src/**/*.ts']",
        );

        let report = run_json(directory.path(), &[]);
        assert_eq!(
            report["testsAnalyzed"], 1,
            "the ambient in-source test must not be analyzed under unresolved `globals`: {report:#?}"
        );
        assert_eq!(
            report["incomplete"], true,
            "an in-source file whose governing project's `globals` is unresolved must mark the \
             run incomplete rather than silently falling back: {report:#?}"
        );

        assert_fail_on_incomplete_is_operational_error(directory.path());
    }

    // -- The ordinary (non-`includeSource`) path must fail closed on -
    // -- an unreadable governing project's `globals`, the same way the `includeSource` path      -
    // -- above already does. -----------------------------------------------------------------

    /// An ambient `test(...)` call with no import and no `import.meta.vitest` guard -- an
    /// ordinary test file, not an in-source one, so this exercises the plain `else` branch in
    /// `run()` (`ProjectSet::governed_globals_for`), not the `file.project_globals` arm above.
    const AMBIENT_ORDINARY_TEST: &str = r#"
        test('ambient works', () => {
            expect(1 + 1).toBe(2);
        });
    "#;

    /// A `test.projects` monorepo with one sub-project whose `globals` is a non-literal
    /// expression this analyzer cannot read statically -- `GlobalsDetection::Unknown` -- and
    /// which governs every file under `sub/`.
    fn write_projects_config(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(
            root.join("vitest.config.ts"),
            r#"export default defineConfig({
                test: {
                    projects: [
                        {
                            test: {
                                name: 'sub',
                                include: ['sub/**/*.test.ts'],
                                globals: someHelper(),
                            },
                        },
                    ],
                },
            });"#,
        )
        .unwrap();
        std::fs::write(root.join("sub/ambient.test.ts"), AMBIENT_ORDINARY_TEST).unwrap();
        // Always recognized regardless of `globals` (explicit import), so `tests` is never empty
        // on its own -- same reasoning as `ORDINARY_TEST`'s doc comment above.
        std::fs::write(root.join("sub/companion.test.ts"), ORDINARY_TEST).unwrap();
    }

    /// Regression fixture for CLI reporting: ordinary file whose projects globals is unknown
    /// marks the run incomplete.
    #[test]
    fn ordinary_file_whose_projects_globals_is_unknown_marks_the_run_incomplete() {
        // `sub/ambient.test.ts` is governed by the `sub` project (matched via its `include`
        // glob, not `includeSource`), whose own `globals` cannot be read. Before this fix,
        // `ProjectSet::globals_for`'s `Unknown` fell through `main.rs`'s `_ => globals` catch-all
        // straight to the repository-wide answer, silently dropping the ambient test. It must
        // instead mark the run incomplete and fail under `--fail-on-incomplete`, exactly like the
        // `includeSource` sibling above.
        let directory = tempfile::tempdir().unwrap();
        write_projects_config(directory.path());

        let report = run_json(directory.path(), &[]);
        assert_eq!(
            report["testsAnalyzed"], 1,
            "the ambient test must not be analyzed under unresolved `globals`: {report:#?}"
        );
        assert_eq!(
            report["incomplete"], true,
            "a file whose governing `test.projects` entry has unresolved `globals` must mark \
             the run incomplete rather than silently falling back: {report:#?}"
        );

        assert_fail_on_incomplete_is_operational_error(directory.path());
    }

    /// Regression fixture for CLI reporting: file governed by no project still uses the
    /// repository wide fallback.
    #[test]
    fn file_governed_by_no_project_still_uses_the_repository_wide_fallback() {
        // No `vitest.config.*`/`vite.config.*` at all: `resolve_projects` reports
        // `config_found == false`, so `run()` passes `project_scoping = None` to `discovery::
        // discover` and every file's `ProjectSet::governing_project` is `NotIncluded` -- no
        // project exists to have an unreadable answer, so this is the ordinary case the
        // repository-wide fallback exists for, and must stay complete. This guards against
        // over-correcting the fix above into failing closed on `NotIncluded` too, which would
        // flip every plain, config-less repository to incomplete.
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("thing.test.ts"), ORDINARY_TEST).unwrap();

        let report = run_json(directory.path(), &[]);
        assert_eq!(report["testsAnalyzed"], 1, "{report:#?}");
        assert_eq!(
            report["incomplete"], false,
            "a file governed by no project must fall back to the repository-wide answer, not \
             be treated as a failure to determine `globals`: {report:#?}"
        );
    }

    // -- The "extracted no tests" guard's *advice* must -----
    // -- name a statically empty `.each`/`.for` table when that is the actual cause, instead of --
    // -- always blaming `test.globals` -- but the guard itself must keep firing (`incomplete`     -
    // -- stays `true`), since Vitest itself refuses such a file ("No test suite found in file").  -

    /// Regression fixture for CLI reporting: statically empty each table explains zero tests
    /// without blaming globals.
    #[test]
    fn statically_empty_each_table_explains_zero_tests_without_blaming_globals() {
        // The exact reproduction from the review: `test.each([])` statically proves zero rows,
        // so extraction correctly contributes nothing for this file -- but Vitest's own verdict on
        // it is still a failure, not a clean pass, so `incomplete` staying `true` is correct. What
        // was wrong before this fix was the *message*: it blamed `test.globals` and recommended
        // `--globals`, which cannot do anything here.
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("empty.test.ts"),
            r#"
                import { test, expect } from 'vitest';
                test.each([])('never runs %s', (value) => {
                    expect(value).toBeDefined();
                });
            "#,
        )
        .unwrap();

        let (stderr, report) = run_json_with_stderr(directory.path(), ExitCode::OperationalError);
        assert!(
            !stderr.contains("--globals"),
            "a statically empty `.each` table has nothing to do with `globals`; that advice must \
             not be shown here: {stderr}"
        );
        assert!(
            stderr.contains("No test suite found in file") && stderr.contains("empty.test.ts"),
            "the message must name the empty-table cause and the file it applies to: {stderr}"
        );

        assert_eq!(report["testsAnalyzed"], 0, "{report:#?}");
        assert_eq!(
            report["incomplete"], true,
            "matches Vitest's own verdict on this file -- this is not a clean run: {report:#?}"
        );
    }

    /// Regression fixture for CLI reporting: a wholly excluded file is named on stderr and
    /// counted in the report.
    #[test]
    fn a_wholly_excluded_file_is_named_on_stderr_and_counted_in_the_report() {
        // The deception this closes: `blind.test.ts` produces no findings, so it appears nowhere
        // in any reporter's output. Without this diagnostic, a reader cannot tell it apart from a
        // file that was compared and came back clean -- and the corpus-wide count cannot help,
        // because it says how MANY tests were excluded, never that they were all of one file.
        //
        // `vi.doMock` is not hoisted, so it taints every test in the file (extract.rs F4): both
        // tests here get an unresolved context, which makes the whole file blind.
        let directory = tempfile::tempdir().unwrap();
        //
        // A file's tests are left out when a wrapper call registers what the file does not
        // spell, which a self-calling helper does.
        let blind = {
            std::fs::write(
                directory.path().join("rec.ts"),
                "import { test, expect } from 'vitest';\n\
                 export function run() { run(); test('x', () => { expect(1).toBe(1); }); }\n",
            )
            .unwrap();
            "import { run } from './rec';\nrun();\n"
        };
        std::fs::write(directory.path().join("blind.test.ts"), blind).unwrap();
        std::fs::write(
            directory.path().join("seen.test.ts"),
            r#"
                import { test, expect } from 'vitest';
                test('c', () => { expect(3).toBe(3); });
            "#,
        )
        .unwrap();

        let (stderr, report) = run_json_with_stderr(directory.path(), ExitCode::Success);
        assert!(
            stderr.contains("blind.test.ts"),
            "a file whose every test was excluded must be NAMED, not merely counted: {stderr}"
        );
        assert!(
            !stderr.contains("seen.test.ts"),
            "a file that was actually compared must not be listed as blind: {stderr}"
        );

        assert_eq!(
            report["blindFiles"], 1,
            "the machine contract must carry the count a gate can act on: {report:#?}"
        );
    }

    /// Regression fixture for CLI reporting: ordinary zero extraction names observation and
    /// both binding sources.
    #[test]
    fn ordinary_zero_extraction_names_observation_and_both_binding_sources() {
        // Contrast with the test above: zero extraction gives no static proof of WHY no test was
        // declared. It may be a project-level globals setting, a build transform may inject the
        // import only at runtime, or a supported binding may simply be unused. The hint must
        // state the observation and preserve both remedies instead of promoting a cause.
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("ambient.test.ts"),
            r#"
                test('ambient, never recognized without globals', () => {
                    expect(1).toBe(1);
                });
            "#,
        )
        .unwrap();
        let (stderr, report) = run_json_with_stderr(directory.path(), ExitCode::OperationalError);
        assert!(
            stderr.contains("The analyzer extracted no tests"),
            "the hint must say what static analysis actually observed: {stderr}"
        );
        assert!(
            stderr.contains("--globals") && stderr.contains("unplugin-auto-import"),
            "the hint must name both supported binding sources to investigate: {stderr}"
        );
        assert!(
            !stderr.contains("The usual cause is"),
            "the hint must not claim an unobserved cause is usual: {stderr}"
        );
        assert!(
            !stderr.contains("No test suite found in file"),
            "the empty-table explanation must not appear when nothing was proven empty: {stderr}"
        );

        assert_eq!(report["testsAnalyzed"], 0, "{report:#?}");
        assert_eq!(report["incomplete"], true, "{report:#?}");
    }

    /// Regression fixture for CLI reporting: imported but empty file is an independent
    /// operational failure.
    #[test]
    fn imported_but_empty_file_is_an_independent_operational_failure() {
        // Kept independent of the ambient-globals fixture so this ordinary supported-import case
        // owns its exit-code contract.
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("imported-empty.test.ts"),
            "import { test } from 'vitest';\n",
        )
        .unwrap();

        let (stderr, report) = run_json_with_stderr(directory.path(), ExitCode::OperationalError);
        assert!(
            stderr.contains("discovered 1 test file(s) but extracted no tests"),
            "the imported-but-empty file must reach extraction, not collapse into the empty-root \
             case: {stderr}"
        );
        assert!(
            stderr.contains("The analyzer extracted no tests"),
            "the diagnostic must state the observed outcome: {stderr}"
        );
        assert_eq!(report["testsAnalyzed"], 0, "{report:#?}");
        assert_eq!(report["incomplete"], true, "{report:#?}");
    }

    /// Pins the exact substring `scripts/check/run-external-corpus.mjs` greps for.
    ///
    /// That gate has no structured surface for the resolved globals mode -- the JSONL summary
    /// carries `filesGlobalsUndetermined`, which counts files whose mode could not be determined
    /// and therefore stays at zero when detection reaches a *confidently wrong* answer. The one
    /// place the run states which mode it actually resolved is this stderr announcement, so the
    /// gate compares its presence against each project's `expectedGlobalsDetection`.
    ///
    /// Matching on prose is what `AnalysisResult::unresolved_tests`' doc comment warns against:
    /// reword the sentence and the gate silently concludes every project derives `globals: false`,
    /// which is a pass. This test is the tripwire that makes such a reword fail here instead.
    ///
    /// Both directions are asserted deliberately. Checking only the `globals: true` arm would let
    /// a detector that announced enablement unconditionally keep this green while destroying the
    /// gate's ability to tell the two modes apart -- the marker has to be a biconditional for the
    /// mode, not merely a string that appears sometimes.
    #[test]
    fn globals_true_announcement_matches_the_string_the_corpus_gate_greps_for() {
        // Keep byte-identical to `GLOBALS_ENABLED_MARKER` in
        // `scripts/check/run-external-corpus.mjs`.
        const MARKER: &str = "sets `test.globals = true`";

        // Same corpus in both projects, differing only in the config's `globals` value, so the
        // marker's presence can be attributed to detection and to nothing else.
        let run_with_config = |config: &str| {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join("vitest.config.ts"), config).unwrap();
            std::fs::write(
                directory.path().join("a.test.ts"),
                r#"
                    import { describe, it, expect } from 'vitest';
                    describe('s', () => {
                        it('a', () => { expect(1).toBe(1); });
                    });
                "#,
            )
            .unwrap();
            let (_, _, stderr) = run_cli(
                directory.path(),
                &[
                    "--reporters",
                    "json",
                    "--output",
                    directory.path().join("report").to_str().unwrap(),
                ],
            );
            stderr
        };

        let enabled = run_with_config("export default { test: { globals: true } }\n");
        assert!(
            enabled.contains(MARKER),
            "a `globals: true` config must announce itself in the exact words the corpus gate \
             matches ({MARKER:?}); rewording it breaks that gate silently: {enabled}"
        );

        let disabled = run_with_config("export default { test: { globals: false } }\n");
        assert!(
            !disabled.contains(MARKER),
            "a `globals: false` config must not print the enablement marker, or the corpus gate \
             cannot tell the two derived modes apart: {disabled}"
        );
    }

    /// A statically readable `mergeConfig` with no `test` object on either side governs no
    /// comparison dimension. It must not partition its test away from an otherwise identical
    /// peer under the analysis root.
    #[test]
    fn a_testless_merge_config_does_not_hide_a_cross_file_duplicate() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("configured")).unwrap();
        std::fs::create_dir_all(root.join("plain")).unwrap();
        std::fs::write(
            root.join("configured/base.ts"),
            "export default { plugins: [] };",
        )
        .unwrap();
        std::fs::write(
            root.join("configured/vitest.config.ts"),
            "import { mergeConfig } from 'vitest/config';\n\
             import base from './base';\n\
             export default mergeConfig(base, { resolve: {} });",
        )
        .unwrap();
        let test_body = |title: &str| {
            format!(
                "import {{ expect, test }} from 'vitest';\n\n\
                 test('{title}', () => {{\n\
                   expect(1 + 1).toBe(2);\n\
                 }});\n"
            )
        };
        std::fs::write(root.join("configured/first.test.ts"), test_body("first")).unwrap();
        std::fs::write(root.join("plain/second.test.ts"), test_body("second")).unwrap();

        let report = run_json(root, &[]);
        assert_eq!(report["testsAnalyzed"], 2, "{report:#?}");
        assert_eq!(report["incomplete"], false, "{report:#?}");
        let findings = report["findings"].as_array().unwrap();
        assert_eq!(
            findings.len(),
            1,
            "the duplicate must not be hidden: {report:#?}"
        );
        assert_eq!(findings[0]["rule"], "cross-file-duplicate");
    }

    /// `--help` and `--version` are successful output, not failures.
    ///
    /// clap models both as `Err`, so they used to reach `main`'s error handler: printed to stderr
    /// and exited 2, the code this tool reserves for "the analyzer broke". A shell, a CI step, or
    /// a release smoke test reading `--version` would all have seen a failure. Asserted on the
    /// real process, because the bug was entirely in how the error was routed to the streams.
    #[test]
    fn help_and_version_succeed_on_stdout() {
        for flag in ["--help", "--version"] {
            let output = assert_cmd::Command::cargo_bin("vitest-dedup")
                .expect("binary")
                .arg(flag)
                .output()
                .expect("run");
            assert!(
                output.status.success(),
                "{flag} exited {:?}",
                output.status.code()
            );
            assert!(!output.stdout.is_empty(), "{flag} wrote nothing to stdout");
            assert!(
                output.stderr.is_empty(),
                "{flag} wrote to stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    /// The other half of the contract: a genuine argv error must still be an operational error on
    /// stderr, so intercepting help/version did not turn every parse failure into a success.
    #[test]
    fn an_invalid_flag_value_is_still_an_operational_error_on_stderr() {
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .expect("binary")
            .args([".", "--reporters", "bogus"])
            .output()
            .expect("run");
        assert_eq!(output.status.code(), Some(2), "expected exit 2");
        assert!(output.stdout.is_empty(), "error text leaked to stdout");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("bogus"),
            "the offending value should be named"
        );
    }

    /// Regression fixture for CLI reporting: a hostile filename reaches stderr sanitized.
    #[test]
    fn a_hostile_filename_reaches_stderr_sanitized() {
        let dir = tempfile::tempdir().expect("tempdir");
        // U+202E only, deliberately: Windows rejects control characters such as ESC in a
        // filename outright (`InvalidFilename`), so including one made this test fail on
        // windows-latest for a reason unrelated to what it checks. A bidi override is a legal
        // NTFS filename and proves the same property -- that an untrusted filename reaches
        // stderr stripped. ESC handling itself is covered by `terminal_safe`'s own unit tests.
        let hostile = dir.path().join("a\u{202e}b.test.ts");
        // Invalid UTF-8 makes the file unreadable, which routes its name into a diagnostic.
        std::fs::write(&hostile, [0xff, 0xfe, 0xfd]).expect("write");

        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .expect("binary")
            .arg(dir.path())
            .args(["--reporters", "jsonl"])
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Assert the *stripped* filename is present, not merely that some "a" and "b" occur --
        // an earlier version of this test checked the latter and passed vacuously when the name
        // never reached stderr at all, which is the same silent-success shape it exists to catch.
        assert!(
            stderr.contains("ab.test.ts"),
            "the sanitized filename should appear in stderr: {stderr}"
        );
        assert!(
            !stderr.contains('\u{202e}'),
            "U+202E survived into stderr: {stderr}"
        );
    }

    // =========================================================================================
    // Baseline and changed-file modes, end to end through the real binary.
    //
    // `vitest_dedup::modes` unit-tests matching and git plumbing, and `analysis::tests` pins what a
    // baselined finding does to the gate. What neither can reach is the thing a user actually
    // depends on: the process exit code, and whether the finding is still on screen. Both are
    // asserted here against the built binary, because the contract is "exit 0 *and* you can still
    // see it", and a test that checked only one half would pass while the other half regressed.
    // =========================================================================================

    /// Two tests with the same normalized body in one file: one `duplicate-test` finding at Error
    /// severity, which fails the default quality gate.
    const DUPLICATE_PAIR: &str = r#"
        import { test, expect } from 'vitest';

        test('adds numbers', () => {
            const total = 1 + 1;
            expect(total).toBe(2);
        });

        test('adds numbers, again', () => {
            const total = 1 + 1;
            expect(total).toBe(2);
        });
    "#;

    /// A second, independent duplicate pair, for "one new finding appeared" scenarios.
    const OTHER_DUPLICATE_PAIR: &str = r#"
        import { test, expect } from 'vitest';

        test('subtracts numbers', () => {
            const total = 5 - 3;
            expect(total).toBe(2);
        });

        test('subtracts numbers, again', () => {
            const total = 5 - 3;
            expect(total).toBe(2);
        });
    "#;

    /// A file with nothing duplicated in it.
    const CLEAN_TESTS: &str = r#"
        import { test, expect } from 'vitest';

        test('multiplies', () => {
            expect(3 * 4).toBe(12);
        });

        test('divides', () => {
            expect(12 / 4).toBe(3);
        });
    "#;

    /// Runs the binary and returns `(exit code, stdout, stderr)`.
    fn run_cli(root: &std::path::Path, args: &[&str]) -> (Option<i32>, String, String) {
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .expect("binary")
            .arg(root)
            .args(args)
            .output()
            .expect("run");
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    /// Refused ignore contents remain visible as a failed read without losing readable positives.
    #[test]
    fn bounded_ignore_inputs_reach_the_cli_summary() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/recall/ignore-input-bounded");
        let (code, stdout, stderr) = run_cli(
            &root,
            &["--reporters", "jsonl", "--fail-on-incomplete=reads"],
        );
        let records: Vec<serde_json::Value> = stdout
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let summary = records
            .iter()
            .find(|record| record["type"] == "summary")
            .unwrap();
        assert_eq!(summary["testsAnalyzed"], 2);
        assert_eq!(summary["unresolvedTests"], 0);
        assert_eq!(summary["incomplete"], true);
        assert_eq!(summary["incompleteCauses"]["discoveryFailed"], true);
        assert!(records
            .iter()
            .any(|record| record["rule"] == "duplicate-test"));
        assert_eq!(code, Some(2));
        assert!(stderr.contains("ignore line exceeds 8 KiB"), "{stderr}");
    }

    /// A user-managed global link honors its rules without turning a valid run incomplete.
    #[test]
    #[cfg(unix)]
    fn bounded_global_ignore_symlink_reaches_the_cli() {
        let source = include_str!("../fixtures/recall/ignore-input-bounded/a.test.ts");
        let directory = single_file_project("a.test.ts", source);
        let root = directory.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join("ignored.test.ts"), source).unwrap();
        std::fs::write(root.join("rules"), "ignored.test.ts\n").unwrap();
        let link = root.join("global-ignore");
        std::os::unix::fs::symlink(root.join("rules"), &link).unwrap();
        let config = root.join("global-config");
        let prefix = root.join("report");
        for (global, tests) in [(link.as_path(), 2), (std::path::Path::new("/dev/null"), 4)] {
            std::fs::write(
                &config,
                format!("[core]\n excludesFile = {}\n", global.display()),
            )
            .unwrap();
            assert_cmd::Command::cargo_bin("vitest-dedup")
                .unwrap()
                .arg(root)
                .args(["--reporters", "json", "--output"])
                .arg(&prefix)
                .arg("--fail-on-incomplete=reads")
                .env("GIT_CONFIG_GLOBAL", &config)
                .assert()
                .code(1);
            let report = read_json_report(&prefix);
            assert_eq!(report["testsAnalyzed"], tests);
            assert_eq!(report["unresolvedTests"], 0);
            assert_eq!(report["incomplete"], false);
            assert!(report["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|finding| finding["rule"] == "duplicate-test"));
        }
    }

    /// An independent repository stays complete despite unusable Git rules above its boundary.
    #[test]
    fn bounded_ancestor_git_ignores_preserve_the_complete_cli_result() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(
            root.join("a.test.ts"),
            include_str!("../fixtures/recall/ignore-input-bounded/a.test.ts"),
        )
        .unwrap();
        std::fs::write(parent.path().join(".gitignore"), "#".repeat(8193)).unwrap();
        let (_, report) = run_json_with_stderr(&root, ExitCode::QualityGateFailed);
        assert_eq!(
            (
                report["testsAnalyzed"].as_u64(),
                report["incomplete"].as_bool()
            ),
            (Some(2), Some(false))
        );
        assert_eq!(report["findings"].as_array().unwrap().len(), 1);
        assert_eq!(report["findings"][0]["rule"], "duplicate-test");
    }

    /// Absent repository markers keep the owned duplicate positive complete in strict mode.
    #[test]
    #[cfg(unix)]
    fn bounded_unresolvable_git_markers_preserve_complete_cli_results() {
        let directory = single_file_project(
            "a.test.ts",
            include_str!("../fixtures/recall/ignore-input-bounded/a.test.ts"),
        );
        let root = directory.path();
        for target in ["missing", ".git"] {
            std::os::unix::fs::symlink(target, root.join(".git")).unwrap();
            let (_, report) = run_json_with_stderr(root, ExitCode::QualityGateFailed);
            assert_eq!(report["testsAnalyzed"], 2);
            assert_eq!(report["unresolvedTests"], 0);
            assert_eq!(report["incomplete"], false);
            let findings = report["findings"].as_array().unwrap();
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0]["rule"], "duplicate-test");
            let (code, _, _) = run_cli(root, &["--fail-on-incomplete=reads"]);
            assert_eq!(code, Some(ExitCode::QualityGateFailed.code()));
            std::fs::remove_file(root.join(".git")).unwrap();
        }
    }

    /// Significant spaces in both metadata pointers preserve Git exclusions and owned positives.
    #[test]
    #[cfg(unix)]
    fn bounded_metadata_paths_preserve_the_cli_and_git_result() {
        let source = include_str!("../fixtures/recall/ignore-input-bounded/a.test.ts");
        for ending in ["", "\n", "\r\n"] {
            for common in [false, true] {
                let directory = single_file_project("a.test.ts", source);
                let root = directory.path();
                git(root, &["init", "--quiet"]);
                std::fs::write(root.join("ignored.test.ts"), source).unwrap();
                let metadata = root.join(" metadata ");
                std::fs::rename(root.join(".git"), &metadata).unwrap();
                std::fs::write(
                    root.join(".git"),
                    format!("gitdir: {}{ending}", metadata.display()),
                )
                .unwrap();
                let excludes = if common {
                    let shared = root.join(" shared ");
                    std::fs::rename(&metadata, &shared).unwrap();
                    std::fs::create_dir(&metadata).unwrap();
                    std::fs::copy(shared.join("HEAD"), metadata.join("HEAD")).unwrap();
                    std::fs::write(metadata.join("commondir"), format!("../ shared {ending}"))
                        .unwrap();
                    shared
                } else {
                    metadata
                };
                std::fs::write(excludes.join("info/exclude"), "ignored.test.ts\n").unwrap();
                git(root, &["check-ignore", "--no-index", "ignored.test.ts"]);
                let (_, report) = run_json_with_stderr(root, ExitCode::QualityGateFailed);
                assert_eq!(report["testsAnalyzed"], 2, "{ending:?}, common={common}");
                assert_eq!(report["unresolvedTests"], 0);
                assert_eq!(report["incomplete"], false);
                let findings = report["findings"].as_array().unwrap();
                assert_eq!(findings.len(), 1);
                assert_eq!(findings[0]["rule"], "duplicate-test");
            }
        }
    }

    /// Read and parse the JSON file for an output prefix, panicking with path context on
    /// failure.
    fn read_json_report(prefix: &std::path::Path) -> serde_json::Value {
        let path = prefix.with_extension("json");
        serde_json::from_str(
            &std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("reading {}: {error}", path.display())),
        )
        .expect("the JSON reporter emits valid JSON")
    }

    /// An oversized file is not a traversal failure, and the banner must not say it is.
    ///
    /// `discovery` sets its aggregate `incomplete` flag for nine different reasons, and mapping
    /// that flag to `discovery_failed` made every one of them print "directory traversal failed,
    /// so files may be missing from this run entirely". For a repository whose only problem is
    /// one large file, that sends the reader looking for a broken filesystem walk that never
    /// happened -- while `filesTooLarge`, the counter that describes exactly what did happen,
    /// stayed at zero because an oversized file is skipped in discovery and never reaches the
    /// read-time size check that increments it.
    #[test]
    fn an_oversized_file_is_counted_as_too_large_and_not_as_a_failed_traversal() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        // A real duplicate pair, so the run has something to say and the report is not empty.
        std::fs::write(root.join("math.test.ts"), DUPLICATE_PAIR).unwrap();
        // Just past the limit, not comfortably past it: a bound is tested at its edge.
        let oversized = "// ".to_owned()
            + &"x".repeat(usize::try_from(vitest_dedup::discovery::MAX_SOURCE_FILE_BYTES).unwrap())
            + "\n";
        std::fs::write(root.join("big.test.ts"), oversized).unwrap();

        let prefix = root.join("report");
        let (_, _, stderr) = run_cli(
            root,
            &["--reporters", "json", "--output", prefix.to_str().unwrap()],
        );
        let report = read_json_report(&prefix);
        let causes = &report["incompleteCauses"];

        assert_eq!(report["incomplete"], true, "{report:#?}");
        assert_eq!(
            causes["filesTooLarge"], 1,
            "the oversized file is counted as what it is: {report:#?}"
        );
        assert_eq!(
            causes["discoveryFailed"], false,
            "nothing failed to traverse: {report:#?}"
        );
        assert!(
            !stderr.contains("traversal failed"),
            "and no reporter may say it did: {stderr}"
        );
    }

    /// `--output` creates the directories its prefix names.
    ///
    /// A reporter writing into `reports/2026-09/run` must not fail because `reports/2026-09` does
    /// not exist yet -- the prefix is a path the caller chose, not a directory they were asked to
    /// prepare. Covers the `create_dir_all` arm, which no other test reaches because every other
    /// `--output` in this suite points straight at a temp directory that already exists.
    #[test]
    fn output_creates_the_directories_its_prefix_names() {
        let directory = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        let prefix = root.join("nested").join("deeper").join("report");

        let (_, _, stderr) = run_cli(
            root,
            &["--reporters", "json", "--output", prefix.to_str().unwrap()],
        );

        assert!(
            prefix.with_extension("json").exists(),
            "the report is written through directories that did not exist: {stderr}"
        );
        let report = read_json_report(&prefix);
        assert_eq!(report["testsAnalyzed"], 2, "{report:#?}");
    }

    #[test]
    /// End to end: record a baseline, re-run, and the finding passes the gate while staying on
    /// screen marked `[baselined]` -- visible debt, not hidden debt.
    fn a_baselined_finding_passes_the_gate_and_is_still_visible_in_every_report() {
        let directory = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        let baseline = root.join("vitest-dedup-baseline.json");

        // Before any baseline: the gate fails. Without this the rest proves nothing.
        let (code, _, _) = run_cli(root, &[]);
        assert_eq!(code, Some(1), "the pair must fail the gate to begin with");

        // Record it.
        let (code, _, stderr) = run_cli(
            root,
            &[
                "--baseline",
                baseline.to_str().unwrap(),
                "--update-baseline",
            ],
        );
        assert_eq!(code, Some(0), "recording a baseline is not a gate failure");
        assert!(stderr.contains("wrote baseline"), "{stderr}");
        assert!(baseline.is_file());

        // Apply it: clean exit, finding still on screen, marked rather than hidden.
        let prefix = root.join("report");
        let (code, stdout, stderr) = run_cli(
            root,
            &[
                "--baseline",
                baseline.to_str().unwrap(),
                "--reporters",
                "terminal,json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        assert_eq!(code, Some(0), "a baselined finding must not fail the gate");
        assert!(
            stdout.contains("duplicate-test") && stdout.contains("math.test.ts"),
            "the finding must stay in the report, not disappear: {stdout}"
        );
        assert!(
            stdout.contains("[baselined]"),
            "a suppressed finding must be labeled as such: {stdout}"
        );
        assert!(
            stdout.contains("Baseline: 1 accepted"),
            "the summary must say how much is accepted: {stdout}"
        );
        assert!(
            stderr.contains("do not affect the quality gate"),
            "{stderr}"
        );

        let report = read_json_report(&prefix);
        assert_eq!(report["findings"].as_array().unwrap().len(), 1);
        assert_eq!(report["findings"][0]["baselined"], true);
        assert_eq!(report["modes"]["baselined"], 1);
        assert_eq!(report["modes"]["newFindings"], 0);
        assert_eq!(report["modes"]["baselineApplied"], true);
    }

    /// P4 flagship: `--baseline-from-ref HEAD` treats the base revision's findings as accepted,
    /// so a PR touching nothing about the duplicates gates clean -- while the finding itself
    /// stays in every report, marked like any baselined debt. The containment is asserted
    /// operationally: the flag works in a repo whose analysis would otherwise be identical.
    #[test]
    fn baseline_from_ref_accepts_the_base_revisions_findings() {
        // The duplicate pair is committed; the working tree is identical to it.
        let directory = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        init_repository(root);

        // Without the flag: the gate fails.
        let (code, _, _) = run_cli(root, &[]);
        assert_eq!(
            code,
            Some(1),
            "the pair must fail the gate without the flag"
        );

        // With the flag: the base revision's finding is accepted debt, and the report says both
        // the source of that answer and the finding's marked status.
        let prefix = root.join("report");
        let (code, stdout, stderr) = run_cli(
            root,
            &[
                "--baseline-from-ref",
                "HEAD",
                "--fail-on-new",
                "0",
                "--reporters",
                "terminal,json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        assert_eq!(
            code,
            Some(0),
            "the committed pair is the base's own debt: {stderr}"
        );
        assert!(
            stderr.contains("baseline from HEAD: 1 finding(s)"),
            "the run must state what the base revision contributed: {stderr}"
        );
        assert!(
            stdout.contains("[baselined]"),
            "the finding stays visible, marked as accepted: {stdout}"
        );
        let report = read_json_report(&prefix);
        assert_eq!(report["modes"]["baselineApplied"], true);
        assert_eq!(report["modes"]["baselined"], 1);
        assert_eq!(report["modes"]["newFindings"], 0);
    }

    /// The base revision's titles render as its own committed Vitest major: an upgrade that
    /// makes two titles collide is new, and the same declaration on both sides keeps the
    /// collision baselined. A base without metadata takes `--vitest-major`, else proves only
    /// what every major agrees on, and never reads the working tree's declaration.
    #[test]
    fn baseline_from_ref_renders_base_titles_with_its_committed_major() {
        let source = "import { expect, test } from 'vitest'\n\
                      test('named abc', () => { expect(1).toBe(1) })\n\
                      test.each([{ v: 'abc' }])('named $v', ({ v }) => { expect(v).toBe('abc') })\n";
        let manifest = |major: u64| format!(r#"{{"devDependencies":{{"vitest":"^{major}.0.0"}}}}"#);
        let new_findings = |root: &std::path::Path, extra: &[&str]| {
            let prefix = root.join("report");
            let mut arguments = vec![
                "--baseline-from-ref",
                "HEAD",
                "--rule",
                "parameterization-candidate=off",
                "--reporters",
                "json",
                "--output",
                prefix.to_str().unwrap(),
            ];
            arguments.extend_from_slice(extra);
            let (_, _, stderr) = run_cli(root, &arguments);
            let report = read_json_report(&prefix);
            assert_eq!(report["findings"].as_array().unwrap().len(), 1, "{stderr}");
            report["modes"]["newFindings"].clone()
        };
        let declared = single_file_project("named.test.ts", source);
        let root = declared.path();
        std::fs::write(root.join("package.json"), manifest(4)).unwrap();
        init_repository(root);
        std::fs::write(root.join("package.json"), manifest(5)).unwrap();
        assert_eq!(
            new_findings(root, &[]),
            1,
            "the upgrade introduced the collision"
        );
        git(root, &["commit", "-qam", "upgrade"]);
        assert_eq!(
            new_findings(root, &[]),
            0,
            "the base already declared Vitest 5"
        );

        let undeclared = single_file_project("named.test.ts", source);
        let root = undeclared.path();
        init_repository(root);
        assert_eq!(new_findings(root, &["--vitest-major", "5"]), 0);
        std::fs::write(root.join("package.json"), manifest(5)).unwrap();
        assert_eq!(
            new_findings(root, &[]),
            1,
            "the uncommitted declaration does not answer for the base"
        );
    }

    /// A base revision that predates the duplication accepts nothing, so the pair is new and
    /// the gate fails -- the flag can only accept what the base actually had, never paper over
    /// the working tree.
    #[test]
    fn baseline_from_ref_accepts_nothing_the_base_revision_did_not_have() {
        // Commit an empty tree, THEN add the duplicate pair uncommitted.
        let directory = single_file_project("placeholder.test.ts", CLEAN_TESTS);
        let root = directory.path();
        init_repository(root);
        std::fs::write(root.join("dirty.test.ts"), DUPLICATE_PAIR).unwrap();

        let (code, _, stderr) = run_cli(
            root,
            &[
                "--baseline-from-ref",
                "HEAD",
                "--fail-on-new",
                "0",
                "--threshold",
                "100",
            ],
        );
        assert_eq!(
            code,
            Some(1),
            "a finding the base lacks is new, and new errors gate"
        );
        assert!(
            stderr.contains("baseline from HEAD: 0 finding(s)"),
            "the empty base must be stated, not silently treated as no flag: {stderr}"
        );
    }

    /// The review-found geometry: the analysis root is a SUBDIRECTORY of the git repository.
    /// `ls-tree` run from there lists paths relative to it; a read that resolved those paths from
    /// the repository root would silently read the wrong files (or fail), and the baseline would
    /// be computed from nothing. Blobs are now read by the object id `ls-tree` lists. Pinned from
    /// `src/`: a committed duplicate pair under a subdirectory must still be accepted from the
    /// base revision.
    #[test]
    fn baseline_from_ref_works_when_the_analysis_root_is_below_the_git_root() {
        // One git repository; the duplicate pair is committed under packages/app/ -- committed
        // by the OUTER repository (a nested `git init` would make the subdirectory its own root
        // and the geometry under test would vanish).
        let repository = single_file_project("placeholder.test.ts", CLEAN_TESTS);
        let root = repository.path();
        init_repository(root);
        let package = root.join("packages").join("app");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("math.test.ts"), DUPLICATE_PAIR).unwrap();
        for arguments in [
            ["add", "."].as_slice(),
            ["commit", "-qm", "add package"].as_slice(),
        ] {
            assert!(std::process::Command::new("git")
                .args(arguments)
                .current_dir(root)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .status()
                .unwrap()
                .success());
        }

        // The analysis root is packages/app: a subdirectory of the git repository.
        let (code, stdout, stderr) = run_cli(&package, &["--baseline-from-ref", "HEAD"]);
        assert_eq!(
            code,
            Some(0),
            "the committed pair under packages/app is the base's own debt: {stderr}"
        );
        assert!(
            stderr.contains("baseline from HEAD: 1 finding(s)"),
            "{stderr}"
        );
        assert!(stdout.contains("[baselined]"), "{stdout}");
    }

    /// The revision is validated like `--changed-since`'s: a flag-looking or empty revision is
    /// an operational error (exit 2), never an injection into a git invocation.
    #[test]
    fn baseline_from_ref_rejects_an_invalid_revision() {
        let directory = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        init_repository(root);
        // `--exec` as a separate arg is rejected by the CLI grammar itself; the `=` form
        // reaches this tool's own validation, which must still refuse it -- a revision is data,
        // never a flag, whichever spelling delivered it.
        let (code, _, stderr) = run_cli(root, &["--baseline-from-ref=--exec"]);
        assert_eq!(code, Some(2));
        assert!(
            stderr.contains("invalid --baseline-from-ref revision"),
            "{stderr}"
        );
    }

    /// Refusals keep listing order: an oversized base file listed before an unreadable entry (a
    /// gitlink named like a test file) still gets its skip line before the run fails, as it did
    /// when each file was read in turn.
    #[test]
    fn baseline_from_ref_reports_earlier_skips_before_a_later_refusal() {
        let directory = single_file_project("c-ok.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        std::fs::write(root.join("a-huge.test.ts"), "x".repeat(8 * 1024 * 1024 + 1)).unwrap();
        init_repository(root);
        // A gitlink's commit need not exist in this repository.
        git(
            root,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000",
                "1234567890123456789012345678901234567890",
                "b-vendor.test.ts",
            ],
        );
        git(root, &["commit", "-qm", "gitlink"]);
        let (code, _, stderr) = run_cli(root, &["--baseline-from-ref", "HEAD"]);
        assert_eq!(code, Some(2), "{stderr}");
        let skip = stderr
            .find("skipping a-huge.test.ts in the base revision: exceeds the byte limit")
            .unwrap_or_else(|| panic!("the earlier skip is reported: {stderr}"));
        let refusal = stderr
            .find("git failed while reading base file `b-vendor.test.ts`")
            .unwrap_or_else(|| panic!("the gitlink is refused: {stderr}"));
        assert!(skip < refusal, "skip before refusal: {stderr}");
    }

    #[test]
    /// P3: the directive beside one participant removes the finding, the summary says so, and
    /// `--update-baseline` on the suppressed tree records nothing -- a baseline records accepted
    /// debt, and a suppressed finding claims there is none.
    fn inline_suppression_removes_the_finding_and_outranks_the_baseline() {
        // The directive sits on the line before the first declaration of the pair.
        let source = DUPLICATE_PAIR.replace(
            "        test('adds numbers', () => {",
            "        // vitest-dedup-disable-next-line these two are deliberately parallel examples\n        test('adds numbers', () => {",
        );
        let directory = single_file_project("math.test.ts", &source);
        let root = directory.path();

        // Suppressed: the gate passes and the run says why.
        let prefix = root.join("report");
        let (code, stdout, stderr) = run_cli(
            root,
            &[
                "--reporters",
                "terminal,json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        assert_eq!(
            code,
            Some(0),
            "a suppressed pair must not fail the gate: {stderr}"
        );
        assert!(
            stdout.contains("1 finding suppressed by inline directive"),
            "the removal must be visible in the summary: {stdout}"
        );
        let report = read_json_report(&prefix);
        assert_eq!(report["findings"].as_array().unwrap().len(), 0);
        assert_eq!(report["modes"]["suppressionsApplied"], 1);

        // Precedence: `--update-baseline` on the suppressed tree records NOTHING, because the
        // finding never survives to the writer -- a baseline is a record of accepted debt, and
        // a suppressed finding is a claim that there is none.
        let baseline = root.join("baseline.json");
        let (code, _, stderr) = run_cli(
            root,
            &[
                "--baseline",
                baseline.to_str().unwrap(),
                "--update-baseline",
            ],
        );
        assert_eq!(code, Some(0), "{stderr}");
        assert!(
            stderr.contains("0 finding(s) recorded"),
            "the suppressed finding must never reach the baseline writer: {stderr}"
        );

        // The unsuppressed twin, same tree: a directive beside only one of the pair's
        // participants would not be enough -- both participants must be checked. Here neither
        // is, on a fresh copy without any directive: it fires.
        let plain = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let (code, _, _) = run_cli(plain.path(), &[]);
        assert_eq!(code, Some(1), "without a directive the pair still fires");
    }

    /// Regression fixture for CLI reporting: fail on new fires only above its limit.
    #[test]
    fn fail_on_new_fires_only_above_its_limit() {
        let directory = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        let baseline = root.join("baseline.json");
        let baseline_arg = baseline.to_str().unwrap();

        run_cli(root, &["--baseline", baseline_arg, "--update-baseline"]);

        // Exactly one finding is now new.
        std::fs::write(root.join("more.test.ts"), OTHER_DUPLICATE_PAIR).unwrap();
        let prefix = root.join("report");
        let (_, _, _) = run_cli(
            root,
            &[
                "--baseline",
                baseline_arg,
                "--reporters",
                "json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        assert_eq!(
            read_json_report(&prefix)["modes"]["newFindings"],
            1,
            "sanity: the boundary cases below are meaningless unless this is exactly 1"
        );

        // `--fail-on-new N` fails when *more than* N are new, so 1 passes at 1 and fails at 0.
        // `--threshold 100` isolates this gate from the severity gate, which at its default of 0
        // would fail on the same error-severity finding and make both boundaries read `1`.
        let boundary = |limit: &str| {
            run_cli(
                root,
                &[
                    "--baseline",
                    baseline_arg,
                    "--threshold",
                    "100",
                    "--fail-on-new",
                    limit,
                ],
            )
            .0
        };
        assert_eq!(
            boundary("1"),
            Some(0),
            "1 new finding is within a limit of 1"
        );
        assert_eq!(
            boundary("0"),
            Some(1),
            "exceeding --fail-on-new is a quality-gate failure (1), never an operational one (2)"
        );

        // The two gates are additive, never alternatives: a generous `--fail-on-new` must not
        // rescue a run the severity gate already fails, or it would become a way to raise the
        // threshold without saying so.
        let (code, _, _) = run_cli(root, &["--baseline", baseline_arg, "--fail-on-new", "99"]);
        assert_eq!(code, Some(1));
    }

    /// Regression fixture for CLI reporting: update baseline refuses to record an incomplete
    /// run.
    #[test]
    fn update_baseline_refuses_to_record_an_incomplete_run() {
        // An incomplete run does not know what it would be accepting, and a baseline written from
        // one silently accepts everything it could not see the first time a complete run finds it.
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        // An ambient-`globals` file with no config: extraction yields nothing and the run is
        // marked incomplete, the same shape `--fail-on-incomplete` exists for.
        std::fs::write(
            root.join("ambient.test.ts"),
            "test('ambient', () => { expect(1).toBe(1); });",
        )
        .unwrap();
        let baseline = root.join("baseline.json");

        let (code, _, stderr) = run_cli(
            root,
            &[
                "--baseline",
                baseline.to_str().unwrap(),
                "--update-baseline",
            ],
        );
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("refusing to write baseline"), "{stderr}");
        assert!(!baseline.exists(), "no partial baseline may be left behind");
    }

    // -- `--changed-since` ------------------------------------------------------------------

    /// Run a fixture Git operation after removing inherited repository-path environment
    /// variables.
    fn git(root: &std::path::Path, args: &[&str]) {
        let mut command = std::process::Command::new("git");
        // The same inherited-environment hazard `modes.rs` documents: when this suite runs from a
        // Git hook, `GIT_DIR` and friends outrank `current_dir` and these fixtures would stage
        // themselves into the real repository's index.
        for variable in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_COMMON_DIR",
            "GIT_PREFIX",
            "GIT_CEILING_DIRECTORIES",
        ] {
            command.env_remove(variable);
        }
        let status = command
            .current_dir(root)
            .args(args)
            .status()
            .expect("git must be available to run these tests");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Initialize and commit the fixture tree with a local test identity on main.
    fn init_repository(root: &std::path::Path) {
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.email", "test@example.com"]);
        git(root, &["config", "user.name", "Test"]);
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "initial"]);
    }

    #[test]
    fn changed_since_reports_an_incomplete_run_rather_than_a_false_clean() {
        // The failure this whole design exists to prevent. The file that changed is clean; the
        // duplication lives in a file nobody touched. A changed-files run has nothing to report --
        // and must not let that be read as "the repository is clean".
        let directory = single_file_project("clean.test.ts", CLEAN_TESTS);
        let root = directory.path();
        std::fs::write(root.join("dirty.test.ts"), DUPLICATE_PAIR).unwrap();
        init_repository(root);
        std::fs::write(
            root.join("clean.test.ts"),
            format!("{CLEAN_TESTS}\n// touched\n"),
        )
        .unwrap();

        let prefix = root.join("report");
        let (code, stdout, stderr) = run_cli(
            root,
            &[
                "--changed-since",
                "HEAD",
                "--reporters",
                "terminal,json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );

        let report = read_json_report(&prefix);
        assert_eq!(
            report["findings"].as_array().unwrap().len(),
            0,
            "the changed file really is clean"
        );
        assert_eq!(
            report["incomplete"], true,
            "an empty changed-files report that withheld a real finding is not a clean run"
        );
        assert_eq!(report["modes"]["withheldUnchanged"], 1);
        assert_eq!(report["modes"]["changedScope"], true);
        assert!(
            stdout.contains("CHANGED-FILES RUN") && stdout.contains("withheld"),
            "the report must say why it cannot speak for the repository: {stdout}"
        );
        assert!(stderr.contains("withheld"), "{stderr}");
        assert_eq!(
            code,
            Some(0),
            "no reported finding means the gate passes -- which is exactly why `incomplete` has \
             to carry the caveat"
        );

        // And CI that refuses degraded runs gets the operational error it asked for.
        let (code, _, _) = run_cli(root, &["--changed-since", "HEAD", "--fail-on-incomplete"]);
        assert_eq!(code, Some(2));
    }

    #[test]
    fn changed_since_still_reports_a_finding_in_a_file_that_changed() {
        let directory = single_file_project("dirty.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        std::fs::write(root.join("clean.test.ts"), CLEAN_TESTS).unwrap();
        init_repository(root);
        std::fs::write(
            root.join("dirty.test.ts"),
            format!("{DUPLICATE_PAIR}\n// touched\n"),
        )
        .unwrap();

        let prefix = root.join("report");
        let (code, _, _) = run_cli(
            root,
            &[
                "--changed-since",
                "HEAD",
                "--reporters",
                "json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        let report = read_json_report(&prefix);
        assert_eq!(report["findings"].as_array().unwrap().len(), 1);
        assert_eq!(report["modes"]["withheldUnchanged"], 0);
        assert_eq!(
            report["incomplete"], false,
            "a changed-files run that withheld nothing is a full run's report and must not cry \
             wolf about coverage"
        );
        assert_eq!(code, Some(1));
    }

    #[test]
    fn changed_since_finds_a_cross_file_duplicate_whose_twin_is_unchanged() {
        // The trap: `cross-file-duplicate` compares across files. Analysis therefore covers the
        // whole corpus and only findings are filtered -- had the corpus been restricted to changed
        // files, the twin would never have been extracted and this run would have reported the
        // changed file as clean.
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let shared = r#"
            import { test, expect } from 'vitest';

            test('shared behavior', () => {
                const total = 2 + 2;
                expect(total).toBe(4);
            });
        "#;
        std::fs::write(root.join("a.test.ts"), shared).unwrap();
        std::fs::write(root.join("b.test.ts"), shared).unwrap();
        init_repository(root);
        std::fs::write(root.join("a.test.ts"), format!("{shared}\n// touched\n")).unwrap();

        let prefix = root.join("report");
        run_cli(
            root,
            &[
                "--changed-since",
                "HEAD",
                "--rule",
                "cross-file-duplicate=error",
                "--reporters",
                "json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        let report = read_json_report(&prefix);
        let findings = report["findings"].as_array().unwrap();
        assert_eq!(
            findings.len(),
            1,
            "the unchanged twin must still be analyzed and reported: {report:#?}"
        );
        assert_eq!(findings[0]["rule"], "cross-file-duplicate");
    }

    #[test]
    fn changed_since_against_a_missing_revision_is_an_operational_error() {
        let directory = single_file_project("a.test.ts", CLEAN_TESTS);
        let root = directory.path();
        init_repository(root);

        let (code, _, stderr) = run_cli(root, &["--changed-since", "no-such-ref"]);
        assert_eq!(
            code,
            Some(2),
            "an unresolvable revision must not be a silent empty analysis: {stderr}"
        );
        assert!(stderr.contains("does not resolve to a commit"), "{stderr}");
    }

    #[test]
    fn changed_since_outside_a_git_repository_is_an_operational_error() {
        let directory = single_file_project("a.test.ts", CLEAN_TESTS);
        let root = directory.path();

        let (code, _, stderr) = run_cli(root, &["--changed-since", "HEAD"]);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("requires a Git repository"), "{stderr}");
    }

    #[test]
    fn changed_since_in_a_shallow_clone_is_an_operational_error_naming_the_fix() {
        // A shallow clone is the dangerous case: `HEAD~1` or a base branch may exist as a name
        // while its history does not, so the diff is silently wrong rather than obviously absent.
        // GitHub Actions checks out depth 1 by default, which makes this the single most likely
        // way `--changed-since` gets used wrongly in CI.
        let origin = tempfile::tempdir().unwrap();
        std::fs::write(origin.path().join("a.test.ts"), CLEAN_TESTS).unwrap();
        init_repository(origin.path());
        std::fs::write(
            origin.path().join("a.test.ts"),
            format!("{CLEAN_TESTS}\n// second\n"),
        )
        .unwrap();
        git(origin.path(), &["commit", "-qam", "second"]);

        let workspace = tempfile::tempdir().unwrap();
        let clone = workspace.path().join("shallow");
        git(
            workspace.path(),
            &[
                "clone",
                "-q",
                "--depth",
                "1",
                "--no-local",
                &format!("file://{}", origin.path().display()),
                clone.to_str().unwrap(),
            ],
        );

        let (code, _, stderr) = run_cli(&clone, &["--changed-since", "HEAD"]);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("shallow clone"), "{stderr}");
        assert!(stderr.contains("fetch-depth: 0"), "{stderr}");
    }

    #[test]
    fn a_stale_baseline_is_an_operational_error_not_a_wall_of_new_findings() {
        // A baseline computed against a different fingerprint derivation matches nothing. Treating
        // that as "everything is new" would look exactly like a sudden mass regression and would
        // be acted on as one; it is a broken input, so it exits 2.
        let directory = single_file_project("math.test.ts", DUPLICATE_PAIR);
        let root = directory.path();
        let baseline = root.join("baseline.json");
        std::fs::write(
            &baseline,
            r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v0","findings":[]}"#,
        )
        .unwrap();

        let (code, _, stderr) = run_cli(root, &["--baseline", baseline.to_str().unwrap()]);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("fingerprint version"), "{stderr}");
        assert!(stderr.contains("--update-baseline"), "{stderr}");
    }

    // =========================================================================================
    // Type-level tests, end to end through the real binary.
    //
    // A type-level suite's whole content is its type arguments. When those were erased from the
    // body fingerprint, `expectTypeOf`/`assertType`/hand-rolled `assertEqual<A, B>` suites read
    // as a file full of byte-identical tests and failed other people's builds at Error severity.
    // The three rules below are asserted separately because they key on different things:
    // `duplicate-test` on `alpha`, `near-duplicate-test` and `subsumed-test` on behavior events.
    // =========================================================================================

    /// Every rule enabled, including `subsumed-test`, the one rule that is off by default, so a
    /// run that reports nothing has actually been asked the question by all three.
    fn run_all_rules(root: &std::path::Path, prefix: &std::path::Path) -> serde_json::Value {
        run_cli(
            root,
            &[
                "--rule",
                "subsumed-test=warning",
                "--reporters",
                "json",
                "--output",
                prefix.to_str().unwrap(),
            ],
        );
        read_json_report(prefix)
    }

    /// The reproduction: two type-level assertions differing only in the type they assert about.
    /// This was one `duplicate-test` at Error severity, `incomplete: false`, on correct code.
    #[test]
    fn type_level_tests_differing_only_in_type_arguments_are_not_duplicates() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(
            root.join("types.test.ts"),
            r#"
            import { test } from 'vitest';
            import { assertEqual } from './helpers';

            type Expected = Record<string, unknown>;

            test('checks a string mapping', () => {
                assertEqual<Record<string, string>, Expected>(true);
            });

            test('checks a number mapping', () => {
                assertEqual<Record<string, number>, Expected>(true);
            });
            "#,
        )
        .unwrap();

        let prefix = root.join("report");
        let report = run_all_rules(root, &prefix);
        assert_eq!(
            report["findings"].as_array().unwrap().len(),
            0,
            "two assertions about different types are not duplicates: {report:#?}"
        );
    }

    /// The same shape under `near-duplicate-test`, which keys on the sorted behavior multiset
    /// rather than on `alpha`: reordering the setup call makes the two bodies' `alpha` differ, so
    /// only the behavior events stand between these two tests and a finding.
    #[test]
    fn reordered_type_level_tests_are_not_near_duplicates() {
        let report = type_level_report(
            ", prepare",
            r#"
            test('checks a string mapping', () => {
                prepare();
                assertEqual<Record<string, string>, Expected>(true);
            });

            test('checks a number mapping', () => {
                assertEqual<Record<string, number>, Expected>(true);
                prepare();
            });
            "#,
        );
        assert_eq!(
            report["findings"].as_array().unwrap().len(),
            0,
            "the two bodies run different assertions, so their behavior events must differ: \
             {report:#?}"
        );
    }

    /// The same shape under `subsumed-test` (off by default, enabled here), which asks whether one
    /// body's behavior events are a strict ordered subsequence of the other's. With the type
    /// arguments erased, the shorter test's events were exactly a prefix of the longer one's.
    #[test]
    fn a_shorter_type_level_test_does_not_subsume_a_longer_one() {
        let report = type_level_report(
            ", prepare, cleanup",
            r#"
            test('checks a string mapping', () => {
                prepare();
                assertEqual<Record<string, string>, Expected>(true);
            });

            test('checks a number mapping and tidies up', () => {
                prepare();
                assertEqual<Record<string, number>, Expected>(true);
                cleanup();
            });
            "#,
        );
        assert_eq!(
            report["findings"].as_array().unwrap().len(),
            0,
            "an assertion about `string` is not contained in one about `number`: {report:#?}"
        );
    }

    /// The control that keeps the fix honest: two type-level tests that really are identical, type
    /// arguments included, are still reported. A fix that silenced the false positive by making
    /// every generic call distinct would pass the three tests above and fail this one.
    #[test]
    fn identical_type_level_tests_are_still_reported_as_duplicates() {
        let report = type_level_report(
            "",
            &[
                STRING_MAPPING_TEST,
                r#"

            test('checks a string mapping, again', () => {
                assertEqual<Record<string, string>, Expected>(true);
            });
            "#,
            ]
            .concat(),
        );
        let findings = report["findings"].as_array().unwrap();
        assert_eq!(findings.len(), 1, "{report:#?}");
        assert_eq!(findings[0]["rule"], "duplicate-test");
    }

    /// Whitespace inside a type argument is formatting, not content: a prettier run that respaced
    /// `Record<string, string>` must not hide a real duplicate.
    #[test]
    fn type_arguments_differing_only_in_whitespace_are_still_duplicates() {
        let report = type_level_report(
            "",
            &[
                STRING_MAPPING_TEST,
                r#"

            test('checks a string mapping, again', () => {
                assertEqual<
                    Record< string , string >,
                    Expected
                >(true);
            });
            "#,
            ]
            .concat(),
        );
        let findings = report["findings"].as_array().unwrap();
        assert_eq!(
            findings.len(),
            1,
            "the two type arguments name the same type: {report:#?}"
        );
        assert_eq!(findings[0]["rule"], "duplicate-test");
    }

    // =========================================================================================
    // `--emit-test-fingerprints`, end to end through the real binary.
    //
    // The records exist so a consumer can check a claim about tests that appear in NO finding --
    // which is what every `rule=none` negative fixture's participants are
    // (`scripts/check/check-fixture-isolation.mjs`). The assertions below are therefore about
    // what the analyzer saw, not about what it reported: two tests differing only in a literal
    // must share `structural` and differ in `alpha`; two differing only in a `beforeEach` must
    // share both and differ in `contextKey`. A record kind that could not distinguish those two
    // cases would leave the gate exactly where it is today.
    // =========================================================================================

    /// The plain string-mapping declaration the identical-duplicate and whitespace fixtures
    /// both open with -- the control both compare their variation against.
    const STRING_MAPPING_TEST: &str = r#"
            test('checks a string mapping', () => {
                assertEqual<Record<string, string>, Expected>(true);
            });"#;

    /// Build a temporary type-level declaration fixture and return its all-rules JSON report.
    fn type_level_report(helpers: &str, declarations: &str) -> serde_json::Value {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(
            root.join("types.test.ts"),
            format!(
                "\n            import {{ test }} from 'vitest';\n            import {{ assertEqual{helpers} }} from './helpers';\n\n            type Expected = Record<string, unknown>;\n{declarations}",
            ),
        )
        .unwrap();
        run_all_rules(root, &root.join("report"))
    }

    /// The additive-flag comparison the two stream-riding flags each get pinned by: one
    /// one-file corpus, run with and without `flag`, keeping stdout and exit code of both --
    /// everything an "adds records, edits nothing" assertion needs.
    fn jsonl_with_and_without(flag: &str) -> ((Option<i32>, String), (Option<i32>, String)) {
        let root = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let (code, stdout, _) = run_cli(root.path(), &["--reporters", "jsonl"]);
        let (flagged_code, flagged_stdout, _) =
            run_cli(root.path(), &["--reporters", "jsonl", flag]);
        ((code, stdout), (flagged_code, flagged_stdout))
    }

    /// The `--emit-test-fingerprints` stream over one fixture file plus its guaranteed-final
    /// summary record: the opening shape of every stream test that reads the summary.
    fn fingerprint_stream_and_summary(
        file: &str,
        contents: &str,
    ) -> (Vec<serde_json::Value>, serde_json::Value) {
        let lines = run_jsonl(
            single_file_project(file, contents).path(),
            &["--reporters", "jsonl", "--emit-test-fingerprints"],
        );
        let summary = lines.last().expect("a summary line is always last").clone();
        assert_eq!(summary["type"], "summary");
        (lines, summary)
    }

    /// Create and return a temporary workspace containing the requested source file.
    fn single_file_project(file: &str, contents: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(file), contents).unwrap();
        directory
    }

    /// Run the fixture CLI and parse each stdout line as an independent JSON record.
    fn run_jsonl(root: &std::path::Path, extra_args: &[&str]) -> Vec<serde_json::Value> {
        let (_, stdout, _) = run_cli(root, extra_args);
        stdout
            .lines()
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("not standalone JSON: {error}\n{line}"))
            })
            .collect()
    }

    /// Borrow records matching the requested JSONL type.
    fn records_of<'a>(lines: &'a [serde_json::Value], kind: &str) -> Vec<&'a serde_json::Value> {
        lines
            .iter()
            .filter(|line| line["type"] == kind)
            .collect::<Vec<_>>()
    }

    /// A row title refused only because its file's Vitest major is not supported yet is marked
    /// on the owned test record; a Vitest 4 file and a title every major renders alike are not.
    #[test]
    fn owned_records_mark_titles_unsupported_on_the_files_major() {
        let unsupported = |fixture: &str| {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures/recall")
                .join(fixture);
            let lines = run_jsonl(&root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
            let tests = records_of(&lines, "test");
            assert_eq!(
                tests.len(),
                29,
                "{fixture}: every collected test has a record"
            );
            tests
                .into_iter()
                .filter(|record| !record["unsupported"].is_null())
                .map(|record| {
                    assert_eq!(record["unsupported"]["feature"], "title-rendering");
                    (
                        record["line"].as_u64().unwrap(),
                        record["unsupported"]["vitest"].as_str().unwrap().to_owned(),
                    )
                })
                .collect::<std::collections::BTreeSet<_>>()
        };
        let on = |vitest: &str, lines: &[u64]| {
            lines
                .iter()
                .map(|&line| (line, vitest.to_owned()))
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(
            unsupported("titles-vitest3"),
            on("3", &[16, 24, 38, 42, 46, 50, 54])
        );
        assert_eq!(unsupported("titles-vitest4"), on("4", &[]));
        assert_eq!(
            unsupported("titles-major-unknown"),
            on("unknown", &[6, 11, 16, 24, 28, 38, 42, 46, 50, 54])
        );
        // An enclosing table suite's refused title marks the tests inside it.
        let root = single_file_project(
            "s.test.ts",
            "import { describe, expect, test } from 'vitest'\n\
             describe.each([['abc']])('over %s %s', (v) => {\n\
               test('inner', () => { expect(v).toHaveLength(3) })\n\
             })\n\
             describe.each([['abc']])('fine %s', (v) => {\n\
               test('inner', () => { expect(v).toHaveLength(3) })\n\
             })\n",
        );
        std::fs::write(
            root.path().join("package.json"),
            r#"{"devDependencies":{"vitest":"3.2.7"}}"#,
        )
        .unwrap();
        let lines = run_jsonl(
            root.path(),
            &["--reporters", "jsonl", "--emit-test-fingerprints"],
        );
        let tests = records_of(&lines, "test");
        assert_eq!(tests.len(), 2);
        assert_eq!(tests[0]["line"], 3);
        assert_eq!(tests[0]["unsupported"]["vitest"], "3");
        assert_eq!(tests[1]["line"], 6);
        assert!(tests[1]["unsupported"].is_null());
    }

    /// The record for the test whose full title is `title`, which must be unique in the run.
    fn record_titled<'a>(lines: &'a [serde_json::Value], title: &str) -> &'a serde_json::Value {
        let matches = records_of(lines, "test")
            .into_iter()
            .filter(|record| record["title"] == title)
            .collect::<Vec<_>>();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one test record titled {title:?}, got {matches:#?}"
        );
        matches[0]
    }

    /// Read a dimension from the selected engine's explicit diagnostic contract.
    fn record_view<'a>(record: &'a serde_json::Value, field: &str) -> &'a serde_json::Value {
        let value = if record["engine"] == "owned" {
            let view = match field {
                "alpha" => "body",
                "structural" => "shape",
                "behavior" => "trace",
                "contextKey" => "context",
                "modifierKey" => "modifiers",
                _ => panic!("unknown diagnostic dimension"),
            };
            &record["views"][view]
        } else {
            &record[field]
        };
        assert!(
            value.is_string(),
            "the requested dimension must be proved: {record}"
        );
        value
    }

    /// Two tests that differ in exactly one literal. `structural` collapses literal values to
    /// their kind, `alpha` keeps them, so this pair is the one that pins the difference between
    /// those two fields being real and both being the same hash under another name.
    const LITERAL_ONLY_DIFFERENCE: &str = r#"
        import { test, expect } from 'vitest';
        import { add } from './math';

        test('adds one and one', () => {
            expect(add(1, 1)).toBe(2);
        });

        test('adds one and two', () => {
            expect(add(1, 2)).toBe(3);
        });
    "#;

    /// Literal-only differences vary the exact body view while preserving the shaped view.
    #[test]
    fn two_tests_differing_only_in_a_literal_share_structural_and_differ_in_alpha() {
        let directory = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let first = record_titled(&lines, "adds one and one");
        let second = record_titled(&lines, "adds one and two");

        assert_eq!(
            record_view(first, "structural"),
            record_view(second, "structural"),
            "literal values are collapsed to their kind in `structural`, so a pair differing \
             only in literals must share it: {first} / {second}"
        );
        assert_ne!(
            record_view(first, "alpha"),
            record_view(second, "alpha"),
            "`alpha` preserves literal values, so this pair must differ in it -- if it does not, \
             `alpha` and `structural` are the same field and a consumer cannot tell a \
             literal-only difference from no difference at all"
        );
        assert_eq!(
            record_view(first, "contextKey"),
            record_view(second, "contextKey"),
            "the pair shares a file, its imports, and its (empty) hook chain"
        );
        assert_eq!(
            record_view(first, "modifierKey"),
            record_view(second, "modifierKey")
        );
        if first["engine"] == "owned" {
            assert_eq!(first["comparable"], true);
        } else {
            assert_eq!(first["trivial"], false);
        }
    }

    /// Two byte-identical bodies under different hook chains. The claim a source-level gate can
    /// only approximate -- "these differ in context and in nothing else" -- is a direct field
    /// comparison here.
    const CONTEXT_ONLY_DIFFERENCE: &str = r#"
        import { test, describe, beforeEach, expect } from 'vitest';
        import { compute, reset } from './machine';

        describe('with setup', () => {
            beforeEach(() => {
                reset();
            });

            test('computes', () => {
                expect(compute()).toBe(3);
            });
        });

        describe('without setup', () => {
            test('computes', () => {
                expect(compute()).toBe(3);
            });
        });
    "#;

    /// Different hook contexts remain visible independently of identical callback bodies.
    #[test]
    fn two_tests_differing_only_in_a_before_each_share_alpha_and_differ_in_context_key() {
        let directory = single_file_project("machine.test.ts", CONTEXT_ONLY_DIFFERENCE);
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let with_setup = record_titled(&lines, "with setup > computes");
        let without_setup = record_titled(&lines, "without setup > computes");

        assert_eq!(
            record_view(with_setup, "alpha"),
            record_view(without_setup, "alpha"),
            "the two bodies are byte-identical: {with_setup} / {without_setup}"
        );
        assert_eq!(
            record_view(with_setup, "structural"),
            record_view(without_setup, "structural")
        );
        assert_eq!(
            record_view(with_setup, "behavior"),
            record_view(without_setup, "behavior")
        );
        assert_ne!(
            record_view(with_setup, "contextKey"),
            record_view(without_setup, "contextKey"),
            "one runs under a `beforeEach` and the other does not; a `contextKey` that could not \
             tell them apart would make an `isolates=context` claim uncheckable, which is the \
             entire reason this field is published"
        );

        // And the analyzer agrees with the records: differing contexts are not a duplicate.
        assert!(
            records_of(&lines, "finding")
                .iter()
                .all(|finding| finding["rule"] != "duplicate-test"),
            "a differing hook chain must suppress `duplicate-test`: {lines:#?}"
        );
    }

    #[test]
    fn tests_that_appear_in_no_finding_still_get_a_record() {
        let (lines, summary) =
            fingerprint_stream_and_summary("machine.test.ts", CONTEXT_ONLY_DIFFERENCE);
        assert_eq!(
            summary["recordCount"], 0,
            "this corpus is deliberately finding-free: {lines:#?}"
        );
        assert_eq!(
            records_of(&lines, "test").len(),
            2,
            "both tests appear in no finding, which is exactly when a consumer has no other way \
             to learn anything about them: {lines:#?}"
        );
    }

    #[test]
    fn record_count_matches_tests_analyzed_and_every_record_precedes_every_finding() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        // Two files, one with findings and one without, plus a `describe` container that must
        // NOT get a record -- `tests_analyzed` counts executable declarations only, and a
        // consumer comparing the two numbers would be misled by a suite record.
        std::fs::write(root.join("math.test.ts"), LITERAL_ONLY_DIFFERENCE).unwrap();
        std::fs::write(root.join("machine.test.ts"), CONTEXT_ONLY_DIFFERENCE).unwrap();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let summary = lines.last().unwrap();
        assert_eq!(
            records_of(&lines, "test").len(),
            summary["testsAnalyzed"].as_u64().unwrap() as usize,
            "one record per analyzed test is the whole contract; a count that disagrees with the \
             summary in the same stream makes it uncheckable: {lines:#?}"
        );

        let last_test = lines.iter().rposition(|line| line["type"] == "test");
        let first_finding = lines.iter().position(|line| line["type"] == "finding");
        assert!(
            !records_of(&lines, "finding").is_empty(),
            "this corpus must produce at least one finding for the ordering claim to mean \
             something: {lines:#?}"
        );
        assert!(
            last_test < first_finding,
            "every test record must precede every finding: {lines:#?}"
        );

        // Order is a property of the code, not of the filesystem walk.
        let again = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        assert_eq!(lines, again, "the stream must be deterministic across runs");
    }

    /// Select an input each engine actually excludes, rather than imposing legacy row policy.
    fn one_refused_record_source() -> &'static str {
        "import { test, expect } from 'vitest'; const shared = () => expect(1).toBe(2); test('unresolved case', shared); test('resolved case', () => expect(1).toBeLessThan(2));"
    }

    /// Published comparability follows the selected engine's recorded exclusion outcome.
    #[test]
    fn a_record_for_an_unresolved_test_carries_comparable_false_and_a_resolved_one_carries_true() {
        let directory = single_file_project("mixed.test.ts", one_refused_record_source());
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let unresolved = record_titled(&lines, "unresolved case");
        let resolved = record_titled(&lines, "resolved case");

        assert_eq!(
            unresolved["comparable"], false,
            "the selected engine excludes this declaration, so the record must not claim \
             comparison eligibility: {unresolved}"
        );
        assert_eq!(
            resolved["comparable"], true,
            "an ordinary test with no unresolved ambient fact must be marked comparable: {resolved}"
        );
    }

    #[test]
    fn the_count_of_non_comparable_records_matches_summary_unresolved_tests() {
        let (lines, summary) =
            fingerprint_stream_and_summary("mixed.test.ts", one_refused_record_source());

        let non_comparable = records_of(&lines, "test")
            .into_iter()
            .filter(|record| record["comparable"] == false)
            .count();
        assert_eq!(
            non_comparable as u64,
            summary["unresolvedTests"].as_u64().unwrap(),
            "a consumer counting `comparable: false` records must land on exactly the summary's \
             `unresolvedTests`, or the two surfaces disagree about how many tests were excluded \
             from comparison: {lines:#?}"
        );
        assert_eq!(
            non_comparable, 1,
            "sanity check that this corpus actually exercises one unresolved test: {lines:#?}"
        );
    }

    /// Removing every optional diagnostic record restores the ordinary stream exactly.
    #[test]
    fn the_flag_is_purely_additive_to_the_stream_it_joins() {
        let ((code_off, stdout_off), (code_on, stdout_on)) =
            jsonl_with_and_without("--emit-test-fingerprints");

        assert!(
            !stdout_off.contains("\"type\":\"test\""),
            "the default stream must not carry a record kind nobody asked for: {stdout_off}"
        );
        let on_without_records: String = stdout_on
            .lines()
            .filter(|line| {
                let value: serde_json::Value = serde_json::from_str(line).unwrap();
                !matches!(value["type"].as_str(), Some("test" | "hook" | "site"))
            })
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(
            on_without_records, stdout_off,
            "with the test records removed, the flagged stream must be byte-identical to the \
             default one -- the flag adds records, it never edits, reorders, or renumbers the \
             ones that were already there"
        );
        assert_eq!(
            code_on, code_off,
            "an output flag must not move the exit code"
        );
    }

    #[test]
    fn the_flag_without_a_jsonl_reporter_is_rejected_rather_than_silently_ignored() {
        let directory = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let root = directory.path();

        for reporters in [
            vec!["--emit-test-fingerprints"],
            vec!["--reporters", "terminal", "--emit-test-fingerprints"],
            vec!["--reporters", "json", "--emit-test-fingerprints"],
        ] {
            let (code, stdout, stderr) = run_cli(root, &reporters);
            assert_eq!(
                code,
                Some(ExitCode::OperationalError.code()),
                "`--emit-test-fingerprints` with no `jsonl` reporter has nowhere to put its \
                 records; producing none of them and exiting 0 is the silent-success failure \
                 this project refuses. args={reporters:?} stdout={stdout} stderr={stderr}"
            );
            assert!(
                stderr.contains("--emit-test-fingerprints"),
                "the message must name the flag that was rejected: {stderr}"
            );
        }
    }

    // =========================================================================================
    // `--debug-memory`'s `"memory"` records.
    //
    // The instrumentation half of a memory fix: the 947 MB -> 149 MB reduction was found and
    // attributed with throwaway instrumentation that was never shipped, so nothing in the tool
    // measured retained bytes at all and the next regression would have to be rediscovered the
    // same way. These pin the contract a budget gate reads, and the two properties that make the
    // number trustworthy: the breakdown accounts for every byte of the total, and the flag adds
    // records without touching the stream it joins.
    // =========================================================================================

    #[test]
    fn the_memory_record_carries_a_breakdown_that_accounts_for_its_own_total() {
        let directory = single_file_project(
            "hooks.test.ts",
            TWO_BEFORE_EACH_HOOKS_DIFFERING_IN_A_LITERAL,
        );
        let root = directory.path();
        std::fs::write(root.join("math.test.ts"), LITERAL_ONLY_DIFFERENCE).unwrap();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--debug-memory"]);
        let records = records_of(&lines, "memory");
        assert_eq!(
            records.len(),
            1,
            "one record per measured phase, and `post-extraction` is the only phase that retains \
             IR: {lines:#?}"
        );
        let record = records[0];
        assert_eq!(record["phase"], "post-extraction");
        assert_eq!(
            record["schemaVersion"],
            vitest_dedup::reporters::JSONL_SCHEMA_VERSION
        );

        let retained = record["retainedBytes"].as_u64().unwrap();
        let tests_analyzed = record["testsAnalyzed"].as_u64().unwrap();
        assert!(
            tests_analyzed >= 4,
            "work-performed floor: a record measured over no tests proves nothing, and a budget \
             read off it would pass forever: {record}"
        );
        assert!(
            retained > 0,
            "the corpus holds titles, paths, and a hook chain; zero retained bytes would mean \
             the accounting is measuring nothing: {record}"
        );

        // `bytesPerTest` is the number a budget gate compares, so it must be exactly the quotient
        // the record's own two other numbers imply -- not a separately derived figure that could
        // drift from them.
        assert_eq!(
            record["bytesPerTest"].as_u64().unwrap(),
            retained / tests_analyzed,
            "bytesPerTest must equal retainedBytes / testsAnalyzed: {record}"
        );

        // The breakdown is what turns "memory grew" into "memory grew *here*". If it does not sum
        // to the total, a consumer cannot tell a field that grew from a field nobody accounted
        // for -- which is the exact failure this instrumentation exists to make impossible.
        let fields = record["fields"].as_object().unwrap();
        for name in [
            "body",
            "context",
            "suitePath",
            "title",
            "modifiers",
            "location",
            "spine",
            "scalars",
        ] {
            assert!(
                fields.contains_key(name),
                "the published breakdown must carry `{name}`: {record}"
            );
        }
        let summed: u64 = fields.values().map(|value| value.as_u64().unwrap()).sum();
        assert_eq!(
            summed, retained,
            "`fields` must be disjoint and exhaustive: {record}"
        );

        // `contextFields` refines `fields.context`, and `engineFields`
        // refines `fields.engine`, rather than adding to them; each carries a hooks bucket.
        let mut hooks = 0;
        for (refinement, parent) in [("contextFields", "context"), ("engineFields", "engine")] {
            let Some(refined) = record[refinement].as_object() else {
                continue;
            };
            let summed: u64 = refined.values().map(|value| value.as_u64().unwrap()).sum();
            assert_eq!(
                summed,
                fields[parent].as_u64().unwrap(),
                "`{refinement}` must sum to `fields.{parent}`, never to something else: {record}"
            );
            hooks += refined["hooks"].as_u64().unwrap();
        }
        assert!(
            hooks > 0,
            "this corpus has two `beforeEach` chains; empty hooks buckets would mean the largest \
             structure the engine retains is invisible to the breakdown: {record}"
        );
    }

    /// The record's counts must be the same counts every other surface of this tool reports, or a
    /// gate reading `bytesPerTest` is dividing by a denominator nothing else agrees with.
    #[test]
    fn the_memory_record_agrees_with_the_summary_about_what_was_analyzed() {
        let directory = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let root = directory.path();
        std::fs::write(root.join("clean.test.ts"), CLEAN_TESTS).unwrap();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--debug-memory"]);
        let record = records_of(&lines, "memory")[0];
        let summary = lines.last().unwrap();
        assert_eq!(summary["type"], "summary");

        assert_eq!(
            record["testsAnalyzed"], summary["testsAnalyzed"],
            "two numbers named `testsAnalyzed` on one stream must be the same number: {lines:#?}"
        );
        assert_eq!(
            record["filesAnalyzed"], summary["filesAnalyzed"],
            "same for `filesAnalyzed`: {lines:#?}"
        );
        assert!(
            record["declarationsRetained"].as_u64().unwrap()
                >= record["testsAnalyzed"].as_u64().unwrap(),
            "every executable declaration is a retained `TestCase`, and suites add more: {record}"
        );
    }

    /// Both notices must ride on the record, because both say how to read a number beside them.
    ///
    /// There is deliberately **no assertion ordering `retainedBytes` against `peakRssBytes`**.
    /// This test used to require retained to be below peak, on the theory that peak bounds it from
    /// above. It does not: `retainedBytes` sums collection *capacity*, and pages that were
    /// reserved and never written are not resident, so a capacity-counted total can legitimately
    /// exceed a high-water mark of resident pages. The ordering held on every corpus measured so
    /// far, which is exactly what made it a latent way to reject a valid measurement rather than a
    /// live failure. `peakRssNotice` is what the record has instead, and asserting it is here is
    /// the point of this test.
    #[test]
    fn the_memory_record_says_how_to_read_peak_rss_and_shared_text() {
        let directory = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--debug-memory"]);
        let record = records_of(&lines, "memory")[0];

        let notice = record["peakRssNotice"].as_str().unwrap();
        assert!(
            notice.contains("high-water mark") && notice.contains("neither direction"),
            "the record must say what peak RSS is, in the record -- a process high-water mark that \
             bounds retained bytes in neither direction: two external reviewers have already read \
             it as though it were the retained figure: {record}"
        );
        assert!(
            record["sharedTextAttribution"].as_str().unwrap().len() > 20,
            "the record must say how shared text is charged: {record}"
        );

        // `null` on a platform with no reader wired up; a number everywhere this project's CI
        // actually runs the binary. Read and type-checked, not ordered against anything.
        assert!(
            record["peakRssBytes"].is_null() || record["peakRssBytes"].as_u64().is_some(),
            "peakRssBytes must be a non-negative integer or null: {record}"
        );
    }

    #[test]
    fn debug_memory_is_purely_additive_to_the_stream_it_joins() {
        let ((code_off, stdout_off), (code_on, stdout_on)) =
            jsonl_with_and_without("--debug-memory");

        assert!(
            !stdout_off.contains("\"type\":\"memory\""),
            "the default stream must not carry a record kind nobody asked for: {stdout_off}"
        );
        let on_without_records: String = stdout_on
            .lines()
            .filter(|line| !line.contains("\"type\":\"memory\""))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(
            on_without_records, stdout_off,
            "with the memory records removed, the flagged stream must be byte-identical to the \
             default one -- the flag adds records, it never edits or reorders the ones that were \
             already there"
        );
        assert_eq!(
            code_on, code_off,
            "an output flag must not move the exit code"
        );
    }

    /// Both stream-riding flags at once: neither may disturb the other's records, and the memory
    /// record comes first so a consumer sees it before the corpus-sized record stream.
    #[test]
    fn debug_memory_and_emit_test_fingerprints_compose() {
        let directory = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let root = directory.path();

        let (_, both, _) = run_cli(
            root,
            &[
                "--reporters",
                "jsonl",
                "--debug-memory",
                "--emit-test-fingerprints",
            ],
        );
        let (_, fingerprints_only, _) =
            run_cli(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);

        let both_without_memory: String = both
            .lines()
            .filter(|line| !line.contains("\"type\":\"memory\""))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(
            both_without_memory, fingerprints_only,
            "each flag's records must be independent of the other's: {both}"
        );
        assert!(
            both.lines().next().unwrap().contains("\"type\":\"memory\""),
            "the memory record describes a phase that is over before any other record could be \
             written, and is emitted first: {both}"
        );
    }

    #[test]
    fn debug_memory_without_a_jsonl_reporter_is_rejected_rather_than_silently_ignored() {
        let directory = single_file_project("math.test.ts", LITERAL_ONLY_DIFFERENCE);
        let root = directory.path();

        for arguments in [
            vec!["--debug-memory"],
            vec!["--reporters", "terminal", "--debug-memory"],
            vec!["--reporters", "json", "--debug-memory"],
            vec!["--reporters", "html,sarif", "--debug-memory"],
        ] {
            let (code, stdout, stderr) = run_cli(root, &arguments);
            assert_eq!(
                code,
                Some(ExitCode::OperationalError.code()),
                "`--debug-memory` with no `jsonl` reporter has nowhere to put its records; \
                 producing none of them and exiting 0 is the silent-success failure this project \
                 refuses. args={arguments:?} stdout={stdout} stderr={stderr}"
            );
            assert!(
                stderr.contains("--debug-memory"),
                "the message must name the flag that was rejected, not its sibling: {stderr}"
            );
        }
    }

    // =========================================================================================
    // `--emit-test-fingerprints`'s `"hook"` records.
    //
    // `redundant-hook` compares hook bodies for alpha-equality but, before this record kind
    // existed, published nothing that let a negative fixture prove its hooks were withheld for
    // the claimed reason (`fixtures/duplicates/not-redundant-hook-different-body.test.ts`'s
    // `unchecked` opt-out). These pin the same two claims that opt-out named as exactly statable:
    // a literal-only difference must still show up at `alpha` while collapsing at `structural`,
    // and two different hook kinds with byte-identical bodies must stay distinguishable.
    // =========================================================================================

    /// Sort borrowed JSONL records by their numeric line field.
    fn sorted_by_line(records: Vec<&serde_json::Value>) -> Vec<&serde_json::Value> {
        let mut records = records;
        records.sort_by_key(|record| record["line"].as_u64().unwrap());
        records
    }

    /// A `beforeEach` repeated one suite down, differing only in the string literal passed to
    /// `connect`. Mirrors `not-redundant-hook-different-body.test.ts`.
    const TWO_BEFORE_EACH_HOOKS_DIFFERING_IN_A_LITERAL: &str = r#"
        import { describe, test, beforeEach, expect } from 'vitest';

        describe('reporting', () => {
            beforeEach(() => {
                connect('primary');
            });

            test('outer', () => {
                expect(1).toBe(1);
            });

            describe('regional', () => {
                beforeEach(() => {
                    connect('replica');
                });

                test('inner', () => {
                    expect(2).toBe(2);
                });
            });
        });
    "#;

    /// Hook body evidence distinguishes literal values independently of effect eligibility.
    #[test]
    fn two_before_each_hooks_differing_only_in_a_literal_share_structural_and_differ_in_alpha() {
        let directory = single_file_project(
            "hooks.test.ts",
            TWO_BEFORE_EACH_HOOKS_DIFFERING_IN_A_LITERAL,
        );
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let hooks = sorted_by_line(records_of(&lines, "hook"));
        assert_eq!(
            hooks.len(),
            2,
            "one record per beforeEach declaration: {lines:#?}"
        );
        let (outer, inner) = (hooks[0], hooks[1]);

        assert_eq!(outer["kind"], "beforeEach");
        assert_eq!(inner["kind"], "beforeEach");
        assert_eq!(
            record_view(outer, "structural"),
            record_view(inner, "structural"),
            "literal values are collapsed to their kind in `structural`, so a pair of hook \
             bodies differing only in the connected replica's name must share it: {outer} / \
             {inner}"
        );
        assert_ne!(
            record_view(outer, "alpha"),
            record_view(inner, "alpha"),
            "`alpha` preserves literal values, so this pair must differ in it -- if it does \
             not, `redundant-hook`'s own alpha-equality check could never distinguish this \
             fixture's claim from a genuine redundancy: {outer} / {inner}"
        );
    }

    /// A `beforeEach` and an `afterEach` with byte-identical bodies.
    const BEFORE_EACH_AND_AFTER_EACH_WITH_IDENTICAL_BODIES: &str = r#"
        import { describe, test, beforeEach, afterEach, expect } from 'vitest';

        describe('reporting', () => {
            beforeEach(() => {
                resetState();
            });

            afterEach(() => {
                resetState();
            });

            test('does the thing', () => {
                expect(1).toBe(1);
            });
        });
    "#;

    /// Identical hook body views do not erase distinct lifecycle phases.
    #[test]
    fn a_before_each_and_an_after_each_with_identical_bodies_stay_distinguishable_by_kind() {
        let directory = single_file_project(
            "hooks.test.ts",
            BEFORE_EACH_AND_AFTER_EACH_WITH_IDENTICAL_BODIES,
        );
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let hooks = records_of(&lines, "hook");
        assert_eq!(
            hooks.len(),
            2,
            "one record per hook declaration: {lines:#?}"
        );
        let before = hooks
            .iter()
            .find(|record| record["kind"] == "beforeEach")
            .expect("a beforeEach record");
        let after = hooks
            .iter()
            .find(|record| record["kind"] == "afterEach")
            .expect("an afterEach record");

        assert_eq!(
            record_view(before, "alpha"),
            record_view(after, "alpha"),
            "the bodies are byte-identical"
        );
        assert_eq!(
            record_view(before, "structural"),
            record_view(after, "structural")
        );
        assert_eq!(
            record_view(before, "behavior"),
            record_view(after, "behavior")
        );
        assert_ne!(
            before["kind"], after["kind"],
            "a beforeEach and an afterEach with byte-identical bodies must remain \
             distinguishable by `kind` -- a record that could not tell them apart would let a \
             consumer treat them as the same claim `redundant-hook` would never make: \
             {before} / {after}"
        );
    }

    #[test]
    fn hook_records_appear_only_with_the_flag_and_precede_every_test_record_and_finding() {
        let directory = single_file_project(
            "hooks.test.ts",
            TWO_BEFORE_EACH_HOOKS_DIFFERING_IN_A_LITERAL,
        );
        let root = directory.path();

        let (_, stdout_off, _) = run_cli(root, &["--reporters", "jsonl"]);
        assert!(
            !stdout_off.contains("\"type\":\"hook\""),
            "the default stream must not carry a record kind nobody asked for: {stdout_off}"
        );

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        assert_eq!(records_of(&lines, "hook").len(), 2, "{lines:#?}");

        let last_hook = lines.iter().rposition(|line| line["type"] == "hook");
        let first_test = lines.iter().position(|line| line["type"] == "test");
        assert!(
            last_hook < first_test,
            "every hook record must precede every test record, which itself precedes every \
             finding: {lines:#?}"
        );
    }

    #[test]
    fn a_hook_record_carries_exactly_its_documented_fields_and_nothing_from_a_test_record() {
        let directory = single_file_project(
            "hooks.test.ts",
            BEFORE_EACH_AND_AFTER_EACH_WITH_IDENTICAL_BODIES,
        );
        let root = directory.path();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);
        let hook = records_of(&lines, "hook")[0];
        let object = hook.as_object().expect("a hook record is a JSON object");
        let mut actual: Vec<&str> = object.keys().map(String::as_str).collect();
        actual.sort_unstable();
        let expected: &[&str] = if hook["engine"] == "owned" {
            &[
                "column",
                "comparisonKeys",
                "contentNotice",
                "engine",
                "kind",
                "line",
                "path",
                "schemaVersion",
                "truncatedFields",
                "type",
                "views",
            ]
        } else {
            &[
                "alpha",
                "behavior",
                "column",
                "contentNotice",
                "kind",
                "line",
                "path",
                "schemaVersion",
                "structural",
                "trivial",
                "truncatedFields",
                "type",
            ]
        };
        assert_eq!(
            actual, expected,
            "a hook record must carry exactly these fields -- no `title`, `comparable`, \
             `contextKey`, or `modifierKey`, none of which is a well-defined fact about a hook \
             body on its own (see `HookFingerprint`'s doc comment for why each is omitted \
             rather than published with an always-the-same value): {hook}"
        );
    }

    /// A nested `beforeEach` whose body is byte-identical to the one above it, which is exactly
    /// what `redundant-hook` fires on -- so one run produces both surfaces that publish a hook
    /// `Location`: the `"hook"` fingerprint records and a `redundant-hook` finding.
    /// The hook body overwrites a module binding with a constant, which the analyzer proves
    /// idempotent; an ordinary call such as `connect('primary')` is unreportable, and a framework
    /// reset is refused, so either would leave the assertions below no finding.
    const TWO_REDUNDANT_BEFORE_EACH_HOOKS: &str = r#"
        import { describe, test, beforeEach, expect } from 'vitest';

        let region = '';

        describe('reporting', () => {
            beforeEach(() => {
                region = 'primary';
            });

            test('outer', () => {
                expect(1).toBe(1);
            });

            describe('regional', () => {
                beforeEach(() => {
                    region = 'primary';
                });

                test('inner', () => {
                    expect(2).toBe(2);
                });
            });
        });
    "#;

    /// Hook locations must be relativized against the analysis root exactly as test locations
    /// are.
    ///
    /// Both surfaces that publish a hook `Location` are asserted in one run, because they share
    /// one root cause and a fix applied to only one of them would leave the other reporting
    /// checkout-specific absolute paths: the `"hook"` records of `--emit-test-fingerprints`
    /// (consumed by the isolation gate, and compared across machines) and the `locations` of a
    /// `redundant-hook` finding (which every reporter prints, and which `Finding::fingerprint`
    /// hashes into a baseline entry).
    ///
    /// The file is written one directory *below* the root so a correct result still carries a
    /// directory component: asserting only on the basename would pass against an implementation
    /// that truncated the path instead of stripping the root prefix.
    #[test]
    fn hook_locations_are_relativized_against_the_analysis_root_like_test_locations() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("suites")).unwrap();
        std::fs::write(
            root.join("suites/hooks.test.ts"),
            TWO_REDUNDANT_BEFORE_EACH_HOOKS,
        )
        .unwrap();

        let lines = run_jsonl(root, &["--reporters", "jsonl", "--emit-test-fingerprints"]);

        let hooks = records_of(&lines, "hook");
        assert_eq!(hooks.len(), 2, "one record per beforeEach: {lines:#?}");
        for hook in &hooks {
            assert_eq!(
                hook["path"], "suites/hooks.test.ts",
                "a hook record's path must be relative to the analysis root, exactly as a test \
                 record's is -- an absolute path here differs on every machine and checkout, \
                 which makes this stream's `\"hook\"` records undiffable for the isolation gate \
                 that consumes them: {hook}"
            );
        }

        // Sibling surface, same `Location`: the finding `emit_redundant_hook` builds directly
        // from `hook.location`.
        let redundant = records_of(&lines, "finding")
            .into_iter()
            .filter(|record| record["rule"] == "redundant-hook")
            .collect::<Vec<_>>();
        assert_eq!(
            redundant.len(),
            1,
            "the nested beforeEach repeats the outer one verbatim, so exactly one \
             `redundant-hook` finding must fire -- without it this test would assert nothing \
             about the finding surface: {lines:#?}"
        );
        let locations = redundant[0]["locations"]
            .as_array()
            .expect("a finding carries an array of locations");
        assert_eq!(locations.len(), 2, "{}", redundant[0]);
        for location in locations {
            assert_eq!(
                location["path"], "suites/hooks.test.ts",
                "a `redundant-hook` finding's locations come straight from `hook.location`, so \
                 they must be relativized too -- otherwise this one rule names an absolute path \
                 where every other rule names a relative one, and its baseline fingerprint \
                 (hashed from these paths) stops matching across checkouts: {}",
                redundant[0]
            );
        }

        // The test records in the same run are the control: whatever form they take, the hook
        // records above must take the identical one.
        for test in records_of(&lines, "test") {
            assert_eq!(
                test["path"], "suites/hooks.test.ts",
                "sanity: test locations were already relativized, and are what the hook \
                 locations above are being held to: {test}"
            );
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Rows specified by the 2026-09-27 tests-implementation plan (S2/S12 blocks)
    // ---------------------------------------------------------------------------------------------

    /// Runs the real binary over `root` and returns its stderr, for the diagnostic-path rows.
    fn run_stderr(root: &std::path::Path, expected: ExitCode) -> String {
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(root)
            .args(["--reporters", "json", "--output"])
            .arg(root.join("report"))
            .output()
            .expect("binary runs");
        assert_eq!(
            output.status.code(),
            Some(expected.code()),
            "the stderr fixture must have the expected process outcome"
        );
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    /// Writes the one file whose analysis carries a diagnostic: a fatal parse error.
    fn write_broken_parse_file(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/broken.test.ts"), "test(: broken {\n").unwrap();
    }

    /// R16 (S2): a run that discovered NO files must be incomplete with a named cause. Before
    /// the fix, the `nothing_read` guard was only reached when files were discovered but yielded
    /// nothing (`tests.is_empty() && !discovered.files.is_empty()`); zero discovered files
    /// cleared every flag and a JSON consumer read a clean, complete pass over nothing -- the
    /// regression where no work is performed but the gate passes.
    #[test]
    fn a_run_that_discovered_no_files_is_incomplete_with_a_named_cause() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("vitest.config.ts"),
            "export default defineConfig({ test: { include: ['nothing/here/**/*.test.ts'] } });",
        )
        .unwrap();
        let prefix = dir.path().join("report");
        let prefix_arg = prefix.to_string_lossy().into_owned();
        let (code, _, stderr) = run_cli(
            dir.path(),
            &["--reporters", "json", "--output", &prefix_arg],
        );
        assert_eq!(code, Some(ExitCode::OperationalError.code()), "{stderr}");
        let report = read_json_report(&prefix);
        assert_eq!(report["incomplete"], serde_json::json!(true));
        assert_eq!(
            report["incompleteCauses"]["nothingRead"],
            serde_json::json!(true),
            "the cause flag must name WHY the run measured nothing: {}",
            report["incompleteCauses"]
        );
    }

    /// T5: discovering a test-shaped file is not evidence that any Vitest declaration was
    /// measured. Competing-runner files are the sharp control: they must remain excluded, and a
    /// corpus containing only them must fail operationally instead of reporting a clean exit.
    #[test]
    fn discovered_but_unextracted_files_exit_operationally() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("native.test.ts"),
            "import { describe, it } from 'node:test';\n\
             describe('native suite', () => { it('native case', () => {}); });",
        )
        .unwrap();
        let prefix = dir.path().join("report");
        let prefix_arg = prefix.to_string_lossy().into_owned();
        let (code, _, stderr) = run_cli(
            dir.path(),
            &["--reporters", "json", "--output", &prefix_arg],
        );

        assert_eq!(code, Some(ExitCode::OperationalError.code()), "{stderr}");
        assert!(stderr.contains("discovered 1 test file(s) but extracted no tests"));
        let report = read_json_report(&prefix);
        assert_eq!(report["testsAnalyzed"], serde_json::json!(0));
        assert_eq!(
            report["incompleteCauses"]["nothingRead"],
            serde_json::json!(true)
        );
    }

    /// A suite container is retained for `duplicate-suite`, but it is not an executable test and
    /// must not turn an empty measurement into a clean quality result.
    #[test]
    fn suite_only_files_exit_operationally() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("suite-only.test.ts"),
            "import { describe } from 'vitest';\n\
             describe('container without children', () => {});",
        )
        .unwrap();
        let prefix = dir.path().join("report");
        let prefix_arg = prefix.to_string_lossy().into_owned();
        let (code, _, stderr) = run_cli(
            dir.path(),
            &["--reporters", "json", "--output", &prefix_arg],
        );

        assert_eq!(code, Some(ExitCode::OperationalError.code()), "{stderr}");
        assert!(
            stderr.contains("only suite containers and no executable tests"),
            "the diagnostic must distinguish a retained container from a test that runs: {stderr}"
        );
        let report = read_json_report(&prefix);
        assert_eq!(report["testsAnalyzed"], serde_json::json!(0));
        assert_eq!(
            report["incompleteCauses"]["nothingRead"],
            serde_json::json!(true)
        );
    }

    /// Negative control for the suite-only case: the same container with one executable child is
    /// a real one-test corpus and must retain the ordinary clean exit.
    #[test]
    fn a_suite_with_an_executable_child_is_analyzed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("suite-with-child.test.ts"),
            "import { describe, test } from 'vitest';\n\
             describe('container', () => { test('child', () => {}); });",
        )
        .unwrap();
        let prefix = dir.path().join("report");
        let prefix_arg = prefix.to_string_lossy().into_owned();
        let (code, _, stderr) = run_cli(
            dir.path(),
            &["--reporters", "json", "--output", &prefix_arg],
        );

        assert_eq!(code, Some(ExitCode::Success.code()), "{stderr}");
        let report = read_json_report(&prefix);
        assert_eq!(report["testsAnalyzed"], serde_json::json!(1));
        assert_eq!(
            report["incompleteCauses"]["nothingRead"],
            serde_json::json!(false)
        );
    }

    /// R26 (S12): stderr diagnostic paths are relativized against the analysis root, like every
    /// `locations[].path` in every report already is. Before the fix, one site was relativized
    /// and its sibling was not: both input routes must preserve the same contract.
    #[test]
    fn stderr_diagnostic_paths_are_relativized_against_the_analysis_root() {
        let dir = tempfile::tempdir().unwrap();
        write_broken_parse_file(dir.path());
        let stderr = run_stderr(dir.path(), ExitCode::OperationalError);
        assert!(
            stderr.contains("src/broken.test.ts"),
            "the diagnostic names its file relative to the root: {stderr}"
        );
        assert!(
            !stderr.contains(dir.path().display().to_string().as_str()),
            "the absolute analysis root must not appear: {stderr}"
        );
    }

    /// R27: the sweep form of R26 -- the property holds over EVERY stderr line, not only the one
    /// site the fix touches. A fix applied to the reported list but not the "could not be fully
    /// analyzed" banner (or the reverse) passes R26 and fails here. The one deliberate
    /// exception is discovery's traversal-error line, which names the analysis root the caller
    /// passed: relativized it would read as a bare `.`, saying nothing.
    #[test]
    fn no_stderr_line_contains_the_analysis_root_prefix() {
        let dir = tempfile::tempdir().unwrap();
        write_broken_parse_file(dir.path());
        // A broken-parse file alone reaches exactly ONE of the sites that print a path, which is
        // how this sweep passed while two others still printed absolute paths: `discovery.rs`'s
        // governance-reason error and `main.rs`'s `includeSource` globals diagnostic, the latter
        // the untouched sibling of a site the same commit relativized. A sweep whose fixture
        // cannot reach a site is not sweeping it. Every shape below exists to reach one more.
        //
        // - an unresolved `test.projects` entry, so every discovered file draws a governance
        //   diagnostic and the project-map banner fires;
        // - a file whose every test has an unresolved context, so the blind-file listing prints;
        // - the broken-parse file above, for the extraction-diagnostic path.
        std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
        std::fs::write(
            dir.path().join("pkg/api.spec.ts"),
            "import { it } from 'vitest'; it('works', () => {});\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/blind.test.ts"),
            "import { it, expect } from 'vitest';\n\
             import { rows } from './rows';\n\
             it.each(rows())('case %s', (v) => { expect(v).toBe(v); });\n",
        )
        .unwrap();
        // An in-source candidate admitted by a project whose own `globals` cannot be read: the
        // site that printed an absolute path while its sibling six lines below was relativized.
        std::fs::write(
            dir.path().join("src/in-source.ts"),
            "export const add = (a: number, b: number) => a + b;\n\
             if (import.meta.vitest) { test('adds', () => { expect(add(1, 1)).toBe(2); }); }\n",
        )
        .unwrap();
        // A helper chain past `MAX_HELPER_EXPANSION_DEPTH`, so S11's refusal diagnostic runs.
        // Without it that site printed an absolute path and this sweep could not see it: the
        // fixture reached no refusal at all. A sweep whose fixture cannot reach a site is not
        // sweeping it -- the same lesson as the four shapes above.
        std::fs::write(dir.path().join("src/deep-helper.test.ts"), {
            let mut lines = vec![
                "import { it, expect } from 'vitest';".to_owned(),
                "h0();".to_owned(),
            ];
            for level in 0..12 {
                lines.push(format!("function h{level}(){{ h{}(); }}", level + 1));
            }
            lines.push("function h12(){ it('deep', () => { expect(1).toBe(1); }); }".to_owned());
            lines.join("\n")
        })
        .unwrap();
        // Oversized, so discovery records a `SkippedFile` and the skipped-diagnostic loop runs.
        // Without this the loop never executes and its relativization is unasserted -- measured:
        // reverting it changed no test.
        std::fs::write(
            dir.path().join("src/huge.test.ts"),
            "x".repeat(discovery::MAX_SOURCE_FILE_BYTES as usize + 1),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("vitest.config.ts"),
            "import { makeProjects } from './projects';\n\
             export default defineConfig({ test: { projects: [makeProjects('other'), \
             { test: { name: 'pkg', globals: someHelper(), \
             includeSource: ['src/**/*.ts'], include: ['pkg/**/*.spec.ts'] } }] } });",
        )
        .unwrap();

        let stderr = run_stderr(dir.path(), ExitCode::Success);
        let root = dir.path().display().to_string();
        let offenders: Vec<&str> = stderr
            .lines()
            .filter(|line| line.contains(root.as_str()))
            .collect();
        assert!(
            offenders.is_empty(),
            "every stderr line must be free of the absolute analysis root: {offenders:?}"
        );

        // A work-performed floor on the sweep itself. Without it, a fixture that stopped
        // producing diagnostics -- a config this scan learns to read, a shape that stops being
        // degraded -- would leave the assertion above true over almost no lines and still green.
        // These are the four shapes the fixture exists to reach.
        // A parse failure's detail, and the helper chain past the depth limit, whose tests are
        // excluded as `registered-elsewhere` rather than printing a diagnostic.
        let (parse_failure, deep_helper) = ("could not be parsed", "registered-elsewhere");
        for expected in [
            "the Vitest project map is incomplete",
            "could not be fully analyzed",
            "no comparable test bodies in",
            parse_failure,
            "exceeds the",
            "includeSource",
            deep_helper,
        ] {
            assert!(
                stderr.contains(expected),
                "the fixture no longer reaches the `{expected}` path, so this sweep is not \
                 sweeping it: {stderr}"
            );
        }
    }

    /// S2's second half, at the boundary where the two outputs meet: an unresolved
    /// `test.projects` entry degrades the run in the JSON report even when every discovered
    /// file attributes to a readable project. Thirteen repositories in the 2026-09-27 scan
    /// printed the "project map is incomplete" warning with `projectConfigDegraded: false`,
    /// because only discovery's fell-through-to-defaults path set the flag.
    #[test]
    fn an_unresolved_projects_entry_degrades_the_json_run_even_when_every_file_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("pkg");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("api.spec.ts"),
            "import { it } from 'vitest'; it('works', () => {});\n",
        )
        .unwrap();
        // Entry ORDER is the whole discrimination, and the first draft of this test got it
        // wrong. With the unreadable entry declared AFTER the project that matches, the file is
        // `IncludeDegraded` and discovery raises `project_config_degraded` on its own -- the test
        // passed identically with the fix reverted, measured. Declared BEFORE, "later wins"
        // settles attribution in `pkg`'s favour, discovery stays silent, and the only thing that
        // knows the map was incomplete is the CLI. Reverted, this run reports
        // `incomplete: false` with every cause clear while stderr prints "the Vitest project map
        // is incomplete" -- the exact disagreement S2's second half is about.
        std::fs::write(
            dir.path().join("vitest.config.ts"),
            "import { makeProjects } from './projects';\nexport default defineConfig({ test: { projects: [makeProjects('other'), { test: { name: 'pkg', include: ['pkg/**/*.spec.ts'] } }] } });",
        )
        .unwrap();
        let report = run_json(dir.path(), &[]);
        assert_eq!(
            report["testsAnalyzed"],
            serde_json::json!(1),
            "precondition: the file still attributes and is analyzed -- this is not a zero-file \
             run wearing a different hat: {report}"
        );
        assert_eq!(
            report["incompleteCauses"]["projectConfigDegraded"],
            serde_json::json!(true),
            "the flag must say what the stderr banner says: {}",
            report["incompleteCauses"]
        );
        assert_eq!(
            report["incomplete"],
            serde_json::json!(true),
            "and the aggregate a CI consumer actually branches on must follow: {report}"
        );
    }
}
