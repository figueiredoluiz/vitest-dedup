//! Source authority controls for resolver factories, text reads and rejected metadata.
use super::*;
use std::sync::Mutex;

/// A virtual source intentionally disagrees with any real file at its path.
#[derive(Debug)]
struct Tree {
    path: PathBuf,
    bytes: Vec<u8>,
    kind: SourceKind,
    errors: Mutex<Vec<String>>,
}

impl SourceTree for Tree {
    /// Return owned virtual content only for the one admitted path.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        (path == self.path)
            .then(|| self.bytes.clone())
            .ok_or(io::ErrorKind::NotFound.into())
    }
    /// Virtual files imply ancestor directories; no host path participates in metadata.
    fn kind(&self, path: &Path) -> Option<SourceKind> {
        if path == self.path {
            Some(self.kind)
        } else {
            (self.kind == SourceKind::File
                && self.path.ancestors().skip(1).any(|parent| parent == path))
            .then_some(SourceKind::Directory)
        }
    }
    /// A virtual identity is already canonical.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.kind(path)
            .map(|_| path.to_owned())
            .ok_or(io::ErrorKind::NotFound.into())
    }
    /// Capture concrete refusals independently of whether a resolver later returns None.
    fn reject(&self, message: String) {
        self.errors.lock().unwrap().push(message);
    }
    /// Keep configuration-gap recording distinct from failed committed reads.
    fn configuration_gap(&self) {
        self.errors
            .lock()
            .unwrap()
            .push("configuration gap".to_owned());
    }
    /// Tests using this single-file view have no committed project package metadata.
    fn unresolved_package(&self, name: &str) {
        self.errors
            .lock()
            .unwrap()
            .push(format!("unresolved package: {name}"));
    }
    /// A distinctive retained size detects missing or duplicate ownership charges.
    fn retained_bytes(&self) -> usize {
        12345
    }
}

/// Declaration ownership uses one authority for both paths, including missing virtual identities.
#[test]
fn relative_identities_cannot_borrow_a_host_file_or_a_host_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let live = root.path().join("live.ts");
    let foreign = outside.path().join("foreign.ts");
    std::fs::write(&live, "live").unwrap();
    std::fs::write(&foreign, "foreign").unwrap();
    let working = Sources::default();
    assert_eq!(
        working.relative(&live, root.path()),
        Some(PathBuf::from("live.ts"))
    );
    assert_eq!(working.relative(&foreign, root.path()), None);
    assert_eq!(
        working.relative(&root.path().join("missing.ts"), root.path()),
        None
    );
    assert_eq!(
        working.relative(&live, &root.path().join("missing-root")),
        None
    );
    let virtual_path = root.path().join("virtual.ts");
    assert!(!virtual_path.exists());
    let committed = Sources::committed(Arc::new(Tree {
        path: virtual_path.clone(),
        bytes: b"committed".to_vec(),
        kind: SourceKind::File,
        errors: Mutex::default(),
    }));
    assert_eq!(
        committed.relative(&virtual_path, root.path()),
        Some(PathBuf::from("virtual.ts"))
    );
    assert_eq!(
        committed.relative(&live, root.path()),
        None,
        "an existing host file has no committed identity"
    );
    assert_eq!(
        committed.relative(&virtual_path, outside.path()),
        None,
        "a host-only root cannot normalize a committed declaration"
    );
}

/// Default construction cannot expose even an existing working file; committed reads win.
#[test]
fn factories_and_reads_preserve_the_source_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("helper.ts");
    std::fs::write(&path, "live").unwrap();
    let empty = CommittedFs::new();
    assert!(empty.read(&path).is_err());
    assert!(empty.read_to_string(&path).is_err());
    assert!(empty.metadata(&path).is_err());
    assert!(empty.canonicalize(&path).is_err());
    assert!(empty.read_link(&path).is_err());
    assert_eq!(Sources::default().read(&path).as_deref(), Some("live"));
    let tree = Arc::new(Tree {
        path: path.clone(),
        bytes: b"committed".to_vec(),
        kind: SourceKind::File,
        errors: Mutex::default(),
    });
    let sources = Sources::committed(tree.clone());
    assert_eq!(sources.read(&path).as_deref(), Some("committed"));
    assert!(sources.read(&dir.path().join("missing")).is_none());
    assert_eq!(crate::memory::HeapBytes::heap_bytes(&sources), 12345);
    let fs = CommittedFs(Some(tree));
    assert_eq!(fs.read(&path).unwrap(), b"committed");
    assert!(fs.metadata(&path).is_ok());
    assert!(fs.symlink_metadata(&path).is_ok());
    assert!(fs.metadata(&dir.path().join("missing")).is_err());
    assert_eq!(fs.canonicalize(&path).unwrap(), path);
}

/// Invalid text and nonregular metadata produce sticky failures rather than working fallback.
#[test]
fn invalid_text_and_nonregular_metadata_are_recorded() {
    for kind in [
        SourceKind::File,
        SourceKind::Directory,
        SourceKind::Symlink,
        SourceKind::Other,
    ] {
        let tree = Arc::new(Tree {
            path: PathBuf::from("/virtual"),
            bytes: vec![255],
            kind,
            errors: Mutex::default(),
        });
        let fs = CommittedFs(Some(tree.clone()));
        assert!(fs.read_to_string(&tree.path).is_err());
        assert_eq!(tree.errors.lock().unwrap().len(), 1);
        assert_eq!(
            fs.metadata(&tree.path).is_ok(),
            matches!(kind, SourceKind::File | SourceKind::Directory)
        );
        assert_eq!(
            tree.errors.lock().unwrap().len(),
            if matches!(kind, SourceKind::File | SourceKind::Directory) {
                1
            } else {
                2
            }
        );
    }
}

/// Typed missing-package causes never swallow malformed, unreadable or escaped configuration.
#[test]
fn package_configuration_absence_is_classified_from_virtual_facts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let tree = Tree {
        path: root.clone(),
        bytes: Vec::new(),
        kind: SourceKind::Directory,
        errors: Mutex::default(),
    };
    let missing = root.join("node_modules/pkg/tsconfig.json");
    for path in [
        PathBuf::from("@tsconfig/node20/tsconfig.json"),
        PathBuf::from("pkg/base"),
        missing.clone(),
    ] {
        assert!(absent_package_config(
            &ResolveError::TsconfigNotFound(path),
            &tree
        ));
    }
    for path in [
        PathBuf::from(""),
        PathBuf::from("./base"),
        PathBuf::from("../base"),
        PathBuf::from("#missing"),
        root.join("missing.json"),
        root.join("../elsewhere/node_modules/pkg/base.json"),
        root.parent()
            .unwrap()
            .join("elsewhere/node_modules/pkg/base.json"),
    ] {
        assert!(!absent_package_config(
            &ResolveError::TsconfigNotFound(path),
            &tree
        ));
    }
    let wrapped = |source| ResolveError::TsconfigLoadFailed {
        path: missing.clone(),
        source: Box::new(source),
    };
    assert!(absent_package_config(
        &wrapped(ResolveError::TsconfigNotFound("pkg/base".into())),
        &tree
    ));
    assert!(absent_package_config(
        &wrapped(io::Error::from(io::ErrorKind::NotFound).into()),
        &tree
    ));
    assert!(!absent_package_config(
        &wrapped(io::Error::from(io::ErrorKind::PermissionDenied).into()),
        &tree
    ));
    assert!(!absent_package_config(
        &wrapped(ResolveError::NotFound("ordinary source".into())),
        &tree
    ));
    let repository_named_node_modules = Tree {
        path: root.join("node_modules"),
        bytes: Vec::new(),
        kind: SourceKind::Directory,
        errors: Mutex::default(),
    };
    assert!(!absent_package_config(
        &ResolveError::TsconfigNotFound(root.join("node_modules/missing-local.json")),
        &repository_named_node_modules,
    ));
    // The real catalog supplies both the package directory and its namespace parents.
    let repository = crate::modes::tests::repository_with(&[
        (".gitignore", b"!node_modules/\n!node_modules/**\n"),
        ("node_modules/pkg/package.json", b"{}"),
        ("node_modules/present/tsconfig.json", b"{}"),
    ]);
    let repository_root = repository.path().canonicalize().unwrap();
    let catalog = crate::modes::RevisionFiles::load(&repository_root, "HEAD", None).unwrap();
    assert!(absent_package_config(
        &ResolveError::TsconfigNotFound(repository_root.join("node_modules/pkg")),
        &catalog,
    ));
    assert!(!absent_package_config(
        &ResolveError::TsconfigNotFound(repository_root.join("node_modules/present")),
        &catalog,
    ));
    let present = Tree {
        path: missing.clone(),
        bytes: Vec::new(),
        kind: SourceKind::File,
        errors: Mutex::default(),
    };
    assert!(!absent_package_config(
        &ResolveError::TsconfigNotFound(missing),
        &present
    ));
}

/// Package uncertainty uses the recorded package name, not directory spelling or query suffixes.
#[test]
fn bare_package_names_are_distinguished_from_other_specifiers() {
    for (request, name) in [
        ("pkg", "pkg"),
        ("pkg/sub?raw", "pkg"),
        ("pkg/sub?loader=http://owned.invalid", "pkg"),
        ("@acme/runner/sub#tag", "@acme/runner"),
    ] {
        assert_eq!(package_name(request), Some(name));
    }
    for request in [
        "",
        "./local",
        "../outside",
        "/absolute",
        "#private",
        "node:fs",
        "C:\\file",
        "@scope",
        "@scope/",
    ] {
        assert_eq!(package_name(request), None, "{request}");
    }
}

/// Opaque builtins/ignored modules and paths must not be mislabeled as workspace uncertainty.
#[test]
fn ordinary_absence_only_records_a_real_bare_package_request() {
    let tree = Tree {
        path: PathBuf::from("/virtual"),
        bytes: Vec::new(),
        kind: SourceKind::Directory,
        errors: Mutex::default(),
    };
    for error in [
        ResolveError::Ignored("ignored".into()),
        ResolveError::Builtin {
            resolved: "node:fs".into(),
            is_runtime_module: true,
        },
        ResolveError::NotFound("./relative".into()),
        ResolveError::NotFound("node:fs".into()),
    ] {
        record_package_absence(&error, &tree);
    }
    assert!(tree.errors.lock().unwrap().is_empty());
    record_package_absence(
        &ResolveError::NotFound("@acme/workspace/sub?raw".into()),
        &tree,
    );
    assert_eq!(
        *tree.errors.lock().unwrap(),
        ["unresolved package: @acme/workspace"]
    );
}

/// A virtual catalog can record one canonical identity for an alternate importer spelling.
#[derive(Debug)]
struct ImporterTree {
    catalog: crate::modes::RevisionFiles,
    presented: PathBuf,
    canonical: PathBuf,
}

impl SourceTree for ImporterTree {
    /// Every byte still comes from the pinned catalog.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.catalog.read(path)
    }
    /// Alternate identity handling does not add filesystem entries.
    fn kind(&self, path: &Path) -> Option<SourceKind> {
        self.catalog.kind(path)
    }
    /// Record the importer's canonical identity without touching the host filesystem.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        if path == self.presented {
            Ok(self.canonical.clone())
        } else {
            self.catalog.canonicalize(path)
        }
    }
    /// Preserve the catalog's sticky refusal handling.
    fn reject(&self, message: String) {
        self.catalog.reject(message);
    }
    /// Preserve the catalog's incomplete-configuration policy.
    fn configuration_gap(&self) {
        self.catalog.configuration_gap();
    }
    /// Package absence still uses the committed metadata catalog.
    fn unresolved_package(&self, name: &str) {
        self.catalog.unresolved_package(name);
    }
    /// This test uses the catalog's retained-ownership measurement.
    fn retained_bytes(&self) -> usize {
        self.catalog.retained_bytes()
    }
}

/// Resolve known importer identities before parsing them, while retaining supplied-source callers.
#[test]
fn resolver_uses_the_catalogs_importer_identity() {
    let dir = crate::modes::tests::repository_with(&[
        ("run.test.ts", b"import './helper';"),
        ("helper.ts", b"export const value = 1;"),
    ]);
    let root = dir.path().canonicalize().unwrap();
    let catalog = crate::modes::RevisionFiles::load(&root, "HEAD", None).unwrap();
    let canonical = catalog.canonicalize(&root.join("run.test.ts")).unwrap();
    let presented = root.join("presentation/run.test.ts");
    let tree = Arc::new(ImporterTree {
        catalog,
        presented: presented.clone(),
        canonical: canonical.clone(),
    });
    let sources = Sources::committed(tree.clone());
    let expected = tree.canonicalize(&root.join("helper.ts")).unwrap();
    for from in [presented, canonical, root.join("supplied.test.ts")] {
        let resolved = sources.resolve_file(&from, "./helper", &root).unwrap();
        assert_eq!(tree.canonicalize(&resolved).unwrap(), expected);
    }
    tree.catalog.ensure_readable().unwrap();
}
