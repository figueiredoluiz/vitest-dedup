//! Module resolution: the project source file an import names.

use oxc_resolver::{ResolveOptions, Resolver, TsconfigDiscovery};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Extensions of the source files a project import may name.
const EXTENSIONS: [&str; 8] = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// The project source file `specifier` names from the module at `from`, as a bundler resolves it:
/// relative paths and tsconfig `paths` aliases. `None` for a package, a file under `node_modules`,
/// a file that is not JavaScript or TypeScript, or one outside `root`.
pub fn resolve(from: &Path, specifier: &str, root: &Path) -> Option<PathBuf> {
    let file = resolve_file(from, specifier, root)?;
    let source = file
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| EXTENSIONS.contains(&extension));
    source.then_some(file)
}

/// The project file of any type `specifier` names from the module at `from` (a stylesheet, a JSON
/// file), resolved as [`resolve`] does; `None` for a package, a file under `node_modules` or one
/// outside `root`.
pub fn resolve_file(from: &Path, specifier: &str, root: &Path) -> Option<PathBuf> {
    static RESOLVER: LazyLock<Resolver> = LazyLock::new(|| {
        Resolver::new(ResolveOptions {
            extensions: EXTENSIONS
                .iter()
                .map(|extension| format!(".{extension}"))
                .collect(),
            condition_names: vec!["module".into(), "import".into()],
            main_fields: vec!["module".into(), "main".into()],
            tsconfig: Some(TsconfigDiscovery::Auto),
            ..ResolveOptions::default()
        })
    });
    let file = RESOLVER.resolve_file(from, specifier).ok()?.into_path_buf();
    let in_packages = file
        .components()
        .any(|component| component.as_os_str() == "node_modules");
    // Compared canonically, so a symlinked or differently spelled root does not decide it.
    let inside = match (std::fs::canonicalize(&file), std::fs::canonicalize(root)) {
        (Ok(file), Ok(root)) => file.starts_with(root),
        _ => false,
    };
    (!in_packages && inside).then_some(file)
}
