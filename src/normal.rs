//! Normal forms: a syntax tree turned into text that is equal exactly when two pieces of code are
//! claimed to behave alike.
//!
//! Each normalization merges spellings that run identically (`docs` of the rule it serves record
//! the runtime justification) and keeps apart those that do not. An identifier declared outside the
//! node being normalized is named by its declaration, never evaluated: see [`Normalizer::external`].
//! A test's key and its context are assembled from the pieces here.

mod form;
mod identity;
mod rows;

pub use rows::{Modifiers, Row};

use crate::framework::{Dialect, HookTiming};
use crate::inventory::Inventory;
use crate::lang::{Name, NodeId, Parsed, Scopes, Span, SymbolId, Tree};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

/// How deep an external declaration's initializer is followed before it is named by identity.
const MAX_NESTING: usize = 6;

/// A 64-bit digest of `text`, for keys compared within one run.
pub fn digest(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Identities of what project modules export, shared by every file of one run.
#[derive(Debug, Default)]
pub struct Exports {
    known: RefCell<HashMap<(PathBuf, String), Option<String>>>,
}

/// What a normalization reads besides the module.
#[derive(Clone, Copy, Debug)]
pub struct Setting<'a> {
    /// The framework's API.
    pub dialect: &'a Dialect,
    /// The analysis root: imports resolving outside it are named by their spelling.
    pub root: &'a Path,
    /// Export identities shared across the run.
    pub exports: &'a Exports,
}

/// A hook's callback, with when it runs and where it applies.
#[derive(Clone, Copy, Debug)]
pub struct HookBody {
    /// The callback.
    pub callback: NodeId,
    /// The callback's span.
    pub span: Span,
    /// When it runs.
    pub timing: HookTiming,
    /// The suite callback it is registered in, `None` at module level.
    pub scope: Option<Span>,
    /// Whether it is registered whenever its file or suite runs: a statement directly in the
    /// program or the suite's body, not under a condition or inside another function.
    pub unconditional: bool,
}

/// What registration-time structure the normal form of a module depends on.
#[derive(Clone, Debug, Default)]
pub struct Structure {
    /// Suite callbacks, by the registering call that receives each.
    pub suites: HashMap<NodeId, NodeId>,
    /// Callbacks that run under the context: test, bench and hook bodies.
    pub run_time: Vec<Span>,
    /// Test and bench bodies.
    pub tests: Vec<Span>,
    /// Hook bodies.
    pub hooks: Vec<HookBody>,
    /// Nodes whose value is a registration function.
    pub registration_values: HashSet<NodeId>,
}

impl Structure {
    /// The structure an inventory found: which callbacks are suite bodies and which run later.
    pub fn of(tree: &Tree, inventory: &Inventory) -> Self {
        use crate::framework::Registers;
        use crate::inventory::SiteKind;
        let mut structure = Structure {
            registration_values: inventory.registration_values.clone(),
            ..Structure::default()
        };
        let suite_calls: HashSet<NodeId> = inventory
            .sites
            .iter()
            .filter(|site| site.kind == SiteKind::Registers(Registers::Suite))
            .map(|site| site.node)
            .collect();
        for site in &inventory.sites {
            for argument in tree.list(site.node, "arguments").into_iter().flatten() {
                let callback = tree.inner(argument);
                if !matches!(
                    tree.kind(callback),
                    "ArrowFunctionExpression" | "FunctionExpression"
                ) {
                    continue;
                }
                match site.kind {
                    SiteKind::Registers(Registers::Suite) => {
                        structure.suites.insert(callback, site.node);
                    }
                    SiteKind::Registers(Registers::Hook(timing)) => {
                        structure.run_time.push(tree.span(callback));
                        // The nearest function around the hook: a suite's callback, or another
                        // function, which registers it only for whoever calls it.
                        let around = tree
                            .ancestors(site.node)
                            .find(|&ancestor| tree.is_function(ancestor));
                        let suite = around.filter(|&function| {
                            tree.outer_parent(function)
                                .is_some_and(|call| suite_calls.contains(&call))
                        });
                        // Registered whenever the file or its suite runs: a statement directly in
                        // the program or in the suite's own body, not under a condition or loop.
                        let body = match around {
                            Some(function) => tree.child(function, "body"),
                            None => Some(tree.root()),
                        };
                        let unconditional = (around.is_none() || suite.is_some())
                            && tree
                                .outer_parent(site.node)
                                .filter(|statement| tree.kind(*statement) == "ExpressionStatement")
                                .and_then(|statement| tree.parent(statement))
                                == body;
                        structure.hooks.push(HookBody {
                            callback,
                            span: tree.span(callback),
                            timing,
                            scope: suite.map(|suite| tree.span(suite)),
                            unconditional,
                        });
                    }
                    SiteKind::Registers(_) => {
                        structure.run_time.push(tree.span(callback));
                        structure.tests.push(tree.span(callback));
                    }
                    SiteKind::Wrapper => {}
                }
            }
        }
        structure
    }

    /// Whether `span` lies in a callback that runs under the context.
    pub fn at_run_time(&self, span: Span) -> bool {
        self.run_time.iter().any(|body| body.contains(span))
    }
}

/// State of one normalization.
struct Walk {
    root: Span,
    depth: usize,
    /// The root's own declarations, numbered in order of appearance.
    locals: HashMap<SymbolId, usize>,
    /// Whether the root reads a function's or class's name, which renaming would change.
    names_observed: bool,
    /// Private names each enclosing class body declares, innermost last.
    privates: Vec<Vec<String>>,
    /// Bindings replaced by the normal form of what they are bound to (a table row's cells).
    substitutions: HashMap<SymbolId, String>,
    /// Outside bindings that each hold one object, numbered in order of appearance.
    objects: HashMap<SymbolId, usize>,
    /// Whether the root's parameters are all bound, so it reads as a callback that takes none.
    bound_parameters: bool,
    /// How many type-level nodes enclose the current one.
    in_type: usize,
}

/// Normalizes the nodes of one module.
pub struct Normalizer<'s> {
    path: &'s Path,
    tree: &'s Tree,
    scopes: &'s Scopes,
    setting: Setting<'s>,
    structure: &'s Structure,
    externals: HashMap<SymbolId, String>,
    active: HashSet<SymbolId>,
    /// How many nodes deep the form being written is, across nested walks.
    nesting: usize,
}

impl<'s> Normalizer<'s> {
    /// A normalizer for the module at `path`.
    pub fn new(
        path: &'s Path,
        parsed: &'s Parsed,
        setting: Setting<'s>,
        structure: &'s Structure,
    ) -> Self {
        Self {
            path,
            tree: &parsed.tree,
            scopes: &parsed.scopes,
            setting,
            structure,
            externals: HashMap::new(),
            active: HashSet::new(),
            nesting: 0,
        }
    }

    /// The normal form of `node`.
    pub fn of(&mut self, node: NodeId) -> String {
        self.at_depth(node, 0)
    }

    /// [`Self::of`], `depth` external declarations deep.
    fn at_depth(&mut self, node: NodeId, depth: usize) -> String {
        self.substituted(node, depth, HashMap::new(), false)
    }

    /// [`Self::of`] with some bindings replaced by the normal form of what they are bound to, and,
    /// when `bound_parameters`, the root's parameters erased because all of them are.
    pub fn substituted(
        &mut self,
        node: NodeId,
        depth: usize,
        substitutions: HashMap<SymbolId, String>,
        bound_parameters: bool,
    ) -> String {
        let mut walk = Walk {
            root: self.tree.span(node),
            depth,
            locals: HashMap::new(),
            names_observed: form::observes_names(self.tree, node, self.setting.dialect),
            privates: Vec::new(),
            substitutions,
            objects: HashMap::new(),
            bound_parameters,
            in_type: 0,
        };
        let mut out = String::new();
        self.write_node(node, &mut walk, &mut out);
        out
    }

    /// The suite callbacks enclosing `node`, outermost first, or `None` when a function that is
    /// neither a suite body nor an iteration callback encloses it: what it registers then depends
    /// on a caller.
    pub fn suites_around(&self, node: NodeId) -> Option<Vec<NodeId>> {
        let tree = self.tree;
        let mut chain = Vec::new();
        for ancestor in tree.ancestors(node) {
            if !tree.is_function(ancestor) {
                continue;
            }
            if self.structure.suites.contains_key(&ancestor) {
                chain.push(ancestor);
                continue;
            }
            // `cases.forEach((c) => it(...))` runs while the suite registers.
            let iterates = tree.outer_parent(ancestor).is_some_and(|call| {
                tree.kind(call) == "CallExpression"
                    && tree.child(call, "callee").is_some_and(|callee| {
                        let callee = tree.inner(callee);
                        tree.kind(callee) == "MemberExpression"
                            && !tree.flag(callee, "computed")
                            && matches!(
                                tree.child(callee, "property")
                                    .and_then(|p| tree.text(p, "name")),
                                Some("forEach" | "map")
                            )
                    })
            });
            if !iterates {
                return None;
            }
        }
        chain.reverse();
        Some(chain)
    }

    /// The statement directly in `scope`'s body (or the program's, for `None`) that holds `node`.
    pub fn statement_in(&self, node: NodeId, scope: Option<NodeId>) -> Option<NodeId> {
        let tree = self.tree;
        let mut current = node;
        for ancestor in tree.ancestors(node) {
            let top = match scope {
                None => tree.kind(ancestor) == "Program",
                Some(scope) => {
                    ancestor == scope
                        || (tree.kind(ancestor) == "BlockStatement"
                            && tree.parent(ancestor) == Some(scope))
                }
            };
            if top {
                return Some(current);
            }
            current = ancestor;
        }
        None
    }

    /// Whether a test whose body is `body` reads state another test writes, or a hook writes in a
    /// way that depends on what it held before: equal text then observes different values
    /// depending on which tests ran first. A binding every test starts from the same value of is
    /// not such state: one the test assigns before reading it, or one a before-each hook around
    /// the test assigns. Writes through a method call or a property (`list.push`) are not seen.
    pub fn order_dependent(&self, body: NodeId) -> bool {
        let tree = self.tree;
        let own = tree.span(body);
        // Only what the body reads: every identifier in it, resolved once.
        let read: BTreeSet<SymbolId> = tree
            .descendants(body)
            .filter(|&node| tree.kind(node) == "Identifier")
            .filter_map(|node| match self.scopes.name_at(tree.span(node).start) {
                Some(Name::Reference(Some(symbol))) => Some(symbol),
                _ => None,
            })
            .collect();
        read.into_iter().any(|id| {
            let symbol = self.scopes.symbol(id);
            if own.contains(symbol.span)
                || self.assigned_first(id, body)
                || self.reset_before_each(id, own)
            {
                return false;
            }
            symbol.references.iter().any(|reference| {
                if !reference.write || own.contains(reference.span) {
                    return false;
                }
                if self
                    .structure
                    .tests
                    .iter()
                    .any(|test| test.contains(reference.span))
                {
                    return true;
                }
                self.structure
                    .hooks
                    .iter()
                    .any(|hook| hook.span.contains(reference.span))
                    && reference.node.is_some_and(|node| self.accumulates(node))
            })
        })
    }

    /// Whether `symbol`'s first reference in the function `body` is a plain assignment, directly in
    /// its body, of a value that does not read it.
    fn assigned_first(&self, symbol: SymbolId, body: NodeId) -> bool {
        let tree = self.tree;
        let own = tree.span(body);
        let Some(first) = self
            .scopes
            .symbol(symbol)
            .references
            .iter()
            .filter(|reference| own.contains(reference.span))
            .min_by_key(|reference| reference.span.start)
        else {
            return false;
        };
        first
            .node
            .filter(|_| first.write)
            .is_some_and(|node| self.plain_assignment(node) && self.runs_directly(node, body))
    }

    /// Whether the assignment to `target` is a statement of `function`'s own body, or its concise
    /// body, so it runs whenever the function does unless a `return` or `throw` before it leaves.
    fn runs_directly(&self, target: NodeId, function: NodeId) -> bool {
        let tree = self.tree;
        let Some(assignment) = tree.outer_parent(target) else {
            return false;
        };
        let body = tree.child(function, "body");
        let direct = body.map(|body| tree.inner(body)) == Some(assignment)
            || tree
                .outer_parent(assignment)
                .filter(|statement| tree.kind(*statement) == "ExpressionStatement")
                .and_then(|statement| tree.parent(statement))
                == body;
        let start = tree.span(assignment).start;
        let leaves_before = tree
            .descendants(function)
            .take_while(|&node| tree.span(node).start < start)
            .any(|node| {
                matches!(tree.kind(node), "ReturnStatement" | "ThrowStatement")
                    && tree
                        .ancestors(node)
                        .find(|&ancestor| tree.is_function(ancestor))
                        == Some(function)
            });
        direct && !leaves_before
    }

    /// Whether a before-each hook registered around the test whose body spans `test` assigns
    /// `symbol` a value that does not read it.
    fn reset_before_each(&self, symbol: SymbolId, test: Span) -> bool {
        self.scopes
            .symbol(symbol)
            .references
            .iter()
            .any(|reference| {
                reference.write
                    && self.structure.hooks.iter().any(|hook| {
                        matches!(hook.timing, HookTiming::BeforeEach | HookTiming::AroundEach)
                            && hook.unconditional
                            && hook.span.contains(reference.span)
                            && reference.node.is_some_and(|node| {
                                self.plain_assignment(node)
                                    && self.runs_directly(node, hook.callback)
                            })
                            && hook.scope.is_none_or(|scope| scope.contains(test))
                    })
            })
    }

    /// Whether the write at identifier `node` is `node = value` with a value that does not read
    /// the binding.
    fn plain_assignment(&self, node: NodeId) -> bool {
        let tree = self.tree;
        tree.outer_parent(node).is_some_and(|parent| {
            tree.kind(parent) == "AssignmentExpression"
                && tree.child(parent, "left") == Some(node)
                && !self.accumulates(node)
        })
    }

    /// Whether the write at identifier `node` depends on the value it replaces: an update, a
    /// compound assignment, or an assignment whose value reads the same binding while it is
    /// evaluated (a read inside a function the value holds runs later, on the new value).
    fn accumulates(&self, node: NodeId) -> bool {
        let tree = self.tree;
        let Some(parent) = tree.outer_parent(node) else {
            return false;
        };
        match tree.kind(parent) {
            "UpdateExpression" => true,
            "AssignmentExpression" => {
                if tree.text(parent, "operator") != Some("=") {
                    return true;
                }
                let symbol = self.scopes.symbol_at(tree.span(node).start);
                let Some(right) = tree.child(parent, "right") else {
                    return false;
                };
                let deferred = |reference: NodeId| {
                    tree.ancestors(reference)
                        .take_while(|&ancestor| ancestor != right)
                        .any(|ancestor| tree.is_function(ancestor))
                        || tree.is_function(right)
                };
                symbol.is_some_and(|symbol| {
                    self.scopes
                        .symbol(symbol)
                        .references
                        .iter()
                        .any(|reference| {
                            tree.span(right).contains(reference.span)
                                && reference.node.is_some_and(|node| !deferred(node))
                        })
                })
            }
            _ => false,
        }
    }

    /// The statements of a suite callback's body, empty for a concise body.
    pub fn body_statements(&self, function: NodeId) -> Vec<NodeId> {
        let tree = self.tree;
        tree.child(function, "body")
            .filter(|body| tree.kind(*body) == "BlockStatement")
            .map(|body| tree.list(body, "body").into_iter().flatten().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
