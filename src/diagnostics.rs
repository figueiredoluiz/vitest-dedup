//! CLI diagnostic rendering. No analysis, environment access or hardcoded output streams.
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use vitest_dedup::model::AnalysisResult;

/// A stderr line that is guaranteed to have been sanitized.
///
/// Sanitization used to be a *convention*: every site that pushed onto the diagnostics vector was
/// expected to remember `terminal_safe`, and every site that printed one was expected to know
/// whether that had already happened. This file held both habits at once -- the per-file
/// diagnostics sanitized on push and printed raw, the project-map diagnostics pushed raw and
/// sanitized on print -- which is indistinguishable from a bug when read locally, and is exactly
/// what a reviewer flagged the print site for. Either habit is one forgotten call away from
/// letting repository-controlled text forge or visually rewrite a stderr line.
///
/// Making it a type moves the guarantee from memory to the compiler. The wrapped `String` is
/// private to this module, `new` is the only way to build one, and `Display` is the only way to
/// read it back, so a `Sanitized` that skipped `terminal_safe` cannot be constructed anywhere in
/// this binary -- including by code written later, which is the case a convention cannot cover.
mod sanitized {
    use std::fmt;
    use vitest_dedup::reporters::terminal_safe;

    /// Sanitized, printable diagnostic text. Construct with [`Sanitized::new`].
    pub struct Sanitized(String);

    impl Sanitized {
        /// Sanitizes `raw` for terminal output. `raw` is assumed to be untrusted: a repository
        /// path, a config value quoted back, or frontend text carrying either.
        pub fn new(raw: &str) -> Self {
            Self(terminal_safe(raw))
        }
    }

    impl fmt::Display for Sanitized {
        /// Write the stored sanitized text to the formatter, propagating formatter errors.
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(&self.0)
        }
    }
}

pub use sanitized::Sanitized;

/// Sanitizes the operational error chain; the caller retains the exit-code decision.
pub fn write_error(writer: &mut dyn Write, error: &anyhow::Error) -> io::Result<()> {
    writeln!(
        writer,
        "vitest-dedup: {}",
        Sanitized::new(&format!("{error:#}"))
    )
}

const MAX_READ_ERRORS: usize = 20;
const MAX_EMPTY_FILES: usize = 10;

/// Prints read failures immediately, bounded independently of deferred extraction diagnostics.
pub fn write_read_error(
    writer: &mut dyn Write,
    count: &mut usize,
    message: &str,
) -> io::Result<()> {
    *count += 1;
    if *count <= MAX_READ_ERRORS {
        writeln!(writer, "vitest-dedup: {}", Sanitized::new(message))?;
    }
    Ok(())
}

/// Makes omitted read failures visible before report I/O can fail.
pub fn write_read_omissions(writer: &mut dyn Write, count: usize) -> io::Result<()> {
    if count > MAX_READ_ERRORS {
        writeln!(
            writer,
            "vitest-dedup: ... and {} more read failure(s)",
            count - MAX_READ_ERRORS
        )?;
    }
    Ok(())
}

/// Lists statically empty-table files without unbounded terminal output.
pub fn write_empty_files(
    writer: &mut dyn Write,
    files: &std::collections::BTreeSet<PathBuf>,
    root: &Path,
) -> io::Result<()> {
    for file in files.iter().take(MAX_EMPTY_FILES) {
        writeln!(
            writer,
            "  {}",
            Sanitized::new(&super::display_relative(file, root))
        )?;
    }
    if files.len() > MAX_EMPTY_FILES {
        writeln!(writer, "  ... and {} more", files.len() - MAX_EMPTY_FILES)?;
    }
    Ok(())
}

/// Called only after all selected file reports succeed and no reporter owns stdout.
pub fn write_reports_written(writer: &mut dyn Write, paths: &[PathBuf]) -> io::Result<()> {
    if !paths.is_empty() {
        writeln!(writer, "Reports written to:")?;
        for path in paths {
            writeln!(writer, "  {}", Sanitized::new(&path.display().to_string()))?;
        }
    }
    Ok(())
}

/// Owned-analysis facts borrowed for the post-report stderr qualification.
pub struct RunDiagnostics<'a> {
    pub result: &'a AnalysisResult,
    pub unresolved_tests: usize,
    pub total_tests: usize,
    pub changed: Option<(&'a str, usize)>,
    pub baseline: Option<&'a Path>,
    pub diagnostics: &'a [Sanitized],
    pub files_with_diagnostics: usize,
    pub census: &'a [(String, u64)],
}

/// Renders existing qualifications in order, preserving each list's bound and omission count.
pub fn write_run(writer: &mut dyn Write, context: &RunDiagnostics<'_>) -> io::Result<()> {
    let RunDiagnostics {
        result,
        unresolved_tests,
        total_tests,
        changed,
        baseline,
        diagnostics,
        files_with_diagnostics,
        census,
    } = *context;
    // Written to stderr, never stdout: a machine reporter owns stdout and must stay parseable.
    // Bounded, because a corpus with thousands of degraded files would otherwise bury the report
    // it is meant to qualify.
    // The remaining source of `incomplete` is a test whose ambient context could not be resolved.
    // Scattered across files, that is a property of individual tests and the actionable figure is
    // how much of the corpus it covers -- so this line counts rather than lists. Where the
    // exclusions are not scattered but account for EVERY test in a file, the count is no longer
    // enough, and the block below names those files: see its comment for why the two need
    // different treatment.
    if unresolved_tests > 0 {
        writeln!(
            writer,
            "\nvitest-dedup: {unresolved_tests} of {total_tests} test(s) had an unresolved \
             context and were excluded from comparison; absence of a finding for them proves \
             nothing."
        )?;
    }

    // Named, where the corpus-wide count above is only counted. A partially excluded file still
    // appears in the report and its findings carry the qualification with them; a wholly excluded
    // one produces nothing and so appears NOWHERE, which is indistinguishable from a file that was
    // compared and came back clean. The count cannot surface that -- 620 of 24,147 reads as a
    // rounding error until one notices it is every test in 35 particular files.
    //
    // Bounded like every other list this binary prints, and sorted worst-first by `analyze`, so a
    // truncated list still names the files that cost the most coverage.
    if !result.blind_files.is_empty() {
        const MAX_LISTED: usize = 10;
        let total_blind: usize = result.blind_files.iter().map(|file| file.tests).sum();
        writeln!(
            writer,
            "\nvitest-dedup: {} file(s) had EVERY test excluded ({total_blind} test(s) in \
             total). They produced no findings, so they appear nowhere above -- this report has \
             not cleared them:",
            result.blind_files.len()
        )?;
        for file in result.blind_files.iter().take(MAX_LISTED) {
            // Analyzed-repository text on a stream a caller may read: sanitized exactly as a
            // reporter sanitizes a finding's path.
            writeln!(
                writer,
                "  {} ({} test(s))",
                Sanitized::new(&file.path.display().to_string()),
                file.tests
            )?;
        }
        if let Some(remaining) = result.blind_files.len().checked_sub(MAX_LISTED) {
            if remaining > 0 {
                writeln!(writer, "  ... and {remaining} more")?;
            }
        }
    }

    if let Some((revision, changed)) = changed {
        // `revision` is argv, which is caller-controlled text; sanitized like every other value
        // echoed to a stream.
        writeln!(
            writer,
            "\nvitest-dedup: --changed-since {} scoped this report to {} changed file(s). The \
             whole corpus was still analyzed, so a cross-file duplicate with a twin in an \
             unchanged file is still reported.",
            Sanitized::new(revision),
            changed
        )?;
        if changed == 0 {
            writeln!(
                writer,
                "vitest-dedup: nothing changed since that revision, so no finding could be \
                 reported. This is not evidence the repository is clean."
            )?;
        }
        if result.modes.withheld_unchanged > 0 {
            writeln!(
                writer,
                "vitest-dedup: {} finding(s) lying entirely within unchanged files were withheld \
                 from this report; the run is marked incomplete because of it.",
                result.modes.withheld_unchanged
            )?;
        }
    }

    if let Some(path) = baseline {
        writeln!(
            writer,
            "\nvitest-dedup: baseline {} accounted for {} finding(s), which are reported but do \
             not affect the quality gate. {} finding(s) are not in it.",
            Sanitized::new(&path.display().to_string()),
            result.modes.baselined,
            result.modes.new_findings
        )?;
    }

    if !diagnostics.is_empty() {
        const MAX_SHOWN: usize = 20;
        // `diagnostics.len()` is a count of individual diagnostic messages, not of files -- one
        // file can contribute several (a skip, plus one entry per extraction-time issue), so the
        // two numbers disagree in general. `diagnostic_files` is the actual file count; a
        // discovery-level traversal error contributes to the former but not the latter, since it
        // is not attributable to one file.
        if files_with_diagnostics == 0 {
            writeln!(
                writer,
                "\nvitest-dedup: {} diagnostic(s) recorded during analysis:",
                diagnostics.len()
            )?;
        } else {
            writeln!(
                writer,
                "\nvitest-dedup: {} diagnostic(s) across {} file(s) could not be fully analyzed:",
                diagnostics.len(),
                files_with_diagnostics
            )?;
        }
        for diagnostic in diagnostics.iter().take(MAX_SHOWN) {
            writeln!(writer, "  {diagnostic}")?;
        }
        if diagnostics.len() > MAX_SHOWN {
            writeln!(writer, "  ... and {} more", diagnostics.len() - MAX_SHOWN)?;
        }
    }

    // T1's census (backlog): `VD_CELL_CENSUS=1` histograms why every unreadable `.each` cell was
    // refused, worst cause first. stderr and last, so it never interleaves with a machine
    // reporter's stdout. Empty -- and free -- on every run without the variable.
    if !census.is_empty() {
        writeln!(
            writer,
            "\nvitest-dedup: unreadable `.each` cell census (VD_CELL_CENSUS):"
        )?;
        for (cause, count) in census {
            writeln!(writer, "  {count:6}  {}", Sanitized::new(cause))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vitest_dedup::model::BlindFile;

    struct FailingWriter(usize);
    impl Write for FailingWriter {
        /// Consume the remaining test-writer byte budget, returning an error once no bytes can
        /// be accepted.
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0 == 0 {
                return Err(io::Error::other("writer failed"));
            }
            let written = bytes.len().min(self.0);
            self.0 -= written;
            Ok(written)
        }
        /// Treat flushing the budgeted test writer as a successful no-op.
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Assert a renderer emits nonempty output and propagates writer errors at every smaller
    /// byte budget.
    fn assert_writer_failures(render: impl Fn(&mut dyn Write) -> io::Result<()>) {
        let mut full = Vec::new();
        render(&mut full).unwrap();
        assert!(!full.is_empty());
        for budget in 0..full.len() {
            assert!(
                render(&mut FailingWriter(budget)).is_err(),
                "budget {budget}"
            );
        }
    }

    /// Check terminal diagnostics: qualifications preserve counts bounds conditions and safety.
    #[test]
    fn qualifications_preserve_counts_bounds_conditions_and_safety() {
        for count in [0, 9, 10, 11, 19, 20, 21] {
            let mut result = AnalysisResult {
                blind_files: (0..count)
                    .map(|i| BlindFile {
                        path: PathBuf::from(format!("blind{i}\u{202e}.test.ts")),
                        tests: 2,
                    })
                    .collect(),
                ..AnalysisResult::default()
            };
            result.modes.withheld_unchanged = 2;
            result.modes.baselined = 1;
            result.modes.new_findings = 3;
            let messages: Vec<_> = (0..count)
                .map(|i| Sanitized::new(&format!("message{i}\n\x1b[31m")))
                .collect();
            let census = vec![("identifier\n\u{202e}".to_owned(), 7)];
            let context = RunDiagnostics {
                result: &result,
                unresolved_tests: 3,
                total_tests: 8,
                changed: Some(("HEAD\n\x1b[31m", 0)),
                baseline: Some(Path::new("base\u{202e}.json")),
                diagnostics: &messages,
                files_with_diagnostics: 2,
                census: &census,
            };
            let mut bytes = Vec::new();
            write_run(&mut bytes, &context).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.contains("3 of 8 test(s) had an unresolved context"));
            assert!(
                text.contains("--changed-since HEAD[31m scoped this report to 0 changed file(s)")
            );
            assert!(text.contains("nothing changed since that revision"));
            assert!(text.contains("2 finding(s) lying entirely within unchanged files"));
            assert!(text.contains("baseline base.json accounted for 1 finding(s)"));
            assert!(text.contains("3 finding(s) are not in it"));
            assert!(text.contains("     7  identifier"));
            assert_eq!(
                text.lines()
                    .filter(|line| line.starts_with("  blind"))
                    .count(),
                count.min(10)
            );
            assert_eq!(
                text.lines()
                    .filter(|line| line.starts_with("  message"))
                    .count(),
                count.min(20)
            );
            if count > 0 {
                assert!(text.contains(&format!(
                    "{count} file(s) had EVERY test excluded ({} test(s)",
                    count * 2
                )));
                assert!(text.contains(&format!("{count} diagnostic(s) across 2 file(s)")));
            }
            if count > 10 {
                assert!(text.contains(&format!("... and {} more", count - 10)));
            }
            if count > 20 {
                assert!(text.contains(&format!("... and {} more", count - 20)));
            }
            assert!(!text.contains(['\x1b', '\u{202e}']));
            assert_writer_failures(|writer| write_run(writer, &context));
        }
    }

    /// Check terminal diagnostics: empty qualifications are silent and traversal messages do
    /// not invent file counts.
    #[test]
    fn empty_qualifications_are_silent_and_traversal_messages_do_not_invent_file_counts() {
        let result = AnalysisResult::default();
        let mut context = RunDiagnostics {
            result: &result,
            unresolved_tests: 0,
            total_tests: 0,
            changed: None,
            baseline: None,
            diagnostics: &[],
            files_with_diagnostics: 0,
            census: &[],
        };
        let mut bytes = Vec::new();
        write_run(&mut bytes, &context).unwrap();
        write_reports_written(&mut bytes, &[]).unwrap();
        write_empty_files(&mut bytes, &Default::default(), Path::new(".")).unwrap();
        write_read_omissions(&mut bytes, 0).unwrap();
        assert!(bytes.is_empty());
        let messages = [Sanitized::new("walk failed")];
        context.diagnostics = &messages;
        context.changed = Some(("HEAD", 2));
        write_run(&mut bytes, &context).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("1 diagnostic(s) recorded during analysis"));
        assert!(
            !text.contains("across")
                && !text.contains("nothing changed")
                && !text.contains("withheld")
        );
    }

    /// Check terminal diagnostics: early lists are bounded and omissions are exact.
    #[test]
    fn early_lists_are_bounded_and_omissions_are_exact() {
        for count in [0, 9, 10, 11, 19, 20, 21] {
            let mut bytes = Vec::new();
            let mut recorded = 0;
            for _ in 0..count {
                write_read_error(&mut bytes, &mut recorded, "skipping bad\n\x1b[31m").unwrap();
            }
            write_read_omissions(&mut bytes, recorded).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            assert_eq!(recorded, count);
            assert_eq!(
                text.lines()
                    .filter(|line| *line == "vitest-dedup: skipping bad[31m")
                    .count(),
                count.min(20)
            );
            assert_eq!(text.contains("more read failure(s)"), count > 20);
            if count > 20 {
                assert!(text.contains(&format!("... and {} more read failure(s)", count - 20)));
            }
            let files = (0..count)
                .map(|i| PathBuf::from(format!("root/empty{i}.test.ts")))
                .collect();
            let mut bytes = Vec::new();
            write_empty_files(&mut bytes, &files, Path::new("root")).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            assert_eq!(
                text.lines()
                    .filter(|line| line.starts_with("  empty"))
                    .count(),
                count.min(10)
            );
            assert_eq!(text.contains("... and"), count > 10);
            if count > 10 {
                assert!(text.contains(&format!("... and {} more", count - 10)));
            }
        }
    }

    /// Check terminal diagnostics: early errors lists and announcements propagate writer
    /// failures.
    #[test]
    fn early_errors_lists_and_announcements_propagate_writer_failures() {
        let error = anyhow::anyhow!("repository\n\x1b[31m\u{202e}").context("loading config");
        let mut bytes = Vec::new();
        write_error(&mut bytes, &error).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "vitest-dedup: loading config: repository[31m\n"
        );
        assert_writer_failures(|writer| write_error(writer, &error));
        assert_writer_failures(|writer| write_read_error(writer, &mut 0, "skipping unreadable"));
        assert_writer_failures(|writer| write_read_omissions(writer, 25));
        let files = (0..12)
            .map(|i| PathBuf::from(format!("empty{i}.test.ts")))
            .collect();
        assert_writer_failures(|writer| write_empty_files(writer, &files, Path::new(".")));
        let paths = [PathBuf::from("reports/naïve\n\x1b[31m\u{202e}.json")];
        assert_writer_failures(|writer| write_reports_written(writer, &paths));
        let mut bytes = Vec::new();
        write_reports_written(&mut bytes, &paths).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "Reports written to:\n  reports/naïve[31m.json\n"
        );
    }
}
