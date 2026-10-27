use super::*;

/// Set one local fixture option without borrowing the analyzed command's overrides.
fn configure(root: &Path, key: &str, value: &str) {
    let result = fixture_git()
        .current_dir(root)
        .args(["config", key, value])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

/// A shell command that leaves only a marker inside its disposable repository.
fn marker_command(root: &Path) -> String {
    let marker = root
        .join("called")
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\\''");
    format!("printf called > '{marker}'; cat")
}

/// Config records retain effective empty overrides, multiline commands and exact driver names.
#[test]
fn conversion_settings_preserve_effective_values_and_unusual_names() {
    let drivers = conversion_drivers(b"filter.Own.ed=one.clean\nfirst\nsecond\0filter.Own.ed=one.process\nrun\0filter.unused.clean\nold\0filter.unused.clean\n\0filter.smudge-only.smudge\n\0").unwrap();
    assert_eq!(drivers, BTreeSet::from(["filter.Own.ed=one".to_owned()]));
    assert!(conversion_drivers(b"\xff").is_err());
    assert!(conversion_drivers(b"filter.\xff.clean\ncommand\0").is_err());
    assert_eq!(
        conversion_drivers(b"filter.owned.clean\n\xff\0").unwrap(),
        BTreeSet::from(["filter.owned".to_owned()])
    );
    assert!(conversion_drivers(b"filter.a.clean\0").is_err());
    assert!(conversion_drivers(b"other.key\nrun\0").is_err());
    assert!(conversion_drivers(b"").unwrap().is_empty());
}

/// Monitor configuration cannot run or refresh the index while tracked/untracked paths stay intact.
#[test]
fn file_monitor_is_ignored_without_losing_changed_paths_or_writing_the_index() {
    let repo = repository_with(&[("changed.test.ts", b"before"), ("same.test.ts", b"same")]);
    let root = repo.path();
    configure(root, "core.fsmonitor", &marker_command(root));
    std::fs::write(root.join("changed.test.ts"), "after changed").unwrap();
    std::fs::write(root.join("new test 空.test.ts"), "new").unwrap();
    let index = std::fs::read(root.join(".git/index")).unwrap();
    let changed = changed_files_since(root, "HEAD").unwrap();
    assert_eq!(
        changed,
        BTreeSet::from([
            PathBuf::from("changed.test.ts"),
            PathBuf::from("new test 空.test.ts")
        ])
    );
    assert!(!root.join("called").exists());
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
    let control = fixture_git()
        .current_dir(root)
        .args(["diff", "--name-only", "HEAD"])
        .output()
        .unwrap();
    assert!(control.status.success());
    assert!(
        root.join("called").exists(),
        "the marker configuration must be executable by ordinary Git"
    );
}

/// Needed clean/process conversions fail visibly, including a subsection containing equals signs.
#[test]
fn executable_conversions_are_refused_without_running_the_driver() {
    for setting in ["clean", "process"] {
        let repo = repository_with(&[
            ("a.test.ts", b"before"),
            (".gitattributes", b"*.ts filter=Own.ed=one\n"),
        ]);
        let root = repo.path();
        configure(
            root,
            &format!("filter.Own.ed=one.{setting}"),
            &marker_command(root),
        );
        std::fs::write(root.join("a.test.ts"), "after changed").unwrap();
        let error = changed_files_since(root, "HEAD").unwrap_err();
        assert!(format!("{error:#}").contains("clean filter 'Own.ed=one' failed"));
        assert!(!root.join("called").exists());
        assert_eq!(
            read_revision(root).len(),
            1,
            "historical object reads need no conversion"
        );
    }
}

/// Installed but unused drivers and a local empty override do not narrow supported repositories.
#[test]
fn unused_and_empty_conversions_keep_ordinary_changes() {
    let repo = repository_with(&[
        ("a.test.ts", b"before"),
        (".gitattributes", b"*.ts filter=owned\n"),
    ]);
    let root = repo.path();
    configure(root, "filter.unused.process", &marker_command(root));
    configure(root, "filter.owned.clean", &marker_command(root));
    let cleared = fixture_git()
        .current_dir(root)
        .args(["config", "--add", "filter.owned.clean", ""])
        .output()
        .unwrap();
    assert!(cleared.status.success());
    std::fs::write(root.join("a.test.ts"), "after changed").unwrap();
    assert_eq!(
        changed_files_since(root, "HEAD").unwrap(),
        BTreeSet::from([PathBuf::from("a.test.ts")])
    );
    assert!(!root.join("called").exists());
}

/// Built-in line endings and ignore rules continue to apply while external diff settings stay inert.
#[test]
fn data_configuration_and_external_diff_controls_preserve_selection() {
    let repo = repository_with(&[
        ("a.test.ts", b"before\n"),
        (".gitattributes", b"*.ts text eol=crlf diff=owned\n"),
        (".gitignore", b"ignored.test.ts\n"),
    ]);
    let root = repo.path();
    for key in ["diff.external", "diff.owned.command", "diff.owned.textconv"] {
        configure(root, key, &marker_command(root));
    }
    std::fs::write(root.join("a.test.ts"), "before\r\n").unwrap();
    std::fs::write(root.join("ignored.test.ts"), "ignored").unwrap();
    assert!(changed_files_since(root, "HEAD").unwrap().is_empty());
    std::fs::write(root.join("a.test.ts"), "after changed\r\n").unwrap();
    assert_eq!(
        changed_files_since(root, "HEAD").unwrap(),
        BTreeSet::from([PathBuf::from("a.test.ts")])
    );
    assert!(!root.join("called").exists());
}

/// Linked worktrees retain staged/renamed files, subdirectory roots and worktree-local settings.
#[test]
fn linked_worktrees_keep_repository_identity_and_local_configuration() {
    let repo = repository_with(&[("packages/app/old.test.ts", b"before")]);
    let directory = tempfile::tempdir().unwrap();
    let worktree = directory.path().join("linked");
    let added = fixture_git()
        .current_dir(repo.path())
        .args(["worktree", "add", "--detach"])
        .arg(&worktree)
        .output()
        .unwrap();
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    configure(repo.path(), "extensions.worktreeConfig", "true");
    let monitor = fixture_git()
        .current_dir(&worktree)
        .args([
            "config",
            "--worktree",
            "core.fsmonitor",
            &marker_command(&worktree),
        ])
        .output()
        .unwrap();
    assert!(monitor.status.success());
    let moved = fixture_git()
        .current_dir(&worktree)
        .args([
            "-c",
            "core.fsmonitor=false",
            "mv",
            "packages/app/old.test.ts",
            "packages/app/new.test.ts",
        ])
        .output()
        .unwrap();
    assert!(moved.status.success());
    let changed = changed_files_since(&worktree.join("packages/app"), "HEAD").unwrap();
    assert_eq!(changed, BTreeSet::from([PathBuf::from("new.test.ts")]));
    assert!(!worktree.join("called").exists());
    let removed = fixture_git()
        .current_dir(repo.path())
        .args(["worktree", "remove", "--force"])
        .arg(&worktree)
        .output()
        .unwrap();
    assert!(removed.status.success());
}

/// Replacement refs cannot substitute the contents of a named commit in either mode.
#[test]
fn replacement_refs_do_not_change_the_named_commit() {
    let repo = repository_with(&[("a.test.ts", b"original")]);
    let root = repo.path();
    let old = fixture_git()
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let old = String::from_utf8(old.stdout).unwrap();
    std::fs::write(root.join("a.test.ts"), "replacement").unwrap();
    for args in [
        ["add", "."].as_slice(),
        ["commit", "-qm", "replacement"].as_slice(),
        ["replace", old.trim(), "HEAD"].as_slice(),
    ] {
        assert!(fixture_git()
            .current_dir(root)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    let sources = collect_sources(root, old.trim(), &["**/*.test.ts".to_owned()], &[]).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].content, "original");
    assert_eq!(
        changed_files_since(root, old.trim()).unwrap(),
        BTreeSet::from([PathBuf::from("a.test.ts")])
    );
}

/// Conversion lookup reports malformed configuration and operating-system refusals explicitly.
#[test]
fn conversion_lookup_errors_are_not_an_empty_driver_set() {
    let repo = repository_with(&[("a.test.ts", b"original")]);
    let config = repo.path().join(".git/config");
    std::fs::write(config, "[unfinished\n").unwrap();
    let error = git_worktree_at(repo.path()).unwrap_err();
    assert!(format!("{error:#}").contains("could not read Git conversion settings"));
    let error = git_worktree_at(Path::new("invalid\0root")).unwrap_err();
    assert!(format!("{error:#}").contains("failed to read Git conversion settings"));
}

/// Includes apply effective settings without turning an unused driver into a rejection.
#[test]
fn included_driver_settings_are_neutralized() {
    let repo = repository_with(&[
        ("a.test.ts", b"before"),
        (".gitattributes", b"*.ts filter=owned\n"),
    ]);
    let root = repo.path();
    let included = root.join(".git/driver-config");
    let status = fixture_git()
        .current_dir(root)
        .arg("config")
        .arg("--file")
        .arg(&included)
        .args(["filter.owned.clean", &marker_command(root)])
        .output()
        .unwrap();
    assert!(status.status.success());
    configure(root, "include.path", "driver-config");
    std::fs::write(root.join("a.test.ts"), "after changed").unwrap();
    assert!(changed_files_since(root, "HEAD").is_err());
    assert!(!root.join("called").exists());
}

/// A missing promisor object remains a visible failure without starting a configured transport.
#[test]
fn object_reads_do_not_fetch_and_transport_permissions_cannot_override_the_command() {
    let repo = repository_with(&[("a.test.ts", b"before")]);
    let root = repo.path();
    let result = git_at(root)
        .args([
            "-c",
            "protocol.ext.allow=always",
            "ls-remote",
            "ext::invalid-command",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("transport 'ext' not allowed"));
    let blob = fixture_git()
        .current_dir(root)
        .args(["rev-parse", "HEAD:a.test.ts"])
        .output()
        .unwrap();
    let blob = String::from_utf8(blob.stdout).unwrap();
    let blob = blob.trim();
    configure(root, "remote.origin.url", "ext::invalid-command");
    configure(root, "remote.origin.promisor", "true");
    configure(root, "extensions.partialClone", "origin");
    std::fs::remove_file(root.join(".git/objects").join(&blob[..2]).join(&blob[2..])).unwrap();
    assert!(base_read(root, &["**/*.test.ts"], &[]).is_err());
}

/// Dirty submodule contents require no child worktree comparison, while changed gitlinks survive.
#[test]
fn submodule_dirt_does_not_execute_filters_or_hide_gitlink_changes() {
    let child = repository_with(&[
        ("child.test.ts", b"before"),
        (".gitattributes", b"*.ts filter=owned\n"),
    ]);
    let parent = repository_with(&[("parent.test.ts", b"before")]);
    let root = parent.path();
    let result = fixture_git()
        .current_dir(root)
        .args(["-c", "protocol.file.allow=always", "submodule", "add", "-q"])
        .arg(child.path())
        .arg("child")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(fixture_git()
        .current_dir(root)
        .args(["commit", "-qm", "submodule"])
        .output()
        .unwrap()
        .status
        .success());
    let nested = root.join("child");
    configure(&nested, "filter.owned.clean", &marker_command(&nested));
    configure(&nested, "core.fsmonitor", &marker_command(&nested));
    std::fs::write(nested.join("child.test.ts"), "dirty nested file").unwrap();
    std::fs::write(root.join("parent.test.ts"), "changed parent").unwrap();
    assert_eq!(
        changed_files_since(root, "HEAD").unwrap(),
        BTreeSet::from([PathBuf::from("parent.test.ts")])
    );
    assert!(!nested.join("called").exists());
    // An empty commit changes the gitlink without reading the child's dirty index or filters.
    let tree = fixture_git()
        .current_dir(&nested)
        .args(["rev-parse", "HEAD^{tree}"])
        .output()
        .unwrap();
    let tree = String::from_utf8(tree.stdout).unwrap();
    let commit = fixture_git()
        .current_dir(&nested)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit-tree",
            tree.trim(),
            "-p",
            "HEAD",
            "-m",
            "child revision",
        ])
        .output()
        .unwrap();
    assert!(commit.status.success());
    let commit = String::from_utf8(commit.stdout).unwrap();
    assert!(fixture_git()
        .current_dir(&nested)
        .args(["update-ref", "HEAD", commit.trim()])
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(
        changed_files_since(root, "HEAD").unwrap(),
        BTreeSet::from([PathBuf::from("child"), PathBuf::from("parent.test.ts")])
    );
    assert!(!nested.join("called").exists());
}
