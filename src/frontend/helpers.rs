//! Imported-helper body resolution: which declaration an imported call site names, and the
//! guards under which that declaration's body may participate in the compared fingerprint.
//!
//! This is the `Helper-body resolution (M3)` backlog row's mechanism. Before it, a call like
//! `checkActive('dashboard')` rendered into `alpha`/`structural` as a call to an opaque
//! `free:import:resolved:<path>:checkActive` identity -- *which export it names*, never *what it
//! does*. Two tests differing only by calling equivalent helpers (different modules, or different
//! exports of one module) could not be recognized as duplicates, because identity is a name and
//! the bodies were never compared. [`fingerprint::render_inlined_helper_body`](super::fingerprint::render_inlined_helper_body) renders the
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
//!   chains, `export default <function | arrow>` and `export default <name>`.** A `let`/`var` export can be reassigned
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
    HelperRendering, ImportOrigin, Mode,
};
use super::resolver::{self, Resolved};
use crate::frontend::js_text::string_text;
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrowFunctionExpression, BindingPattern, Declaration, ExportDefaultDeclarationKind,
    ExportFromDeclaration, ExportSpecifier, Expression, Function, FunctionType,
    IdentifierReference, Program, Statement, VariableDeclaration, VariableDeclarationKind,
};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// How many named re-export hops a locate walk will follow before refusing.
///
/// Resolution hops, not body-inlining depth (that is bounded to one level by
/// [`fingerprint::render_inlined_helper_body`](super::fingerprint::render_inlined_helper_body) itself). Real chains are one or two long -- a
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
) -> Option<HelperRendering> {
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
///
/// Memoized process-wide per (path, export): every inline decision asks this, and the walk parses
/// the helper and each re-export hop. An answer is served only while every file the walk read
/// still has the stamp it had then (see [`settled_stamp`]); a walk that read an unsettled or
/// unstampable file is not cached.
fn locate(path: &Path, export: &str) -> Option<usize> {
    let key = (path.to_path_buf(), export.to_owned());
    let lock = || {
        LOCATE_ANSWERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    let cached = lock()
        .as_ref()
        .and_then(|answers| answers.get(&key).cloned());
    if let Some((files, answer)) = cached {
        if files
            .iter()
            .all(|(file, stamp)| current_stamp(file).as_ref() == Some(stamp))
        {
            return answer;
        }
    }
    let mut files = Vec::new();
    let answer = walk_locate(path, export, &mut files);
    if let Some(files) = files.into_iter().collect::<Option<Vec<_>>>() {
        let mut guard = lock();
        let answers = guard.get_or_insert_with(std::collections::HashMap::new);
        if answers.len() < MAX_LOCATE_ANSWERS || answers.contains_key(&key) {
            answers.insert(key, (files, answer));
        }
    }
    answer
}

/// Upper bound on memoized [`locate`] answers; past it, new lookups are walked uncached.
const MAX_LOCATE_ANSWERS: usize = 16_384;

/// One memoized [`locate`] answer: the files its walk read, with their stamps, and the answer.
type LocateAnswer = (Vec<(PathBuf, FileStamp)>, Option<usize>);

/// Memoized [`locate`] answers keyed by (helper path, export name).
static LOCATE_ANSWERS: std::sync::Mutex<
    Option<std::collections::HashMap<(PathBuf, String), LocateAnswer>>,
> = std::sync::Mutex::new(None);

/// Returns `path`'s current stamp, or `None` when it cannot be read or was modified too recently
/// to trust (see [`settled_stamp`]).
fn current_stamp(path: &Path) -> Option<FileStamp> {
    std::fs::metadata(path)
        .ok()
        .and_then(|metadata| settled_stamp(&metadata))
}

/// The uncached [`locate`] walk. Records each file it reads, with its stamp (`None` when the file
/// is unsettled, unstampable or not admitted), so the caller can decide whether the answer may be
/// memoized.
fn walk_locate(
    path: &Path,
    export: &str,
    files: &mut Vec<Option<(PathBuf, FileStamp)>>,
) -> Option<usize> {
    let mut visited = HashSet::new();
    let mut path = path.to_path_buf();
    let mut export = export.to_owned();
    for _ in 0..=MAX_REEXPORT_HOPS {
        if !visited.insert((path.clone(), export.clone())) {
            return None;
        }
        // Memoizable only while the answer is a property of the files' content: a file that was
        // not admitted (a read error, a refusal) leaves the walk uncached, so a helper made
        // readable again is walked afresh. An installed package is refused by path alone. The
        // admission check runs only on a settled file, whose source the cache then serves.
        files.push(
            current_stamp(&path)
                .filter(|_| is_dependency(&path) || admitted_source(&path).is_some())
                .map(|stamp| (path.clone(), stamp)),
        );
        // The program borrows this frame's arena; everything found in it is consumed before the
        // loop moves to the next hop's file.
        let found = with_parsed_file(&path, |program, _| match find_export(program, &export)? {
            Export::Local(callable) => bindable_parameters(&callable).map(CallableOrHop::Callable),
            Export::Reexport { specifier, local } => {
                let next = resolve_hop(&path, &specifier);
                // An unresolved target is no file to stamp; creating it must change the answer.
                if next.is_none() {
                    files.push(None);
                }
                next.map(|next| CallableOrHop::Hop((next, local)))
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
        canonical, find_export, find_value_export, is_dependency, resolve_hop, with_parsed_file,
        Export, ValueExport, MAX_REEXPORT_HOPS,
    };
    use oxc_ast::ast::Program;
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
        /// The chain's finder ([`find_export`] for callables, [`find_value_export`] for values)
        /// found no admitted export of this name in this file. A present export of a shape that
        /// finder does not admit (for callables `export const run = 5`; for either a class or
        /// `export *`) folds into this reason: the finder cannot distinguish it, and this
        /// contract adds no second opinion.
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
        /// The export name looked up in each of `modules`, index-parallel.
        pub(in crate::frontend) exports: Vec<String>,
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
        walk_chain(start, export, |program, export| {
            find_export(program, export).map(|found| match found {
                Export::Local(_) => Step::Terminal,
                Export::Reexport { specifier, local } => Step::Hop { specifier, local },
            })
        })
    }

    /// [`resolve_export_chain`] for a value export: the terminal file declares the name with a
    /// module-level binding ([`find_value_export`]) rather than an admitted callable.
    pub(in crate::frontend) fn resolve_value_export_chain(
        start: &Path,
        export: &str,
    ) -> Result<ResolvedExportChain, ExportRefusal> {
        walk_chain(start, export, |program, export| {
            find_value_export(program, export).map(|found| match found {
                ValueExport::Declared(_) => Step::Terminal,
                ValueExport::Reexport { specifier, local } => Step::Hop { specifier, local },
            })
        })
    }

    /// The bounded hop loop both chains share; `find` decides one file's step.
    fn walk_chain(
        start: &Path,
        export: &str,
        find: impl Fn(&Program, &str) -> Option<Step>,
    ) -> Result<ResolvedExportChain, ExportRefusal> {
        let mut path = start.to_path_buf();
        let mut export = export.to_owned();
        let mut visited = HashSet::new();
        let mut modules = Vec::new();
        let mut exports = Vec::new();
        for hop in 0..=MAX_REEXPORT_HOPS {
            // The visited identity is the canonical spelling: `start` arrives verbatim while
            // `resolve_hop` returns symlink-followed paths (macOS /var vs /private/var, Windows
            // 8.3 short names), so a cycle returning to the start file would otherwise be
            // missed for one lap -- and a cycle long enough that the extra lap crosses the hop
            // budget would surface as the hop limit instead of the cycle reason (review-found;
            // pinned by `a_misspelled_start_still_names_a_cycle_at_the_hop_boundary`).
            let identity = canonical(&path);
            if !visited.insert((identity, export.clone())) {
                return Err(ExportRefusal::ReexportCycle);
            }
            if is_dependency(&path) {
                return Err(ExportRefusal::InstalledPackage);
            }
            // `Some(None)` keeps a missing export distinct from a file that cannot be read or
            // parsed; `with_parsed_file` flattens both to `None` otherwise.
            modules.push(path.clone());
            exports.push(export.clone());
            let step = with_parsed_file(&path, |program, _| Some(find(program, &export)))
                .ok_or(ExportRefusal::UnreadableOrUnparseable)?;
            match step {
                Some(Step::Terminal) => {
                    return Ok(ResolvedExportChain {
                        terminal: ResolvedExport { path, export },
                        modules,
                        exports,
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
) -> Option<HelperRendering> {
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
                let semantic_return = SemanticBuilder::new().with_build_nodes(true).build(program);
                if !semantic_return.diagnostics.is_empty() {
                    return None;
                }
                let semantic = semantic_return.semantic;
                // A reassigned `function f` is not the body this walk read: `f = x => x > 5`
                // replaces it before any call.
                if let Callable::Function(function) = &callable {
                    let reassigned = function
                        .id
                        .as_ref()
                        .and_then(|id| id.symbol_id.get())
                        .is_some_and(|symbol| semantic.scoping().symbol_is_mutated(symbol));
                    if reassigned {
                        return None;
                    }
                }
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

/// `path` with symlinks followed, or as given when it cannot be resolved: the identity two paths
/// naming one file share.
pub(super) fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
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
pub(super) fn is_dependency(path: &Path) -> bool {
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
    #[cfg(test)]
    HELPER_PARSES.with(|parses| parses.set(parses.get() + 1));
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

/// The stamp a cache entry for this file may be keyed on: its length and modification time, or
/// `None` when the modification time is unreadable or within [`RACY_MODIFICATION_WINDOW`] of now.
fn settled_stamp(metadata: &std::fs::Metadata) -> Option<FileStamp> {
    let modified = metadata.modified().ok()?;
    let age = modified.elapsed().ok()?;
    (age > RACY_MODIFICATION_WINDOW).then_some(FileStamp {
        len: metadata.len(),
        modified,
    })
}

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
    /// Test-only count of helper files this thread parsed.
    static HELPER_PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Returns `path`'s source when it is admitted for parsing (readable, within the size bound and
/// the raw syntax budget), or `None` for a refusal. Each distinct file version is read and
/// checked once; later calls reuse the answer while the file's length and modification time are
/// unchanged. A file without a readable modification time, or modified within
/// [`RACY_MODIFICATION_WINDOW`], is never cached.
pub(super) fn admitted_source(path: &Path) -> Option<std::sync::Arc<str>> {
    let metadata = std::fs::metadata(path).ok()?;
    let stamp = settled_stamp(&metadata);
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
                if let Some(local) = specifier_local(&it.specifiers, export) {
                    return module_level_callable(program, &local).map(Export::Local);
                }
            }
            Statement::ExportFromDeclaration(it) => {
                if let Some((specifier, local)) = reexport_hop(it, export) {
                    return Some(Export::Reexport { specifier, local });
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
                    // `export default name`: the same targets `export { name as default }` admits.
                    // The statement evaluates the name where it stands, so a `const` declared
                    // later is still in its temporal dead zone; only a hoisted `function`
                    // declaration may follow it.
                    ExportDefaultDeclarationKind::Identifier(identifier) => {
                        module_level_callable(program, identifier.name.as_str())
                            .filter(|callable| match callable {
                                // A bodyless `declare function` is erased: nothing exists to name.
                                Callable::Function(function) if function.body.is_none() => false,
                                Callable::Function(function) => {
                                    function.r#type == FunctionType::FunctionDeclaration
                                        || function.span.end <= it.span.start
                                }
                                Callable::Arrow(arrow) => arrow.span.end <= it.span.start,
                            })
                            .map(Export::Local)
                    }
                    _ => None,
                };
            }
            _ => {}
        }
    }
    None
}

/// The local side of the non-type `export { local as export }` specifier in `specifiers`.
fn specifier_local(specifiers: &[ExportSpecifier], export: &str) -> Option<String> {
    specifiers
        .iter()
        .find(|specifier| {
            !matches!(
                specifier.export_kind,
                oxc_ast::ast::ImportOrExportKind::Type
            ) && module_export_name(&specifier.exported) == export
        })
        .map(|specifier| module_export_name(&specifier.local))
}

/// The `(specifier, local)` hop a non-type `export { local as export } from 'specifier'` names.
fn reexport_hop(it: &ExportFromDeclaration, export: &str) -> Option<(String, String)> {
    if matches!(it.export_kind, oxc_ast::ast::ImportOrExportKind::Type) {
        return None;
    }
    specifier_local(&it.specifiers, export)
        .map(|local| (string_text(&it.source).into_owned(), local))
}

/// Whether `value` may be a function: anything but a value that provably is not one -- a literal,
/// a template, an array or object literal, an operator's result, or a call to the global `Symbol`
/// (`is_global` says whether a name resolves to nothing in its file). A call, `new`, a member read
/// or a name may produce a function, so it may.
pub(in crate::frontend) fn value_may_call(
    value: &Expression,
    is_global: &dyn Fn(&IdentifierReference) -> bool,
) -> bool {
    let value = value.get_inner_expression();
    !value.is_literal()
        && !matches!(
            value,
            Expression::TemplateLiteral(_)
                | Expression::ArrayExpression(_)
                | Expression::ObjectExpression(_)
                | Expression::UnaryExpression(_)
                | Expression::BinaryExpression(_)
                | Expression::UpdateExpression(_)
        )
        && !matches!(value, Expression::CallExpression(call)
            if matches!(&call.callee, Expression::Identifier(callee)
                if callee.name == "Symbol" && is_global(callee)))
}

/// Whether the first-party export `export`, reached from `start` through its re-export chain, may
/// be a function: a callable export, a `const` whose initializer may be one
/// ([`value_may_call`]), or anything this cannot read.
pub(in crate::frontend) fn exported_value_may_call(start: &Path, export: &str) -> bool {
    if export_contract::resolve_export_chain(start, export).is_ok() {
        return true;
    }
    let Ok(chain) = export_contract::resolve_value_export_chain(start, export) else {
        return true;
    };
    let terminal = chain.terminal;
    with_parsed_file(&terminal.path, |program, _| {
        let Some(ValueExport::Declared(local)) = find_value_export(program, &terminal.export)
        else {
            return Some(true);
        };
        let semantic = SemanticBuilder::new().build(program).semantic;
        let scoping = semantic.scoping();
        let is_global = |ident: &IdentifierReference| {
            ident
                .reference_id
                .get()
                .and_then(|reference| scoping.get_reference(reference).symbol_id())
                .is_none()
        };
        let declarator = program.body.iter().find_map(|statement| {
            let variables = match statement {
                Statement::VariableDeclaration(variables) => variables,
                Statement::ExportDeclaration(export) => match &export.declaration {
                    Declaration::VariableDeclaration(variables) => variables,
                    _ => return None,
                },
                _ => return None,
            };
            variables
                .declarations
                .iter()
                .find(|declarator| {
                    matches!(&declarator.id, BindingPattern::BindingIdentifier(binding)
                        if binding.name.as_str() == local)
                })
                .map(|declarator| (variables.kind, declarator))
        });
        Some(match declarator {
            Some((VariableDeclarationKind::Const, declarator)) => declarator
                .init
                .as_ref()
                .is_none_or(|init| value_may_call(init, &is_global)),
            _ => true,
        })
    })
    .unwrap_or(true)
}

/// What a named export of a value is in `program`: declared by a module-level binding (its local
/// name; whether that binding is a provable constant is the caller's semantic question), or one
/// more named re-export hop. Default exports and `export *` are refused, as in [`find_export`].
pub(super) fn find_value_export(program: &Program, export: &str) -> Option<ValueExport> {
    for statement in &program.body {
        match statement {
            Statement::ExportDeclaration(it) => {
                if let Declaration::VariableDeclaration(variables) = &it.declaration {
                    let declares = variables.declarations.iter().any(|declarator| {
                        matches!(&declarator.id, BindingPattern::BindingIdentifier(binding)
                            if binding.name.as_str() == export)
                    });
                    if declares {
                        return Some(ValueExport::Declared(export.to_owned()));
                    }
                }
            }
            Statement::ExportNamedDeclaration(it) => {
                if matches!(it.export_kind, oxc_ast::ast::ImportOrExportKind::Type) {
                    continue;
                }
                if let Some(local) = specifier_local(&it.specifiers, export) {
                    // `import { X } from './a'; export { X }` is a re-export hop too.
                    return Some(match imported_binding(program, &local) {
                        Some((specifier, local)) => ValueExport::Reexport { specifier, local },
                        None => ValueExport::Declared(local),
                    });
                }
            }
            Statement::ExportFromDeclaration(it) => {
                if let Some((specifier, local)) = reexport_hop(it, export) {
                    return Some(ValueExport::Reexport { specifier, local });
                }
            }
            _ => {}
        }
    }
    None
}

/// The `(specifier, imported name)` of the named value import that binds `local` in `program`.
fn imported_binding(program: &Program, local: &str) -> Option<(String, String)> {
    program.body.iter().find_map(|statement| {
        let Statement::ImportDeclaration(import) = statement else {
            return None;
        };
        if matches!(import.import_kind, oxc_ast::ast::ImportOrExportKind::Type) {
            return None;
        }
        import
            .specifiers
            .iter()
            .flatten()
            .find_map(|specifier| match specifier {
                oxc_ast::ast::ImportDeclarationSpecifier::ImportSpecifier(specifier)
                    if specifier.local.name == local
                        && !matches!(
                            specifier.import_kind,
                            oxc_ast::ast::ImportOrExportKind::Type
                        ) =>
                {
                    Some((
                        string_text(&import.source).into_owned(),
                        module_export_name(&specifier.imported),
                    ))
                }
                _ => None,
            })
    })
}

/// What [`find_value_export`] found.
pub(super) enum ValueExport {
    /// The module-level binding, by local name, that declares the export in this file.
    Declared(String),
    /// One more named re-export hop, as in [`Export::Reexport`].
    Reexport { specifier: String, local: String },
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

/// A module-level `function name(...)` or `const name = <function or arrow>` declaration (exported
/// or not), for `export { name as exported }` and `export default name`: the target must be one of
/// the admitted shapes in this same file.
fn module_level_callable<'p>(program: &'p Program<'p>, name: &str) -> Option<Callable<'p>> {
    for statement in &program.body {
        match statement {
            // The declaration may itself be exported (`export const name = ...`), the usual shape
            // when the same function is also the default export.
            Statement::ExportDeclaration(it) => match &it.declaration {
                Declaration::FunctionDeclaration(function)
                    if function
                        .id
                        .as_ref()
                        .is_some_and(|id| id.name.as_str() == name) =>
                {
                    return Some(Callable::Function(function));
                }
                Declaration::VariableDeclaration(variables) => {
                    if let Some(callable) = exported_const_callable(variables, name) {
                        return Some(callable);
                    }
                }
                _ => {}
            },
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
pub(super) fn bindable_parameters(callable: &Callable<'_>) -> Option<usize> {
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
mod tests;
