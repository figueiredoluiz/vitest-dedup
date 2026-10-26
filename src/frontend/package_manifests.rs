//! Whether importing an installed package can register Vitest tests.
//!
//! A helper module that imports a package runs that package's code, and a package's body is a
//! build artifact the analyzer never reads. What can be read is its manifest: code that
//! registers Vitest tests has to reach Vitest, so a package whose whole dependency closure names
//! no test runner cannot do it. Reading the closure is bounded and conservative -- a manifest
//! that cannot be read, a dependency that is not installed, or a closure past the cap all mean
//! "cannot tell", which is the refusal this check replaces.
//!
//! Only registration is ruled out. Import-time side effects of a package (patching globals,
//! polyfills) are not modelled, exactly as they are not for a package the test file imports.

use super::extract::registration_free::REGISTRATION;
use super::package_supervisor::supervise;
use crate::frontend::js_text::string_text;
use crate::model::PackageWorkerFailure;
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, CallExpression, Expression, IdentifierReference, StaticMemberExpression,
};
use oxc_ast::AstKind;
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::{GetSpan, SourceType};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use subprocess::Exec;

/// Most packages one closure may visit before it is refused.
const MAX_PACKAGES: usize = 256;

thread_local! {
    /// Verdict per package root, so a barrel's many imports of one package read its closure once.
    static VERDICTS: RefCell<HashMap<PathBuf, bool>> = RefCell::new(HashMap::new());
}

/// Whether `name` is a test runner, or a package that exists to extend one.
fn is_runner(name: &str) -> bool {
    matches!(
        name,
        "vitest" | "jest" | "mocha" | "jasmine" | "playwright" | "playwright-core"
    ) || ["@vitest/", "@jest/", "@playwright/", "vitest-"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// The directory of the installed package that owns `file`: the component after the last
/// `node_modules`, plus a second one for a scope (`@scope/name`).
fn package_root(file: &Path) -> Option<PathBuf> {
    let components: Vec<Component> = file.components().collect();
    let index = components
        .iter()
        .rposition(|component| component.as_os_str() == "node_modules")?;
    let name = components.get(index + 1)?;
    let length = if name.as_os_str().to_string_lossy().starts_with('@') {
        index + 3
    } else {
        index + 2
    };
    (components.len() >= length).then(|| components[..length].iter().collect())
}

/// The root of installed package `name`, searched from `from` the way Node does: a sibling in the
/// enclosing `node_modules`, then `node_modules` of every ancestor directory.
fn find_package(from: &Path, name: &str) -> Option<PathBuf> {
    // A manifest key is repository-controlled text: only `name` or `@scope/name` may reach a path.
    let segment =
        |text: &str| !text.is_empty() && text != "." && text != ".." && !text.contains(['\\', ':']);
    let shaped = match name.split('/').collect::<Vec<_>>().as_slice() {
        [package] => segment(package),
        [scope, package] => scope.starts_with('@') && segment(scope) && segment(package),
        _ => false,
    };
    if !shaped {
        return None;
    }
    from.ancestors().find_map(|directory| {
        let candidate = if directory.file_name().is_some_and(|n| n == "node_modules") {
            directory.join(name)
        } else {
            directory.join("node_modules").join(name)
        };
        std::fs::symlink_metadata(candidate.join("package.json"))
            .map_or_else(
                |error| error.kind() != std::io::ErrorKind::NotFound,
                |_| true,
            )
            .then_some(candidate)
    })
}

/// Declared dependency names of one manifest: `(name, required)`. Optional dependencies and
/// peers marked optional are not required to be installed.
fn dependencies(manifest: &Value) -> Vec<(String, bool)> {
    let table = |field: &str| {
        manifest
            .get(field)
            .and_then(Value::as_object)
            .map(|object| object.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let optional_peer = |name: &str| {
        manifest
            .get("peerDependenciesMeta")
            .and_then(|meta| meta.get(name))
            .and_then(|entry| entry.get("optional"))
            .and_then(Value::as_bool)
            == Some(true)
    };
    let mut names: Vec<(String, bool)> = table("dependencies")
        .into_iter()
        .map(|name| (name, true))
        .collect();
    names.extend(
        table("peerDependencies")
            .into_iter()
            .map(|name| (name.clone(), !optional_peer(&name))),
    );
    names.extend(
        table("optionalDependencies")
            .into_iter()
            .map(|name| (name, false)),
    );
    names
}

/// Reads the dependency closure of the package at `root`.
fn closure_has_no_runner(root: &Path) -> bool {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(package) = pending.pop() {
        if !visited.insert(package.clone()) {
            continue;
        }
        if visited.len() > MAX_PACKAGES {
            return false;
        }
        let Some(manifest) = crate::package_json::read(&package.join("package.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            return false;
        };
        for (name, required) in dependencies(&manifest) {
            if is_runner(&name) {
                return false;
            }
            match find_package(&package, &name) {
                Some(found) => pending.push(found),
                None if required => return false,
                None => {}
            }
        }
    }
    true
}

/// Largest entry file the text check reads; a larger one is "cannot tell".
const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;

/// Most local files one package's entry may reach before the text rule gives up.
const MAX_LOCAL_FILES: usize = 1024;
/// Most bytes those files may hold in all.
const MAX_LOCAL_BYTES: u64 = 32 * 1024 * 1024;

/// Verdicts of the text rule over what an entry reaches, per entry and globals mode, shared by
/// every worker: a package is read once per run, not once per test file that imports it. A run is
/// bounded by [`forget_reached`], which the end of every run calls; extraction is reachable only
/// through a run, so installed files are taken as fixed for its duration.
fn reached_verdicts() -> &'static Mutex<HashMap<(PathBuf, bool), bool>> {
    static VERDICTS: OnceLock<Mutex<HashMap<(PathBuf, bool), bool>>> = OnceLock::new();
    VERDICTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Forgets every verdict of the text rule: at the end of a run, so the next one reads the files as
/// they are then, and in tests that rewrite a file and ask again. Verdicts are process-wide, so a
/// caller that analyses more than one project in a process must call
/// `resolver::take_resolution_gaps` (which calls this) between runs.
pub(crate) fn forget_reached() {
    // The closure verdicts of the calling thread go with them; a worker thread's die with it.
    VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
    reached_verdicts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// One module load found in a script.
#[derive(Debug, PartialEq, Eq)]
enum Load {
    /// A relative specifier, to be followed inside the package.
    Relative(String),
    /// A load whose target the script does not state (a computed specifier, a use of `require`
    /// other than a call with a string), or a script that cannot be parsed.
    Unknown,
}

/// Reads the module loads of a script from its syntax tree: `import` and `export ... from`
/// declarations, `import()`, and calls of `require` (also `module.require`, `require?.()` and
/// `(require)()`). A bare specifier names another package, which the manifest closure covers, and
/// is left out; so is a Node builtin. A specifier that is neither relative nor a package name
/// (`#internal`, absolute, `file:`), carries a `?`, `#` or `%`, or is not a plain string, is an
/// [`Load::Unknown`], as is any other use of the name `require` (an alias, `(0, require)(x)`).
fn scan_script(
    path: &Path,
    text: &str,
    own: Option<&str>,
    guarded: bool,
    globals: bool,
) -> ScriptScan {
    struct Scan<'n> {
        /// The package's own name: a bare load of it resolves inside the package.
        own: Option<&'n str>,
        found: Vec<Load>,
        /// Callee identifiers already read as a call, by span start.
        handled: HashSet<u32>,
    }
    impl Scan<'_> {
        /// Records one written specifier: relative to follow, a package or builtin to skip, else unknown.
        fn specifier(&mut self, specifier: &str) {
            let relative = matches!(specifier, "." | "..")
                || specifier.starts_with("./")
                || specifier.starts_with("../");
            let package_or_builtin = specifier.starts_with("node:")
                || specifier
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_alphanumeric() || first == '@')
                    && !specifier.contains(':');
            let self_reference = self.own.is_some_and(|own| {
                specifier == own
                    || specifier
                        .strip_prefix(own)
                        .is_some_and(|rest| rest.starts_with('/'))
            });
            // `module` builds loaders (`createRequire`, `Module._load`), which are not followed.
            let loader_builtin = matches!(specifier, "module" | "node:module");
            if self_reference
                || loader_builtin
                || specifier.contains(['?', '#', '%', '\\', '\n'])
                || !(relative || package_or_builtin)
            {
                self.found.push(Load::Unknown);
            } else if relative {
                self.found.push(Load::Relative(specifier.to_owned()));
            }
        }

        /// Whether `object` is the `module` object; any other receiver of `.require` is a load
        /// the walk cannot place.
        fn module_object(&mut self, object: &Expression<'_>) -> bool {
            if matches!(object, Expression::Identifier(identifier) if identifier.name == "module") {
                return true;
            }
            self.found.push(Load::Unknown);
            false
        }

        /// The first argument of a load: a string, or a template with no substitution.
        fn argument(&mut self, expression: Option<&Expression<'_>>) {
            match expression {
                Some(Expression::StringLiteral(literal)) => {
                    self.specifier(string_text(literal).as_ref())
                }
                Some(Expression::TemplateLiteral(template)) if template.expressions.is_empty() => {
                    match template.quasis.first() {
                        Some(quasi) => self.specifier(quasi.value.raw.as_str()),
                        None => self.found.push(Load::Unknown),
                    }
                }
                _ => self.found.push(Load::Unknown),
            }
        }
    }
    impl<'a> Visit<'a> for Scan<'_> {
        /// `import { createRequire } from 'module'`: the loader it builds is not followed.
        fn visit_import_specifier(&mut self, it: &oxc_ast::ast::ImportSpecifier<'a>) {
            if it.imported.name() == "createRequire" {
                self.found.push(Load::Unknown);
            }
        }
        /// `import x = require('y')`.
        fn visit_ts_external_module_reference(
            &mut self,
            it: &oxc_ast::ast::TSExternalModuleReference<'a>,
        ) {
            self.specifier(string_text(&it.expression).as_ref());
        }
        /// `import ... from 'x'`.
        fn visit_import_declaration(&mut self, it: &oxc_ast::ast::ImportDeclaration<'a>) {
            self.specifier(string_text(&it.source).as_ref());
            walk::walk_import_declaration(self, it);
        }
        /// `export { a } from 'x'`.
        fn visit_export_from_declaration(&mut self, it: &oxc_ast::ast::ExportFromDeclaration<'a>) {
            self.specifier(string_text(&it.source).as_ref());
        }
        /// `export * from 'x'`.
        fn visit_export_all_declaration(&mut self, it: &oxc_ast::ast::ExportAllDeclaration<'a>) {
            self.specifier(string_text(&it.source).as_ref());
        }
        /// `import('x')`.
        fn visit_import_expression(&mut self, it: &oxc_ast::ast::ImportExpression<'a>) {
            self.argument(Some(&it.source));
            walk::walk_import_expression(self, it);
        }
        /// `require('x')` and `module.require('x')`, however the callee is wrapped.
        fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
            let callee = crate::frontend::registry::unwrap_transparent(&it.callee);
            let loads = match callee {
                Expression::Identifier(identifier) if identifier.name == "require" => {
                    self.handled.insert(identifier.span.start);
                    true
                }
                // `module.require(x)` loads like `require(x)`; `require.main.require(x)` and
                // `module.parent.require(x)` resolve from another module's directory.
                Expression::StaticMemberExpression(member) if member.property.name == "require" => {
                    self.module_object(&member.object)
                }
                Expression::ComputedMemberExpression(member)
                    if matches!(
                        &member.expression,
                        Expression::StringLiteral(literal) if literal.value == "require"
                    ) =>
                {
                    self.module_object(&member.object)
                }
                _ => false,
            };
            if loads {
                self.argument(it.arguments.first().and_then(Argument::as_expression));
            }
            walk::walk_call_expression(self, it);
        }
        /// Skips `typeof require`, which only asks whether it exists.
        fn visit_unary_expression(&mut self, it: &oxc_ast::ast::UnaryExpression<'a>) {
            // `typeof require` only asks whether it exists.
            if it.operator == oxc_ast::ast::UnaryOperator::Typeof
                && matches!(&it.argument, Expression::Identifier(identifier) if identifier.name == "require")
            {
                return;
            }
            walk::walk_unary_expression(self, it);
        }
        /// Skips `require.resolve` and the like, which do not load.
        fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
            if it.property.name == "createRequire" {
                self.found.push(Load::Unknown);
            }
            // `import.meta.glob(...)` imports files by pattern, which are not followed.
            if matches!(&it.object, Expression::ImportMeta(_))
                && matches!(it.property.name.as_str(), "glob" | "globEager")
            {
                self.found.push(Load::Unknown);
            }
            // `require.resolve`, `require.cache` and the like do not load; `require.call(...)`
            // and `require.apply(...)` do.
            if matches!(&it.object, Expression::Identifier(identifier) if identifier.name == "require")
            {
                if !matches!(
                    it.property.name.as_str(),
                    "resolve" | "cache" | "main" | "extensions"
                ) {
                    self.found.push(Load::Unknown);
                }
                return;
            }
            walk::walk_static_member_expression(self, it);
        }
        /// `x['createRequire']`.
        fn visit_computed_member_expression(
            &mut self,
            it: &oxc_ast::ast::ComputedMemberExpression<'a>,
        ) {
            if matches!(&it.expression, Expression::StringLiteral(literal) if literal.value == "createRequire")
            {
                self.found.push(Load::Unknown);
            }
            walk::walk_computed_member_expression(self, it);
        }
        /// Any other mention of `require`, or of `createRequire` (which returns a loader), is a use
        /// the walk cannot follow.
        fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
            if it.name == "createRequire" {
                self.found.push(Load::Unknown);
            }
            if it.name == "require" && !self.handled.contains(&it.span.start) {
                self.found.push(Load::Unknown);
            }
        }
    }
    let unknown = || ScriptScan {
        loads: vec![Load::Unknown],
        registers_global: false,
    };
    if guarded && super::extract::parse_refusal(text).is_some() {
        return unknown();
    }
    let allocator = Allocator::default();
    // `.mjs` and `.cjs` (and their TypeScript forms) say what they are; only the ambiguous
    // extensions are guessed from the syntax.
    let source_type = SourceType::from_path(path).unwrap_or_default();
    let source_type = if matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("mjs" | "cjs" | "mts" | "cts")
    ) {
        source_type
    } else {
        source_type.with_unambiguous(true)
    };
    let parsed = Parser::new(&allocator, text, source_type).parse();
    if parsed.fatal_error || !parsed.diagnostics.is_empty() {
        return unknown();
    }
    let mut scan = Scan {
        own,
        found: Vec::new(),
        handled: HashSet::new(),
    };
    scan.visit_program(&parsed.program);
    ScriptScan {
        loads: scan.found,
        // Only a project that enables globals can lean on them.
        registers_global: globals && reads_registration_global(&parsed.program),
    }
}

/// What one script was found to do.
struct ScriptScan {
    loads: Vec<Load>,
    /// Whether it reads a registration global that no declaration in the file binds.
    registers_global: bool,
}

/// Whether `program` reads one of the [`REGISTRATION`] names as a free name: a reference no
/// binding in the file resolves (so a local `function test` or a parameter named `it` is not one,
/// and neither is a property, a key or a method). Any read counts, whether the name is called,
/// tagged, indexed, parenthesised or stored for later, except under `typeof`; a write alone does
/// not. A property of the global object (`globalThis.test`, also `global`, `window`, `self` when
/// they are not bound in the file) counts as well.
fn reads_registration_global(program: &oxc_ast::ast::Program<'_>) -> bool {
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(program)
        .semantic;
    let scoping = semantic.scoping();
    let nodes = semantic.nodes();
    // A `with` statement looks names up on an object first, so no binding settles a name in it.
    if nodes
        .iter()
        .any(|node| matches!(node.kind(), AstKind::WithStatement(_)))
    {
        return true;
    }
    // `declare const test: ...` binds nothing at run time: the name is still the global.
    if REGISTRATION.iter().any(|name| {
        scoping
            .get_root_binding((*name).into())
            .is_some_and(|symbol| scoping.symbol_flags(symbol).is_ambient())
    }) {
        return true;
    }
    // The node that uses the reference, skipping the parentheses and the TypeScript wrappers
    // (`as`, `!`, `satisfies`, `<T>x`, `x<T>`) around it.
    let user_id = |id: oxc_semantic::NodeId| {
        let mut id = nodes.parent_id(id);
        while matches!(
            nodes.kind(id),
            AstKind::ParenthesizedExpression(_)
                | AstKind::TSAsExpression(_)
                | AstKind::TSSatisfiesExpression(_)
                | AstKind::TSTypeAssertion(_)
                | AstKind::TSNonNullExpression(_)
                | AstKind::TSInstantiationExpression(_)
        ) {
            id = nodes.parent_id(id);
        }
        id
    };
    let user_of = |id: oxc_semantic::NodeId| nodes.kind(user_id(id));
    for (name, references) in scoping.root_unresolved_references() {
        let name = name.as_str();
        let registration = REGISTRATION.contains(&name);
        if !registration && !matches!(name, "globalThis" | "global" | "window" | "self") {
            continue;
        }
        for id in references.iter() {
            let reference = scoping.get_reference(*id);
            if !reference.is_read() {
                continue;
            }
            let user = user_of(reference.node_id());
            let counts = if registration {
                !matches!(user, AstKind::UnaryExpression(unary)
                    if unary.operator == oxc_ast::ast::UnaryOperator::Typeof)
            } else {
                let (property, span) = match user {
                    AstKind::StaticMemberExpression(member) => (
                        Some(std::borrow::Cow::Borrowed(member.property.name.as_str())),
                        member.span,
                    ),
                    AstKind::ComputedMemberExpression(member) => match &member.expression {
                        Expression::StringLiteral(literal) => {
                            (Some(string_text(literal)), member.span)
                        }
                        _ => (None, member.span),
                    },
                    _ => (None, oxc_span::Span::default()),
                };
                // `globalThis.test = handler` only sets the property; `+=` and `++` read it first.
                let assigned = property.is_some()
                    && matches!(
                        user_of(user_id(reference.node_id())),
                        AstKind::AssignmentExpression(assign)
                            if assign.operator == oxc_ast::ast::AssignmentOperator::Assign
                                && assign.left.span() == span
                    );
                !assigned
                    && property.is_some_and(|property| REGISTRATION.contains(&property.as_ref()))
            };
            if counts {
                return true;
            }
        }
    }
    false
}

/// `path` with `.` and `..` folded away, without touching the file system.
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// What following one relative load came to.
#[derive(Debug, PartialEq, Eq)]
enum Followed {
    /// The files a load may name, by real path: every candidate that exists, since the loader
    /// that runs the package (Node, or the bundler that inlines it) is not known.
    Scripts(Vec<PathBuf>),
    /// A target that cannot be read as part of the package (it leaves the root, or a manifest
    /// that steers the load cannot be read): the answer is "cannot tell".
    Unfollowable,
}

/// Extensions tried, in order, for a load written without one. Vite resolves the TypeScript and
/// JSX forms as well as Node's three.
const SCRIPT_EXTENSIONS: [&str; 8] = ["js", "mjs", "cjs", "ts", "mts", "cts", "tsx", "jsx"];

/// Extensions of data and binary assets that run nothing. Native code (`.node`, `.wasm`) is
/// separate; any other extension is run as JavaScript by Node's `require`, so it is read as a
/// script.
const NOT_SCRIPTS: &[&str] = &[
    "json", "css", "scss", "sass", "less", "svg", "png", "jpg", "jpeg", "gif", "webp", "ico",
    "woff", "woff2", "ttf", "otf",
];

/// What a reached file is, by extension.
#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Script,
    /// Parsed as data or served as an asset: runs nothing and loads nothing.
    Data,
    /// Native code (`.node`, `.wasm`): a WebAssembly module can import and run JavaScript.
    Binary,
}

/// Classifies `path` by its extension, case-sensitively as Node's extension dispatch is: `.JSON`
/// is run as a script.
fn kind_of(path: &Path) -> Kind {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("node" | "wasm") => Kind::Binary,
        Some(extension) if NOT_SCRIPTS.contains(&extension) => Kind::Data,
        _ => Kind::Script,
    }
}

/// Follows the relative `specifier` written in `from`: every file it can name, then a directory
/// through its manifest's `main` and its index. All in real paths: `from` and `real_root` are
/// canonical, and a target whose real path leaves the root is unfollowable, so a link cannot take
/// the walk out of the package.
fn follow(from: &Path, real_root: &Path, specifier: &str) -> Followed {
    let Some(directory) = from.parent() else {
        return Followed::Unfollowable;
    };
    let base = normalized(&directory.join(specifier));
    if !base.starts_with(real_root) {
        return Followed::Unfollowable;
    }
    // A trailing slash, `.` or `..` can only name a directory.
    let directory_only = specifier.ends_with('/')
        || specifier.ends_with("/.")
        || specifier.ends_with("/..")
        || matches!(specifier, "." | "..");
    let mut found = Found::default();
    // A load that names nothing on disk cannot be judged: the loader that runs it may find it.
    match resolve_target(&base, real_root, directory_only, true, &mut found) {
        true if found.existed => Followed::Scripts(found.scripts),
        _ => Followed::Unfollowable,
    }
}

/// What following one load found.
#[derive(Default)]
struct Found {
    /// Scripts, by real path.
    scripts: Vec<PathBuf>,
    /// Whether any file or directory the load names exists, data files included.
    existed: bool,
}

/// Adds the scripts `file` names to `found`, by real path. False when it leaves the package.
fn add_script(file: &Path, real_root: &Path, found: &mut Found) -> bool {
    let Ok(real) = file.canonicalize() else {
        return false;
    };
    if !real.starts_with(real_root) {
        return false;
    }
    found.existed = true;
    if !found.scripts.contains(&real) {
        found.scripts.push(real);
    }
    true
}

/// Collects every script `base` may name: files as written or with a script extension (a `.js`
/// also names its TypeScript twin, as Vite resolves it), and, if `base` is a directory, its
/// manifest's `main` (when `use_manifest`) and its index. A `main` target is resolved without a
/// manifest of its own, as Node does. False when a candidate leaves the package or a manifest
/// cannot be read.
fn resolve_target(
    base: &Path,
    real_root: &Path,
    directory_only: bool,
    use_manifest: bool,
    found: &mut Found,
) -> bool {
    if !directory_only {
        let mut candidates = vec![base.to_path_buf()];
        for extension in SCRIPT_EXTENSIONS {
            let mut named = base.to_path_buf().into_os_string();
            named.push(format!(".{extension}"));
            candidates.push(PathBuf::from(named));
        }
        // Node resolves `./settings` to `settings.json`: data, found but not scanned.
        let mut json = base.to_path_buf().into_os_string();
        json.push(".json");
        candidates.push(PathBuf::from(json));
        if let Some(stem) = base.to_str().and_then(|text| {
            ["js", "mjs", "cjs", "jsx"]
                .iter()
                .find_map(|extension| text.strip_suffix(&format!(".{extension}")))
        }) {
            for extension in ["ts", "mts", "cts", "tsx"] {
                candidates.push(PathBuf::from(format!("{stem}.{extension}")));
            }
        }
        for candidate in candidates
            .into_iter()
            .filter(|candidate| candidate.is_file())
        {
            if !add_script(&candidate, real_root, found) {
                return false;
            }
        }
    }
    if !base.is_dir() {
        return true;
    }
    found.existed = true;
    let manifest = base.join("package.json");
    if use_manifest
        && std::fs::symlink_metadata(&manifest).map_or_else(
            |error| error.kind() != std::io::ErrorKind::NotFound,
            |_| true,
        )
    {
        let Some(value) = crate::package_json::read(&manifest)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            return false;
        };
        // Fields that route a load elsewhere than `main` and the index are not followed.
        if value.get("exports").is_some() || value.get("browser").is_some() {
            return false;
        }
        // A bundler that inlines the package may take `module` over `main`.
        for field in ["main", "module"] {
            let Some(main) = value.get(field).and_then(Value::as_str) else {
                continue;
            };
            let target = normalized(&base.join(main));
            // A directory target with a manifest of its own would be routed by it.
            if !target.starts_with(real_root)
                || std::fs::symlink_metadata(target.join("package.json")).is_ok()
                || !resolve_target(&target, real_root, false, false, found)
            {
                return false;
            }
        }
    }
    for extension in SCRIPT_EXTENSIONS {
        let index = base.join(format!("index.{extension}"));
        if index.is_file() && !add_script(&index, real_root, found) {
            return false;
        }
    }
    true
}

/// Whether package scans run in a child process (see [`use_package_worker`]).
static WORKER_MODE: AtomicBool = AtomicBool::new(false);

/// The argument that starts the binary as a package scan worker.
const WORKER_ARG: &str = "--vitest-dedup-package-worker";

/// Set in a worker's environment: a worker never starts another, so an embedding program that
/// forgot [`run_package_worker`] cannot recurse.
const WORKER_ENV: &str = "VITEST_DEDUP_PACKAGE_WORKER";

/// Longest a worker may run before it is killed and the package refused.
const WORKER_DEADLINE: Duration = Duration::from_secs(20);

/// Stack of the thread that parses in a worker: ample for deeply nested but valid code. A stack
/// overflow past it aborts the worker, never the analyzer.
const WORKER_STACK: usize = 64 * 1024 * 1024;

/// Runs the installed-package scan in a child process from now on.
///
/// A parser stack overflow aborts the whole process and `oxc_parser` has no recursion limit, so
/// by default (library callers, tests) a script that trips the parser's raw syntax size
/// budget (36,864 units) is refused unread. The command-line tool calls this instead: the scan then runs
/// in a worker started from the current executable, without the budget, and a worker that
/// crashes, times out, cannot be started or answers anything but a complete verdict refuses the
/// package. There is no in-process fallback once this is on. The embedding executable must call
/// [`run_package_worker`] first thing in `main`.
pub fn use_package_worker() {
    WORKER_MODE.store(true, Ordering::Relaxed);
}

/// Whether this process may start a worker: not if it is one.
fn may_start_worker() -> bool {
    std::env::var_os(WORKER_ENV).is_none()
}

/// Failure counts describe uncached attempts, not affected tests or cache hits.
static WORKER_FAILURES: Mutex<BTreeMap<PackageWorkerFailure, usize>> = Mutex::new(BTreeMap::new());

/// Drain worker failures once per analysis. Like resolution gaps, simultaneous analyses in
/// one process share these counters; library callers drain them between runs.
pub fn take_package_worker_failures() -> BTreeMap<PackageWorkerFailure, usize> {
    std::mem::take(
        &mut *WORKER_FAILURES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Valid refusal and clean responses are distinct from infrastructure or protocol failures.
fn registers_from_worker(
    output: Result<String, PackageWorkerFailure>,
) -> Result<bool, PackageWorkerFailure> {
    match output?.as_str() {
        "clean\n" => Ok(false),
        "registers\n" => Ok(true),
        _ => Err(PackageWorkerFailure::Output),
    }
}

/// Refuse a failed attempt and retain its fixed, path-free cause for the run notice.
fn record_worker_result(result: Result<bool, PackageWorkerFailure>) -> bool {
    result.unwrap_or_else(|cause| {
        let mut failures = WORKER_FAILURES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *failures.entry(cause).or_default() += 1;
        true
    })
}

/// Ask a supervised worker whether this package can register tests.
fn scan_in_worker(file: &Path, globals: bool) -> bool {
    let answer = if !may_start_worker() {
        Err(PackageWorkerFailure::Nested)
    } else if let Ok(exe) = std::env::current_exe() {
        let command = Exec::cmd(exe)
            .env(WORKER_ENV, "1")
            .arg(WORKER_ARG)
            .arg(file)
            .arg(if globals { "1" } else { "0" });
        registers_from_worker(supervise(command, WORKER_DEADLINE))
    } else {
        Err(PackageWorkerFailure::Start)
    };
    record_worker_result(answer)
}

/// The worker side: `Some(exit code)` when this process was started as a package scan worker, so
/// `main` can return it before any other work.
pub fn run_package_worker(args: &[OsString]) -> Option<i32> {
    if args.get(1).and_then(|argument| argument.to_str()) != Some(WORKER_ARG) {
        return None;
    }
    // Only the supervisor starts a worker: it marks the environment and passes exactly a file and
    // the globals flag.
    let (Some(file), Some(globals), 4, true) = (
        args.get(2),
        args.get(3).and_then(|globals| globals.to_str()),
        args.len(),
        !may_start_worker(),
    ) else {
        return Some(2);
    };
    if !matches!(globals, "0" | "1") {
        return Some(2);
    }
    // A worker that outlives its deadline (its parent may be gone) ends itself.
    std::thread::spawn(|| {
        std::thread::sleep(WORKER_DEADLINE + Duration::from_secs(5));
        std::process::exit(3);
    });
    let file = PathBuf::from(file);
    let globals = globals == "1";
    let registers = std::thread::Builder::new()
        .stack_size(WORKER_STACK)
        .spawn(move || reached_files_register(&file, globals, false));
    let Ok(scan) = registers else {
        return Some(4);
    };
    let registers = scan.join().ok();
    match registers {
        Some(registers) => {
            println!("{}", if registers { "registers" } else { "clean" });
            Some(0)
        }
        None => Some(5),
    }
}

/// Whether the entry file, or a script of the same package it loads by a relative path, names
/// Vitest anywhere in its text, or -- when the project enables globals, which a package can lean
/// on without importing anything -- calls a registration global. A package that registers tests
/// without declaring Vitest as a dependency is caught here. Past the file and size caps, or at a
/// file too large to read, the answer is "cannot tell", which is the refusal.
fn entry_registers(file: &Path, globals: bool) -> bool {
    let key = (file.to_path_buf(), globals);
    if let Some(known) = reached_verdicts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .copied()
    {
        return known;
    }
    let registers = if WORKER_MODE.load(Ordering::Relaxed) {
        scan_in_worker(file, globals)
    } else {
        reached_files_register(file, globals, true)
    };
    reached_verdicts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, registers);
    registers
}

/// Whether any script reachable from `entry` registers tests, or the walk cannot tell.
fn reached_files_register(entry: &Path, globals: bool, guarded: bool) -> bool {
    // Everything below is in real paths, so a link inside the package neither hides a file nor
    // changes which directory a later relative load resolves from.
    let Ok(real_entry) = entry.canonicalize() else {
        return true;
    };
    let Some(real_root) = package_root(&real_entry) else {
        return true;
    };
    let Some(manifest) = crate::package_json::read(&real_root.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    else {
        return true;
    };
    let own = manifest.get("name").and_then(Value::as_str);
    let mut pending = vec![real_entry];
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut bytes = 0u64;
    while let Some(file) = pending.pop() {
        if !seen.insert(file.clone()) {
            continue;
        }
        let Some(length) = std::fs::metadata(&file)
            .ok()
            .map(|metadata| metadata.len())
            .filter(|length| *length <= MAX_ENTRY_BYTES)
        else {
            return true;
        };
        bytes += length;
        if seen.len() > MAX_LOCAL_FILES || bytes > MAX_LOCAL_BYTES {
            return true;
        }
        let Ok(raw) = std::fs::read(&file) else {
            return true;
        };
        let text = String::from_utf8_lossy(&raw);
        if text.contains("vitest") {
            return true;
        }
        let kind = kind_of(&file);
        // Native code cannot be read.
        if kind == Kind::Binary {
            return true;
        }
        // Data and assets are inert to the bundler that serves them (Vite), so they are not parsed
        // or followed; Node's `require` would run an unknown extension as JavaScript.
        if kind == Kind::Data {
            continue;
        }
        let scan = scan_script(&file, &text, own, guarded, globals);
        if globals && scan.registers_global {
            return true;
        }
        for load in scan.loads {
            match load {
                Load::Unknown => return true,
                Load::Relative(specifier) => match follow(&file, &real_root, &specifier) {
                    Followed::Scripts(next) => pending.extend(next),
                    Followed::Unfollowable => return true,
                },
            }
        }
    }
    false
}

/// Whether the installed package that owns `file` provably cannot register Vitest tests: no
/// runner anywhere in its manifest closure, and none named in the file imported. `globals` is
/// whether the project enables Vitest globals.
pub(crate) fn cannot_register_tests(file: &Path, globals: bool) -> bool {
    let Some(root) = package_root(file) else {
        return false;
    };
    let closure = VERDICTS.with(|verdicts| verdicts.borrow().get(&root).copied());
    let closure = closure.unwrap_or_else(|| {
        let verdict = closure_has_no_runner(&root);
        VERDICTS.with(|verdicts| verdicts.borrow_mut().insert(root, verdict));
        verdict
    });
    closure && !entry_registers(file, globals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Refused manifests taint closure roots, installed optional peers and nested entry routing.
    #[test]
    fn refused_manifests_never_become_clean_package_proofs() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "owned", r#"{"name":"owned"}"#);
        let oversized = " ".repeat(crate::package_json::MAX_BYTES as usize + 1);
        assert!(closure_has_no_runner(&package));
        fs::write(package.join("package.json"), &oversized).unwrap();
        assert!(!closure_has_no_runner(&package));
        assert!(reached_files_register(
            &package.join("index.js"),
            false,
            true
        ));
        fs::write(
            package.join("package.json"),
            r#"{"optionalDependencies":{"peer":"1"}}"#,
        )
        .unwrap();
        assert!(closure_has_no_runner(&package));
        let peer = install(root, "peer", &oversized);
        assert!(!closure_has_no_runner(&package));
        fs::remove_file(peer.join("package.json")).unwrap();
        fs::create_dir(peer.join("package.json")).unwrap();
        assert!(!closure_has_no_runner(&package));
        fs::write(package.join("package.json"), "{}").unwrap();
        write_file(&package, "sub/package.json", &oversized);
        write_file(&package, "sub/index.js", "");
        write_file(&package, "index.js", "import './sub';");
        assert!(reached_files_register(
            &package.join("index.js"),
            false,
            true
        ));
        fs::write(package.join("sub/package.json"), "{}").unwrap();
        assert!(!reached_files_register(
            &package.join("index.js"),
            false,
            true
        ));
    }

    /// Writes `node_modules/<name>/package.json` under `root` and returns the package directory.
    fn install(root: &Path, name: &str, manifest: &str) -> PathBuf {
        let directory = root.join("node_modules").join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("package.json"), manifest).unwrap();
        fs::write(directory.join("index.js"), "").unwrap();
        directory
    }

    /// Writes `text` at `name` inside `package`, creating directories.
    fn write_file(package: &Path, name: &str, text: &str) {
        let path = package.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// Makes `entry` the package's `index.js` and asserts whether the package registers tests.
    fn assert_entry(package: &Path, entry: &str, globals: bool, registers: bool) {
        write_file(package, "index.js", entry);
        forget_reached();
        VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
        assert_eq!(
            cannot_register_tests(&package.join("index.js"), globals),
            !registers,
            "{entry} (globals {globals})"
        );
    }

    /// The package's verdict with every cache cleared.
    fn verdict(package: &Path) -> bool {
        VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
        forget_reached();
        cannot_register_tests(&package.join("index.js"), false)
    }

    /// A package with no runner anywhere in its closure is admitted; one that names a runner
    /// directly, as a peer, or through a dependency is not.
    #[test]
    fn a_runner_anywhere_in_the_closure_refuses_the_package() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let clean = install(root, "clean", r#"{"dependencies":{"leaf":"1"}}"#);
        install(root, "leaf", r#"{"name":"leaf"}"#);
        assert!(verdict(&clean));

        let direct = install(root, "direct", r#"{"dependencies":{"vitest":"1"}}"#);
        assert!(!verdict(&direct));
        let peer = install(root, "peer", r#"{"peerDependencies":{"vitest":"*"}}"#);
        assert!(!verdict(&peer));
        let scoped = install(root, "scoped", r#"{"dependencies":{"@vitest/spy":"1"}}"#);
        assert!(!verdict(&scoped));
        let indirect = install(root, "indirect", r#"{"dependencies":{"wrapper":"1"}}"#);
        install(root, "wrapper", r#"{"peerDependencies":{"vitest":"*"}}"#);
        assert!(!verdict(&indirect));
    }

    /// Anything unreadable is "cannot tell": a missing required dependency, a broken manifest,
    /// a file outside any package. A missing optional dependency is not.
    #[test]
    fn what_cannot_be_read_is_refused_and_optional_gaps_are_not() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let missing = install(root, "missing", r#"{"dependencies":{"absent":"1"}}"#);
        assert!(!verdict(&missing));
        let broken = install(root, "broken", "{ not json");
        assert!(!verdict(&broken));
        let optional = install(
            root,
            "optional",
            r#"{"optionalDependencies":{"absent":"1"},"peerDependencies":{"gone":"1"},
                "peerDependenciesMeta":{"gone":{"optional":true}}}"#,
        );
        assert!(verdict(&optional));
        assert!(!cannot_register_tests(&root.join("src/app.ts"), false));
    }

    /// Scoped packages are rooted at `@scope/name`, and a package that is its own dependency
    /// cycle terminates.
    #[test]
    fn scopes_and_cycles_are_handled() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let scoped = install(root, "@org/tool", r#"{"dependencies":{"@org/tool":"1"}}"#);
        assert!(verdict(&scoped));
        assert_eq!(package_root(&scoped.join("dist/index.js")).unwrap(), scoped);
    }

    /// A package that names Vitest in the file imported is refused whatever its manifest says.
    #[test]
    fn an_entry_that_names_vitest_is_refused_despite_a_clean_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "undeclared", r#"{"name":"undeclared"}"#);
        assert!(verdict(&package));
        fs::write(
            package.join("index.js"),
            "import { test } from 'vitest'\nexport const run = () => test('a', () => {})",
        )
        .unwrap();
        assert!(!verdict(&package));
        assert!(!cannot_register_tests(&package.join("missing.js"), false));
    }

    /// Under globals a package can register without importing anything; a registration global called
    /// as a free name in the entry refuses it, a property of that name does not.
    #[test]
    fn an_entry_that_calls_a_registration_global_is_refused_under_globals() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "lean", r#"{"name":"lean"}"#);
        for (source, registers) in [
            ("export const run = () => test('a', () => {})", true),
            (
                "export const run = () => describe.each([1])('a', () => {})",
                true,
            ),
            ("export const run = () => { beforeEach(() => {}) }", true),
            ("export const run = (re) => re.test('a')", false),
            (
                "export const attest = () => 1; export const latest = (x) => x",
                false,
            ),
            ("export const run = () => detest(1)", false),
            (
                "// do not change it.\n/* test( */ export const run = () => 'describe(x)'",
                false,
            ),
            ("export const run = () => it.skip('a', () => {})", true),
            ("export const items = (it) => it.map((x) => x)", false),
            ("export default { describe(path) { return path } }", false),
            (
                "function test(value) { return value }\nexport const run = () => 1",
                false,
            ),
            ("test('a', () => {})\n{ const x = 1 }", true),
            ("exports.run = () => test ('hidden', () => {})", true),
            ("exports.run = () => `${test('hidden', () => {})}`", true),
            ("exports.run = () => `plain test('x') text`", false),
            (
                "const q = /'/; exports.run = () => test('x', () => {})",
                true,
            ),
            (
                "const q = /[\"'`]/; exports.run = () => describe('x', () => {})",
                true,
            ),
            (
                "const half = \"a\" / 2; exports.run = () => test('x', () => {}); const t = 4 / 2",
                true,
            ),
            (
                "const half = `${n}px` / 2; exports.run = () => test('x', () => {}); const t = 4 / 2",
                true,
            ),
            ("exports.run = (a, b) => a / 2 + b / 3; /* it( */", false),
            ("exports.run = () => { return /it\\(/.test('x') }", false),
        ] {
            fs::write(package.join("index.js"), source).unwrap();
            forget_reached();
            VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
            assert_eq!(
                cannot_register_tests(&package.join("index.js"), true),
                !registers,
                "{source}"
            );
            assert!(
                cannot_register_tests(&package.join("index.js"), false),
                "{source}"
            );
        }
    }

    /// A dependency key is text from the repository: one that is not a package name never reaches
    /// the file system, and counts as a dependency that is not installed.
    #[test]
    fn dependency_names_that_are_paths_are_not_followed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        install(root, "elsewhere", r#"{"name":"elsewhere"}"#);
        // A clean manifest outside `node_modules`, which only a traversing key would reach.
        fs::create_dir_all(root.join("outside")).unwrap();
        fs::write(root.join("outside/package.json"), r#"{"name":"outside"}"#).unwrap();
        for key in [
            "../outside",
            "../elsewhere",
            "/etc/passwd",
            "a/b/c",
            "@scope",
            "..",
            "C:\\x",
            "a\\b",
        ] {
            let package = install(
                root,
                "holder",
                &format!(r#"{{"dependencies":{{"{key}":"1"}}}}"#),
            );
            assert!(!verdict(&package), "{key}");
        }
        let package = install(root, "good", r#"{"dependencies":{"elsewhere":"1"}}"#);
        assert!(verdict(&package));
    }

    /// A package that registers from a file other than the one imported is refused, however that
    /// file is reached; files outside the package and files that do not exist are not followed.
    #[test]
    fn local_files_an_entry_reaches_are_judged_too() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "split", r#"{"name":"split"}"#);
        let write = |name: &str, text: &str| write_file(&package, name, text);
        write(
            "registrar.js",
            "const { test } = require('vitest'); exports.go = () => test('a', () => {})",
        );
        write("globals.js", "exports.go = () => describe('a', () => {})");
        write("lib/index.js", "exports.go = () => test('a', () => {})");
        write("clean.js", "exports.go = () => 1");

        fs::write(root.join("node_modules/outside.js"), "test('a', () => {})").unwrap();
        for (entry, globals, registers) in [
            ("exports.go = require('./registrar').go", false, true),
            ("module.exports = require('./registrar.js')", false, true),
            ("export * from './registrar'", false, true),
            ("export { go } from './registrar.js'", false, true),
            ("import('./registrar')", false, true),
            ("import './registrar'", false, true),
            ("exports.go = require('./globals').go", false, false),
            ("exports.go = require('./globals').go", true, true),
            ("exports.go = require('./lib').go", true, true),
            ("exports.go = require('./clean').go", true, false),
            ("exports.go = require('../outside').go", true, true),
            ("exports.go = require('./absent').go", true, true),
            ("exports.go = require('lodash').go", true, false),
            ("// require('./registrar')\nexports.go = 1", false, false),
            ("exports.go = require /*c*/ ('./registrar')", false, true),
            ("exports.go = require\n('./registrar')", false, true),
            ("exports.go = require?.('./registrar')", false, true),
            ("exports.go = (0, require)('./registrar')", false, true),
            ("exports.go = typeof require", false, false),
            ("exports.go = require.resolve('./registrar')", false, false),
        ] {
            assert_entry(&package, entry, globals, registers);
        }
    }

    /// A link out of the package cannot be followed, which refuses the package.
    /// A manifest that starts naming a runner is seen once the run's verdicts are forgotten.
    #[test]
    fn closure_verdicts_are_forgotten_with_the_run() {
        let directory = tempfile::tempdir().unwrap();
        let package = install(directory.path(), "stale", r#"{"name":"stale"}"#);
        assert!(verdict(&package));
        fs::write(
            package.join("package.json"),
            r#"{"name":"stale","dependencies":{"vitest":"1"}}"#,
        )
        .unwrap();
        forget_reached();
        assert!(!cannot_register_tests(&package.join("index.js"), false));
    }

    /// Without globals the registration-global pass does not run, and the loads are still read.
    #[test]
    fn registration_globals_are_only_looked_for_under_globals() {
        let code = "test('a', () => {}); require('./x')";
        let on = scan_script(Path::new("x.js"), code, None, false, true);
        let off = scan_script(Path::new("x.js"), code, None, false, false);
        assert!(on.registers_global);
        assert!(!off.registers_global);
        assert_eq!(on.loads, off.loads);
        assert_eq!(off.loads, vec![Load::Relative("./x".to_owned())]);
    }

    /// A large flat bundle trips the in-process size budget and is refused unread; the unguarded
    /// scan a worker runs reads it, and still refuses a bundle that registers a test.
    #[test]
    fn the_size_budget_applies_in_process_but_not_in_a_worker() {
        let directory = tempfile::tempdir().unwrap();
        let package = install(directory.path(), "bundle", r#"{"name":"bundle"}"#);
        let entry = package.join("index.js");
        let flat = "x();\n".repeat(40_000);
        fs::write(&entry, &flat).unwrap();
        assert!(reached_files_register(&entry, true, true));
        assert!(!reached_files_register(&entry, true, false));
        fs::write(&entry, format!("{flat}describe('a', () => {{}})")).unwrap();
        assert!(reached_files_register(&entry, true, false));
    }

    /// The dispatch leaves an ordinary command line alone and answers a malformed worker call with
    /// a failure code; a process marked as a worker never starts another.
    #[test]
    fn worker_dispatch_ignores_ordinary_arguments() {
        let args = |items: &[&str]| items.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(run_package_worker(&args(&["vitest-dedup"])), None);
        assert_eq!(run_package_worker(&args(&["vitest-dedup", "."])), None);
        assert_eq!(run_package_worker(&args(&[])), None);
        assert_eq!(
            run_package_worker(&args(&["vitest-dedup", WORKER_ARG])),
            Some(2)
        );
        assert_eq!(
            run_package_worker(&args(&["vitest-dedup", WORKER_ARG, "/x"])),
            Some(2)
        );
        // Well-formed operands without the supervisor's marker, and malformed ones with it.
        assert_eq!(
            run_package_worker(&args(&["vitest-dedup", WORKER_ARG, "/x", "1"])),
            Some(2)
        );
        // The one test that sets the marker, so no other test sees it.
        assert!(may_start_worker());
        std::env::set_var(WORKER_ENV, "1");
        let marked_may_start = may_start_worker();
        let malformed = [
            run_package_worker(&args(&["vitest-dedup", WORKER_ARG, "/x", "2"])),
            run_package_worker(&args(&["vitest-dedup", WORKER_ARG, "/x", "1", "extra"])),
        ];
        std::env::remove_var(WORKER_ENV);
        assert!(
            !marked_may_start,
            "a marked process must not start a worker"
        );
        assert_eq!(malformed, [Some(2), Some(2)]);
    }

    /// Clean and refusal responses are valid; all other bytes are infrastructure failures.
    #[test]
    fn only_a_complete_clean_answer_admits_a_package() {
        assert_eq!(registers_from_worker(Ok("clean\n".into())), Ok(false));
        assert_eq!(registers_from_worker(Ok("registers\n".into())), Ok(true));
        for output in ["", "clean", "clean\nclean\n", "clean \n"] {
            assert_eq!(
                registers_from_worker(Ok(output.into())),
                Err(PackageWorkerFailure::Output)
            );
        }
        assert_eq!(
            registers_from_worker(Err(PackageWorkerFailure::Start)),
            Err(PackageWorkerFailure::Start)
        );
    }

    /// Successful verdicts do not count as worker failures, and draining resets the run.
    #[test]
    fn worker_failures_are_counted_and_drained() {
        assert!(take_package_worker_failures().is_empty());
        assert!(!record_worker_result(Ok(false)));
        assert!(record_worker_result(Ok(true)));
        assert!(take_package_worker_failures().is_empty());
        assert!(record_worker_result(Err(PackageWorkerFailure::Start)));
        assert!(record_worker_result(Err(PackageWorkerFailure::Start)));
        assert!(record_worker_result(Err(PackageWorkerFailure::Timeout)));
        assert_eq!(
            take_package_worker_failures(),
            BTreeMap::from([
                (PackageWorkerFailure::Start, 2),
                (PackageWorkerFailure::Timeout, 1),
            ])
        );
        assert!(take_package_worker_failures().is_empty());
    }

    /// A registration global is read as a free name however the call is spelled, and only then:
    /// a binding in the file, a property, a key or a method named the same is not the global.
    #[test]
    fn registration_globals_are_found_by_reference() {
        let reads = |code: &str, file: &str| {
            let scan = scan_script(Path::new(file), code, None, false, true);
            assert!(
                !scan.loads.contains(&Load::Unknown),
                "{code:?} did not parse"
            );
            scan.registers_global
        };
        for (code, file, registers) in [
            // Every spelling of a call.
            ("test('a', () => {})", "x.js", true),
            ("test\n('a', () => {})", "x.js", true),
            ("test /* c */ ('a', () => {})", "x.js", true),
            ("test?.('a', () => {})", "x.js", true),
            ("test`a${1}`", "x.js", true),
            ("describe\n.each([1])('a', () => {})", "x.js", true),
            ("describe?.each([1])('a', () => {})", "x.js", true),
            ("test['each']([1])('a', () => {})", "x.js", true),
            ("describe<Ctx>('a', () => {})", "x.ts", true),
            ("(test)('a', () => {})", "x.js", true),
            ("(test)?.('a', () => {})", "x.js", true),
            ("(0, test)('a', () => {})", "x.js", true),
            ("[...test('a')]", "x.js", true),
            ("beforeEach\n(() => {})", "x.js", true),
            // Inside a template interpolation, however it is written.
            ("const s = `${test /* c */ ('a')}`", "x.js", true),
            ("const s = `${'}' + test('a')}`", "x.js", true),
            // The global object.
            ("globalThis.test('a', () => {})", "x.js", true),
            ("(globalThis).test('a', () => {})", "x.js", true),
            ("(globalThis.test)?.('a')", "x.js", true),
            (
                "globalThis?.describe.each([1])('a', () => {})",
                "x.js",
                true,
            ),
            ("window\n.it('a', () => {})", "x.js", true),
            ("self.test`a`", "x.js", true),
            ("globalThis['test']('a')", "x.js", true),
            // Stored for later.
            ("const t = test; t('a')", "x.js", true),
            ("[test].forEach((f) => f('a'))", "x.js", true),
            ("test++", "x.js", true),
            ("globalThis.test += 1", "x.js", true),
            ("globalThis.test++", "x.js", true),
            ("globalThis.test = handler", "x.js", false),
            ("(globalThis as any).test = handler", "x.ts", false),
            ("globalThis['test'] = handler", "x.js", false),
            ("globalThis.test = globalThis.test", "x.js", true),
            ("{ function test() {} }\ntest('a')", "x.mjs", true),
            (
                "declare const test: (name: string, fn: () => void) => void\ntest('a', () => {})",
                "x.ts",
                true,
            ),
            (
                "declare function describe(name: string): void\ndescribe('a')",
                "x.ts",
                true,
            ),
            (
                "function test() {}\nwith (globalThis) { test('a') }",
                "x.js",
                true,
            ),
            ("(test as any)('a')", "x.ts", true),
            ("(globalThis as any).test('a')", "x.ts", true),
            ("(test!)('a')", "x.ts", true),
            // Not the global.
            ("typeof test", "x.js", false),
            ("type T = typeof test", "x.ts", false),
            ("typeof (test as any) === 'function'", "x.ts", false),
            ("typeof (test!) === 'function'", "x.ts", false),
            ("{ function test() {} }\ntest('a')", "x.cjs", false),
            ("var test = function () {}; test('a')", "x.js", false),
            ("var self = this; self.test('a')", "x.js", false),
            // Known limit: a name taken from the global object is a local binding afterwards.
            ("const { test } = globalThis; test('a')", "x.js", false),
            ("typeof test === 'function'", "x.js", false),
            ("test = 1", "x.js", false),
            ("pattern.test('a')", "x.js", false),
            ("pattern?.test('a')", "x.js", false),
            ("a.window.test('a')", "x.js", false),
            ("localThis.test('a')", "x.js", false),
            ("const self = this; self.test('a')", "x.js", false),
            ("function test(a) { return a } test('a')", "x.js", false),
            ("const it = 1; it", "x.js", false),
            (
                "function f(describe) { return describe('a') }",
                "x.js",
                false,
            ),
            ("class A { test(a) { return a } }", "x.js", false),
            ("class A {\n  test(a)\n  { return a }\n}", "x.js", false),
            (
                "const o = { test(x) /*\n*/ { return x }, it: 1 }",
                "x.js",
                false,
            ),
            ("const o = { test: 1 }; o.test", "x.js", false),
            ("// test('a')", "x.js", false),
            ("x = 'test(1)'; y = `test(2)`; z = /test(3)/", "x.js", false),
            (
                "const other = 1; const s = `test ${other} it(1)`",
                "x.js",
                false,
            ),
            ("export function describe() {}", "x.js", false),
            ("import { test } from 'node:test'; test('a')", "x.js", false),
        ] {
            assert_eq!(reads(code, file), registers, "{code:?}");
        }
    }

    /// A package imported as a data file (a locale table, a stylesheet) is judged by its manifest
    /// and by the word `vitest`, not parsed as JavaScript; a native addon is refused.
    #[test]
    fn data_files_reached_directly_run_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let package = install(directory.path(), "locale", r#"{"name":"locale"}"#);
        for (name, text, refused) in [
            ("ja.json", r#"{"messages":{"required":"x"}}"#, false),
            ("theme.css", "a { color: red }", false),
            ("logo.svg", "<svg/>", false),
            ("ja.json", r#"{"note":"see vitest"}"#, true),
            ("addon.node", "\u{7f}ELF\u{0}\u{1}", true),
            ("addon.node", "exports.go = 1", true),
            ("boot.wasm", "\u{0}asm\u{1}", true),
            // Data is not read as code: only the word `vitest` refuses it.
            ("entry.json", r#"{"describe": "x", "test": ["y"]}"#, false),
            ("entry.png", "describe('x', () => {})", false),
            // The same text as a script is refused.
            ("entry.js", "describe('x', () => {})", true),
        ] {
            write_file(&package, name, text);
            forget_reached();
            VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
            assert_eq!(
                cannot_register_tests(&package.join(name), true),
                !refused,
                "{name}: {text}"
            );
        }
    }

    /// TypeScript's `import x = require(...)` is a load, and a script too deep to parse safely
    /// refuses the package.
    #[test]
    fn typescript_import_assignments_and_unparseable_scripts_are_judged() {
        let directory = tempfile::tempdir().unwrap();
        let package = install(directory.path(), "ts", r#"{"name":"ts"}"#);
        write_file(
            &package,
            "registrar.cjs",
            "exports.go = () => test('a', () => {})",
        );
        write_file(&package, "clean.cjs", "exports.go = () => 1");
        for (entry, refused) in [
            ("import r = require('./registrar.cjs'); r.go();", false),
            ("import r = require('./clean.cjs'); r.go();", true),
        ] {
            write_file(&package, "index.ts", entry);
            forget_reached();
            VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
            assert_eq!(
                cannot_register_tests(&package.join("index.ts"), true),
                refused,
                "{entry}"
            );
        }
        let deep = format!("{}0;", "!".repeat(1_000_000));
        assert_entry(&package, &deep, true, true);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_package_is_not_followed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "linked", r#"{"name":"linked"}"#);
        fs::write(
            root.join("outside.js"),
            "exports.go = () => test('a', () => {})",
        )
        .unwrap();
        std::os::unix::fs::symlink(root.join("outside.js"), package.join("registrar.js")).unwrap();
        fs::write(
            package.join("index.js"),
            "exports.go = require('./registrar').go",
        )
        .unwrap();
        forget_reached();
        VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
        assert!(!cannot_register_tests(&package.join("index.js"), true));
    }

    /// What the text of a load does not state makes the package "cannot tell"; a directory is
    /// followed through its manifest; links are followed by their target; unusual script names count.
    #[test]
    fn loads_that_cannot_be_followed_refuse_and_directories_follow_their_main() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "odd", r#"{"name":"odd"}"#);
        let write = |name: &str, text: &str| write_file(&package, name, text);
        let registers = "exports.go = () => test('a', () => {})";
        write("registrar.js", registers);
        write("registrar.JS", registers);
        write("noext", registers);
        write("sub/package.json", r#"{"main":"register.js"}"#);
        write("sub/register.js", registers);
        write("sub/index.js", "exports.go = () => 1");
        write("broken/package.json", "{ not json");
        write("broken/index.js", "exports.go = () => 1");
        write("lib/load.js", "exports.go = require('./target').go");
        write("lib/target.js", registers);
        write("target.js", "exports.go = () => 1");
        write("clean.js", "exports.go = () => 1");
        write("typed.ts", registers);
        write("view.tsx", registers);
        #[cfg(unix)]
        std::os::unix::fs::symlink(package.join("lib/load.js"), package.join("alias.js")).unwrap();
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut cases = vec![
            ("exports.go = require(`./registrar`).go", true),
            ("exports.go = require('./' + 'registrar').go", true),
            ("exports.go = require(/* c */ './registrar').go", true),
            ("exports.go = require('./\\u0072egistrar').go", true),
            ("exports.go = require(name).go", true),
            ("exports.go = import(`./${name}`)", true),
            ("exports.go = require('./sub').go", true),
            ("exports.go = require('./broken').go", true),
            ("exports.go = require('./noext').go", true),
            ("exports.go = require('./registrar.JS').go", true),
            ("exports.go = require('./clean').go", false),
            ("exports.go = require('./typed').go", true),
            ("exports.go = require('./view').go", true),
            ("exports.go = typeof require === 'function'", false),
            ("exports.go = Array.from(items)", false),
            ("exports.go = require.resolve('./registrar')", false),
        ];
        #[cfg(unix)]
        cases.push(("exports.go = require('./alias').go", true));
        for (entry, registers) in cases {
            assert_entry(&package, entry, true, registers);
        }
    }

    /// Loads spelled with comments, Unicode space, `module.require`, URL suffixes, package-internal
    /// or absolute specifiers cannot be read as plain relative paths and refuse; Node's precedence
    /// between files and directories, and between `main` and a nested manifest, is followed.
    #[test]
    fn spellings_and_precedence_follow_node() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "node", r#"{"name":"node"}"#);
        let write = |name: &str, text: &str| write_file(&package, name, text);
        let registers = "exports.go = () => test('a', () => {})";
        let clean = "exports.go = () => 1";
        write("registrar.cjs", registers);
        write("registrar.txt", registers);
        write("registrar.JSON", registers);
        write("data.json", "{}");
        write("branch.js", clean);
        write("branch/index.js", registers);
        write("solo.js", clean);
        write("solo/index.js", clean);
        write("sub/package.json", r#"{"main":"nested"}"#);
        write("sub/nested/package.json", r#"{"main":"clean.js"}"#);
        write("sub/nested/clean.js", clean);
        write("sub/nested/index.js", registers);
        write(
            "modular/package.json",
            r#"{"main":"clean.js","module":"reg.js"}"#,
        );
        write("modular/clean.js", clean);
        write("modular/reg.js", registers);
        write(
            "routed/package.json",
            r#"{"main":"clean.js","exports":"./reg.js"}"#,
        );
        write("routed/clean.js", clean);
        write("routed/reg.js", registers);
        write("nested/package.json", r#"{"main":"./inner"}"#);
        write("nested/inner/package.json", r#"{"main":"reg.js"}"#);
        write("nested/inner/reg.js", registers);
        write("nested/inner/index.js", clean);
        write("style.css", "a { color: red }");
        write("addon.node", "\u{7f}ELF\u{0}");
        write("boot.wasm", "\u{0}asm");
        write("noted.json", r#"{"note":"vitest"}"#);
        write("twin.js", clean);
        write("twin.ts", registers);
        write("both.js", clean);
        write("both.mjs", registers);
        for (entry, registers) in [
            (
                "exports.go = import.meta.glob('./*.js', { eager: true })",
                true,
            ),
            ("exports.go = require('node').go", true),
            ("exports.go = require('node/sub').go", true),
            ("exports.go = require('nodes').go", false),
            ("exports.go = require('./modular').go", true),
            ("exports.go = require.call(null, './registrar.cjs')", true),
            (
                "import { createRequire } from 'module'; exports.go = 1",
                true,
            ),
            ("exports.go = module.createRequire(__filename)('./x')", true),
            ("import './style.css'", false),
            ("exports.go = require('./addon.node')", true),
            ("exports.go = import('./boot.wasm')", true),
            ("exports.go = require('./noted.json')", true),
            ("exports.go = require('module')['createRequire'](__filename)('./x')", true),
            ("exports.go = require('./data').go", false),
            ("exports.go = require.main.require('./solo/').go", true),
            ("exports.go = module.parent.require('./solo/').go", true),
            ("const { createRequire: load } = require('module'); exports.go = load(__filename)('./x')", true),
            ("exports.go = require('node:module')._load('./x')", true),
            ("const { createRequire } = require('node:module'); exports.go = createRequire(__filename)('./x')", true),
            (
                "exports.go = require.apply(null, ['./registrar.cjs'])",
                true,
            ),
            ("exports.go = require('./routed').go", true),
            ("exports.go = require('./nested').go", true),
            ("exports.go = require('./data.json')", false),
            ("exports.go = require('./twin.js').go", true),
            ("exports.go = require('./both').go", true),
            ("exports.go = require /* c */ ('./registrar.cjs').go", true),
            ("exports.go = require\u{a0}('./registrar.cjs').go", true),
            ("exports.go = module.require('./registrar.cjs').go", true),
            ("exports.go = require('#registrar').go", true),
            ("exports.go = require('/abs/registrar').go", true),
            ("exports.go = require('file:///abs/registrar').go", true),
            ("exports.go = import('./registrar.cjs?init')", true),
            ("exports.go = import('./registrar.cjs#frag')", true),
            ("exports.go = import('./registrar%2Ecjs')", true),
            ("exports.go = require('./registrar.txt').go", true),
            ("exports.go = require('./registrar.JSON').go", true),
            ("exports.go = require('./data.json')", false),
            ("exports.go = require('./branch').go", true),
            ("exports.go = require('./branch/').go", true),
            ("exports.go = require('./solo/').go", false),
            ("exports.go = require('./sub').go", true),
            ("exports.go = require('node:fs').go", false),
            ("exports.go = require('lodash/fp').go", false),
        ] {
            assert_entry(&package, entry, true, registers);
        }
    }

    /// Verdicts are dropped when a run ends, so the next one reads the files as they are then.
    #[test]
    fn verdicts_are_forgotten_at_the_end_of_a_run() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "changing", r#"{"name":"changing"}"#);
        fs::write(package.join("index.js"), "exports.go = require('./a').go").unwrap();
        fs::write(package.join("a.js"), "exports.go = () => 1").unwrap();
        forget_reached();
        VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
        assert!(cannot_register_tests(&package.join("index.js"), true));
        fs::write(
            package.join("a.js"),
            "exports.go = () => test('a', () => {})",
        )
        .unwrap();
        {
            let _gaps = crate::frontend::resolver::GAPS_TEST_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::frontend::take_resolution_gaps();
        }
        VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
        assert!(!cannot_register_tests(&package.join("index.js"), true));
    }

    /// `require('.')` and `require('..')` name a directory index.
    #[test]
    fn dot_specifiers_name_a_directory_index() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "dots", r#"{"name":"dots"}"#);
        fs::create_dir_all(package.join("sub")).unwrap();
        fs::write(
            package.join("index.js"),
            "exports.go = () => test('a', () => {})",
        )
        .unwrap();
        fs::write(
            package.join("sub/index.js"),
            "module.exports = require('..')",
        )
        .unwrap();
        fs::write(package.join("sub/self.js"), "module.exports = require('.')").unwrap();
        for entry in ["sub/index.js", "sub/self.js"] {
            forget_reached();
            VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
            assert!(
                !cannot_register_tests(&package.join(entry), true),
                "{entry}"
            );
        }
    }

    /// Reading follows cycles once, and gives up past the file cap rather than reading without bound.
    #[test]
    fn local_files_are_read_once_and_within_a_cap() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let package = install(root, "ring", r#"{"name":"ring"}"#);
        fs::write(package.join("index.js"), "require('./a')").unwrap();
        fs::write(package.join("a.js"), "require('./b')").unwrap();
        fs::write(package.join("b.js"), "require('./a')").unwrap();
        forget_reached();
        VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
        assert!(cannot_register_tests(&package.join("index.js"), false));
        let wide = install(root, "wide", r#"{"name":"wide"}"#);
        let imports: Vec<String> = (0..=MAX_LOCAL_FILES)
            .map(|n| format!("require('./f{n}')"))
            .collect();
        fs::write(wide.join("index.js"), imports.join(";")).unwrap();
        for n in 0..=MAX_LOCAL_FILES {
            fs::write(wide.join(format!("f{n}.js")), "exports.x = 1").unwrap();
        }
        forget_reached();
        VERDICTS.with(|verdicts| verdicts.borrow_mut().clear());
        assert!(!cannot_register_tests(&wide.join("index.js"), false));
    }

    /// A closure past the cap is refused instead of read without bound.
    #[test]
    fn a_closure_past_the_cap_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let names: Vec<String> = (0..=MAX_PACKAGES).map(|n| format!("p{n}")).collect();
        let table: Vec<String> = names.iter().map(|n| format!("\"{n}\":\"1\"")).collect();
        let hub = install(
            root,
            "hub",
            &format!("{{\"dependencies\":{{{}}}}}", table.join(",")),
        );
        for name in &names {
            install(root, name, r#"{"name":"leaf"}"#);
        }
        assert!(!verdict(&hub));
    }
}
