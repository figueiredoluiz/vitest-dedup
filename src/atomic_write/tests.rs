use super::*;

/// Both a new output and a previous report receive the full successfully flushed content.
#[test]
fn replaces_only_after_complete_content_is_ready() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("report.json");
    for previous in [None, Some(b"previous report".as_slice())] {
        if let Some(bytes) = previous {
            std::fs::write(&path, bytes).unwrap();
        }
        let content = "complete report".repeat(2048);
        write(&path, |writer| {
            if let Some(bytes) = previous {
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            } else {
                assert!(!path.exists());
            }
            writer.write_all(content.as_bytes())?;
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}

/// A reporter that emits bytes and then errors cannot publish partial output.
#[test]
fn failed_reporter_preserves_existing_output_and_removes_temporary_file() {
    for previous in [Some(b"previous report".as_slice()), None] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        if let Some(bytes) = previous {
            std::fs::write(&path, bytes).unwrap();
        }
        let failure = write(&path, |writer| {
            writer.write_all(&vec![b'x'; 32 * 1024])?;
            anyhow::bail!("reporter failed after a partial report")
        })
        .unwrap_err();
        assert!(failure.to_string().contains("reporter failed"));
        if let Some(bytes) = previous {
            assert!(
                std::fs::read(&path).unwrap() == bytes,
                "previous report bytes must survive"
            );
        } else {
            assert!(!path.exists());
        }
        assert_eq!(
            std::fs::read_dir(directory.path()).unwrap().count(),
            usize::from(previous.is_some())
        );
    }
}

/// Failed replacement retains an existing directory and cleans up the sibling temp file.
#[test]
fn replacement_failure_preserves_destination_and_cleans_up() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("report.json");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("sentinel"), b"keep").unwrap();
    let failure = write(&path, |writer| {
        writer.write_all(b"new report")?;
        Ok(())
    })
    .unwrap_err();
    assert!(failure.to_string().contains("replacing"));
    assert_eq!(std::fs::read(path.join("sentinel")).unwrap(), b"keep");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

/// A missing parent produces an actionable error rather than a silent successful no-op.
#[test]
fn missing_parent_is_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing/report.json");
    let error = write(&path, |_| Ok(())).unwrap_err().to_string();
    assert!(error.contains("creating temporary file"), "{error}");
    assert!(error.contains("report.json"), "{error}");
    assert!(!path.exists());
}

/// Hard links are replaced independently, so another name keeps its previous bytes.
#[test]
fn replacing_a_hard_link_does_not_modify_its_other_name() {
    let directory = tempfile::tempdir().unwrap();
    let victim = directory.path().join("victim");
    let path = directory.path().join("report.json");
    std::fs::write(&victim, b"keep").unwrap();
    std::fs::hard_link(&victim, &path).unwrap();
    write(&path, |writer| {
        writer.write_all(b"complete report")?;
        Ok(())
    })
    .unwrap();
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
    assert_eq!(std::fs::read(&path).unwrap(), b"complete report");
}

/// Final-component links are replaced whether their referents exist or not.
#[cfg(unix)]
#[test]
fn replaces_symlink_without_following_it() {
    let directory = tempfile::tempdir().unwrap();
    let victim = directory.path().join("victim");
    let path = directory.path().join("report.json");
    for exists in [false, true] {
        if exists {
            std::fs::write(&victim, b"keep").unwrap();
        }
        std::os::unix::fs::symlink(&victim, &path).unwrap();
        write(&path, |writer| {
            writer.write_all(b"complete report")?;
            Ok(())
        })
        .unwrap();
        assert!(!path.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&path).unwrap(), b"complete report");
        if exists {
            assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
        } else {
            assert!(!victim.exists());
        }
        std::fs::remove_file(&path).unwrap();
    }
}
