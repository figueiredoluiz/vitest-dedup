//! Committed object contents, path isolation and retained catalog bounds.
use super::*;
use crate::modes::tests::{fixture_git, repository_with};

/// Historical bytes and ancestor metadata survive present changes, deletion and new commits.
#[test]
fn immutable_catalog_reads_only_its_pinned_objects() {
    let dir = repository_with(&[("tsconfig.json", b"{}"), ("nested/helper.ts", b"old")]);
    let root = dir.path().canonicalize().unwrap();
    let files = RevisionFiles::load(&root.join("nested"), "HEAD", None).unwrap();
    let commit = files.commit().to_owned();
    let before = files.retained_bytes();
    let owned = files
        .files
        .iter()
        .map(|(p, e)| p.capacity() + e.object.capacity())
        .sum::<usize>()
        + files
            .directories
            .iter()
            .map(|p| p.capacity())
            .sum::<usize>();
    assert_eq!(files.owned, owned);
    assert_eq!(files.kind(&root), Some(SourceKind::Directory));
    assert_eq!(
        files.kind(&root.join("tsconfig.json")),
        Some(SourceKind::File)
    );
    std::fs::write(root.join("nested/helper.ts"), "present").unwrap();
    std::fs::remove_file(root.join("tsconfig.json")).unwrap();
    std::fs::write(root.join("only-present.ts"), "present").unwrap();
    assert!(fixture_git()
        .current_dir(&root)
        .args(["add", "."])
        .status()
        .unwrap()
        .success());
    assert!(fixture_git()
        .current_dir(&root)
        .args(["commit", "-qm", "test(fixture): change live source"])
        .status()
        .unwrap()
        .success());
    assert_eq!(files.read(&root.join("nested/helper.ts")).unwrap(), b"old");
    assert_eq!(files.read(&root.join("tsconfig.json")).unwrap(), b"{}");
    assert_eq!(files.commit(), commit);
    let stdout = files
        .reader
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .output
        .capacity();
    assert_eq!(
        files.retained_bytes(),
        before + stdout + 4 * super::super::MAX_KEPT_STDERR_BYTES as usize
    );
    assert_eq!(files.kind(&root.join("only-present.ts")), None);
    assert!(files.read(&root.join("only-present.ts")).is_err());
    assert_eq!(
        files
            .canonicalize(&root.join("nested/../tsconfig.json"))
            .unwrap(),
        lexical(&root.join("tsconfig.json")).unwrap()
    );
    assert!(files.canonicalize(&root.join("../outside")).is_err());
    assert!(files.regular_root(&root.join("nested")).is_err());
    files.regular_root(&root.join("nested/helper.ts")).unwrap();
    files.ensure_readable().unwrap();
    files.reject("bad\x1b[31m config".to_owned());
    files.reject("second".to_owned());
    let error = files.ensure_readable().unwrap_err().to_string();
    assert!(error.contains("bad"));
    assert!(!error.contains('\x1b'));
    assert!(!error.contains("second"));
}

/// Tiny budgets stop metadata growth, and untrusted structural records cannot enter the view.
#[test]
fn catalog_bounds_and_structural_validation_fail_closed() {
    let dir = repository_with(&[("helper.ts", b"old")]);
    let root = dir.path().canonicalize().unwrap();
    assert!(RevisionFiles::load(&root, "HEAD", Some(1))
        .unwrap_err()
        .to_string()
        .contains("memory budget"));
    assert!(RevisionFiles::load(&root, "HEAD", Some(800)).is_err());
    let mut files = RevisionFiles::load(&root, "HEAD", None).unwrap();
    for record in [
        b"bad".as_slice(),
        b"100644 blob bad 2\tx",
        b"100644 blob 0000000000000000000000000000000000000000 2\t../x",
        b"100644 blob 0000000000000000000000000000000000000000 2\t",
        b"x\t\xff",
    ] {
        assert!(files.insert(record, None).is_err(), "{record:?}");
    }
    assert!(files
        .insert(
            b"100644 blob 0000000000000000000000000000000000000000 2\thelper.ts",
            None
        )
        .is_err());
    for (mode, kind, expected) in [
        ("160000", "commit", SourceKind::Other),
        ("120000", "blob", SourceKind::Symlink),
    ] {
        let record = format!(
            "{mode} {kind} 0000000000000000000000000000000000000000 {}\t{mode}",
            if kind == "blob" { "3" } else { "-" }
        );
        files.insert(record.as_bytes(), None).unwrap();
        assert_eq!(files.kind(&root.join(mode)), Some(expected));
        assert!(files.regular_root(&root.join(mode)).is_err());
    }
    assert!(lexical(Path::new("relative")).is_none());
    assert!(lexical(Path::new("/../../escape")).is_none());
}

/// Oversized and nonregular dependency entries are rejected before starting a content process.
#[test]
fn catalog_rejects_nonregular_and_oversized_dependencies() {
    for (kind, size) in [
        (SourceKind::File, MAX_REVISION_SOURCE_BYTES + 1),
        (SourceKind::Symlink, 3),
        (SourceKind::Other, 0),
    ] {
        let dir = repository_with(&[("helper.ts", b"old")]);
        let root = dir.path().canonicalize().unwrap();
        let mut files = RevisionFiles::load(&root, "HEAD", None).unwrap();
        let entry = files
            .files
            .get_mut(&lexical(&root.join("helper.ts")).unwrap())
            .unwrap();
        entry.kind = kind;
        entry.size = Some(size);
        assert!(files.read(&root.join("helper.ts")).is_err());
        assert!(files.reader.lock().unwrap().is_none());
        assert!(files.ensure_readable().is_err());
        if kind != SourceKind::File {
            assert!(files.canonicalize(&root.join("helper.ts")).is_err());
        }
    }
}

/// An absent immutable object is a recorded operational failure, never empty helper source.
#[test]
fn unavailable_objects_do_not_turn_into_empty_source() {
    let dir = repository_with(&[("helper.ts", b"old")]);
    let root = dir.path().canonicalize().unwrap();
    let mut files = RevisionFiles::load(&root, "HEAD", None).unwrap();
    files
        .files
        .get_mut(&lexical(&root.join("helper.ts")).unwrap())
        .unwrap()
        .object = "0".repeat(40);
    assert!(files.read(&root.join("helper.ts")).is_err());
    assert!(files
        .ensure_readable()
        .unwrap_err()
        .to_string()
        .contains("unexpected git cat-file answer"));
}

/// BAD sizes are read prerequisites; unrelated unavailable blobs do not invalidate the index.
#[test]
fn unavailable_blob_metadata_is_deferred_until_read() {
    let dir = repository_with(&[("helper.ts", b"old")]);
    let root = dir.path().canonicalize().unwrap();
    let mut files = RevisionFiles::load(&root, "HEAD", None).unwrap();
    files
        .insert(
            b"100644 blob 0000000000000000000000000000000000000000 BAD\tunrelated.bin",
            None,
        )
        .unwrap();
    assert_eq!(
        files.kind(&root.join("unrelated.bin")),
        Some(SourceKind::File)
    );
    assert!(files.reader.lock().unwrap().is_none());
    files.ensure_readable().unwrap();
    let error = files
        .read(&root.join("unrelated.bin"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("not available in local Git objects"),
        "{error}"
    );
    assert!(files.reader.lock().unwrap().is_none());
    assert!(files.ensure_readable().is_err());
    assert!(files
        .insert(
            b"100644 blob 0000000000000000000000000000000000000000 nonsense\tbad",
            None
        )
        .is_err());
}

/// Non-UTF-8 names cannot be virtual lookup keys, but their structural header is still checked.
#[test]
fn unqueryable_names_are_ignored_after_header_validation() {
    let dir = repository_with(&[("helper.ts", b"old")]);
    let root = dir.path().canonicalize().unwrap();
    let mut files = RevisionFiles::load(&root, "HEAD", None).unwrap();
    let count = files.files.len();
    files
        .insert(
            b"100644 blob 0000000000000000000000000000000000000000 2\t\xff.bin",
            None,
        )
        .unwrap();
    assert_eq!(files.files.len(), count);
    assert!(files.insert(b"invalid metadata\t\xff.bin", None).is_err());
    files.ensure_readable().unwrap();
}

/// Project names are cached once, scoped to project source, and included in the heap ledger.
#[test]
fn project_package_uncertainty_is_scoped_cached_and_accounted() {
    let dir = repository_with(&[
        ("project/pkg/package.json", br#"{"name":"@acme/runner"}"#),
        ("outside/package.json", br#"{"name":"outside"}"#),
        (
            "project/node_modules/ignored/package.json",
            br#"{"name":"ignored"}"#,
        ),
    ]);
    let root = dir.path().canonicalize().unwrap();
    let files = RevisionFiles::load(&root.join("project"), "HEAD", None).unwrap();
    files.unresolved_package("outside");
    assert!(!files.configuration_degraded());
    let cache = files.project_packages.lock().unwrap();
    let names = cache.as_ref().unwrap();
    assert_eq!(names.names.len(), 1);
    assert!(names.names.contains("@acme/runner"));
    assert_eq!(
        names.owned,
        names.names.iter().map(String::capacity).sum::<usize>()
    );
    drop(cache);
    let bytes = files.retained_bytes();
    files.unresolved_package("ignored");
    assert_eq!(bytes, files.retained_bytes());
    assert!(!files.configuration_degraded());
    files.unresolved_package("@acme/runner");
    assert!(files.configuration_degraded());
    files.unresolved_package("another");
    assert_eq!(bytes, files.retained_bytes());
    files.ensure_readable().unwrap();
}

/// Failed or over-budget auxiliary metadata is uncertainty, not a fatal unrelated file read.
#[test]
fn uncertain_project_metadata_never_claims_a_complete_package_catalog() {
    for body in [
        b"{bad".as_slice(),
        b"[]",
        br#"{"name":false}"#,
        br#"{"name":""}"#,
        br#"{"name":"valid"}"#,
    ] {
        let dir = repository_with(&[("pkg/package.json", body)]);
        let root = dir.path().canonicalize().unwrap();
        let mut files = RevisionFiles::load(&root, "HEAD", None).unwrap();
        if body == br#"{"name":"valid"}"# {
            files
                .files
                .get_mut(&lexical(&root.join("pkg/package.json")).unwrap())
                .unwrap()
                .size = None;
        }
        files.unresolved_package("anything");
        assert!(files.configuration_degraded());
        files.ensure_readable().unwrap();
    }
    let dir = repository_with(&[
        ("a/package.json", br#"{"name":"same"}"#),
        ("b/package.json", br#"{"name":"same"}"#),
        ("plain/package.json", b"{}"),
    ]);
    let root = dir.path().canonicalize().unwrap();
    let mut files = RevisionFiles::load(&root, "HEAD", None).unwrap();
    for (path, entry) in &mut files.files {
        if path.file_name().unwrap() == "package.json" {
            entry.size = Some(MAX_REVISION_SOURCE_BYTES / 2 + 1);
        }
    }
    files.unresolved_package("anything");
    assert!(files.configuration_degraded());
    files.ensure_readable().unwrap();
    let mut files = RevisionFiles::load(&root, "HEAD", None).unwrap();
    files
        .files
        .get_mut(&lexical(&root.join("a/package.json")).unwrap())
        .unwrap()
        .object = "0".repeat(40);
    files.unresolved_package("anything");
    assert!(files.configuration_degraded());
    files.ensure_readable().unwrap();
}

/// Poisoned derived state remains charged while access records uncertainty or a real refusal.
#[test]
fn poisoned_metadata_and_reader_keep_their_owned_heap() {
    let dir = repository_with(&[("pkg/package.json", br#"{"name":"pkg"}"#)]);
    let root = dir.path().canonicalize().unwrap();
    let files = RevisionFiles::load(&root, "HEAD", None).unwrap();
    files.unresolved_package("external");
    let before = files.retained_bytes();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = files.project_packages.lock().unwrap();
        panic!("owned metadata poison probe");
    }))
    .is_err());
    files.unresolved_package("external");
    assert!(files.configuration_degraded());
    assert_eq!(files.retained_bytes(), before);
    files.ensure_readable().unwrap();
    let files = RevisionFiles::load(&root, "HEAD", None).unwrap();
    files.read(&root.join("pkg/package.json")).unwrap();
    let before = files.retained_bytes();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = files.reader.lock().unwrap();
        panic!("owned reader poison probe");
    }))
    .is_err());
    files.unresolved_package("external");
    assert!(files.configuration_degraded());
    assert_eq!(files.retained_bytes(), before);
    files.ensure_readable().unwrap();
    assert!(files.read(&root.join("pkg/package.json")).is_err());
    assert!(files.ensure_readable().is_err());
}
