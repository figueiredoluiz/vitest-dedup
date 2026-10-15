//! Imported-helper body resolution: which declaration an imported call site names, and the
//! guards under which that declaration's body may participate in the compared fingerprint.
//!
//! This is the `Helper-body resolution (M3)` backlog row's mechanism. Before it, a call like
//! `checkActive('dashboard')` rendered into `alpha`/`structural` as a call to an opaque
//! `free:import:resolved:<path>:checkActive` identity -- *which export it names*, never *what it
//! does*. Two tests differing only by calling equivalent helpers (different modules, or different
//! exports of one module) could not be recognized as duplicates, because identity is a name and
//! the bodies were never compared. [`fingerprint::render_inlined_helper_body`] renders the
//! helper's own statements in place of the call (parameters substituted with the call site's
//! serialized arguments); this module answers the two questions that must precede that rendering:
//! which file and declaration the import actually names, and whether the parameter binding is
//! exact enough to substitute at all.
//!
//! ## Where the division of labor sits
//!
//! `resolver.rs` answers "what file does this specifier name" and is reused unchanged here --
//! every walk below starts from a resolved path, never re-derives one lexically. What this module
//! adds on top is *export* resolution: finding the exported declaration inside that file,
//! following named re-export chains (`export { run } from './runner'`, the shape whose absence
//! from a single-file extractor is recorded in `extract.rs`'s S11 notes), and refusing every
//! shape whose runtime meaning a body rendering would misrepresent.
//!
//! ## The guards, and what each one is protecting
//!
//! Every guard is a refusal, and every refusal lands on the call's ordinary identity rendering
//! -- an under-match (the pair stays unrecognized), never a guess:
//!
//! - **The framework itself** (`specifier == "vitest"`, the same text the registry's own
//!   framework check keys on). Inlining `expect`'s implementation would replace a stable
//!   identity with the text of whatever build happens to be installed, and a test calling
//!   `expect` does not "contain" the framework's assertions. The specifier check (rather than
//!   a path check alone) survives an in-repo `vitest` stub aliased through tsconfig `paths`.
//! - **Installed packages** (any resolved path with a `node_modules` component, checked at
//!   [`with_parsed_file`]'s single read choke point so re-export hops cannot dodge it). The
//!   same build-artifact reasoning as the framework, plus the cost: a package's dist file is
//!   the largest input this walk could ever parse, and it would be parsed per call site.
//!   Workspace-linked first-party packages still inline -- `resolver.rs` follows symlinks, so
//!   they resolve to their real path outside `node_modules`.
//! - **Parameters that are written** (`x = 5`, `x += 1`, `x++`, a destructuring write): the
//!   write renders as a canonical local while later reads still render the original argument,
//!   which merged two helpers returning different values (review-found; the pin is in
//!   `fingerprint.rs`'s tests).
//! - **Resolution must succeed.** An unresolved import keeps its `unresolved:` identity; two
//!   unverifiable imports must not compare as if their bodies were read.
//! - **Every parameter a plain binding identifier, no initializer (a default), no rest; call
//!   site arity exact, no spread.** Parameter-to-argument substitution is only sound when each
//!   parameter binds exactly the argument in its position: a default value is a body expression
//!   whose evaluation depends on the *caller's* arity, a rest or destructuring parameter has no
//!   single argument to substitute, and a spread at the call site (`f(...args)`) makes every
//!   position unknown -- `extract.rs`'s `bind_helper_parameters` refuses wholesale on the same
//!   terms, for the same reason.
//! - **Declaration shapes: `export function`, `export const name = <arrow or function
//!   expression>`, `export { local as name }` to one of those in the same file, named re-export
//!   chains, and `export default <function | arrow>`.** A `let`/`var` export can be reassigned
//!   by the helper module between calls, so its initializer at declaration time proves nothing
//!   about the call. Anything else (a class, an object, a call result, `export *`, CommonJS
//!   `module.exports`) is not a body this module can name.
//!
//! ## Cost
//!
//! Each inline decision parses the helper file, and the `alpha` rendering parses it again
//! (`structural` never inlines -- see `fingerprint.rs`'s `try_inline_imported_helper` for why).
//! The test file's own imports are parsed once per file (`fingerprint.rs`'s
//! `with_file_import_origins`). A helper file is read and admission-checked once per version
//! ([`admitted_source`]): the admitted text is kept process-wide, revalidated on every lookup
//! by length and modification time so a rewritten helper is read again, and capped in total.
//! Parsed trees are not kept. The worst input -- an installed package's dist file -- is refused
//! before it is read (see the guards).
//!
//! ## `vi.mock` interplay
//!
//! Inlining reads the helper's *real* body, while a test that mocks the helper's module runs the
//! factory's replacement at runtime. No refusal is needed for that gap, because every
//! mock-differing pair is separated before `alpha` is ever consulted: a `vi.mock` this scan
//! could not fully read folds into `Context::unresolved` (the test leaves comparison entirely),
//! and every readable mock participates in `Context::mocked_modules` keyed by the same resolved
//! module identity and compared with its factory's alpha -- so two tests mocking differently,
//! or one mocking and one not, never share a context. Two tests mocking *identically* match
//! exactly as they did before this module existed, which is correct: they run the same code.
//!
//! ## What this module does not do
//!
//! Imported declaration extraction uses this export contract in its own parsed frame in
//! `extract/imported.rs`. That path has narrower callable/context admission rules than body
//! fingerprint inlining and never follows installed-package bodies.

use super::fingerprint::{
    import_origins_from_program, module_export_name, render_inlined_helper_body, FunctionLike,
    ImportOrigin, Mode,
};
use super::resolver::{self, Resolved};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrowFunctionExpression, BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression,
    Function, FunctionType, Program, Statement, VariableDeclaration, VariableDeclarationKind,
};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// How many named re-export hops a locate walk will follow before refusing.
///
/// Resolution hops, not body-inlining depth (that is bounded to one level by
/// [`fingerprint::render_inlined_helper_body`] itself). Real chains are one or two long -- a
/// barrel re-exporting a runner that re-exports a package's export; eight is the same bound
/// `extract.rs`'s helper expansion uses, and past it the refusal is the ordinary under-match.
const MAX_REEXPORT_HOPS: usize = 8;

/// Whether a call site with `argument_count` arguments (and spread arguments, when
/// `has_spread`) would have its callee's helper body inlined.
///
/// The pre-check `try_inline_imported_helper` runs before spending serialization on arguments,
/// and the shape question behind extract.rs's context drop (which consumes the alpha pass's own
/// report of what it rendered, not this function -- see `fingerprint`'s construction of it).
/// Must agree with [`render_inline`] exactly, which is why both share the same locate walk as
/// the single authority on what the import names and whether its parameters bind.
pub(super) fn would_inline(origin: &ImportOrigin, argument_count: usize, has_spread: bool) -> bool {
    if origin.specifier == "vitest" || origin.resolved.is_none() || has_spread {
        return false;
    }
    let Some(path) = origin.resolved.as_ref() else {
        return false;
    };
    locate(path, &origin.exported).is_some_and(|parameters| parameters == argument_count)
}

/// Renders the helper `origin` names, with `argument_fragments` (the call site's arguments,
/// already serialized by the importing file's pass) substituted for the helper's parameters.
/// `None` is a refusal; the caller renders the call exactly as it did before this module existed.
pub(super) fn render_inline(
    origin: &ImportOrigin,
    argument_fragments: &[String],
    mode: Mode,
    depth: u32,
) -> Option<String> {
    let path = origin.resolved.as_ref()?;
    let mut visited = HashSet::new();
    render_from(
        path,
        &origin.exported,
        argument_fragments,
        mode,
        depth,
        &mut visited,
        0,
    )
}

/// The shape-only locate: the parameter count behind a would-be inline, or `None` when the
/// export cannot be located or does not qualify. Split out so [`would_inline`] decides without
/// paying for a `Semantic` build, while walking the *same* export logic [`render_from`] does.
fn locate(path: &Path, export: &str) -> Option<usize> {
    let mut visited = HashSet::new();
    let mut path = path.to_path_buf();
    let mut export = export.to_owned();
    for _ in 0..=MAX_REEXPORT_HOPS {
        if !visited.insert((path.clone(), export.clone())) {
            return None;
        }
        // The program borrows this frame's arena; everything found in it is consumed before the
        // loop moves to the next hop's file.
        let found = with_parsed_file(&path, |program, _| match find_export(program, &export)? {
            Export::Local(callable) => bindable_parameters(&callable).map(CallableOrHop::Callable),
            Export::Reexport { specifier, local } => {
                resolve_hop(&path, &specifier).map(|next| CallableOrHop::Hop((next, local)))
            }
        })?;
        match found {
            CallableOrHop::Callable(parameters) => return Some(parameters),
            CallableOrHop::Hop((next_path, next_export)) => {
                path = next_path;
                export = next_export;
            }
        }
    }
    None
}

/// The C10 export-resolution contract: which first-party file an imported binding's export
/// chain terminates in, and -- when it does not terminate -- exactly why it refused.
///
/// [`locate`] and [`render_from`] collapse every refusal into `None` because their failure mode
/// is the ordinary under-match: the pair stays unrecognized, nothing is reported. C10's
/// declaration extraction (backlog plan, point 3) cannot do that -- a file whose runner import
/// fails to resolve must read `incomplete` with a cause, never as a smaller test count that
/// happens to be zero -- so this module names the reasons. It resolves through the same pieces
/// the inline walks use ([`find_export`], [`resolve_hop`], [`is_dependency`],
/// [`MAX_REEXPORT_HOPS`]) and adds no second opinion of its own; the inline walks' behavior is
/// unchanged, and the pins in this module's tests include the exact hop boundary so the three
/// loops cannot silently disagree about what a hop means.
///
/// `#[allow(dead_code)]` because the extraction consumer arrives in a later PR of the same plan;
/// the contract is pinned first so that consumer builds against tested refusal semantics rather
/// than inventing them inline.
pub(in crate::frontend) mod export_contract {
    use super::{
        find_export, is_dependency, resolve_hop, with_parsed_file, Export, MAX_REEXPORT_HOPS,
    };
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    /// The terminal an export-resolution walk reached, or the explicit reason it refused.
    ///
    /// The variants' order is the walk's per-hop precedence: cycle, then package, then
    /// read/parse, then the export lookup.
    #[derive(Debug, PartialEq, Eq)]
    pub(in crate::frontend) enum ExportRefusal {
        /// The export chain revisited a `(path, export)` pair it had already resolved through.
        /// An *export* cycle is chain-level and distinct from a recursive helper call (`f`
        /// calling `f` inside one body), which is body content this walk never opens.
        ReexportCycle,
        /// A file in the chain sits inside an installed package, refused before it is read --
        /// the same boundary `with_parsed_file` enforces for every walk. C10 retains it; the
        /// backlog's C11 row owns any decision to widen it.
        InstalledPackage,
        /// The file could not be read, or did not parse. One reason for both: no consumer can
        /// act differently on the difference.
        UnreadableOrUnparseable,
        /// A re-export hop's specifier did not resolve against the re-exporting file.
        UnresolvedSpecifier,
        /// `find_export` found no admitted export of this name in this file. A present export
        /// of an unadmitted shape (`export const run = 5`, a class, `export *`) folds into this
        /// reason: `find_export` cannot distinguish it, and this contract adds no second
        /// opinion.
        MissingExport,
        /// The chain was still a re-export after [`MAX_REEXPORT_HOPS`] hops. Checked before the
        /// next specifier is resolved, so a chain that is both too long and dangling reports the
        /// hop limit, not the dangling specifier.
        HopLimit,
    }

    /// The first-party file that *declares* the export, paired with the name under which that
    /// file's own exports admit it (a chain that renames mid-hop ends under the declaring
    /// file's local name). Owned, because the declaring file's arena dies with the walk: a
    /// consumer re-parses `path` and looks `export` up again inside its own frame.
    ///
    /// Self-contained by construction -- `resolve_export(&terminal.path, &terminal.export)`
    /// reaches the same terminal with no hops -- so a consumer that re-resolves from a terminal
    /// cannot drift from the walk that produced it.
    #[derive(Debug, PartialEq, Eq)]
    pub(in crate::frontend) struct ResolvedExport {
        pub(in crate::frontend) path: PathBuf,
        pub(in crate::frontend) export: String,
    }

    /// Provenance of every module traversed, used to reject mocked/setup-bearing barrels.
    pub(in crate::frontend) struct ResolvedExportChain {
        pub(in crate::frontend) terminal: ResolvedExport,
        pub(in crate::frontend) modules: Vec<PathBuf>,
    }

    /// One step's outcome, owned: `with_parsed_file`'s closure cannot return arena-borrowed
    /// data (its `T` is fixed outside the higher-ranked lifetime), so the callable itself stays
    /// in the walk's frame and only the hop decision crosses.
    enum Step {
        Terminal,
        Hop { specifier: String, local: String },
    }

    /// Resolves `export` starting from `start` to an owned terminal, or the explicit refusal.
    ///
    /// A separate walk, deliberately, not a refactor of the existing two: [`super::render_from`]
    /// renders inside the terminal file's parse frame, and [`super::locate`] counts parameters
    /// in the same parse that finds the export, so both would pay a second parse per call site
    /// to consume this contract instead of walking themselves.
    #[cfg(test)]
    pub(in crate::frontend) fn resolve_export(
        start: &Path,
        export: &str,
    ) -> Result<ResolvedExport, ExportRefusal> {
        resolve_export_chain(start, export).map(|chain| chain.terminal)
    }

    /// Resolves a bounded named-export chain, retaining every module for setup and mock checks.
    pub(in crate::frontend) fn resolve_export_chain(
        start: &Path,
        export: &str,
    ) -> Result<ResolvedExportChain, ExportRefusal> {
        let mut path = start.to_path_buf();
        let mut export = export.to_owned();
        let mut visited = HashSet::new();
        let mut modules = Vec::new();
        for hop in 0..=MAX_REEXPORT_HOPS {
            // The visited identity is the canonical spelling: `start` arrives verbatim while
            // `resolve_hop` returns symlink-followed paths (macOS /var vs /private/var, Windows
            // 8.3 short names), so a cycle returning to the start file would otherwise be
            // missed for one lap -- and a cycle long enough that the extra lap crosses the hop
            // budget would surface as the hop limit instead of the cycle reason (review-found;
            // pinned by `a_misspelled_start_still_names_a_cycle_at_the_hop_boundary`).
            let identity = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !visited.insert((identity, export.clone())) {
                return Err(ExportRefusal::ReexportCycle);
            }
            if is_dependency(&path) {
                return Err(ExportRefusal::InstalledPackage);
            }
            // `Some(None)` keeps a missing export distinct from a file that cannot be read or
            // parsed; `with_parsed_file` flattens both to `None` otherwise.
            modules.push(path.clone());
            let step = with_parsed_file(&path, |program, _| {
                Some(find_export(program, &export).map(|found| match found {
                    Export::Local(_) => Step::Terminal,
                    Export::Reexport { specifier, local } => Step::Hop { specifier, local },
                }))
            })
            .ok_or(ExportRefusal::UnreadableOrUnparseable)?;
            match step {
                Some(Step::Terminal) => {
                    return Ok(ResolvedExportChain {
                        terminal: ResolvedExport { path, export },
                        modules,
                    })
                }
                Some(Step::Hop { specifier, local }) => {
                    if hop == MAX_REEXPORT_HOPS {
                        return Err(ExportRefusal::HopLimit);
                    }
                    path =
                        resolve_hop(&path, &specifier).ok_or(ExportRefusal::UnresolvedSpecifier)?;
                    export = local;
                }
                None => return Err(ExportRefusal::MissingExport),
            }
        }
        Err(ExportRefusal::HopLimit)
    }
}

/// [`render_inline`]'s walk: locate, guard, then hand the callable to the renderer with the
/// helper file's own semantic context.
fn render_from(
    path: &Path,
    export: &str,
    argument_fragments: &[String],
    mode: Mode,
    depth: u32,
    visited: &mut HashSet<(PathBuf, String)>,
    hops: usize,
) -> Option<String> {
    if hops > MAX_REEXPORT_HOPS || !visited.insert((path.to_path_buf(), export.to_owned())) {
        return None;
    }
    with_parsed_file(path, |program, source| {
        match find_export(program, export)? {
            Export::Local(callable) => {
                let parameters = bindable_parameters(&callable)?;
                if parameters != argument_fragments.len() {
                    return None;
                }
                // The helper's own parse, made with its real source type: a TSX re-parse would
                // reject valid `.ts` syntax and silently empty the map.
                let imports = import_origins_from_program(program, path);
                let semantic_return = SemanticBuilder::new().build(program);
                if !semantic_return.diagnostics.is_empty() {
                    return None;
                }
                let semantic = semantic_return.semantic;
                // Substituting a read with the call-site argument is sound only while the
                // parameter still *holds* that argument. A written parameter -- `x = 5`,
                // `x += 1`, `x++`, a destructuring write -- does not: the write renders as a
                // canonical local while later reads still render the original argument, so
                // `f(x) { x = 5; return x; }` called `f(1)` and `g(x) { x = 5; return 1; }`
                // called `g(1)` would render identically while returning 5 and 1 -- a
                // review-found false merge at Error severity. `locate` cannot see this (it
                // builds no semantic), so this refusal can fire after `would_inline` said
                // yes; that split is already handled -- the call falls back to its identity
                // rendering and the accounting records the identity.
                let parameter_written =
                    callable
                        .function_like()
                        .params()
                        .items
                        .iter()
                        .any(|parameter| {
                            let BindingPattern::BindingIdentifier(binding) = &parameter.pattern
                            else {
                                return true;
                            };
                            binding
                                .symbol_id
                                .get()
                                .is_none_or(|symbol| semantic.scoping().symbol_is_mutated(symbol))
                        });
                if parameter_written {
                    return None;
                }
                Some(render_inlined_helper_body(
                    &semantic,
                    source,
                    &imports,
                    path,
                    &callable.function_like(),
                    argument_fragments,
                    mode,
                    depth,
                ))
            }
            Export::Reexport { specifier, local } => {
                let next = resolve_hop(path, &specifier)?;
                render_from(
                    &next,
                    &local,
                    argument_fragments,
                    mode,
                    depth,
                    visited,
                    hops + 1,
                )
            }
        }
    })
}

/// Resolves one re-export hop's `specifier` against the file that wrote it.
fn resolve_hop(reexporting_file: &Path, specifier: &str) -> Option<PathBuf> {
    match resolver::resolve(reexporting_file, specifier) {
        Resolved::Module(resolved_path) => Some(resolved_path),
        Resolved::Unresolved => None,
    }
}

/// A file inside an installed package: its body is a build artifact, not project code. The
/// component scan (not a substring test) is exact -- a first-party directory merely *named*
/// like one segment would have to be exactly `node_modules` to be refused.
fn is_dependency(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "node_modules")
}

/// Parses `path`'s admitted source (read once per file version, see [`admitted_source`]), then
/// runs `visit` on the program and its source while the arena that
/// owns every AST node is still alive. This closure shape *is* the lifetime discipline: a
/// `Program` cannot outlive its `Allocator`, so locate-and-use happens inside one call frame and
/// no `Program` reference ever escapes. `None` from the read, the parse, or `visit` itself is a
/// refusal.
pub(super) fn with_parsed_file<T>(
    path: &Path,
    visit: impl for<'a> FnOnce(&'a Program<'a>, &'a str) -> Option<T>,
) -> Option<T> {
    // The dependency refusal lives at this one choke point -- the only place a helper file is
    // so much as read -- so no walk (initial resolution or any re-export hop) can parse an
    // installed package: a package's body is a build artifact, its "identity" is the published
    // package, and reading it would pay dist-file parse cost at every call site (review-found
    // cost class: `render`/`waitFor`/any `lodash-es` export resolves here). First-party code
    // survives this even in monorepos, because `resolver.rs` follows symlinks: a
    // workspace-linked package resolves to its real path outside `node_modules`.
    if is_dependency(path) {
        return None;
    }
    let source = admitted_source(path)?;
    let allocator = Allocator::default();
    // Use the resolved helper extension, so .ts casts and .tsx JSX keep their own syntax.
    let parsed = Parser::new(&allocator, &source, SourceType::from_path(path).ok()?).parse();
    if parsed.fatal_error || !parsed.diagnostics.is_empty() {
        return None;
    }
    visit(&parsed.program, &source)
}

/// Total helper source text [`admitted_source`] keeps. Past it, files are read uncached, exactly
/// as before the cache existed.
const MAX_CACHED_HELPER_BYTES: usize = 64 * 1024 * 1024;

/// How recent a modification time must be before [`admitted_source`] stops trusting it. On a
/// filesystem with coarse timestamps (HFS+ 1 s, FAT 2 s, some network mounts) a same-length
/// rewrite inside one tick keeps the old stamp, so a file modified this recently is read afresh
/// every time, the same way Git re-checks racily clean index entries.
const RACY_MODIFICATION_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

/// The metadata a cached helper entry is valid for. A file whose length or modification time
/// differs is read again, so a rewritten helper is not served from an old copy. A same-length
/// rewrite that also restores the old modification time is not detected; that needs a content
/// comparison, which would cost the read this cache exists to avoid.
#[derive(Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: std::time::SystemTime,
}

/// Admitted helper sources (or refusals) keyed by path, with the bytes they hold.
#[derive(Default)]
struct HelperSources {
    entries: std::collections::HashMap<PathBuf, (FileStamp, Option<std::sync::Arc<str>>)>,
    bytes: usize,
}

/// Process-wide, so every test file calling the same helper shares one read; a `Mutex` keeps it
/// sound once extraction runs on several threads.
static HELPER_SOURCES: std::sync::Mutex<Option<HelperSources>> = std::sync::Mutex::new(None);

#[cfg(test)]
thread_local! {
    /// Test-only count of helper files this thread actually read.
    static HELPER_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Returns `path`'s source when it is admitted for parsing (readable, within the size bound and
/// the raw syntax budget), or `None` for a refusal. Each distinct file version is read and
/// checked once; later calls reuse the answer while the file's length and modification time are
/// unchanged. A file without a readable modification time, or modified within
/// [`RACY_MODIFICATION_WINDOW`], is never cached.
fn admitted_source(path: &Path) -> Option<std::sync::Arc<str>> {
    let metadata = std::fs::metadata(path).ok()?;
    let settled = |modified: &std::time::SystemTime| {
        modified
            .elapsed()
            .is_ok_and(|age| age > RACY_MODIFICATION_WINDOW)
    };
    let stamp = metadata
        .modified()
        .ok()
        .filter(settled)
        .map(|modified| FileStamp {
            len: metadata.len(),
            modified,
        });
    let lock = || {
        HELPER_SOURCES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    if let Some(stamp) = &stamp {
        let cached = lock().as_ref().and_then(|sources| {
            sources
                .entries
                .get(path)
                .filter(|(cached, _)| cached == stamp)
                .map(|(_, source)| source.clone())
        });
        if let Some(source) = cached {
            return source;
        }
    }
    // A read error is not a property of the file version (permissions, transient I/O), so it is
    // refused without being remembered and the next lookup tries again.
    let source = read_admitted(path).ok()?;
    if let Some(stamp) = stamp {
        let mut guard = lock();
        let sources = guard.get_or_insert_with(HelperSources::default);
        sources.remember(path, stamp, source.clone(), MAX_CACHED_HELPER_BYTES);
    }
    source
}

impl HelperSources {
    /// Records `source` for `path` at `stamp` when it fits under `cap`. Any older version of the
    /// path is dropped first: it can never be served again, so keeping it would only hold bytes.
    fn remember(
        &mut self,
        path: &Path,
        stamp: FileStamp,
        source: Option<std::sync::Arc<str>>,
        cap: usize,
    ) {
        if let Some((_, old)) = self.entries.remove(path) {
            self.bytes -= old.map_or(0, |old| old.len());
        }
        let held = source.as_ref().map_or(0, |source| source.len());
        if self.bytes + held <= cap {
            self.bytes += held;
            self.entries.insert(path.to_path_buf(), (stamp, source));
        }
    }
}

/// Reads `path` and applies the admission checks every helper parse needs: `Ok(Some)` admitted,
/// `Ok(None)` a refusal decided by the file's content (over the size bound, over the raw syntax
/// budget), `Err` a read that failed and says nothing about the content.
fn read_admitted(path: &Path) -> std::io::Result<Option<std::sync::Arc<str>>> {
    #[cfg(test)]
    HELPER_READS.with(|reads| reads.set(reads.get() + 1));
    let Some(source) =
        crate::discovery::read_bounded(path, crate::discovery::MAX_SOURCE_FILE_BYTES)?
    else {
        return Ok(None);
    };
    if super::extract::parse_refusal(&source).is_some() {
        return Ok(None);
    }
    Ok(Some(std::sync::Arc::from(source)))
}

/// What [`find_export`] found: either the callable declaration in this file, or one more named
/// re-export hop to follow.
pub(super) enum Export<'a> {
    Local(Callable<'a>),
    Reexport {
        /// The specifier the re-export pulls from, resolved against the re-exporting file.
        specifier: String,
        /// The name to look up on the next hop: the *local* side of `export { local as X }`.
        local: String,
    },
}

/// The intermediate [`locate`] builds so its loop can carry a hop without holding arena borrows
/// across iterations.
enum CallableOrHop {
    Callable(usize),
    Hop((PathBuf, String)),
}

/// An admitted declaration shape, normalized: a `function` declaration/expression, or an arrow
/// (the initializer of an `export const`, or a default export).
pub(super) enum Callable<'a> {
    Function(&'a Function<'a>),
    Arrow(&'a ArrowFunctionExpression<'a>),
}

impl Callable<'_> {
    /// The callable shape the renderer takes; the guard set has already admitted both variants
    /// by the time this is reached.
    fn function_like(&self) -> FunctionLike<'_, '_> {
        match self {
            Callable::Function(function) => FunctionLike::Function(function),
            Callable::Arrow(arrow) => FunctionLike::Arrow(arrow),
        }
    }
}

/// Finds what `export` names in `program`: the exported declaration itself, or the re-export hop
/// that leads toward it. Refuses (`None`) every shape the guards do not admit; `export *` is
/// among them -- a star re-export's name resolution is the linker's job, not a question this
/// walk can answer from one file.
pub(super) fn find_export<'p>(program: &'p Program<'p>, export: &str) -> Option<Export<'p>> {
    for statement in &program.body {
        match statement {
            Statement::ExportDeclaration(it) => match &it.declaration {
                Declaration::FunctionDeclaration(function) => {
                    if function
                        .id
                        .as_ref()
                        .is_some_and(|id| id.name.as_str() == export)
                    {
                        return Some(Export::Local(Callable::Function(function)));
                    }
                }
                Declaration::VariableDeclaration(variables) => {
                    if let Some(callable) = exported_const_callable(variables, export) {
                        return Some(Export::Local(callable));
                    }
                }
                _ => {}
            },
            Statement::ExportNamedDeclaration(it) => {
                if matches!(it.export_kind, oxc_ast::ast::ImportOrExportKind::Type) {
                    continue;
                }
                // `export { local as exported }` -- the alias's target must be one of the
                // admitted shapes in this same file, found by its local name.
                for specifier in &it.specifiers {
                    if matches!(
                        specifier.export_kind,
                        oxc_ast::ast::ImportOrExportKind::Type
                    ) || module_export_name(&specifier.exported) != export
                    {
                        continue;
                    }
                    let local = module_export_name(&specifier.local);
                    return module_level_callable(program, &local).map(Export::Local);
                }
            }
            Statement::ExportFromDeclaration(it) => {
                if matches!(it.export_kind, oxc_ast::ast::ImportOrExportKind::Type) {
                    continue;
                }
                for specifier in &it.specifiers {
                    if matches!(
                        specifier.export_kind,
                        oxc_ast::ast::ImportOrExportKind::Type
                    ) || module_export_name(&specifier.exported) != export
                    {
                        continue;
                    }
                    return Some(Export::Reexport {
                        specifier: it.source.value.as_str().to_owned(),
                        local: module_export_name(&specifier.local),
                    });
                }
            }
            Statement::ExportDefaultDeclaration(it) => {
                if export != "default" {
                    continue;
                }
                return match &it.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                        Some(Export::Local(Callable::Function(function)))
                    }
                    ExportDefaultDeclarationKind::ArrowFunctionExpression(arrow) => {
                        Some(Export::Local(Callable::Arrow(arrow)))
                    }
                    ExportDefaultDeclarationKind::FunctionExpression(function) => {
                        Some(Export::Local(Callable::Function(function)))
                    }
                    _ => None,
                };
            }
            _ => {}
        }
    }
    None
}

/// `export const name = <arrow or function expression>`: the one variable shape admitted,
/// because a `const` initializer cannot be reassigned. `let`/`var` exports and non-callable
/// initializers are refused by the shape of this function itself.
fn exported_const_callable<'p>(
    variables: &'p VariableDeclaration<'p>,
    export: &str,
) -> Option<Callable<'p>> {
    if variables.kind != VariableDeclarationKind::Const {
        return None;
    }
    let declarator = variables.declarations.first()?;
    let BindingPattern::BindingIdentifier(binding) = &declarator.id else {
        return None;
    };
    if binding.name.as_str() != export {
        return None;
    }
    match declarator.init.as_ref()? {
        Expression::FunctionExpression(function) => Some(Callable::Function(function)),
        Expression::ArrowFunctionExpression(arrow) => Some(Callable::Arrow(arrow)),
        _ => None,
    }
}

/// A module-level `function name(...)` or `const name = <function or arrow>` declaration, for
/// `export { name as exported }`: the alias's target must be one of the admitted shapes in this
/// same file.
fn module_level_callable<'p>(program: &'p Program<'p>, name: &str) -> Option<Callable<'p>> {
    for statement in &program.body {
        match statement {
            Statement::FunctionDeclaration(function) => {
                if function
                    .id
                    .as_ref()
                    .is_some_and(|id| id.name.as_str() == name)
                {
                    return Some(Callable::Function(function));
                }
            }
            Statement::VariableDeclaration(variables) => {
                if let Some(callable) = exported_const_callable(variables, name) {
                    return Some(callable);
                }
            }
            _ => {}
        }
    }
    None
}

/// The parameter count, when every parameter-list guard passes: plain binding identifiers only,
/// no initializer (a default), no rest, and a body present. Anything else is a refusal -- there
/// is no argument whose rendering could stand in for a destructured, defaulted, or rest
/// parameter without asserting an equivalence nobody verified.
fn bindable_parameters(callable: &Callable<'_>) -> Option<usize> {
    let params = match callable {
        Callable::Function(function) => {
            // `TSDeclareFunction` and an overload signature have no body to render.
            if function.r#type == FunctionType::TSDeclareFunction || function.body.is_none() {
                return None;
            }
            &function.params
        }
        // An arrow's body is a block or an expression; both render, and the declare/empty-body
        // shapes have no arrow equivalent.
        Callable::Arrow(arrow) => &arrow.params,
    };
    if params.rest.is_some() {
        return None;
    }
    for parameter in &params.items {
        if !matches!(parameter.pattern, BindingPattern::BindingIdentifier(_))
            || parameter.initializer.is_some()
        {
            return None;
        }
    }
    Some(params.items.len())
}

#[cfg(test)]
mod tests {
    use super::export_contract::{resolve_export, ExportRefusal};
    use super::*;

    /// Returns how many helper files this thread has read so far.
    fn helper_reads() -> usize {
        HELPER_READS.with(std::cell::Cell::get)
    }

    /// Writes `text` to `path` and sets its modification time to `modified`.
    fn write_at(path: &Path, text: &str, modified: std::time::SystemTime) {
        std::fs::write(path, text).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    /// A modification time old enough for the cache to trust.
    fn settled_time() -> std::time::SystemTime {
        std::time::SystemTime::now() - std::time::Duration::from_secs(3600)
    }

    /// One helper file is read once however many call sites ask about it (P4): every inline
    /// decision used to re-read and re-scan the file, which dominated helper-heavy runs.
    #[test]
    fn a_helper_file_is_read_once_across_repeated_lookups() {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helpers.ts");
        write_at(
            &helper,
            "export function add(a, b) { return a + b; }",
            settled_time(),
        );
        let before = helper_reads();
        for _ in 0..20 {
            assert_eq!(locate(&helper, "add"), Some(2));
        }
        assert_eq!(
            helper_reads() - before,
            1,
            "20 lookups of one unchanged helper"
        );
    }

    /// A helper rewritten in place is read again, even when the rewrite keeps the old
    /// modification time: the length alone tells the versions apart.
    #[test]
    fn a_rewritten_helper_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helpers.ts");
        let modified = settled_time();
        write_at(&helper, "export function add(a) { return a; }", modified);
        assert_eq!(locate(&helper, "add"), Some(1));
        write_at(
            &helper,
            "export function add(a, b) { return a + b; }",
            modified,
        );
        assert_eq!(
            locate(&helper, "add"),
            Some(2),
            "the rewritten helper's arity"
        );
    }

    /// A recently modified file is never cached: on a coarse-timestamp filesystem a same-length
    /// rewrite inside one tick keeps the old stamp, so only a re-read sees the new text.
    #[test]
    fn a_recently_modified_helper_is_read_afresh() {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helpers.ts");
        let tick = std::time::SystemTime::now();
        let two = "export function add(a, b) { return a + b; }";
        let one = "export function add(a) { return a + 1001; }";
        assert_eq!(two.len(), one.len(), "the rewrite keeps the length");
        write_at(&helper, two, tick);
        assert_eq!(locate(&helper, "add"), Some(2));
        write_at(&helper, one, tick);
        assert_eq!(
            locate(&helper, "add"),
            Some(1),
            "same length, same stamp, new text"
        );
    }

    /// A read error is not remembered: a helper that becomes readable again is admitted even
    /// though its length and modification time never changed (`chmod` touches neither).
    #[cfg(unix)]
    #[test]
    fn a_read_error_is_retried_rather_than_cached() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helpers.ts");
        write_at(
            &helper,
            "export function add(a, b) { return a + b; }",
            settled_time(),
        );
        let set_mode = |mode| {
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        set_mode(0o000);
        if std::fs::read(&helper).is_ok() {
            set_mode(0o644);
            return; // running as root: permissions cannot make the file unreadable here
        }
        assert_eq!(
            locate(&helper, "add"),
            None,
            "an unreadable helper is refused"
        );
        set_mode(0o644);
        assert_eq!(
            locate(&helper, "add"),
            Some(2),
            "and admitted once readable again"
        );
    }

    /// The cap bounds held bytes, and a version the cap rejects also evicts the path's older,
    /// unservable version instead of leaving it counted.
    #[test]
    fn the_cache_cap_evicts_a_replaced_version_it_cannot_hold() {
        let stamp = |len| FileStamp {
            len,
            modified: std::time::SystemTime::UNIX_EPOCH,
        };
        let text = |s: &str| Some(std::sync::Arc::<str>::from(s));
        let mut sources = HelperSources::default();
        let a = Path::new("/a.ts");
        sources.remember(a, stamp(6), text("abcdef"), 10);
        assert_eq!(sources.bytes, 6);
        sources.remember(a, stamp(12), text("abcdefghijkl"), 10);
        assert!(!sources.entries.contains_key(a), "the old version is gone");
        assert_eq!(sources.bytes, 0, "and so are its bytes");
        sources.remember(Path::new("/b.ts"), stamp(8), text("abcdefgh"), 10);
        sources.remember(Path::new("/c.ts"), stamp(3), text("abc"), 10);
        assert_eq!(sources.bytes, 8, "a file past the cap is not held");
        sources.remember(Path::new("/d.ts"), stamp(1), None, 10);
        assert!(
            sources.entries.contains_key(Path::new("/d.ts")),
            "refusals cost no bytes"
        );
    }

    /// A refused helper stays refused and is not re-read while unchanged; an installed package
    /// is never read at all.
    #[test]
    fn refusals_are_cached_and_dependencies_are_never_read() {
        let dir = tempfile::tempdir().unwrap();
        let refused = dir.path().join("refused.ts");
        let deep = "(".repeat(9_000);
        write_at(
            &refused,
            &format!("export function f(a) {{ return {deep}a; }}"),
            settled_time(),
        );
        let before = helper_reads();
        assert_eq!(locate(&refused, "f"), None);
        assert_eq!(locate(&refused, "f"), None);
        assert_eq!(
            helper_reads() - before,
            1,
            "a refusal is decided once per file version"
        );

        let package = dir.path().join("node_modules").join("pkg");
        std::fs::create_dir_all(&package).unwrap();
        let dependency = package.join("index.ts");
        std::fs::write(&dependency, "export function g(a) { return a; }").unwrap();
        let before = helper_reads();
        assert_eq!(locate(&dependency, "g"), None);
        assert_eq!(
            helper_reads() - before,
            0,
            "an installed package is never read"
        );
    }

    /// An unparseable helper file refuses the whole inline (under-match), never panics --
    /// the product's degrade-visibly rule.
    #[test]
    fn a_parse_failure_is_a_refusal_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helpers.ts");
        std::fs::write(&helper, "export function broken( {").unwrap();
        assert_eq!(
            locate(&helper, "broken"),
            None,
            "an unparseable helper file must refuse, never panic"
        );
    }

    /// A defaulted parameter's value depends on the caller's arity, so the whole export is
    /// refused rather than partially substituted.
    #[test]
    fn default_parameters_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helpers.ts");
        std::fs::write(
            &helper,
            "export function greet(name = 'x') { return name; }",
        )
        .unwrap();
        assert_eq!(
            locate(&helper, "greet"),
            None,
            "a defaulted parameter has no argument to substitute; the whole export is refused"
        );
    }

    /// A rest parameter binds no single argument; the export is refused wholesale.
    #[test]
    fn rest_parameters_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helpers.ts");
        std::fs::write(&helper, "export function log(...parts) { return parts; }").unwrap();
        assert_eq!(locate(&helper, "log"), None);
    }

    /// A destructured parameter binds no single symbol to substitute; refused wholesale.
    #[test]
    fn destructured_parameters_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helpers.ts");
        std::fs::write(&helper, "export function read({ name }) { return name; }").unwrap();
        assert_eq!(locate(&helper, "read"), None);
    }

    /// A `let` export can be reassigned by its own module between calls, so its initializer
    /// at declaration time proves nothing about any later call.
    #[test]
    fn let_exports_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helpers.ts");
        std::fs::write(
            &helper,
            "export let run = function (name) { return name; };",
        )
        .unwrap();
        assert_eq!(locate(&helper, "run"), None);
    }

    /// `export const name = <arrow>` is an admitted shape; its parameter count is the arity.
    #[test]
    fn const_arrows_are_admitted_and_counted() {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helpers.ts");
        std::fs::write(
            &helper,
            "export const run = (name, greeting) => greeting + name;",
        )
        .unwrap();
        assert_eq!(locate(&helper, "run"), Some(2));
    }

    /// `export default function` is admitted under the exported name `default`.
    #[test]
    fn default_exported_functions_are_admitted() {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helpers.ts");
        std::fs::write(&helper, "export default function (label) { return label; }").unwrap();
        assert_eq!(locate(&helper, "default"), Some(1));
    }

    /// `export { local as exported }` resolves to the local declaration in the same file.
    #[test]
    fn same_file_aliases_resolve_to_the_local_declaration() {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helpers.ts");
        std::fs::write(
            &helper,
            "function check(label) { return label; }\nexport { check as verify };",
        )
        .unwrap();
        assert_eq!(locate(&helper, "verify"), Some(1));
    }

    /// Named re-export chains are followed (local names may change mid-chain), and a cycle
    /// in the chain terminates in a refusal.
    #[test]
    fn reexport_chains_are_followed_and_cycles_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        // A two-hop chain with the name changing on the middle hop: index exports `go`, which is
        // mid's `go`, which is inner's `run`.
        std::fs::write(dir.path().join("index.ts"), "export { go } from './mid';").unwrap();
        std::fs::write(
            dir.path().join("mid.ts"),
            "export { run as go } from './inner';",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("inner.ts"),
            "export function run(label) { return label; }",
        )
        .unwrap();
        assert_eq!(locate(&dir.path().join("index.ts"), "go"), Some(1));

        // A cycle: a re-exports b, b re-exports a. Following must terminate in a refusal.
        let cycle = cycle_dir();
        assert_eq!(locate(&cycle.path().join("a.ts"), "spin"), None);
    }

    /// A resolved path inside `node_modules` is refused before the file is read, both for
    /// a direct import and for a first-party barrel's re-export hop into the package.
    #[test]
    fn an_installed_package_is_refused_before_it_is_read() {
        let dir = package_and_barrel();
        assert_eq!(
            locate(
                &dir.path()
                    .join("node_modules")
                    .join("a-pkg")
                    .join("index.js"),
                "run"
            ),
            None,
            "a direct hit inside node_modules is refused"
        );
        assert_eq!(
            locate(&dir.path().join("barrel.ts"), "run"),
            None,
            "a re-export hop into an installed package is refused"
        );
    }

    /// A helper file that cannot be read is a refusal, never a panic.
    #[test]
    fn a_missing_file_is_a_refusal() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(locate(&dir.path().join("absent.ts"), "run"), None);
    }

    // -- the export walk's refusals, one case per shape ----------------------------------------

    /// A temporary directory holding `files` as (name, source) pairs.
    fn helper_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, source) in files {
            std::fs::write(dir.path().join(name), source).unwrap();
        }
        dir
    }

    /// `length + 1` files `r0.ts`..: each re-exports `go` from the next, the last defines it.
    fn reexport_chain(length: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..length {
            let source = format!("export {{ go }} from './r{}';", index + 1);
            std::fs::write(dir.path().join(format!("r{index}.ts")), source).unwrap();
        }
        let last = dir.path().join(format!("r{length}.ts"));
        std::fs::write(last, "export function go(a) { return a; }").unwrap();
        dir
    }

    /// A directory holding an installed package whose entry exports `run`, plus a first-party
    /// `barrel.ts` re-exporting it: the dependency-refusal shape, shared by the inline walks'
    /// tests and the export contract's pins.
    fn package_and_barrel() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("node_modules").join("a-pkg");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"a-pkg","main":"index.js"}"#,
        )
        .unwrap();
        std::fs::write(
            package.join("index.js"),
            "export function run(label) { return label; }",
        )
        .unwrap();
        std::fs::write(dir.path().join("barrel.ts"), "export { run } from 'a-pkg';").unwrap();
        dir
    }

    /// Two files re-exporting one name to each other: the export-cycle shape.
    fn cycle_dir() -> tempfile::TempDir {
        helper_dir(&[
            ("a.ts", "export { spin } from './b';"),
            ("b.ts", "export { spin } from './a';"),
        ])
    }

    fn origin_at(path: PathBuf, exported: &str) -> ImportOrigin {
        ImportOrigin {
            component: String::new(),
            exported: exported.to_owned(),
            specifier: "./helper".to_owned(),
            resolved: Some(path),
        }
    }

    /// Every export shape the walk must step over, refuse, or admit, in one file each. The
    /// positive rows are the controls: a walk that refused everything would pass the refusals.
    #[test]
    fn the_export_walk_steps_over_unrelated_statements_and_refuses_unadmitted_shapes() {
        let cases: &[(&str, &str, &str, Option<usize>)] = &[
            ("class before the helper", "export class Box {}\nexport function run(a) { return a; }", "run", Some(1)),
            ("a class is not a callable", "export class Box {}", "Box", None),
            ("alias skips its siblings", "function a(x) { return x; }\nfunction b(x, y) { return y; }\nexport { a, b as c };", "c", Some(2)),
            ("default skipped for a named export", "export default function (x) { return x; }\nexport function run(a, b) { return a; }", "run", Some(2)),
            ("default arrow", "export default (label) => label;", "default", Some(1)),
            ("default non-callable", "export default 42;", "default", None),
            ("const function expression", "export const run = function (a, b) { return a; };", "run", Some(2)),
            ("const under another name", "export const other = (a) => a;", "run", None),
            ("const destructured", "export const { run } = helpers;", "run", None),
            ("const non-callable", "export const run = 5;", "run", None),
            ("alias to a const arrow", "const check = (label) => label;\nclass Unrelated {}\nexport { check as verify };", "verify", Some(1)),
            ("alias to nothing", "export { missing as verify };", "verify", None),
            ("overload signature without a body", "export function over(a: string): void;", "over", None),
        ];
        for (label, source, export, expected) in cases {
            let dir = helper_dir(&[("helper.ts", source)]);
            assert_eq!(
                locate(&dir.path().join("helper.ts"), export),
                *expected,
                "{label}: `{source}`"
            );
        }
    }

    /// A re-export hop the walk cannot follow ends in a refusal, and one it can follow still
    /// resolves: a sibling specifier is skipped, an unresolvable specifier refuses, and a chain
    /// longer than the hop cap refuses while a shorter one resolves.
    #[test]
    fn reexport_hops_refuse_what_they_cannot_follow() {
        let dir = helper_dir(&[
            ("multi.ts", "export { a, b } from './inner';"),
            ("inner.ts", "export function b(x) { return x; }"),
            ("dangling.ts", "export { x } from './nowhere';"),
        ]);
        assert_eq!(
            locate(&dir.path().join("multi.ts"), "b"),
            Some(1),
            "the sibling `a` is skipped"
        );
        assert_eq!(
            locate(&dir.path().join("dangling.ts"), "x"),
            None,
            "an unresolvable hop refuses"
        );

        let chain = reexport_chain(MAX_REEXPORT_HOPS + 2);
        assert_eq!(
            locate(&chain.path().join("r5.ts"), "go"),
            Some(1),
            "inside the cap"
        );
        assert_eq!(
            locate(&chain.path().join("r0.ts"), "go"),
            None,
            "past the cap"
        );
    }

    /// Rendering refuses on its own grounds, after `locate` has said yes or while it says no: a
    /// wrong argument count, a re-export cycle, a chain past the hop cap, and a helper file the
    /// semantic builder rejects. The first row is the control that rendering works at all.
    #[test]
    fn rendering_a_helper_refuses_on_arity_cycles_hop_cap_and_semantic_errors() {
        let dir = helper_dir(&[
            ("run.ts", "export function run(a) { return a; }"),
            (
                "bad.ts",
                "export function run(a) { let b = 1; let b = 2; return a; }",
            ),
        ]);
        let cycle = cycle_dir();
        let render = |path: std::path::PathBuf, export: &str, arguments: &[&str]| {
            let arguments: Vec<String> = arguments.iter().map(|a| (*a).to_owned()).collect();
            render_inline(&origin_at(path, export), &arguments, Mode::Alpha, 0)
        };
        assert!(
            render(dir.path().join("run.ts"), "run", &["x"]).is_some(),
            "the control renders"
        );
        assert_eq!(
            render(dir.path().join("run.ts"), "run", &["x", "y"]),
            None,
            "an arity mismatch refuses"
        );
        assert_eq!(
            render(cycle.path().join("a.ts"), "spin", &[]),
            None,
            "a re-export cycle refuses"
        );
        assert_eq!(
            render(dir.path().join("bad.ts"), "run", &["x"]),
            None,
            "a semantic error refuses"
        );

        let chain = reexport_chain(MAX_REEXPORT_HOPS + 2);
        let from_start = origin_at(chain.path().join("r0.ts"), "go");
        assert_eq!(
            render_inline(&from_start, &["x".to_owned()], Mode::Alpha, 0),
            None,
            "past the cap"
        );
        let near_end = origin_at(chain.path().join("r5.ts"), "go");
        assert!(
            render_inline(&near_end, &["x".to_owned()], Mode::Alpha, 0).is_some(),
            "inside the cap"
        );
    }

    // -- the C10 export-resolution contract ------------------------------------------------------

    /// The contract names the declaring file and the name that file's own exports admit, through
    /// direct exports, same-file aliases, and re-export chains that rename mid-chain. A terminal
    /// is self-contained: resolving from it again reaches itself with no hops.
    #[test]
    fn export_resolution_names_the_declaring_file_and_its_own_export_name() {
        let dir = helper_dir(&[
            ("direct.ts", "export function run(a) { return a; }"),
            (
                "alias.ts",
                "function check(a) { return a; }\nexport { check as verify };",
            ),
            ("index.ts", "export { go } from './mid';"),
            ("mid.ts", "export { run as go } from './inner';"),
            ("inner.ts", "export function run(a, b) { return a; }"),
        ]);
        let terminal = |file: &str, export: &str| {
            let resolved = resolve_export(&dir.path().join(file), export)
                .unwrap_or_else(|refusal| panic!("{file}:{export} refused: {refusal:?}"));
            (resolved.path.canonicalize().unwrap(), resolved.export)
        };
        // Canonicalized on both sides: the walk keeps a start file's path verbatim while
        // resolver-produced paths get their own spelling per platform (macOS tempdirs sit
        // behind a /var -> /private/var symlink; Windows resolves to 8.3 short names without
        // the \\?\ prefix std's canonicalize adds), so only canonical forms compare equal.
        let canonical = |file: &str| dir.path().join(file).canonicalize().unwrap();
        assert_eq!(
            terminal("direct.ts", "run"),
            (canonical("direct.ts"), "run".to_owned()),
            "a direct export's terminal is the declaring file under its own name"
        );
        assert_eq!(
            terminal("alias.ts", "verify"),
            (canonical("alias.ts"), "verify".to_owned()),
            "a same-file alias's terminal stays in that file, keyed by the exported name"
        );
        assert_eq!(
            terminal("index.ts", "go"),
            (canonical("inner.ts"), "run".to_owned()),
            "a chain that renames mid-hop ends at the declaring file, under the name it \
             declares there"
        );

        // Idempotence: re-resolving a terminal reaches itself, so a consumer re-parsing the
        // terminal file and looking the export up again cannot drift from the walk.
        let resolved = resolve_export(&dir.path().join("index.ts"), "go").unwrap();
        assert_eq!(
            resolve_export(&resolved.path, &resolved.export)
                .as_ref()
                .err(),
            None,
            "a terminal resolves from itself with no hops"
        );
    }

    /// Every refusal the walk can end in is an explicit, distinct reason. The rows are one
    /// edit away from their nearest positive in the tests above, so a reason firing on the
    /// wrong shape is caught, not just a reason existing.
    #[test]
    fn export_refusals_carry_explicit_reasons() {
        // An export name the file simply does not have.
        let absent_name = helper_dir(&[("helper.ts", "export function other(a) { return a; }")]);
        assert_eq!(
            resolve_export(&absent_name.path().join("helper.ts"), "run"),
            Err(ExportRefusal::MissingExport),
            "an absent export name"
        );
        // A present export of an unadmitted shape folds into the same reason: `find_export`
        // cannot distinguish it and this contract adds no second opinion.
        let unadmitted = helper_dir(&[("helper.ts", "export const run = 5;")]);
        assert_eq!(
            resolve_export(&unadmitted.path().join("helper.ts"), "run"),
            Err(ExportRefusal::MissingExport),
            "a present export of an unadmitted shape is the missing-export refusal"
        );

        let dangling = helper_dir(&[("dangling.ts", "export { x } from './nowhere';")]);
        assert_eq!(
            resolve_export(&dangling.path().join("dangling.ts"), "x"),
            Err(ExportRefusal::UnresolvedSpecifier),
            "a re-export hop whose specifier resolves nowhere"
        );

        let absent_file = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_export(&absent_file.path().join("absent.ts"), "run"),
            Err(ExportRefusal::UnreadableOrUnparseable),
            "a file that cannot be read"
        );
        let broken = helper_dir(&[("broken.ts", "export function broken( {")]);
        assert_eq!(
            resolve_export(&broken.path().join("broken.ts"), "broken"),
            Err(ExportRefusal::UnreadableOrUnparseable),
            "a file that does not parse"
        );

        let cycle = cycle_dir();
        assert_eq!(
            resolve_export(&cycle.path().join("a.ts"), "spin"),
            Err(ExportRefusal::ReexportCycle),
            "an export chain that returns to a (path, export) it already resolved through"
        );

        // Installed packages are refused before they are read, both as the chain's start and
        // as a first-party barrel's terminal hop.
        let package = package_and_barrel();
        assert_eq!(
            resolve_export(
                &package
                    .path()
                    .join("node_modules")
                    .join("a-pkg")
                    .join("index.js"),
                "run"
            ),
            Err(ExportRefusal::InstalledPackage),
            "a chain starting inside an installed package"
        );
        assert_eq!(
            resolve_export(&package.path().join("barrel.ts"), "run"),
            Err(ExportRefusal::InstalledPackage),
            "a first-party barrel whose terminal hop lands in an installed package"
        );
    }

    /// The exact hop boundary, on both sides: a chain of exactly [`MAX_REEXPORT_HOPS`] hops
    /// resolves to the declaring file, one hop more refuses with the hop-limit reason. This is
    /// the pin that keeps this walk and the inline walks' loops from silently disagreeing about
    /// what the shared bound means.
    #[test]
    fn the_reexport_hop_limit_is_exact() {
        // reexport_chain(N) is N hops: r0 -> ... -> rN defines `go`. From r1: exactly
        // MAX_REEXPORT_HOPS hops; from r0: one more.
        let chain = reexport_chain(MAX_REEXPORT_HOPS + 1);
        let resolved = resolve_export(&chain.path().join("r1.ts"), "go")
            .expect("a chain of exactly MAX_REEXPORT_HOPS hops resolves");
        // Canonicalized on both sides, like the terminal test above: a resolver-produced path
        // and a canonicalized one spell one file differently per platform (macOS /var symlink;
        // Windows 8.3 short names without the \\?\ prefix), so only canonical forms compare.
        assert_eq!(
            resolved.path.canonicalize().unwrap(),
            chain
                .path()
                .join(format!("r{}.ts", MAX_REEXPORT_HOPS + 1))
                .canonicalize()
                .unwrap(),
            "the terminal is the chain's declaring file"
        );
        assert_eq!(
            resolve_export(&chain.path().join("r0.ts"), "go"),
            Err(ExportRefusal::HopLimit),
            "one hop past the bound refuses with the hop-limit reason"
        );
    }

    /// The visited identity is canonical, so a cycle whose start was spelled differently from
    /// the resolver's spelling of the same file (`./`-component, symlinked or short-named
    /// directory) is still named a cycle. The construction lands the start's first revisit
    /// exactly on the hop boundary: eight files (`a` plus seven intermediates) means the return
    /// to `a` is iteration 8, inside the loop, while the lap the miss would have cost never
    /// runs -- without the canonical identity this exact chain refuses with the hop-limit
    /// reason instead.
    #[test]
    fn a_misspelled_start_still_names_a_cycle_at_the_hop_boundary() {
        let mut files = vec![("a.ts".to_owned(), "export { spin } from './c1';".to_owned())];
        for index in 1..MAX_REEXPORT_HOPS {
            let target = if index == MAX_REEXPORT_HOPS - 1 {
                "a".to_owned()
            } else {
                format!("c{}", index + 1)
            };
            files.push((
                format!("c{index}.ts"),
                format!("export {{ spin }} from './{target}';"),
            ));
        }
        let sources: Vec<(&str, &str)> = files
            .iter()
            .map(|(name, source)| (name.as_str(), source.as_str()))
            .collect();
        let dir = helper_dir(&sources);
        // `./` in the start spelling: canonicalized away on every platform, but never present
        // in a `resolve_hop` result, so the two spellings of `a.ts` differ as strings.
        assert_eq!(
            resolve_export(&dir.path().join("./a.ts"), "spin"),
            Err(ExportRefusal::ReexportCycle),
            "the start's first revisit is detected through the canonical identity"
        );
    }
}
