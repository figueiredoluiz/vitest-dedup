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
