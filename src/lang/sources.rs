//! Explicit source views for project imports, with no OS fallback from a committed tree.
use super::resolve;
use oxc_resolver::{FileMetadata, FileSystem, ResolveError, ResolverGeneric};
use std::fmt::Debug;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// Owned filesystem facts; parser-library types do not cross this boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// Regular file content.
    File,
    /// A directory in the committed tree.
    Directory,
    /// A symbolic link, never followed by a historical view.
    Symlink,
    /// A nonregular entry such as a submodule.
    Other,
}

/// An immutable, bounded source catalog owned by one historical analysis.
pub trait SourceTree: Debug + Send + Sync {
    /// Read committed bytes, enforcing the source-size limit before allocation.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    /// Classify a path without consulting the host filesystem.
    fn kind(&self, path: &Path) -> Option<SourceKind>;
    /// Normalize an existing virtual path without following a host symlink.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;
    /// Record an actual dependency/configuration failure for the caller to refuse.
    fn reject(&self, message: String);
    /// Record absent package configuration without trusting identities formed after the gap.
    fn configuration_gap(&self);
    /// Record an unresolved bare package that may name committed project source.
    fn unresolved_package(&self, name: &str);
    /// Constant-time retained ownership, charged once by the run, not by cache aliases.
    fn retained_bytes(&self) -> usize;
}

/// The source authority shared by every import consumer and its run-scoped caches.
#[derive(Clone, Debug, Default)]
pub struct Sources(Option<Arc<dyn SourceTree>>);

impl Sources {
    /// Relativize identities through this authority, including equivalent host/root spellings.
    /// A committed catalog never borrows filesystem canonicalization from the working tree.
    pub(crate) fn relative(&self, path: &Path, root: &Path) -> Option<PathBuf> {
        let canonical = |path: &Path| match &self.0 {
            None => std::fs::canonicalize(path),
            Some(tree) => tree.canonicalize(path),
        };
        canonical(path)
            .ok()?
            .strip_prefix(canonical(root).ok()?)
            .ok()
            .map(Path::to_path_buf)
    }

    /// Pin a committed source tree; a missing virtual path can never become a working file.
    pub fn committed(tree: Arc<dyn SourceTree>) -> Self {
        Self(Some(tree))
    }

    /// Bounded project-source text from the selected view.
    pub(crate) fn read(&self, path: &Path) -> Option<String> {
        match &self.0 {
            None => crate::discovery::read_bounded(path, crate::discovery::MAX_SOURCE_FILE_BYTES)
                .ok()
                .flatten(),
            Some(tree) => text(tree, path).ok(),
        }
    }

    /// A project JavaScript/TypeScript import, under the same policy in either view.
    pub(crate) fn resolve(&self, from: &Path, specifier: &str, root: &Path) -> Option<PathBuf> {
        let file = self.resolve_file(from, specifier, root)?;
        file.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| resolve::EXTENSIONS.contains(&extension))
            .then_some(file)
    }

    /// How a static import names its module in a key: the file it resolves to, root-relative so
    /// one tree keys alike wherever it lies, or its spelling when it resolves to none, each tagged
    /// so a project path never reads as a package spelling. The resolver drops a `?query` or
    /// `#fragment` that Vite still loads differently, so it is kept.
    pub(crate) fn module_key(&self, from: &Path, specifier: &str, root: &Path) -> String {
        let Some(file) = self.resolve_file(from, specifier, root) else {
            return format!("spelled:{specifier}");
        };
        let module = self.relative(&file, root).unwrap_or(file);
        format!("file:{}{}", module.display(), query(specifier))
    }

    /// Resolve any project file, isolating every resolver metadata/configuration read as well.
    pub(crate) fn resolve_file(
        &self,
        from: &Path,
        specifier: &str,
        root: &Path,
    ) -> Option<PathBuf> {
        let Some(tree) = &self.0 else {
            return resolve::resolve_file(from, specifier, root);
        };
        // This resolver lasts one lookup; its opaque caches are never retained by the engine.
        let resolver = ResolverGeneric::new_with_file_system(
            CommittedFs(Some(Arc::clone(tree))),
            resolve::options(),
        );
        // A verbatim Windows prefix contains `?`, which the resolver parses as a query.
        // Use the catalog's identity; supplied caller text may have no catalog entry.
        let from = tree.canonicalize(from).unwrap_or_else(|_| from.to_owned());
        let file = match resolver.resolve_file(&from, specifier) {
            Ok(file) => file.into_path_buf(),
            Err(
                error @ (ResolveError::NotFound(_)
                | ResolveError::Builtin { .. }
                | ResolveError::Ignored(_)),
            ) => {
                record_package_absence(&error, tree.as_ref());
                return None;
            }
            Err(error) if absent_package_config(&error, tree.as_ref()) => {
                tree.configuration_gap();
                return None;
            }
            Err(error) => {
                tree.reject(format!(
                    "historical resolution from {} for {specifier:?} failed: {error}",
                    from.display()
                ));
                return None;
            }
        };
        resolve::project_file(file, root, |path| tree.canonicalize(path))
    }
}

/// The `?query` or `#fragment` ending `specifier`, which the resolver drops and Vite loads as a
/// different module; empty when there is none. A leading `#` is a package import, not a fragment.
pub(crate) fn query(specifier: &str) -> &str {
    specifier
        .char_indices()
        .find(|&(at, c)| c == '?' || (c == '#' && at > 0))
        .map_or("", |(at, _)| &specifier[at..])
}

/// Builtins and ignored modules are not evidence of missing project-package linkage.
fn record_package_absence(error: &ResolveError, tree: &dyn SourceTree) {
    if let ResolveError::NotFound(missing) = error {
        if let Some(name) = package_name(missing) {
            tree.unresolved_package(name);
        }
    }
}

/// The package portion of a recorded bare request, excluding paths, private imports and URLs.
fn package_name(specifier: &str) -> Option<&str> {
    if specifier.starts_with(['.', '/', '#']) {
        return None;
    }
    let path = specifier.split(['?', '#']).next()?;
    if path.contains(['\\', ':']) {
        return None;
    }
    let mut parts = path.split('/');
    let first = parts.next().filter(|part| !part.is_empty())?;
    if first.starts_with('@') {
        let second = parts.next().filter(|part| !part.is_empty())?;
        Some(&path[..first.len() + 1 + second.len()])
    } else {
        Some(first)
    }
}

/// Only typed absence of package configuration may degrade the pass instead of refusing it.
fn absent_package_config(error: &ResolveError, tree: &dyn SourceTree) -> bool {
    match error {
        ResolveError::TsconfigNotFound(path) => {
            let bare = path.to_str().is_some_and(|text| {
                !text.is_empty()
                    && !text.starts_with(['.', '#'])
                    && !text.contains(['\\', ':'])
                    && path
                        .components()
                        .all(|part| matches!(part, Component::Normal(_)))
            });
            bare || absent_committed_package_path(path, tree)
        }
        ResolveError::TsconfigLoadFailed { path, source } => {
            absent_package_config(source, tree)
                || matches!(source.as_ref(), ResolveError::IOError(error)
                    if io::Error::from(error.clone()).kind() == io::ErrorKind::NotFound
                        && absent_committed_package_path(path, tree))
        }
        _ => false,
    }
}

/// An absolute package path must be absent inside the virtual namespace, never on the host.
fn absent_committed_package_path(path: &Path, tree: &dyn SourceTree) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        && path.ancestors().any(|ancestor| {
            ancestor
                .file_name()
                .is_some_and(|name| name == "node_modules")
                && ancestor
                    .parent()
                    .is_some_and(|parent| tree.kind(parent) == Some(SourceKind::Directory))
        })
        && match tree.kind(path) {
            None => true,
            Some(SourceKind::Directory) => tree.kind(&path.join("tsconfig.json")).is_none(),
            Some(_) => false,
        }
}

impl crate::memory::HeapBytes for Sources {
    /// Runs charge the shared catalog once; inventory/export cache aliases do not charge it.
    fn heap_bytes(&self) -> usize {
        self.0.as_ref().map_or(0, |tree| tree.retained_bytes())
    }
}

/// Decode source/config text and record invalid committed UTF-8 as an actual failed read.
fn text(tree: &Arc<dyn SourceTree>, path: &Path) -> io::Result<String> {
    let bytes = tree.read(path)?;
    String::from_utf8(bytes).map_err(|_| {
        let message = format!(
            "historical dependency {} is not valid UTF-8",
            path.display()
        );
        tree.reject(message.clone());
        io::Error::new(io::ErrorKind::InvalidData, message)
    })
}

/// The default required by the resolver trait is an empty filesystem, never OS access.
#[derive(Default)]
struct CommittedFs(Option<Arc<dyn SourceTree>>);

impl CommittedFs {
    /// An uninitialized adapter fails closed even if a future resolver calls its factory.
    fn tree(&self) -> io::Result<&Arc<dyn SourceTree>> {
        self.0
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no committed source view"))
    }
}

impl FileSystem for CommittedFs {
    /// A factory cannot manufacture access to the working tree.
    fn new() -> Self {
        Self::default()
    }

    /// All content is supplied by the pinned catalog.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.tree()?.read(path)
    }

    /// Resolver configuration uses the same bounded committed bytes as source files.
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        text(self.tree()?, path)
    }

    /// Symbolic links and other nonregular entries are refused instead of traversed.
    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        let tree = self.tree()?;
        match tree.kind(path) {
            Some(SourceKind::File) => Ok(FileMetadata::new(true, false, false)),
            Some(SourceKind::Directory) => Ok(FileMetadata::new(false, true, false)),
            Some(SourceKind::Symlink | SourceKind::Other) => {
                let message = format!(
                    "historical dependency {} is not a regular file or directory",
                    path.display()
                );
                tree.reject(message.clone());
                Err(io::Error::new(io::ErrorKind::Unsupported, message))
            }
            None => Err(io::ErrorKind::NotFound.into()),
        }
    }

    /// Historical links are never followed, so lstat and stat share the refusal policy.
    fn symlink_metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        self.metadata(path)
    }

    /// No link target is ever read from either the host or committed link content.
    fn read_link(&self, _path: &Path) -> Result<PathBuf, ResolveError> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "historical symbolic links are not followed",
        )
        .into())
    }

    /// Canonicalization stays within the virtual namespace.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.tree()?.canonicalize(path)
    }
}

#[cfg(test)]
mod tests;
