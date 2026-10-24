//! Real module resolution for free-import identity.
//!
//! `fingerprint::import_origins` used to anchor a relative specifier lexically -- string manipulation
//! on the importer's own path, no filesystem access (see the removed `normalize_relative_specifier`
//! in `fingerprint.rs`'s history). That stopped the cross-directory collision it was built for, but
//! it is not resolution: it does not follow tsconfig `paths`, package `exports` maps, or symlinked
//! workspace packages, so two specifiers that reach the same file on disk can still read as
//! different, and (for the anchoring scheme, not this one) a bare alias could misread as matching
//! an unrelated one. This module is the piece the fingerprint module doc's history section called
//! "a later milestone" -- it exists to answer "what file does this specifier actually name",
//! authoritatively, via [`oxc_resolver`].
//!
//! `oxc_resolver` is a standalone crate, independently versioned (11.x, ordinary semver) from the
//! pinned-at-0.149.0 `oxc_*` parser/AST/semantic family this codebase otherwise depends on. It was
//! considered and deliberately deferred, not rejected, when the parser-containment rule in
//! `AGENTS.md` was written: "only `src/frontend/` may import an `oxc_*` crate" was written with the
//! parser/AST/semantic crates in mind, but the rule is about the *directory*, not the parser
//! specifically, so `oxc_resolver` living here is exactly what that rule already permits -- there is
//! no separate exception to grant.
//!
//! ## Cost and determinism
//!
//! Filesystem resolution is neither instantaneous nor (across a mutable tree, in principle) stable
//! within a single process run the way pure string manipulation is. Two things keep this module
//! from turning the fingerprinter into something that silently gets slower or flakier as it scales:
//!
//! - **Memoized, not repeated.** [`resolve`] is keyed on `(importing file, specifier)` and cached
//!   for the lifetime of the extraction worker (see [`CACHE`]). `import_origins` runs once per
//!   extracted file and again for each helper file it inlines; without memoization, every one of
//!   those runs would pay a real filesystem walk per specifier. The cache collapses it to one
//!   resolution per distinct specifier actually written in a given file, which is the bound this
//!   design requires: resolve once per (importing file, specifier), never once per call site.
//!
//!   The key is the importing *file*, not its directory, so two files in one directory each pay
//!   for their own first resolution of the same specifier. Directory keying would be sound --
//!   `oxc_resolver` resolves relative specifiers against the importing file's directory, and
//!   walks up from that same directory for bare ones -- and would collapse those duplicates. It
//!   is deliberately not done here: it is a change to what the cache *means*, not a correction,
//!   and the duplicate cost is one resolution per file per specifier, which the measurement in
//!   `fingerprint`'s doc comment already accounts for.
//! - **Never resolved mid-traversal.** [`resolve`] is only ever called from `import_origins`, which
//!   runs once per file, before any body's identifier walk begins. The serializer and behavior collector never
//!   touch the filesystem themselves -- they consume the already-resolved [`Resolved`] value.
//! - **A thread-local cache, not a global lock.** Extraction runs on an explicitly sized worker
//!   stack. Each worker retains its resolver and memoized results throughout the file and all of
//!   its helper/fingerprint passes, then drops them. Independent extractions do not share cache
//!   state or contend for a lock. Resolution still assumes the analyzed tree is unchanged during
//!   a run; cache lifetime only affects speed, not the result under that assumption.
//!
//! `fingerprint`'s own doc comment records the measured wall-clock cost of adding this on the
//! external corpus.
//!
//! ## Confinement
//!
//! A tsconfig `paths` or `baseUrl` entry is configuration from the analyzed repository, and can
//! name a file anywhere on disk. When the extraction names an analysis root ([`begin_file`]), a
//! resolution that leaves it is accepted only if a resolver with no tsconfig reaches the same file
//! from the same specifier: a relative import, or a symlinked workspace package, is not
//! configuration-derived and keeps resolving. Anything else is refused as
//! [`Resolved::Unresolved`], never opened, and recorded for [`take_refusals`] so the file's
//! extraction says so.

use oxc_resolver::{ResolveOptions, Resolver, TsconfigDiscovery};
use oxc_span::VALID_EXTENSIONS;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A tsconfig the resolver could not use, as the resolver itself reported it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TsconfigGap {
    /// The file does not exist (an absolute path, or a package specifier as written).
    Absent(PathBuf),
    /// The file exists and could not be loaded.
    Unreadable(PathBuf),
}

/// Resolutions that failed because something the run needs is absent, collected across workers
/// and drained once per run. See [`take_resolution_gaps`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolutionGaps {
    /// Failed resolutions per unusable tsconfig. A tsconfig that cannot be loaded fails every
    /// resolution under it, relative specifiers included.
    pub tsconfig: BTreeMap<TsconfigGap, usize>,
    /// Package-style specifiers (not relative, absolute, `vitest` or `node:`) that were not found,
    /// per directory of the importing file: the manifest that governs a file is its directory's
    /// business, so the count is kept where it can be attributed.
    pub packages_not_found: BTreeMap<PathBuf, usize>,
}

/// Process-wide on purpose: extraction runs on worker threads and the run drains the total once.
/// One analysis per process, as the binary is; an embedder running several concurrently would see
/// their gaps mixed.
static GAPS: Mutex<ResolutionGaps> = Mutex::new(ResolutionGaps {
    tsconfig: BTreeMap::new(),
    packages_not_found: BTreeMap::new(),
});

/// Held by tests that record into [`GAPS`] and read it back, or that drain it, so they do not
/// take each other's entries.
#[cfg(test)]
pub(crate) static GAPS_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Takes and clears the gaps recorded since the last call.
pub fn take_resolution_gaps() -> ResolutionGaps {
    // The end of a run: what was read about installed packages is no longer current.
    super::package_manifests::forget_reached();
    std::mem::take(
        &mut *GAPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Node core module specifiers that are imported without the `node:` prefix and are never installed:
/// `module.builtinModules` of Node 26 without its prefixed entries, plus the `_stream_*` names that
/// earlier versions still list. Matched exactly, subpaths included, so `fs/promises` is a builtin
/// and `fs/not-real` is a package lookup.
const NODE_BUILTINS: &[&str] = &[
    "_http_agent",
    "_http_client",
    "_http_common",
    "_http_incoming",
    "_http_outgoing",
    "_http_server",
    "_tls_common",
    "_stream_duplex",
    "_stream_passthrough",
    "_stream_readable",
    "_stream_transform",
    "_stream_wrap",
    "_stream_writable",
    "_tls_wrap",
    "assert",
    "assert/strict",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "dns/promises",
    "domain",
    "events",
    "fs",
    "fs/promises",
    "http",
    "http2",
    "https",
    "inspector",
    "inspector/promises",
    "module",
    "net",
    "os",
    "path",
    "path/posix",
    "path/win32",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "readline/promises",
    "repl",
    "stream",
    "stream/consumers",
    "stream/promises",
    "stream/web",
    "string_decoder",
    "sys",
    "timers",
    "timers/promises",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "util/types",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

/// Whether `specifier` is spelled like a request for an installed package rather than as something
/// that is plainly not one: a relative, absolute or `#`-internal path, a `~`, `~~`, `@@` or `@/` alias, a URI
/// scheme or drive path (a colon in the first segment, `npm:` and `jsr:` included), a backslash
/// path, a package name with whitespace, or a scope with no name. Only the package part is judged: a
/// subpath or a `?query` suffix may contain anything, and legacy names are accepted, because
/// refusing a real package would hide a missing install. An alias written like a package
/// (`@app/missing`) cannot be told apart by its spelling.
fn has_package_syntax(specifier: &str) -> bool {
    let head = specifier.split(['?', '#']).next().unwrap_or_default();
    if head.is_empty()
        || head.starts_with(['.', '/'])
        || ["~", "~~", "@@"].contains(&head)
        || ["~/", "~~/", "@@/"]
            .iter()
            .any(|alias| head.starts_with(alias))
        || head.contains('\\')
    {
        return false;
    }
    let mut parts = head.split('/');
    let first = parts.next().unwrap_or_default();
    let plain = |segment: &str| !segment.contains(':') && !segment.contains(char::is_whitespace);
    match first.strip_prefix('@') {
        Some(scope) => {
            let name = parts.next().unwrap_or_default();
            !scope.is_empty() && !name.is_empty() && plain(scope) && plain(name)
        }
        None => plain(first),
    }
}

/// Whether an unresolved `specifier` could still name a first-party module: it is neither a Node
/// core module nor written as a package name (a path, or an alias such as `~/x` or `@/x`).
pub(super) fn may_name_first_party(specifier: &str) -> bool {
    !is_node_builtin(specifier) && !has_package_syntax(specifier)
}

/// Whether `specifier` is a Node core module (prefixed, or one of the unprefixed names).
pub(super) fn is_node_builtin(specifier: &str) -> bool {
    specifier.starts_with("node:") || NODE_BUILTINS.contains(&specifier)
}

/// Whether `specifier` names an installable package the analyzer does not resolve by name.
///
/// Node's prefix-only modules (`node:sqlite`, `node:sea`, `node:test`) are not in the builtin list:
/// without the prefix those names are ordinary packages, and `test` is a real one.
fn is_package_specifier(specifier: &str) -> bool {
    has_package_syntax(specifier)
        && !(specifier.starts_with("node:")
            || NODE_BUILTINS.contains(&specifier)
            || specifier == "vitest"
            || specifier.starts_with("vitest/")
            || specifier.starts_with("@vitest/"))
}

/// Records why a resolution failed when the cause is an absent prerequisite.
fn record_failure(importing_file: &Path, specifier: &str, error: &oxc_resolver::ResolveError) {
    use oxc_resolver::ResolveError;
    let gap = match error {
        ResolveError::TsconfigNotFound(target) => Some(TsconfigGap::Absent(target.clone())),
        ResolveError::TsconfigLoadFailed { path, .. } if !path.exists() => {
            Some(TsconfigGap::Absent(path.clone()))
        }
        ResolveError::TsconfigLoadFailed { path, .. } => {
            Some(TsconfigGap::Unreadable(path.clone()))
        }
        _ => None,
    };
    let mut gaps = GAPS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match gap {
        Some(gap) => *gaps.tsconfig.entry(gap).or_insert(0) += 1,
        None if matches!(error, ResolveError::NotFound(_)) && is_package_specifier(specifier) => {
            let directory = importing_file.parent().unwrap_or(importing_file);
            *gaps
                .packages_not_found
                .entry(directory.to_path_buf())
                .or_insert(0) += 1;
        }
        None => {}
    }
}

thread_local! {
    /// One resolver per worker, built lazily and reused until that extraction completes.
    /// `TsconfigDiscovery::Auto` means each call finds *its own* nearest `tsconfig.json` by
    /// walking up from the importing file -- the right default for a monorepo with one
    /// `tsconfig.json` per package (two of the pinned corpora are), and equally correct for a single
    /// project with one config at the root. There is no CLI-level tsconfig override to plumb
    /// through here, so `Auto` is not a placeholder for a future flag -- it is the whole policy.
    static RESOLVER: Resolver = build_resolver(Some(TsconfigDiscovery::Auto));

    /// The same options with tsconfig handling off: the oracle for "would this specifier resolve
    /// there without any tsconfig", consulted only for a resolution that leaves the analysis root.
    static PLAIN_RESOLVER: Resolver = build_resolver(None);

    /// The analysis root the current extraction confines tsconfig-derived resolutions to.
    static ANALYSIS_ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };

    /// Resolutions refused by confinement during the current extraction.
    static REFUSALS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };

    /// Memoizes [`resolve`] for the lifetime of this thread. See the module doc's cost section for
    /// why this is the mechanism that keeps resolution from being paid for on every test body
    /// rather than once per specifier actually written in a file.
    static CACHE: RefCell<HashMap<(PathBuf, String), Resolved>> = RefCell::new(HashMap::new());

    /// Set inside [`quietly`]: a confinement refusal is neither recorded nor memoized.
    static QUIET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `body` with failures unrecorded (confinement refusals and resolution gaps): for an
/// optional lookup whose failure must not mark the file incomplete or report a prerequisite. A
/// failed result is not memoized either, so a later lookup that needs the module records it.
pub(super) fn quietly<T>(body: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        /// Restores the enclosing setting, also when `body` panics.
        fn drop(&mut self) {
            QUIET.with(|quiet| quiet.set(self.0));
        }
    }
    let _restore = Restore(QUIET.with(|quiet| quiet.replace(true)));
    body()
}

/// Builds a worker resolver with parser-supported extensions, `module`/`import` conditions,
/// `module`/`main` entry fields and the given tsconfig handling. Other options retain their
/// resolver defaults.
fn build_resolver(tsconfig: Option<TsconfigDiscovery>) -> Resolver {
    Resolver::new(ResolveOptions {
        // The base `oxc_resolver` defaults only try `.js`/`.json`/`.node` -- correct for plain
        // Node resolution, wrong for a TypeScript test corpus, where `./helpers` routinely names
        // `helpers.ts`. `oxc_span::VALID_EXTENSIONS` is the same list this codebase's own parser
        // uses to decide what it can parse at all (`oxc_span = "=0.149.0"` is already a
        // dependency), so the extensions this resolver will try are guaranteed to match the
        // extensions the rest of this crate considers source files.
        extensions: VALID_EXTENSIONS
            .iter()
            .map(|ext| format!(".{ext}"))
            .collect(),
        // Matches the condition names `oxc_linter`'s own import resolver uses for the same
        // Node/TS-source-resolution problem, favoring a package's ESM/bundler entry point over a
        // CJS one when a package exports map offers both.
        condition_names: vec!["module".into(), "import".into()],
        main_fields: vec!["module".into(), "main".into()],
        tsconfig,
        ..ResolveOptions::default()
    })
}

/// The outcome of resolving one specifier as written in one importing file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Resolved {
    /// `oxc_resolver` found a real file on disk. The path is exactly what
    /// [`oxc_resolver::Resolution::into_path_buf`] returned -- absolute, symlinks followed (the
    /// default `symlinks: true`), extension and `exports`/`paths` resolution already applied.
    /// Two different specifiers that resolve to equal paths here name the same module; the caller
    /// (`free_identity` in `fingerprint.rs`) is what turns this into a fingerprint-safe string.
    Module(PathBuf),
    /// Resolution failed: an unresolvable specifier, a dependency that is not installed, a typo,
    /// or (in this codebase's own test suite, deliberately) a specifier resolved against a file
    /// that was never written to disk at all. See `free_identity`'s doc for why this must never be
    /// treated as if it were a `Module` naming some file -- doing so would let two independently
    /// broken imports that merely *look* alike collide.
    Unresolved,
}

/// Resolves `specifier`, exactly as it appears in an `import` statement, against `importing_file`
/// -- the file that statement lives in -- to a real file on disk, or [`Resolved::Unresolved`] if it
/// cannot be. Memoized per `(importing_file, specifier)` for the process's lifetime; see the module
/// doc's cost section.
///
/// `importing_file` is passed to `oxc_resolver::ResolverGeneric::resolve_file` rather than
/// `resolve` (the directory-based entry point) because only `resolve_file` honors
/// `TsconfigDiscovery::Auto` (per `oxc_resolver`'s own doc comment) -- and because a tsconfig's
/// `include`/`exclude` fields can in principle assign different files in the same directory to
/// different configs (project references), so resolving from the real file, not merely its parent
/// directory, is the more correct of the two available entry points, not just the one that
/// supports `Auto`.
///
/// Never panics: a nonexistent `importing_file`, a specifier with no matching file, a malformed
/// `tsconfig.json`, or any other `oxc_resolver` error all degrade to [`Resolved::Unresolved`]
/// rather than propagating -- matching this codebase's rule (see `fingerprint`'s doc comment) that
/// operational degradation stays visible as an under-match, never a crash.
pub(super) fn resolve(importing_file: &Path, specifier: &str) -> Resolved {
    let key = (importing_file.to_path_buf(), specifier.to_owned());
    if let Some(cached) = CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return cached;
    }
    let quiet = QUIET.with(std::cell::Cell::get);
    let resolved =
        RESOLVER.with(
            |resolver| match resolver.resolve_file(importing_file, specifier) {
                Ok(resolution) => Resolved::Module(resolution.into_path_buf()),
                Err(error) => {
                    if !quiet {
                        record_failure(importing_file, specifier, &error);
                    }
                    Resolved::Unresolved
                }
            },
        );
    let resolved = confine_resolution(importing_file, specifier, resolved);
    if quiet && resolved == Resolved::Unresolved {
        return resolved;
    }
    CACHE.with(|cache| {
        cache.borrow_mut().insert(key, resolved.clone());
    });
    resolved
}

/// Starts confining this thread's resolutions to `root` (`None`: no confinement) and forgets any
/// earlier file's memoized results and refusals. Each extraction runs on its own worker thread,
/// so in production there is nothing to forget; the reset keeps direct callers honest.
pub(super) fn begin_file(root: Option<&Path>) {
    ANALYSIS_ROOT.with(|slot| *slot.borrow_mut() = root.map(Path::to_path_buf));
    REFUSALS.with(|refusals| refusals.borrow_mut().clear());
    CACHE.with(|cache| cache.borrow_mut().clear());
}

/// Drains the diagnostics for resolutions refused since [`begin_file`].
pub(super) fn take_refusals() -> Vec<String> {
    REFUSALS.with(|refusals| std::mem::take(&mut *refusals.borrow_mut()))
}

/// Refuses a resolution that leaves the analysis root unless it is independent of tsconfig.
fn confine_resolution(importing_file: &Path, specifier: &str, resolved: Resolved) -> Resolved {
    let Resolved::Module(path) = &resolved else {
        return resolved;
    };
    let Some(root) = ANALYSIS_ROOT.with(|slot| slot.borrow().clone()) else {
        return resolved;
    };
    if crate::confine::contains(&root, path) {
        return resolved;
    }
    let without_tsconfig = PLAIN_RESOLVER
        .with(|resolver| resolver.resolve_file(importing_file, specifier))
        .ok()
        .map(oxc_resolver::Resolution::into_path_buf);
    if without_tsconfig.as_deref() == Some(path.as_path()) {
        return resolved;
    }
    let message = format!(
        "import `{specifier}` resolves outside the analysis root through the tsconfig and was \
         not read"
    );
    if !QUIET.with(std::cell::Cell::get) {
        REFUSALS.with(|refusals| {
            let mut refusals = refusals.borrow_mut();
            if !refusals.contains(&message) {
                refusals.push(message);
            }
        });
    }
    Resolved::Unresolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Two differently-spelled relative specifiers that both name the same file on disk must
    /// resolve to the same [`Resolved::Module`] path -- this is the whole point of real resolution
    /// over lexical anchoring, which had no way to know `./sibling/../target` and `./target` are
    /// the same file.
    #[test]
    fn different_specifiers_reaching_the_same_file_resolve_identically() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.ts"), "export const x = 1;").unwrap();
        fs::create_dir(dir.path().join("sibling")).unwrap();
        let importer = dir.path().join("importer.ts");
        fs::write(&importer, "// importer").unwrap();

        let direct = resolve(&importer, "./target");
        let roundabout = resolve(&importer, "./sibling/../target");

        assert_ne!(
            direct,
            Resolved::Unresolved,
            "direct specifier must resolve"
        );
        assert_eq!(
            direct, roundabout,
            "two specifiers naming the same file on disk must resolve to the same module"
        );
    }

    /// Two specifiers that look identical as text but are written from different directories, and
    /// so name different files, must resolve to different paths -- the case real resolution must
    /// not regress relative to the lexical anchoring it replaces.
    #[test]
    fn identical_specifier_text_from_different_directories_resolves_differently() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_a = dir.path().join("pkgA");
        let pkg_b = dir.path().join("pkgB");
        fs::create_dir(&pkg_a).unwrap();
        fs::create_dir(&pkg_b).unwrap();
        fs::write(pkg_a.join("helpers.ts"), "export const a = 1;").unwrap();
        fs::write(pkg_b.join("helpers.ts"), "export const a = 2;").unwrap();
        let importer_a = pkg_a.join("a.test.ts");
        let importer_b = pkg_b.join("b.test.ts");
        fs::write(&importer_a, "// a").unwrap();
        fs::write(&importer_b, "// b").unwrap();

        let resolved_a = resolve(&importer_a, "./helpers");
        let resolved_b = resolve(&importer_b, "./helpers");

        assert_ne!(resolved_a, Resolved::Unresolved);
        assert_ne!(resolved_b, Resolved::Unresolved);
        assert_ne!(
            resolved_a, resolved_b,
            "the same specifier text written from two different directories must resolve to two \
             different files"
        );
    }

    /// A specifier naming a file that does not exist must degrade to `Unresolved`, never panic,
    /// and never coincide with a real `Module` resolution.
    #[test]
    fn unresolvable_specifier_is_unresolved() {
        let dir = tempfile::tempdir().unwrap();
        let importer = dir.path().join("importer.ts");
        fs::write(&importer, "// importer").unwrap();

        assert_eq!(resolve(&importer, "./does-not-exist"), Resolved::Unresolved);
    }

    /// Only the modules Node accepts without a prefix are builtins; the prefix-only ones are
    /// builtins solely with `node:`, and the bare names are packages that can be missing.
    #[test]
    fn only_package_shaped_specifiers_are_counted_as_packages() {
        for package in [
            "left-pad",
            "lodash/fp",
            "@scope/pkg",
            "@scope/pkg/sub/path",
            "test",
            "a.b",
            "pkg_x",
            "@scope/_private",
            "@_scope/pkg",
            "old!pkg",
            "pkg?query",
            "@scope/pkg?x",
            "pkg#fragment",
            "Pkg-Upper",
            "pkg/sub:file",
            "pkg?a=b:c",
            "@scope/pkg?x=a:b",
            "pkg/sub file",
            "pkg?x=a b",
            "~pkg",
        ] {
            assert!(is_package_specifier(package), "{package}");
        }
        for other in [
            "~/x",
            "~",
            "~~/x",
            "@@/y",
            "@@",
            "@/y",
            "@scope",
            "@scope/",
            "https://example.com/z.js",
            "data:text/javascript,1",
            "file:///x",
            "C:/w/v",
            "C:\\w\\v",
            "\\\\host\\share",
            "./x",
            "../x",
            "/x",
            "#internal",
            "",
            ".hidden",
            " ",
            "a b",
            "npm:left-pad",
            "jsr:@std/path",
        ] {
            assert!(!is_package_specifier(other), "{other:?}");
        }
    }

    /// Only the modules Node accepts without a prefix are builtins; the prefix-only ones are
    /// builtins solely with `node:`, and the bare names are packages that can be missing.
    #[test]
    fn prefix_only_node_modules_are_packages_without_the_prefix() {
        for package in [
            "test",
            "sqlite",
            "sea",
            "test/reporters",
            "left-pad",
            "@scope/pkg",
            "fs/not-real",
            "path/nope",
        ] {
            assert!(is_package_specifier(package), "{package}");
        }
        for other in [
            "node:test",
            "node:sqlite",
            "node:sea",
            "fs",
            "fs/promises",
            "path",
            "path/posix",
            "assert/strict",
            "stream/consumers",
            "_http_agent",
            "_tls_wrap",
            "_stream_readable",
            ".",
            "./x",
            "/x",
            "#alias",
            "vitest",
            "vitest/config",
            "@vitest/ui",
        ] {
            assert!(!is_package_specifier(other), "{other}");
        }
    }

    /// A tsconfig that fails to load is a gap whether the resolver reports the file as missing
    /// (`TsconfigNotFound`) or wraps a load failure for a path that is not there: both are
    /// "absent". A load failure for a file that exists is "unreadable". Other errors record
    /// nothing. Matched on unique paths because the accumulator is shared by the whole process.
    #[test]
    fn tsconfig_failures_are_classified_as_absent_or_unreadable() {
        use oxc_resolver::ResolveError;
        let _gaps = GAPS_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let missing_a = dir.path().join("unique-classification-a/tsconfig.json");
        let missing_b = dir.path().join("unique-classification-b/tsconfig.json");
        let present = dir.path().join("unique-classification-c.json");
        fs::write(&present, "{").unwrap();
        let load_failed = |path: &Path| ResolveError::TsconfigLoadFailed {
            path: path.to_path_buf(),
            source: Box::new(ResolveError::TsconfigNotFound(path.to_path_buf())),
        };
        record_failure(
            Path::new("/p/a.test.ts"),
            "./x",
            &ResolveError::TsconfigNotFound(missing_a.clone()),
        );
        record_failure(Path::new("/p/a.test.ts"), "./x", &load_failed(&missing_b));
        record_failure(Path::new("/p/a.test.ts"), "./x", &load_failed(&present));
        record_failure(
            Path::new("/p/a.test.ts"),
            "./x",
            &ResolveError::Ignored(missing_a.clone()),
        );
        let gaps = take_resolution_gaps();
        assert_eq!(gaps.tsconfig.get(&TsconfigGap::Absent(missing_a)), Some(&1));
        assert_eq!(gaps.tsconfig.get(&TsconfigGap::Absent(missing_b)), Some(&1));
        assert_eq!(
            gaps.tsconfig.get(&TsconfigGap::Unreadable(present)),
            Some(&1)
        );
    }

    /// A `tsconfig.json` `paths` alias and the equivalent relative import must resolve to the same
    /// file -- the case lexical anchoring could never handle, since it never looked at
    /// `tsconfig.json` at all.
    #[test]
    fn tsconfig_paths_alias_resolves_like_the_equivalent_relative_import() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("tsconfig.json"),
            r#"{
                "compilerOptions": {
                    "baseUrl": ".",
                    "paths": { "@app/*": ["src/*"] }
                }
            }"#,
        )
        .unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src").join("target.ts"),
            "export const x = 1;",
        )
        .unwrap();
        let importer = dir.path().join("src").join("importer.ts");
        fs::write(&importer, "// importer").unwrap();

        let via_alias = resolve(&importer, "@app/target");
        let via_relative = resolve(&importer, "./target");

        assert_ne!(
            via_alias,
            Resolved::Unresolved,
            "the tsconfig paths alias must resolve"
        );
        assert_eq!(
            via_alias, via_relative,
            "a tsconfig paths alias and the equivalent relative import must name the same module"
        );
    }

    /// Same input, twice: resolution must be deterministic, including through the memoization
    /// cache (a second call for the same key must not, say, return a stale or differently-cased
    /// path from a cache bug).
    #[test]
    fn resolution_is_deterministic_across_repeated_calls() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("target.ts"), "export const x = 1;").unwrap();
        let importer = dir.path().join("importer.ts");
        fs::write(&importer, "// importer").unwrap();

        let first = resolve(&importer, "./target");
        let second = resolve(&importer, "./target");
        assert_eq!(first, second);
    }

    /// An analysis root with a sibling `outside` holding `target.ts`, and an importer whose
    /// tsconfig `paths` maps `@out/*` there. Returns `(root, importer)`; the guard keeps both alive.
    fn escaping_alias() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let guard = tempfile::tempdir().unwrap();
        let root = guard.path().join("root");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(guard.path().join("outside")).unwrap();
        fs::write(
            guard.path().join("outside/target.ts"),
            "export const x = 1;",
        )
        .unwrap();
        fs::write(
            root.join("tsconfig.json"),
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@out/*": ["../outside/*"] } } }"#,
        )
        .unwrap();
        let importer = root.join("importer.ts");
        fs::write(&importer, "// importer").unwrap();
        (guard, root, importer)
    }

    /// A tsconfig `paths` alias that leaves the analysis root resolves to nothing and is recorded,
    /// while the same alias without a root (the library default) still resolves.
    #[test]
    fn tsconfig_alias_outside_the_root_is_refused_only_when_confined() {
        let (_guard, root, importer) = escaping_alias();

        begin_file(None);
        assert!(matches!(
            resolve(&importer, "@out/target"),
            Resolved::Module(_)
        ));
        assert!(take_refusals().is_empty());

        begin_file(Some(&root));
        assert_eq!(resolve(&importer, "@out/target"), Resolved::Unresolved);
        assert_eq!(resolve(&importer, "@out/target"), Resolved::Unresolved);
        let refusals = take_refusals();
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(refusals[0].contains("`@out/target`") && refusals[0].contains("outside"));
        assert!(take_refusals().is_empty(), "taking drains the list");
    }

    /// A missing package looked up inside [`quietly`] is no resolution gap; the same lookup outside
    /// it, not answered from a memo, is.
    #[test]
    fn a_quiet_failure_records_no_gap() {
        let _gaps = GAPS_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let importer = dir.path().join("importer.ts");
        fs::write(&importer, "// importer").unwrap();
        begin_file(None);
        let missing = "vitest-dedup-quiet-missing-package";
        assert_eq!(
            quietly(|| resolve(&importer, missing)),
            Resolved::Unresolved
        );
        let recorded = |gaps: &ResolutionGaps| gaps.packages_not_found.contains_key(dir.path());
        assert!(!recorded(&take_resolution_gaps()));
        assert_eq!(resolve(&importer, missing), Resolved::Unresolved);
        assert!(recorded(&take_resolution_gaps()));
    }

    /// A refusal inside [`quietly`] is not recorded and not memoized, so a later lookup of the
    /// same specifier that needs the module still records it.
    #[test]
    fn a_quiet_refusal_is_neither_recorded_nor_memoized() {
        let (_guard, root, importer) = escaping_alias();
        begin_file(Some(&root));
        assert_eq!(
            quietly(|| resolve(&importer, "@out/target")),
            Resolved::Unresolved
        );
        assert!(take_refusals().is_empty());
        assert_eq!(resolve(&importer, "@out/target"), Resolved::Unresolved);
        assert_eq!(take_refusals().len(), 1);
    }

    /// The analyzer canonicalizes its root, which on Windows yields a verbatim `\\?\` path, and
    /// hands the resolver files under it. Measured on Windows CI, the resolver does not apply a
    /// tsconfig `paths` alias from such an importer at all (the alias stays unresolved even without
    /// confinement), so there the check is limited to that fact; elsewhere the alias must resolve
    /// and be refused exactly as from an ordinary path.
    #[test]
    fn tsconfig_alias_is_judged_from_canonical_paths() {
        let (_guard, root, importer) = escaping_alias();
        let root = root.canonicalize().unwrap();
        let importer = importer.canonicalize().unwrap();

        begin_file(None);
        let unconfined = resolve(&importer, "@out/target");
        assert_eq!(
            matches!(unconfined, Resolved::Module(_)),
            !cfg!(windows),
            "an alias from a canonical importer: {unconfined:?}"
        );

        // Either way nothing outside is read; the refusal is recorded only where the alias resolved.
        begin_file(Some(&root));
        let confined = resolve(&importer, "@out/target");
        assert_eq!(
            (confined, take_refusals().len()),
            (Resolved::Unresolved, usize::from(!cfg!(windows)))
        );
    }

    /// A relative import that leaves the root is not tsconfig-derived and keeps resolving, as does
    /// an alias that stays inside.
    #[test]
    fn relative_import_outside_and_alias_inside_the_root_still_resolve() {
        let (guard, root, importer) = escaping_alias();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/inner.ts"), "export const y = 1;").unwrap();
        fs::write(
            root.join("tsconfig.json"),
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@in/*": ["src/*"], "@out/*": ["../outside/*"] } } }"#,
        )
        .unwrap();

        begin_file(Some(&root));
        assert!(matches!(
            resolve(&importer, "../outside/target"),
            Resolved::Module(_)
        ));
        assert!(matches!(
            resolve(&importer, "@in/inner"),
            Resolved::Module(_)
        ));
        assert!(take_refusals().is_empty());
        drop(guard);
    }

    /// A symlinked workspace package that resolves outside the root without any tsconfig help is
    /// kept; a tsconfig alias to the same file is not. Skipped where symlinks cannot be created.
    #[test]
    fn symlinked_package_outside_the_root_is_not_tsconfig_derived() {
        let (guard, root, importer) = escaping_alias();
        let package = guard.path().join("outside/pkg");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("index.ts"), "export const z = 1;").unwrap();
        fs::write(
            package.join("package.json"),
            r#"{ "name": "ws-pkg", "main": "index.ts" }"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("node_modules")).unwrap();
        crate::confine::with_symlink(&package, &root.join("node_modules/ws-pkg"), || {
            fs::write(
                root.join("tsconfig.json"),
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@pkg": ["../outside/pkg/index.ts"] } } }"#,
            )
            .unwrap();

            begin_file(Some(&root));
            assert!(matches!(resolve(&importer, "ws-pkg"), Resolved::Module(_)));
            assert!(take_refusals().is_empty());
            assert_eq!(resolve(&importer, "@pkg"), Resolved::Unresolved);
            assert_eq!(take_refusals().len(), 1);
        });
    }
}
