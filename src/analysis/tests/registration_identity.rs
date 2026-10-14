//! Contracts for declaration source locations and the files registering those declarations.
//! These owned IR inputs prepare imported extraction without introducing an imported walk.

use crate::analysis::{analyze, test_fingerprints};
use crate::config::{Config, ConfigOverrides};
use crate::model::{
    AnalysisResult, BehaviorTrace, BodyDigest, Context, DeclarationKind, Fingerprints,
    IncompleteCauses, Location, Position, Rule, Severity, Span, TestCase, Title,
};
use crate::modes;
use crate::reporters::{
    HtmlReporter, JsonReporter, JsonlReporter, Reporter, SarifReporter, TerminalReporter,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Build a declaration at its source location; when owner differs from source, attach caller
/// provenance through the serialized model contract.
fn registered_case(source: &str, owner: &str, line: u32) -> TestCase {
    let case = TestCase {
        registration_file: None,
        location: Location {
            path: PathBuf::from(source),
            span: Span {
                start: Position { line, column: 1 },
                end: Position { line, column: 20 },
                start_byte: line * 100,
                end_byte: line * 100 + 20,
            },
        },
        kind: DeclarationKind::Test,
        suite_path: Vec::new(),
        title: Title::Static(format!("case {line}")),
        modifiers: Vec::new(),
        context: Context::default(),
        body: Fingerprints {
            alpha: BodyDigest::of("expect(1).toBe(1)"),
            ..Fingerprints::default()
        },
        has_body: true,
        table_row_class: None,
        row_cells_fully_observable: true,
    };
    // The wire contract lets the same input reproduce the old model's ignored ownership.
    let mut value = serde_json::to_string(&case).unwrap();
    if source != owner {
        assert_eq!(value.pop(), Some('}'));
        value.push_str(&format!(
            ",\"registration_file\":{}}}",
            serde_json::to_string(owner).unwrap()
        ));
    }
    serde_json::from_str(&value).unwrap()
}

/// Analyze the supplied cases with default rules, assert the extracted count, and return the
/// findings.
fn findings(cases: Vec<TestCase>) -> Vec<crate::model::Finding> {
    let expected_count = cases.len();
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let result = analyze(cases, &config, 2, IncompleteCauses::default()).unwrap();
    assert_eq!(result.tests_analyzed, expected_count);
    result.findings
}

/// Analyze fixture cases with only the selected rule enabled and return the full result.
fn result_for(cases: Vec<TestCase>, rule: Rule) -> AnalysisResult {
    result_for_rules(cases, &[rule])
}

/// Analyze fixture cases with the selected rules enabled, asserting the non-suite test count.
fn result_for_rules(cases: Vec<TestCase>, rules: &[Rule]) -> AnalysisResult {
    let expected_count = cases
        .iter()
        .filter(|case| case.kind != DeclarationKind::Suite)
        .count();
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(
        directory.path(),
        ConfigOverrides {
            rules: Rule::ALL
                .iter()
                .copied()
                .map(|candidate| {
                    (
                        candidate,
                        if rules.contains(&candidate) {
                            Severity::Error
                        } else {
                            Severity::Off
                        },
                    )
                })
                .collect(),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();
    let result = analyze(cases, &config, 2, IncompleteCauses::default()).unwrap();
    assert_eq!(result.tests_analyzed, expected_count);
    result
}

/// Build a rule-specific declaration pair whose registration owners are either shared or
/// distinct.
fn scoped_pair(rule: Rule, same_caller: bool) -> Vec<TestCase> {
    let mut a = registered_case("a-runner.ts", "a.test.ts", 1);
    let mut b = registered_case(
        "b-runner.ts",
        if same_caller {
            "a.test.ts"
        } else {
            "b.test.ts"
        },
        2,
    );
    if matches!(rule, Rule::DuplicateTestName | Rule::DuplicateSuite) {
        b.title = a.title.clone();
    }
    if rule == Rule::DuplicateSuite {
        a.kind = DeclarationKind::Suite;
        b.kind = DeclarationKind::Suite;
    }
    if matches!(
        rule,
        Rule::ParameterizationCandidate | Rule::NearDuplicateTest | Rule::SubsumedTest
    ) {
        b.body.alpha = BodyDigest::of("different body");
        a.body.behavior = BehaviorTrace::new(&["setup".to_owned(), "assert".to_owned()]);
        b.body.behavior = if rule == Rule::NearDuplicateTest {
            BehaviorTrace::new(&["assert".to_owned(), "setup".to_owned()])
        } else if rule == Rule::SubsumedTest {
            BehaviorTrace::new(&["setup".to_owned(), "assert".to_owned(), "extra".to_owned()])
        } else {
            a.body.behavior.clone()
        };
    }
    vec![a, b]
}

/// Verify direct and imported declarations in one caller are same file.
#[test]
fn registration_identity_direct_and_imported_declarations_in_one_caller_are_same_file() {
    let result = findings(vec![
        registered_case("caller.test.ts", "caller.test.ts", 1),
        registered_case("runner.ts", "caller.test.ts", 2),
    ]);
    assert_eq!(result.len(), 1, "{result:?}");
    assert_eq!(result[0].rule, Rule::DuplicateTest);
    assert_eq!(result[0].locations[1].path, PathBuf::from("runner.ts"));
}

/// Verify different callers in one helper are cross file.
#[test]
fn registration_identity_different_callers_in_one_helper_are_cross_file() {
    let result = findings(vec![
        registered_case("runner.ts", "a.test.ts", 1),
        registered_case("runner.ts", "b.test.ts", 2),
    ]);
    assert_eq!(result.len(), 1, "{result:?}");
    assert_eq!(result[0].rule, Rule::CrossFileDuplicate);
    assert!(result[0].message.contains("across 2 files"));
}

/// Verify repeated calls to one declaration do not pair.
#[test]
fn registration_identity_repeated_calls_to_one_declaration_do_not_pair() {
    let result = findings(vec![
        registered_case("runner.ts", "a.test.ts", 1),
        registered_case("runner.ts", "b.test.ts", 1),
    ]);
    assert!(result.is_empty(), "{result:?}");
}

/// Verify shared declaration does not hide a cross file pair.
#[test]
fn registration_identity_shared_declaration_does_not_hide_a_cross_file_pair() {
    let result = result_for(
        vec![
            registered_case("a-runner.ts", "a.test.ts", 1),
            registered_case("a-runner.ts", "b.test.ts", 1),
            registered_case("b-runner.ts", "a.test.ts", 2),
        ],
        Rule::CrossFileDuplicate,
    );
    assert_eq!(result.findings.len(), 1);
    let finding = &result.findings[0];
    assert_eq!(finding.locations.len(), 2);
    assert_ne!(finding.locations[0], finding.locations[1]);
    assert_eq!(
        finding.registration_files_for(1).collect::<Vec<_>>(),
        vec![std::path::Path::new("a.test.ts")]
    );
    assert!(finding
        .registration_files_for(0)
        .any(|path| path == std::path::Path::new("b.test.ts")));
}

/// Verify different caller configurations do not compare.
#[test]
fn registration_identity_different_caller_configurations_do_not_compare() {
    let a = registered_case("runner.ts", "a.test.ts", 1);
    let mut b = registered_case("runner.ts", "b.test.ts", 2);
    b.context.config_scope = Some("happy-dom".to_owned());
    assert!(findings(vec![a, b]).is_empty());
}

/// Verify every file scoped rule uses the caller.
#[test]
fn registration_identity_every_file_scoped_rule_uses_the_caller() {
    for rule in [
        Rule::DuplicateTest,
        Rule::DuplicateTestName,
        Rule::DuplicateSuite,
        Rule::ParameterizationCandidate,
        Rule::NearDuplicateTest,
        Rule::SubsumedTest,
    ] {
        let same = result_for(scoped_pair(rule, true), rule);
        assert_eq!(same.findings.len(), 1, "{rule:?}: {same:?}");
        assert_eq!(same.findings[0].rule, rule);
        assert!(
            result_for(scoped_pair(rule, false), rule)
                .findings
                .is_empty(),
            "{rule:?}"
        );
    }
}

/// Verify table rows do not combine across callers.
#[test]
fn registration_identity_table_rows_do_not_combine_across_callers() {
    let mut a = registered_case("runner.ts", "a.test.ts", 1);
    a.table_row_class = Some(0);
    let mut b = a.clone();
    let same = result_for(vec![a.clone(), b.clone()], Rule::DuplicateEachRow);
    assert_eq!(same.findings.len(), 1);
    b.registration_file = Some(PathBuf::from("b.test.ts"));
    assert!(result_for(vec![a, b], Rule::DuplicateEachRow)
        .findings
        .is_empty());
}

/// Verify cross file representatives need not be adjacent.
#[test]
fn registration_identity_cross_file_representatives_need_not_be_adjacent() {
    let result = result_for(
        vec![
            registered_case("a-runner.ts", "a.test.ts", 1),
            registered_case("b-runner.ts", "b.test.ts", 2),
            registered_case("c-runner.ts", "a.test.ts", 3),
        ],
        Rule::CrossFileDuplicate,
    );
    assert_eq!(result.findings.len(), 1, "{result:?}");
    assert_eq!(result.findings[0].locations.len(), 2);
    assert!(result.findings[0].message.contains("across 2 files"));
}

/// Verify collapsed callers remain visible to changed file filtering.
#[test]
fn registration_identity_collapsed_callers_remain_visible_to_changed_file_filtering() {
    let result = result_for(
        vec![
            registered_case("runner.ts", "a.test.ts", 1),
            registered_case("runner.ts", "b.test.ts", 1),
            registered_case("other.ts", "c.test.ts", 2),
        ],
        Rule::CrossFileDuplicate,
    );
    assert_eq!(result.findings.len(), 1);
    for changed in [
        "a.test.ts",
        "b.test.ts",
        "c.test.ts",
        "runner.ts",
        "other.ts",
        "unrelated.ts",
    ] {
        let mut findings = result.findings.clone();
        let removed = modes::retain_changed_findings(
            &mut findings,
            &BTreeSet::from([PathBuf::from(changed)]),
        );
        assert_eq!(removed, usize::from(changed == "unrelated.ts"), "{changed}");
    }
}

/// Verify suppressions and html snippets follow source locations.
#[test]
fn registration_identity_suppressions_and_html_snippets_follow_source_locations() {
    let mut result = result_for(scoped_pair(Rule::DuplicateTest, true), Rule::DuplicateTest);
    assert_eq!(result.findings.len(), 1);
    result.findings[0].locations[0].span.start_byte = 0;
    result.findings[0].locations[0].span.end_byte = 6;
    result.findings[0].locations[0].span.end.column = 7;
    for path in ["a.test.ts", "a-runner.ts"] {
        let mut findings = result.findings.clone();
        let suppression = modes::InlineSuppression {
            path: PathBuf::from(path),
            line: 0,
            reason: "intentional".to_owned(),
        };
        assert_eq!(
            modes::apply_inline_suppressions(&mut findings, &[suppression]),
            usize::from(path == "a-runner.ts")
        );
    }
    let mut output = Vec::new();
    HtmlReporter::with_sources(BTreeMap::from([
        (PathBuf::from("a-runner.ts"), "HELPER".to_owned()),
        (PathBuf::from("a.test.ts"), "CALLER".to_owned()),
    ]))
    .write_report(&result, &mut output)
    .unwrap();
    let html = String::from_utf8(output).unwrap();
    assert!(html.contains("HELPER"));
    assert!(!html.contains("CALLER"));
}

/// Verify baselines distinguish callers and ignore unrelated source edits.
#[test]
fn registration_identity_baselines_distinguish_callers_and_ignore_unrelated_source_edits() {
    let result = result_for(scoped_pair(Rule::DuplicateTest, true), Rule::DuplicateTest);
    assert_eq!(result.findings.len(), 1);
    let original = &result.findings[0];
    let mut shifted = original.clone();
    shifted.locations[0].span.start.line += 20;
    assert_eq!(original.fingerprint(), shifted.fingerprint());
    let mut reversed = original.clone();
    reversed.locations.reverse();
    reversed.titles.reverse();
    reversed.registration_files.reverse();
    assert_eq!(original.fingerprint(), reversed.fingerprint());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("baseline.json");
    modes::write_baseline(&path, &result.findings).unwrap();
    let baseline = modes::load_baseline(&path).unwrap();
    let mut findings = vec![shifted];
    assert_eq!(modes::apply_baseline(&mut findings, &baseline), (1, 0));
    let mut changed = original.clone();
    changed.set_registration_files(vec![vec![PathBuf::from("another.test.ts")]; 2]);
    assert_ne!(original.fingerprint(), changed.fingerprint());
    assert_eq!(modes::apply_baseline(&mut [changed], &baseline), (0, 1));
}

/// Source-owned v1 identities remain byte-for-byte compatible with existing baselines.
#[test]
fn registration_identity_legacy_fingerprints_and_optional_output_are_preserved() {
    let cases = vec![
        registered_case("a.test.ts", "a.test.ts", 1),
        registered_case("a.test.ts", "a.test.ts", 2),
    ];
    let result = result_for(cases.clone(), Rule::DuplicateTest);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(
        result.findings[0].fingerprint(),
        "vitestDedupFingerprint/v1:49bde1b5f5b5f1b3"
    );
    let mut explicit = result.findings[0].clone();
    explicit.set_registration_files(vec![vec![PathBuf::from("a.test.ts")]; 2]);
    assert!(explicit.registration_files.is_empty());
    assert_eq!(explicit.fingerprint(), result.findings[0].fingerprint());
    let mut output = Vec::new();
    JsonReporter::new()
        .write_report(&result, &mut output)
        .unwrap();
    assert!(!String::from_utf8(output)
        .unwrap()
        .contains("registrationFiles"));
    let mut output = Vec::new();
    JsonlReporter::with_fingerprints(Vec::new(), test_fingerprints(&cases))
        .write_report(&result, &mut output)
        .unwrap();
    assert!(!String::from_utf8(output)
        .unwrap()
        .contains("registrationFile"));
}

/// UTF-8 extended identities keep their existing hash input and separator normalization.
#[test]
fn registration_identity_utf8_extended_fingerprints_remain_compatible() {
    let result = result_for(scoped_pair(Rule::DuplicateTest, true), Rule::DuplicateTest);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(
        result.findings[0].fingerprint(),
        "vitestDedupFingerprint/v1:1a2f7d37706a50de"
    );
    let mut a = result.findings[0].clone();
    a.set_registration_files(vec![vec![PathBuf::from("tests\\caller.test.ts")]; 2]);
    let mut b = a.clone();
    b.set_registration_files(vec![vec![PathBuf::from("tests/caller.test.ts")]; 2]);
    assert_eq!(a.fingerprint(), b.fingerprint());
}

/// Builds an opaque path in owned IR without creating a host-platform filename.
#[cfg(any(unix, windows))]
fn non_unicode_path(discriminator: u8) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let mut units = b"opaque".to_vec();
        units.push(0x80 + discriminator);
        units.extend_from_slice(b".ts");
        std::ffi::OsString::from_vec(units).into()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        let mut units: Vec<u16> = "opaque".encode_utf16().collect();
        units.push(0xd800 + u16::from(discriminator));
        units.extend(".ts".encode_utf16());
        std::ffi::OsString::from_wide(&units).into()
    }
}

/// Source and caller paths must retain distinctions that their display strings erase.
#[cfg(any(unix, windows))]
#[test]
fn registration_identity_non_unicode_paths_do_not_alias_baselines() {
    let result = result_for(scoped_pair(Rule::DuplicateTest, true), Rule::DuplicateTest);
    assert_eq!(result.findings.len(), 1);
    let opaque_a = non_unicode_path(0);
    let opaque_b = non_unicode_path(1);
    assert_ne!(opaque_a, opaque_b);
    assert_eq!(opaque_a.to_string_lossy(), opaque_b.to_string_lossy());
    let source = PathBuf::from("a-runner.ts");
    let owner = PathBuf::from("a.test.ts");
    for (label, source_a, caller_a, source_b, caller_b) in [
        ("callers", &source, &opaque_a, &source, &opaque_b),
        ("sources", &opaque_a, &owner, &opaque_b, &owner),
        ("legacy routing", &opaque_a, &opaque_a, &opaque_a, &opaque_b),
        (
            "replacement text",
            &source,
            &opaque_a,
            &source,
            &PathBuf::from(opaque_a.to_string_lossy().as_ref()),
        ),
    ] {
        let mut a = result.findings[0].clone();
        a.locations[0].path = source_a.clone();
        a.set_registration_files(vec![vec![caller_a.clone()], vec![owner.clone()]]);
        let mut b = a.clone();
        b.locations[0].path = source_b.clone();
        b.set_registration_files(vec![vec![caller_b.clone()], vec![owner.clone()]]);
        if label == "legacy routing" {
            assert_eq!(
                a.fingerprint(),
                "vitestDedupFingerprint/v1:c4b7cf6251ed416a"
            );
        }
        assert_ne!(a.fingerprint(), b.fingerprint(), "{label}");
        let directory = tempfile::tempdir().unwrap();
        let baseline = directory.path().join("baseline.json");
        modes::write_baseline(&baseline, &[a.clone()]).unwrap();
        let baseline = modes::load_baseline(&baseline).unwrap();
        assert_eq!(modes::apply_baseline(&mut [a], &baseline), (1, 0));
        assert_eq!(
            modes::apply_baseline(&mut [b], &baseline),
            (0, 1),
            "{label}"
        );
    }
    let mut a = result.findings[0].clone();
    a.set_registration_files(vec![vec![opaque_a.clone()], vec![owner.clone()]]);
    let mut b = a.clone();
    b.registration_files[0].push(opaque_b.clone());
    assert_ne!(
        a.fingerprint(),
        b.fingerprint(),
        "distinct callers must not deduplicate by display text"
    );
    a.registration_files[0] = vec![opaque_b, opaque_a.clone(), opaque_a.clone()];
    assert_eq!(
        a.fingerprint(),
        b.fingerprint(),
        "caller order and repeated provenance are immaterial"
    );
    for finding in [&mut a, &mut b] {
        finding.set_registration_files(vec![vec![owner.clone()]; 2]);
    }
    let mut backslash = std::ffi::OsString::from("nested\\");
    backslash.push(opaque_a.as_os_str());
    let mut slash = std::ffi::OsString::from("nested/");
    slash.push(opaque_a.as_os_str());
    a.locations[0].path = backslash.into();
    b.locations[0].path = slash.into();
    assert_eq!(
        a.fingerprint(),
        b.fingerprint(),
        "native paths retain separator normalization"
    );
}

/// Verify structured reports publish sanitized caller provenance.
#[test]
fn registration_identity_structured_reports_publish_sanitized_caller_provenance() {
    let cases = vec![
        registered_case("a-runner.ts", "a\u{202e}.test.ts", 1),
        registered_case("b-runner.ts", "a\u{202e}.test.ts", 2),
    ];
    let result = result_for(cases.clone(), Rule::DuplicateTest);
    assert_eq!(result.findings.len(), 1);
    for reporter in [
        Box::new(JsonReporter::new()) as Box<dyn Reporter>,
        Box::new(JsonlReporter::with_fingerprints(
            Vec::new(),
            test_fingerprints(&cases),
        )),
        Box::new(SarifReporter::new()),
        Box::new(HtmlReporter::new()),
    ] {
        let mut output = Vec::new();
        reporter.write_report(&result, &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("registrationFiles"));
        assert!(output.contains("a.test.ts"));
        assert!(!output.contains('\u{202e}'));
    }
    let mut output = Vec::new();
    TerminalReporter::new()
        .write_report(&result, &mut output)
        .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("a-runner.ts:1:1"));
    assert!(output.contains("b-runner.ts:2:1"));
}

/// Verify unresolved counts belong to callers.
#[test]
fn registration_identity_unresolved_counts_belong_to_callers() {
    let mut a = registered_case("runner.ts", "a.test.ts", 1);
    let mut b = registered_case("runner.ts", "b.test.ts", 2);
    a.context.unresolved = true;
    b.context.unresolved = true;
    let result = result_for(vec![a, b], Rule::DuplicateTest);
    assert_eq!(result.unresolved_tests, 2);
    assert_eq!(result.blind_files.len(), 2);
    assert!(result.findings.is_empty());
}

/// Verify memory includes the retained caller path.
#[test]
fn registration_identity_memory_includes_the_retained_caller_path() {
    use crate::memory::HeapBytes;
    let mut case = registered_case("runner.ts", "a.test.ts", 1);
    let owner = case.registration_file.take().unwrap();
    let ordinary = case.heap_bytes();
    let owner_bytes = owner.heap_bytes();
    assert!(owner_bytes > 0);
    case.registration_file = Some(owner);
    assert_eq!(case.heap_bytes(), ordinary + owner_bytes);
}

/// Verify copied suite suppression is scoped to the caller.
#[test]
fn registration_identity_copied_suite_suppression_is_scoped_to_the_caller() {
    let mut suites = scoped_pair(Rule::DuplicateSuite, true);
    for suite in &mut suites {
        suite.location.span.start_byte = 0;
        suite.location.span.end_byte = 10_000;
    }
    let children = vec![
        registered_case("a-runner.ts", "b.test.ts", 10),
        registered_case("b-runner.ts", "b.test.ts", 11),
    ];
    let mut input = suites.clone();
    input.extend(children.clone());
    let result = result_for_rules(input, &[Rule::DuplicateSuite, Rule::DuplicateTest]);
    assert_eq!(result.findings.len(), 2, "{result:?}");
    let mut input = suites;
    input.extend(children.into_iter().map(|mut test| {
        test.registration_file = Some(PathBuf::from("a.test.ts"));
        test
    }));
    let same = result_for_rules(input, &[Rule::DuplicateSuite, Rule::DuplicateTest]);
    assert_eq!(same.findings.len(), 1, "{same:?}");
    assert_eq!(same.findings[0].rule, Rule::DuplicateSuite);
}

/// Verify shared hook pairs preserve every caller.
#[test]
fn registration_identity_shared_hook_pairs_preserve_every_caller() {
    use crate::model::{Hook, HookKind};
    let mut cases = scoped_pair(Rule::DuplicateTest, false);
    let hooks: Vec<Hook> = cases
        .iter()
        .map(|case| Hook {
            kind: HookKind::BeforeEach,
            location: case.location.clone(),
            body: case.body.clone(),
            idempotent: true,
            alpha_source: std::sync::Arc::from("reset()"),
        })
        .collect();
    for case in &mut cases {
        case.context.hooks = hooks.clone();
    }
    let result = result_for(cases, Rule::RedundantHook);
    assert_eq!(result.findings.len(), 1, "{result:?}");
    assert_eq!(
        result.findings[0].registration_files,
        vec![vec![PathBuf::from("a.test.ts"), PathBuf::from("b.test.ts")]; 2]
    );
}

/// Verify jsonl bounds and names provenance fields.
#[test]
fn registration_identity_jsonl_bounds_and_names_provenance_fields() {
    let owner = "x".repeat(2_100);
    let cases = vec![
        registered_case("a.ts", &owner, 1),
        registered_case("b.ts", &owner, 2),
    ];
    let result = result_for(cases.clone(), Rule::DuplicateTest);
    assert_eq!(result.findings.len(), 1);
    let mut output = Vec::new();
    JsonlReporter::with_fingerprints(Vec::new(), test_fingerprints(&cases))
        .write_report(&result, &mut output)
        .unwrap();
    let output = String::from_utf8(output).unwrap();
    let records: Vec<serde_json::Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let test = &records[0];
    assert_eq!(
        test["registrationFile"].as_str().unwrap().chars().count(),
        2_000
    );
    assert!(test["truncatedFields"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("registrationFile")));
    let finding = records
        .iter()
        .find(|record| record["type"] == "finding")
        .unwrap();
    assert_eq!(
        finding["registrationFiles"][0][0]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        2_000
    );
    assert!(finding["truncatedFields"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("registrationFiles[0][0]")));
}
