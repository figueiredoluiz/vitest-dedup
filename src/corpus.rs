//! The tests of a run, each keyed for comparison or excluded with a stated reason.
//!
//! One [`TestCase`] per test site and table row. A test is either compared, carrying a same-file
//! key and a cross-file key, or excluded, carrying the [`Exclusion`] that says why: no test is
//! dropped without one. Rules read this record and never the syntax.

use crate::framework::{Dialect, Registers};
use crate::inventory::{self, SiteKind};
use crate::lang::{self, Lines, NodeId, Refusal};
use crate::normal::{self, digest, Normalizer, Structure};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// Why a test is left out of comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Exclusion {
    /// It is registered inside a function that is neither a suite body nor an iteration callback,
    /// so what registers it, and how often, depends on a caller.
    RegisteredInsideFunction,
    /// Its callback has an empty body.
    EmptyBody,
    /// It has no callback the analysis can read.
    NoCallback,
    /// It reads state another test writes, or a hook accumulates.
    OrderDependentState,
    /// Its callback is passed by name, not written inline: whether that binding is reassigned, or
    /// shared with another test, decides what runs.
    NamedCallback,
    /// A wrapper call, a module load or a registration function read as a value registers it: the
    /// file does not spell the tests, nor how many there are, so one is counted per site.
    RegisteredElsewhere,
}

/// One test, or one row of a test table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestCase {
    /// Where the registering call starts, in bytes.
    pub site: u32,
    /// 1-based line of the position Vitest reports.
    pub line: u32,
    /// 1-based UTF-16 column of that position.
    pub column: u32,
    /// The title, when the source spells it.
    pub title: Option<String>,
    /// Which row of its table, when it has one.
    pub row: Option<usize>,
    /// Equal for two tests of one file claimed to be duplicates.
    pub key: u64,
    /// Equal for two tests of different files claimed to be duplicates.
    pub module_key: u64,
    /// Why it is not compared, if it is not.
    pub exclusion: Option<Exclusion>,
}

/// A registration site, as the inventory reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteRecord {
    /// The dialect's name for what it registers (`describe` for a suite), `wrapper` or
    /// `module-load`.
    pub kind: &'static str,
    /// 1-based line of the position Vitest reports.
    pub line: u32,
    /// The first line a transform's source map may place it on.
    pub line_from: u32,
    /// 1-based UTF-16 column.
    pub column: u32,
    /// The callee chain past the registration name.
    pub chain: String,
}

/// What the corpus holds for one file.
#[derive(Clone, Debug, Default)]
pub struct FileCorpus {
    /// Its tests, in source order, rows in table order.
    pub tests: Vec<TestCase>,
    /// Its registration sites and module loads.
    pub sites: Vec<SiteRecord>,
    /// Parse errors the parser recovered from.
    pub parse_errors: usize,
}

impl FileCorpus {
    /// Excluded tests per reason.
    pub fn exclusions(&self) -> BTreeMap<Exclusion, usize> {
        let mut counts = BTreeMap::new();
        for exclusion in self.tests.iter().filter_map(|test| test.exclusion) {
            *counts.entry(exclusion).or_default() += 1;
        }
        counts
    }
}

/// Caches one run shares across its files.
#[derive(Debug, Default)]
pub struct Run {
    modules: inventory::Modules,
    exports: normal::Exports,
}

/// What a file is read with.
#[derive(Clone, Copy, Debug)]
pub struct Setting<'a> {
    /// The framework's API.
    pub dialect: &'a Dialect,
    /// Whether the file's project enables globals.
    pub globals: bool,
    /// The analysis root.
    pub root: &'a Path,
}

/// Whether reaching `statement` runs nothing: a function, type or import with bindings, or a
/// declaration whose initializers create nothing. A class runs its decorators, computed keys and
/// static members when declared, so it acts.
fn inert(normalizer: &Normalizer, tree: &lang::Tree, statement: NodeId) -> bool {
    match tree.kind(statement) {
        "FunctionDeclaration"
        | "TSInterfaceDeclaration"
        | "TSTypeAliasDeclaration"
        | "TSDeclareFunction"
        | "EmptyStatement" => true,
        "ImportDeclaration" => !tree.list(statement, "specifiers").is_empty(),
        "VariableDeclaration" | "TSEnumDeclaration" => !normalizer.creates(statement),
        "ExportNamedDeclaration" => tree
            .child(statement, "declaration")
            .is_none_or(|declaration| inert(normalizer, tree, declaration)),
        _ => false,
    }
}

/// The site's kind as a report spells it: the dialect's own name for what it registers.
fn site_kind(dialect: &Dialect, kind: SiteKind) -> &'static str {
    match kind {
        SiteKind::Registers(registers) => dialect
            .registrations
            .iter()
            .find(|registration| registration.registers == registers)
            .map_or("registration", |registration| registration.canonical),
        SiteKind::Wrapper => "wrapper",
    }
}

/// Reads one test file into the corpus.
pub fn file(
    path: &Path,
    source: &str,
    setting: Setting,
    run: &mut Run,
) -> Result<FileCorpus, Refusal> {
    let parsed = lang::parse(path, source)?;
    let found = inventory::inventory(
        path,
        source,
        &parsed,
        inventory::Setting {
            dialect: setting.dialect,
            globals: setting.globals,
            root: setting.root,
        },
        &mut run.modules,
    );
    let tree = &parsed.tree;
    let lines = Lines::new(source);
    let structure = Structure::of(&parsed, &found);
    let mut normalizer = Normalizer::new(
        path,
        &parsed,
        normal::Setting {
            dialect: setting.dialect,
            root: setting.root,
            exports: &run.exports,
        },
        &structure,
    );
    // A statement that is a call registering a test, suite or benchmark is that registration, not
    // context; any other statement holding one (a loop, a sequence with setup) is context whole.
    let registration = |statement: NodeId| {
        tree.kind(statement) == "ExpressionStatement"
            && tree
                .child(statement, "expression")
                .is_some_and(|expression| {
                    let call = tree.inner(expression);
                    found.sites.iter().any(|site| {
                        site.node == call
                            && matches!(
                                site.kind,
                                SiteKind::Registers(
                                    Registers::Test | Registers::Suite | Registers::Bench
                                )
                            )
                    })
                })
    };

    // Statements that set up a scope: those registering hooks or calling wrappers, and, in a suite
    // body, every statement that neither declares nor registers a test, suite or benchmark.
    let mut ambient: HashMap<Option<NodeId>, Vec<NodeId>> = HashMap::new();
    for site in found.sites.iter().filter(|site| {
        matches!(
            site.kind,
            SiteKind::Registers(Registers::Hook(_)) | SiteKind::Wrapper
        )
    }) {
        let Some(suites) = normalizer.suites_around(site.node) else {
            continue;
        };
        let scope = suites.last().copied();
        if let Some(statement) = normalizer.statement_in(site.node, scope) {
            ambient.entry(scope).or_default().push(statement);
        }
    }
    for &suite in structure.suites.keys() {
        for statement in normalizer.body_statements(suite) {
            if !inert(&normalizer, tree, statement) && !registration(statement) {
                ambient.entry(Some(suite)).or_default().push(statement);
            }
        }
    }
    let mut contexts: HashMap<Option<NodeId>, String> = HashMap::new();
    for (scope, mut statements) in ambient {
        statements.sort_by_key(|&statement| tree.span(statement).start);
        statements.dedup();
        let normal: Vec<String> = statements
            .into_iter()
            .map(|statement| normalizer.of(statement))
            .collect();
        contexts.insert(scope, normal.join(";"));
    }

    // What the file does while it loads, for comparison across files: top-level statements that
    // act without registering, and imports run for their effects alone.
    let mut module_context = String::new();
    for statement in tree.list(tree.root(), "body").into_iter().flatten() {
        let kind = tree.kind(statement);
        let acts = !inert(&normalizer, tree, statement) && !registration(statement);
        let bare_import =
            kind == "ImportDeclaration" && tree.list(statement, "specifiers").is_empty();
        if acts || bare_import {
            module_context.push_str(&normalizer.of(statement));
            // What a bare import runs is the file it resolves to, not its spelling.
            if let Some(file) = tree
                .child(statement, "source")
                .and_then(|source| tree.text(source, "value"))
                .filter(|_| bare_import)
                .and_then(|source| crate::lang::resolve_file(path, source, setting.root))
            {
                module_context.push_str(&file.display().to_string());
            }
            module_context.push(';');
        }
    }

    let mut tests = Vec::new();
    for site in found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Test))
    {
        let (line, column) = lines.position(source, site.position);
        let record = TestCase {
            site: site.span.start,
            line,
            column,
            title: site.title.clone(),
            row: None,
            key: 0,
            module_key: 0,
            exclusion: None,
        };
        let callee = tree.child(site.node, "callee");
        let modifiers = callee.map(|callee| normalizer.modifiers(callee));
        // An excluded test is still one case per row of a table the file spells out.
        let row_count = modifiers
            .as_ref()
            .and_then(|modifiers| modifiers.rows.as_ref())
            .map(Vec::len);
        let excluded = |exclusion| -> Vec<TestCase> {
            let case = |row| TestCase {
                row,
                exclusion: Some(exclusion),
                ..record.clone()
            };
            match row_count {
                None => vec![case(None)],
                Some(count) => (0..count).map(|index| case(Some(index))).collect(),
            }
        };
        let arguments: Vec<NodeId> = tree
            .list(site.node, "arguments")
            .into_iter()
            .flatten()
            .collect();
        let (Some(modifiers), Some(callback)) = (modifiers, normalizer.callback(site.node)) else {
            tests.extend(excluded(Exclusion::NoCallback));
            continue;
        };
        if !callback.inline {
            tests.extend(excluded(Exclusion::NamedCallback));
            continue;
        }
        let (body, function) = (callback.argument, callback.function);
        let empty = function.is_some_and(|function| {
            tree.child(function, "body").is_some_and(|body| {
                tree.kind(body) == "BlockStatement" && tree.list(body, "body").is_empty()
            })
        });
        if empty {
            tests.extend(excluded(Exclusion::EmptyBody));
            continue;
        }
        let Some(suites) = normalizer.suites_around(site.node) else {
            tests.extend(excluded(Exclusion::RegisteredInsideFunction));
            continue;
        };
        if function.is_some_and(|function| normalizer.order_dependent(function)) {
            tests.extend(excluded(Exclusion::OrderDependentState));
            continue;
        }
        let mut context = contexts.get(&None).cloned().unwrap_or_default();
        for scope in &suites {
            // An enclosing suite's run condition, table or expected failure applies to the test
            // as well. How deep the suites nest changes nothing; only what each one adds does.
            if let Some(suite_callee) = structure
                .suites
                .get(scope)
                .and_then(|call| tree.child(*call, "callee"))
            {
                let suite = normalizer.modifiers(suite_callee);
                let call = structure.suites[scope];
                let suite_arguments: Vec<NodeId> =
                    tree.list(call, "arguments").into_iter().flatten().collect();
                let suite_body = suite_arguments
                    .iter()
                    .copied()
                    .find(|&argument| tree.inner(argument) == *scope)
                    .unwrap_or(*scope);
                let (suite_options, options_fail) =
                    normalizer.options(&suite_arguments, suite_body);
                let fails = suite.fails || options_fail;
                if suite.chain.contains('.') || fails || !suite_options.is_empty() {
                    context.push('|');
                    context.push_str(&suite.chain);
                    if fails {
                        context.push_str(".!fails");
                    }
                    context.push_str(&suite_options.join(","));
                }
            }
            if let Some(statements) = contexts.get(&Some(*scope)) {
                context.push('|');
                context.push_str(statements);
            }
        }
        let (options, option_fails) = normalizer.options(&arguments, body);
        let fails = if modifiers.fails || option_fails {
            ".!fails"
        } else {
            ""
        };
        let options = options.join(",");
        // A row of a table the file spells out is the callback with its parameters bound to the
        // row's cells, so it compares with a test that spells those values itself.
        // Only an inline callback's parameters are bound: a named one may run elsewhere.
        let parameters: Option<Vec<NodeId>> = callback.inline.then(|| {
            function
                .map(|function| {
                    tree.list(function, "params")
                        .into_iter()
                        .flatten()
                        .collect()
                })
                .unwrap_or_default()
        });
        let rows = match &modifiers.rows {
            None => vec![(HashMap::new(), false, String::new())],
            Some(rows) => rows
                .iter()
                .map(|row| {
                    match parameters
                        .as_deref()
                        .and_then(|parameters| normalizer.bind_row(parameters, row))
                    {
                        Some((bound, every)) => (bound, every, String::new()),
                        None => {
                            let cells: Vec<NodeId> = match row {
                                normal::Row::Spread(cells) => cells.clone(),
                                normal::Row::Whole(row, _) => vec![*row],
                            };
                            let written: Vec<String> =
                                cells.into_iter().map(|cell| normalizer.of(cell)).collect();
                            (HashMap::new(), false, written.join(","))
                        }
                    }
                })
                .collect(),
        };
        let table = modifiers.rows.is_some();
        for (index, (bound, every, unbound)) in rows.into_iter().enumerate() {
            let normal_body = normalizer.substituted(body, bound, every);
            // A row whose every parameter is bound is the test it would be written out as.
            let chain = if table && every {
                &modifiers.untabled
            } else {
                &modifiers.chain
            };
            let key = format!(
                "{chain}{fails}\u{1}{options}\u{1}{unbound}\u{1}{normal_body}\u{1}{context}"
            );
            tests.push(TestCase {
                row: table.then_some(index),
                key: digest(&key),
                module_key: digest(&format!("{key}\u{1}{module_context}")),
                ..record.clone()
            });
        }
    }

    // What a wrapper call, a module load or an escaped registration function registers is spelled
    // in another module: one excluded case per site, since how many tests it is is not known.
    let elsewhere = found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Wrapper)
        .map(|site| site.position)
        .chain(found.loads.iter().copied())
        .chain(found.escapes.iter().map(|escape| escape.offset));
    for start in elsewhere {
        let (line, column) = lines.position(source, start);
        tests.push(TestCase {
            site: start,
            line,
            column,
            title: None,
            row: None,
            key: 0,
            module_key: 0,
            exclusion: Some(Exclusion::RegisteredElsewhere),
        });
    }

    let mut sites: Vec<SiteRecord> = found
        .loads
        .iter()
        .map(|&start| {
            let (line, column) = lines.position(source, start);
            SiteRecord {
                kind: "module-load",
                line,
                line_from: line,
                column,
                chain: String::new(),
            }
        })
        .collect();
    sites.extend(found.sites.iter().map(|site| {
        let (line, column) = lines.position(source, site.position);
        SiteRecord {
            kind: site_kind(setting.dialect, site.kind),
            line,
            line_from: lines.line(site.from),
            column,
            chain: site.chain.join("."),
        }
    }));
    Ok(FileCorpus {
        tests,
        sites,
        parse_errors: parsed.parse_errors,
    })
}

#[cfg(test)]
mod tests;
