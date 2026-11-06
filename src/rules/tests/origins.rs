//! Source identity, caller scope and collapsed caller provenance are independent dimensions.
use super::*;
use crate::corpus::Origin;
use std::collections::BTreeSet;

/// One comparable declaration whose execution is owned by a caller.
fn imported(source: &str, owner: &str, line: u32) -> TestCase {
    let mut case = case(line, line * 100, 1, 1);
    case.origin = Some(Origin {
        path: PathBuf::from(source),
        registration_files: vec![PathBuf::from(owner)],
        call: 0,
    });
    case
}

/// Restrict observations to one rule while retaining the same owned case population.
fn claims(files: Vec<(PathBuf, Vec<TestCase>)>, rule: Rule) -> Vec<Finding> {
    run(&files, &config_with(&[(rule, Severity::Warning)]))
        .findings
        .into_iter()
        .filter(|finding| finding.rule == rule)
        .collect()
}

/// Repeated executions never become a cross-file source duplicate.
#[test]
fn one_shared_source_cannot_pair_with_itself() {
    let files = vec![
        file("a.test.ts", vec![imported("runner.ts", "a.test.ts", 1)]),
        file("b.test.ts", vec![imported("runner.ts", "b.test.ts", 1)]),
    ];
    assert_eq!(files.iter().map(|(_, cases)| cases.len()).sum::<usize>(), 2);
    assert!(claims(files, Rule::CrossFileDuplicate).is_empty());
}

/// A shared declaration may need its second caller to witness a distinct-source cross-file pair.
#[test]
fn shared_sources_preserve_witnesses_and_nonrepresentative_changed_callers() {
    let files = vec![
        file(
            "a.test.ts",
            vec![
                imported("a-runner.ts", "a.test.ts", 1),
                imported("b-runner.ts", "a.test.ts", 2),
            ],
        ),
        file("b.test.ts", vec![imported("a-runner.ts", "b.test.ts", 1)]),
    ];
    assert_eq!(files.iter().map(|(_, cases)| cases.len()).sum::<usize>(), 3);
    let findings = claims(files, Rule::CrossFileDuplicate);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].locations[0].path, PathBuf::from("a-runner.ts"));
    assert_eq!(findings[0].locations[1].path, PathBuf::from("b-runner.ts"));
    assert_eq!(
        findings[0].registration_files[0],
        [PathBuf::from("a.test.ts"), PathBuf::from("b.test.ts")]
    );
    for path in ["a.test.ts", "b.test.ts", "a-runner.ts", "unrelated.ts"] {
        let mut scoped = findings.clone();
        let removed = crate::modes::retain_changed_findings(
            &mut scoped,
            &BTreeSet::from([PathBuf::from(path)]),
        );
        assert_eq!(removed, usize::from(path == "unrelated.ts"), "{path}");
    }
}

/// Equal byte offsets in distinct modules remain distinct declarations in every source-based guard.
#[test]
fn equal_offsets_in_two_helpers_still_compare() {
    let files = vec![file(
        "a.test.ts",
        vec![
            imported("first.ts", "a.test.ts", 1),
            imported("second.ts", "a.test.ts", 1),
        ],
    )];
    assert_eq!(files[0].1.len(), 2);
    let findings = claims(files, Rule::DuplicateTest);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_ne!(findings[0].locations[0].path, findings[0].locations[1].path);
}

/// Changing the recorded comparison key, while retaining origins and eligibility, keeps silence.
#[test]
fn origins_do_not_erase_conflicting_module_keys() {
    let left = imported("first.ts", "a.test.ts", 1);
    let mut right = imported("second.ts", "b.test.ts", 1);
    right.module_key = 2;
    assert!(left.exclusion.is_none() && right.exclusion.is_none());
    assert!(claims(
        vec![
            file("a.test.ts", vec![left]),
            file("b.test.ts", vec![right])
        ],
        Rule::CrossFileDuplicate
    )
    .is_empty());
}

/// One row of a runner's table, registered by `owner`.
fn imported_row(owner: &str, row: usize) -> TestCase {
    let mut case = imported("runner.ts", owner, 5);
    case.row = Some(row);
    case.row_class = Some(0);
    case
}

/// A one-row table reached by two calls in one file is one row twice, not two equal rows; two equal
/// rows reached by the same two calls stay one row finding.
#[test]
fn a_table_row_reached_by_two_calls_is_not_its_own_duplicate() {
    let once = vec![imported_row("a.test.ts", 0), imported_row("a.test.ts", 0)];
    assert!(once.iter().all(|case| case.exclusion.is_none()));
    assert!(claims(vec![file("a.test.ts", once)], Rule::DuplicateEachRow).is_empty());
    let twice = vec![
        imported_row("a.test.ts", 0),
        imported_row("a.test.ts", 1),
        imported_row("a.test.ts", 0),
        imported_row("a.test.ts", 1),
    ];
    let findings = claims(vec![file("a.test.ts", twice)], Rule::DuplicateEachRow);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].locations.len(), 2, "{findings:?}");
}

/// Two local tables whose rows pair up row by row make one claim per declaration pair; with an
/// imported case elsewhere in the run, the second row pair merges its callers into that claim.
#[test]
fn row_pairs_of_two_tables_are_one_claim_beside_imported_cases() {
    let row = |site: u32, row: usize, key: u64| {
        let mut case = case(site, site, key, 1);
        case.row = Some(row);
        case
    };
    let files = vec![
        file(
            "a.test.ts",
            vec![row(10, 0, 1), row(10, 1, 2), row(20, 0, 1), row(20, 1, 2)],
        ),
        file("b.test.ts", vec![imported("runner.ts", "b.test.ts", 30)]),
    ];
    let findings = claims(files, Rule::DuplicateTest);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].locations.len(), 2, "{findings:?}");
}
