//! Every registration site of a module, found by provenance, never by evaluation.
//!
//! A call is a site when its callee resolves to a registration function: an API export reached
//! through imports, namespaces, `const` aliases, values a fixture extension builds, the in-source
//! member or a free global (when the project enables globals); or a function whose body holds a
//! site (a wrapper), in this module or imported from another project module. `test.each(rows)('t',
//! fn)` is one site at the outer call. A registration function read as a value (`[test].forEach`,
//! `run(test)`) is an escape: what it registers there is not followed. Nothing is executed or
//! unrolled.
//!
//! API names are read from the [`Dialect`]; this module spells none.

mod module;
mod reach;
pub mod runners;

use crate::framework::{Dialect, Registers};
use crate::lang::{NodeId, Parsed, Span};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// What a site registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteKind {
    /// A test, suite, benchmark or hook.
    Registers(Registers),
    /// A call of a wrapper, and what it may register.
    Wrapper(Reach),
}

/// What calling a wrapper may register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reach {
    /// Hooks only: every registration in its body is a hook or a call of a hooks-only wrapper.
    Hooks,
    /// A hook read under another name: it registers a hook, and the framework reads its arguments.
    Hook,
    /// Tests, suites or benchmarks, or what the inventory cannot tell.
    Tasks,
}

/// What an export of a module holds, for the modules importing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Export {
    /// The API's namespace, which nothing replaces.
    Namespace,
    /// A registration function the API's module exports, re-exported as it is.
    Api {
        /// The registration category exposed by the binding.
        registers: Registers,
        /// Its terminal dialect API module, borrowed from immutable dialect data.
        module: &'static str,
        /// Its terminal export name, retained through renamed barrel exports.
        name: &'static str,
    },
    /// A framework registration value exported through a local alias or builder.
    Derived(Registers),
    /// A function or value that registers when called.
    Wrapper(Reach),
}

impl Export {
    /// A later star needs the same proven terminal binding, not merely the same reach.
    fn through_star(self, prior: Option<Self>, first: bool, spelled: bool) -> Self {
        match self {
            Self::Wrapper(_) => self,
            _ if !spelled
                && (first || (matches!(self, Self::Api { .. }) && prior == Some(self))) =>
            {
                self
            }
            _ => Self::Wrapper(Reach::Tasks),
        }
    }

    /// What a name holds when it is reached twice: the widest reach of two wrappers, and anything
    /// else that disagrees may be either.
    fn widest(self, other: Self) -> Self {
        match (self, other) {
            (Self::Wrapper(left), Self::Wrapper(right)) => Self::Wrapper(left.max(right)),
            (left, right) if left == right => left,
            _ => Self::Wrapper(Reach::Tasks),
        }
    }
}

/// How the callee of a site reaches the API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provenance {
    /// An import, a namespace member, the in-source member, an alias or a value derived from one.
    Direct,
    /// A name no scope declares, which the project's globals provide.
    Global,
    /// A wrapper, or a value a call handed a registration function returns.
    Wrapper,
    /// An exported framework registration value, rather than a user-defined runner body.
    Derived,
}

/// A lexical fact around a site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    /// Inside a loop.
    Loop,
    /// Inside a branch.
    Conditional,
    /// Inside a `try`.
    Try,
    /// Inside the callback a registration receives.
    RegistrationBody,
    /// Inside a callback another call receives.
    Callback,
    /// Inside some other function.
    Function,
}

/// One registering call.
#[derive(Clone, Debug)]
pub struct Site {
    /// The call (or tagged template).
    pub node: NodeId,
    /// Its span.
    pub span: Span,
    /// What it registers.
    pub kind: SiteKind,
    /// The callee chain past the registration name: member names, `()` for a call.
    pub chain: Vec<String>,
    /// The member of a test function the callee registers through (`test.skip.describe`), when
    /// it is one: nothing chained before it applies to the call.
    pub member: Option<NodeId>,
    /// How the callee reaches the API.
    pub provenance: Provenance,
    /// The title, when the first argument is a string the source spells.
    pub title: Option<String>,
    /// Where V8 places the call, which is where Vitest places what it registers: the property of
    /// a member callee, the `(` of a call whose callee is itself a call, else the callee.
    pub position: u32,
    /// Where a transform's source map may place it instead: the callee's start for a call of a
    /// call, else [`Site::position`].
    pub from: u32,
    /// The lexical facts around it, innermost first.
    pub labels: Vec<Label>,
}

/// A registration function read as a value rather than called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Escape {
    /// Where the value is read.
    pub offset: u32,
    /// The expression reading it, cut to 80 characters.
    pub text: String,
}

/// A module-top-level statement calling a mock member the framework does not hoist
/// (`tools.mock` for `vi as tools`): it changes module state where it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InPlaceMock {
    /// The top-level statement.
    pub statement: NodeId,
    /// The call.
    pub call: NodeId,
    /// The member it calls on the canonical mocking namespace (`vi.mock`), whatever binding
    /// spells it.
    pub callee: String,
}

/// What one module registers.
#[derive(Clone, Debug, Default)]
pub struct Inventory {
    /// Import analysis reached a depth or aggregate work bound; its ambient effects are unknown.
    pub imports_limited: bool,
    /// The first import analysis bound reached, retained for internal diagnostics.
    pub(crate) import_limit: Option<ImportLimit>,
    /// Intrinsic direct eval may write lexical bindings without ordinary assignment references.
    pub direct_eval: bool,
    /// A `vi.doMock` or `vi.doUnmock`, or a mock call the framework does not hoist outside a
    /// top-level statement, changes module state at its execution point; a hoisted mock call
    /// outside the top level makes the framework refuse the file.
    pub non_hoisted_mock: bool,
    /// Reachable top-level mock calls the framework does not hoist, in source order.
    pub in_place_mocks: Vec<InPlaceMock>,
    /// A `vi.mock` factory, hoisted or in place, is not an inline function or proven spy option.
    pub unreadable_mock_factory: bool,
    /// A `test.extend` fixture value cannot be represented without evaluating user code.
    pub unreadable_fixture: bool,
    /// The module can read non-registration, non-assertion framework utilities. An invocation
    /// proof must account for their setup/mocking semantics even when a read is in a callback.
    pub setup_access: bool,
    /// Registering calls, in source order.
    pub sites: Vec<Site>,
    /// Imports of project modules that may register while they load, by where the import starts.
    pub loads: Vec<u32>,
    /// Registration functions read as values.
    pub escapes: Vec<Escape>,
    /// Nodes whose value is a registration function (`test.extend({...})`, `describe`), so the
    /// normal form can tell building one from creating an object.
    pub registration_values: HashSet<NodeId>,
}

/// Whether intrinsic eval can write lexical state outside the static reference graph.
pub(crate) fn direct_eval(parsed: &Parsed) -> bool {
    let tree = &parsed.tree;
    tree.nodes()
        .filter(|&node| tree.kind(node) == "CallExpression")
        .filter_map(|node| tree.child(node, "callee"))
        .map(|callee| tree.inner(callee))
        .any(|callee| {
            tree.kind(callee) == "Identifier"
                && tree.text(callee, "name") == Some("eval")
                && !matches!(
                    parsed.scopes.name_at(tree.span(callee).start),
                    Some(crate::lang::Name::Reference(Some(_)))
                )
        })
}

/// Whether a module can read framework setup utilities whose effects this inventory does not model.
/// This is deliberately a capability refusal, not a prediction of which calls are hoisted.
pub(crate) fn setup_access(parsed: &Parsed, setting: Setting) -> bool {
    use crate::lang::{Declaration, Imported, Name};
    let tree = &parsed.tree;
    let safe = |name: &str| {
        setting.dialect.registration(name).is_some()
            || setting
                .dialect
                .assertions
                .iter()
                .any(|entry| entry.name == name)
    };
    for symbol in parsed.scopes.symbols() {
        let Declaration::Import {
            import,
            specifier,
            ref imported,
            ..
        } = parsed.scopes.symbol(symbol).declaration
        else {
            continue;
        };
        if tree.text(import, "importKind") == Some("type")
            || tree.text(specifier, "importKind") == Some("type")
        {
            continue;
        }
        if tree
            .child(import, "source")
            .and_then(|node| tree.text(node, "value"))
            .is_some_and(|source| setting.dialect.is_api_module(source))
            && !matches!(imported, Imported::Named(name) if safe(name))
        {
            return true;
        }
    }
    for statement in tree.list(tree.root(), "body").into_iter().flatten() {
        if tree.kind(statement) == "ExportNamedDeclaration"
            && tree.text(statement, "exportKind") != Some("type")
            && tree
                .child(statement, "source")
                .and_then(|node| tree.text(node, "value"))
                .is_some_and(|source| setting.dialect.is_api_module(source))
            && tree
                .list(statement, "specifiers")
                .into_iter()
                .flatten()
                .any(|specifier| {
                    tree.text(specifier, "exportKind") != Some("type")
                        && tree
                            .child(specifier, "local")
                            .is_none_or(|local| !safe(&crate::lang::export_name(tree, local)))
                })
        {
            return true;
        }
    }
    tree.nodes().any(|node| {
        tree.kind(node) == "Identifier"
            && parsed.scopes.name_at(tree.span(node).start) == Some(Name::Reference(None))
            && tree.text(node, "name").is_some_and(|name| {
                setting
                    .dialect
                    .namespaces
                    .iter()
                    .any(|entry| entry.name == name)
                    && !safe(name)
            })
    })
}

/// The export name standing for "registers while the module loads".
const ON_LOAD: &str = "\0load";
/// Export-map marker: a derived registration API carries an unreadable fixture context.
const UNREADABLE_FIXTURE: &str = "\0fixture";
/// How many imports deep a module's exports are followed. Reaching it refuses the whole root, so
/// it sits above real import chains; the analysis and byte caps below bound the work.
const MAX_DEPTH: usize = 32;
/// Uncached module analysis attempts allowed by one pass over a root inventory.
const MAX_IMPORT_ANALYSES: usize = 64;
/// Passes allowed when a refused pass leaves new complete module proofs in the cache.
const MAX_IMPORT_ANALYSIS_PASSES: usize = 3;
/// Source bytes admitted to uncached import analysis by one root inventory reading.
const MAX_IMPORT_SOURCE_BYTES: usize = 8 * 1024 * 1024;

/// Which bounded import resource stopped one root's inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImportLimitKind {
    /// The import chain passed [`MAX_DEPTH`].
    Depth,
    /// The root exhausted its bounded passes of [`MAX_IMPORT_ANALYSES`] uncached analyses.
    Analyses,
    /// Imported sources would pass [`MAX_IMPORT_SOURCE_BYTES`].
    SourceBytes,
}

/// The first bounded import resource exhausted while reading one root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportLimit {
    /// The exhausted bound.
    pub(crate) kind: ImportLimitKind,
    /// The first module refused, relative to the analysis root when it is inside it.
    pub(crate) module: PathBuf,
    /// The import depth requested for that module.
    pub(crate) depth: usize,
    /// Uncached module analyses already attempted, including a source refused by the byte bound.
    pub(crate) analyses: usize,
    /// Imported source bytes admitted before the refusal.
    pub(crate) bytes: usize,
}

impl ImportLimit {
    /// A root-independent diagnostic for reports.
    pub(crate) fn detail(&self) -> String {
        let bound = match self.kind {
            ImportLimitKind::Depth => "depth",
            ImportLimitKind::Analyses => "module-analysis count",
            ImportLimitKind::SourceBytes => "source-byte work",
        };
        let module = self.module.to_string_lossy().replace('\\', "/");
        format!(
            "import analysis reached its {bound} limit at {module} (depth {}, {} analyses, {} source bytes), so imported effects and registrations are unknown",
            self.depth, self.analyses, self.bytes
        )
    }
}

/// Work already spent by one root reading; a refusal stops its remaining import analyses.
#[derive(Debug)]
struct ImportWork {
    /// Analyses completed by earlier passes over this root.
    prior_analyses: usize,
    /// Analyses attempted by the current pass.
    analyses: usize,
    /// Aggregate imported source bytes admitted by every pass over this root.
    bytes: usize,
    refusal: Option<ImportLimit>,
    /// Whether completed cache entries are the only lasting effects of this pass.
    retry_safe: bool,
}

impl Default for ImportWork {
    fn default() -> Self {
        Self {
            prior_analyses: 0,
            analyses: 0,
            bytes: 0,
            refusal: None,
            retry_safe: true,
        }
    }
}

/// Export names whose value registers when called, per project module and globals setting (a
/// project with globals reads free names as the API), shared by every file of one run.
#[derive(Debug, Default)]
pub struct Modules {
    sources: crate::lang::Sources,
    cache: crate::memory::RetainedMap<(PathBuf, bool), HashMap<String, Export>>,
    /// Advances only when a new complete module proof enters `cache`.
    cache_generation: usize,
    active: HashSet<PathBuf>,
    /// Reset for each pass over a root; incomplete ancestor proofs never enter the shared cache.
    work: ImportWork,
}

/// What one run's inventory reads besides the module itself.
#[derive(Clone, Copy, Debug)]
pub struct Setting<'a> {
    /// The framework's API.
    pub dialect: &'a Dialect,
    /// Whether free names are the API.
    pub globals: bool,
    /// The analysis root: imports resolving outside it are not followed.
    pub root: &'a Path,
}

impl Modules {
    /// One private cache belongs to one source authority for its whole lifetime.
    pub(crate) fn with_sources(sources: crate::lang::Sources) -> Self {
        Self {
            sources,
            ..Self::default()
        }
    }

    /// Record only the first refusal: later imports stop before they can replace its cause.
    fn refuse(&mut self, kind: ImportLimitKind, path: &Path, setting: Setting, depth: usize) {
        if self.work.refusal.is_none() {
            self.work.refusal = Some(ImportLimit {
                kind,
                module: self
                    .sources
                    .relative(path, setting.root)
                    .or_else(|| path.strip_prefix(setting.root).ok().map(Path::to_path_buf))
                    .or_else(|| path.file_name().map(PathBuf::from))
                    .unwrap_or_else(|| PathBuf::from("<module>")),
                depth,
                analyses: self.work.prior_analyses.saturating_add(self.work.analyses),
                bytes: self.work.bytes,
            });
        }
    }

    /// The registering exports of the module at `path`, with what each may register, analysed
    /// once unless a bound cut the analysis short. An empty map after a cutoff is partial:
    /// the root's inventory records the refusal and cannot license comparison claims.
    fn exports(&mut self, path: &Path, setting: Setting, depth: usize) -> HashMap<String, Export> {
        if self.work.refusal.is_some() {
            return HashMap::new();
        }
        let key = (path.to_path_buf(), setting.globals);
        if let Some(found) = self.cache.get(&key) {
            return found.clone();
        }
        if depth > MAX_DEPTH {
            self.refuse(ImportLimitKind::Depth, path, setting, depth);
            return HashMap::new();
        }
        if !self.active.insert(path.to_path_buf()) {
            return HashMap::new();
        }
        if self.work.analyses >= MAX_IMPORT_ANALYSES {
            self.active.remove(path);
            self.refuse(ImportLimitKind::Analyses, path, setting, depth);
            return HashMap::new();
        }
        self.work.analyses += 1;
        let confined_before = crate::lang::confinement_refusals();
        let refused_before = crate::lang::manifest_refusals();
        let unread_before = UNREAD_IMPORTS.with(std::cell::Cell::get);
        let mut unparsed = false;
        let exports = self
            .sources
            .read(path)
            .and_then(|source| {
                if source.len() > MAX_IMPORT_SOURCE_BYTES - self.work.bytes {
                    self.refuse(ImportLimitKind::SourceBytes, path, setting, depth);
                    return None;
                }
                self.work.bytes += source.len();
                match crate::lang::parse(path, &source) {
                    Ok(parsed) => {
                        Some(module::analyse(path, &source, &parsed, setting, self, depth).1)
                    }
                    Err(_) => {
                        unparsed = true;
                        None
                    }
                }
            })
            .unwrap_or_default();
        self.active.remove(path);
        // Each importer records the refusal itself, so the module is read again, not cached.
        if unparsed {
            self.work.retry_safe = false;
            UNREAD_IMPORTS.with(|count| count.set(count.get() + 1));
            return exports;
        }
        // A module whose analysis saw a confinement refusal, a refused package manifest or a module
        // it could not parse is not cached, as with a cut: the next file re-reads it and records
        // its own refusal instead of inheriting silence.
        let reusable = crate::lang::confinement_refusals() == confined_before
            && crate::lang::manifest_refusals() == refused_before
            && UNREAD_IMPORTS.with(std::cell::Cell::get) == unread_before;
        if !reusable {
            self.work.retry_safe = false;
        }
        if self.work.refusal.is_none()
            && reusable
            && self.cache.insert(key, exports.clone()).is_none()
        {
            self.cache_generation = self.cache_generation.saturating_add(1);
        }
        exports
    }
}

thread_local! {
    /// Modules a reading followed and could not parse, on the analyzing thread.
    static UNREAD_IMPORTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Takes whether the calling thread followed a module it could not parse since the last take.
pub(crate) fn take_unread_imports() -> bool {
    UNREAD_IMPORTS.with(|count| count.replace(0)) > 0
}

/// The registration sites of the module at `path`, following project imports through `modules`.
pub fn inventory(
    path: &Path,
    source: &str,
    parsed: &Parsed,
    setting: Setting,
    modules: &mut Modules,
) -> Inventory {
    let mut prior_analyses = 0usize;
    let mut bytes = 0usize;
    for pass in 1..=MAX_IMPORT_ANALYSIS_PASSES {
        let cache_generation = modules.cache_generation;
        modules.work = ImportWork {
            prior_analyses,
            bytes,
            ..ImportWork::default()
        };
        let found = module::analyse(path, source, parsed, setting, modules, 0).0;
        let analysis_refusal = matches!(
            modules.work.refusal.as_ref().map(|limit| limit.kind),
            Some(ImportLimitKind::Analyses)
        );
        let cache_progress = modules.cache_generation > cache_generation;
        if !analysis_refusal
            || !cache_progress
            || !modules.work.retry_safe
            || pass == MAX_IMPORT_ANALYSIS_PASSES
        {
            return found;
        }
        prior_analyses = prior_analyses.saturating_add(modules.work.analyses);
        bytes = modules.work.bytes;
    }
    unreachable!("the bounded import retry loop always returns")
}

#[cfg(test)]
mod tests;

impl crate::memory::HeapBytes for Export {
    /// Cached registration reach is inline; exhaustively account for every export kind.
    fn heap_bytes(&self) -> usize {
        match self {
            Self::Namespace => 0,
            Self::Api { registers, .. } | Self::Derived(registers) => registers.heap_bytes(),
            Self::Wrapper(Reach::Hook | Reach::Hooks | Reach::Tasks) => 0,
        }
    }
}

impl crate::memory::HeapBytes for Modules {
    /// Charge retained export maps and active-set capacity, never temporary parsed trees.
    fn heap_bytes(&self) -> usize {
        let Self {
            // The owning Run charges this shared catalog once, alongside the module cache.
            sources: _,
            cache,
            cache_generation: _,
            active,
            work: _,
        } = self;
        cache.heap_bytes().saturating_add(active.heap_bytes())
    }
}
