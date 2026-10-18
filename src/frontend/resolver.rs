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

use oxc_resolver::{ResolveOptions, Resolver, TsconfigDiscovery};
use oxc_span::VALID_EXTENSIONS;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

thread_local! {
    /// One resolver per worker, built lazily and reused until that extraction completes.
    /// `TsconfigDiscovery::Auto` means each call finds *its own* nearest `tsconfig.json` by
    /// walking up from the importing file -- the right default for a monorepo with one
    /// `tsconfig.json` per package (two of the pinned corpora are), and equally correct for a single
    /// project with one config at the root. There is no CLI-level tsconfig override to plumb
    /// through here, so `Auto` is not a placeholder for a future flag -- it is the whole policy.
    static RESOLVER: Resolver = build_resolver();

    /// Memoizes [`resolve`] for the lifetime of this thread. See the module doc's cost section for
    /// why this is the mechanism that keeps resolution from being paid for on every test body
    /// rather than once per specifier actually written in a file.
    static CACHE: RefCell<HashMap<(PathBuf, String), Resolved>> = RefCell::new(HashMap::new());
}

/// Builds the worker resolver with parser-supported extensions, `module`/`import` conditions,
/// `module`/`main` entry fields and automatic tsconfig discovery. Other options retain their
/// resolver defaults.
fn build_resolver() -> Resolver {
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
        tsconfig: Some(TsconfigDiscovery::Auto),
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
    let resolved =
        RESOLVER.with(
            |resolver| match resolver.resolve_file(importing_file, specifier) {
                Ok(resolution) => Resolved::Module(resolution.into_path_buf()),
                Err(_) => Resolved::Unresolved,
            },
        );
    CACHE.with(|cache| {
        cache.borrow_mut().insert(key, resolved.clone());
    });
    resolved
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
}
