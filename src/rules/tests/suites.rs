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
