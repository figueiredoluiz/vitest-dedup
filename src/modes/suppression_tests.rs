use super::*;
use crate::model::{Location, Position, Rule, Severity, Span};

/// A one-participant location at `path:line`, column 3, for the suppression tests.
fn location(path: &str, line: u32) -> Location {
    Location {
        path: PathBuf::from(path),
        span: Span {
            start: Position { line, column: 3 },
            end: Position { line, column: 8 },
            start_byte: 0,
            end_byte: 5,
        },
    }
}

/// An Error-severity `duplicate-test` finding with the single participant [`location`]
/// builds.
fn finding(path: &str, line: u32) -> Finding {
    Finding {
        identity: Default::default(),
        registration_files: Vec::new(),
        rule: Rule::DuplicateTest,
        severity: Severity::Error,
        message: "m".to_string(),
        locations: vec![location(path, line)],
        titles: vec!["t".to_string()],
        inert: vec![false],
        baselined: false,
    }
}

/// The directive on line 4 suppresses a participant on line 5, and nothing else: a
/// participant on line 6, in another file, or on the directive's own line survives.
#[test]
fn a_directive_suppresses_exactly_the_next_line_of_its_file() {
    let suppressions = scan_inline_suppressions(
        "a\nb\nc\n// vitest-dedup-disable-next-line this pair is intentional\nx\ny\nz",
        PathBuf::from("src/a.test.ts"),
    );
    assert_eq!(suppressions.len(), 1);
    assert_eq!(suppressions[0].path, PathBuf::from("src/a.test.ts"));
    assert_eq!(suppressions[0].line, 4);
    assert_eq!(suppressions[0].reason, "this pair is intentional");

    let mut findings = vec![
        finding("src/a.test.ts", 5),
        finding("src/a.test.ts", 6),
        finding("src/other.test.ts", 5),
    ];
    let removed = apply_inline_suppressions(&mut findings, &suppressions);
    assert_eq!(removed, 1);
    assert_eq!(findings.len(), 2);
    assert_eq!(findings[0].locations[0].span.start.line, 6);
    assert_eq!(
        findings[0].locations[0].path,
        PathBuf::from("src/a.test.ts")
    );
}

/// A finding is suppressed when ANY participant carries the directive: the author wrote it
/// at one test's declaration, and the finding is about that test's relationship.
#[test]
fn any_suppressed_participant_suppresses_the_whole_finding() {
    let suppressions =
        scan_inline_suppressions("// vitest-dedup-disable-next-line", PathBuf::from("b.ts"));
    let mut finding = finding("a.ts", 2);
    finding.locations.push(location("b.ts", 2));
    let mut findings = vec![finding];
    assert_eq!(apply_inline_suppressions(&mut findings, &suppressions), 1);
    assert!(findings.is_empty());
}

/// Empty input, no directives, no findings: each is a no-op that reports zero, never an
/// error -- the common case (no directives anywhere) must cost nothing.
#[test]
fn empty_inputs_are_no_ops_reporting_zero() {
    assert!(scan_inline_suppressions("", PathBuf::from("a.ts")).is_empty());
    assert!(scan_inline_suppressions(
        "const x = 1;\ntest('no directives here', () => {});",
        PathBuf::from("a.ts")
    )
    .is_empty());
    let mut findings = vec![finding("a.ts", 2)];
    assert_eq!(apply_inline_suppressions(&mut findings, &[]), 0);
    assert_eq!(findings.len(), 1);
}

/// The precedence claim, pinned through the real baseline file format: a finding suppressed
/// by a directive never lands in a `--update-baseline` file, because the suppression runs
/// before the result is finalized -- a repository that suppresses one pair and baselines
/// another records only the other. Suppression is "not debt"; a baseline file that contained
/// it would misstate the repository.
#[test]
fn a_suppressed_finding_is_never_recorded_by_the_baseline() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("baseline.json");

    let suppressions = scan_inline_suppressions(
        "// vitest-dedup-disable-next-line\nconst x = 1;",
        PathBuf::from("a.ts"),
    );
    let mut findings = vec![finding("a.ts", 2), finding("a.ts", 9)];
    apply_inline_suppressions(&mut findings, &suppressions);
    assert_eq!(findings.len(), 1, "the directive-covered participant went");

    write_baseline(&path, &findings).unwrap();
    let baseline = load_baseline(&path).unwrap();
    assert_eq!(
        baseline.total(),
        1,
        "only the unsuppressed finding is recorded"
    );

    // And on the next run the same suppressed finding is suppressed again before the
    // baseline is applied, so the recorded allowance still matches exactly what is new.
    let mut next_run = vec![finding("a.ts", 2), finding("a.ts", 9)];
    apply_inline_suppressions(&mut next_run, &suppressions);
    let (baselined, new_findings) = apply_baseline(&mut next_run, &baseline);
    assert_eq!((baselined, new_findings), (1, 0));
}
