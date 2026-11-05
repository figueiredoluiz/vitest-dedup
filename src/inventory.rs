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
    /// Intrinsic direct eval may write lexical bindings without ordinary assignment references.
    pub direct_eval: bool,
    /// A `vi.doMock` or `vi.doUnmock`, or a mock call the framework does not hoist outside a
    /// top-level statement, changes module state at its execution point.
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
/// How many imports deep a module's exports are followed.
const MAX_DEPTH: usize = 8;

/// Export names whose value registers when called, per project module and globals setting (a
/// project with globals reads free names as the API), shared by every file of one run.
#[derive(Debug, Default)]
pub struct Modules {
    sources: crate::lang::Sources,
    cache: crate::memory::RetainedMap<(PathBuf, bool), HashMap<String, Export>>,
    active: HashSet<PathBuf>,
    /// How many lookups the depth limit has cut short. A module whose analysis saw a cut is not
    /// cached: read again from nearer the test file, the limit may not reach it. (A cycle is cut
    /// too, but every member's result is cached: re-reading cycles would re-read a barrel's whole
    /// cluster for every test file.)
    deep_cuts: usize,
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

    /// The registering exports of the module at `path`, with what each may register, analysed
    /// once unless the depth limit cut the analysis short. A cut import reads as exporting
    /// nothing, so no hooks-only answer rests on it (see `Module::closed`).
    fn exports(&mut self, path: &Path, setting: Setting, depth: usize) -> HashMap<String, Export> {
        let key = (path.to_path_buf(), setting.globals);
        if let Some(found) = self.cache.get(&key) {
            return found.clone();
        }
        if depth > MAX_DEPTH {
            self.deep_cuts += 1;
            return HashMap::new();
        }
        if !self.active.insert(path.to_path_buf()) {
            return HashMap::new();
        }
        let cuts = self.deep_cuts;
        let confined_before = crate::lang::confinement_refusals();
        let refused_before = crate::lang::manifest_refusals();
        let unread_before = UNREAD_IMPORTS.with(std::cell::Cell::get);
        let mut unparsed = false;
        let exports = self
            .sources
            .read(path)
            .and_then(|source| match crate::lang::parse(path, &source) {
                Ok(parsed) => Some(module::analyse(path, &source, &parsed, setting, self, depth).1),
                Err(_) => {
                    unparsed = true;
                    None
                }
            })
            .unwrap_or_default();
        self.active.remove(path);
        // Each importer records the refusal itself, so the module is read again, not cached.
        if unparsed {
            UNREAD_IMPORTS.with(|count| count.set(count.get() + 1));
            return exports;
        }
        // A module whose analysis saw a confinement refusal, a refused package manifest or a module
        // it could not parse is not cached, as with a cut: the next file re-reads it and records
        // its own refusal instead of inheriting silence.
        if self.deep_cuts == cuts
            && crate::lang::confinement_refusals() == confined_before
            && crate::lang::manifest_refusals() == refused_before
            && UNREAD_IMPORTS.with(std::cell::Cell::get) == unread_before
        {
            self.cache.insert(key, exports.clone());
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
    module::analyse(path, source, parsed, setting, modules, 0).0
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
            active,
            deep_cuts,
        } = self;
        cache
            .heap_bytes()
            .saturating_add(active.heap_bytes())
            .saturating_add(deep_cuts.heap_bytes())
    }
}
