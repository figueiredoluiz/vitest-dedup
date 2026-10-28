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
use crate::frontend::js_text::string_text;
use crate::frontend::resolver::{self, Resolved};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, BindingPattern, Class, Declaration, ExportDefaultDeclarationKind, Expression,
    FormalParameters, Function, IdentifierReference, ImportDeclarationSpecifier, ImportExpression,
    ImportOrExportKind, ObjectPropertyKind, Program, PropertyKind, Statement,
    StaticMemberExpression, TSEnumDeclaration, TSNamespaceDeclaration, TSNamespaceDeclarationBody,
    ThisExpression, VariableDeclaration, VariableDeclarator,
};
use oxc_ast::AstKind;
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::{NodeId, Semantic, SemanticBuilder, SymbolId};
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::scope::ScopeFlags;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Most modules one proof may visit before it refuses.
const MAX_MODULES: usize = 64;

/// Vitest names that register a test, a suite or a hook.
pub(in crate::frontend) const REGISTRATION: &[&str] = &[
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

/// A module to prove, the part of it, and whether that code runs while an importing module loads:
/// everything an import-time scan reaches runs then too.
type Edge = (PathBuf, Root, bool);

/// A bounded proof over modules reachable from a starting point.
#[derive(Default)]
struct Proof {
    visited: HashSet<Edge>,
    /// Edges being proved right now, to tell a cyclic re-export from a diamond.
    active: HashSet<Edge>,
    modules: Vec<PathBuf>,
}

impl Proof {
    /// Proves one edge once, refusing a re-export that comes back to itself while it is still
    /// being proved.
    fn prove(&mut self, path: &Path, root: Root, at_import: bool) -> bool {
        let edge = (path.to_path_buf(), root.clone(), at_import);
        if !self.visited.insert(edge.clone()) {
            return !(matches!(root, Root::Reexport(_)) && self.active.contains(&edge));
        }
        self.active.insert(edge.clone());
        let proved = self.prove_new(path, root, at_import);
        self.active.remove(&edge);
        proved
    }

    /// Proves an edge not seen before: an installed package by its manifests, an asset outright,
    /// and a first-party module by scanning it and proving every edge the scan reaches.
    fn prove_new(&mut self, path: &Path, root: Root, at_import: bool) -> bool {
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
        let Some(edges) = scan_file(path, &root, at_import) else {
            return false;
        };
        edges
            .into_iter()
            .all(|(next, next_root, next_at_import)| self.prove(&next, next_root, next_at_import))
    }

    /// Proves every edge a scan produced; true when none is refused.
    fn prove_edges(&mut self, edges: Vec<Edge>) -> bool {
        edges
            .into_iter()
            .all(|(path, root, at_import)| self.prove(&path, root, at_import))
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
    let at_import = matches!(root, Root::ImportTime);
    let key = (path.to_path_buf(), root.clone(), at_import);
    if let Some(done) = PROVED.with(|proved| proved.borrow().get(&key).cloned()) {
        return done;
    }
    let mut proof = Proof::default();
    let verdict = proof.prove(path, root, at_import).then_some(proof.modules);
    PROVED.with(|proved| proved.borrow_mut().insert(key, verdict.clone()));
    verdict
}

/// Whether importing the module at `path` provably registers nothing and uses no `vi` or `expect`
/// while it loads (see [`Scanner::import_time`]).
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
    evaluation_registers_nothing(
        program,
        semantic,
        written,
        file,
        None,
        arguments,
        Vec::new(),
    )
}

/// Whether the inline `callbacks` handed to a call this analyzer does not read, written in the
/// file at `file`, provably register nothing when that call runs them. Their parameters hold
/// whatever the callee passes (`[test].forEach((fn) => fn(...))`), so following one fails.
pub(super) fn callbacks_register_nothing<'p, 'a: 'p>(
    program: &Program<'_>,
    semantic: &Semantic<'_>,
    written: &HashSet<SymbolId>,
    file: &Path,
    callbacks: &[&'p Argument<'a>],
) -> bool {
    let supplied = callbacks
        .iter()
        .filter_map(
            |callback| match callback.as_expression()?.get_inner_expression() {
                Expression::FunctionExpression(function) => Some(function.params.span),
                Expression::ArrowFunctionExpression(arrow) => Some(arrow.params.span),
                _ => None,
            },
        )
        .collect();
    evaluation_registers_nothing(
        program,
        semantic,
        written,
        file,
        None,
        callbacks.iter().copied(),
        supplied,
    )
    .is_some()
}

/// Whether calling `callee`, a binding written in the file at `file`, with `arguments` provably
/// registers nothing: neither the value its declaration gives it nor the arguments reach a
/// registration API. A binding written after it is declared fails the proof.
pub(super) fn local_call_registers_nothing(
    program: &Program<'_>,
    semantic: &Semantic<'_>,
    written: &HashSet<SymbolId>,
    file: &Path,
    callee: &Expression<'_>,
    arguments: &[Argument<'_>],
) -> bool {
    evaluation_registers_nothing(
        program,
        semantic,
        written,
        file,
        Some(callee),
        arguments,
        Vec::new(),
    )
    .is_some()
}

/// Proves an expression the helper walker ignores has no registration effects when evaluated.
pub(super) fn ignored_expression_registers_nothing(
    program: &Program<'_>,
    semantic: &Semantic<'_>,
    written: &HashSet<SymbolId>,
    file: &Path,
    parameters: Span,
    expression: &Expression<'_>,
) -> bool {
    creates_without_running(semantic, expression)
        || ignored_evaluation_registers_nothing(
            program,
            semantic,
            written,
            file,
            (parameters, expression.span()),
            Pending::Expression(expression),
        )
}

/// Destructuring evaluates defaults, computed keys and getters as well as its initializer.
pub(super) fn ignored_declarator_registers_nothing(
    program: &Program<'_>,
    semantic: &Semantic<'_>,
    written: &HashSet<SymbolId>,
    file: &Path,
    parameters: Span,
    declarator: &VariableDeclarator<'_>,
) -> bool {
    ignored_evaluation_registers_nothing(
        program,
        semantic,
        written,
        file,
        (parameters, declarator.span),
        Pending::Declarator(declarator),
    )
}

/// Reuses the bounded registration proof, treating helper parameters as values supplied by callers.
fn ignored_evaluation_registers_nothing<'a>(
    program: &'a Program<'a>,
    semantic: &Semantic<'a>,
    written: &HashSet<SymbolId>,
    file: &Path,
    call: (Span, Span),
    pending: Pending<'a>,
) -> bool {
    let mut scanner = Scanner::new(semantic, program, file);
    scanner.written = Some(written);
    scanner.helper_call = Some(call);
    scanner.queue.push(pending);
    scanner.drain();
    !scanner.failed && Proof::default().prove_edges(scanner.edges)
}

/// Scans `callee` (when given) and `arguments` as code that runs in the file at `file`, and proves
/// the modules they reach; the modules the proof relied on, or `None`. `supplied` lists parameter
/// lists whose values the scan cannot see (see [`Scanner::unknown_at_call`]).
fn evaluation_registers_nothing<'p, 'a: 'p>(
    program: &Program<'_>,
    semantic: &Semantic<'_>,
    written: &HashSet<SymbolId>,
    file: &Path,
    callee: Option<&Expression<'_>>,
    arguments: impl IntoIterator<Item = &'p Argument<'a>>,
    supplied: Vec<Span>,
) -> Option<Vec<PathBuf>> {
    let mut scanner = Scanner::new(semantic, program, file);
    scanner.written = Some(written);
    scanner.supplied = supplied;
    if let Some(callee) = callee {
        scanner.queue.push(Pending::Expression(callee));
    }
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

/// Whether what a call to a local helper evaluates before its body runs provably registers
/// nothing: the call's `arguments`, the parameter defaults that can run, the reads a destructuring
/// parameter makes on its value (a getter or an iterator runs code), and the defaults and computed
/// keys nested in its pattern. An input that only creates values ([`creates_without_running`])
/// runs no code; any other input must pass the registration scan, which follows getters, called
/// functions and imports.
pub(super) fn call_inputs_register_nothing(
    program: &Program<'_>,
    semantic: &Semantic<'_>,
    written: &HashSet<SymbolId>,
    file: &Path,
    parameters: &FormalParameters<'_>,
    arguments: &[Argument<'_>],
    call: Span,
) -> bool {
    let mut supplied = Vec::new();
    for argument in arguments {
        let Some(expression) = argument.as_expression() else {
            return false;
        };
        supplied.push(expression);
    }
    let mut inputs: Vec<&Expression> = Vec::new();
    // Read through a destructuring pattern, so scanned even when the expression itself is inert.
    let mut destructured: Vec<&Expression> = Vec::new();
    for (index, parameter) in parameters.items.iter().enumerate() {
        // An argument written as `undefined` is passed as if absent.
        let argument = supplied
            .get(index)
            .copied()
            .filter(|argument| !written_undefined(semantic, argument));
        // A default runs only when its argument is `undefined`, which a literal never is.
        let default = parameter
            .initializer
            .as_deref()
            .filter(|_| argument.is_none_or(|argument| !never_undefined(argument)));
        inputs.extend(default);
        if !matches!(parameter.pattern, BindingPattern::BindingIdentifier(_)) {
            // The pattern reads whichever value it receives: the argument, or the default when
            // that can run.
            destructured.extend(argument);
            destructured.extend(default);
            pattern_inputs(&parameter.pattern, &mut inputs, &mut destructured);
        }
    }
    if let Some(rest) = &parameters.rest {
        if !matches!(rest.rest.argument, BindingPattern::BindingIdentifier(_)) {
            destructured.extend(supplied.iter().skip(parameters.items.len()).copied());
            pattern_inputs(&rest.rest.argument, &mut inputs, &mut destructured);
        }
    }
    let mut scanner = Scanner::new(semantic, program, file);
    scanner.written = Some(written);
    scanner.helper_call = Some((parameters.span, call));
    for input in supplied.iter().chain(&inputs) {
        if !creates_without_running(semantic, input) {
            scanner.queue.push(Pending::Expression(input));
        }
    }
    for value in destructured {
        scanner.queue.push(Pending::Expression(value));
    }
    scanner.drain();
    !scanner.failed && Proof::default().prove_edges(scanner.edges)
}

/// What a binding pattern receives, as far as the source shows it.
#[derive(Clone, Copy)]
enum Bound<'p, 'a> {
    /// `undefined`: no argument, an argument spelled `undefined`, or a key or index a literal
    /// lacks.
    Missing,
    Value(&'p Expression<'a>),
}

/// Whether some parameter of a call is known to fail to bind: destructuring `undefined` or `null`,
/// or array-destructuring a value that is not iterable, throws before the helper's body runs.
/// Only values the source spells out are judged, through literal object and array arguments and
/// defaults; any other value is assumed to bind.
pub(super) fn call_binding_fails(
    semantic: &Semantic<'_>,
    parameters: &FormalParameters<'_>,
    arguments: &[Argument<'_>],
) -> bool {
    // A spread moves every later argument to a position the source does not show.
    if arguments
        .iter()
        .any(|argument| argument.as_expression().is_none())
    {
        return false;
    }
    parameters
        .items
        .iter()
        .enumerate()
        .any(|(index, parameter)| {
            let received = arguments
                .get(index)
                .and_then(Argument::as_expression)
                .map_or(Bound::Missing, Bound::Value);
            let received = match (received, parameter.initializer.as_deref()) {
                (bound, Some(default)) if bound_is_undefined(semantic, bound) => {
                    Bound::Value(default)
                }
                (bound, _) => bound,
            };
            pattern_binding_fails(semantic, &parameter.pattern, received)
        })
}

/// Whether `bound` is `undefined`, the value that makes a parameter's default run.
fn bound_is_undefined(semantic: &Semantic<'_>, bound: Bound<'_, '_>) -> bool {
    match bound {
        Bound::Missing => true,
        Bound::Value(value) => written_undefined(semantic, strip_value(value)),
    }
}

/// Whether binding `pattern` to `received` is known to throw.
fn pattern_binding_fails(
    semantic: &Semantic<'_>,
    pattern: &BindingPattern<'_>,
    received: Bound<'_, '_>,
) -> bool {
    let value = match received {
        Bound::Value(value) => Some(strip_value(value)),
        Bound::Missing => None,
    };
    match pattern {
        BindingPattern::BindingIdentifier(_) => false,
        BindingPattern::AssignmentPattern(assignment) => {
            let received = if bound_is_undefined(semantic, received) {
                Bound::Value(&assignment.right)
            } else {
                received
            };
            pattern_binding_fails(semantic, &assignment.left, received)
        }
        BindingPattern::ObjectPattern(object) => match value {
            None => true,
            Some(value) if written_undefined(semantic, value) => true,
            Some(Expression::NullLiteral(_)) => true,
            Some(Expression::ObjectExpression(literal)) => {
                object.properties.iter().any(|property| {
                    if property.computed {
                        return false;
                    }
                    let Some(name) = property.key.static_name() else {
                        return false;
                    };
                    literal_property(literal, &name).is_some_and(|bound| {
                        pattern_binding_fails(semantic, &property.value, bound)
                    })
                })
            }
            Some(_) => false,
        },
        BindingPattern::ArrayPattern(array) => match value {
            None => true,
            Some(value) if written_undefined(semantic, value) => true,
            Some(Expression::ObjectExpression(literal)) => {
                literal.properties.iter().all(|property| {
                    matches!(property, ObjectPropertyKind::ObjectProperty(property)
                    if !property.computed && !sets_prototype(property))
                })
            }
            Some(Expression::ArrayExpression(literal)) => {
                array.elements.iter().enumerate().any(|(index, element)| {
                    let Some(element) = element else { return false };
                    // A spread before this index moves it to a position the source does not show.
                    if literal
                        .elements
                        .iter()
                        .take(index + 1)
                        .any(|item| item.is_spread())
                    {
                        return false;
                    }
                    let bound = match literal
                        .elements
                        .get(index)
                        .and_then(|item| item.as_expression())
                    {
                        Some(item) => Bound::Value(item),
                        None => Bound::Missing,
                    };
                    pattern_binding_fails(semantic, element, bound)
                })
            }
            // A string is iterable; every other scalar literal and a function is not. A class is
            // only through a static iterator, so refusing one costs a read, never a test.
            Some(other) => {
                matches!(other, Expression::ClassExpression(_))
                    || (!matches!(other, Expression::StringLiteral(_)) && is_value_literal(other))
            }
        },
    }
}

/// What an object literal holds under `name`: `None` when unknown (a spread, an accessor or a
/// computed key could supply it), `Missing` when no own property can.
fn literal_property<'p, 'a>(
    literal: &'p oxc_ast::ast::ObjectExpression<'a>,
    name: &oxc_ast::StaticPropertyName<'_>,
) -> Option<Bound<'p, 'a>> {
    let mut found = None;
    for property in &literal.properties {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            return None;
        };
        if property.computed || property.kind != PropertyKind::Init || sets_prototype(property) {
            return None;
        }
        if property.key.static_name().is_some_and(|key| &key == name) {
            found = Some(&property.value);
        }
    }
    // An absent own key may still be inherited (`toString`); reading it as `undefined` refuses
    // more, never less.
    Some(found.map_or(Bound::Missing, Bound::Value))
}

/// Whether an object-literal entry is `__proto__: value`, which sets the prototype the object
/// inherits keys and an iterator from instead of defining a key.
fn sets_prototype(property: &oxc_ast::ast::ObjectProperty<'_>) -> bool {
    !property.shorthand && !property.method && property.key.is_specific_static_name("__proto__")
}

/// `expression` without the wrappers that do not change its value.
fn strip_value<'p, 'a>(expression: &'p Expression<'a>) -> &'p Expression<'a> {
    match expression {
        Expression::ParenthesizedExpression(inner) => strip_value(&inner.expression),
        Expression::TSAsExpression(inner) => strip_value(&inner.expression),
        Expression::TSSatisfiesExpression(inner) => strip_value(&inner.expression),
        Expression::TSNonNullExpression(inner) => strip_value(&inner.expression),
        other => other,
    }
}

/// Whether `expression` is spelled as `undefined`: the free name (a local binding of that name
/// holds whatever it was given), or a `void` expression.
fn written_undefined(semantic: &Semantic<'_>, expression: &Expression<'_>) -> bool {
    match expression {
        Expression::Identifier(identifier) => {
            identifier.name == "undefined"
                && identifier.reference_id.get().is_some_and(|reference| {
                    semantic
                        .scoping()
                        .get_reference(reference)
                        .symbol_id()
                        .is_none()
                })
        }
        Expression::UnaryExpression(unary) => {
            unary.operator == oxc_syntax::operator::UnaryOperator::Void
        }
        Expression::ParenthesizedExpression(inner) => {
            written_undefined(semantic, &inner.expression)
        }
        _ => false,
    }
}

/// Whether `expression` evaluates to something other than `undefined` whatever the program does:
/// a literal other than a name, so a parameter default it is passed for never runs.
fn never_undefined(expression: &Expression<'_>) -> bool {
    match expression {
        Expression::TemplateLiteral(_)
        | Expression::ArrayExpression(_)
        | Expression::ObjectExpression(_) => true,
        Expression::ParenthesizedExpression(inner) => never_undefined(&inner.expression),
        other => is_value_literal(other),
    }
}

/// A scalar literal or a function: evaluating it creates a value that is never `undefined` and
/// runs no code.
fn is_value_literal(expression: &Expression<'_>) -> bool {
    matches!(
        expression,
        Expression::StringLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::BigIntLiteral(_)
            | Expression::RegExpLiteral(_)
            | Expression::FunctionExpression(_)
            | Expression::ArrowFunctionExpression(_)
    )
}

/// Whether evaluating `expression` only creates values and runs no code: a literal, a name this
/// file binds or imports (reading a binding calls nothing; a free name may be a global getter), a
/// function or arrow, or an array or object literal of those with plain keys and no spread.
/// Member reads, computed keys, operators and calls can run code and are not admitted.
fn creates_without_running(semantic: &Semantic<'_>, expression: &Expression<'_>) -> bool {
    if is_value_literal(expression) {
        return true;
    }
    match expression {
        Expression::TemplateLiteral(template) => template.expressions.is_empty(),
        Expression::Identifier(identifier) => {
            identifier.reference_id.get().is_some_and(|reference| {
                semantic
                    .scoping()
                    .get_reference(reference)
                    .symbol_id()
                    .is_some()
            })
        }
        Expression::ArrayExpression(array) => array.elements.iter().all(|element| {
            element.is_elision()
                || element
                    .as_expression()
                    .is_some_and(|inner| creates_without_running(semantic, inner))
        }),
        Expression::ObjectExpression(object) => object.properties.iter().all(|property| {
            matches!(property, ObjectPropertyKind::ObjectProperty(property)
                if !property.computed
                    && (property.method
                        || property.kind != PropertyKind::Init
                        || creates_without_running(semantic, &property.value)))
        }),
        Expression::ParenthesizedExpression(inner) => {
            creates_without_running(semantic, &inner.expression)
        }
        Expression::TSAsExpression(inner) => creates_without_running(semantic, &inner.expression),
        Expression::TSSatisfiesExpression(inner) => {
            creates_without_running(semantic, &inner.expression)
        }
        Expression::TSNonNullExpression(inner) => {
            creates_without_running(semantic, &inner.expression)
        }
        _ => false,
    }
}

/// The expressions a binding pattern evaluates while it binds: defaults go to `inputs`, and a
/// value the pattern goes on to read or coerce goes to `read` -- a default destructured further
/// (its getters run) and a computed key (its `toString` runs).
fn pattern_inputs<'p, 'a>(
    pattern: &'p BindingPattern<'a>,
    inputs: &mut Vec<&'p Expression<'a>>,
    read: &mut Vec<&'p Expression<'a>>,
) {
    match pattern {
        BindingPattern::BindingIdentifier(_) => {}
        BindingPattern::AssignmentPattern(assignment) => {
            inputs.push(&assignment.right);
            if !matches!(assignment.left, BindingPattern::BindingIdentifier(_)) {
                read.push(&assignment.right);
            }
            pattern_inputs(&assignment.left, inputs, read);
        }
        BindingPattern::ObjectPattern(object) => {
            for property in &object.properties {
                read.extend(property.key.as_expression());
                pattern_inputs(&property.value, inputs, read);
            }
            if let Some(rest) = &object.rest {
                pattern_inputs(&rest.argument, inputs, read);
            }
        }
        BindingPattern::ArrayPattern(array) => {
            for element in array.elements.iter().flatten() {
                pattern_inputs(element, inputs, read);
            }
            if let Some(rest) = &array.rest {
                pattern_inputs(&rest.argument, inputs, read);
            }
        }
    }
}

/// Scans the part of the file at `path` named by `root`, and returns the edges it reaches.
/// `at_import` says the code runs while an importing module loads.
fn scan_file(path: &Path, root: &Root, at_import: bool) -> Option<Vec<Edge>> {
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
        edges.extend(scan_block(path, block, root, at_import)?);
    }
    Some(edges)
}

/// Scans one script block for the part `root` names, returning the module edges it reaches, or
/// `None` when the block cannot be proved. A `<script setup>` block runs at mount, never while
/// its module loads.
fn scan_block(path: &Path, block: &Block, root: &Root, at_import: bool) -> Option<Vec<Edge>> {
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
    scanner.import_time = at_import && !block.setup;
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

/// The one declaration of `symbol` that runs, or `None` when several do: a later declaration
/// replaces or extends what the first one says (`var x = a; var x = b`, `function Run` merged with
/// a `namespace Run` that holds values). Interfaces, type aliases, overload signatures, `declare`
/// declarations and namespaces of types only are erased, so `const Foo` beside `interface Foo`
/// resolves to the `const`, and an overload set to its implementation. A name with no declaration
/// that runs (only `declare` overloads) resolves to its first declaration, as an ambient one does.
pub(in crate::frontend::extract) fn runtime_declaration(
    semantic: &Semantic<'_>,
    symbol: SymbolId,
) -> Option<oxc_semantic::NodeId> {
    let scoping = semantic.scoping();
    if scoping.symbol_redeclarations(symbol).is_empty() {
        return Some(scoping.symbol_declaration(symbol));
    }
    let nodes = semantic.nodes();
    let runs = |node: oxc_semantic::NodeId| match nodes.kind(node) {
        AstKind::TSInterfaceDeclaration(_) | AstKind::TSTypeAliasDeclaration(_) => false,
        AstKind::Function(function) => function.body.is_some() && !function.declare,
        AstKind::Class(class) => !class.declare,
        AstKind::TSEnumDeclaration(declaration) => !declaration.declare && !declaration.r#const,
        AstKind::TSNamespaceDeclaration(declaration) => {
            !declaration.declare && namespace_holds_values(declaration)
        }
        AstKind::VariableDeclarator(_) => !matches!(
            nodes.parent_kind(node),
            AstKind::VariableDeclaration(declaration) if declaration.declare
        ),
        _ => true,
    };
    let mut running = scoping
        .symbol_declarations(symbol)
        .filter(|node| runs(*node));
    let Some(first) = running.next() else {
        return Some(scoping.symbol_declaration(symbol));
    };
    running.next().is_none().then_some(first)
}

/// Whether `symbol` has more than one declaration that runs (see [`runtime_declaration`]).
pub(in crate::frontend) fn redeclared_at_runtime(
    semantic: &Semantic<'_>,
    symbol: SymbolId,
) -> bool {
    runtime_declaration(semantic, symbol).is_none()
}

/// Whether the value a reference to `symbol` observes can differ from what its first declaration
/// says: more than one declaration runs, or the binding is reassigned. `Scoping::symbol_declaration`
/// answers from the first declaration alone, and `runtime_declaration` from the unique declaration
/// that runs -- so a reader that claims what the name holds from either must refuse first. The
/// fingerprint side has no reason to report, so refusal there is silent.
pub(in crate::frontend) fn binding_may_differ_from_declaration(
    semantic: &Semantic<'_>,
    symbol: SymbolId,
) -> bool {
    redeclared_at_runtime(semantic, symbol) || semantic.scoping().symbol_is_mutated(symbol)
}

/// Whether a namespace emits code: anything in it but interfaces, type aliases, `declare`
/// statements and namespaces of types only.
fn namespace_holds_values(namespace: &TSNamespaceDeclaration<'_>) -> bool {
    let mut body = &namespace.body;
    let block = loop {
        match body {
            TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => body = &inner.body,
            TSNamespaceDeclarationBody::TSModuleBlock(block) => break block,
        }
    };
    block.body.iter().any(|statement| {
        let declaration = match statement {
            Statement::ExportDeclaration(export) => &export.declaration,
            other => match other.as_declaration() {
                Some(declaration) => declaration,
                None => return true,
            },
        };
        match declaration {
            Declaration::TSInterfaceDeclaration(_) | Declaration::TSTypeAliasDeclaration(_) => {
                false
            }
            Declaration::TSNamespaceDeclaration(inner) => {
                !inner.declare && namespace_holds_values(inner)
            }
            Declaration::VariableDeclaration(variables) => !variables.declare,
            Declaration::FunctionDeclaration(function) => {
                !function.declare && function.body.is_some()
            }
            Declaration::ClassDeclaration(class) => !class.declare,
            Declaration::TSEnumDeclaration(declaration) => {
                !declaration.declare && !declaration.r#const
            }
            _ => true,
        }
    })
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
        /// Every simple target reaches here: `a.b = 1`, `a.b++`, a member inside a destructuring
        /// pattern (`({ mode: a.b } = x)`, `[a.b] = x`) and the head of `for (a.b of xs)`.
        fn visit_simple_assignment_target(
            &mut self,
            it: &oxc_ast::ast::SimpleAssignmentTarget<'a>,
        ) {
            if let Some(member) = it.as_member_expression() {
                self.mark_root(member);
            }
            walk::walk_simple_assignment_target(self, it);
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
    /// The parameter list of the helper whose call inputs are scanned, and the call: following a
    /// binding whose value is unknown there is refused (see [`Scanner::unknown_at_call`]).
    helper_call: Option<(Span, Span)>,
    /// Parameter lists of callbacks handed to a call this analyzer does not read: the callee
    /// chooses what they hold, so following one is refused.
    supplied: Vec<Span>,
    failed: bool,
    /// Whether the scan is of a module's import-time code (see [`Root::ImportTime`]). `vi` and
    /// `expect` used there change what every test in the importing file sees, and no test's
    /// context records it, so they fail the proof.
    import_time: bool,
}

impl<'s, 'a> Scanner<'s, 'a> {
    /// A scanner over `program`, with its import bindings indexed; nothing is queued yet.
    fn new(semantic: &'s Semantic<'a>, program: &Program<'a>, file: &'s Path) -> Self {
        let mut bindings = HashMap::new();
        for statement in &program.body {
            let Statement::ImportDeclaration(import) = statement else {
                continue;
            };
            let source: &str = &string_text(&import.source);
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
            helper_call: None,
            supplied: Vec::new(),
            failed: false,
            import_time: false,
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
                // A re-export of `vi` or `expect` read while a module loads is the same effect as
                // using it there directly.
                Root::Export(name) | Root::Reexport(name) | Root::ExportIfPresent(name) => {
                    if REGISTRATION.contains(&name.as_str())
                        || (self.import_time && matches!(name.as_str(), "vi" | "vitest" | "expect"))
                    {
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
            Resolved::Module(path) => {
                let at_import = self.import_time || matches!(root, Root::ImportTime);
                self.edges.push((path, root, at_import));
            }
            Resolved::Unresolved => self.failed = true,
        }
    }

    /// The edge for an `import` declaration. A declaration whose bindings are never read as values
    /// is erased by the TypeScript transform, so one that names no module on disk (a package
    /// that ships only types) costs nothing; a resolvable one is still loaded.
    fn import_edge(&mut self, import: &'a oxc_ast::ast::ImportDeclaration<'a>) {
        let specifier: &str = &string_text(&import.source);
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
                    self.module_edge(self.file, &string_text(&export.source), Root::ImportTime);
                }
                Statement::ExportAllDeclaration(export)
                    if !matches!(export.export_kind, ImportOrExportKind::Type) =>
                {
                    self.module_edge(self.file, &string_text(&export.source), Root::ImportTime);
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
            if let Some(queued) = self.queue_named(statement, name) {
                found |= queued;
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
                                string_text(&export.source).as_ref(),
                                Root::Reexport(specifier.local.name().to_string()),
                            );
                        }
                    }
                }
                Statement::ExportAllDeclaration(export) => match &export.exported {
                    Some(exported) if exported.name() == name => {
                        found = true;
                        self.module_edge(self.file, &string_text(&export.source), Root::Whole);
                    }
                    Some(_) => {}
                    None => {
                        stars = true;
                        self.module_edge(
                            self.file,
                            string_text(&export.source).as_ref(),
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
            if let Some(queued) = self.queue_named(statement, name) {
                found |= queued;
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
                    self.module_edge(self.file, string_text(&import.source).as_ref(), root);
                }
            }
        }
        if !found {
            self.failed = true;
        }
    }

    /// Queues every declaration, when nothing narrower can be named.
    fn seed_everything(&mut self, program: &'a Program<'a>) {
        self.seed_declarations(&program.body);
        for statement in &program.body {
            if let Statement::ExportDefaultDeclaration(export) = statement {
                self.queue_default(program, &export.declaration);
            }
        }
    }

    /// Queues what `statement` declares under `name`, a merged namespace's members included:
    /// `Some(whether anything was queued)` when it is a declaration, `None` otherwise.
    fn queue_named(&mut self, statement: &'a Statement<'a>, name: &str) -> Option<bool> {
        if self.seed_namespace_named(statement, name) {
            return Some(true);
        }
        declared_by(statement).map(|declared| self.queue_declared(declared, Some(name)))
    }

    /// Queues the members of `statement` when it is a namespace called `name`, which merges into
    /// any function, class or enum of that name. Whether it was one.
    fn seed_namespace_named(&mut self, statement: &'a Statement<'a>, name: &str) -> bool {
        let namespace = match statement {
            Statement::TSNamespaceDeclaration(namespace) => namespace,
            Statement::ExportDeclaration(export) => match &export.declaration {
                Declaration::TSNamespaceDeclaration(namespace) => namespace,
                _ => return false,
            },
            _ => return false,
        };
        if namespace.id.name.as_str() != name {
            return false;
        }
        self.seed_declarations(std::slice::from_ref(statement));
        true
    }

    /// Queues every declaration inside `namespace`, nested namespaces included.
    fn seed_namespace_members(&mut self, namespace: &'a TSNamespaceDeclaration<'a>) {
        let mut body = &namespace.body;
        loop {
            match body {
                TSNamespaceDeclarationBody::TSNamespaceDeclaration(inner) => body = &inner.body,
                TSNamespaceDeclarationBody::TSModuleBlock(block) => {
                    self.seed_declarations(&block.body);
                    return;
                }
            }
        }
    }

    /// Queues every declaration among `statements`, and those inside namespaces, whose members
    /// are reachable as properties of the namespace object.
    fn seed_declarations(&mut self, statements: &'a [Statement<'a>]) {
        for statement in statements {
            if let Some(declared) = declared_by(statement) {
                self.queue_declared(declared, None);
                continue;
            }
            let namespace = match statement {
                Statement::TSNamespaceDeclaration(namespace) => namespace,
                Statement::ExportDeclaration(export) => match &export.declaration {
                    Declaration::TSNamespaceDeclaration(namespace) => namespace,
                    _ => continue,
                },
                _ => continue,
            };
            self.seed_namespace_members(namespace);
        }
    }

    /// Queues the declaration a local symbol stands for, once.
    fn follow_symbol(&mut self, symbol: SymbolId) {
        if !self.seen.insert(symbol) {
            return;
        }
        // The declaration that runs: a redeclaration (`var x = a; var x = b`) has no single one.
        let Some(declaration) = runtime_declaration(self.semantic, symbol) else {
            self.failed = true;
            return;
        };
        if self
            .written
            .is_some_and(|written| written.contains(&symbol))
            || (self.written.is_some() && self.semantic.scoping().symbol_is_mutated(symbol))
        {
            self.failed = true;
            return;
        }
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
            AstKind::FormalParameter(_)
            | AstKind::FormalParameterRest(_)
            | AstKind::CatchParameter(_)
                if self.unknown_at_call(declaration) =>
            {
                self.failed = true;
            }
            AstKind::Function(function) => self.queue.push(Pending::Function(function)),
            AstKind::Class(class) => self.queue.push(Pending::Class(class)),
            // A namespace object carries every member it declares.
            AstKind::TSNamespaceDeclaration(namespace) => self.seed_namespace_members(namespace),
            _ => {}
        }
    }

    /// Whether the parameter or catch binding `declaration` holds a value no declaration shows at
    /// the helper call: the helper's own parameter holds the call's argument (`hook = register()`
    /// runs the caller's function), and one of a function or `catch` the call sits in holds what
    /// that function was called with or what was thrown (`forward(opts) { setup(opts) }`). A
    /// parameter of a callback in [`Scanner::supplied`] holds what the unread callee passes.
    fn unknown_at_call(&self, declaration: NodeId) -> bool {
        let nodes = self.semantic.nodes();
        let owner = nodes.parent_id(declaration);
        if let AstKind::FormalParameters(list) = nodes.get_node(owner).kind() {
            if self.supplied.contains(&list.span) {
                return true;
            }
        }
        let Some((parameters, call)) = self.helper_call else {
            return false;
        };
        let scope = match nodes.get_node(owner).kind() {
            AstKind::FormalParameters(list) if list.span == parameters => return true,
            AstKind::FormalParameters(_) => nodes.parent_kind(owner).span(),
            AstKind::CatchClause(clause) => clause.span,
            _ => return false,
        };
        scope.start <= call.start && call.end <= scope.end
    }

    /// Whether the `this` or `arguments` at `node` belongs to a callback in [`Scanner::supplied`]:
    /// the nearest enclosing `function` is one, so the unread callee chooses its value. An arrow
    /// takes both from where it is written; a class body binds its own `this`.
    fn bound_by_callee(&self, node: NodeId) -> bool {
        for kind in self.semantic.nodes().ancestor_kinds(node) {
            match kind {
                AstKind::Function(function) => {
                    return self.supplied.contains(&function.params.span);
                }
                AstKind::Class(_) => return false,
                _ => {}
            }
        }
        false
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
    /// A callback's `this` holds what the unread callee binds (`forEach(fn, thisArg)`).
    fn visit_this_expression(&mut self, this: &ThisExpression) {
        if self.bound_by_callee(this.node_id.get()) {
            self.failed = true;
        }
    }

    /// Judges a name the scanned code reads: a type-only use is erased, a Vitest export is judged by
    /// name, an import becomes an edge, and a local binding is followed to its declaration.
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
        if free && ident.name == "arguments" && self.bound_by_callee(ident.node_id.get()) {
            self.failed = true;
            return;
        }
        if free && REFLECTIVE.contains(&ident.name.as_str()) {
            self.failed = true;
            return;
        }
        if free && !self.known_global(ident.name.as_str()) {
            return;
        }
        if let Some(name) = self.vitest_name(ident) {
            // At import time any read of `vi` or `expect` counts as using it: through an alias, a
            // computed member, destructuring or an argument it reaches the same effects.
            if REGISTRATION.contains(&name.as_str())
                || name == "vitest"
                || name == "*"
                || (self.import_time && matches!(name.as_str(), "vi" | "vitest" | "expect"))
            {
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

    /// Judges `object.property`: a registration API or reflective access fails the proof, a `vi` or
    /// `expect` member fails it only in import-time code, and a namespace member is one more export
    /// to prove.
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
                // `globalThis.vi` is Vitest's `vi` when globals are enabled.
                if REGISTRATION.contains(&property)
                    || REFLECTIVE.contains(&property)
                    || (self.import_time && matches!(property, "vi" | "vitest" | "expect"))
                {
                    self.failed = true;
                }
                return;
            }
        }
        if let Expression::Identifier(object) = &member.object {
            // `vi` and `expect` mock modules and change global state, which is setup a helper call's
            // identity carries; neither registers a test or a hook. Run while a module is imported,
            // they reach every test in the importing file instead, which no context records.
            if matches!(self.vitest_name(object).as_deref(), Some("vi" | "expect")) {
                if self.import_time {
                    self.failed = true;
                }
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

    /// A dynamic `import('...')` with a literal specifier loads that whole module when it runs;
    /// any other specifier could load anything.
    fn visit_import_expression(&mut self, import: &ImportExpression<'a>) {
        match &import.source {
            Expression::StringLiteral(literal) => {
                let specifier = string_text(literal);
                self.module_edge(self.file, &specifier, Root::Whole);
            }
            _ => self.failed = true,
        }
        walk::walk_import_expression(self, import);
    }
}
