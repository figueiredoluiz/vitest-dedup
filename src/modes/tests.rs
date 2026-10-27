mod git_context;

use super::*;
use crate::model::{Location, Position, Rule, Severity, Span};

/// Build a mode-test location with a deterministic line-based byte span.
fn location(path: &str, line: u32) -> Location {
    Location {
        path: PathBuf::from(path),
        span: Span {
            start: Position { line, column: 3 },
            end: Position { line, column: 8 },
            start_byte: line * 100,
            end_byte: line * 100 + 5,
        },
    }
}

/// Build a mode-filter finding with participant provenance, default severity and no initial
/// baseline flag.
fn finding(rule: Rule, participants: &[(&str, u32, &str)]) -> Finding {
    Finding {
        registration_files: Vec::new(),
        rule,
        severity: rule.default_severity(),
        message: "identical bodies under an equivalent context".to_owned(),
        locations: participants
            .iter()
            .map(|(path, line, _)| location(path, *line))
            .collect(),
        titles: participants
            .iter()
            .map(|(_, _, title)| (*title).to_owned())
            .collect(),
        inert: participants.iter().map(|_| false).collect(),
        baselined: false,
    }
}

/// Build a duplicate-test finding with two participants ten lines apart.
fn pair(path: &str, title: &str, line: u32) -> Finding {
    finding(
        Rule::DuplicateTest,
        &[(path, line, title), (path, line + 10, title)],
    )
}

// -- round trip ------------------------------------------------------------------------

/// Regression fixture for baseline/change filtering: a written baseline reads back and
/// matches the findings it recorded.
#[test]
fn a_written_baseline_reads_back_and_matches_the_findings_it_recorded() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested/vitest-dedup-baseline.json");
    let mut findings = vec![
        pair("src/a.test.ts", "suite > a", 10),
        pair("src/b.test.ts", "suite > b", 10),
    ];

    let update = write_baseline(&path, &findings).unwrap();
    assert_eq!(update.recorded, 2);
    assert_eq!(update.added, 2);
    assert_eq!(update.removed, 0);

    let baseline = load_baseline(&path).unwrap();
    assert_eq!(baseline.total(), 2);
    let (baselined, new) = apply_baseline(&mut findings, &baseline);
    assert_eq!((baselined, new), (2, 0));
    assert!(findings.iter().all(|finding| finding.baselined));
}

/// Regression fixture for baseline/change filtering: the written file is sorted
/// deterministic and newline terminated.
#[test]
fn the_written_file_is_sorted_deterministic_and_newline_terminated() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("baseline.json");
    let findings = vec![
        pair("src/z.test.ts", "suite > z", 10),
        pair("src/a.test.ts", "suite > a", 10),
    ];
    write_baseline(&path, &findings).unwrap();
    let first = std::fs::read_to_string(&path).unwrap();

    // Same findings, opposite order in: the file must be byte-identical, or a baseline would
    // churn in review every time analysis reordered anything.
    let reversed: Vec<Finding> = findings.iter().rev().cloned().collect();
    write_baseline(&path, &reversed).unwrap();
    assert_eq!(first, std::fs::read_to_string(&path).unwrap());
    assert!(first.ends_with("}\n"));

    let parsed: BaselineFile = serde_json::from_str(&first).unwrap();
    let fingerprints: Vec<&String> = parsed
        .findings
        .iter()
        .map(|entry| &entry.fingerprint)
        .collect();
    let mut sorted = fingerprints.clone();
    sorted.sort();
    assert_eq!(fingerprints, sorted, "entries must be fingerprint-sorted");
    assert_eq!(parsed.fingerprint_version, FINGERPRINT_VERSION);
    assert_eq!(parsed.schema_version, BASELINE_SCHEMA_VERSION);
}

/// Regression fixture for baseline/change filtering: a baseline survives a finding moving
/// within its file.
#[test]
fn a_baseline_survives_a_finding_moving_within_its_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("baseline.json");
    write_baseline(&path, &[pair("src/a.test.ts", "suite > a", 10)]).unwrap();

    // Same finding, 200 lines further down: the fingerprint is location-independent, so the
    // baseline must still match it.
    let mut moved = vec![pair("src/a.test.ts", "suite > a", 210)];
    let baseline = load_baseline(&path).unwrap();
    assert_eq!(apply_baseline(&mut moved, &baseline), (1, 0));
}

// -- the collision case ----------------------------------------------------------------

#[test]
fn one_fingerprint_can_legitimately_cover_several_findings_and_is_counted_not_deduplicated() {
    // `Finding::fingerprint`'s documented collision: a run of more than two tests sharing one
    // copy-pasted title is emitted as a spanning set of pairwise findings, and two of those
    // pairs carry the identical (path, title) participant set. A baseline keyed on
    // fingerprint *uniqueness* accepts one and reports the other as new forever.
    let path = "src/copy-paste.test.ts";
    let title = "suite > renders correctly";
    let first = finding(Rule::DuplicateTest, &[(path, 10, title), (path, 20, title)]);
    let second = finding(Rule::DuplicateTest, &[(path, 30, title), (path, 40, title)]);
    assert_eq!(
        first.fingerprint(),
        second.fingerprint(),
        "sanity: this test is meaningless unless these genuinely collide"
    );
    assert_ne!(first.locations, second.locations);

    let directory = tempfile::tempdir().unwrap();
    let baseline_path = directory.path().join("baseline.json");
    let mut findings = vec![first, second];
    let update = write_baseline(&baseline_path, &findings).unwrap();
    assert_eq!(update.recorded, 2, "both occurrences must be recorded");

    let file: BaselineFile =
        serde_json::from_str(&std::fs::read_to_string(&baseline_path).unwrap()).unwrap();
    assert_eq!(file.findings.len(), 1, "one fingerprint");
    assert_eq!(file.findings[0].count, 2, "two occurrences of it");

    let baseline = load_baseline(&baseline_path).unwrap();
    assert_eq!(
        apply_baseline(&mut findings, &baseline),
        (2, 0),
        "both colliding findings must be accepted, not just the first"
    );
}

#[test]
fn adding_a_third_occurrence_of_an_accepted_fingerprint_is_still_new() {
    // The other half of counting: an allowance of 2 must not silently accept a 3rd copy.
    let path = "src/copy-paste.test.ts";
    let title = "suite > renders correctly";
    let make = |a: u32, b: u32| finding(Rule::DuplicateTest, &[(path, a, title), (path, b, title)]);

    let directory = tempfile::tempdir().unwrap();
    let baseline_path = directory.path().join("baseline.json");
    write_baseline(&baseline_path, &[make(10, 20), make(30, 40)]).unwrap();

    let baseline = load_baseline(&baseline_path).unwrap();
    let mut grown = vec![make(10, 20), make(30, 40), make(50, 60)];
    assert_eq!(apply_baseline(&mut grown, &baseline), (2, 1));
    assert!(!grown[2].baselined, "the third copy is new debt");
}

// -- rejecting a baseline that cannot be trusted -----------------------------------------

#[test]
fn a_stale_or_malformed_baseline_is_an_error_never_a_silent_empty_baseline() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("baseline.json");

    let cases: [(&str, &str); 5] = [
        ("not json at all", "failed to parse baseline"),
        (
            r#"{"schemaVersion":99,"fingerprintVersion":"vitestDedupFingerprint/v1","findings":[]}"#,
            "schema version",
        ),
        (
            r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v0","findings":[]}"#,
            "fingerprint version",
        ),
        (
            r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v1","findings":[{"fingerprint":"vitestDedupFingerprint/v1:aaaa","count":0}]}"#,
            "0 occurrences",
        ),
        (
            r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v1","findings":[{"fingerprint":"vitestDedupFingerprint/v1:aaaa","count":1},{"fingerprint":"vitestDedupFingerprint/v1:aaaa","count":1}]}"#,
            "same fingerprint twice",
        ),
    ];
    for (content, expected) in cases {
        std::fs::write(&path, content).unwrap();
        let error = load_baseline(&path).unwrap_err();
        let text = format!("{error:#}");
        assert!(
            text.contains(expected),
            "expected {expected:?} in {text:?} for {content}"
        );
    }
}

/// The two bounds on an untrusted baseline file, each checked at the size that crosses it
/// rather than comfortably past it. Both must be operational errors: a baseline whose input
/// is unknown cannot be allowed to resolve to "suppress nothing" (a wall of apparently-new
/// findings) or "suppress everything" (a silent pass).
///
/// The entry-count file is kept under the byte limit on purpose, so it is the entry limit and
/// not the byte limit that rejects it -- the two guards are otherwise indistinguishable from
/// the outside, and removing the entry limit would leave this file parsing happily.
#[test]
fn an_oversized_or_overlong_baseline_is_rejected_by_the_bound_it_actually_crosses() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("baseline.json");

    let mut oversized = String::from("[");
    oversized.push_str(&" ".repeat(usize::try_from(MAX_BASELINE_BYTES).unwrap()));
    assert!(oversized.len() as u64 > MAX_BASELINE_BYTES);
    std::fs::write(&path, &oversized).unwrap();
    let error = format!("{:#}", load_baseline(&path).unwrap_err());
    assert!(
        error.contains(&format!("exceeds the {MAX_BASELINE_BYTES} byte limit")),
        "{error}"
    );

    let mut overlong = String::from(
        r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v1","findings":["#,
    );
    for index in 0..=MAX_BASELINE_ENTRIES {
        if index > 0 {
            overlong.push(',');
        }
        overlong.push_str(r#"{"fingerprint":"","count":1}"#);
    }
    overlong.push_str("]}");
    assert!(
        (overlong.len() as u64) < MAX_BASELINE_BYTES,
        "the entry-limit fixture must stay under the byte limit, or it proves the wrong guard"
    );
    std::fs::write(&path, &overlong).unwrap();
    let error = format!("{:#}", load_baseline(&path).unwrap_err());
    assert!(
        error.contains(&format!("past the {MAX_BASELINE_ENTRIES} entry limit")),
        "{error}"
    );
}

/// A file-level `fingerprintVersion` that matches says nothing about the entries under it: a
/// three-way merge can leave a `v0` entry inside a `v1` file. Such an entry can never match a
/// finding this build emits, so it would silently accept nothing while looking like accepted
/// debt -- an allowance that quietly does nothing is worse than an error.
#[test]
fn a_baseline_entry_whose_fingerprint_is_not_a_versioned_value_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("baseline.json");
    std::fs::write(
        &path,
        r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v1","findings":[{"fingerprint":"cukeDedupFingerprint/v2:aaaa","count":1}]}"#,
    )
    .unwrap();
    let error = format!("{:#}", load_baseline(&path).unwrap_err());
    assert!(
        error.contains(&format!("not a `{FINGERPRINT_VERSION}` value")),
        "{error}"
    );
}

/// `is_empty` is the question "does this baseline accept anything at all", and a baseline is
/// measured by the total instances it accepts, not by how many distinct fingerprints it
/// lists. A `count`-blind implementation would call a file holding one fingerprint with
/// `count: 3` a baseline of one.
#[test]
fn a_baseline_is_empty_only_when_it_accepts_no_instance_at_all() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("baseline.json");

    std::fs::write(
        &path,
        r#"{"schemaVersion":1,"fingerprintVersion":"vitestDedupFingerprint/v1","findings":[]}"#,
    )
    .unwrap();
    let empty = load_baseline(&path).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.total(), 0);
    assert!(Baseline::default().is_empty());

    write_baseline(&path, &[pair("src/a.test.ts", "suite > a", 10)]).unwrap();
    let recorded = load_baseline(&path).unwrap();
    assert!(!recorded.is_empty());
    assert_eq!(recorded.total(), 1);

    // One fingerprint, three accepted instances: the total counts instances, not keys.
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace(r#""count": 1"#, r#""count": 3"#);
    std::fs::write(&path, text).unwrap();
    let multiple = load_baseline(&path).unwrap();
    assert!(!multiple.is_empty());
    assert_eq!(multiple.total(), 3);
}

#[test]
fn annotation_fields_are_sanitized_and_bounded_on_the_way_into_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("baseline.json");
    let hostile = format!("a\u{202e}b\nc{}", "x".repeat(500));
    write_baseline(&path, &[pair("src/a.test.ts", &hostile, 10)]).unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(!raw.contains('\u{202e}'), "bidi override reached the file");
    let file: BaselineFile = serde_json::from_str(&raw).unwrap();
    for title in &file.findings[0].tests {
        assert!(!title.contains('\n'));
        assert!(title.chars().count() <= MAX_BASELINE_TITLE_CHARS);
    }
    assert_eq!(file.findings[0].rule, "duplicate-test");
}

#[test]
fn rewriting_reports_what_it_added_and_removed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("baseline.json");
    write_baseline(
        &path,
        &[
            pair("src/a.test.ts", "suite > a", 10),
            pair("src/b.test.ts", "suite > b", 10),
        ],
    )
    .unwrap();

    // `b` was fixed, `c` appeared.
    let update = write_baseline(
        &path,
        &[
            pair("src/a.test.ts", "suite > a", 10),
            pair("src/c.test.ts", "suite > c", 10),
        ],
    )
    .unwrap();
    assert_eq!(update.recorded, 2);
    assert_eq!(update.added, 1);
    assert_eq!(update.removed, 1);
}

// -- changed-file filtering ---------------------------------------------------------------

#[test]
fn a_finding_is_kept_when_any_participant_is_in_a_changed_file() {
    let mut findings = vec![
        // Cross-file: only the *second* participant was touched. Keying on the primary
        // location alone would drop this, which is the half of cross-file-duplicate a
        // changed-files run most needs to see.
        finding(
            Rule::CrossFileDuplicate,
            &[
                ("src/untouched.test.ts", 10, "a"),
                ("src/touched.test.ts", 10, "b"),
            ],
        ),
        pair("src/elsewhere.test.ts", "suite > x", 10),
    ];
    let changed = BTreeSet::from([PathBuf::from("src/touched.test.ts")]);
    let withheld = retain_changed_findings(&mut findings, &changed);
    assert_eq!(withheld, 1);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule, Rule::CrossFileDuplicate);
}

#[test]
fn severity_and_inert_flags_do_not_affect_baseline_identity() {
    // A config-only change (lowering a rule's severity) must not re-earn every baseline entry.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("baseline.json");
    write_baseline(&path, &[pair("src/a.test.ts", "suite > a", 10)]).unwrap();

    let mut lowered = vec![pair("src/a.test.ts", "suite > a", 10)];
    lowered[0].severity = Severity::Warning;
    lowered[0].inert = vec![true, false];
    let baseline = load_baseline(&path).unwrap();
    assert_eq!(apply_baseline(&mut lowered, &baseline), (1, 0));
}

// -- git -----------------------------------------------------------------------------------

/// Create a Git command with inherited repository environment removed for isolated fixture
/// operations.
fn fixture_git() -> Command {
    let mut command = Command::new("git");
    for variable in INHERITED_GIT_VARIABLES {
        command.env_remove(variable);
    }
    command
}

/// Initialize the fixture main branch, configure a test identity and commit its current
/// files.
fn init_repository(root: &Path) {
    for arguments in [
        ["init", "-q", "-b", "main"].as_slice(),
        ["config", "user.email", "test@example.com"].as_slice(),
        ["config", "user.name", "Test"].as_slice(),
        ["add", "."].as_slice(),
        ["commit", "-qm", "initial"].as_slice(),
    ] {
        assert!(fixture_git()
            .args(arguments)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
}

/// Changed files are reported relative to the analysis root, untracked files included.
#[test]
fn changed_files_are_relative_to_the_root_and_include_untracked_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir_all(root.join("packages/app/src")).unwrap();
    std::fs::write(root.join("packages/app/src/tracked.test.ts"), "before").unwrap();
    std::fs::write(root.join("packages/app/src/untouched.test.ts"), "same").unwrap();
    init_repository(root);
    std::fs::write(root.join("packages/app/src/tracked.test.ts"), "after").unwrap();
    std::fs::write(root.join("packages/app/src/fresh.test.ts"), "new").unwrap();

    let scoped = root.join("packages/app");
    let changed = changed_files_since(&scoped, "HEAD").unwrap();
    assert_eq!(
        changed,
        BTreeSet::from([
            PathBuf::from("src/fresh.test.ts"),
            PathBuf::from("src/tracked.test.ts"),
        ]),
        "paths must be relative to the analysis root, matching a Finding's location path"
    );
}

/// A revision that does not resolve is an error naming `--changed-since`, never an empty set.
#[test]
fn a_missing_revision_is_an_error_not_an_empty_changed_set() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("a.test.ts"), "x").unwrap();
    init_repository(directory.path());

    for revision in ["no-such-ref", "", "  ", "--all"] {
        let error = changed_files_since(directory.path(), revision).unwrap_err();
        let text = format!("{error:#}");
        assert!(
            text.contains("--changed-since"),
            "{revision:?} produced {text:?}"
        );
    }
}

/// Reads `root`'s committed test files the way `--baseline-from-ref` does, as sorted
/// `(path, content)` pairs.
fn read_revision(root: &Path) -> Vec<(String, String)> {
    let mut sources: Vec<_> = collect_sources(root, "HEAD", &["**/*.test.ts".to_owned()], &[])
        .unwrap()
        .into_iter()
        .map(|source| {
            (
                source.relative.to_string_lossy().into_owned(),
                source.content,
            )
        })
        .collect();
    sources.sort();
    sources
}

/// The base read returns exactly the committed text of every admitted file, whatever its
/// name, and skips files the classifier or globs decline.
#[test]
fn revision_reads_return_the_committed_text_of_admitted_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut files = vec![
        ("a.test.ts", "test('a', () => {});\n"),
        ("-dash.test.ts", "test('dash', () => {});\n"),
        ("with space.test.ts", "test('space', () => {});\n"),
        ("nested/b.test.ts", "test('b', () => {});\n"),
    ];
    if cfg!(unix) {
        files.push(("line\nbreak.test.ts", "test('newline', () => {});\n"));
    }
    for (path, content) in &files {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }
    std::fs::write(root.join("notes.md"), "not source").unwrap();
    std::fs::write(root.join("helper.ts"), "export const x = 1;").unwrap();
    init_repository(root);
    std::fs::write(root.join("a.test.ts"), "changed in the working tree").unwrap();

    let mut expected: Vec<_> = files
        .iter()
        .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
        .collect();
    expected.sort();
    assert_eq!(read_revision(root), expected);
}

/// An analysis root below the git root reads its own subtree with root-relative paths.
#[test]
fn revision_reads_resolve_paths_from_an_analysis_root_below_the_git_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::write(root.join("pkg/x.test.ts"), "test('x', () => {});\n").unwrap();
    std::fs::write(root.join("x.test.ts"), "test('outer', () => {});\n").unwrap();
    init_repository(root);
    assert_eq!(
        read_revision(&root.join("pkg")),
        vec![("x.test.ts".to_owned(), "test('x', () => {});\n".to_owned())]
    );
}

/// Oversized and non-UTF-8 blobs are skipped; the other files are still read.
#[test]
fn revision_reads_skip_oversized_and_non_utf8_blobs() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::write(root.join("ok.test.ts"), "test('ok', () => {});\n").unwrap();
    std::fs::write(root.join("bytes.test.ts"), [0xff, 0xfe, 0x00]).unwrap();
    let limit = usize::try_from(MAX_REVISION_SOURCE_BYTES).unwrap();
    std::fs::write(root.join("huge.test.ts"), "x".repeat(limit + 1)).unwrap();
    init_repository(root);
    assert_eq!(
        read_revision(root),
        vec![(
            "ok.test.ts".to_owned(),
            "test('ok', () => {});\n".to_owned()
        )]
    );
}

/// A child that floods stderr cannot block: the drain keeps a bounded prefix and reads the
/// rest, so the child exits and `wait()` returns. The watchdog turns a regression into a
/// failure instead of a hung test run.
#[cfg(unix)]
#[test]
fn a_flooding_stderr_is_drained_so_the_child_can_exit() {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut child = Command::new("sh")
            .args(["-c", "head -c 300000 /dev/zero | tr '\\0' x >&2"])
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = drain_stderr(child.stderr.take().unwrap());
        let status = child.wait().unwrap();
        let _ = done.send((status.success(), stderr.join().unwrap().len()));
    });
    let (success, kept) = finished
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the child exited instead of blocking on a full stderr pipe");
    assert!(success);
    assert_eq!(kept, usize::try_from(MAX_KEPT_STDERR_BYTES).unwrap());
}

/// A failed batch read names the object, and aborting reaps the git process instead of
/// leaving it running. The watchdog turns a cleanup regression (which would otherwise block
/// on the live child) into a failure rather than a hung test run.
#[test]
fn a_failed_blob_read_is_an_error_and_abort_reaps_git() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("a.test.ts"), "x").unwrap();
    init_repository(directory.path());
    let root = directory.path().to_path_buf();
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BlobReader::spawn(&root).unwrap();
        let missing = "0123456789012345678901234567890123456789";
        let error = format!("{:#}", reader.read(missing).unwrap_err());
        reader.stop();
        let reaped = matches!(reader.child.try_wait(), Ok(Some(_)));
        let _ = done.send((error.contains(missing), reaped));
        let _ = reader.abort();
    });
    let (named, reaped) = finished
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the read failed and the process was stopped without blocking");
    assert!(named, "the error names the missing object");
    assert!(reaped, "the git process has exited and been reaped");
}

/// The base read costs a fixed number of git processes, not one per file (P5): spawning one
/// `cat-file` per file made `--baseline-from-ref` take seconds on ordinary repositories.
#[test]
fn revision_reads_spawn_a_fixed_number_of_git_processes() {
    let spawns_for = |count: usize| {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..count {
            std::fs::write(
                directory.path().join(format!("t{index}.test.ts")),
                format!("test('{index}', () => {{}});\n"),
            )
            .unwrap();
        }
        init_repository(directory.path());
        let before = GIT_SPAWNS.with(std::cell::Cell::get);
        assert_eq!(read_revision(directory.path()).len(), count);
        GIT_SPAWNS.with(std::cell::Cell::get) - before
    };
    assert_eq!(
        spawns_for(2),
        spawns_for(30),
        "git processes must not grow with files"
    );
}

/// Changed-file mode outside a Git repository is an error, not an empty set.
#[test]
fn a_directory_outside_any_repository_is_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let error = changed_files_since(directory.path(), "HEAD").unwrap_err();
    assert!(
        format!("{error:#}").contains("requires a Git repository"),
        "{error:#}"
    );
}

#[test]
fn a_shallow_clone_is_an_error_naming_the_fix() {
    let origin = tempfile::tempdir().unwrap();
    std::fs::write(origin.path().join("a.test.ts"), "first").unwrap();
    init_repository(origin.path());
    std::fs::write(origin.path().join("a.test.ts"), "second").unwrap();
    assert!(fixture_git()
        .args(["commit", "-qam", "second"])
        .current_dir(origin.path())
        .status()
        .unwrap()
        .success());

    let clone = tempfile::tempdir().unwrap();
    let target = clone.path().join("shallow");
    assert!(fixture_git()
        .args(["clone", "-q", "--depth", "1", "--no-local"])
        .arg(format!("file://{}", origin.path().display()))
        .arg(&target)
        .status()
        .unwrap()
        .success());

    let error = changed_files_since(&target, "HEAD").unwrap_err();
    let text = format!("{error:#}");
    assert!(text.contains("shallow clone"), "{text}");
    assert!(text.contains("fetch-depth: 0"), "{text}");
}

/// A git invocation that fails must be an operational error that says which listing failed
/// and carries git's own diagnosis. The alternative -- treating a failed listing as "nothing
/// changed" -- filters every finding away and reports a clean run over a repository nobody
/// looked at, the exact false clean `--changed-since` is documented never to produce.
#[test]
fn a_failing_git_listing_is_an_error_naming_the_listing_and_quoting_git() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("a.test.ts"), "x").unwrap();
    init_repository(directory.path());

    let error = git_paths(
        directory.path(),
        &["ls-files", "--no-such-option", "-z", "--"],
        "listing untracked files for --changed-since",
    )
    .unwrap_err();
    let text = format!("{error:#}");
    assert!(
        text.contains("git failed while listing untracked files for --changed-since"),
        "the message must name what was being attempted: {text}"
    );
    assert!(
        text.contains("no-such-option"),
        "git's own stderr must survive into the message: {text}"
    );
}

// -- the base-revision read ----------------------------------------------------------------

/// A committed repository holding exactly `files`, parents created as needed.
fn repository_with(files: &[(&str, &[u8])]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    for (name, bytes) in files {
        let path = directory.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    init_repository(directory.path());
    directory
}

/// `revision_sources` at `HEAD` with the given admission globs.
fn base_read(root: &Path, include: &[&str], exclude: &[&str]) -> Result<Vec<RevisionSource>> {
    let owned = |globs: &[&str]| {
        globs
            .iter()
            .map(|glob| (*glob).to_owned())
            .collect::<Vec<_>>()
    };
    collect_sources(root, "HEAD", &owned(include), &owned(exclude))
}

/// Every source a base read yields, collected in listing order.
fn collect_sources(
    root: &Path,
    revision: &str,
    include: &[String],
    exclude: &[String],
) -> Result<Vec<RevisionSource>> {
    let mut sources = Vec::new();
    revision_sources(root, revision, include, exclude, |source| {
        sources.push(source);
        Ok(true)
    })?;
    Ok(sources)
}

/// The refusal text of a base read that must fail, as the user would see it.
fn base_read_error(root: &Path, include: &[&str], exclude: &[&str]) -> String {
    let error = base_read(root, include, exclude).err();
    format!("{:#}", error.expect("the base read must be refused"))
}

/// Admission is the classifier AND the globs: a non-source file and an excluded source file
/// are both skipped, silently, and only the admitted paths come back, relative to the root.
#[test]
fn a_base_read_admits_only_source_files_the_globs_allow() {
    let repository = repository_with(&[
        ("a.test.ts", b"one"),
        ("notes.md", b"not source"),
        ("skipped.test.ts", b"excluded by glob"),
        ("nested/b.test.ts", b"two"),
    ]);
    let sources = base_read(repository.path(), &["**/*.test.ts"], &["skipped.*"]).unwrap();
    let relatives: BTreeSet<_> = sources.iter().map(|s| s.relative.clone()).collect();
    assert_eq!(
        relatives,
        BTreeSet::from([
            PathBuf::from("a.test.ts"),
            PathBuf::from("nested/b.test.ts")
        ])
    );
    assert_eq!(sources.len(), 2, "no source is listed twice");
}

/// The base pass admits what discovery admits: a single `*` stays within one directory in
/// include and exclude alike, `**` recurses, and a leading `./` is stripped.
#[test]
fn a_base_read_admits_what_discovery_admits() {
    let repository = repository_with(&[("src/a.test.ts", b"a"), ("src/deep/b.test.ts", b"b")]);
    let root = repository.path();
    let admitted = |include: &[&str], exclude: &[&str]| -> BTreeSet<PathBuf> {
        base_read(root, include, exclude)
            .unwrap()
            .into_iter()
            .map(|source| source.relative)
            .collect()
    };
    let direct = BTreeSet::from([PathBuf::from("src/a.test.ts")]);
    let nested = BTreeSet::from([PathBuf::from("src/deep/b.test.ts")]);
    let both: BTreeSet<_> = direct.union(&nested).cloned().collect();
    assert_eq!(admitted(&["src/*.test.ts"], &[]), direct);
    assert_eq!(admitted(&["src/**/*.test.ts"], &[]), both);
    assert_eq!(admitted(&["./src/**/*.test.ts"], &[]), both);
    assert_eq!(admitted(&["src/**/*.test.ts"], &["src/*.test.ts"]), nested);
}

/// A blob over the byte cap and a blob that is not UTF-8 each cost a skip, never an error
/// and never the rest of the read: the neighbouring file is still returned.
#[test]
fn an_oversized_or_non_utf8_base_blob_is_skipped_and_the_rest_is_read() {
    let oversized = vec![b'a'; MAX_REVISION_SOURCE_BYTES as usize + 1];
    let repository = repository_with(&[
        ("big.test.ts", oversized.as_slice()),
        ("binary.test.ts", &[0xff, 0xfe, 0xfd]),
        ("ok.test.ts", b"fine"),
    ]);
    let sources = base_read(repository.path(), &["**/*.test.ts"], &[]).unwrap();
    let [only] = &sources[..] else {
        panic!("exactly the readable file survives, got {}", sources.len());
    };
    assert_eq!(only.relative, PathBuf::from("ok.test.ts"));
    assert_eq!(only.content, "fine");
}

/// A tree entry whose blob cannot be read (a submodule is listed by `ls-tree -r` as a commit,
/// not a blob) is an operational error naming the file and quoting git, not a silent gap in
/// the base report that would make the gate more lenient than it claims.
#[test]
fn an_unreadable_base_entry_is_an_error_naming_the_file_and_quoting_git() {
    let repository = repository_with(&[("a.test.ts", b"x")]);
    let root = repository.path();
    let head = fixture_git()
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .unwrap();
    let commit = String::from_utf8(head.stdout).unwrap().trim().to_owned();
    for arguments in [
        vec![
            "update-index",
            "--add",
            "--cacheinfo",
            "160000",
            &commit,
            "vendor.test.ts",
        ],
        vec!["commit", "-qm", "gitlink"],
    ] {
        assert!(fixture_git()
            .args(arguments)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
    let text = base_read_error(root, &["**/*.test.ts"], &[]);
    assert!(
        text.contains("git failed while reading base file `vendor.test.ts`"),
        "{text}"
    );
}

/// A visitor that stops reads no further blob, and a non-blob entry past the stop still fails
/// the read: stopping early must not make the gate more lenient.
#[test]
fn a_stopped_base_read_reads_no_further_blob_but_still_refuses_a_gitlink() {
    let repository = repository_with(&[
        ("a.test.ts", b"a"),
        ("b.test.ts", b"b"),
        ("c.test.ts", b"c"),
    ]);
    let root = repository.path();
    let include = ["**/*.test.ts".to_owned()];
    let before = BLOB_READS.with(std::cell::Cell::get);
    let mut visited = Vec::new();
    let unread = revision_sources(root, "HEAD", &include, &[], |source| {
        visited.push(source.relative);
        Ok(false)
    })
    .unwrap();
    assert_eq!(visited, [PathBuf::from("a.test.ts")]);
    assert_eq!(unread, 2);
    assert_eq!(BLOB_READS.with(std::cell::Cell::get) - before, 1);

    let head = fixture_git()
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .unwrap();
    let commit = String::from_utf8(head.stdout).unwrap().trim().to_owned();
    for arguments in [
        vec![
            "update-index",
            "--add",
            "--cacheinfo",
            "160000",
            &commit,
            "z.test.ts",
        ],
        vec!["commit", "-qm", "gitlink"],
    ] {
        assert!(fixture_git()
            .args(arguments)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
    let stopped = revision_sources(root, "HEAD", &include, &[], |_| Ok(false));
    assert!(
        format!("{:#}", stopped.expect_err("the gitlink is still refused")).contains("`z.test.ts`")
    );
}

/// Each refusal before any file is read, under the flag's own name: an invalid glob in either
/// list, a revision that is not a commit, and a directory outside any repository.
#[test]
fn a_base_read_refuses_bad_input_before_reading_anything() {
    let repository = repository_with(&[("a.test.ts", b"x")]);
    let root = repository.path();
    let text = base_read_error(root, &["["], &[]);
    assert!(text.contains("invalid include glob pattern `[`"), "{text}");
    assert!(text.contains("base revision read"), "{text}");
    let text = base_read_error(root, &["**/*.test.ts"], &["{"]);
    assert!(text.contains("invalid exclude glob pattern `{`"), "{text}");

    let missing = collect_sources(root, "no-such-ref", &[], &[]).err();
    let text = format!(
        "{:#}",
        missing.expect("an unresolvable revision is refused")
    );
    assert!(
        text.contains("--baseline-from-ref revision `no-such-ref` does not resolve"),
        "{text}"
    );

    let outside = tempfile::tempdir().unwrap();
    let text = format!(
        "{:#}",
        ensure_not_shallow(outside.path(), "--baseline-from-ref").unwrap_err()
    );
    assert!(text.contains("could not determine whether"), "{text}");
}

/// A baseline whose parent directory cannot be created is a named error, not a panic and not
/// a silent no-op: here the would-be parent is an existing regular file.
#[test]
fn a_baseline_that_cannot_create_its_directory_is_an_error_naming_it() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("blocker"), "a file, not a directory").unwrap();
    let path = directory.path().join("blocker/sub/baseline.json");
    let text = format!("{:#}", write_baseline(&path, &[]).unwrap_err());
    assert!(text.contains("creating baseline directory"), "{text}");
}

/// A blob disappearing or corrupting after the listing is an error, with partial reads rejected.
#[test]
fn a_base_blob_changed_after_listing_is_an_error_naming_the_unread_file() {
    for corrupt in [false, true] {
        let repository = repository_with(&[("a.test.ts", b"first"), ("z.test.ts", b"second")]);
        let root = repository.path();
        let object = fixture_git()
            .args(["rev-parse", "HEAD:z.test.ts"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(object.status.success());
        let oid = String::from_utf8(object.stdout).unwrap();
        let oid = oid.trim();
        let loose = root.join(".git/objects").join(&oid[..2]).join(&oid[2..]);
        #[cfg(windows)]
        {
            let mut permissions = std::fs::metadata(&loose).unwrap().permissions();
            permissions.set_readonly(false);
            std::fs::set_permissions(&loose, permissions).unwrap();
        }
        let mut visited = Vec::new();
        let error = revision_sources(root, "HEAD", &["**/*.test.ts".to_owned()], &[], |source| {
            visited.push(source.relative);
            if corrupt {
                // Remove first: loose objects may be read-only on Windows.
                std::fs::remove_file(&loose).unwrap();
                std::fs::write(&loose, b"not a compressed git object").unwrap();
            } else {
                std::fs::remove_file(&loose).unwrap();
            }
            Ok(true)
        })
        .unwrap_err();
        assert_eq!(visited, vec![PathBuf::from("a.test.ts")]);
        let text = format!("{error:#}");
        assert!(
            text.contains("git failed while reading base file `z.test.ts`"),
            "{text}"
        );
        assert!(text.contains("unexpected git cat-file answer"), "{text}");
        if corrupt {
            assert!(
                text.contains("error:"),
                "git's corruption diagnostic must survive: {text}"
            );
        }
    }
}

/// A deterministic child speaks only the selected cat-file response, after reading one request.
#[cfg(unix)]
fn protocol_reader(response: &str, diagnostic: &str, status: i32) -> BlobReader {
    let mut child = Command::new("sh")
        .args([
            "-c",
            "read request; printf '%s' \"$2\" >&2; printf '%s' \"$1\"; exit \"$3\"",
            "owned-cat-file",
        ])
        .arg(response)
        .arg(diagnostic)
        .arg(status.to_string())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let input = child.stdin.take();
    let output = std::io::BufReader::new(child.stdout.take().unwrap());
    let stderr = Some(drain_stderr(child.stderr.take().unwrap()));
    BlobReader {
        child,
        input,
        output,
        stderr,
    }
}

/// EOF, malformed lengths, truncation and missing framing must never become accepted blobs.
#[cfg(unix)]
#[test]
fn malformed_batch_responses_are_errors_and_are_reaped() {
    for (response, message) in [
        ("", "ended before answering"),
        ("object blob nope\n", "invalid digit"),
        ("object blob 3\nx", "failed to fill whole buffer"),
        ("object blob 3\nabc!", "not newline-terminated"),
    ] {
        let mut reader = protocol_reader(response, "owned diagnostic", 0);
        let text = format!("{:#}", reader.read("owned-object").unwrap_err());
        assert!(text.contains(message), "{text}");
        assert_eq!(reader.abort(), "owned diagnostic");
    }
    let mut reader = protocol_reader("object blob 3\nabc\n", "", 0);
    assert_eq!(reader.read("owned-object").unwrap(), b"abc");
    reader.finish().unwrap();
}

/// A closed request stream and a failed child exit are operational errors, even without blobs.
#[cfg(unix)]
#[test]
fn a_closed_batch_input_or_failed_exit_is_an_error() {
    let mut reader = protocol_reader("", "", 0);
    reader.input = None;
    assert!(reader
        .read("owned-object")
        .unwrap_err()
        .to_string()
        .contains("input is closed"));
    reader.abort();
    let reader = protocol_reader("", "owned failure", 7);
    let text = format!("{:#}", reader.finish().unwrap_err());
    assert!(text.contains("git failed while reading"), "{text}");
    assert!(text.contains("owned failure"), "{text}");
}

/// A commit whose tree is unavailable must fail both revision modes instead of yielding no files.
#[test]
fn revision_modes_reject_a_commit_with_a_missing_tree() {
    use std::io::Write;
    let repository = repository_with(&[("a.test.ts", b"first")]);
    let root = repository.path();
    let commit = fixture_git()
        .args(["cat-file", "commit", "HEAD"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(commit.status.success());
    let commit = String::from_utf8(commit.stdout).unwrap();
    let (tree, body) = commit.split_once('\n').unwrap();
    let width = tree.strip_prefix("tree ").unwrap().len();
    let damaged = format!("tree {}\n{body}", "0".repeat(width));
    let mut child = fixture_git()
        .args(["hash-object", "-t", "commit", "-w", "--stdin"])
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(damaged.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let oid = String::from_utf8(output.stdout).unwrap();
    let mut visits = 0;
    let text = format!(
        "{:#}",
        revision_sources(root, oid.trim(), &["**/*.test.ts".to_owned()], &[], |_| {
            visits += 1;
            Ok(true)
        })
        .unwrap_err()
    );
    assert_eq!(visits, 0);
    assert!(
        text.contains("git failed while listing the base revision's files"),
        "{text}"
    );
    assert!(text.contains("not a tree object"), "{text}");
    let text = format!("{:#}", changed_files_since(root, oid.trim()).unwrap_err());
    assert!(text.contains("listing files changed since"), "{text}");
}

/// Baseline creation makes nested directories and records the exact accepted finding count.
#[test]
fn baseline_creation_supports_nested_missing_directories() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing/nested/baseline.json");
    let finding = finding(
        Rule::DuplicateTest,
        &[("a.test.ts", 1, "a"), ("a.test.ts", 2, "b")],
    );
    let update = write_baseline(&path, &[finding]).unwrap();
    assert_eq!((update.recorded, update.added, update.removed), (1, 1, 0));
    assert_eq!(load_baseline(&path).unwrap().total(), 1);
}

/// OS-rejected paths remain operational errors at each Git boundary, before any child is started.
#[test]
fn git_process_boundaries_report_os_refusals() {
    let root = Path::new("invalid\0root");
    let error = format!(
        "{:#}",
        git_paths(root, &["status"], "listing owned paths").unwrap_err()
    );
    assert!(
        error.contains("failed to run git while listing owned paths"),
        "{error}"
    );
    let error = format!(
        "{:#}",
        ensure_git_repository(root, "--baseline-from-ref").unwrap_err()
    );
    assert!(error.contains("--baseline-from-ref"), "{error}");
    let error = format!(
        "{:#}",
        ensure_not_shallow(root, "--baseline-from-ref").unwrap_err()
    );
    assert!(
        error.contains("failed to determine whether the repository is a shallow clone"),
        "{error}"
    );
    let error = format!(
        "{:#}",
        resolve_commit(root, "HEAD", "--baseline-from-ref").unwrap_err()
    );
    assert!(
        error.contains("failed to resolve the --baseline-from-ref revision"),
        "{error}"
    );
    let error = match BlobReader::spawn(root) {
        Ok(mut reader) => {
            reader.stop();
            panic!("an OS-rejected path cannot start git")
        }
        Err(error) => format!("{error:#}"),
    };
    assert!(
        error.contains("git failed to start reading the base revision's files"),
        "{error}"
    );
}

/// Reading an absent baseline and writing over a directory fail with the selected path named.
#[test]
fn baseline_read_and_write_errors_name_the_destination_and_preserve_it() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing-baseline.json");
    let error = format!("{:#}", load_baseline(&missing).unwrap_err());
    assert!(error.contains("reading baseline"), "{error}");
    assert!(error.contains("missing-baseline.json"), "{error}");
    let blocked = directory.path().join("blocked-baseline.json");
    std::fs::create_dir(&blocked).unwrap();
    std::fs::write(blocked.join("sentinel"), b"preserve").unwrap();
    let error = format!("{:#}", write_baseline(&blocked, &[]).unwrap_err());
    assert!(error.contains("writing baseline"), "{error}");
    assert!(error.contains("blocked-baseline.json"), "{error}");
    assert_eq!(
        std::fs::read(blocked.join("sentinel")).unwrap(),
        b"preserve"
    );
}

/// A consumer failure stops revision reads and propagates instead of returning a partial success.
#[test]
fn a_revision_visitor_error_is_propagated_after_the_first_source() {
    let repository = repository_with(&[("a.test.ts", b"first"), ("z.test.ts", b"second")]);
    let mut visited = Vec::new();
    let error = revision_sources(
        repository.path(),
        "HEAD",
        &["**/*.test.ts".to_owned()],
        &[],
        |source| {
            visited.push(source.relative);
            anyhow::bail!("owned visitor failure")
        },
    )
    .unwrap_err();
    assert_eq!(visited, vec![PathBuf::from("a.test.ts")]);
    assert_eq!(error.to_string(), "owned visitor failure");
}

/// A batch process that dies before a request cannot yield an empty successful source.
#[test]
fn a_terminated_batch_child_is_an_error_and_is_reaped() {
    let repository = repository_with(&[("a.test.ts", b"first")]);
    let mut reader = BlobReader::spawn(repository.path()).unwrap();
    reader.child.kill().unwrap();
    let status = reader.child.wait().unwrap();
    assert!(!status.success());
    assert!(reader
        .read("0123456789012345678901234567890123456789")
        .is_err());
    assert!(matches!(reader.child.try_wait(), Ok(Some(_))));
    reader.abort();
}

/// A base tree with no admitted sources completes without calling the visitor or reading a blob.
#[test]
fn an_all_excluded_base_tree_reads_no_sources() {
    let repository = repository_with(&[("a.test.ts", b"first"), ("README.md", b"notes")]);
    let before = BLOB_READS.with(std::cell::Cell::get);
    let mut visited = 0;
    let unread = revision_sources(
        repository.path(),
        "HEAD",
        &["**/*.test.ts".to_owned()],
        &["**/*.test.ts".to_owned()],
        |_| {
            visited += 1;
            Ok(true)
        },
    )
    .unwrap();
    assert_eq!((unread, visited), (0, 0));
    assert_eq!(BLOB_READS.with(std::cell::Cell::get), before);
}

/// A syntactically valid pattern that exceeds the matcher's resource limit is a named error.
#[test]
fn a_glob_compilation_resource_refusal_is_an_operational_error() {
    // 512 KiB of input stays below the config byte limit, but exceeds globset's 10 MiB NFA cap.
    let pattern = "a?".repeat(256 * 1024);
    let error = match compile_revision_globs(&[pattern], "include") {
        Ok(_) => panic!("the oversized matcher must be refused"),
        Err(error) => format!("{error:#}"),
    };
    assert!(
        error.contains("compiling include globs for the base revision read"),
        "{error}"
    );
    assert!(error.contains("error building NFA"), "{error}");
}
