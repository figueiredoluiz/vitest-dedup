//! Buffered, atomic replacement of generated report and baseline files.

use anyhow::{Context, Result};
use std::io::{BufWriter, Write};
use std::path::Path;

/// Writes a complete file before atomically replacing its destination.
///
/// A final-component symlink is replaced, never opened. Parent directories may contain
/// symlinks. On an ordinary write or replacement error, the previous destination survives.
/// Replacement changes the inode and uses temporary-file permissions; this does not promise
/// durability across a system crash or a transaction across several output files.
pub fn write(path: &Path, produce: impl FnOnce(&mut dyn Write) -> Result<()>) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temporary file for {}", path.display()))?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        produce(&mut writer)?;
        writer
            .flush()
            .with_context(|| format!("flushing {}", path.display()))?;
    }
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests;
