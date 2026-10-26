//! Bounded reads of package metadata shared by resolution, proofs and prerequisite advice.

use std::io::{self, Read};
use std::path::Path;

/// Largest supported package manifest, including whitespace.
pub(crate) const MAX_BYTES: u64 = 1024 * 1024;

/// Reads a UTF-8 regular manifest, following links to regular files only.
pub(crate) fn read(path: &Path) -> io::Result<String> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        // A dangling link is present but unreadable, not an absent manifest to fall back past.
        if error.kind() == io::ErrorKind::NotFound && std::fs::symlink_metadata(path).is_ok() {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "unreadable package manifest link",
            )
        } else {
            error
        }
    })?;
    admit(&metadata)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // The path may become a FIFO after metadata was checked. Regular files ignore this flag.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    admit(&file.metadata()?)?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "package manifest exceeds 1 MiB",
        ));
    }
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Rejects special files before opening or reading, and oversized regular files before allocation.
fn admit(metadata: &std::fs::Metadata) -> io::Result<()> {
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "package manifest must be a regular file of at most 1 MiB",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Exact admission boundaries retain whitespace and reject oversized or non-text metadata.
    #[test]
    fn manifest_admission_is_bounded_and_regular() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("package.json");
        assert_eq!(read(&path).unwrap_err().kind(), io::ErrorKind::NotFound);
        fs::write(&path, " ".repeat(MAX_BYTES as usize)).unwrap();
        assert_eq!(read(&path).unwrap().len(), MAX_BYTES as usize);
        fs::write(&path, " ".repeat(MAX_BYTES as usize + 1)).unwrap();
        assert_eq!(read(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
        fs::write(&path, [0xff]).unwrap();
        assert_eq!(read(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(read(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    /// Package links remain supported while links to special or missing targets stay refused.
    #[cfg(unix)]
    #[test]
    fn manifest_links_follow_only_regular_targets() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        let path = directory.path().join("package.json");
        symlink(&target, &path).unwrap();
        assert_eq!(read(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
        fs::write(&target, "{}").unwrap();
        assert_eq!(read(&path).unwrap(), "{}");
        fs::remove_file(&target).unwrap();
        symlink("/dev/null", &target).unwrap();
        assert_eq!(read(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
        fs::remove_file(&target).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(&target)
            .status()
            .unwrap()
            .success());
        assert_eq!(read(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
