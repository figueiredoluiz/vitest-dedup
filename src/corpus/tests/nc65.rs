//! Lossless strings against the same owned sources used by the runtime oracle.
use super::*;
use crate::lang::text;
use crate::model::Rule;

/// The owned message fixture retains distinct title identities and all eligible pair findings.
fn message_findings(rule: Rule) -> Vec<crate::model::Finding> {
    let source = include_str!("../../../fixtures/recall/lossless-string-messages/run.test.ts");
    let corpus = counted_corpus(source, "run.test.ts", 4, 0);
    assert_eq!(corpus.tests[0].title, corpus.tests[1].title);
    assert_eq!(corpus.tests[2].title, corpus.tests[3].title);
    assert_ne!(corpus.tests[0].title, corpus.tests[2].title);
    let outcome = owned_findings("run.test.ts", corpus);
    assert_eq!(outcome.findings.len(), 5, "{:?}", outcome.findings);
    outcome
        .findings
        .into_iter()
        .filter(|finding| finding.rule == rule)
        .collect()
}

/// Name messages display title code units without exposing internal tokens.
#[test]
fn nc65_name_messages_display_code_units() {
    let findings = message_findings(Rule::DuplicateTestName);
    assert_eq!(findings.len(), 2);
    for finding in findings {
        let expected = text::display(&finding.titles[0]);
        assert!(
            finding.message.contains(expected.as_ref()),
            "{}",
            finding.message
        );
        assert!(!finding.message.contains("�fffdd800"));
    }
}

/// Inert duplicate participants use display text in messages but retain semantic title fields.
#[test]
fn nc65_inert_messages_display_code_units() {
    let findings = message_findings(Rule::DuplicateTest);
    assert_eq!(findings.len(), 3);
    for finding in findings {
        assert!(finding.inert.iter().any(|inert| *inert));
        for (title, inert) in finding.titles.iter().zip(&finding.inert) {
            if *inert {
                assert!(
                    finding.message.contains(text::display(title).as_ref()),
                    "{}",
                    finding.message
                );
            }
        }
        assert!(!finding.message.contains("�fffdd800"));
    }
}

/// Report sanitization cannot expose forbidden pairs or alter semantic title identities.
#[test]
fn nc65_reports_filter_pairs_exposed_by_sanitization() {
    use crate::reporters::{JsonReporter, JsonlReporter, Reporter};
    let source = include_str!("../../../fixtures/reporter-runtime/lossless-strings.test.ts");
    let corpus = counted_corpus(source, "run.test.ts", 4, 0);
    let outcome = crate::model::AnalysisResult {
        findings: owned_findings("run.test.ts", corpus).findings,
        tests_analyzed: 4,
        eligible_cases: 4,
        files_analyzed: 1,
        ..Default::default()
    };
    assert_eq!(outcome.findings.len(), 3, "{:?}", outcome.findings);
    assert!(outcome
        .findings
        .iter()
        .all(|finding| finding.rule == Rule::DuplicateTest && finding.titles.len() == 2));
    let fingerprint = outcome.findings[0].fingerprint();
    for reporter in [&JsonReporter::new() as &dyn Reporter, &JsonlReporter::new()] {
        let mut bytes = Vec::new();
        reporter.write_report(&outcome, &mut bytes).unwrap();
        let output = String::from_utf8(bytes).unwrap();
        assert!(!output.contains('\u{e0061}'), "{output}");
        assert!(!output.contains('\u{200b}'), "{output}");
        assert!(output.contains("😀"), "{output}");
        assert!(output.contains("�db40�dc61"), "{output}");
        assert_eq!(outcome.findings[0].fingerprint(), fingerprint);
    }
}

/// Joined row values retain equality while the existing nonliteral-selector refusal stays intact.
#[test]
fn nc65_joined_values_keep_dynamic_selectors_uncertain() {
    let source = include_str!("../../../fixtures/oracle/lossless-strings/joined.test.ts");
    let corpus = counted_corpus(source, "joined.test.ts", 4, 0);
    assert_owned_row_findings(corpus, &[("joined value", 2)]);
}

/// Helper invocation bindings keep the same value identity as literals and row substitutions.
#[test]
fn nc65_helper_arguments_preserve_string_identity() {
    let source = include_str!("../../../fixtures/oracle/lossless-strings/helper.test.ts");
    let corpus = counted_corpus(source, "helper.test.ts", 6, 0);
    assert_eq!(corpus.tests[0].key, corpus.tests[1].key);
    assert_eq!(corpus.tests[2].key, corpus.tests[3].key);
    assert_ne!(corpus.tests[0].key, corpus.tests[2].key);
    assert_eq!(corpus.tests[0].key, corpus.tests[4].key);
    let outcome = owned_findings("helper.test.ts", corpus);
    let exact: Vec<_> = outcome
        .findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateTest)
        .collect();
    // Each invocation is its own declaration, so the two equal-argument calls pair with each
    // other declaration by declaration; a repeated source declaration pair (lines 4 and 5) is
    // still one claim across invocation groups, holding the replacement pair's cases too.
    let lines: Vec<Vec<u32>> = exact
        .iter()
        .map(|f| f.locations.iter().map(|l| l.span.start.line).collect())
        .collect();
    assert_eq!(lines, [[4, 4], [4, 5], [5, 5]], "{exact:?}");
    assert_eq!(exact[1].cases.len(), 4, "{:?}", exact[1].cases);
}

/// Re-parsed scalar substitutions equal literal source without turning the encoding into data.
#[test]
fn nc65_scalar_source_roundtrip_preserves_final_findings() {
    let source = include_str!("../../../fixtures/oracle/lossless-strings/roundtrip.test.ts");
    let corpus = counted_corpus(source, "roundtrip.test.ts", 5, 0);
    assert_eq!(
        corpus.tests[4].full_title.as_deref(),
        Some(r#"escaped "a\n\tb\r\b\f\"\\""#)
    );
    let outcome = owned_findings("roundtrip.test.ts", corpus);
    let exact: Vec<_> = outcome
        .findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateTest)
        .collect();
    assert_eq!(exact.len(), 2, "{exact:?}");
    assert!(exact
        .iter()
        .any(|f| f.titles == ["surrogate row", "surrogate literal"]));
    assert!(exact
        .iter()
        .any(|f| f.titles == ["replacement row", "replacement literal"]));
}

/// Display truncation counts actual UTF-16 units; long names still withhold name equivalence.
#[test]
fn nc65_title_truncation_preserves_surrogates_at_the_boundary() {
    let source = include_str!("../../../fixtures/oracle/lossless-strings/edges.test.ts");
    let corpus = counted_corpus(source, "edges.test.ts", 4, 0);
    let prefix = format!("edge {}", "a".repeat(38));
    let expected = [
        format!("{prefix}…"),
        format!("{prefix}{}…", text::from_utf16([0xdc00])),
        format!("{prefix}…"),
        format!("{prefix}{}…", text::from_utf8("�")),
    ];
    for (case, expected) in corpus.tests.iter().zip(expected) {
        assert_eq!(case.full_title.as_deref(), Some(expected.as_str()));
        assert!(case.name.is_none());
    }
    assert!(!owned_findings("edges.test.ts", corpus)
        .findings
        .iter()
        .any(|f| matches!(f.rule, Rule::DuplicateTestName | Rule::DuplicateEachRow)));
}

/// Different UTF-16 values cannot become duplicate bodies; equal surrogate bodies stay eligible.
#[test]
fn nc65_string_identity_preserves_findings_and_retains_positives() {
    let source = include_str!("../../../fixtures/recall/lossless-string-identity/identity.test.ts");
    let corpus = counted_corpus(source, "identity.test.ts", 8, 0);
    let outcome = owned_findings("identity.test.ts", corpus);
    let exact: Vec<_> = outcome
        .findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateTest)
        .collect();
    assert_eq!(exact.len(), 1, "{exact:?}");
    assert_eq!(
        exact[0].titles,
        [
            "retains the first surrogate body",
            "retains the second surrogate body"
        ]
    );
    assert!(!outcome
        .findings
        .iter()
        .any(|f| f.rule == Rule::DuplicateEachRow));
    assert!(outcome
        .findings
        .iter()
        .any(|f| f.rule == Rule::ParameterizationCandidate
            && f.titles
                == [
                    "reads a surrogate code unit",
                    "reads replacement character text"
                ]));
}

/// Titles, cooked substitutions, imported scalars and escaped export names share one identity.
#[test]
fn nc65_titles_and_imports_preserve_code_units() {
    let source = include_str!("../../../fixtures/oracle/lossless-strings/names.test.ts");
    let values = include_str!("../../../fixtures/oracle/lossless-strings/values�fffd.ts");
    let corpus = corpora(
        &[("names.test.ts", source), ("values�fffd.ts", values)],
        &["names.test.ts"],
    )
    .remove(0);
    assert_eq!(corpus.tests.len(), 19);
    assert_eq!(
        corpus
            .tests
            .iter()
            .filter(|c| c.exclusion.is_some())
            .count(),
        0
    );
    let titles: Vec<_> = corpus
        .tests
        .iter()
        .map(|c| text::json(&c.name.as_ref().expect("proved title").join(" > ")))
        .collect();
    assert!(titles.contains(&"\"\\ud800\"".to_owned()));
    assert!(titles.contains(&"\"�d800\"".to_owned()));
    assert_eq!(titles.iter().filter(|t| *t == "\"😀\"").count(), 2);
    assert!(titles.contains(&"\"j \\\"\\\\ud800\\\"\"".to_owned()));
    assert_eq!(
        titles
            .iter()
            .filter(|t| *t == "\"imported \\ud800\"")
            .count(),
        2
    );
    assert!(titles.contains(&"\"imported �d800\"".to_owned()));
    assert!(titles.contains(&"\"imported �fffd\"".to_owned()));
    let outcome = owned_findings("names.test.ts", corpus);
    let repeated: Vec<_> = outcome
        .findings
        .iter()
        .filter(|f| f.rule == Rule::DuplicateEachRow)
        .collect();
    assert_eq!(repeated.len(), 1, "{repeated:?}");
    assert_eq!(
        repeated[0].titles,
        vec![text::from_utf16("imported ".encode_utf16().chain([0xd800])); 2]
    );
}
