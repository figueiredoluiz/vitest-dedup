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

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, CallExpression, Expression, IdentifierReference, StaticMemberExpression,
};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_span::SourceType;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
        candidate
            .join("package.json")
            .is_file()
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
        let Some(manifest) = std::fs::read_to_string(package.join("package.json"))
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

/// Names a test file calls without importing them when the project enables Vitest globals.
const GLOBAL_REGISTRATION: &[&str] = &[
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

/// Whether `c` ends a line in JavaScript.
fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `text` with comments, string literals and regular-expression literals blanked, so prose ("don't
/// change it."), messages and quotes inside a pattern are not mistaken for code. A `/` starts a
/// regular expression where an operand is expected (after an operator, a bracket, `,`, `;`, or
/// `return`/`typeof`); elsewhere it divides. A pattern read as a division after a `)` (as in
/// `if (x) /re/.test(y)`) is the one shape this misses.
fn without_comments_and_strings(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    // The last character of code seen, and the word it ended, to tell a pattern from a division.
    let mut previous = ' ';
    let mut word = String::new();
    while let Some(c) = chars.next() {
        let operand_expected = matches!(
            previous,
            ' ' | '('
                | ','
                | '='
                | ':'
                | '['
                | '!'
                | '&'
                | '|'
                | '?'
                | '{'
                | '}'
                | ';'
                | '+'
                | '-'
                | '*'
                | '%'
                | '<'
                | '>'
                | '~'
                | '^'
        ) || matches!(word.as_str(), "return" | "typeof");
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for next in chars.by_ref() {
                    if is_line_terminator(next) {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = ' ';
                let mut lines = false;
                for next in chars.by_ref() {
                    if last == '*' && next == '/' {
                        break;
                    }
                    lines |= is_line_terminator(next);
                    last = next;
                }
                // A comment that spans lines still ends the line it started on.
                out.push(if lines { '\n' } else { ' ' });
            }
            '/' if operand_expected => {
                let mut in_class = false;
                while let Some(next) = chars.next() {
                    match next {
                        '\\' => {
                            chars.next();
                        }
                        '[' => in_class = true,
                        ']' => in_class = false,
                        '/' if !in_class => break,
                        c if is_line_terminator(c) => break,
                        _ => {}
                    }
                }
                out.push(' ');
                previous = ')';
                word.clear();
            }
            '`' => {
                // Template text is blanked, but an interpolation is code. The opening is kept as
                // a marker, so a template right after a name reads as a tagged call.
                out.push('`');
                let mut interpolation = 0usize;
                while let Some(next) = chars.next() {
                    if interpolation == 0 {
                        if next == '\\' {
                            chars.next();
                        } else if next == '`' {
                            break;
                        } else if next == '$' && chars.peek() == Some(&'{') {
                            chars.next();
                            interpolation = 1;
                            out.push(' ');
                        }
                    } else {
                        match next {
                            '{' => interpolation += 1,
                            '}' => {
                                interpolation -= 1;
                                if interpolation == 0 {
                                    out.push(';');
                                    continue;
                                }
                            }
                            _ => {}
                        }
                        // A nested template's backticks are not tag markers.
                        out.push(if next == '`' { ' ' } else { next });
                    }
                }
                out.push(' ');
                previous = ')';
                word.clear();
            }
            '\'' | '"' => {
                while let Some(next) = chars.next() {
                    if next == '\\' {
                        chars.next();
                    } else if next == c {
                        break;
                    }
                }
                out.push(' ');
                previous = ')';
                word.clear();
            }
            other => {
                out.push(other);
                if other.is_alphanumeric() || other == '_' || other == '$' {
                    word.push(other);
                } else if !other.is_whitespace() {
                    word.clear();
                }
                if !other.is_whitespace() {
                    previous = other;
                }
            }
        }
    }
    out
}

/// Whether the `(` at `open` starts the parameters of a function or method being defined rather
/// than a call: its matching `)` is followed by a block (`describe(path) { ... }`).
fn defines_function(code: &str, open: usize) -> bool {
    let mut depth = 0usize;
    for (offset, byte) in code.as_bytes()[open..].iter().take(4096).enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    // A block on the next line after a call is a statement, not a body.
                    let rest = &code[open + offset + 1..];
                    let gap = rest.len() - rest.trim_start_matches([' ', '\t']).len();
                    return rest[gap..].starts_with('{');
                }
            }
            _ => {}
        }
    }
    false
}

/// Whether `text` calls (or chains from) a registration global as a free name: the name stands
/// alone, not part of a longer identifier and not a property (`pattern.test(...)`; a property of
/// the global object, `globalThis.test(...)`, is the global), and is followed, across any
/// whitespace (a newline included; comments are already blanked), by `(`, `?.(`, a template
/// literal (a tagged call), or `.`/`?.` and a Vitest modifier (`describe.each(...)`). Textual and
/// deliberately generous: a local function that happens to be called `test` is a false alarm,
/// which only costs the proof.
fn calls_registration_global(text: &str) -> bool {
    let code = without_comments_and_strings(text);
    let bytes = code.as_bytes();
    GLOBAL_REGISTRATION.iter().any(|name| {
        code.match_indices(name).any(|(start, _)| {
            let before = start.checked_sub(1).map(|index| bytes[index]);
            if before.is_some_and(is_identifier_byte) {
                return false;
            }
            let prior =
                code[..start].trim_end_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
            // A property access is the global only on the global object; `...` is a spread.
            if !prior.ends_with("...")
                && prior
                    .strip_suffix('.')
                    .is_some_and(|receiver| !receiver_is_global_object(receiver))
            {
                return false;
            }
            let end = start + name.len();
            let mut at = skip_space(&code, end);
            // `?.` reaches the same call or modifier as `.`.
            let optional = code[at..].starts_with("?.");
            if optional {
                at = skip_space(&code, at + 2);
            }
            match bytes.get(at) {
                Some(b'(') => !defines_function(&code, at),
                Some(b'`') => true,
                Some(b'.') if !optional => names_modifier(&code[skip_space(&code, at + 1)..]),
                _ => optional && names_modifier(&code[at..]),
            }
        })
    })
}

/// Whether `byte` can be part of an identifier name.
fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

/// The offset of the first character at or after `from` that JavaScript does not count as
/// whitespace (which includes U+FEFF).
fn skip_space(code: &str, from: usize) -> usize {
    code.len()
        - code[from..]
            .trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
            .len()
}

/// Whether the text before a `.` ends in the global object (`globalThis`, `global`, `window`;
/// not `self`, which is often an alias of `this`), so `globalThis.test(...)` reads as the global call it is.
fn receiver_is_global_object(before_dot: &str) -> bool {
    let head = before_dot.trim_end_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let head = head.strip_suffix('?').unwrap_or(head);
    let head = head.trim_end_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let start = head
        .bytes()
        .rposition(|byte| !is_identifier_byte(byte))
        .map_or(0, |index| index + 1);
    // A longer chain (`a.window.test(...)`) is a property of something else.
    let chained = head[..start]
        .trim_end_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
        .ends_with('.');
    !chained && matches!(&head[start..], "globalThis" | "global" | "window")
}

/// Whether `rest` starts with a Vitest modifier as a whole word.
fn names_modifier(rest: &str) -> bool {
    const MODIFIERS: &[&str] = &[
        "each",
        "skip",
        "only",
        "todo",
        "concurrent",
        "sequential",
        "runIf",
        "skipIf",
        "extend",
        "fails",
        "for",
        "shuffle",
    ];
    MODIFIERS.iter().any(|modifier| {
        rest.strip_prefix(modifier)
            .is_some_and(|after| !after.bytes().next().is_some_and(is_identifier_byte))
    })
}

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
fn loads(path: &Path, text: &str, own: Option<&str>) -> Vec<Load> {
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
                Some(Expression::StringLiteral(literal)) => self.specifier(literal.value.as_str()),
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
            self.specifier(it.expression.value.as_str());
        }
        /// `import ... from 'x'`.
        fn visit_import_declaration(&mut self, it: &oxc_ast::ast::ImportDeclaration<'a>) {
            self.specifier(it.source.value.as_str());
            walk::walk_import_declaration(self, it);
        }
        /// `export { a } from 'x'`.
        fn visit_export_from_declaration(&mut self, it: &oxc_ast::ast::ExportFromDeclaration<'a>) {
            self.specifier(it.source.value.as_str());
        }
        /// `export * from 'x'`.
        fn visit_export_all_declaration(&mut self, it: &oxc_ast::ast::ExportAllDeclaration<'a>) {
            self.specifier(it.source.value.as_str());
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
    if super::extract::parse_refusal(text).is_some() {
        return vec![Load::Unknown];
    }
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path)
        .unwrap_or_default()
        .with_unambiguous(true);
    let parsed = Parser::new(&allocator, text, source_type).parse();
    if parsed.fatal_error || !parsed.diagnostics.is_empty() {
        return vec![Load::Unknown];
    }
    let mut scan = Scan {
        own,
        found: Vec::new(),
        handled: HashSet::new(),
    };
    scan.visit_program(&parsed.program);
    scan.found
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
    /// The scripts a load may name, by real path: every candidate that exists, since the loader
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
    if kind_of(&real) == Kind::Script && !found.scripts.contains(&real) {
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
    if use_manifest && manifest.is_file() {
        let Some(value) = std::fs::read_to_string(&manifest)
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
                || target.join("package.json").is_file()
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
    let registers = reached_files_register(file, globals);
    reached_verdicts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, registers);
    registers
}

/// Whether any script reachable from `entry` registers tests, or the walk cannot tell.
fn reached_files_register(entry: &Path, globals: bool) -> bool {
    // Everything below is in real paths, so a link inside the package neither hides a file nor
    // changes which directory a later relative load resolves from.
    let Ok(real_entry) = entry.canonicalize() else {
        return true;
    };
    let Some(real_root) = package_root(&real_entry) else {
        return true;
    };
    let own = std::fs::read_to_string(real_root.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("name").and_then(Value::as_str).map(str::to_owned));
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
        // Text that calls a registration global is refused whatever its extension: Node runs an
        // unknown extension as JavaScript.
        if globals && calls_registration_global(&text) {
            return true;
        }
        // Data and assets are inert to the bundler that serves them (Vite), so they are not parsed
        // or followed; Node's `require` would run an unknown extension as JavaScript.
        if kind == Kind::Data {
            continue;
        }
        for load in loads(&file, &text, own.as_deref()) {
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

    /// A registration global is a call however the name is separated from its parenthesis, and a
    /// near miss one edit away is not.
    #[test]
    fn registration_calls_are_found_across_whitespace_comments_and_optional_calls() {
        for (code, calls) in [
            ("test('a', () => {})", true),
            ("test\n('a', () => {})", true),
            ("test\r\n\t('a', () => {})", true),
            ("test /* c */ ('a', () => {})", true),
            ("test // c\n('a', () => {})", true),
            ("test?.('a', () => {})", true),
            ("test ?. ('a', () => {})", true),
            ("test`a`", true),
            ("test\n`a`", true),
            ("describe\n.each([1])('a', () => {})", true),
            ("describe.\neach([1])('a', () => {})", true),
            ("test?.each([1])('a', () => {})", true),
            ("test?.\nskip('a', () => {})", true),
            ("beforeEach\n(() => {})", true),
            ("test", false),
            ("test;\n('a')", false),
            ("test = 1\n", false),
            ("globalThis.test('a', () => {})", true),
            ("globalThis?.describe.each([1])('a', () => {})", true),
            ("window\n.it('a', () => {})", true),
            ("window.test`a`", true),
            ("self.test('a')", false),
            ("test\u{feff}('a', () => {})", true),
            ("a.window.test('a')", false),
            ("a. window.test('a')", false),
            ("patterns. test?.('a')", false),
            ("patterns.\ntest`a`", false),
            ("patterns./* c */test('a')", false),
            ("[...test('a')]", true),
            ("const s = `${test}${`hello`}`;", false),
            ("const s = `${`test`}`;", false),
            ("const s = `${test}`\n`x`;", false),
            ("test // c\r('a')", true),
            ("x = /a\r test('a')", true),
            ("test // c\u{2028}('a')", true),
            ("test('a', () => {}) /*\u{2028}*/ {}", true),
            ("a./* c */window.test('a')", false),
            ("test`a${1}`", true),
            ("globalThis.test`a${1}`", true),
            ("const s = `${test}`;", false),
            ("test('a', () => {}) /*\n*/ {}", true),
            ("myglobalThis.test('a')", false),
            ("localThis.test('a')", false),
            ("pattern.test('a')", false),
            ("pattern\n.test('a')", false),
            ("pattern?.test('a')", false),
            ("mytest\n('a')", false),
            ("test?.other('a')", false),
            ("test.eachx('a')", false),
            ("test\n.other('a')", false),
            ("// test\n('a')", false),
            ("x = 'test'\n('a')", false),
            ("class A { test\n(a) { return a } }", false),
        ] {
            assert_eq!(calls_registration_global(code), calls, "{code:?}");
        }
    }

    /// A package imported as a data file (a locale table, a stylesheet) is judged by its manifest
    /// and by the word `vitest`, not parsed as JavaScript; a native addon is still refused.
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
            (
                "entry.png",
                "describe('x', () => { test('y', () => {}) })",
                true,
            ),
            ("entry.json", "describe('x', () => {})", true),
            ("entry.json", r#"{"describe": "x", "test": ["y"]}"#, false),
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
