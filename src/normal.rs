//! Normal forms: a syntax tree turned into text that is equal exactly when two pieces of code are
//! claimed to behave alike.
//!
//! Each normalization merges spellings that run identically (`docs` of the rule it serves record
//! the runtime justification) and keeps apart those that do not. An identifier declared outside the
//! node being normalized is named by its declaration, never evaluated: see [`Normalizer::external`].
//! A test's key and its context are assembled from the pieces here.

mod form;
mod identity;
pub(crate) mod row_identity;
mod rows;
mod suites;
pub(crate) mod titles;

pub use rows::{Modifiers, Row};

use crate::framework::{Dialect, HookTiming};
use crate::inventory::Inventory;
use crate::lang::{Declaration, Name, NodeId, Parsed, Scopes, Span, SymbolId, Tree};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

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
    /// The structure an inventory found: which callbacks are suite bodies and which run later. A
    /// callback passed by name is the local function it names.
    pub fn of(parsed: &Parsed, inventory: &Inventory) -> Self {
        let tree = &parsed.tree;
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
            if matches!(site.kind, SiteKind::Wrapper(_)) {
                continue;
            }
            let titled = !matches!(site.kind, SiteKind::Registers(Registers::Hook(_)));
            let inline = callback(tree, &parsed.scopes, site.node, titled)
                .filter(|callback| callback.inline)
                .and_then(|callback| callback.function);
            // A function passed by name may be any argument (a timeout can be a binding too): each
            // local function an argument names counts as code that runs at test time, so what it
            // writes is seen. It is never a suite body, and a named hook never a reset.
            let named = || {
                tree.list(site.node, "arguments")
                    .into_iter()
                    .flatten()
                    .skip(usize::from(titled))
                    .filter_map(|argument| local_function(tree, &parsed.scopes, argument))
                    .collect::<Vec<_>>()
            };
            let Some(callback) = inline else {
                // A named suite body runs while the file registers, so it stays registration time.
                if site.kind == SiteKind::Registers(Registers::Suite) {
                    continue;
                }
                for function in named() {
                    structure.run_time.push(tree.span(function));
                    match site.kind {
                        SiteKind::Registers(Registers::Hook(timing)) => {
                            structure.hooks.push(HookBody {
                                callback: function,
                                span: tree.span(function),
                                timing,
                                scope: None,
                                unconditional: false,
                            })
                        }
                        _ => structure.tests.push(tree.span(function)),
                    }
                }
                continue;
            };
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
                SiteKind::Wrapper(_) => {}
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
pub struct Walk {
    /// The span of the node being written: what separates its own declarations, numbered by
    /// appearance, from outside bindings named by what declares them.
    pub(crate) root: Span,
    /// The root's own declarations, numbered in order of appearance.
    locals: HashMap<SymbolId, usize>,
    /// Whether the root reads a function's or class's name, which renaming would change.
    names_observed: bool,
    /// Private names each enclosing class body declares, innermost last.
    privates: Vec<Vec<String>>,
    /// Bindings replaced by the normal form of what they are bound to (a table row's cells; the
    /// trace's member-access substitutions).
    pub(crate) substitutions: HashMap<SymbolId, String>,
    /// Outside bindings that each hold one object, numbered in order of appearance. The trace's
    /// policy turns the numbering off: its events render one at a time, and an ordinal taken at
    /// first render would make two tests whose setups run in opposite orders key apart.
    objects: HashMap<SymbolId, usize>,
    /// Whether the root's parameters are all bound, so it reads as a callback that takes none.
    bound_parameters: bool,
    /// How many type-level nodes enclose the current one.
    in_type: usize,
    /// The trace's naming policy: an outside binding that holds one object is named by its
    /// declaration alone, without this walk's ordinal.
    pub(crate) trace: bool,
    /// A literal renders as its kind, not its value (an assertion subject's shape).
    pub(crate) structural: bool,
    /// Text appended to a symbol's rendering wherever it appears (a destructured binding's
    /// property path). A suffix can only tell two events apart, never make them equal.
    pub(crate) suffixes: HashMap<SymbolId, String>,
    /// Locals whose one read stands for their own initializer (the trace's single-use inlining):
    /// the read renders the initializer's form, at the position the read occupies, with the
    /// numbering the walk has reached by then.
    pub(crate) inline: HashMap<SymbolId, NodeId>,
    /// Local classes the trace renders by their content wherever a read names them, so two
    /// constructions over different classes never share a shape.
    pub(crate) classes: HashMap<SymbolId, NodeId>,
}

impl Walk {
    /// A walk over `root`: its own declarations numbered by appearance, outside bindings named by
    /// what declares them. `trace` selects the trace's two naming policies; the other policy
    /// fields start off.
    fn new(
        root: Span,
        names_observed: bool,
        substitutions: HashMap<SymbolId, String>,
        bound_parameters: bool,
        trace: bool,
    ) -> Self {
        Self {
            root,
            locals: HashMap::new(),
            names_observed,
            privates: Vec::new(),
            substitutions,
            objects: HashMap::new(),
            bound_parameters,
            in_type: 0,
            trace,
            structural: false,
            suffixes: HashMap::new(),
            inline: HashMap::new(),
            classes: HashMap::new(),
        }
    }
}

/// The callback a registration at `site` passes. After the title when `titled` (a hook has none):
/// the last inline function argument, else the last argument that is not an options object, a
/// literal (a timeout), a template, an array or a spread. `None` when it passes none.
pub fn callback(tree: &Tree, scopes: &Scopes, site: NodeId, titled: bool) -> Option<Callback> {
    let arguments: Vec<NodeId> = tree
        .list(site, "arguments")
        .into_iter()
        .flatten()
        .skip(usize::from(titled))
        .collect();
    if let Some(&argument) = arguments
        .iter()
        .rev()
        .find(|&&argument| tree.is_function(tree.inner(argument)))
    {
        return Some(Callback {
            argument,
            function: Some(tree.inner(argument)),
            inline: true,
        });
    }
    let argument = arguments.iter().rev().copied().find(|&argument| {
        !matches!(
            tree.kind(tree.inner(argument)),
            "ObjectExpression"
                | "ArrayExpression"
                | "Literal"
                | "TemplateLiteral"
                | "SpreadElement"
        )
    })?;
    Some(Callback {
        argument,
        function: local_function(tree, scopes, argument),
        inline: false,
    })
}

/// The function a reference to a local binding names: a function declaration, or a `const` bound
/// to a function expression or arrow.
fn local_function(tree: &Tree, scopes: &Scopes, reference: NodeId) -> Option<NodeId> {
    let reference = tree.inner(reference);
    if tree.kind(reference) != "Identifier" {
        return None;
    }
    let symbol = scopes.symbol_at(tree.span(reference).start)?;
    match scopes.symbol(symbol).declaration {
        Declaration::Function(function) => Some(function),
        Declaration::Variable(declarator)
            if tree
                .parent(declarator)
                .is_some_and(|declaration| tree.text(declaration, "kind") == Some("const")) =>
        {
            tree.child(declarator, "init")
                .map(|init| tree.inner(init))
                .filter(|&init| tree.is_function(init))
        }
        _ => None,
    }
}

/// The callback a registration passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Callback {
    /// The argument.
    pub argument: NodeId,
    /// The function that runs: the inline function, or the local one the argument names; `None`
    /// when the argument names something declared elsewhere.
    pub function: Option<NodeId>,
    /// Whether the function is written in the argument itself.
    pub inline: bool,
}

/// Whether a body's arrow and `function` forms act on the same object, and which was written when
/// they do not. A behaviour-keyed rule agrees on this before it compares events at all: it is a
/// property of the function enclosing what the test does, not something the test does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CallbackForm {
    /// Neither form reads the function's own `this`, `arguments`, `super` or `new.target`, and it
    /// is not a generator: the two spellings run the same code.
    Interchangeable,
    /// An arrow whose body reads what only a `function` binds: the two forms differ.
    Arrow,
    /// A `function` (or a generator) whose body reads what only a `function` binds.
    Function,
}

impl CallbackForm {
    /// The token the comparison keys spell.
    pub fn token(self) -> &'static str {
        match self {
            Self::Interchangeable => "same",
            Self::Arrow => "arrow",
            Self::Function => "function",
        }
    }
}

/// Which form was written, before the body's own reads decide whether it matters.
#[derive(Clone, Copy, Debug)]
enum Written {
    Arrow,
    Function,
}

/// Normalizes the nodes of one module.
pub struct Normalizer<'s> {
    path: &'s Path,
    tree: &'s Tree,
    scopes: &'s Scopes,
    setting: Setting<'s>,
    structure: &'s Structure,
    externals: HashMap<SymbolId, String>,
    /// While a declaration's dependencies are listed, the outside bindings its walks read.
    discovering: Option<Vec<SymbolId>>,
    /// Whether identities are being resolved, so every read must already be named.
    resolving: bool,
    /// Whether an import is named by where it resolves rather than by what it declares: so in a
    /// module read for an export, which is not followed further.
    shallow_imports: bool,
    /// How many nodes deep the form being written is.
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
            discovering: None,
            resolving: false,
            shallow_imports: false,
            nesting: 0,
        }
    }

    /// The normal form of `node`.
    pub fn of(&mut self, node: NodeId) -> String {
        self.substituted(node, HashMap::new(), false)
    }

    /// [`Self::of`] with some bindings replaced by the normal form of what they are bound to, and,
    /// when `bound_parameters`, the root's parameters erased because all of them are.
    ///
    /// The outside bindings the node reads are named first, each from its own declaration: the
    /// node's walk then only looks them up, so the stack never holds two walks.
    pub fn substituted(
        &mut self,
        node: NodeId,
        substitutions: HashMap<SymbolId, String>,
        bound_parameters: bool,
    ) -> String {
        if !self.resolving && self.discovering.is_none() {
            self.discovering = Some(Vec::new());
            let _ = self.walk(node, substitutions.clone(), bound_parameters);
            for read in self.discovering.take().unwrap_or_default() {
                let _ = self.external(read);
            }
        }
        self.walk(node, substitutions, bound_parameters)
    }

    /// A walk a caller drives and shares across renders, naming by the policy the trace reads:
    /// locals numbered at first appearance in what the caller renders, outside objects by their
    /// declaration. `node` is the root whose own declarations the walk numbers (the callback's
    /// function node, for a trace) and whose name observations it reads.
    pub(crate) fn body_walk(
        &self,
        node: NodeId,
        substitutions: HashMap<SymbolId, String>,
        bound_parameters: bool,
    ) -> Walk {
        Walk::new(
            self.tree.span(node),
            form::observes_names(self.tree, node, self.setting.dialect),
            substitutions,
            bound_parameters,
            true,
        )
    }

    /// The normal form of `node`, written in a walk the caller drives and shares across calls, so
    /// every name the caller renders comes from one numbering. Each call is its own depth count:
    /// past the writer's depth bound the position token keeps a deeper node able to stop a match
    /// but never to make one.
    pub fn form_of(&mut self, node: NodeId, walk: &mut Walk) -> String {
        let mut out = String::new();
        let saved = std::mem::replace(&mut self.nesting, 0);
        self.write_node(node, walk, &mut out);
        self.nesting = saved;
        out
    }

    /// The flat form of a member read, spelled the way the writer spells one, for a substitution
    /// the tree does not hold: a destructured read standing for `object.property` renders exactly
    /// as the member access written out would.
    pub(crate) fn member_form(&mut self, object_form: &str, property: &str) -> String {
        format!(
            "(MemberExpression computed:false object:{object_form} optional:false \
             property:#{property})"
        )
    }

    /// Whether a callback's arrow and `function` forms mean the same thing here, and which one was
    /// written. An arrow does not bind `this`/`arguments` and a `function` does, so a body that
    /// reads either acts on a different object in the two forms; a generator could be constructed
    /// or have its `prototype` read, which an arrow cannot.
    pub fn callback_form(&self, function: NodeId) -> CallbackForm {
        let tree = self.tree;
        let written = match tree.kind(function) {
            "ArrowFunctionExpression" => Written::Arrow,
            _ => Written::Function,
        };
        let interchangeable = tree
            .child(function, "body")
            .is_some_and(|body| !form::binds_this(tree, body))
            && !tree.flag(function, "generator");
        match interchangeable {
            true => CallbackForm::Interchangeable,
            false => match written {
                Written::Arrow => CallbackForm::Arrow,
                Written::Function => CallbackForm::Function,
            },
        }
    }

    /// Whether a body reads the function's own `this`, `arguments`, `super` or `new.target`.
    pub fn binds_this(&self, body: NodeId) -> bool {
        form::binds_this(self.tree, body)
    }

    /// The dialect every form reads its framework knowledge from.
    pub fn dialect(&self) -> &'s Dialect {
        self.setting.dialect
    }

    /// Writes `node`'s normal form in a walk of its own.
    fn walk(
        &mut self,
        node: NodeId,
        substitutions: HashMap<SymbolId, String>,
        bound_parameters: bool,
    ) -> String {
        let mut walk = Walk::new(
            self.tree.span(node),
            form::observes_names(self.tree, node, self.setting.dialect),
            substitutions,
            bound_parameters,
            false,
        );
        let mut out = String::new();
        self.write_node(node, &mut walk, &mut out);
        out
    }

    /// The callback the test registered at `site` passes; see [`callback`].
    pub fn callback(&self, site: NodeId) -> Option<Callback> {
        callback(self.tree, self.scopes, site, true)
    }

    /// Whether evaluating `node` may create or change an object: a call, `new`, `await` or an
    /// assignment outside a nested function, a registration function's own call excepted.
    pub fn creates(&self, node: NodeId) -> bool {
        form::creates(self, node)
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
