//! Nuxt's generated TypeScript configuration, without evaluating project code.

use oxc_resolver::{FileMetadata, FileSystem, Resolution, ResolveError, ResolverGeneric};
use std::borrow::Cow;
use std::io;
use std::path::{Component, Path, PathBuf, Prefix};

/// Nuxt's supported configuration filenames, used as project markers only.
pub(crate) const CONFIG_NAMES: [&str; 5] = [
    "nuxt.config.ts",
    "nuxt.config.js",
    "nuxt.config.mjs",
    "nuxt.config.mts",
    "nuxt.config.cjs",
];

const LEGACY: &str = r#"{"extends":"./.nuxt/tsconfig.json"}"#;
const CONTEXTS: &str = r#"{"files":[],"references":[
    {"path":"./.nuxt/tsconfig.app.json"},
    {"path":"./.nuxt/tsconfig.server.json"},
    {"path":"./.nuxt/tsconfig.shared.json"},
    {"path":"./.nuxt/tsconfig.node.json"}
]}"#;

/// Preserve automatic discovery first; only an unconfigured Nuxt project gets a fallback.
pub(super) fn resolve_file<F: FileSystem + Clone + 'static>(
    resolver: &ResolverGeneric<F>,
    fs: &F,
    from: &Path,
    specifier: &str,
    root: &Path,
) -> Result<Resolution, ResolveError> {
    let from = resolver_path(from);
    let from = from.as_ref();
    if resolver.find_tsconfig(from)?.is_some() {
        return resolver.resolve_file(from, specifier);
    }
    let Some(project) = project(fs, from, root)? else {
        return resolver.resolve_file(from, specifier);
    };
    // A legacy Nuxt server config can exist on its own. App/shared/node configurations mark
    // the solution layout; all four references must load, just as in Nuxt's documented root.
    let mut contexts = false;
    for name in ["app", "shared", "node"] {
        contexts |= is_file(fs, &project.join(format!(".nuxt/tsconfig.{name}.json")))?;
    }
    let fs = GeneratedConfig {
        inner: fs.clone(),
        config: Some((
            project.join("tsconfig.json"),
            if contexts { CONTEXTS } else { LEGACY },
        )),
    };
    // A fresh resolver prevents an absent-config cache entry from hiding this virtual file,
    // and avoids retaining a separate cache for every project in a long-lived process.
    let generated = ResolverGeneric::new_with_file_system(fs, super::resolve::options());
    // Auto discovery skips I/O failures; validate the generated graph before selecting ownership.
    generated.resolve_tsconfig(project.join("tsconfig.json"))?;
    generated.resolve_file(from, specifier)
}

/// Oxc parses importers as specifiers: the question mark in a Windows verbatim prefix is a
/// query separator. Preserve the disk/UNC identity in its ordinary absolute path spelling.
fn resolver_path(path: &Path) -> Cow<'_, Path> {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Cow::Borrowed(path);
    };
    let mut ordinary = match prefix.kind() {
        Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:", char::from(drive))),
        Prefix::VerbatimUNC(server, share) => {
            let mut prefix = std::ffi::OsString::from(r"\\");
            prefix.push(server);
            prefix.push(r"\");
            prefix.push(share);
            PathBuf::from(prefix)
        }
        _ => return Cow::Borrowed(path),
    };
    ordinary.extend(components);
    Cow::Owned(ordinary)
}

/// The closest recognized project, bounded by the analysis root and nested package boundaries.
fn project<F: FileSystem>(
    fs: &F,
    from: &Path,
    root: &Path,
) -> Result<Option<PathBuf>, ResolveError> {
    let Ok(root) = fs.canonicalize(root) else {
        return Ok(None);
    };
    let Some(parent) = from.parent() else {
        return Ok(None);
    };
    let Ok(directory) = fs.canonicalize(parent) else {
        return Ok(None);
    };
    for directory in directory
        .ancestors()
        .take_while(|path| path.starts_with(&root))
    {
        // An existing user config remains authoritative even when it excludes this file or
        // could not be read by automatic discovery. No generated config overrides that choice.
        let config = directory.join("tsconfig.json");
        match fs.symlink_metadata(&config) {
            Err(error) if is_absent(fs, &config, &error) => {}
            _ => return Ok(None),
        }
        for name in CONFIG_NAMES {
            if is_file(fs, &directory.join(name))? {
                return Ok(Some(directory.to_owned()));
            }
        }
        let manifest = directory.join("package.json");
        if is_file(fs, &manifest)? {
            let text = fs.read_to_string(&manifest)?;
            let nuxt = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .is_some_and(|value| {
                    ["dependencies", "devDependencies", "optionalDependencies"]
                        .into_iter()
                        .any(|field| {
                            value
                                .get(field)
                                .and_then(serde_json::Value::as_object)
                                .is_some_and(|table| table.contains_key("nuxt"))
                        })
                });
            return Ok(nuxt.then(|| directory.to_owned()));
        }
    }
    Ok(None)
}

/// Foreign metadata error kinds need independent absence evidence from the same source view.
fn is_absent<F: FileSystem>(fs: &F, path: &Path, error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
        || matches!(fs.canonicalize(path), Err(error) if error.kind() == io::ErrorKind::NotFound)
}

/// Missing entries are ordinary absence; other source-authority failures stay errors.
fn is_file<F: FileSystem>(fs: &F, path: &Path) -> io::Result<bool> {
    match fs.metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if is_absent(fs, path, &error) => Ok(false),
        Err(error) => Err(error),
    }
}

/// One virtual root config; all generated content and metadata still use the selected view.
struct GeneratedConfig<F> {
    inner: F,
    config: Option<(PathBuf, &'static str)>,
}

impl<F: FileSystem> GeneratedConfig<F> {
    /// Only the absent project root config has synthetic content.
    fn content(&self, path: &Path) -> Option<&'static str> {
        self.config
            .as_ref()
            .filter(|(config, _)| {
                config == path
                    || (path.file_name() == config.file_name()
                        && path
                            .parent()
                            .and_then(|parent| self.inner.canonicalize(parent).ok())
                            .as_deref()
                            == config.parent())
            })
            .map(|(_, text)| *text)
    }
}

impl<F: FileSystem> FileSystem for GeneratedConfig<F> {
    /// Default construction never manufactures a generated config or changes source authority.
    fn new() -> Self {
        Self {
            inner: F::new(),
            config: None,
        }
    }

    /// Generated files retain the underlying reader's size and package-manifest checks.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.content(path).map_or_else(
            || self.inner.read(path),
            |text| Ok(text.as_bytes().to_vec()),
        )
    }

    /// Expose only the synthetic root text; real files use the original text reader.
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.content(path).map_or_else(
            || self.inner.read_to_string(path),
            |text| Ok(text.to_owned()),
        )
    }

    /// Let automatic discovery see the in-memory root as a regular file.
    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        self.content(path).map_or_else(
            || self.inner.metadata(path),
            |_| Ok(FileMetadata::new(true, false, false)),
        )
    }

    /// The virtual root is not a link; other entries keep their original link identity.
    fn symlink_metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        self.content(path).map_or_else(
            || self.inner.symlink_metadata(path),
            |_| Ok(FileMetadata::new(true, false, false)),
        )
    }

    /// No generated link targets are invented.
    fn read_link(&self, path: &Path) -> Result<PathBuf, ResolveError> {
        self.inner.read_link(path)
    }

    /// The synthetic root is already under the source authority's canonical project directory.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.content(path).map_or_else(
            || self.inner.canonicalize(path),
            |_| {
                Ok(self
                    .config
                    .as_ref()
                    .expect("synthetic content has an identity")
                    .0
                    .clone())
            },
        )
    }
}

#[cfg(test)]
mod tests;
