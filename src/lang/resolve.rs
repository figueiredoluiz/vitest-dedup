//! Module resolution: the project source file an import names.

use oxc_resolver::{ResolveOptions, Resolver, TsconfigDiscovery};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// Extensions of the source files a project import may name.
pub(super) const EXTENSIONS: [&str; 8] = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// Identical resolution policy for working files and committed virtual files.
pub(super) fn options() -> ResolveOptions {
    ResolveOptions {
        extensions: EXTENSIONS
            .iter()
            .map(|extension| format!(".{extension}"))
            .collect(),
        condition_names: vec!["module".into(), "import".into()],
        main_fields: vec!["module".into(), "main".into()],
        tsconfig: Some(TsconfigDiscovery::Auto),
        ..ResolveOptions::default()
    }
}

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
    // Package manifests are read through the bounded reader: a FIFO, a device or an oversized
    // `package.json` is refused, never opened and waited on.
    static RESOLVER: LazyLock<super::manifest_fs::ManifestResolver> = LazyLock::new(|| {
        super::manifest_fs::ManifestResolver::new(options(), record_manifest_refusal)
    });
    // The resolver keeps one refusal slot per lookup: a lookup and the drain of its slot run under
    // one lock, so threads resolving at once cannot take each other's refusal.
    static LOOKUP: Mutex<()> = Mutex::new(());
    let resolved = {
        let _lookup = LOOKUP
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        RESOLVER.resolve_file(from, specifier)
    };
    if let Err(error) = &resolved {
        record_gap(from, specifier, error);
    }
    let file = resolved.ok()?.into_path_buf();
    let canonicalize = |path: &Path| std::fs::canonicalize(path);
    let inside = inside_root(&file, root, &canonicalize);
    let projected = (!in_packages(&file) && inside).then_some(file.clone());
    if projected.is_none() && !inside {
        // Confined when the configuration reaches past the root; a package inside the root is
        // opaque, not confined.
        flag_confinement(from, specifier, &file);
    }
    projected
}

thread_local! {
    /// Set on the analyzing thread when a resolution needed a package manifest the bounded reader
    /// refused: what the file imports from that package is unknown.
    static MANIFEST_REFUSED: Cell<usize> = const { Cell::new(0) };

    /// Incremented on the analyzing thread when a resolution leaves the analysis root through
    /// the tsconfig: the file's context depends on configuration the run must not silently
    /// ignore. A count, because one file's read walks several modules and each refusal counts.
    static CONFINEMENT_REFUSED: Cell<usize> = const { Cell::new(0) };
}

/// Package-style imports that were not found, per directory of the importing file: what the
/// missing-prerequisite report counts against each project's install. Process-wide, as
/// [`TSCONFIG_GAPS`] is.
static PACKAGE_GAPS: Mutex<BTreeMap<PathBuf, usize>> = Mutex::new(BTreeMap::new());

/// The imports a gap was recorded for, by importing module and specifier: several readers resolve
/// one import, and the report counts imports.
static RECORDED: Mutex<std::collections::BTreeSet<(PathBuf, String)>> =
    Mutex::new(std::collections::BTreeSet::new());

/// Records why `specifier`, imported by the module at `from`, did not resolve, once per import.
fn record_gap(from: &Path, specifier: &str, error: &oxc_resolver::ResolveError) {
    let gap = classify_tsconfig_gap(error);
    let package = gap.is_none()
        && matches!(error, oxc_resolver::ResolveError::NotFound(_))
        && super::packages::is_package_specifier(specifier);
    if gap.is_none() && !package {
        return;
    }
    let first = RECORDED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert((from.to_path_buf(), specifier.to_owned()));
    if !first {
        return;
    }
    if let Some(gap) = gap {
        *TSCONFIG_GAPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(gap)
            .or_insert(0) += 1;
    } else {
        let directory = from.parent().unwrap_or(from).to_path_buf();
        *PACKAGE_GAPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(directory)
            .or_insert(0) += 1;
    }
}

/// Takes and clears the package imports not found since the last call, per importing directory.
pub fn take_package_gaps() -> BTreeMap<PathBuf, usize> {
    forget_recorded();
    std::mem::take(
        &mut *PACKAGE_GAPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Clears which imports were recorded, so the next run counts its own.
fn forget_recorded() {
    RECORDED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// Records a refused package manifest on the analyzing thread.
fn record_manifest_refusal(_path: &Path) {
    MANIFEST_REFUSED.with(|refused| refused.set(refused.get() + 1));
}

/// Takes whether the calling thread needed a package manifest the bounded reader refused since the
/// last take. The corpus drains it per file.
pub fn take_manifest_refusal() -> bool {
    MANIFEST_REFUSED.with(|refused| refused.replace(0)) > 0
}

/// How many manifest refusals the calling thread has recorded since the last take. The module
/// caches compare before and after an analysis to see whether it needed one.
pub fn manifest_refusals() -> usize {
    MANIFEST_REFUSED.with(Cell::get)
}

/// Records a confinement refusal on the analyzing thread.
fn record_confinement_refusal() {
    CONFINEMENT_REFUSED.with(|flag| flag.set(flag.get() + 1));
}

/// How many confinement refusals the calling thread has recorded since the last take. The
/// module caches compare before and after an analysis to see whether it traversed one.
pub fn confinement_refusals() -> usize {
    CONFINEMENT_REFUSED.with(|flag| flag.get())
}

/// Takes the calling thread's confinement refusals, recorded by [`resolve_file`]. The corpus
/// drains them per file to mark the read partial.
pub fn take_confinement_refusal() -> bool {
    CONFINEMENT_REFUSED.with(|flag| flag.replace(0)) > 0
}

/// A resolution that left the analysis root counts as configuration confinement only when a
/// tsconfig-less resolver would not land on the same file: an import outside the root on its own
/// is a package or an external file, not this project's configuration reaching past the root.
fn flag_confinement(from: &Path, specifier: &str, file: &Path) {
    static PLAIN: LazyLock<Resolver> = LazyLock::new(|| {
        Resolver::new(ResolveOptions {
            tsconfig: None,
            ..options()
        })
    });
    let without = PLAIN
        .resolve_file(from, specifier)
        .ok()
        .map(oxc_resolver::Resolution::into_path_buf);
    if without.as_deref() != Some(file) {
        record_confinement_refusal();
    }
}

/// A tsconfig the resolver could not use, as the resolver itself reported it. A project whose
/// tsconfig is in this state is config-degraded: the shell names the file and marks the run
/// incomplete.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TsconfigGap {
    /// The file does not exist.
    Absent(PathBuf),
    /// The file exists and could not be loaded.
    Unreadable(PathBuf),
}

/// Process-wide on purpose: resolution runs on the extraction's scoped threads and the shell
/// drains the total once per run.
static TSCONFIG_GAPS: Mutex<BTreeMap<TsconfigGap, usize>> = Mutex::new(BTreeMap::new());

/// Resolutions that failed because something the run needs is absent, drained once per run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolutionGaps {
    /// Failed resolutions per unusable tsconfig. A tsconfig that cannot be loaded fails every
    /// resolution under it, relative specifiers included.
    pub tsconfig: BTreeMap<TsconfigGap, usize>,
    /// Package-style specifiers that were not found, per directory of the importing file: the
    /// manifest that governs a file is its directory's business, so the count is kept where it can
    /// be attributed.
    pub packages_not_found: BTreeMap<PathBuf, usize>,
}

/// Takes and clears the tsconfig gaps recorded since the last call.
pub fn take_tsconfig_gaps() -> BTreeMap<TsconfigGap, usize> {
    forget_recorded();
    std::mem::take(
        &mut *TSCONFIG_GAPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Classifies why a resolution failed when the cause is an unusable tsconfig, so a broken project
/// configuration is reported once, by one classification.
pub fn classify_tsconfig_gap(error: &oxc_resolver::ResolveError) -> Option<TsconfigGap> {
    use oxc_resolver::ResolveError;
    match error {
        ResolveError::TsconfigNotFound(target) => Some(TsconfigGap::Absent(target.clone())),
        ResolveError::TsconfigLoadFailed { path, .. } if !path.exists() => {
            Some(TsconfigGap::Absent(path.clone()))
        }
        ResolveError::TsconfigLoadFailed { path, .. } => {
            Some(TsconfigGap::Unreadable(path.clone()))
        }
        _ => None,
    }
}

/// Packages and paths outside the analysis root stay opaque in either source view.
pub(super) fn project_file(
    file: PathBuf,
    root: &Path,
    canonicalize: impl Fn(&Path) -> std::io::Result<PathBuf>,
) -> Option<PathBuf> {
    (inside_root(&file, root, &canonicalize) && !in_packages(&file)).then_some(file)
}

/// Whether the file sits inside the root, compared canonically so a symlinked or differently
/// spelled root does not decide it.
fn inside_root(
    file: &Path,
    root: &Path,
    canonicalize: &impl Fn(&Path) -> std::io::Result<PathBuf>,
) -> bool {
    match (canonicalize(file), canonicalize(root)) {
        (Ok(file), Ok(root)) => file.starts_with(root),
        _ => false,
    }
}

/// Whether the file lives under a `node_modules` component: a package dependency, opaque to the
/// analyzer.
fn in_packages(file: &Path) -> bool {
    file.components()
        .any(|component| component.as_os_str() == "node_modules")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique per-test directories: the gap registry is process-wide and `cargo test` runs
    /// tests in parallel, so one test's tsconfig must never be another's.
    #[test]
    fn tsconfig_failures_are_classified_as_absent_or_unreadable() {
        // Unique per-project directories: the gap registry is process-wide and `cargo test`
        // runs tests in parallel, so one test's tsconfig must never be another's.
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("absent");
        let unreadable = dir.path().join("unreadable");
        std::fs::create_dir_all(absent.clone()).unwrap();
        std::fs::create_dir_all(unreadable.join("generated")).unwrap();
        std::fs::write(
            absent.join("tsconfig.json"),
            "{ \"extends\": \"./generated/tsconfig.json\" }",
        )
        .unwrap();
        std::fs::write(
            unreadable.join("tsconfig.json"),
            "{ \"extends\": \"./generated/tsconfig.json\" }",
        )
        .unwrap();
        std::fs::write(unreadable.join("generated/tsconfig.json"), "not json").unwrap();
        let absent_test = absent.join("a.test.ts");
        let unreadable_test = unreadable.join("b.test.ts");
        std::fs::write(&absent_test, "import { x } from '~/y';\n").unwrap();
        std::fs::write(&unreadable_test, "import { x } from '~/y';\n").unwrap();

        assert!(resolve_file(&absent_test, "~/y", &absent).is_none());
        assert!(resolve_file(&unreadable_test, "~/y", &unreadable).is_none());
        let gaps = take_tsconfig_gaps();
        let absent_gap = gaps
            .iter()
            .filter(
                |(gap, _)| matches!(gap, TsconfigGap::Absent(path) if path.starts_with(&absent)),
            )
            .map(|(_, count)| count)
            .sum::<usize>();
        let unreadable_gap = gaps
            .iter()
            .filter(|(gap, _)| matches!(gap, TsconfigGap::Unreadable(path) if path.starts_with(&unreadable)))
            .map(|(_, count)| count)
            .sum::<usize>();
        assert!(
            absent_gap >= 1,
            "the missing target is an absent gap: {gaps:?}"
        );
        assert!(
            unreadable_gap >= 1,
            "the unreadable file is an unreadable gap: {gaps:?}"
        );
    }

    #[test]
    fn a_tsconfig_alias_leaving_the_root_refuses_and_flags_confinement() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let outside = dir.path().join("outside");
        // The analysis root is `project`: the alias reaches its sibling, outside the root.
        std::fs::create_dir_all(project.join("utils")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        // One configuration, written before the first resolution (the resolver caches it): the
        // alias reaches past the root for `helper` and into this project's node_modules for
        // `types`, so each scenario exercises its own drop through the same loaded config.
        std::fs::write(
            project.join("tsconfig.json"),
            "{ \"compilerOptions\": { \"paths\": { \"~/*\": [\"../outside/*\", \"node_modules/@shim/*\", \"../outside/node_modules/*\"] } } }",
        )
        .unwrap();
        std::fs::write(outside.join("helper.ts"), "export const x = 1;\n").unwrap();
        std::fs::write(project.join("utils/local.ts"), "export const y = 2;\n").unwrap();
        let shim = project.join("node_modules/@shim").join("types.ts");
        std::fs::create_dir_all(shim.parent().unwrap()).unwrap();
        std::fs::write(&shim, "export const z = 3;\n").unwrap();
        let test = project.join("a.test.ts");
        std::fs::write(
            &test,
            "import { x } from '~/helper';\nimport { y } from './utils/local';\nimport { z } from '~/types';\n",
        )
        .unwrap();

        // The alias leaves the root through the tsconfig: refused, and the read is confined.
        assert!(resolve_file(&test, "~/helper", &project).is_none());
        assert!(take_confinement_refusal());
        // An import inside the root is not a confinement, and the flag does not stick.
        assert!(resolve_file(&test, "./utils/local", &project).is_some());
        assert!(!take_confinement_refusal());
        // An alias into this project's node_modules is a package, not a confinement either: the
        // underlying resolver does select the shim, so the drop below is project_file's package
        // rule and not a failed lookup.
        let shimmed = Resolver::new(options())
            .resolve_file(&test, "~/types")
            .unwrap()
            .into_path_buf();
        assert!(in_packages(&shimmed));
        assert!(resolve_file(&test, "~/types", &project).is_none());
        assert!(!take_confinement_refusal());
        // A package outside the root is still the configuration reaching past the root: refused
        // AND confined.
        std::fs::create_dir_all(outside.join("node_modules/dep")).unwrap();
        std::fs::write(
            outside.join("node_modules/dep/index.ts"),
            "export const d = 4;\n",
        )
        .unwrap();
        assert!(resolve_file(&test, "~/dep", &project).is_none());
        assert!(take_confinement_refusal());
    }
}
