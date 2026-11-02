//! Committed source/configuration access: immutable object IDs and no checkout or OS fallback.
use super::{git_at, revision_commit, terminal_safe, BlobReader, MAX_REVISION_SOURCE_BYTES};
use crate::lang::{SourceKind, SourceTree};
use crate::memory::HeapBytes;
use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// A bounded listing entry accommodates supported OS path lengths without unbounded buffering.
const MAX_ENTRY_BYTES: u64 = 128 * 1024;

/// Immutable content identity and filesystem type, never a path passed to cat-file.
#[derive(Debug)]
struct Entry {
    object: String,
    size: Option<u64>,
    kind: SourceKind,
}

/// Bounded derived metadata, with an O(1) retained-heap ledger.
#[derive(Default)]
struct ProjectPackages {
    names: HashSet<String>,
    owned: usize,
    uncertain: bool,
}

/// One pinned commit's source namespace, including resolver configuration and non-test helpers.
pub struct RevisionFiles {
    root: PathBuf,
    analysis_root: PathBuf,
    commit: String,
    files: HashMap<PathBuf, Entry>,
    directories: HashSet<PathBuf>,
    /// Live key/value allocations, excluding separately measured map/set entry storage.
    owned: usize,
    reader: Mutex<Option<BlobReader>>,
    problem: Mutex<Option<String>>,
    configuration_gap: AtomicBool,
    project_packages: Mutex<Option<ProjectPackages>>,
}

impl std::fmt::Debug for RevisionFiles {
    /// Debug metadata cannot expose the analyzed source or lock a running pipe reader.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RevisionFiles")
            .field("commit", &self.commit)
            .field("files", &self.files.len())
            .finish_non_exhaustive()
    }
}

impl RevisionFiles {
    /// Index one commit through bounded plumbing; configuration lookup may use its ancestors
    /// within the repository, while source resolution retains the caller's analysis-root limit.
    pub fn load(root: &Path, revision: &str, budget: Option<u64>) -> Result<Self> {
        let commit = revision_commit(root, revision)?;
        let output = git_at(root)
            .args(["rev-parse", "--show-toplevel"])
            .output()?;
        if !output.status.success() {
            bail!("could not determine the base revision repository root");
        }
        let path = String::from_utf8(output.stdout).context("non-UTF-8 repository root")?;
        let repository = lexical(Path::new(path.strip_suffix('\n').unwrap_or(&path)))
            .context("repository root is not an absolute path")?;
        let output = git_at(root).args(["rev-parse", "--show-prefix"]).output()?;
        if !output.status.success() {
            bail!("could not determine the base revision analysis scope");
        }
        let prefix = String::from_utf8(output.stdout).context("non-UTF-8 analysis prefix")?;
        let prefix = Path::new(prefix.strip_suffix('\n').unwrap_or(&prefix));
        if prefix
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            bail!("unsafe base revision analysis prefix");
        }
        let mut files = Self {
            analysis_root: repository.join(prefix),
            root: repository,
            commit,
            files: HashMap::new(),
            directories: HashSet::new(),
            owned: 0,
            reader: Mutex::new(None),
            problem: Mutex::new(None),
            configuration_gap: AtomicBool::new(false),
            project_packages: Mutex::new(None),
        };
        files.directory(files.root.clone());
        files.check_budget(budget)?;
        let mut listing = Listing(
            git_at(&files.root)
                .args(["ls-tree", "--full-tree", "-r", "-l", "-z", "--"])
                .arg(&files.commit)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?,
        );
        let stderr = super::drain_stderr(
            listing
                .0
                .stderr
                .take()
                .context("listing stderr is missing")?,
        );
        let mut pipe = io::BufReader::new(
            listing
                .0
                .stdout
                .take()
                .context("listing stdout is missing")?,
        );
        let parsed = (|| -> Result<()> {
            loop {
                let mut entry = Vec::new();
                if (&mut pipe)
                    .take(MAX_ENTRY_BYTES + 1)
                    .read_until(0, &mut entry)?
                    == 0
                {
                    break;
                }
                if entry.len() as u64 > MAX_ENTRY_BYTES || entry.pop() != Some(0) {
                    bail!(
                        "base revision tree entry exceeds the path byte limit or is unterminated"
                    );
                }
                files.insert(&entry, budget)?;
            }
            Ok(())
        })();
        if parsed.is_err() {
            let _ = listing.0.kill();
        }
        let status = listing.0.wait()?;
        let said = stderr.join().unwrap_or_default();
        parsed?;
        if !status.success() {
            bail!(
                "git failed while indexing the base revision: {}",
                terminal_safe(said.trim())
            );
        }
        if files
            .files
            .keys()
            .any(|path| files.directories.contains(path))
        {
            bail!("base revision has an ambiguous file/directory path");
        }
        Ok(files)
    }

    /// The immutable ID passed to the existing streamed root reader, never a mutable ref again.
    pub fn commit(&self) -> &str {
        &self.commit
    }

    /// A root handed out by the shared reader must be regular before it reaches the parser.
    pub fn regular_root(&self, path: &Path) -> Result<()> {
        if self.kind(path) != Some(SourceKind::File) {
            bail!(
                "base revision source {} is not a regular file",
                terminal_safe(&path.display().to_string())
            );
        }
        Ok(())
    }

    /// Refuse the baseline rather than accepting claims built after a failed dependency read.
    pub fn ensure_readable(&self) -> Result<()> {
        let problem = self
            .problem
            .lock()
            .map_err(|_| anyhow::anyhow!("historical source state was poisoned"))?;
        if let Some(problem) = &*problem {
            bail!("base revision source access failed: {problem}");
        }
        Ok(())
    }

    /// Missing package configuration makes the entire historical allowance uncertain.
    pub fn configuration_degraded(&self) -> bool {
        self.configuration_gap.load(Ordering::Relaxed)
    }

    /// Accumulate each live allocation once, including empty-but-reserved collection storage.
    fn directory(&mut self, path: PathBuf) {
        let bytes = path.capacity();
        if self.directories.insert(path) {
            self.owned = self.owned.saturating_add(bytes);
        }
    }

    /// Check metadata retention as it grows, before reading source contents or more entries.
    fn check_budget(&self, budget: Option<u64>) -> Result<()> {
        if budget.is_some_and(|limit| self.retained_bytes() as u64 > limit) {
            bail!("base revision source index exceeds the memory budget; raise --max-memory or use --no-memory-cap");
        }
        Ok(())
    }

    /// Parse only structural Git metadata; paths and object IDs are independently validated.
    fn insert(&mut self, bytes: &[u8], budget: Option<u64>) -> Result<()> {
        let tab = bytes
            .iter()
            .position(|&b| b == b'\t')
            .context("unreadable base tree entry")?;
        let header = std::str::from_utf8(&bytes[..tab])?;
        let fields: Vec<_> = header.split_whitespace().collect();
        let [mode, object_type, object, size] = fields.as_slice() else {
            bail!("unreadable base tree metadata");
        };
        if !matches!(object.len(), 40 | 64) || !object.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("invalid base tree object ID");
        }
        let kind = match (*object_type, *mode) {
            ("blob", "100644" | "100755") => SourceKind::File,
            ("blob", "120000") => SourceKind::Symlink,
            _ => SourceKind::Other,
        };
        let size = if *object_type == "blob" && *size != "BAD" {
            Some(size.parse::<u64>()?)
        } else {
            None
        };
        // Such names cannot be queried through the UTF-8 resolver; root admission is separate.
        let Ok(name) = std::str::from_utf8(&bytes[tab + 1..]) else {
            return Ok(());
        };
        let relative = Path::new(name);
        if name.is_empty()
            || relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            bail!("unsafe base revision tree path {}", terminal_safe(name));
        }
        let path = self.root.join(relative);
        let entry = Entry {
            object: (*object).to_owned(),
            size,
            kind,
        };
        self.owned = self
            .owned
            .saturating_add(path.capacity())
            .saturating_add(entry.object.capacity());
        let mut parent = path.parent().map(Path::to_path_buf);
        if self.files.insert(path, entry).is_some() {
            bail!("duplicate base revision tree path");
        }
        self.check_budget(budget)?;
        while let Some(directory) = parent {
            if !directory.starts_with(&self.root) {
                break;
            }
            parent = directory.parent().map(Path::to_path_buf);
            self.directory(directory);
            self.check_budget(budget)?;
        }
        Ok(())
    }

    /// Virtual paths are lexical; host symlinks and present files never participate.
    fn path(&self, path: &Path) -> Option<PathBuf> {
        let result = lexical(path)?;
        result.starts_with(&self.root).then_some(result)
    }

    /// Convert a failed committed read into a sticky diagnostic with no terminal controls.
    fn failed(&self, path: &Path, error: impl std::fmt::Display) -> io::Error {
        let message = format!("historical dependency {}: {error}", path.display());
        self.reject(message.clone());
        io::Error::other(terminal_safe(&message))
    }

    /// Read one bounded immutable object without deciding whether the caller needs it.
    fn blob(&self, entry: &Entry) -> io::Result<Vec<u8>> {
        if entry.kind != SourceKind::File {
            return Err(io::Error::other("not a regular file"));
        }
        let size = entry
            .size
            .ok_or_else(|| io::Error::other("not available in local Git objects"))?;
        if size > MAX_REVISION_SOURCE_BYTES {
            return Err(io::Error::other("exceeds the source byte limit"));
        }
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| io::Error::other("reader state was poisoned"))?;
        if reader.is_none() {
            *reader = Some(
                BlobReader::spawn(&self.root)
                    .map_err(|error| io::Error::other(error.to_string()))?,
            );
        }
        reader
            .as_mut()
            .expect("initialized above")
            .read(&entry.object)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    /// Catalog possible project names once, with a bounded total metadata read and explicit doubt.
    fn collect_project_packages(&self) -> ProjectPackages {
        let mut packages = ProjectPackages::default();
        let mut bytes = 0_u64;
        for (path, entry) in &self.files {
            let Ok(relative) = path.strip_prefix(&self.analysis_root) else {
                continue;
            };
            if path.file_name().is_none_or(|name| name != "package.json")
                || relative
                    .components()
                    .any(|part| part.as_os_str() == "node_modules")
            {
                continue;
            }
            let Some(size) = entry.size.filter(|size| *size <= MAX_REVISION_SOURCE_BYTES) else {
                packages.uncertain = true;
                continue;
            };
            bytes = bytes.saturating_add(size);
            if bytes > MAX_REVISION_SOURCE_BYTES {
                packages.uncertain = true;
                break;
            }
            let metadata = self
                .blob(entry)
                .ok()
                .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok());
            let Some(metadata) = metadata else {
                packages.uncertain = true;
                continue;
            };
            if !metadata.is_object() {
                packages.uncertain = true;
                continue;
            }
            match metadata.get("name") {
                Some(serde_json::Value::String(name)) if !name.is_empty() => {
                    let name = name.to_owned();
                    let capacity = name.capacity();
                    if packages.names.insert(name) {
                        packages.owned = packages.owned.saturating_add(capacity);
                    }
                }
                None => {}
                _ => packages.uncertain = true,
            }
        }
        packages
    }
}

/// Normalize lexical components and Windows disk prefixes without opening a host path.
fn lexical(path: &Path) -> Option<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    return None;
                }
            }
            Component::Prefix(prefix) => match prefix.kind() {
                std::path::Prefix::Disk(disk) | std::path::Prefix::VerbatimDisk(disk) => {
                    result.push(format!("{}:", char::from(disk.to_ascii_uppercase())));
                }
                std::path::Prefix::VerbatimUNC(server, share) => {
                    let mut prefix = std::ffi::OsString::from("\\\\");
                    prefix.push(server);
                    prefix.push("\\");
                    prefix.push(share);
                    result.push(prefix);
                }
                _ => result.push(prefix.as_os_str()),
            },
            component => result.push(component.as_os_str()),
        }
    }
    result.is_absolute().then_some(result)
}

impl SourceTree for RevisionFiles {
    /// A size/type checked immutable object read, through one serialized batch process.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let key = self.path(path).ok_or(io::ErrorKind::NotFound)?;
        let entry = self.files.get(&key).ok_or(io::ErrorKind::NotFound)?;
        self.blob(entry).map_err(|error| self.failed(path, error))
    }

    /// Tree entries and synthesized ancestor directories are the only existing paths.
    fn kind(&self, path: &Path) -> Option<SourceKind> {
        let path = self.path(path)?;
        self.files.get(&path).map(|entry| entry.kind).or_else(|| {
            self.directories
                .contains(&path)
                .then_some(SourceKind::Directory)
        })
    }

    /// Existing non-link virtual paths have no physical canonicalization step.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        let normalized = self.path(path).ok_or(io::ErrorKind::NotFound)?;
        match self.kind(&normalized) {
            Some(SourceKind::File | SourceKind::Directory) => Ok(normalized),
            Some(_) => Err(self.failed(path, "nonregular path cannot be canonicalized")),
            None => Err(io::ErrorKind::NotFound.into()),
        }
    }

    /// Keep the first concrete access failure, including failures the resolver represents as None.
    fn reject(&self, message: String) {
        if let Ok(mut problem) = self.problem.lock() {
            if problem.is_none() {
                *problem = Some(terminal_safe(&message));
            }
        }
    }

    /// Sticky across files and cache resets; no fallback identity becomes baseline allowance.
    fn configuration_gap(&self) {
        self.configuration_gap.store(true, Ordering::Relaxed);
    }

    /// A missing recorded project name cannot silently become an opaque external dependency.
    fn unresolved_package(&self, name: &str) {
        if self.configuration_degraded() {
            return;
        }
        let Ok(mut cache) = self.project_packages.lock() else {
            self.configuration_gap();
            return;
        };
        let packages = cache.get_or_insert_with(|| self.collect_project_packages());
        if packages.uncertain || packages.names.contains(name) {
            self.configuration_gap();
        }
    }

    /// No catalog walk at each budget sample; entry heaps were accumulated during construction.
    fn retained_bytes(&self) -> usize {
        let entries = self
            .files
            .capacity()
            .saturating_mul(size_of::<(PathBuf, Entry)>());
        let directories = self
            .directories
            .capacity()
            .saturating_mul(size_of::<PathBuf>());
        let buffer = self
            .reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|reader| {
                // The drain retains at most 64 KiB of raw stderr. Lossy UTF-8 conversion and
                // geometric String capacity can reserve up to four times that payload.
                reader
                    .output
                    .capacity()
                    .saturating_add((super::MAX_KEPT_STDERR_BYTES as usize).saturating_mul(4))
            })
            .unwrap_or(0);
        let problem = self
            .problem
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(String::capacity)
            .unwrap_or(0);
        let packages = self
            .project_packages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|packages| {
                packages.owned.saturating_add(
                    packages
                        .names
                        .capacity()
                        .saturating_mul(size_of::<String>()),
                )
            })
            .unwrap_or(0);
        [
            size_of::<Self>(),
            self.root.heap_bytes(),
            self.analysis_root.heap_bytes(),
            self.commit.heap_bytes(),
            self.owned,
            entries,
            directories,
            buffer,
            problem,
            packages,
        ]
        .into_iter()
        .fold(0, usize::saturating_add)
    }
}

/// Reap a listing abandoned by a parse, budget, or I/O error.
struct Listing(Child);

impl Drop for Listing {
    /// Never leave a tree-listing child behind after an early return.
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests;
