//! Each rule's claim and each refusal, as finding outcomes over retained corpus cases.

use super::*;
use crate::config::{Config, ConfigOverrides};
use crate::corpus::Exclusion;
use crate::model::{Position, Rule, Severity, Span};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A case at `line` with the keys and flags given; rows of one table share `site`.
fn case(line: u32, site: u32, key: u64, module_key: u64) -> TestCase {
    TestCase {
        span: Span {
            start: Position { line, column: 3 },
            end: Position { line, column: 40 },
            start_byte: site,
            end_byte: site + 30,
        },
        title: Some(format!("t{line}")),
        full_title: Some(format!("t{line}")),
        name: Some(vec![format!("t{line}")]),
        row: None,
        row_class: None,
        key,
        module_key,
        body: 0,
        parameterization_key: None,
        trace: Vec::new(),
        trace_complete: true,
        similarity_key: 0,
        subsumption_key: 0,
        callback_form: crate::normal::CallbackForm::Interchangeable,
        inert: false,
        exclusion: None,
    }
}

/// One file's population: its relative path and the cases given.
fn file(path: &str, cases: Vec<TestCase>) -> (PathBuf, Vec<TestCase>) {
    (PathBuf::from(path), cases)
}

/// A config carrying the given rules, or the built-in defaults.
fn config_with(rules: &[(Rule, Severity)]) -> Config {
    Config {
        root: PathBuf::from("."),
        include: Vec::new(),
        include_is_explicit: false,
        exclude: Vec::new(),
        rules: rules.iter().copied().collect::<BTreeMap<_, _>>(),
        threshold: 100,
        source: "test".to_owned(),
    }
}

/// The `(rule, participant paths in order)` of every finding.
fn outline(outcome: &Outcome) -> Vec<(Rule, Vec<String>)> {
    outcome
        .findings
        .iter()
        .map(|finding| {
            (
                finding.rule,
                finding
                    .locations
                    .iter()
                    .map(|location| location.path.display().to_string())
                    .collect(),
            )
        })
        .collect()
}

/// Two compared cases of one file sharing a same-file key.
fn same_file_pair() -> [(PathBuf, Vec<TestCase>); 1] {
    [file(
        "a.test.ts",
        vec![case(5, 100, 1, 1), case(9, 200, 1, 1)],
    )]
}

/// One compared case in each of two files, sharing a key.
fn cross_file_pair() -> [(PathBuf, Vec<TestCase>); 2] {
    [
        file("a.test.ts", vec![case(5, 100, 1, 1)]),
        file("b.test.ts", vec![case(7, 100, 1, 1)]),
    ]
}

/// Two compared cases of one file sharing a same-file key are one finding, participants in order.
#[test]
fn a_same_file_pair_is_one_duplicate_test_finding() {
    let files = same_file_pair();
    let outcome = run(&files, &config_with(&[]));
    assert_eq!(
        outline(&outcome),
        vec![(
            Rule::DuplicateTest,
            vec!["a.test.ts".to_owned(), "a.test.ts".to_owned()]
        )],
        "the pair shares one same-file key and is reported once, participants in order"
    );
    let finding = &outcome.findings[0];
    assert_eq!(finding.titles, vec!["t5", "t9"]);
    assert_eq!(finding.inert, vec![false, false]);
    assert!(finding.message.contains("2 tests have identical bodies"));
}

/// The same key in two files is cross-file only: duplicate-test groups by file and key.
#[test]
fn a_relationship_across_files_is_cross_file_and_never_duplicate_test() {
    let files = cross_file_pair();
    let outcome = run(&files, &config_with(&[]));
    assert_eq!(
        outline(&outcome),
        vec![(
            Rule::CrossFileDuplicate,
            vec!["a.test.ts".to_owned(), "b.test.ts".to_owned()]
        )]
    );
    assert!(
        outcome.findings[0].message.contains("across 2 files"),
        "the message names how many files the finding spans: {}",
        outcome.findings[0].message
    );
}

/// A differing same-file key and a differing module key are both silences.
#[test]
fn differing_keys_stay_silent() {
    let files = [
        file("a.test.ts", vec![case(5, 100, 1, 1), case(9, 200, 2, 1)]),
        file("b.test.ts", vec![case(7, 100, 1, 2)]),
    ];
    let outcome = run(&files, &config_with(&[]));
    assert!(
        outcome.findings.is_empty(),
        "a differing same-file key and a differing module key are both silences"
    );
}

/// Every exclusion reason keeps the case out of every group, whichever rule would claim it.
#[test]
fn an_excluded_case_never_anchors_a_finding() {
    for exclusion in [
        Exclusion::RegisteredInsideFunction,
        Exclusion::EmptyBody,
        Exclusion::NoCallback,
        Exclusion::OrderDependentState,
        Exclusion::NamedCallback,
        Exclusion::RegisteredElsewhere,
        Exclusion::Benchmark,
    ] {
        let mut excluded = case(9, 200, 1, 1);
        excluded.exclusion = Some(exclusion);
        let files = [file("a.test.ts", vec![case(5, 100, 1, 1), excluded])];
        let outcome = run(&files, &config_with(&[]));
        assert!(
            outcome.findings.is_empty(),
            "{exclusion:?} must keep the case out of every group"
        );
    }
}

/// Two rows sharing their registering call collapse to one representative, never a self-pair.
#[test]
fn duplicate_rows_of_one_table_are_one_declaration_not_a_pair() {
    let mut first = case(5, 100, 1, 1);
    first.row = Some(0);
    let mut second = case(5, 100, 1, 1);
    second.row = Some(1);
    let files = [file("a.test.ts", vec![first, second])];
    let outcome = run(&files, &config_with(&[]));
    assert!(
        outcome.findings.is_empty(),
        "two rows of one table share their registering call: their relationship is \
         duplicate-each-row's claim, not this rule's"
    );
}

/// One row beside a spelled test with the same key is one ordinary pair.
#[test]
fn one_row_of_a_table_pairs_with_a_spelled_test() {
    let mut row = case(5, 100, 1, 1);
    row.row = Some(0);
    let files = [file("a.test.ts", vec![row, case(9, 200, 1, 1)])];
    let outcome = run(&files, &config_with(&[]));
    assert_eq!(outline(&outcome).len(), 1, "one declaration each, one pair");
}

/// A group of three reports consecutive pairs, not the full cross product.
#[test]
fn three_members_become_a_spanning_set_of_pairs() {
    let files = [file(
        "a.test.ts",
        vec![case(3, 100, 1, 1), case(7, 200, 1, 1), case(11, 300, 1, 1)],
    )];
    let outcome = run(&files, &config_with(&[]));
    assert_eq!(
        outcome.findings.len(),
        2,
        "consecutive pairs, not n*(n-1)/2"
    );
    assert_eq!(outcome.findings[0].titles, vec!["t3", "t7"]);
    assert_eq!(outcome.findings[1].titles, vec!["t7", "t11"]);
}

/// A group of five reports one cluster finding naming every member.
#[test]
fn a_group_past_the_spanning_limit_is_one_cluster() {
    let cases: Vec<TestCase> = (1..=5)
        .map(|line| case(line * 4, line * 100, 1, 1))
        .collect();
    let files = [file("a.test.ts", cases)];
    let outcome = run(&files, &config_with(&[]));
    assert_eq!(outcome.findings.len(), 1, "one cluster finding");
    let finding = &outcome.findings[0];
    assert_eq!(finding.locations.len(), 5);
    assert!(
        finding.message.contains("form one cluster"),
        "the cluster names what it replaces: {}",
        finding.message
    );
}

/// A skip participant is marked inert and named in the message.
#[test]
fn inert_participants_are_marked_and_named() {
    let mut skipped = case(5, 100, 1, 1);
    skipped.inert = true;
    skipped.full_title = Some("outer > t5".to_owned());
    let files = [file("a.test.ts", vec![skipped, case(9, 200, 1, 1)])];
    let outcome = run(&files, &config_with(&[]));
    assert_eq!(outcome.findings[0].inert, vec![true, false]);
    assert!(
        outcome.findings[0]
            .message
            .contains("inert (skip/todo) participant(s), still counted as duplicates: outer > t5"),
        "{}",
        outcome.findings[0].message
    );
}

/// A case the source gives no full title is named by where it is, never by an invented title, in
/// the titles and in the message that calls out an inert participant.
#[test]
fn a_participant_without_a_spelled_title_is_named_by_its_line() {
    let mut dynamic = case(5, 100, 1, 1);
    dynamic.full_title = None;
    dynamic.inert = true;
    let files = [file("a.test.ts", vec![dynamic, case(9, 200, 1, 1)])];
    let outcome = run(&files, &config_with(&[]));
    let finding = &outcome.findings[0];
    assert_eq!(finding.titles, vec!["<dynamic title at line 5>", "t9"]);
    assert!(
        finding
            .message
            .ends_with("still counted as duplicates: <dynamic title at line 5>"),
        "{}",
        finding.message
    );
}

/// Two copied tables pair row by row, and every row pair is the same claim about the same two
/// calls: one finding, in either rule. A row pair over other declarations stays its own.
#[test]
fn copied_tables_are_one_finding_however_many_rows_they_share() {
    let row = |line, site, key, module_key, index| TestCase {
        row: Some(index),
        ..case(line, site, key, module_key)
    };
    let files = [file(
        "a.test.ts",
        vec![
            row(5, 100, 1, 11, 0),
            row(5, 100, 2, 12, 1),
            row(5, 100, 3, 13, 2),
            row(9, 200, 1, 11, 0),
            row(9, 200, 2, 12, 1),
            row(9, 200, 3, 13, 2),
            case(12, 300, 3, 13),
        ],
    )];
    let outcome = run(&files, &config_with(&[]));
    let pairs: Vec<Vec<u32>> = outcome
        .findings
        .iter()
        .map(|finding| {
            finding
                .locations
                .iter()
                .map(|location| location.span.start.line)
                .collect()
        })
        .collect();
    assert_eq!(pairs, vec![vec![5, 9], vec![9, 12]]);
}

/// Five copied tables are one cluster, however many rows repeat it.
#[test]
fn copied_tables_in_a_cluster_are_one_finding() {
    let cases = (0..5)
        .flat_map(|table: u32| {
            (0..2).map(move |row: u32| TestCase {
                row: Some(row as usize),
                ..case(table + 1, table * 100, u64::from(row) + 1, 50)
            })
        })
        .collect();
    let outcome = run(&[file("a.test.ts", cases)], &config_with(&[]));
    assert_eq!(outcome.findings.len(), 1, "{:?}", outcome.findings);
    assert_eq!(outcome.findings[0].locations.len(), 5);
}

/// A rule the config turns off neither groups nor reports, in either rule.
#[test]
fn an_off_rule_reports_nothing() {
    let files = same_file_pair();
    let outcome = run(
        &files,
        &config_with(&[(Rule::DuplicateTest, Severity::Off)]),
    );
    assert!(outcome.findings.is_empty());

    let files = cross_file_pair();
    let outcome = run(
        &files,
        &config_with(&[(Rule::CrossFileDuplicate, Severity::Off)]),
    );
    assert!(outcome.findings.is_empty());
}

/// The configured severity is the finding severity.
#[test]
fn a_severity_override_reaches_the_finding() {
    let files = same_file_pair();
    let outcome = run(
        &files,
        &config_with(&[(Rule::DuplicateTest, Severity::Warning)]),
    );
    assert_eq!(outcome.findings[0].severity, Severity::Warning);
}

/// Reading the same files in reverse gives the same findings.
#[test]
fn the_output_does_not_depend_on_the_order_the_files_were_read() {
    let forward = [
        file("a.test.ts", vec![case(5, 100, 1, 1)]),
        file("b.test.ts", vec![case(7, 100, 1, 1)]),
        file("c.test.ts", vec![case(6, 100, 1, 1)]),
    ];
    let mut reversed = forward.clone();
    reversed.reverse();
    let config = config_with(&[]);
    assert_eq!(
        outline(&run(&forward, &config)),
        outline(&run(&reversed, &config))
    );
}

/// The budget pays for whole findings, stops before what it cannot afford, and says so.
#[test]
fn an_exhausted_budget_stops_the_run_and_says_so() {
    let mut cases: Vec<TestCase> = (1..=5)
        .map(|line| {
            let mut case = case(line * 4 + 16, line * 100 + 200, 1, 1);
            case.key = 1;
            case.module_key = 1;
            case
        })
        .collect();
    cases.push(case(4, 100, 2, 2));
    cases.push(case(8, 200, 2, 2));
    let files = [file("a.test.ts", cases)];
    // The pair, whose group comes first, costs one unit; the cluster of five costs four. A
    // budget of two pays for the pair and stops before the cluster.
    let outcome = run_with_budget(&files, &config_with(&[]), 2);
    assert!(outcome.budget_exhausted);
    assert_eq!(outline(&outcome).len(), 1, "only what the budget paid for");
    assert_eq!(outline(&outcome)[0].1, vec!["a.test.ts", "a.test.ts"]);
    let outcome = run_with_budget(&files, &config_with(&[]), 100);
    assert!(!outcome.budget_exhausted);
    assert_eq!(outline(&outcome).len(), 2);
    // A cluster whose charge the remaining budget cannot cover reports nothing rather than a
    // truncated one.
    let outcome = run_with_budget(&files, &config_with(&[]), 3);
    assert!(outcome.budget_exhausted);
    assert_eq!(outline(&outcome).len(), 1);
}

/// With no configuration file, the two rules report at their shipped severities.
#[test]
fn load_built_in_defaults_report_the_shipped_severities() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let files = [
        file("a.test.ts", vec![case(5, 100, 1, 1), case(9, 200, 1, 1)]),
        file("b.test.ts", vec![case(7, 100, 1, 1), case(11, 200, 1, 1)]),
    ];
    let outcome = run(&files, &config);
    let duplicate = outcome
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateTest)
        .unwrap();
    assert_eq!(duplicate.severity, Severity::Error);
    let cross_file = outcome
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::CrossFileDuplicate)
        .unwrap();
    assert_eq!(cross_file.severity, Severity::Warning);
    assert_eq!(
        cross_file.locations.len(),
        2,
        "one representative per file: the two copies of each file collapse to one member"
    );
    assert_eq!(
        cross_file.titles,
        vec!["t5".to_owned(), "t7".to_owned()],
        "the first-spelled case of each file represents it"
    );
}

/// A budget too small for the first pair stops the run before anything is reported.
#[test]
fn an_exhausted_budget_stops_before_the_first_spanning_pair() {
    let files = same_file_pair();
    let outcome = run_with_budget(&files, &config_with(&[]), 0);
    assert!(outcome.budget_exhausted);
    assert!(outcome.findings.is_empty(), "nothing was affordable");
}

/// Entry equality and order follow the file, then the span, so grouping is content-ordered.
#[test]
fn entries_order_and_equality_follow_where_the_case_is() {
    let a = case(5, 100, 1, 1);
    let other = case(5, 100, 1, 1);
    let left = Entry {
        path: Path::new("a.test.ts"),
        case: &a,
    };
    let left_again = Entry {
        path: Path::new("a.test.ts"),
        case: &a,
    };
    let right = Entry {
        path: Path::new("b.test.ts"),
        case: &other,
    };
    assert_eq!(left, left_again);
    assert_ne!(left, right);
    assert!(left < right);
    let earlier = Entry {
        path: Path::new("a.test.ts"),
        case: &case(3, 90, 1, 1),
    };
    assert!(earlier < left, "line and column order within one file");
}

/// Name collisions depend on file, title segments and distinct declarations, not callback keys.
#[test]
fn names_use_proven_segments_and_preserve_all_exclusions() {
    let config = config_with(&[
        (Rule::DuplicateTest, Severity::Off),
        (Rule::CrossFileDuplicate, Severity::Off),
    ]);
    let mut left = case(1, 10, 1, 1);
    left.name = Some(vec!["suite".to_owned(), "name".to_owned()]);
    let mut right = case(2, 20, 2, 2);
    right.name = left.name.clone();
    let files = [file("a.test.ts", vec![left.clone(), right.clone()])];
    let result = run(&files, &config);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].rule, Rule::DuplicateTestName);
    assert_eq!(result.findings[0].titles, ["suite > name", "suite > name"]);
    for exclusion in [
        Exclusion::EmptyBody,
        Exclusion::NoCallback,
        Exclusion::UnreadableCallback,
        Exclusion::NamedCallback,
        Exclusion::RegisteredInsideFunction,
        Exclusion::RegisteredElsewhere,
        Exclusion::OrderDependentState,
        Exclusion::Benchmark,
    ] {
        right.exclusion = Some(exclusion);
        assert!(
            run(
                &[file("a.test.ts", vec![left.clone(), right.clone()])],
                &config
            )
            .findings
            .is_empty(),
            "{exclusion:?}"
        );
    }
    right.exclusion = None;
    for name in [
        None,
        Some(vec!["suite > name".to_owned()]),
        Some(vec!["other".to_owned(), "name".to_owned()]),
    ] {
        right.name = name;
        assert!(run(
            &[file("a.test.ts", vec![left.clone(), right.clone()])],
            &config
        )
        .findings
        .is_empty());
    }
    right.name = left.name.clone();
    assert!(run(
        &[
            file("a.test.ts", vec![left.clone()]),
            file("b.test.ts", vec![right.clone()])
        ],
        &config
    )
    .findings
    .is_empty());
    right.span = left.span;
    right.row = Some(1);
    assert!(run(&[file("a.test.ts", vec![left, right])], &config)
        .findings
        .is_empty());
}

/// A name collision can coexist with a body duplicate; config and budget affect it independently.
#[test]
fn name_config_budget_and_coexisting_rules_are_independent() {
    let mut cases = vec![case(1, 10, 1, 1), case(2, 20, 1, 1)];
    for case in &mut cases {
        case.name = Some(vec!["same".to_owned()]);
    }
    let files = [file("a.test.ts", cases)];
    let result = run(&files, &config_with(&[]));
    assert_eq!(
        result
            .findings
            .iter()
            .map(|finding| finding.rule)
            .collect::<Vec<_>>(),
        vec![Rule::DuplicateTest, Rule::DuplicateTestName]
    );
    let only_names = config_with(&[
        (Rule::DuplicateTest, Severity::Off),
        (Rule::DuplicateTestName, Severity::Error),
    ]);
    assert_eq!(
        run(&files, &only_names).findings[0].severity,
        Severity::Error
    );
    let exhausted = run_with_budget(&files, &only_names, 0);
    assert!(exhausted.budget_exhausted);
    assert!(exhausted.findings.is_empty());
    assert_eq!(
        run(
            &files,
            &config_with(&[(Rule::DuplicateTestName, Severity::Off)])
        )
        .findings
        .len(),
        1
    );
}

/// Every distinct rendered title survives even when five tables share the same declaration set.
#[test]
fn name_clusters_keep_every_title_and_ignore_input_order() {
    let cases: Vec<_> = (0..5)
        .flat_map(|site| {
            (0..2).map(move |row| {
                let mut test = case(site + 1, site * 10, u64::from(site), u64::from(site));
                test.row = Some(row);
                test.name = Some(vec![format!("row {row}")]);
                test
            })
        })
        .collect();
    let config = config_with(&[
        (Rule::DuplicateTest, Severity::Off),
        (Rule::CrossFileDuplicate, Severity::Off),
    ]);
    let expected = run(&[file("a.test.ts", cases.clone())], &config);
    assert_eq!(expected.findings.len(), 2);
    assert!(expected
        .findings
        .iter()
        .all(|finding| finding.locations.len() == 5));
    let reversed = run(
        &[file("a.test.ts", cases.into_iter().rev().collect())],
        &config,
    );
    assert_eq!(expected.findings, reversed.findings);
}

/// Rows share a declaration and input class; unrelated rules cannot consume this test's budget.
fn row_population(size: usize) -> Vec<TestCase> {
    (0..size)
        .map(|index| {
            let mut row = case(5, 100, 1, 1);
            row.row = Some(index);
            row.row_class = Some(0);
            row
        })
        .collect()
}

/// Every dimension in the row proof is required; body or name equality alone is insufficient.
#[test]
fn row_rule_partitions_by_all_proof_dimensions() {
    let config = config_with(&[(Rule::DuplicateEachRow, Severity::Error)]);
    let base = row_population(2);
    let finding = run(&[file("a.test.ts", base.clone())], &config);
    assert_eq!(finding.findings.len(), 1);
    assert_eq!(finding.findings[0].rule, Rule::DuplicateEachRow);
    assert_eq!(finding.findings[0].severity, Severity::Error);
    for dimension in 0..8 {
        let mut rows = base.clone();
        match dimension {
            0 => rows[1].row_class = None,
            1 => rows[1].row_class = Some(1),
            2 => rows[1].row = None,
            3 => rows[1].name = None,
            4 => rows[1].name = Some(vec!["different".into()]),
            5 => rows[1].key = 2,
            6 => rows[1].exclusion = Some(Exclusion::EmptyBody),
            _ => rows[1].span.start_byte = 200,
        }
        let findings = run(&[file("a.test.ts", rows)], &config).findings;
        assert!(
            findings
                .iter()
                .all(|finding| finding.rule != Rule::DuplicateEachRow),
            "dimension {dimension}"
        );
    }
    let findings = run(
        &[
            file("a.test.ts", vec![base[0].clone()]),
            file("b.test.ts", vec![base[1].clone()]),
        ],
        &config,
    )
    .findings;
    assert!(findings
        .iter()
        .all(|finding| finding.rule != Rule::DuplicateEachRow));
    let disabled = config_with(&[(Rule::DuplicateEachRow, Severity::Off)]);
    assert!(run(&[file("a.test.ts", base)], &disabled)
        .findings
        .is_empty());
}

/// A whole row group must fit before emission, for spanning pairs and clusters alike.
#[test]
fn row_rule_preserves_spanning_edges_clusters_and_budget() {
    let config = config_with(&[]);
    for (size, expected) in [(3, 2), (5, 1)] {
        let mut cases = row_population(size);
        let files = [file("a.test.ts", cases.clone())];
        let short = run_with_budget(&files, &config, size as u64 - 2);
        assert!(short.budget_exhausted);
        assert!(short.findings.is_empty());
        let enough = run_with_budget(&files, &config, size as u64 - 1);
        assert!(!enough.budget_exhausted);
        assert_eq!(enough.findings.len(), expected);
        cases.reverse();
        assert_eq!(
            run(&[file("a.test.ts", cases)], &config).findings,
            enough.findings
        );
    }
}

mod suites;

mod hooks;
mod parameters;
