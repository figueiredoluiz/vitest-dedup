//! Human-first terminal report.
//!
//! Findings are grouped by rule and then by the file of their first location, because that is
//! how a developer scanning output decides what to fix next: pick a rule, pick a file, follow
//! the locations. The summary line at the end answers "did this run pass" without scrolling
//! back through every group, and an incomplete run says so before the findings, not after —
//! nothing here may let the reader assume that no findings means no duplicates.

use super::shared::{
    findings_by_file, forward_slash_path, severity_label, terminal_safe, SeverityCounts,
};
use super::Reporter;
use crate::model::{AnalysisResult, Rule};
use anyhow::Result;
use std::collections::BTreeMap;
use std::io::Write;

/// Renders an [`AnalysisResult`] as text with optional terminal colors.
///
/// Findings are grouped by rule and then by file; every participating location is printed as
/// `path:line:column` next to the test's full title so an editor or terminal can jump straight
/// to it. A trailing summary line reports counts by severity plus tests and files analyzed.
#[derive(Debug, Clone, Copy)]
pub struct TerminalReporter {
    /// Disables tool-owned ANSI styles (the default). Set to `false` to opt into color.
    /// The CLI resolves this from stdout's TTY status and `NO_COLOR`; library rendering
    /// never inspects the environment. Untrusted text is sanitized before styling.
    pub no_color: bool,
}

impl TerminalReporter {
    /// Creates a plain-text reporter without inspecting the environment or destination.
    pub fn new() -> Self {
        Self { no_color: true }
    }

    /// Wraps already-sanitized text in a tool-owned style, resetting it before returning.
    fn colorize(&self, text: String, code: &str) -> String {
        if self.no_color {
            text
        } else {
            format!("\x1b[{code}m{text}\x1b[0m")
        }
    }
}

impl Default for TerminalReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Reporter for TerminalReporter {
    /// Writes the grouped findings and the summary block to `writer`.
    fn write_report(&self, result: &AnalysisResult, writer: &mut dyn Write) -> Result<()> {
        writeln!(writer, "VitestDedup v{}", env!("CARGO_PKG_VERSION"))?;
        writeln!(writer)?;

        // The banner names the causes that actually fired. It used to assert "analysis hit a
        // limit or parse failure" on every incomplete run, which on a real 24,147-test corpus was
        // simply untrue -- neither limit was hit, and the cause was 1,729 unresolved contexts.
        // Telling a reader the tool had been overwhelmed by their repository, when it had not,
        // invites them to dismiss the warning instead of fixing the files behind it.
        let reasons = result.incomplete_causes.reasons();
        if !reasons.is_empty() {
            writeln!(
                writer,
                "INCOMPLETE: some tests could not be compared — absence of a finding below is \
                 not proof they are duplicate-free."
            )?;
            for reason in &reasons {
                writeln!(writer, "  - {reason}")?;
            }
            writeln!(writer)?;
        }

        // Its own banner rather than a footnote on the summary line, and above the findings for
        // the same reason the incomplete banner is: a reader who sees a short list and stops here
        // must not conclude the repository is clean. The whole corpus *was* analyzed (see
        // `crate::modes`), so the findings shown are real and complete for the files named — what
        // is missing is everything else, and that is what this says.
        if result.modes.changed_scope {
            writeln!(
                writer,
                "CHANGED-FILES RUN: only findings touching a file changed since the given \
                 revision are shown."
            )?;
            if result.modes.withheld_unchanged > 0 {
                writeln!(
                    writer,
                    "  {} finding(s) lying entirely within unchanged files were withheld. This \
                     report cannot speak for the rest of the repository.",
                    result.modes.withheld_unchanged
                )?;
            }
            writeln!(writer)?;
        }

        if result.findings.is_empty() {
            writeln!(writer, "No findings.")?;
            writeln!(writer)?;
        } else {
            let mut by_rule: BTreeMap<_, Vec<&crate::model::Finding>> = BTreeMap::new();
            for finding in &result.findings {
                by_rule.entry(finding.rule).or_default().push(finding);
            }

            for (rule, findings) in by_rule {
                writeln!(writer, "{}", self.colorize(rule.id().to_string(), "1;36"))?;

                writeln!(writer)?;
                // Keep first-file ordering without repeating a file heading above locations.
                for (index, finding) in findings_by_file(&findings)
                    .into_values()
                    .flatten()
                    .enumerate()
                {
                    if index > 0 {
                        writeln!(writer)?;
                    }
                    // Accepted debt remains visible with its original severity.
                    writeln!(
                        writer,
                        "  {}{} {}",
                        self.colorize(
                            format!("[{}]", severity_label(finding.severity)),
                            match finding.severity {
                                crate::model::Severity::Error => "31",
                                crate::model::Severity::Warning => "33",
                                crate::model::Severity::Off => "90",
                            },
                        ),
                        if finding.baselined {
                            " [baselined]"
                        } else {
                            ""
                        },
                        terminal_safe(&finding.message)
                    )?;
                    for (index, (location, title)) in
                        finding.locations.iter().zip(&finding.titles).enumerate()
                    {
                        writeln!(
                            writer,
                            "    {} {}  {}",
                            if index == 0 { "-->" } else { "related:" },
                            self.colorize(
                                format!(
                                    "{}:{}:{}",
                                    terminal_safe(&forward_slash_path(&location.path)),
                                    location.span.start.line,
                                    location.span.start.column,
                                ),
                                "94",
                            ),
                            terminal_safe(title)
                        )?;
                    }
                    writeln!(writer, "    action: {}", suggested_action(rule))?;
                }
                writeln!(writer)?;
            }
        }

        // `off`-severity findings still occupy a spot in `counts.total()` (a rule configured to
        // `off` still detected something), so they are reported explicitly here rather than
        // folded silently into the total: an off-only result would otherwise print
        // "0 errors, 0 warnings (1 finding total)", which is not honest about where that one
        // finding went missing from the breakdown.
        let counts = SeverityCounts::count(&result.findings);
        writeln!(
            writer,
            "{}",
            self.colorize(
                format!(
                    "Summary: {} {}, {} {}, {} off ({} {} total)",
                    counts.errors,
                    pluralize(counts.errors, "error"),
                    counts.warnings,
                    pluralize(counts.warnings, "warning"),
                    counts.off,
                    counts.total(),
                    pluralize(counts.total(), "finding")
                ),
                "90",
            )
        )?;
        if result.modes.suppressions_applied > 0 {
            // Its own line, same reason the baseline line is: the breakdown answers "what did
            // this run find", and this answers "what did the analyzed repository itself answer".
            // Findings removed by a directive are absent from the counts above by design -- the
            // count here is what stops that absence from reading as a clean bill of health the
            // reader cannot see the reasoning for (the reason lives next to the code).
            writeln!(
                writer,
                "{} {} suppressed by inline directive(s) -- the reason is stated next to the code",
                result.modes.suppressions_applied,
                pluralize(result.modes.suppressions_applied, "finding")
            )?;
        }
        if result.modes.baseline_applied {
            // Stated as its own line rather than folded into the severity breakdown above: the
            // breakdown answers "what did this run find", which a baseline does not change, and
            // this answers "what of that counts against you", which is the only thing it changes.
            writeln!(
                writer,
                "Baseline: {} accepted (not counted against the quality gate), {} new",
                result.modes.baselined, result.modes.new_findings
            )?;
        }
        writeln!(
            writer,
            "{}",
            self.colorize(format!("Tests analyzed: {}", result.tests_analyzed), "90")
        )?;
        writeln!(
            writer,
            "{}",
            self.colorize(format!("Files analyzed: {}", result.files_analyzed), "90")
        )?;
        Ok(())
    }
}

/// Tool-owned remedies preserve each rule's certainty, coverage and affected construct.
fn suggested_action(rule: Rule) -> &'static str {
    match rule {
        Rule::DuplicateTest => {
            "Review whether to remove a redundant test or repair a test whose body misses its named behavior."
        }
        Rule::DuplicateTestName => {
            "Rename tests to distinguish their intent while preserving all test bodies."
        }
        Rule::ParameterizationCandidate => {
            "Consolidate into a parameterized table while preserving every case and assertion."
        }
        Rule::NearDuplicateTest => {
            "Review equivalence before consolidating into a parameterized table or shared helper; preserve every case."
        }
        Rule::CrossFileDuplicate => {
            "Review whether duplication across files is intentional before consolidating or removing a copy."
        }
        Rule::SubsumedTest => {
            "Review whether the smaller test adds distinct coverage before changing either test."
        }
        Rule::DuplicateSuite => {
            "Review whether to remove the duplicated describe block or repair its intended coverage."
        }
        Rule::DuplicateEachRow => {
            "Review whether to remove a duplicate row while retaining every distinct case."
        }
        Rule::RedundantHook => {
            "Review whether to remove the nested hook; its ancestor already provides the same setup."
        }
    }
}

/// Pluralizes `noun` for `count`, the way a summary line reads naturally at both 0 and 1.
fn pluralize(count: usize, noun: &str) -> String {
    if count == 1 {
        noun.to_string()
    } else {
        format!("{noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{finding_with_title, location, rendered_result};
    use super::*;
    use crate::model::{AnalysisResult, IncompleteCauses, ModeSummary, Rule, Severity, Title};

    /// A writer that accepts `budget` bytes and then fails, so the `?` on each `writeln!` can be
    /// reached deliberately.
    ///
    /// These arms are not decoration: a report going to a closed pipe or a full disk must return
    /// the error rather than silently truncating, and `main` turns that into a non-zero exit. A
    /// `Vec<u8>` never fails, so nothing else in this module can observe them.
    struct FailingWriter {
        budget: usize,
    }

    impl Write for FailingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.budget == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "pipe closed",
                ));
            }
            let written = buf.len().min(self.budget);
            self.budget -= written;
            Ok(written)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A changed-files run that withheld nothing says so by omission.
    ///
    /// The withheld sentence is a claim about findings this report is NOT showing. Printing it
    /// when the count is zero would tell a reader the view is narrowed when it is not -- and a
    /// `--changed-since` run that withheld nothing is byte-for-byte a full run's findings.
    #[test]
    fn a_changed_run_that_withheld_nothing_omits_the_withheld_sentence() {
        let mut result = result_exercising_every_banner();
        result.modes.withheld_unchanged = 0;
        let output = render(&result);
        assert!(
            output.contains("CHANGED-FILES RUN"),
            "the scope banner still appears: {output}"
        );
        assert!(
            !output.contains("were withheld"),
            "but nothing claims findings were withheld: {output}"
        );
    }

    #[test]
    fn the_failing_writer_flushes() {
        // `Write` requires `flush`, and an uncalled one is dead code in this module's own test
        // body -- which llvm-cov counts, because it has no region-level notion of `#[cfg(test)]`.
        let mut writer = FailingWriter { budget: 4 };
        writer.write_all(b"ok").expect("within budget");
        writer.flush().expect("flush is infallible here");
    }

    /// The report a failing writer is driven against: one finding, an incomplete banner and a
    /// changed-files banner, so every `writeln!` group in `write_report` is on the path.
    fn result_exercising_every_banner() -> AnalysisResult {
        AnalysisResult {
            findings: vec![finding_with_title(
                Rule::DuplicateTest,
                "src/a.test.ts",
                "a",
            )],
            tests_analyzed: 2,
            files_analyzed: 1,
            incomplete_causes: IncompleteCauses {
                unresolved_contexts: 3,
                ..IncompleteCauses::default()
            },
            modes: ModeSummary {
                changed_scope: true,
                withheld_unchanged: 1,
                baseline_applied: true,
                baselined: 2,
                new_findings: 1,
                suppressions_applied: 0,
            },
            ..AnalysisResult::default()
        }
    }

    #[test]
    fn a_write_error_is_propagated_from_every_stage_of_the_report() {
        let result = result_exercising_every_banner();
        // Walk the budget across the whole report. Each value fails inside a different
        // `writeln!`, so this reaches the `?` arms one at a time rather than only the first.
        for no_color in [true, false] {
            let reporter = TerminalReporter { no_color };
            let mut full = Vec::new();
            reporter.write_report(&result, &mut full).unwrap();
            for budget in 0..full.len() {
                let error = reporter
                    .write_report(&result, &mut FailingWriter { budget })
                    .expect_err(&format!("a writer failing after {budget} bytes must error"));
                assert!(
                    error.to_string().contains("pipe closed"),
                    "the underlying I/O error must survive rather than be replaced: {error}"
                );
            }
        }
    }

    #[test]
    fn a_writer_with_room_for_the_whole_report_succeeds() {
        // The control. Without it the test above would pass just as well if `write_report`
        // returned `Err` unconditionally.
        let result = result_exercising_every_banner();
        let full = render(&result).len();
        TerminalReporter::new()
            .write_report(&result, &mut FailingWriter { budget: full })
            .expect("a writer with room for the whole report must succeed");
    }

    #[test]
    fn default_matches_new() {
        let result = result_exercising_every_banner();
        let mut from_default = Vec::new();
        TerminalReporter::default()
            .write_report(&result, &mut from_default)
            .expect("writing to a Vec<u8> never fails");
        assert_eq!(
            String::from_utf8(from_default).expect("utf-8"),
            render(&result)
        );
    }

    fn render(result: &AnalysisResult) -> String {
        let mut buffer = Vec::new();
        TerminalReporter::new()
            .write_report(result, &mut buffer)
            .expect("writing to a Vec<u8> never fails");
        String::from_utf8(buffer).expect("terminal reporter output must be valid UTF-8")
    }

    /// Only known tool-owned styles survive, and removing them recovers every plain-text byte.
    #[test]
    fn color_preserves_plain_text_and_sanitizes_untrusted_values() {
        let mut result = result_exercising_every_banner();
        result.modes.suppressions_applied = 1;
        result.findings[0].message = "hostile\x1b[2J\nmessage\u{202e}".into();
        result.findings[0].titles[0] = "hostile\x1b[0m\ntitle".into();
        result.findings[0].locations[0].path = "src/hostile\x1b[1;36m\n.test.ts".into();
        result.findings[0].baselined = true;
        let mut warning = finding_with_title(Rule::DuplicateTestName, "src/b.test.ts", "warning");
        warning.severity = Severity::Warning;
        let mut off = warning.clone();
        off.severity = Severity::Off;
        result.findings.extend([warning, off]);
        assert_eq!(result.findings.len(), 3);
        let mut buffer = Vec::new();
        TerminalReporter { no_color: false }
            .write_report(&result, &mut buffer)
            .unwrap();
        let styled = String::from_utf8(buffer).unwrap();
        assert!(styled.contains("\x1b[1;36mduplicate-test\x1b[0m"));
        assert!(styled.contains("\x1b[31m[error]\x1b[0m [baselined]"));
        assert!(styled.contains("\x1b[33m[warning]\x1b[0m"));
        assert!(styled.contains("\x1b[90m[off]\x1b[0m"));
        assert!(styled.contains("\x1b[94msrc/b.test.ts:20:3\x1b[0m"));
        assert!(styled.contains("\x1b[90mSummary:"));
        assert!(styled.contains("\x1b[90mFiles analyzed: 1\x1b[0m"));
        let mut plain = styled.clone();
        let mut openings = 0;
        for code in ["1;36", "31", "33", "90", "94"] {
            let escape = format!("\x1b[{code}m");
            openings += styled.matches(&escape).count();
            plain = plain.replace(&escape, "");
        }
        assert_eq!(openings, styled.matches("\x1b[0m").count());
        plain = plain.replace("\x1b[0m", "");
        assert!(!plain.contains('\x1b'), "untrusted escape survived");
        assert_eq!(plain, render(&result));
    }

    /// Final output must retain the distinct remedies and uncertainty of all nine rules.
    #[test]
    fn actions_follow_every_rule_contract_including_baseline_and_off() {
        use crate::model::Verdict;
        let cases: &[(Rule, &[&str])] = &[
            (
                Rule::DuplicateTest,
                &["redundant test", "repair", "named behavior"],
            ),
            (
                Rule::DuplicateTestName,
                &["Rename", "intent", "all test bodies"],
            ),
            (
                Rule::ParameterizationCandidate,
                &["parameterized table", "every case", "assertion"],
            ),
            (
                Rule::NearDuplicateTest,
                &[
                    "Review equivalence",
                    "parameterized table",
                    "preserve every case",
                ],
            ),
            (
                Rule::CrossFileDuplicate,
                &["across files", "intentional", "before"],
            ),
            (
                Rule::SubsumedTest,
                &["smaller test", "distinct coverage", "before changing"],
            ),
            (
                Rule::DuplicateSuite,
                &["describe block", "repair", "coverage"],
            ),
            (
                Rule::DuplicateEachRow,
                &["duplicate row", "every distinct case"],
            ),
            (
                Rule::RedundantHook,
                &["nested hook", "ancestor", "same setup"],
            ),
        ];
        assert_eq!(cases.len(), Rule::ALL.len());
        for rule in Rule::ALL {
            assert_eq!(
                cases
                    .iter()
                    .filter(|(candidate, _)| candidate == rule)
                    .count(),
                1
            );
        }
        for &(rule, required) in cases {
            for severity in [Severity::Error, Severity::Warning, Severity::Off] {
                for baselined in [false, true] {
                    let mut finding = finding_with_title(rule, "src/a.test.ts", "first");
                    finding.severity = severity;
                    finding.baselined = baselined;
                    let mut result = rendered_result(vec![finding]);
                    super::super::test_support::append_third_participant(&mut result);
                    result.findings[0].inert.push(false);
                    assert_eq!(result.findings.len(), 1);
                    assert_eq!(result.findings[0].locations.len(), 3);
                    let output = render(&result);
                    let actions: Vec<_> = output
                        .lines()
                        .filter_map(|line| line.strip_prefix("    action: "))
                        .collect();
                    assert_eq!(actions.len(), 1, "{rule:?}: {output}");
                    let action = actions[0];
                    for phrase in required {
                        assert!(
                            action.contains(phrase),
                            "{rule:?} missing {phrase}: {action}"
                        );
                    }
                    match rule.verdict() {
                        Verdict::Redundant | Verdict::SimilarUncertain => {
                            assert!(action.starts_with("Review"));
                        }
                        Verdict::NameCollision => assert!(action.starts_with("Rename")),
                        Verdict::Consolidatable => assert!(action.starts_with("Consolidate")),
                    }
                    if rule.verdict() != Verdict::Redundant {
                        assert!(
                            !action.contains("remov") && !action.contains("delet"),
                            "{rule:?} must not prescribe deletion: {action}"
                        );
                    }
                    assert!(
                        output.find("related: src/c.test.ts:30:1").unwrap()
                            < output.find("    action: ").unwrap()
                    );
                    assert_eq!(output.contains("[baselined]"), baselined);
                }
            }
        }
    }

    /// Pins whitespace and the version even when there are no finding blocks.
    #[test]
    fn layout_empty_report() {
        assert_eq!(
            render(&AnalysisResult::default()),
            concat!(
                "VitestDedup v",
                env!("CARGO_PKG_VERSION"),
                "\n\n",
                "No findings.\n\n",
                "Summary: 0 errors, 0 warnings, 0 off (0 findings total)\n",
                "Tests analyzed: 0\nFiles analyzed: 0\n"
            )
        );
    }

    /// A cross-file cluster retains every title and distinguishes its first location.
    #[test]
    fn layout_cross_file_cluster() {
        let mut result = super::super::test_support::cross_file_result("same body");
        super::super::test_support::append_third_participant(&mut result);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].locations.len(), 3);
        assert_eq!(result.findings[0].titles.len(), 3);
        assert_eq!(
            render(&result),
            concat!(
                "VitestDedup v",
                env!("CARGO_PKG_VERSION"),
                "\n\n",
                "duplicate-test\n\n",
                "  [error] same body\n",
                "    --> src/a.test.ts:10:3  suite > a\n",
                "    related: src/b.test.ts:20:5  suite > b\n",
                "    related: src/c.test.ts:30:1  suite > c\n",
                "    action: Review whether to remove a redundant test or repair a test whose body misses its named behavior.\n\n",
                "Summary: 1 error, 0 warnings, 0 off (1 finding total)\n",
                "Tests analyzed: 4\nFiles analyzed: 2\n"
            )
        );
    }

    /// Shuffled input keeps rule/file order and spacing across file boundaries, including debt.
    #[test]
    fn layout_multiple_rules_and_files_preserves_debt_and_off() {
        let mut debt = finding_with_title(Rule::DuplicateTest, "src/a.test.ts", "accepted");
        debt.baselined = true;
        debt.message = "accepted copy".into();
        let mut off = finding_with_title(Rule::DuplicateTest, "src/b.test.ts", "off test");
        off.severity = Severity::Off;
        off.message = "disabled severity".into();
        let mut warning = finding_with_title(Rule::DuplicateTestName, "src/a.test.ts", "collision");
        warning.severity = Severity::Warning;
        warning.message = "same title".into();
        let result = rendered_result(vec![warning, off, debt]);
        assert_eq!(result.findings.len(), 3);
        assert!(result
            .findings
            .iter()
            .all(|finding| finding.locations.len() == 2));
        assert_eq!(
            render(&result),
            concat!(
                "VitestDedup v",
                env!("CARGO_PKG_VERSION"),
                "\n\n",
                "duplicate-test\n\n",
                "  [error] [baselined] accepted copy\n",
                "    --> src/a.test.ts:10:3  accepted\n",
                "    related: src/a.test.ts:20:3  accepted (copy)\n",
                "    action: Review whether to remove a redundant test or repair a test whose body misses its named behavior.\n\n",
                "  [off] disabled severity\n",
                "    --> src/b.test.ts:10:3  off test\n",
                "    related: src/b.test.ts:20:3  off test (copy)\n",
                "    action: Review whether to remove a redundant test or repair a test whose body misses its named behavior.\n\n",
                "duplicate-test-name\n\n",
                "  [warning] same title\n",
                "    --> src/a.test.ts:10:3  collision\n",
                "    related: src/a.test.ts:20:3  collision (copy)\n",
                "    action: Rename tests to distinguish their intent while preserving all test bodies.\n\n",
                "Summary: 1 error, 1 warning, 1 off (3 findings total)\n",
                "Tests analyzed: 2\nFiles analyzed: 1\n"
            )
        );
    }

    #[test]
    fn empty_result_produces_a_clear_success_line_not_empty_output() {
        let output = render(&AnalysisResult::default());
        assert!(output.contains("No findings."));
        assert!(output.contains("0 errors, 0 warnings, 0 off (0 findings total)"));
        assert!(output.contains("Tests analyzed: 0"));
        assert!(output.contains("Files analyzed: 0"));
    }

    #[test]
    fn shows_severity_rule_message_and_every_location_with_full_title() {
        let result = rendered_result(vec![finding_with_title(
            Rule::DuplicateTest,
            "src/math.test.ts",
            "suite > adds two numbers",
        )]);
        let output = render(&result);
        assert!(output.contains("duplicate-test"));
        assert!(output.contains("[error]"));
        assert!(output.contains("identical bodies under an equivalent context"));
        assert!(output.contains("src/math.test.ts:10:3"));
        assert!(output.contains("src/math.test.ts:20:3"));
        assert!(output.contains("suite > adds two numbers"));
        assert!(output.contains("suite > adds two numbers (copy)"));
        assert!(output.contains("1 error, 0 warnings, 0 off (1 finding total)"));
    }

    #[test]
    fn groups_findings_by_rule_then_by_file_in_stable_order() {
        let result = rendered_result(vec![
            finding_with_title(Rule::DuplicateTestName, "src/b.test.ts", "b test"),
            finding_with_title(Rule::DuplicateTest, "src/a.test.ts", "a test"),
            finding_with_title(Rule::DuplicateTest, "src/b.test.ts", "another b test"),
        ]);
        let output = render(&result);
        let rule_pos = output.find("duplicate-test\n").unwrap();
        let name_pos = output.find("duplicate-test-name\n").unwrap();
        assert!(
            rule_pos < name_pos,
            "duplicate-test group must precede duplicate-test-name"
        );
        let a_pos = output.find("src/a.test.ts").unwrap();
        let b_pos = output.find("src/b.test.ts").unwrap();
        assert!(a_pos > rule_pos && a_pos < name_pos);
        assert!(b_pos > rule_pos && b_pos < name_pos);
        assert!(a_pos < b_pos, "files within a rule are ordered by path");
    }

    #[test]
    fn finding_with_more_than_two_locations_lists_all_of_them() {
        let mut finding = finding_with_title(Rule::DuplicateTest, "src/c.test.ts", "c test");
        finding.locations.push(location("src/c.test.ts", 30, 1));
        finding.titles.push("suite > c test (third)".to_string());
        let result = rendered_result(vec![finding]);
        let output = render(&result);
        assert!(output.contains("src/c.test.ts:10:3"));
        assert!(output.contains("src/c.test.ts:20:3"));
        assert!(output.contains("src/c.test.ts:30:1"));
        assert!(output.contains("suite > c test (third)"));
    }

    #[test]
    fn incomplete_flag_is_reported_prominently_before_findings() {
        let result = AnalysisResult {
            had_error_finding: false,
            error_finding_locations: 0,
            unresolved_tests: 0,
            blind_files: Vec::new(),
            findings: vec![finding_with_title(
                Rule::DuplicateTest,
                "src/a.test.ts",
                "a",
            )],
            tests_analyzed: 2,
            files_analyzed: 1,
            // The shape a real run produced: contexts that could not be resolved and files that
            // could not be read in full, and *neither* hard limit hit.
            incomplete_causes: IncompleteCauses {
                unresolved_contexts: 1729,
                files_partially_read: 12,
                ..IncompleteCauses::default()
            },
            modes: ModeSummary::default(),
        };
        let output = render(&result);
        assert!(output.contains("INCOMPLETE"));
        let incomplete_pos = output.find("INCOMPLETE").unwrap();
        let rule_pos = output.find("duplicate-test\n").unwrap();
        assert!(
            incomplete_pos < rule_pos,
            "the incomplete warning must appear before findings, not buried after them"
        );
        assert!(
            output.contains("1729 test(s) were excluded from comparison"),
            "the banner must name the cause that fired and how much it accounts for: {output}"
        );
        assert!(
            output.contains("12 file(s) could not be read in full"),
            "every cause that fired is named, not just the largest: {output}"
        );
        // The regression guard. The banner used to assert this unconditionally, so on a run like
        // this one -- where no limit was hit -- it told the reader the analyzer had been
        // overwhelmed by their repository. A reader who believes that stops looking for the
        // twelve files they could actually fix.
        assert!(
            !output.contains("hit a limit"),
            "a run that hit no limit must not claim it did: {output}"
        );
        assert!(
            !output.contains("budget ran out"),
            "the pair budget was not exhausted here, so nothing may say it was: {output}"
        );
    }

    #[test]
    fn sanitizes_newlines_and_ansi_escapes_in_titles_without_forging_lines() {
        let clean = AnalysisResult {
            had_error_finding: false,
            error_finding_locations: 0,
            unresolved_tests: 0,
            blind_files: Vec::new(),
            findings: vec![finding_with_title(
                Rule::DuplicateTest,
                "src/a.test.ts",
                "plaintitle",
            )],
            tests_analyzed: 2,
            files_analyzed: 1,
            incomplete_causes: IncompleteCauses::default(),
            modes: ModeSummary::default(),
        };
        let dirty = AnalysisResult {
            had_error_finding: false,
            error_finding_locations: 0,
            unresolved_tests: 0,
            blind_files: Vec::new(),
            findings: vec![finding_with_title(
                Rule::DuplicateTest,
                "src/a.test.ts",
                "plain\ntitle\x1b[31m!\x1b[0m",
            )],
            tests_analyzed: 2,
            files_analyzed: 1,
            incomplete_causes: IncompleteCauses::default(),
            modes: ModeSummary::default(),
        };
        let clean_output = render(&clean);
        let dirty_output = render(&dirty);
        assert!(!dirty_output.contains('\u{1b}'));
        // A raw newline or ESC inside a title must not add a line to the report: the sanitized
        // title merges into a single line rather than forging structure the rule didn't emit.
        assert_eq!(
            clean_output.lines().count(),
            dirty_output.lines().count(),
            "an embedded newline must not change the report's line count"
        );
    }

    #[test]
    fn very_long_title_is_included_in_full_without_corrupting_output() {
        let long_title = "x".repeat(5_000);
        let result = rendered_result(vec![finding_with_title(
            Rule::DuplicateTest,
            "src/a.test.ts",
            &long_title,
        )]);
        let output = render(&result);
        assert!(output.contains(&long_title));
    }

    #[test]
    fn non_ascii_paths_and_titles_render_correctly() {
        let result = rendered_result(vec![finding_with_title(
            Rule::DuplicateTest,
            "src/tëst-ünïcödé/a.test.ts",
            "suite > 日本語のタイトル",
        )]);
        let output = render(&result);
        assert!(output.contains("src/tëst-ünïcödé/a.test.ts"));
        assert!(output.contains("suite > 日本語のタイトル"));
    }

    #[test]
    fn off_only_result_reports_an_honest_total() {
        // R1: an off-only result must not print "0 errors, 0 warnings (1 finding total)" -- that
        // total would count a finding the breakdown never accounts for. The off count is now part
        // of the breakdown, so the numbers always add up.
        let mut finding = finding_with_title(Rule::DuplicateTest, "src/a.test.ts", "a");
        finding.severity = Severity::Off;
        let result = rendered_result(vec![finding]);
        let output = render(&result);
        assert!(output.contains("0 errors, 0 warnings, 1 off (1 finding total)"));
    }

    #[test]
    fn windows_style_backslashes_are_normalized_to_forward_slashes() {
        // R2: `docs/reports.md` states paths use forward slashes in every reporter's output; the
        // terminal reporter must agree with json/jsonl/html rather than emitting host-native
        // separators.
        let result = rendered_result(vec![finding_with_title(
            Rule::DuplicateTest,
            "src\\nested\\a.test.ts",
            "a test",
        )]);
        let output = render(&result);
        assert!(output.contains("src/nested/a.test.ts"));
        assert!(!output.contains("src\\nested\\a.test.ts"));
    }

    #[test]
    fn title_enum_display_is_available_for_future_suite_path_rendering() {
        // Guards the assumption terminal rendering leans on: `Title::display` never panics and
        // returns the underlying text regardless of variant.
        let titles = [
            Title::Static("static".to_string()),
            Title::Dynamic("dynamic".to_string()),
            Title::Parameterized("parameterized".to_string()),
        ];
        for title in titles {
            assert!(!title.display().is_empty());
        }
    }
}
