//! Package-aware filesystem boundary for the module resolver.

use oxc_resolver::{
    FileMetadata, FileSystem, FileSystemOs, Resolution, ResolveError, ResolveOptions,
    ResolverGeneric,
};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Resolver plus the first refused manifest from its current lookup.
pub(super) struct ManifestResolver {
    resolver: ResolverGeneric<ManifestFileSystem>,
    refused: Arc<Mutex<Option<PathBuf>>>,
}

impl ManifestResolver {
    /// Keeps ordinary resolver options and filesystem behavior, bounding only package manifests.
    pub(super) fn new(options: ResolveOptions) -> Self {
        let refused = Arc::new(Mutex::new(None));
        let fs = ManifestFileSystem {
            inner: FileSystemOs::new(),
            refused: Arc::clone(&refused),
        };
        Self {
            resolver: ResolverGeneric::new_with_file_system(fs, options),
            refused,
        }
    }

    /// Rejects the whole lookup if upstream tried to fall back past refused package metadata.
    pub(super) fn resolve_file(
        &self,
        file: &Path,
        specifier: &str,
    ) -> Result<Resolution, ResolveError> {
        let result = self.resolver.resolve_file(file, specifier);
        let refused = self
            .refused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(path) = refused {
            // Upstream treats read errors as missing metadata and caches ancestor/index fallback.
            // These resolvers are thread-local, so clearing cannot race another lookup.
            self.resolver.clear_cache();
            super::record_manifest_refusal(&path);
            return Err(ResolveError::InvalidPackageConfig(path));
        }
        result
    }
}

/// Delegates path handling to Oxc while retaining any rejected manifest read.
struct ManifestFileSystem {
    inner: FileSystemOs,
    refused: Arc<Mutex<Option<PathBuf>>>,
}

impl FileSystem for ManifestFileSystem {
    /// Constructs an independent filesystem for the resolver's generic default constructor.
    fn new() -> Self {
        Self {
            inner: FileSystemOs::new(),
            refused: Arc::default(),
        }
    }

    /// Uses the bounded UTF-8 reader for manifests; leaves other resolver data unchanged.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        if path.file_name().is_none_or(|name| name != "package.json") {
            return self.inner.read(path);
        }
        let result = crate::package_json::read(path).map(String::into_bytes);
        if result
            .as_ref()
            .is_err_and(|error| error.kind() != io::ErrorKind::NotFound)
        {
            self.refused
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get_or_insert_with(|| path.to_path_buf());
        }
        result
    }

    /// Preserves the same manifest policy when a resolver consumer requests text.
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        if path.file_name().is_some_and(|name| name == "package.json") {
            return self.read(path).and_then(|bytes| {
                String::from_utf8(bytes)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
            });
        }
        self.inner.read_to_string(path)
    }

    /// Keeps the resolver's metadata and symlink semantics.
    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        self.inner.metadata(path)
    }

    /// Keeps metadata about a link distinct from metadata about its target.
    fn symlink_metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        self.inner.symlink_metadata(path)
    }

    /// Delegates platform-specific link normalization to Oxc.
    fn read_link(&self, path: &Path) -> Result<PathBuf, ResolveError> {
        self.inner.read_link(path)
    }

    /// Retains canonical paths for workspace links and package-boundary checks.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both resolver read interfaces enforce the manifest bound while preserving ordinary files.
    #[test]
    fn filesystem_contract_preserves_paths_and_bounds_manifest_reads() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let fs = ManifestFileSystem::new();
        let manifest = root.join("package.json");
        std::fs::write(&manifest, "{}").unwrap();
        assert_eq!(fs.read_to_string(&manifest).unwrap(), "{}");
        assert!(fs.metadata(&manifest).unwrap().is_file());
        assert!(fs.symlink_metadata(&manifest).unwrap().is_file());
        assert_eq!(
            fs.canonicalize(&manifest).unwrap(),
            manifest.canonicalize().unwrap()
        );
        assert!(fs.read_link(&manifest).is_err());
        std::fs::write(
            &manifest,
            " ".repeat(crate::package_json::MAX_BYTES as usize + 1),
        )
        .unwrap();
        assert_eq!(
            fs.read_to_string(&manifest).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(*fs.refused.lock().unwrap(), Some(manifest));
        let other = root.join("config.json");
        std::fs::write(&other, "{\"compilerOptions\":{}}").unwrap();
        assert_eq!(fs.read(&other).unwrap(), b"{\"compilerOptions\":{}}");
        assert_eq!(
            fs.read_to_string(&other).unwrap(),
            "{\"compilerOptions\":{}}"
        );
    }
}
