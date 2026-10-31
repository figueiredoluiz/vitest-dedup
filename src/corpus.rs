//! The tests of a run, each keyed for comparison or excluded with a stated reason.
//!
//! One [`TestCase`] per test site and table row. A test is either compared, carrying a same-file
//! key and a cross-file key, or excluded, carrying the [`Exclusion`] that says why: no test is
//! dropped without one. Rules read this record and never the syntax.

mod suites;

use crate::exclusions;
use crate::framework::{Dialect, Registers};
use crate::inventory::{self, Reach, Site, SiteKind};
use crate::lang::{self, Lines, NodeId, Refusal};
use crate::model::{BlindFile, ExcludedFile, ExclusionReason, Position, Span};
use crate::normal::{self, digest, Normalizer, Structure};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// Why a test is left out of comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Exclusion {
    /// It is registered inside a function that is neither a suite body nor an iteration callback,
    /// so what registers it, and how often, depends on a caller.
    RegisteredInsideFunction,
    /// Its callback has an empty body.
    EmptyBody,
    /// It has no callback: nothing after the title but options or literals, which Vitest marks
    /// `todo`.
    NoCallback,
    /// A spread argument, or a callee the analysis cannot read, may hide the callback that runs.
    UnreadableCallback,
    /// It reads state another test writes, or a hook accumulates.
    OrderDependentState,
    /// Its callback is passed by name, not written inline: whether that binding is reassigned, or
    /// shared with another test, decides what runs.
    NamedCallback,
    /// It is a benchmark, which the next engine does not compare yet.
    Benchmark,
    /// A wrapper call, a module load or a registration function read as a value registers it: the
    /// file does not spell the tests, nor how many there are, so one is counted per site.
    RegisteredElsewhere,
}

impl Exclusion {
    /// The report's reason code and sentence, for an exclusion that leaves a test unresolved;
    /// `None` for a test with nothing to compare, which the current core screens as trivial.
    pub fn reason(self) -> Option<(ExclusionReason, &'static str)> {
        match self {
            Self::RegisteredInsideFunction => Some((
                ExclusionReason::RegisteredInsideFunction,
                "a test is registered inside a function whose caller decides what it registers",
            )),
            Self::OrderDependentState => Some((
                ExclusionReason::OrderDependentState,
                "a test reads state another test writes, or a hook accumulates, so it depends on \
                 which tests ran first",
            )),
            Self::UnreadableCallback => Some((
                ExclusionReason::UnreadableCallback,
                "a test's callback is hidden behind a spread argument or an unreadable callee",
            )),
            Self::NamedCallback => Some((
                ExclusionReason::NamedCallback,
                "a test's callback is passed by name, and the analyzer reads only inline callbacks",
            )),
            Self::Benchmark => Some((
                ExclusionReason::UnsupportedBenchmark,
                "a benchmark is counted but not compared yet",
            )),
            Self::RegisteredElsewhere => Some((
                ExclusionReason::RegisteredElsewhere,
                "a wrapper call, a module load or a registration function read as a value \
                 registers tests this file does not spell",
            )),
            Self::EmptyBody | Self::NoCallback => None,
        }
    }
}

/// One test, one row of a test table, or a suite in the separate suite population.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestCase {
    /// The registering call.
    pub span: Span,
    /// The title, when the source spells it.
    pub title: Option<String>,
    /// The suite path joined with the title the way a report spells a participant, when every
    /// segment is a literal the source spells.
    pub full_title: Option<String>,
    /// Proven title segments for name comparison, distinct from display text or raw patterns.
    /// None when any segment, row rendering or registration multiplicity is unknown.
    pub name: Option<Vec<String>>,
    /// Which row of its table, when it has one.
    pub row: Option<usize>,
    /// Declaration-local class of proven callback inputs and a supported rendered row title.
    pub row_class: Option<u64>,
    /// Equal for two tests of one file claimed to be duplicates.
    pub key: u64,
    /// Equal for two tests of different files claimed to be duplicates.
    pub module_key: u64,
    /// Whether the chain keeps the test from running (`skip`, `todo`).
    pub inert: bool,
    /// Why it is not compared, if it is not.
    pub exclusion: Option<Exclusion>,
}

/// A registration site, as the inventory reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteRecord {
    /// The dialect's name for what it registers (`describe` for a suite), `wrapper`,
    /// `hook-wrapper` (a wrapper that registers only hooks) or `module-load`.
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
    /// Comparable whole suites, separate from executable tests and exclusion counts.
    pub suites: Vec<TestCase>,
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
        SiteKind::Wrapper(Reach::Tasks) => "wrapper",
        SiteKind::Wrapper(Reach::Hooks | Reach::Hook) => "hook-wrapper",
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
            SiteKind::Registers(Registers::Hook(_)) | SiteKind::Wrapper(_)
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

    let table_site = |site: &Site| {
        site.chain.iter().any(|member| {
            matches!(
                setting.dialect.chain_member(member),
                Some(crate::framework::Chain::Table(_))
            )
        })
    };
    let suite_names: HashMap<NodeId, Option<String>> = found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Suite))
        .map(|site| {
            (
                site.node,
                if table_site(site) {
                    None
                } else {
                    normal::titles::title(&parsed, site.node)
                },
            )
        })
        .collect();
    // What each suite spells as its title, by its registering call: the segments of the suite
    // path a report names a participant by.
    let suite_titles: HashMap<NodeId, Option<String>> = found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Suite))
        .map(|site| (site.node, site.title.clone()))
        .collect();
    // A report location: the registering call, from its first byte, the way the default engine
    // reports a test, so a suppression comment above the declaration applies to it.
    let at = |start: u32, end: u32| {
        let (start_line, start_column) = lines.position(source, start);
        let (end_line, end_column) = lines.position(source, end);
        Span {
            start: Position {
                line: start_line,
                column: start_column,
            },
            end: Position {
                line: end_line,
                column: end_column,
            },
            start_byte: start,
            end_byte: end,
        }
    };
    // A case as its site spells it, before anything keys it.
    let unkeyed = |site: &Site, exclusion: Option<Exclusion>| TestCase {
        span: at(site.span.start, site.span.end),
        title: site.title.clone(),
        full_title: None,
        name: None,
        row: None,
        row_class: None,
        key: 0,
        module_key: 0,
        inert: false,
        exclusion,
    };

    let mut tests = Vec::new();
    for site in found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Test))
    {
        let mut record = unkeyed(site, None);
        let callee = tree.child(site.node, "callee");
        let modifiers = callee.map(|callee| normalizer.modifiers(callee));
        let arguments: Vec<NodeId> = tree
            .list(site.node, "arguments")
            .into_iter()
            .flatten()
            .collect();
        record.inert = modifiers.as_ref().is_some_and(|modifiers| modifiers.inert)
            || normalizer.inert_options(&arguments);
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
        let unread_callee = modifiers.is_none();
        let (Some(modifiers), Some(callback)) = (modifiers, normalizer.callback(site.node)) else {
            // A spread may expand to the callback Vitest runs; only a test with none is a todo.
            let hidden = unread_callee
                || arguments
                    .iter()
                    .any(|argument| tree.kind(*argument) == "SpreadElement");
            tests.extend(excluded(if hidden {
                Exclusion::UnreadableCallback
            } else {
                Exclusion::NoCallback
            }));
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
        // The suite path a report names a participant by: every enclosing suite's spelled title,
        // then the test's own. One segment the source does not spell leaves it unnamed.
        let full_title = record.title.clone().and_then(|title| {
            let mut segments = Vec::with_capacity(suites.len() + 1);
            for scope in &suites {
                let spelled = structure
                    .suites
                    .get(scope)
                    .and_then(|call| suite_titles.get(call))
                    .and_then(|title| title.as_deref())?;
                segments.push(spelled.to_owned());
            }
            segments.push(title);
            Some(segments.join(" > "))
        });
        // Name comparison consumes values, not display fallbacks. In particular an unread table
        // or a suite table cannot stand for its runtime names through a raw pattern.
        let name_prefix: Option<Vec<String>> = suites
            .iter()
            .map(|scope| suite_names.get(structure.suites.get(scope)?)?.clone())
            .collect();
        let pattern = normal::titles::title(&parsed, site.node);
        let row_prefix_known = suites.iter().all(|scope| {
            structure.suites.get(scope).is_some_and(|&call| {
                normal::row_identity::suite_prefix_known(&parsed, call, setting.dialect)
            })
        });
        for (index, (bound, every, unbound)) in rows.into_iter().enumerate() {
            let name =
                name_prefix
                    .clone()
                    .zip(pattern.as_deref())
                    .and_then(|(mut segments, pattern)| {
                        let title = if table_site(site) {
                            let row = modifiers.rows.as_ref()?.get(index)?;
                            let (arguments, fields) = normal::titles::row(&parsed, row);
                            (setting.dialect.render_title)(
                                pattern,
                                &arguments,
                                fields.as_ref(),
                                index,
                            )?
                        } else {
                            pattern.to_owned()
                        };
                        segments.push(title);
                        Some(segments)
                    });
            let row_class = (|| {
                if !row_prefix_known {
                    return None;
                }
                let row = modifiers.rows.as_ref()?.get(index)?;
                let identity = normal::row_identity::identity(&parsed, row, function?)?;
                let (arguments, fields) = normal::titles::row(&parsed, row);
                let rendered = (setting.dialect.render_row_title)(
                    pattern.as_deref()?,
                    &arguments,
                    fields.as_ref(),
                    index,
                )?;
                if name.as_ref()?.last()? != &rendered {
                    return None;
                }
                Some(digest(&identity))
            })();
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
                row_class,
                key: digest(&key),
                module_key: digest(&format!("{key}\u{1}{module_context}")),
                full_title: full_title.clone(),
                name,
                inert: record.inert,
                ..record.clone()
            });
        }
    }

    // What a wrapper call, a module load or an escaped registration function registers is spelled
    // in another module: one excluded case per site, since how many tests it is is not known.
    let elsewhere = found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Wrapper(Reach::Tasks))
        .map(|site| site.position)
        .chain(found.loads.iter().copied())
        .chain(found.escapes.iter().map(|escape| escape.offset));
    for start in elsewhere {
        tests.push(TestCase {
            span: at(start, start),
            title: None,
            full_title: None,
            name: None,
            row: None,
            row_class: None,
            key: 0,
            module_key: 0,
            inert: false,
            exclusion: Some(Exclusion::RegisteredElsewhere),
        });
    }
    // Benchmarks run and are counted, one excluded case each, until they are compared.
    for site in found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Bench))
    {
        tests.push(unkeyed(site, Some(Exclusion::Benchmark)));
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
    let suites = suites::collect(
        &parsed,
        &found,
        &structure,
        &contexts,
        &tests,
        &mut normalizer,
        (setting, source),
    );
    Ok(FileCorpus {
        suites,
        tests,
        sites,
        parse_errors: parsed.parse_errors,
    })
}

/// The next engine inside the shell's file loop: what it read, summed over the run.
#[derive(Debug, Default)]
pub struct Engine {
    run: Run,
    /// Tests read, rows counted one each.
    pub tests: usize,
    /// Tests left unresolved: excluded for a reason that has a report code.
    pub unresolved: usize,
    /// Registration sites, by the file's path relative to the root, when they are kept.
    pub sites: Vec<(PathBuf, SiteRecord)>,
    /// Each file's cases, by the file's path relative to the root: what the rules read once
    /// every file has been read. Charged to the RSS backstop alone until the engine's state is
    /// measured onto `--max-memory`.
    pub files: Vec<(PathBuf, Vec<TestCase>)>,
    /// Comparable suite containers, retained separately from the executable population.
    pub suites: Vec<(PathBuf, Vec<TestCase>)>,
    /// Files every test of which is unresolved.
    blind: Vec<BlindFile>,
}

/// Why a file's read did not finish.
#[derive(Debug)]
enum Failure {
    /// The thread it runs on could not be started.
    Start(String),
    /// It panicked and was recovered.
    Panicked,
}

/// The variable that selects the engine, and the value that selects this one.
const ENGINE_VARIABLE: (&str, &str) = ("VITEST_DEDUP_ENGINE", "next");

impl Engine {
    /// The engine, when the environment selects it.
    pub fn selected() -> Option<Self> {
        let (variable, value) = ENGINE_VARIABLE;
        std::env::var_os(variable)
            .is_some_and(|selected| selected == value)
            .then(Self::default)
    }

    /// Reads one test file at `path` (`relative` to the root): its exclusion entry, if it has one,
    /// and whether the file was read only in part. `further` holds reasons the shell found before
    /// the read (an undetermined `globals`), listed after the engine's own.
    pub fn read(
        &mut self,
        path: &Path,
        relative: &Path,
        source: &str,
        setting: Setting,
        keep_sites: bool,
        further: Vec<(ExclusionReason, String)>,
    ) -> (Option<ExcludedFile>, bool) {
        // On a stack sized for the parser, as the current core runs: [`crate::lang::limits`].
        let run = &mut self.run;
        let read = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("vitest-dedup-corpus".to_owned())
                .stack_size(crate::lang::limits::EXTRACTION_STACK_BYTES)
                .spawn_scoped(scope, move || file(path, source, setting, run))
                .map_err(|error| Failure::Start(error.to_string()))?
                .join()
                .map_err(|_| Failure::Panicked)
        });
        self.record(relative, read, keep_sites, further)
    }

    /// Adds what reading the file at `relative` gave, or why it did not run, with the shell's
    /// `further` reasons: its exclusion entry, if it has one, and whether it was read only in
    /// part.
    fn record(
        &mut self,
        relative: &Path,
        read: Result<Result<FileCorpus, Refusal>, Failure>,
        keep_sites: bool,
        further: Vec<(ExclusionReason, String)>,
    ) -> (Option<ExcludedFile>, bool) {
        let unparsed = |reason, detail: String| {
            let mut reasons = vec![(reason, detail)];
            reasons.extend(further.iter().cloned());
            let entry = exclusions::counted_file(relative.to_path_buf(), reasons, 0, 0);
            (entry, true)
        };
        let corpus = match read {
            Ok(Ok(corpus)) => corpus,
            Ok(Err(refusal)) => {
                return match refusal {
                    Refusal::UnsupportedSource => unparsed(
                        ExclusionReason::UnsupportedSource,
                        "the file's extension is not a JavaScript or TypeScript source type"
                            .to_owned(),
                    ),
                    Refusal::Fatal => unparsed(
                        ExclusionReason::ParseError,
                        "the file could not be parsed".to_owned(),
                    ),
                    Refusal::SourceLimit => unparsed(
                        ExclusionReason::SourceLimit,
                        "the file is larger or nests deeper than the parser is given".to_owned(),
                    ),
                };
            }
            // Nothing ran, so the caches are as the previous file left them.
            Err(Failure::Start(error)) => {
                return unparsed(
                    ExclusionReason::ExtractionFailed,
                    format!("could not start the bounded analysis thread: {error}"),
                );
            }
            Err(Failure::Panicked) => {
                // A panic may have left the caches half-written. Keys do not depend on what was
                // read before, so the files after this one key the same from empty caches.
                self.run = Run::default();
                return unparsed(
                    ExclusionReason::ExtractionFailed,
                    "the file could not be read: the analysis failed and was recovered".to_owned(),
                );
            }
        };
        let mut reasons: Vec<(ExclusionReason, String)> = Vec::new();
        if corpus.parse_errors > 0 {
            reasons.push((
                ExclusionReason::ParseError,
                "the parser reported errors; the file was read as far as it parsed".to_owned(),
            ));
        }
        let mut unresolved = 0;
        for (exclusion, count) in corpus.exclusions() {
            if let Some((reason, detail)) = exclusion.reason() {
                unresolved += count;
                reasons.push((reason, detail.to_owned()));
            }
        }
        reasons.extend(further);
        let case_count = corpus.tests.len();
        let mut corpus = corpus;
        self.files
            .push((relative.to_path_buf(), std::mem::take(&mut corpus.tests)));
        self.suites
            .push((relative.to_path_buf(), std::mem::take(&mut corpus.suites)));
        self.tests += case_count;
        self.unresolved += unresolved;
        if unresolved > 0 && unresolved == case_count {
            self.blind.push(BlindFile {
                path: relative.to_path_buf(),
                tests: unresolved,
            });
        }
        if keep_sites {
            self.sites.extend(
                corpus
                    .sites
                    .into_iter()
                    .map(|site| (relative.to_path_buf(), site)),
            );
        }
        let entry =
            exclusions::counted_file(relative.to_path_buf(), reasons, unresolved, case_count);
        (entry, corpus.parse_errors > 0)
    }

    /// Files every test of which is unresolved, the most tests first, then by path.
    pub fn blind_files(&self) -> Vec<BlindFile> {
        let mut blind = self.blind.clone();
        blind.sort_by(|a, b| b.tests.cmp(&a.tests).then_with(|| a.path.cmp(&b.path)));
        blind
    }
}

#[cfg(test)]
mod tests;
