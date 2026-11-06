//! Suite ownership, boundaries and delivery behavior.

use super::*;

/// A suite containing child byte offsets in the given range.
fn suite(line: u32, start: u32, end: u32) -> TestCase {
    let mut suite = case(line, start, 90, 0);
    suite.span.end_byte = end;
    suite.title = Some("copy".into());
    suite.full_title = Some("copy".into());
    suite.name = Some(vec!["copy".into()]);
    suite
}

/// Two suites enclosing one equivalent child each.
type OwnedFiles = Vec<(PathBuf, Vec<TestCase>)>;

/// Isolates suite accounting while retaining its error-severity quality gate.
fn suite_only() -> Config {
    config_with(
        &Rule::ALL
            .iter()
            .map(|&rule| {
                (
                    rule,
                    if rule == Rule::DuplicateSuite {
                        Severity::Error
                    } else {
                        Severity::Off
                    },
                )
            })
            .collect::<Vec<_>>(),
    )
}

/// The permanent corpus input retains suite findings without a corpus scan per copy.
#[test]
fn suite_case_corpus_visits_only_matching_files() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = root.join("fixtures/recall/suite-case-file-index/run.test.ts");
    let corpus = crate::corpus::file(
        &path,
        include_str!("../../../fixtures/recall/suite-case-file-index/run.test.ts"),
        crate::corpus::Setting {
            dialect: &crate::framework::vitest::DIALECT,
            globals: false,
            root: &root,
            benchmarks: false,
        },
        &mut crate::corpus::Run::default(),
    )
    .unwrap();
    assert_eq!(corpus.tests.len(), 3);
    assert_eq!(corpus.suites.len(), 2);
    assert!(corpus
        .tests
        .iter()
        .all(|case| case.exclusion.is_none() && case.gate_eligible()));
    assert!(corpus.suites.iter().all(|case| case.exclusion.is_none()));
    for count in [1, 16, 128] {
        let files: OwnedFiles = (0..count)
            .map(|i| file(&format!("{i}/run.test.ts"), corpus.tests.clone()))
            .collect();
        let suites: OwnedFiles = (0..count)
            .map(|i| file(&format!("{i}/run.test.ts"), corpus.suites.clone()))
            .collect();
        crate::rules::suites::DESCENDANT_FILE_VISITS.with(|visits| visits.set(0));
        let result = run_with_suites(&files, &suites, &suite_only(), &[]);
        assert!(!result.budget_exhausted);
        assert_eq!(result.findings.len(), count);
        assert!(result
            .findings
            .iter()
            .all(|finding| finding.rule == Rule::DuplicateSuite && finding.cases.len() == 2));
        assert_eq!(gate(result.findings, false), (true, count * 2));
        let visits = crate::rules::suites::DESCENDANT_FILE_VISITS.with(|visits| visits.get());
        assert_eq!(
            visits,
            count * 2,
            "{count} files must not cause a full-corpus scan per suite copy"
        );
    }
}

/// Equal paths retain every entry and row, but unrelated paths and ineligible cases stay out.
#[test]
fn suite_case_index_preserves_rows_and_file_identity() {
    let (mut files, suites) = population();
    let mut row = case(3, 20, 1, 1);
    row.runtime_row = Some(7);
    let mut unresolved = case(4, 60, 1, 1);
    unresolved.exclusion = Some(Exclusion::UnprovedTableRow);
    let mut benchmark = case(5, 65, 1, 1);
    benchmark.exclusion = Some(Exclusion::Benchmark);
    let mut origin = case(6, 66, 1, 1);
    origin.origin = Some(crate::corpus::Origin {
        path: "helper.ts".into(),
        registration_files: vec!["a.test.ts".into()],
        call: 0,
    });
    let mut bodyless = case(4, 70, 1, 1);
    bodyless.bodyless = true;
    files.push(file(
        "a.test.ts",
        vec![row.clone(), row, unresolved, benchmark, origin, bodyless],
    ));
    files.push(file("other/a.test.ts", vec![case(5, 65, 1, 1)]));
    files.reverse();
    let result = run_with_suites(&files, &suites, &suite_only(), &[]);
    assert_eq!(result.findings.len(), 1);
    let finding = &result.findings[0];
    assert_eq!(finding.cases.len(), 4);
    assert!(finding
        .cases
        .iter()
        .all(|(location, ..)| location.path == std::path::Path::new("a.test.ts")));
    assert!(finding.cases.iter().any(|(_, row, ..)| *row == Some(7)));
    assert_eq!(gate(result.findings, false), (true, 4));
    let missing = run_with_suites(&files[0..1], &suites, &suite_only(), &[]);
    assert_eq!(missing.findings.len(), 1);
    assert!(missing.findings[0].cases.is_empty());
}

/// Builds separate executable and container populations.
fn population() -> (OwnedFiles, OwnedFiles) {
    (
        vec![file(
            "a.test.ts",
            vec![case(3, 20, 1, 1), case(8, 220, 1, 1)],
        )],
        vec![file(
            "a.test.ts",
            vec![suite(2, 10, 100), suite(7, 210, 300)],
        )],
    )
}

/// Three copies A, B and C on lines 2, 7 and 12, each enclosing one child; B's child has key
/// `middle`, the others key 1.
fn three_copies(middle: u64) -> (OwnedFiles, OwnedFiles) {
    (
        vec![file(
            "a.test.ts",
            vec![
                case(3, 20, 1, 1),
                case(8, 220, middle, middle),
                case(13, 420, 1, 1),
            ],
        )],
        vec![file(
            "a.test.ts",
            vec![suite(2, 10, 100), suite(7, 210, 300), suite(12, 410, 500)],
        )],
    )
}

/// Disabled or inline-suppressed suite reporting cannot hide a child relation.
#[test]
fn disabled_or_suppressed_suites_keep_children() {
    let (files, suites) = population();
    let off = config_with(&[(Rule::DuplicateSuite, Severity::Off)]);
    let result = run_with_suites(&files, &suites, &off, &[]);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].rule, Rule::DuplicateTest);
    let suppressions = crate::modes::scan_inline_suppressions(
        "// vitest-dedup-disable-next-line intentional",
        "a.test.ts".into(),
    );
    assert_eq!(suppressions.len(), 1);
    let mut result = run_with_suites(&files, &suites, &config_with(&[]), &suppressions);
    assert_eq!(result.findings.len(), 2);
    assert_eq!(
        crate::modes::apply_inline_suppressions(&mut result.findings, &suppressions),
        1
    );
    assert_eq!(result.findings[0].rule, Rule::DuplicateTest);
}

/// What the exit code and threshold read from `findings`: an Error, and its distinct locations.
fn gate(findings: Vec<Finding>, baselined: bool) -> (bool, usize) {
    let findings = findings
        .into_iter()
        .map(|finding| Finding {
            baselined,
            ..finding
        })
        .collect();
    let finished = crate::analysis::finish_findings(
        findings,
        crate::model::IncompleteCauses::default(),
        crate::analysis::RunModes::default(),
    );
    (finished.had_error_finding, finished.error_cases)
}

/// Reporting a copied block once gates exactly as its child findings would, at their severity.
#[test]
fn a_copied_block_gates_as_its_children() {
    let (files, suites) = population();
    let suites_off = config_with(&[(Rule::DuplicateSuite, Severity::Off)]);
    for (child, carried) in [
        (Severity::Error, Some(Severity::Error)),
        (Severity::Warning, Some(Severity::Warning)),
        (Severity::Off, None),
    ] {
        let config = config_with(&[(Rule::DuplicateTest, child)]);
        let result = run_with_suites(&files, &suites, &config, &[]);
        assert_eq!(result.findings.len(), 1, "{child:?}");
        let suite = &result.findings[0];
        assert_eq!(suite.rule, Rule::DuplicateSuite);
        assert_eq!(suite.severity, Severity::Warning);
        assert_eq!(suite.covers.as_ref().map(|covers| covers.severity), carried);
        if carried.is_some() {
            assert_eq!(suite.covers.as_ref().unwrap().locations.len(), 2);
        }
        let children = run_with_suites(
            &files,
            &suites,
            &config_with(&[
                (Rule::DuplicateSuite, Severity::Off),
                (Rule::DuplicateTest, child),
            ]),
            &[],
        );
        assert_eq!(
            gate(result.findings.clone(), false),
            gate(children.findings, false),
            "{child:?}"
        );
        assert_eq!(gate(result.findings, true), (false, 0), "{child:?}");
    }
    let flat = run_with_suites(&files, &suites, &suites_off, &[]);
    assert_eq!(gate(flat.findings, false), (true, 2));
}

/// Covers keep only the highest severity's locations, whichever child arrives first.
#[test]
fn carried_children_keep_the_highest_severity() {
    let (files, suites) = population();
    let mut suite = run_with_suites(&files, &suites, &config_with(&[]), &[]).findings[0].clone();
    suite.covers = None;
    let at = |line| {
        let mut location = suite.locations[0].clone();
        location.span.start.line = line;
        vec![(location, None, 0)]
    };
    let (one, two, three) = (at(1), at(2), at(3));
    crate::rules::suites::carry(&mut suite, Severity::Warning, one);
    crate::rules::suites::carry(&mut suite, Severity::Error, two.clone());
    crate::rules::suites::carry(&mut suite, Severity::Warning, three);
    crate::rules::suites::carry(&mut suite, Severity::Error, two);
    let covers = suite.covers.unwrap();
    assert_eq!(covers.severity, Severity::Error);
    assert_eq!(
        covers
            .locations
            .iter()
            .map(|location| location.span.start.line)
            .collect::<Vec<_>>(),
        [2]
    );
}

/// An Error suite carrying Error children counts their tests, not its blocks on top of them.
#[test]
fn an_error_suite_counts_only_the_tests_it_covers() {
    let (files, suites) = population();
    let both = config_with(&[(Rule::DuplicateSuite, Severity::Error)]);
    let result = run_with_suites(&files, &suites, &both, &[]);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(gate(result.findings, false), (true, 2));
}

/// Nested copies count each test once: the outer suite carries what the inner suite carries
/// instead of adding its own blocks as tests.
#[test]
fn nested_copies_count_each_test_once() {
    let outer = |line, start, end| {
        let mut outer = suite(line, start, end);
        outer.key = 91;
        outer.name = Some(vec!["outer".into()]);
        outer
    };
    let files = vec![file(
        "a.test.ts",
        vec![case(4, 30, 1, 1), case(10, 230, 1, 1)],
    )];
    let suites = vec![file(
        "a.test.ts",
        vec![
            outer(2, 10, 200),
            suite(3, 20, 100),
            outer(8, 210, 400),
            suite(9, 220, 300),
        ],
    )];
    let error = config_with(&[(Rule::DuplicateSuite, Severity::Error)]);
    let result = run_with_suites(&files, &suites, &error, &[]);
    assert_eq!(result.findings.len(), 2, "{:?}", result.findings);
    for finding in &result.findings {
        let carried: Vec<u32> = finding
            .covers
            .as_ref()
            .unwrap()
            .locations
            .iter()
            .map(|l| l.span.start.line)
            .collect();
        assert_eq!(carried, [4, 10], "{finding:?}");
    }
    assert_eq!(gate(result.findings, false), (true, 2));
}

/// Each suite finding carries only the child tests inside the copies it names.
#[test]
fn a_suite_finding_carries_only_its_own_copies() {
    let (files, suites) = three_copies(1);
    let result = run_with_suites(&files, &suites, &config_with(&[]), &[]);
    let pairs: Vec<_> = result
        .findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateSuite)
        .collect();
    assert_eq!(pairs.len(), 2);
    for pair in pairs {
        let copies: Vec<u32> = pair.locations.iter().map(|l| l.span.start.line).collect();
        let carried: Vec<u32> = pair
            .covers
            .as_ref()
            .unwrap()
            .locations
            .iter()
            .map(|l| l.span.start.line - 1)
            .collect();
        assert_eq!(carried, copies, "{pair:?}");
    }
}

/// A child relation gates with the one suite finding that stands for it: baselining A–B accepts
/// A–B's children, and suppressing C's child leaves B–C nothing to carry.
#[test]
fn a_child_relation_gates_with_the_suite_finding_that_stands_for_it() {
    let (files, suites) = three_copies(1);
    let suppressions = crate::modes::scan_inline_suppressions(
        &format!(
            "{}// vitest-dedup-disable-next-line intentional",
            "\n".repeat(11)
        ),
        "a.test.ts".into(),
    );
    assert_eq!(suppressions.len(), 1);
    let gate_with = |baseline_first: bool| {
        let mut findings =
            run_with_suites(&files, &suites, &config_with(&[]), &suppressions).findings;
        assert_eq!(findings.len(), 2, "{findings:?}");
        if baseline_first {
            let first = findings
                .iter_mut()
                .find(|f| f.locations[0].span.start.line == 2)
                .unwrap();
            first.baselined = true;
        }
        let finished = crate::analysis::finish_findings(
            findings,
            crate::model::IncompleteCauses::default(),
            crate::analysis::RunModes {
                suppressions: &suppressions,
                ..Default::default()
            },
        );
        (finished.had_error_finding, finished.error_cases)
    };
    assert_eq!(gate_with(true), (false, 0));
    assert_eq!(gate_with(false), (true, 2));
}

/// A child relation across copies that no single suite finding names stays visible.
#[test]
fn a_child_relation_no_suite_finding_names_stays_visible() {
    let (files, suites) = three_copies(2);
    let result = run_with_suites(&files, &suites, &config_with(&[]), &[]);
    let child = result
        .findings
        .iter()
        .find(|f| f.rule == Rule::DuplicateTest)
        .expect("A and C's children are reported, since neither A–B nor B–C stands for them");
    assert_eq!(
        child
            .locations
            .iter()
            .map(|l| l.span.start.line)
            .collect::<Vec<_>>(),
        [3, 13]
    );
    assert!(result.findings.iter().all(|f| f.covers.is_none()));
}

/// A child the user suppressed inline would not have gated, so the suite does not carry it.
#[test]
fn an_inline_suppressed_child_is_not_carried() {
    let (files, suites) = population();
    let suppressions = crate::modes::scan_inline_suppressions(
        "\n// vitest-dedup-disable-next-line intentional",
        "a.test.ts".into(),
    );
    assert_eq!(suppressions.len(), 1);
    let result = run_with_suites(&files, &suites, &config_with(&[]), &suppressions);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].rule, Rule::DuplicateSuite);
    assert_eq!(result.findings[0].covers, None);
    let carried = run_with_suites(&files, &suites, &config_with(&[]), &[]);
    assert!(carried.findings[0].covers.is_some());
}

/// Outsiders, local duplicate pairs and cross-file relationships keep their evidence.
#[test]
fn suite_suppression_keeps_uncovered_relationships() {
    let (mut files, suites) = population();
    files[0].1.insert(1, case(4, 60, 1, 1));
    files[0].1.push(case(9, 260, 1, 1));
    let result = run_with_suites(&files, &suites, &config_with(&[]), &[]);
    assert_eq!(
        result
            .findings
            .iter()
            .filter(|f| f.rule == Rule::DuplicateTest)
            .count(),
        2
    );
    files[0].1.push(case(20, 400, 1, 1));
    files.push(file("b.test.ts", vec![case(1, 10, 1, 1)]));
    let result = run_with_suites(&files, &suites, &config_with(&[]), &[]);
    let local = result
        .findings
        .iter()
        .find(|f| f.rule == Rule::DuplicateTest)
        .unwrap();
    assert_eq!(
        local.locations.len(),
        5,
        "mixed cluster must retain outsider and local evidence"
    );
    assert_eq!(
        result
            .findings
            .iter()
            .filter(|f| f.rule == Rule::CrossFileDuplicate)
            .count(),
        1
    );
}

/// Suite groups charge spanning edges, honor severity, and never count a non-emitted group.
#[test]
fn suite_budget_clusters_and_severity_are_shared() {
    for size in [2, 3, 6] {
        let suites = vec![file(
            "a.test.ts",
            (0..size)
                .map(|i| suite(i + 1, 100 * i, 100 * i + 80))
                .collect(),
        )];
        let config = config_with(&[(Rule::DuplicateSuite, Severity::Error)]);
        let short = run_with_suites_and_budget(&[], &suites, &config, &[], (size - 2) as u64);
        assert!(short.budget_exhausted);
        assert!(short.findings.is_empty());
        let result = run_with_suites_and_budget(&[], &suites, &config, &[], (size - 1) as u64);
        assert!(!result.budget_exhausted);
        assert_eq!(
            result.findings.len(),
            if size > 4 { 1 } else { (size - 1) as usize }
        );
        assert!(result
            .findings
            .iter()
            .all(|f| f.rule == Rule::DuplicateSuite && f.severity == Severity::Error));
    }
}

/// A matching key in another file is not a same-file suite copy.
#[test]
fn suite_keys_cannot_cross_files_or_exclusions() {
    let suites = vec![
        file("a.test.ts", vec![suite(1, 0, 100)]),
        file("b.test.ts", vec![suite(1, 0, 100)]),
    ];
    assert!(run_with_suites(&[], &suites, &config_with(&[]), &[])
        .findings
        .is_empty());
    for excluded in [true, false] {
        let (_, mut suites) = population();
        if excluded {
            suites[0].1[0].exclusion = Some(Exclusion::OrderDependentState);
        } else {
            suites[0].1[0].inert = true;
        }
        assert!(run_with_suites(&[], &suites, &config_with(&[]), &[])
            .findings
            .is_empty());
    }
}
