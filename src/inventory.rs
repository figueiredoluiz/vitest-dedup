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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Export {
    /// The API's namespace, which nothing replaces.
    Namespace,
    /// A function or value that registers when called.
    Wrapper(Reach),
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

/// What one module registers.
#[derive(Clone, Debug, Default)]
pub struct Inventory {
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

/// The export name standing for "registers while the module loads".
const ON_LOAD: &str = "\0load";
/// How many imports deep a module's exports are followed.
const MAX_DEPTH: usize = 8;

/// Export names whose value registers when called, per project module and globals setting (a
/// project with globals reads free names as the API), shared by every file of one run.
#[derive(Debug, Default)]
pub struct Modules {
    cache: HashMap<(PathBuf, bool), HashMap<String, Export>>,
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
        let exports = crate::discovery::read_bounded(path, crate::discovery::MAX_SOURCE_FILE_BYTES)
            .ok()
            .flatten()
            .and_then(|source| {
                let parsed = crate::lang::parse(path, &source).ok()?;
                Some(module::analyse(path, &source, &parsed, setting, self, depth).1)
            })
            .unwrap_or_default();
        self.active.remove(path);
        if self.deep_cuts == cuts {
            self.cache.insert(key, exports.clone());
        }
        exports
    }
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
