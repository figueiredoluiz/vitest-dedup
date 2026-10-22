//! Proof that running some code provably registers no test, hook or mock.
//!
//! A helper's body is only ever read when it declares tests in a shape the extractor models.
//! Many helpers declare nothing: they build mock handlers, plugin objects, fixtures or fill a
//! registry. A test or hook can only be registered by code that can reach Vitest's registration
//! API, so code whose reachable part never names that API cannot do it. The proof is a lexical
//! reachability walk, and anything it cannot place is "cannot tell":
//!
//! - **What runs.** Importing a module runs its top-level initialisers (never the bodies of the
//!   functions it merely declares). Calling an export runs that export. Both start a walk; every
//!   binding the walk reads joins it -- a local declaration, an import (its module and export),
//!   a global.
//! - **What is refused.** A reference that names a registration API, or a way to reach one by name (`globalThis`, `eval`, `require`, a computed
//!   `import()`, `import.meta.glob`); a top-level statement that is not a declaration; an import
//!   that does not resolve; a free name the host does not define and the project's generated
//!   auto-import list does not name.
//! - **Packages** are judged by their manifest closure
//!   ([`crate::frontend::package_manifests`]), Node builtins, Nuxt virtual modules and assets are
//!   harmless, and a single-file component's `<script setup>` runs at mount, not import.
//!
//! Initialisers at the top level are treated as value creation. A module that mutates state from
//! one (`const x = (globalThis.y = 1)`) is outside what this proves; `globalThis` is refused for
//! that reason.

use crate::frontend::auto_imports;
use crate::frontend::helpers;
use crate::frontend::resolver::{self, Resolved};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, Class, Declaration, ExportDefaultDeclarationKind, Expression, Function,
    IdentifierReference, ImportDeclarationSpecifier, ImportExpression, ImportOrExportKind, Program,
    Statement, StaticMemberExpression, TSEnumDeclaration, TSNamespaceDeclaration,
    TSNamespaceDeclarationBody, VariableDeclaration, VariableDeclarator,
};
use oxc_ast::AstKind;
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::{Semantic, SemanticBuilder, SymbolId};
use oxc_span::SourceType;
use oxc_syntax::scope::ScopeFlags;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Most modules one proof may visit before it refuses.
const MAX_MODULES: usize = 64;

/// Vitest names that register a test, a suite or a hook.
const REGISTRATION: &[&str] = &[
    "test",
    "it",
    "describe",
    "suite",
    "bench",
    "beforeAll",
    "beforeEach",
    "afterAll",
    "afterEach",
    "onTestFinished",
    "onTestFailed",
    "aroundEach",
    "aroundAll",
];

/// Free names that reach code by name or load code by a computed route.
const REFLECTIVE: &[&str] = &[
    "globalThis",
    "global",
    "window",
    "self",
    "eval",
    "Function",
    "require",
];

/// Framework runtime modules whose exports are known to mount, mock or register endpoints and
/// never register a Vitest test or hook. Their packages declare Vitest as a peer because other
/// entry points (`@nuxt/test-utils/e2e`) register hooks, so the manifest closure alone would
/// refuse them. Judged by the specifier as written, per export.
const KNOWN_RUNTIME: &[(&str, &[&str])] = &[(
    "@nuxt/test-utils/runtime",
    &[
        "mountSuspended",
        "renderSuspended",
        "registerEndpoint",
        "mockNuxtImport",
        "mockComponent",
    ],
)];

/// Whether `specifier` names a framework runtime module and `export` is one of the exports
/// known to register nothing.
pub(super) fn is_known_runtime_export(specifier: &str, export: &str) -> bool {
    KNOWN_RUNTIME
        .iter()
        .any(|(module, exports)| *module == specifier && exports.contains(&export))
}

/// Whether importing `specifier` runs only framework runtime code known to register nothing.
pub(super) fn is_known_runtime_module(specifier: &str) -> bool {
    KNOWN_RUNTIME.iter().any(|(module, _)| *module == specifier)
}

/// Node core modules that compile and run code from text, or load it by a computed route.
const CODE_RUNNING_BUILTINS: &[&str] = &[
    "vm",
    "child_process",
    "worker_threads",
    "module",
    "repl",
    "cluster",
    "inspector",
];

/// Nuxt's virtual module prefixes: the framework runtime, never a test registrar.
const NUXT_VIRTUAL: &[&str] = &["#app", "#internal", "#head", "#vue-router"];

/// Globals the language and the host define: none of them registers a Vitest test. Everything
/// else that is not bound in the module is unknown unless the project's generated auto-import
/// list names it.
const BUILTINS: &[&str] = &[
    "undefined",
    "NaN",
    "Infinity",
    "arguments",
    "Object",
    "Array",
    "String",
    "Number",
    "Boolean",
    "Symbol",
    "BigInt",
    "Math",
    "JSON",
    "Date",
    "RegExp",
    "Error",
    "TypeError",
    "RangeError",
    "SyntaxError",
    "ReferenceError",
    "EvalError",
    "URIError",
    "AggregateError",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "WeakRef",
    "Promise",
    "Proxy",
    "Reflect",
    "Intl",
    "ArrayBuffer",
    "SharedArrayBuffer",
    "DataView",
    "Atomics",
    "Int8Array",
    "Uint8Array",
    "Uint8ClampedArray",
    "Int16Array",
    "Uint16Array",
    "Int32Array",
    "Uint32Array",
    "Float32Array",
    "Float64Array",
    "BigInt64Array",
    "BigUint64Array",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "encodeURIComponent",
    "decodeURIComponent",
    "encodeURI",
    "decodeURI",
    "console",
    "setTimeout",
    "clearTimeout",
    "setInterval",
    "clearInterval",
    "setImmediate",
    "clearImmediate",
    "queueMicrotask",
    "structuredClone",
    "URL",
    "URLSearchParams",
    "TextEncoder",
    "TextDecoder",
    "AbortController",
    "AbortSignal",
    "fetch",
    "Headers",
    "Request",
    "Response",
    "FormData",
    "Blob",
    "File",
    "FileReader",
    "atob",
    "btoa",
    "performance",
    "crypto",
    "process",
    "Buffer",
    "document",
    "navigator",
    "location",
    "localStorage",
    "sessionStorage",
    "history",
    "Event",
    "CustomEvent",
    "EventTarget",
    "KeyboardEvent",
    "MouseEvent",
    "HTMLElement",
    "Element",
    "Node",
    "NodeList",
    "DOMParser",
    "MutationObserver",
    "ResizeObserver",
    "IntersectionObserver",
    "requestAnimationFrame",
    "cancelAnimationFrame",
    "getComputedStyle",
    "matchMedia",
    "WebSocket",
    "XMLHttpRequest",
    "Worker",
    "MessageChannel",
    "BroadcastChannel",
    "Notification",
    "Image",
    "Audio",
    "DOMException",
    "CSS",
    "Selection",
    "Range",
    "Storage",
    "SubtleCrypto",
];

/// A file that carries data or styling, never code that could register a test.
fn is_asset(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        matches!(
            extension.to_string_lossy().as_ref(),
            "css"
                | "scss"
                | "sass"
                | "less"
                | "styl"
                | "svg"
                | "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "avif"
                | "ico"
                | "woff"
                | "woff2"
                | "ttf"
                | "otf"
                | "json"
        )
    })
}

/// One script of a file: its text, whether it is TypeScript, and whether it is a `<script setup>`.
struct Block {
    text: String,
    source_type: SourceType,
    setup: bool,
}

/// The `<script>` blocks of a single-file component. `None` when one cannot be read with
/// certainty (an external `src`, a missing close tag).
fn script_blocks(source: &str) -> Option<Vec<Block>> {
    let mut blocks = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find("<script") {
        let after = &rest[start + "<script".len()..];
        let tag_end = after.find('>')?;
        let attributes = &after[..tag_end];
        // `<script-foo>` is some other element.
        if !attributes.is_empty() && !attributes.starts_with(char::is_whitespace) {
            rest = after;
            continue;
        }
        if attributes.contains("src=") {
            return None;
        }
        let body = &after[tag_end + 1..];
        let close = body.find("</script>")?;
        blocks.push(Block {
            text: body[..close].to_owned(),
            source_type: if attributes.contains("ts") {
                SourceType::ts()
            } else {
                SourceType::mjs()
            },
            setup: attributes.split_whitespace().any(|token| token == "setup"),
        });
        rest = &body[close + "</script>".len()..];
    }
    Some(blocks)
}

/// Declarations only: bare statements and control flow at module level are where state changes.
fn top_level_is_declarations(program: &Program) -> bool {
    program.body.iter().all(is_declaration)
}

/// Whether `statement` only declares: it cannot run anything as it is evaluated beyond its
/// initialisers.
fn is_declaration(statement: &Statement) -> bool {
    match statement {
        Statement::ImportDeclaration(_)
        | Statement::FunctionDeclaration(_)
        | Statement::ClassDeclaration(_)
        | Statement::VariableDeclaration(_)
        | Statement::TSTypeAliasDeclaration(_)
        | Statement::TSInterfaceDeclaration(_)
        | Statement::TSEnumDeclaration(_)
        | Statement::TSExternalModuleDeclaration(_)
        | Statement::TSGlobalDeclaration(_)
        | Statement::EmptyStatement(_)
        | Statement::ExportNamedDeclaration(_)
        | Statement::ExportFromDeclaration(_)
        | Statement::ExportAllDeclaration(_)
        | Statement::ExportDefaultDeclaration(_) => true,
        Statement::ExportDeclaration(export) => match &export.declaration {
            Declaration::TSNamespaceDeclaration(namespace) => namespace_is_declarations(namespace),
            Declaration::TSImportEqualsDeclaration(_) => false,
            _ => true,
        },
        Statement::TSNamespaceDeclaration(namespace) => namespace_is_declarations(namespace),
        // A `vi.mock(...)` or `expect.extend(...)` statement is framework setup, not program logic.
        Statement::ExpressionStatement(statement) => is_framework_setup(&statement.expression),
        _ => false,
    }
}

/// Whether `expression` is a call chain rooted at `vi` or `expect`.
fn is_framework_setup(expression: &Expression) -> bool {
    let mut cursor = expression;
    loop {
        match cursor {
            Expression::CallExpression(call) => cursor = &call.callee,
            Expression::StaticMemberExpression(member) => cursor = &member.object,
            Expression::Identifier(identifier) => {
                return matches!(identifier.name.as_str(), "vi" | "expect")
            }
            _ => return false,
        }
    }
}

/// Whether a namespace body only declares. It compiles to a function run at load, so a bare
/// statement inside it is as much a change of state as one at module level.
fn namespace_is_declarations(namespace: &TSNamespaceDeclaration) -> bool {
    if namespace.declare {
        return true;
    }
    match &namespace.body {
        TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => {
            namespace_is_declarations(inner)
        }
        TSNamespaceDeclarationBody::TSModuleBlock(block) => block.body.iter().all(is_declaration),
    }
}

/// What part of a module to prove.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Root {
    /// Importing the module: its top-level initialisers.
    ImportTime,
    /// Running one export.
    Export(String),
    /// Running one export of a module that may not have it (a `export *` target).
    ExportIfPresent(String),
    /// Following a re-export hop to the export it names. A hop that comes back to itself never
    /// reaches a declaration, which is not a recursion but a broken module graph.
    Reexport(String),
    /// Everything the module contains, when no narrower part can be named.
    Whole,
}

type Edge = (PathBuf, Root);

/// A bounded proof over modules reachable from a starting point.
#[derive(Default)]
struct Proof {
    visited: HashSet<Edge>,
    /// Edges being proved right now, to tell a cyclic re-export from a diamond.
    active: HashSet<Edge>,
    modules: Vec<PathBuf>,
}

impl Proof {
    fn prove(&mut self, path: &Path, root: Root) -> bool {
        let edge = (path.to_path_buf(), root.clone());
        if !self.visited.insert(edge.clone()) {
            return !(matches!(root, Root::Reexport(_)) && self.active.contains(&edge));
        }
        self.active.insert(edge.clone());
        let proved = self.prove_new(path, root);
        self.active.remove(&edge);
        proved
    }

    fn prove_new(&mut self, path: &Path, root: Root) -> bool {
        if helpers::is_dependency(path) {
            return crate::frontend::package_manifests::cannot_register_tests(
                path,
                globals_enabled(),
            );
        }
        if is_asset(path) {
            return true;
        }
        if !self.modules.iter().any(|seen| seen == path) {
            self.modules.push(path.to_path_buf());
            if self.modules.len() > MAX_MODULES {
                return false;
            }
        }
        let Some(edges) = scan_file(path, &root) else {
            return false;
        };
        edges
            .into_iter()
            .all(|(next, next_root)| self.prove(&next, next_root))
    }

    /// Proves every edge a scan produced; true when none is refused.
    fn prove_edges(&mut self, edges: Vec<Edge>) -> bool {
        edges
            .into_iter()
            .all(|(path, root)| self.prove(&path, root))
    }
}

thread_local! {
    /// Whether the project enables Vitest globals, for the file being extracted on this thread.
    static GLOBALS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };

    /// Finished proofs, by starting point. Only whole queries are kept: a part proved inside a
    /// cycle leans on its ancestors being proved, so it is no verdict on its own.
    static PROVED: RefCell<HashMap<Edge, Option<Vec<PathBuf>>>> = RefCell::new(HashMap::new());
}

/// Records whether the project enables Vitest globals for the extraction about to run on this
/// thread: under them an installed package can register tests without importing anything.
pub(super) fn begin_file(globals_enabled: bool) {
    GLOBALS.with(|globals| globals.set(globals_enabled));
}

/// Whether the project enables Vitest globals for the file being extracted on this thread.
pub(super) fn globals_enabled() -> bool {
    GLOBALS.with(std::cell::Cell::get)
}

/// Forgets every finished proof. Each extraction runs on a fresh thread and so starts empty;
/// this is for tests that rewrite a file and ask again.
#[cfg(test)]
pub(super) fn forget() {
    PROVED.with(|proved| proved.borrow_mut().clear());
}

/// The verdict for `root` of the module at `path`, proved once per worker.
fn proved(path: &Path, root: Root) -> Option<Vec<PathBuf>> {
    let key = (path.to_path_buf(), root.clone());
    if let Some(done) = PROVED.with(|proved| proved.borrow().get(&key).cloned()) {
        return done;
    }
    let mut proof = Proof::default();
    let verdict = proof.prove(path, root).then_some(proof.modules);
    PROVED.with(|proved| proved.borrow_mut().insert(key, verdict.clone()));
    verdict
}

/// Whether importing the module at `path` provably registers nothing.
pub(super) fn imports_register_nothing(path: &Path) -> bool {
    proved(path, Root::ImportTime).is_some()
}

/// Whether calling export `export` of the module at `path` provably registers nothing. Returns
/// the modules the proof relied on, so the caller can ask whether any of them is mocked.
pub(super) fn call_registers_nothing(path: &Path, export: &str) -> Option<Vec<PathBuf>> {
    proved(path, Root::Export(export.to_owned()))
}

/// Whether evaluating `arguments`, written in the file at `file`, provably registers nothing: no
/// registration API is reachable from them, so a helper handed them cannot run one.
pub(super) fn arguments_register_nothing(
    program: &Program<'_>,
    semantic: &Semantic<'_>,
    written: &HashSet<SymbolId>,
    file: &Path,
    arguments: &[Argument<'_>],
) -> Option<Vec<PathBuf>> {
    let mut scanner = Scanner::new(semantic, program, file);
    scanner.written = Some(written);
    for argument in arguments {
        if matches!(argument, Argument::SpreadElement(_)) {
            return None;
        }
        scanner.queue.push(Pending::Argument(argument));
    }
    scanner.drain();
    if scanner.failed {
        return None;
    }
    let mut proof = Proof::default();
    proof.prove_edges(scanner.edges).then_some(proof.modules)
}

/// Scans the part of the file at `path` named by `root`, and returns the edges it reaches.
fn scan_file(path: &Path, root: &Root) -> Option<Vec<Edge>> {
    let source = helpers::admitted_source(path)?;
    let is_component = path.extension().is_some_and(|extension| extension == "vue");
    let blocks: Vec<Block> = if is_component {
        script_blocks(&source)?
    } else {
        vec![Block {
            text: source.to_string(),
            source_type: SourceType::from_path(path).ok()?,
            setup: false,
        }]
    };
    let mut edges = Vec::new();
    for block in &blocks {
        // A component's `<script setup>` runs at mount: only its imports run at import.
        let root = if is_component { &Root::Whole } else { root };
        edges.extend(scan_block(path, block, root)?);
    }
    Some(edges)
}

fn scan_block(path: &Path, block: &Block, root: &Root) -> Option<Vec<Edge>> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &block.text, block.source_type).parse();
    if parsed.fatal_error || !parsed.diagnostics.is_empty() {
        return None;
    }
    let semantic_return = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&parsed.program);
    if !semantic_return.diagnostics.is_empty() {
        return None;
    }
    let program = &parsed.program;
    let mut scanner = Scanner::new(&semantic_return.semantic, program, path);
    if !block.setup && !top_level_is_declarations(program) {
        return None;
    }
    scanner.seed_import_time(&program.body);
    match root {
        Root::ImportTime => {}
        Root::Export(name) | Root::Reexport(name) => {
            if !scanner.seed_export(program, name) {
                scanner.seed_everything(program);
            }
        }
        Root::ExportIfPresent(name) => {
            scanner.seed_export(program, name);
        }
        Root::Whole => scanner.seed_everything(program),
    }
    scanner.drain();
    (!scanner.failed).then_some(scanner.edges)
}

/// Code a walk has yet to scan.
enum Pending<'a> {
    Expression(&'a Expression<'a>),
    Argument(&'a Argument<'a>),
    Function(&'a Function<'a>),
    Declarator(&'a VariableDeclarator<'a>),
    Class(&'a Class<'a>),
    Enum(&'a TSEnumDeclaration<'a>),
}

/// The declaration a top-level statement introduces, exported or not.
enum Declared<'a> {
    Function(&'a Function<'a>),
    Class(&'a Class<'a>),
    Variables(&'a VariableDeclaration<'a>),
}

fn declared_by<'a>(statement: &'a Statement<'a>) -> Option<Declared<'a>> {
    match statement {
        Statement::FunctionDeclaration(function) => Some(Declared::Function(function)),
        Statement::ClassDeclaration(class) => Some(Declared::Class(class)),
        Statement::VariableDeclaration(variables) => Some(Declared::Variables(variables)),
        Statement::ExportDeclaration(export) => match &export.declaration {
            Declaration::FunctionDeclaration(function) => Some(Declared::Function(function)),
            Declaration::ClassDeclaration(class) => Some(Declared::Class(class)),
            Declaration::VariableDeclaration(variables) => Some(Declared::Variables(variables)),
            _ => None,
        },
        _ => None,
    }
}

/// The local binding an import specifier introduces, and the name it has in the imported module.
fn import_binding<'a>(
    specifier: &'a ImportDeclarationSpecifier<'a>,
) -> (&'a oxc_ast::ast::BindingIdentifier<'a>, String) {
    match specifier {
        ImportDeclarationSpecifier::ImportSpecifier(it) => {
            (&it.local, it.imported.name().to_string())
        }
        ImportDeclarationSpecifier::ImportDefaultSpecifier(it) => (&it.local, "default".to_owned()),
        ImportDeclarationSpecifier::ImportNamespaceSpecifier(it) => (&it.local, "*".to_owned()),
    }
}

/// The bindings of `program` that are written after they are declared: reassigned, or changed
/// through a member (`config.mode = 2`, `items[0]++`, `delete config.mode`). A call that reads one
/// stands on a value its declaration does not describe.
pub(super) fn written_bindings(
    program: &Program<'_>,
    semantic: &Semantic<'_>,
) -> HashSet<SymbolId> {
    struct Writes<'s, 'a> {
        semantic: &'s Semantic<'a>,
        symbols: HashSet<SymbolId>,
    }
    impl<'a> Writes<'_, 'a> {
        /// Marks the binding at the root of a member chain.
        fn mark_root(&mut self, member: &oxc_ast::ast::MemberExpression<'a>) {
            let mut object = member.object();
            loop {
                object = match crate::frontend::registry::unwrap_transparent(object) {
                    Expression::StaticMemberExpression(inner) => &inner.object,
                    Expression::ComputedMemberExpression(inner) => &inner.object,
                    Expression::Identifier(identifier) => {
                        if let Some(symbol) = identifier.reference_id.get().and_then(|reference| {
                            self.semantic.scoping().get_reference(reference).symbol_id()
                        }) {
                            self.symbols.insert(symbol);
                        }
                        return;
                    }
                    _ => return,
                };
            }
        }
    }
    impl<'a> Visit<'a> for Writes<'_, 'a> {
        fn visit_assignment_expression(&mut self, it: &oxc_ast::ast::AssignmentExpression<'a>) {
            if let Some(member) = it.left.as_member_expression() {
                self.mark_root(member);
            }
            walk::walk_assignment_expression(self, it);
        }
        fn visit_update_expression(&mut self, it: &oxc_ast::ast::UpdateExpression<'a>) {
            if let Some(member) = it.argument.as_member_expression() {
                self.mark_root(member);
            }
            walk::walk_update_expression(self, it);
        }
        fn visit_unary_expression(&mut self, it: &oxc_ast::ast::UnaryExpression<'a>) {
            if it.operator == oxc_ast::ast::UnaryOperator::Delete {
                if let Some(member) = it.argument.as_member_expression() {
                    self.mark_root(member);
                }
            }
            walk::walk_unary_expression(self, it);
        }
    }
    let mut writes = Writes {
        semantic,
        symbols: HashSet::new(),
    };
    writes.visit_program(program);
    writes.symbols
}

/// What an import binding is: the module it came from and the name it has there.
struct Binding {
    source: String,
    imported: String,
}

/// The reachability scan of one module.
struct Scanner<'s, 'a> {
    semantic: &'s Semantic<'a>,
    file: &'s Path,
    bindings: HashMap<SymbolId, Binding>,
    seen: HashSet<SymbolId>,
    queue: Vec<Pending<'a>>,
    edges: Vec<Edge>,
    /// Bindings that are written after they are declared. When present, following one is refused:
    /// the scan is of a call's arguments, and a callback reassigned or a config changed before the
    /// call is not the declaration that was read.
    written: Option<&'s HashSet<SymbolId>>,
    failed: bool,
}

impl<'s, 'a> Scanner<'s, 'a> {
    fn new(semantic: &'s Semantic<'a>, program: &Program<'a>, file: &'s Path) -> Self {
        let mut bindings = HashMap::new();
        for statement in &program.body {
            let Statement::ImportDeclaration(import) = statement else {
                continue;
            };
            let source = import.source.value.as_str();
            for specifier in import.specifiers.iter().flatten() {
                let (local, imported) = import_binding(specifier);
                if let Some(symbol) = local.symbol_id.get() {
                    // Erased imports are kept: a registration API spelled through one is not harmless.
                    bindings.insert(
                        symbol,
                        Binding {
                            source: source.to_owned(),
                            imported,
                        },
                    );
                }
            }
        }
        Self {
            semantic,
            file,
            bindings,
            seen: HashSet::new(),
            queue: Vec::new(),
            edges: Vec::new(),
            written: None,
            failed: false,
        }
    }

    fn drain(&mut self) {
        while let Some(pending) = self.queue.pop() {
            if self.failed {
                return;
            }
            match pending {
                Pending::Expression(expression) => self.visit_expression(expression),
                Pending::Argument(argument) => self.visit_argument(argument),
                Pending::Function(function) => self.visit_function(function, ScopeFlags::Function),
                Pending::Declarator(declarator) => self.visit_variable_declarator(declarator),
                Pending::Class(class) => self.visit_class(class),
                Pending::Enum(declaration) => self.visit_ts_enum_declaration(declaration),
            }
        }
    }

    /// The edge to what the project's generated list says `name` stands for, for code written in
    /// `file`; a name it does not list is unknown.
    fn auto_import_edge(&mut self, file: &Path, name: &str) {
        match auto_imports::lookup(file, name) {
            Some(origin) => {
                let root = if origin.imported == "*" {
                    Root::Whole
                } else {
                    Root::Export(origin.imported.clone())
                };
                self.module_edge(&origin.importer, &origin.specifier, root);
            }
            None => self.failed = true,
        }
    }

    /// An edge to the module `specifier` names, written in `importer`.
    fn module_edge(&mut self, importer: &Path, specifier: &str, root: Root) {
        if specifier == "vitest" {
            // Importing it runs nothing that registers; what is taken from it is judged by name.
            match &root {
                Root::ImportTime => {}
                Root::Export(name) | Root::Reexport(name) | Root::ExportIfPresent(name) => {
                    if REGISTRATION.contains(&name.as_str()) {
                        self.failed = true;
                    }
                }
                Root::Whole => self.failed = true,
            }
            return;
        }
        if specifier.starts_with("vitest/") || specifier.starts_with("@vitest/") {
            self.failed = true;
            return;
        }
        let known = match &root {
            Root::ImportTime => is_known_runtime_module(specifier),
            Root::Export(name) | Root::Reexport(name) | Root::ExportIfPresent(name) => {
                is_known_runtime_export(specifier, name)
            }
            Root::Whole => false,
        };
        if known {
            return;
        }
        // `#imports` is the generated auto-import list under another spelling: each name is
        // judged by what the project says it stands for.
        if specifier == "#imports" {
            match root {
                Root::ImportTime => {}
                Root::Export(name) | Root::Reexport(name) | Root::ExportIfPresent(name) => {
                    self.auto_import_edge(importer, &name);
                }
                Root::Whole => self.failed = true,
            }
            return;
        }
        if resolver::is_node_builtin(specifier) {
            // These run code from a string or in another context, which no lexical walk can see.
            let stem = specifier.strip_prefix("node:").unwrap_or(specifier);
            let stem = stem.split('/').next().unwrap_or(stem);
            if CODE_RUNNING_BUILTINS.contains(&stem) {
                self.failed = true;
            }
            return;
        }
        if NUXT_VIRTUAL.iter().any(|prefix| {
            specifier == *prefix
                || specifier
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
        }) {
            return;
        }
        match resolver::resolve(importer, specifier) {
            Resolved::Module(path) => self.edges.push((path, root)),
            Resolved::Unresolved => self.failed = true,
        }
    }

    /// The edge for an `import` declaration. A declaration whose bindings are never read as values
    /// is erased by the TypeScript transform, so one that names no module on disk (a package
    /// that ships only types) costs nothing; a resolvable one is still loaded.
    fn import_edge(&mut self, import: &'a oxc_ast::ast::ImportDeclaration<'a>) {
        let specifier = import.source.value.as_str();
        let unread = import.specifiers.as_ref().is_some_and(|specifiers| {
            !specifiers.is_empty()
                && specifiers.iter().all(|specifier| {
                    let (local, _) = import_binding(specifier);
                    local.symbol_id.get().is_some_and(|symbol| {
                        !self
                            .semantic
                            .scoping()
                            .get_resolved_references(symbol)
                            .any(|reference| reference.is_value())
                    })
                })
        });
        if unread
            && matches!(
                resolver::resolve(self.file, specifier),
                Resolved::Unresolved
            )
        {
            return;
        }
        self.module_edge(self.file, specifier, Root::ImportTime);
    }

    /// Queues what importing the module runs: its top-level initialisers, never a function body
    /// it merely declares.
    fn seed_import_time(&mut self, statements: &'a [Statement<'a>]) {
        for statement in statements {
            match statement {
                Statement::ImportDeclaration(import)
                    if !matches!(import.import_kind, ImportOrExportKind::Type) =>
                {
                    self.import_edge(import);
                }
                Statement::ExportFromDeclaration(export)
                    if !matches!(export.export_kind, ImportOrExportKind::Type) =>
                {
                    self.module_edge(self.file, export.source.value.as_str(), Root::ImportTime);
                }
                Statement::ExportAllDeclaration(export)
                    if !matches!(export.export_kind, ImportOrExportKind::Type) =>
                {
                    self.module_edge(self.file, export.source.value.as_str(), Root::ImportTime);
                }
                Statement::ClassDeclaration(class) => self.seed_class(class),
                Statement::ExpressionStatement(statement) => {
                    self.queue.push(Pending::Expression(&statement.expression));
                }
                Statement::VariableDeclaration(variables) => {
                    for declarator in &variables.declarations {
                        self.seed_declarator(declarator);
                    }
                }
                Statement::TSNamespaceDeclaration(namespace) => self.seed_namespace(namespace),
                Statement::TSEnumDeclaration(declaration) => {
                    self.queue.push(Pending::Enum(declaration));
                }
                Statement::ExportDeclaration(export) => match &export.declaration {
                    Declaration::TSEnumDeclaration(declaration) => {
                        self.queue.push(Pending::Enum(declaration));
                    }
                    Declaration::ClassDeclaration(class) => self.seed_class(class),
                    Declaration::VariableDeclaration(variables) => {
                        for declarator in &variables.declarations {
                            self.seed_declarator(declarator);
                        }
                    }
                    Declaration::TSNamespaceDeclaration(namespace) => {
                        self.seed_namespace(namespace);
                    }
                    _ => {}
                },
                Statement::ExportDefaultDeclaration(export) => match &export.declaration {
                    ExportDefaultDeclarationKind::ClassDeclaration(class) => self.seed_class(class),
                    ExportDefaultDeclarationKind::FunctionDeclaration(_)
                    | ExportDefaultDeclarationKind::ArrowFunctionExpression(_)
                    | ExportDefaultDeclarationKind::FunctionExpression(_)
                    | ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => {}
                    other => {
                        if let Some(expression) = other.as_expression() {
                            self.queue.push(Pending::Expression(expression));
                        }
                    }
                },
                _ => {}
            }
        }
    }

    fn seed_namespace(&mut self, namespace: &'a TSNamespaceDeclaration<'a>) {
        match &namespace.body {
            TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => self.seed_namespace(inner),
            TSNamespaceDeclarationBody::TSModuleBlock(block) => self.seed_import_time(&block.body),
        }
    }

    /// A class runs code when it is defined only through computed keys, decorators and static members.
    fn seed_class(&mut self, class: &'a Class<'a>) {
        if !super::imported::class_is_inert(class) {
            self.queue.push(Pending::Class(class));
        }
    }

    /// A declarator whose value is a function only defines it; any other initialiser runs.
    fn seed_declarator(&mut self, declarator: &'a VariableDeclarator<'a>) {
        let defines_function = matches!(
            &declarator.init,
            Some(Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_))
        ) && matches!(
            declarator.id,
            oxc_ast::ast::BindingPattern::BindingIdentifier(_)
        );
        if declarator.init.is_some() && !defines_function {
            self.queue.push(Pending::Declarator(declarator));
        }
    }

    /// Queues the declaration of export `name`. False when the module has no readable export of
    /// that name (and no `export *` that could supply it).
    fn seed_export(&mut self, program: &'a Program<'a>, name: &str) -> bool {
        let mut found = false;
        let mut stars = false;
        for statement in &program.body {
            if let Some(declared) = declared_by(statement) {
                found |= self.queue_declared(declared, Some(name));
                continue;
            }
            match statement {
                Statement::ExportNamedDeclaration(export) => {
                    for specifier in &export.specifiers {
                        if specifier.exported.name() == name {
                            found = true;
                            self.seed_local(program, specifier.local.name().as_str());
                        }
                    }
                }
                Statement::ExportFromDeclaration(export) => {
                    for specifier in &export.specifiers {
                        if specifier.exported.name() == name {
                            found = true;
                            self.module_edge(
                                self.file,
                                export.source.value.as_str(),
                                Root::Reexport(specifier.local.name().to_string()),
                            );
                        }
                    }
                }
                Statement::ExportAllDeclaration(export) => match &export.exported {
                    Some(exported) if exported.name() == name => {
                        found = true;
                        self.module_edge(self.file, export.source.value.as_str(), Root::Whole);
                    }
                    Some(_) => {}
                    None => {
                        stars = true;
                        self.module_edge(
                            self.file,
                            export.source.value.as_str(),
                            Root::ExportIfPresent(name.to_owned()),
                        );
                    }
                },
                Statement::ExportDefaultDeclaration(export) if name == "default" => {
                    found = true;
                    self.queue_default(program, &export.declaration);
                }
                _ => {}
            }
        }
        found || stars
    }

    /// Queues a default export's value.
    fn queue_default(
        &mut self,
        program: &'a Program<'a>,
        kind: &'a ExportDefaultDeclarationKind<'a>,
    ) {
        match kind {
            ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                self.queue.push(Pending::Function(function));
            }
            ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                self.queue.push(Pending::Class(class));
            }
            ExportDefaultDeclarationKind::Identifier(identifier) => {
                self.seed_local(program, identifier.name.as_str());
            }
            ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => {}
            other => {
                if let Some(expression) = other.as_expression() {
                    self.queue.push(Pending::Expression(expression));
                }
            }
        }
    }

    /// Queues what `declared` introduces, all of it or only the binding called `only`. Whether
    /// anything was queued. A named function with no body is a signature, not something to run.
    fn queue_declared(&mut self, declared: Declared<'a>, only: Option<&str>) -> bool {
        let wanted = |id: Option<&str>| only.is_none_or(|name| id == Some(name));
        match declared {
            Declared::Function(function)
                if wanted(function.id.as_ref().map(|id| id.name.as_str())) =>
            {
                if function.body.is_none() && only.is_some() {
                    self.failed = true;
                }
                self.queue.push(Pending::Function(function));
                true
            }
            Declared::Class(class) if wanted(class.id.as_ref().map(|id| id.name.as_str())) => {
                self.queue.push(Pending::Class(class));
                true
            }
            Declared::Variables(variables) => {
                let mut queued = false;
                for declarator in &variables.declarations {
                    if wanted(declarator.id.get_identifier_name().as_deref()) {
                        self.queue.push(Pending::Declarator(declarator));
                        queued = true;
                    }
                }
                queued
            }
            _ => false,
        }
    }

    /// Queues the top-level declaration called `name`; an import binding of that name is
    /// re-exported, so its source is followed. A name with no declaration refuses the proof.
    fn seed_local(&mut self, program: &'a Program<'a>, name: &str) {
        let mut found = false;
        for statement in &program.body {
            if let Some(declared) = declared_by(statement) {
                found |= self.queue_declared(declared, Some(name));
                continue;
            }
            let Statement::ImportDeclaration(import) = statement else {
                continue;
            };
            for specifier in import.specifiers.iter().flatten() {
                let (local, imported) = import_binding(specifier);
                if local.name == name {
                    found = true;
                    let root = if imported == "*" {
                        Root::Whole
                    } else {
                        Root::Export(imported)
                    };
                    self.module_edge(self.file, import.source.value.as_str(), root);
                }
            }
        }
        if !found {
            self.failed = true;
        }
    }

    /// Queues every declaration, when nothing narrower can be named.
    fn seed_everything(&mut self, program: &'a Program<'a>) {
        for statement in &program.body {
            if let Some(declared) = declared_by(statement) {
                self.queue_declared(declared, None);
            } else if let Statement::ExportDefaultDeclaration(export) = statement {
                self.queue_default(program, &export.declaration);
            }
        }
    }

    /// Queues the declaration a local symbol stands for, once.
    fn follow_symbol(&mut self, symbol: SymbolId) {
        if !self.seen.insert(symbol) {
            return;
        }
        if self
            .written
            .is_some_and(|written| written.contains(&symbol))
            || (self.written.is_some() && self.semantic.scoping().symbol_is_mutated(symbol))
        {
            self.failed = true;
            return;
        }
        let declaration = self.semantic.scoping().symbol_declaration(symbol);
        match self.semantic.nodes().get_node(declaration).kind() {
            AstKind::VariableDeclarator(declarator) => {
                // In a call's arguments a declarator with no initialiser takes its value from
                // somewhere the declaration does not show (a `for...of` head).
                if self.written.is_some() && declarator.init.is_none() {
                    self.failed = true;
                    return;
                }
                self.queue.push(Pending::Declarator(declarator));
            }
            AstKind::Function(function) => self.queue.push(Pending::Function(function)),
            AstKind::Class(class) => self.queue.push(Pending::Class(class)),
            _ => {}
        }
    }

    /// Judges a free name: a Vitest export is judged by the caller, a host global is harmless, an
    /// auto-import becomes one more module to prove, and anything else is unknown. Returns true
    /// when the name must still be judged as a possible Vitest export.
    fn known_global(&mut self, name: &str) -> bool {
        if REGISTRATION.contains(&name) || matches!(name, "vi" | "expect" | "vitest") {
            return true;
        }
        if BUILTINS.contains(&name) {
            return false;
        }
        self.auto_import_edge(self.file, name);
        false
    }

    /// The Vitest export an identifier stands for: the imported name, or the free name itself in
    /// globals mode. `None` when it is anything else.
    fn vitest_name(&self, ident: &IdentifierReference<'a>) -> Option<String> {
        let symbol = self.symbol_of(ident);
        match symbol {
            Some(symbol) => {
                let binding = self.bindings.get(&symbol)?;
                (binding.source == "vitest").then(|| binding.imported.clone())
            }
            None => Some(ident.name.as_str().to_owned()),
        }
    }

    fn symbol_of(&self, ident: &IdentifierReference<'a>) -> Option<SymbolId> {
        ident
            .reference_id
            .get()
            .and_then(|reference| self.semantic.scoping().get_reference(reference).symbol_id())
    }
}

impl<'a> Visit<'a> for Scanner<'_, 'a> {
    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        // A name used only as a type is erased and reaches no code.
        if ident
            .reference_id
            .get()
            .is_some_and(|reference| !self.semantic.scoping().get_reference(reference).is_value())
        {
            return;
        }
        let symbol = self.symbol_of(ident);
        let free = symbol.is_none();
        if free && REFLECTIVE.contains(&ident.name.as_str()) {
            self.failed = true;
            return;
        }
        if free && !self.known_global(ident.name.as_str()) {
            return;
        }
        if let Some(name) = self.vitest_name(ident) {
            if REGISTRATION.contains(&name.as_str()) || name == "vitest" || name == "*" {
                self.failed = true;
            }
            return;
        }
        let Some(symbol) = symbol else { return };
        match self.bindings.get(&symbol) {
            Some(binding) => {
                // An imported object changed through a member is not the module's export.
                if self
                    .written
                    .is_some_and(|written| written.contains(&symbol))
                {
                    self.failed = true;
                    return;
                }
                let (source, imported) = (binding.source.clone(), binding.imported.clone());
                let root = if imported == "*" {
                    Root::Whole
                } else {
                    Root::Export(imported)
                };
                self.module_edge(self.file, &source, root);
            }
            None => self.follow_symbol(symbol),
        }
    }

    fn visit_static_member_expression(&mut self, member: &StaticMemberExpression<'a>) {
        // `globalThis.ready` reads or writes one named property, which cannot reach a registration
        // API unless it is one; the bare object or a computed key could.
        if let Expression::Identifier(object) =
            crate::frontend::registry::unwrap_transparent(&member.object)
        {
            if self.symbol_of(object).is_none()
                && REFLECTIVE.contains(&object.name.as_str())
                && !matches!(object.name.as_str(), "eval" | "Function" | "require")
            {
                let property = member.property.name.as_str();
                if REGISTRATION.contains(&property) || REFLECTIVE.contains(&property) {
                    self.failed = true;
                }
                return;
            }
        }
        if let Expression::Identifier(object) = &member.object {
            // `vi` and `expect` mock modules and change global state, which is setup the call's
            // identity carries; neither registers a test or a hook.
            if matches!(self.vitest_name(object).as_deref(), Some("vi" | "expect")) {
                return;
            }
            // `ns.member` on a namespace import reads one export.
            if let Some(binding) = self
                .symbol_of(object)
                .and_then(|symbol| self.bindings.get(&symbol))
            {
                if binding.imported == "*" && binding.source != "vitest" {
                    let source = binding.source.clone();
                    self.module_edge(
                        self.file,
                        &source,
                        Root::Export(member.property.name.to_string()),
                    );
                    return;
                }
            }
        }
        if let Expression::ImportMeta(_) = &member.object {
            // `import.meta.vitest` exposes the registration API to in-source tests and
            // `import.meta.glob*` loads modules by pattern; the rest is metadata.
            let property = member.property.name.as_str();
            if property == "vitest" || property.starts_with("glob") {
                self.failed = true;
            }
            return;
        }
        walk::walk_static_member_expression(self, member);
    }

    /// `import.meta` reached any way but a named property (`import.meta['vitest']`, a
    /// destructuring source) could hand over the registration API.
    fn visit_import_meta(&mut self, _it: &oxc_ast::ast::ImportMeta) {
        self.failed = true;
    }

    fn visit_import_expression(&mut self, import: &ImportExpression<'a>) {
        match &import.source {
            Expression::StringLiteral(literal) => {
                let specifier = literal.value.to_string();
                self.module_edge(self.file, &specifier, Root::Whole);
            }
            _ => self.failed = true,
        }
        walk::walk_import_expression(self, import);
    }
}
