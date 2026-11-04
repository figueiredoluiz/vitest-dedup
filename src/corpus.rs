//! The tests of a run, each keyed for comparison or excluded with a stated reason.
//!
//! One [`TestCase`] per test site and table row. A test is either compared, carrying a same-file
//! key and a cross-file key, or excluded, carrying the [`Exclusion`] that says why: no test is
//! dropped without one. Rules read this record and never the syntax.

mod hooks;
mod memory;
pub mod records;
mod runners;
mod suites;
pub use hooks::{Hook, Hooks};

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
    /// Its body is deeper than the normal form reads, so it can match nothing.
    TruncatedBody,
    /// Its callback is passed by name, not written inline: whether that binding is reassigned, or
    /// shared with another test, decides what runs.
    NamedCallback,
    /// It is a benchmark, which is not compared yet.
    Benchmark,
    /// A wrapper call, a module load or a registration function read as a value registers it: the
    /// file does not spell the tests, nor how many there are, so one is counted per site.
    RegisteredElsewhere,
    /// A runner's effects on its calling scope have no complete invocation proof.
    UnprovedRunner,
    /// Direct eval can observe or change bindings outside the static reference graph.
    DynamicBindings,
    /// A module the file imports could not be parsed, so what it exports, and registers, is
    /// unknown.
    UnreadableImport,
    /// A package manifest the file's imports need could not be read, so what they import is
    /// unknown.
    UnreadableManifest,
    /// A non-hoisted Vitest mock changes dynamic imports at the point it runs.
    NonHoistedMock,
    /// A hoisted mock factory is not written as an inline function.
    UnreadableMockFactory,
    /// A fixture value requires evaluating user code the analyzer does not model.
    UnreadableFixture,
    /// The declaration title cannot be proved, so neither its name nor invocation is comparable.
    UnresolvedTitle,
}

impl TestCase {
    /// Whether the quality gate's denominator counts this case: every runtime test, compared or
    /// not, except a benchmark, a placeholder for what a wrapper call or module load registers,
    /// and a todo with no callback.
    pub fn gate_eligible(&self) -> bool {
        !self.bodyless
            && !matches!(
                self.exclusion,
                Some(Exclusion::Benchmark | Exclusion::RegisteredElsewhere | Exclusion::NoCallback)
            )
    }
}

impl Exclusion {
    /// The report's reason code and sentence, for an exclusion that leaves a test unresolved;
    /// `None` for a test with nothing to compare.
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
            Self::TruncatedBody => Some((
                ExclusionReason::UnresolvedContext,
                "a test's body is deeper than the analyzer reads, so it cannot be compared",
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
            Self::UnprovedRunner => Some((
                ExclusionReason::HelperEffectsUnproved,
                "a runner's invocation is unproved; its calling scope is excluded",
            )),
            Self::DynamicBindings => Some((
                ExclusionReason::UnresolvedContext,
                "direct eval can observe or change lexical bindings outside static analysis",
            )),
            Self::UnreadableManifest => Some((
                ExclusionReason::PackageManifestUnreadable,
                "a package manifest the file's imports need could not be read as a UTF-8 regular \
                 file of at most 1 MiB, so what they import is unknown",
            )),
            Self::UnreadableImport => Some((
                ExclusionReason::HelperExportUnreadable,
                "a helper module the file imports could not be parsed, so what it exports and \
                 registers is unknown",
            )),
            Self::NonHoistedMock => Some((
                ExclusionReason::UnresolvedContext,
                "a non-hoisted Vitest mock can change dynamic imports at the point it runs",
            )),
            Self::UnreadableMockFactory => Some((
                ExclusionReason::MockFactoryUnreadable,
                "a `vi.mock` factory could not be read, so the tests it governs were excluded",
            )),
            Self::UnreadableFixture => Some((
                ExclusionReason::FixtureUnreadable,
                "a `test.extend` fixture value could not be read, so the tests it governs were excluded",
            )),
            Self::UnresolvedTitle => Some((
                ExclusionReason::UnresolvedContext,
                "the test title could not be resolved, so the declaration cannot be compared",
            )),
            Self::EmptyBody | Self::NoCallback => None,
        }
    }
}

/// One test, one row of a test table, or a suite in the separate suite population.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    /// The source module spelling the declaration, relative to the analysis root.
    pub path: PathBuf,
    /// Caller modules whose registrations own this declaration's executions.
    pub registration_files: Vec<PathBuf>,
}

/// One test, one row of a test table, or a suite in the separate suite population.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestCase {
    /// A declaration outside the file that registers it; absent for ordinary inline tests.
    pub origin: Option<Origin>,
    /// Optional owned normalization evidence, retained only for diagnostic records.
    pub views: Option<Box<records::Views>>,
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
    /// The normal body's digest: equal for equal text, differing whenever the bodies do, so a
    /// rule that keys on behavior can require the bodies to differ first.
    pub body: u64,
    /// Same-file consolidation key: literal-shaped body under exact context and modifiers.
    /// Absent without behavior or with source-sensitive assertions, unknown selectors or lossy text.
    pub parameterization_key: Option<u64>,
    /// The body's behavior trace, one 128-bit digest per event in source order.
    pub trace: Vec<u128>,
    /// Ordinary behavior steps; opaque binding accounting cannot make a rule eligible.
    pub ordinary_events: usize,
    /// Whether the trace read the body to the end. A rule keyed on the trace must skip a case
    /// where it did not, and say so: a trace with a hole is no evidence either way. Rules keyed on
    /// the flat form do not read it.
    pub trace_complete: bool,
    /// Equal for two tests of one file the near-duplicate rule may pair: chain, expected
    /// failure, options, unbound row cells, context and callback form — everything two
    /// near-duplicates must agree on before their events are compared at all.
    pub similarity_key: u64,
    /// Subsumption context, callback form and unresolved row inputs; absent for placeholder bodies
    /// and for a test expected to fail, by its own declaration or an enclosing suite's.
    pub subsumption_key: Option<u64>,
    /// Whether the callback's arrow and `function` forms act on the same object, and which was
    /// written when they do not.
    pub callback_form: normal::CallbackForm,
    /// Whether the chain keeps the test from running (`skip`, `todo`).
    pub inert: bool,
    /// Whether the declaration has no callback, so Vitest reports it as a todo: decided apart from
    /// which exclusion wins, since another refusal can be recorded first.
    pub bodyless: bool,
    /// Why it is not compared, if it is not.
    pub exclusion: Option<Exclusion>,
}

impl TestCase {
    /// The declaration's source path, independently of which file registers this execution.
    pub fn declaration_path<'a>(&'a self, registered_in: &'a Path) -> &'a Path {
        self.origin
            .as_ref()
            .map_or(registered_in, |origin| origin.path.as_path())
    }
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
    /// Module-wide uncertainty, retained even when no static test site was found.
    pub context_refusals: Vec<Exclusion>,
    /// Exact helper source snapshots used during expansion, once per relative source path.
    pub helper_sources: Vec<(PathBuf, String)>,
    /// Hook declarations and distinct lifecycle chains, separate from executable tests.
    pub hooks: Hooks,
    /// Its tests, in source order, rows in table order.
    pub tests: Vec<TestCase>,
    /// Comparable whole suites, separate from executable tests and exclusion counts.
    pub suites: Vec<TestCase>,
    /// Its registration sites and module loads.
    pub sites: Vec<SiteRecord>,
    /// Known hook declarations, including those outside the redundancy proof population.
    pub diagnostic_hooks: Vec<(PathBuf, records::DiagnosticHook)>,
    /// Table declarations for which normalization proved zero rows.
    pub empty_tables: usize,
    /// Suite containers registered, whether or not a comparable suite came of them.
    pub containers: usize,
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
    sources: lang::Sources,
    diagnostics: bool,
    modules: inventory::Modules,
    exports: normal::Exports,
}

impl Run {
    /// Pin every import consumer to the same view, keeping their caches separate from other runs.
    pub fn with_sources(sources: lang::Sources) -> Self {
        Self {
            modules: inventory::Modules::with_sources(sources.clone()),
            exports: normal::Exports::with_sources(sources.clone()),
            sources,
            diagnostics: false,
        }
    }

    /// Recovery discards uncertain cached answers without changing the source authority.
    fn clear(&mut self) {
        *self = Self::with_sources(self.sources.clone());
    }
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

/// Convert a declaration byte range to the report's one-based UTF-16 coordinates.
fn at_span(lines: &Lines, source: &str, start: u32, end: u32) -> Span {
    let (line, column) = lines.position(source, start);
    let (end_line, end_column) = lines.position(source, end);
    Span {
        start: Position { line, column },
        end: Position {
            line: end_line,
            column: end_column,
        },
        start_byte: start,
        end_byte: end,
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
    read_parsed(path, source, setting, run, &parsed, None)
}

/// Analyze one ordinary file or one already proved registration invocation over its own tree.
fn read_parsed(
    path: &Path,
    source: &str,
    setting: Setting,
    run: &mut Run,
    parsed: &lang::Parsed,
    frame: Option<runners::Frame>,
) -> Result<FileCorpus, Refusal> {
    let mut found = inventory::inventory(
        path,
        source,
        parsed,
        inventory::Setting {
            dialect: setting.dialect,
            globals: setting.globals,
            root: setting.root,
        },
        &mut run.modules,
    );
    // The inventory resolved every import the file makes: a package manifest one of them needed
    // and the bounded reader refused leaves every test of the file unresolved.
    let manifest_refused = frame.is_none() && lang::take_manifest_refusal();
    // Instantiation selects registrations, but every callback in the module still contributes
    // mutation facts used to reject order-dependent comparisons.
    let structure = Structure::of(parsed, &found);
    // Registrations through a test function's member (`test.describe`), by call.
    let members: HashMap<NodeId, NodeId> = found
        .sites
        .iter()
        .filter_map(|site| Some((site.node, site.member?)))
        .collect();
    if let Some(frame) = &frame {
        found.sites.retain(|site| frame.sites.contains(&site.node));
        found.loads.clear();
        found.escapes.clear();
    }
    let expanded = if frame.is_none() {
        runners::expand(path, source, parsed, &found, setting, run)
    } else {
        runners::Batch::default()
    };
    let admitted: std::collections::HashSet<_> = expanded
        .calls
        .iter()
        .map(|expansion| expansion.call)
        .collect();
    let tree = &parsed.tree;
    let lines = Lines::new(source);
    let mut normalizer = Normalizer::new(
        path,
        parsed,
        normal::Setting {
            dialect: setting.dialect,
            root: setting.root,
            exports: &run.exports,
        },
        &structure,
    );
    if let Some(frame) = &frame {
        normalizer = normalizer.for_registration(frame.function, frame.bindings.clone());
    }
    // A statement that is a call registering a test, suite or benchmark is that registration, not
    // context; any other statement holding one (a loop, a sequence with setup) is context whole.
    let registration = |statement: NodeId| {
        tree.kind(statement) == "ExpressionStatement"
            && tree
                .child(statement, "expression")
                .is_some_and(|expression| {
                    let call = tree.inner(expression);
                    admitted.contains(&call)
                        || found.sites.iter().any(|site| {
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
        !admitted.contains(&site.node)
            && matches!(
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
        // A mock Vitest does not hoist runs here, after the static imports, whichever binding
        // spells it: it is its canonical callee and arguments, apart from a hoisted `vi.mock`.
        if let Some(mock) = found
            .in_place_mocks
            .iter()
            .find(|mock| mock.statement == statement)
        {
            let arguments: Vec<String> = tree
                .list(mock.call, "arguments")
                .into_iter()
                .flatten()
                .map(|argument| normalizer.of(argument))
                .collect();
            module_context.push_str("inplace:");
            module_context.push_str(&mock.callee);
            module_context.push('(');
            module_context.push_str(&arguments.join(","));
            module_context.push_str(");");
            continue;
        }
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
                .and_then(|source| run.sources.resolve_file(path, source, setting.root))
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
                    runners::title(parsed, site.node, frame.as_ref())
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
    // A report location: the registering call, from its first byte, so a suppression comment
    // above the declaration applies to it.
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
    let unkeyed_case = |span: Span, title: Option<String>, exclusion: Option<Exclusion>| TestCase {
        origin: None,
        views: None,
        span,
        title,
        full_title: None,
        name: None,
        row: None,
        row_class: None,
        key: 0,
        module_key: 0,
        body: 0,
        parameterization_key: None,
        trace: Vec::new(),
        ordinary_events: 0,
        trace_complete: false,
        similarity_key: 0,
        subsumption_key: None,
        callback_form: normal::CallbackForm::Interchangeable,
        inert: false,
        bodyless: false,
        exclusion,
    };
    let unkeyed = |site: &Site, exclusion: Option<Exclusion>| {
        unkeyed_case(
            at(site.span.start, site.span.end),
            runners::title(parsed, site.node, frame.as_ref()),
            exclusion,
        )
    };

    // Suite callbacks whose table normalization proved empty: Vitest runs none of them, so nothing
    // declared inside one, at any depth, registers.
    let dead: std::collections::HashSet<NodeId> = structure
        .suites
        .iter()
        .filter(|(_, &call)| {
            tree.child(call, "callee")
                .and_then(|callee| {
                    normalizer
                        .modifiers(callee, members.get(&call).copied())
                        .rows
                })
                .is_some_and(|rows| rows.is_empty())
        })
        .map(|(&scope, _)| scope)
        .collect();
    let unregistered = |normalizer: &mut Normalizer<'_>, node: NodeId| {
        normalizer
            .suites_around(node)
            .is_some_and(|scopes| scopes.iter().any(|scope| dead.contains(scope)))
    };

    // Comparison names take only a rendering the values establish; display may also cut long
    // values as the default limit does, which a project can change.
    let render_with = |render: crate::framework::RenderTitle| {
        move |pattern: &str, row: &normal::TableRow, whole_rows| {
            let (arguments, fields) = normal::titles::row(parsed, &row.value, whole_rows);
            render(pattern, &arguments, fields.as_ref(), row.index)
        }
    };
    let render_test_row = render_with(setting.dialect.render_title);
    let display_test_row = render_with(setting.dialect.render_display_title);
    let mut tests = Vec::new();
    let mut empty_tables = 0;
    // Suite registration provenance, by the registering call: the prefix a row finding may
    // anchor on is read from the record, not re-derived from the import's spelling.
    let suite_provenance: HashMap<NodeId, crate::inventory::Provenance> = found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Suite))
        .map(|site| (site.node, site.provenance))
        .collect();
    for site in found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Test))
    {
        if unregistered(&mut normalizer, site.node) {
            continue;
        }
        let mut record = unkeyed(site, None);
        // Display identity survives every refusal; it is not a name-comparison proof.
        let display_prefix = normalizer.suites_around(site.node).and_then(|scopes| {
            scopes
                .iter()
                .map(|scope| {
                    structure
                        .suites
                        .get(scope)
                        .and_then(|call| suite_titles.get(call))
                        .and_then(|title| title.clone())
                })
                .collect::<Option<Vec<_>>>()
        });
        let display_title = |title: &str| {
            let mut segments = display_prefix.clone()?;
            segments.push(title.to_owned());
            Some(segments.join(" > "))
        };
        record.full_title = record.title.as_deref().and_then(display_title);
        let callee = tree.child(site.node, "callee");
        let modifiers = callee.map(|callee| normalizer.modifiers(callee, site.member));
        let arguments: Vec<NodeId> = tree
            .list(site.node, "arguments")
            .into_iter()
            .flatten()
            .collect();
        record.inert = modifiers.as_ref().is_some_and(|modifiers| modifiers.inert)
            || normalizer.inert_options(&arguments);
        // The same test the callback branch below applies, before any refusal can take its place.
        record.bodyless = modifiers.is_some()
            && normalizer.callback(site.node).is_none()
            && !arguments
                .iter()
                .any(|argument| tree.kind(*argument) == "SpreadElement");
        // An excluded test is still one case per row of a table the file spells out.
        let table_rows = modifiers
            .as_ref()
            .and_then(|modifiers| modifiers.rows.as_ref());
        empty_tables += usize::from(table_rows.is_some_and(Vec::is_empty));
        let excluded = |exclusion| -> Vec<TestCase> {
            let case = |row: Option<&normal::TableRow>| {
                let rendered = row.and_then(|row| {
                    let pattern = record.title.as_deref()?;
                    let modifiers = modifiers.as_ref()?;
                    display_test_row(pattern, row, modifiers.whole_rows)
                });
                let mut case = record.clone();
                case.row = row.map(|row| row.index);
                if let Some(title) = rendered.as_deref().and_then(display_title) {
                    case.full_title = Some(title);
                }
                case.exclusion = Some(exclusion);
                case
            };
            match table_rows {
                None => vec![case(None)],
                Some(rows) => rows.iter().map(|row| case(Some(row))).collect(),
            }
        };
        let unread_callee = modifiers.is_none();
        if manifest_refused {
            tests.extend(excluded(Exclusion::UnreadableManifest));
            continue;
        }
        if found.direct_eval
            || found.non_hoisted_mock
            || found.unreadable_mock_factory
            || found.unreadable_fixture
        {
            tests.extend(excluded(if found.direct_eval {
                Exclusion::DynamicBindings
            } else if found.non_hoisted_mock {
                Exclusion::NonHoistedMock
            } else if found.unreadable_mock_factory {
                Exclusion::UnreadableMockFactory
            } else {
                Exclusion::UnreadableFixture
            }));
            continue;
        }
        let refused = expanded.refused.contains(&None)
            || normalizer.suites_around(site.node).is_some_and(|scopes| {
                scopes
                    .iter()
                    .any(|scope| expanded.refused.contains(&Some(*scope)))
            });
        if refused {
            tests.extend(excluded(Exclusion::UnprovedRunner));
            continue;
        }
        let (Some(modifiers), Some(callback)) =
            (modifiers.as_ref(), normalizer.callback(site.node))
        else {
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
        if record.title.is_none() {
            tests.extend(excluded(Exclusion::UnresolvedTitle));
            continue;
        }
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
        let mut suite_expects_failure = false;
        for scope in &suites {
            // An enclosing suite's run condition, table or expected failure applies to the test
            // as well. How deep the suites nest changes nothing; only what each one adds does.
            if let Some(suite_callee) = structure
                .suites
                .get(scope)
                .and_then(|call| tree.child(*call, "callee"))
            {
                let call = structure.suites[scope];
                let suite = normalizer.modifiers(suite_callee, members.get(&call).copied());
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
                suite_expects_failure |= fails;
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
        // A larger passing test proves a smaller one's events only when both expect to pass.
        let expects_failure = suite_expects_failure || !fails.is_empty();
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
                        .and_then(|parameters| normalizer.bind_row(parameters, &row.value))
                    {
                        Some((bound, every)) => (bound, every, String::new()),
                        None => {
                            let cells: Vec<NodeId> = match &row.value {
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
        // Name comparison consumes values, not display fallbacks. In particular an unread table
        // or a suite table cannot stand for its runtime names through a raw pattern.
        let name_prefix: Option<Vec<String>> = suites
            .iter()
            .map(|scope| suite_names.get(structure.suites.get(scope)?)?.clone())
            .collect();
        let pattern = runners::title(parsed, site.node, frame.as_ref());
        let row_prefix_known = suites.iter().all(|scope| {
            structure.suites.get(scope).is_some_and(|&call| {
                suite_provenance.get(&call).is_some_and(|&provenance| {
                    normal::row_identity::suite_prefix_known(parsed, call, provenance)
                })
            })
        });
        // Each row's body, rendered flat and traced. An incomplete trace says nothing about the
        // flat form, so it excludes nothing here: only a rule reading traces skips the row. A flat
        // form past the depth bound can match nothing, so its test is excluded out loud.
        let parameterization_uncertain = normalizer.parameterization_uncertain(body);
        let mut traced: Vec<(String, crate::behavior::Trace, normal::CallbackForm)> = Vec::new();
        let mut truncated = false;
        // The trace walks the function itself, not the argument as written: a parenthesized or
        // type-wrapped callback would otherwise read as a body with no statements in it.
        let traced_body = function.unwrap_or(body);
        for (bound, every, _) in &rows {
            // Cleared first: the flag is shared, and only this render decides this test.
            normalizer.take_truncated();
            let normal_body = normalizer.substituted(body, bound.clone(), *every);
            truncated |= normalizer.take_truncated();
            let (trace, callback_form) =
                crate::behavior::trace(&mut normalizer, parsed, traced_body, bound.clone(), *every);
            traced.push((normal_body, trace, callback_form));
        }
        if truncated {
            tests.extend(excluded(Exclusion::TruncatedBody));
            continue;
        }
        for (index, ((_, every, unbound), (normal_body, trace, callback_form))) in
            rows.iter().zip(traced).enumerate()
        {
            let name =
                name_prefix
                    .clone()
                    .zip(pattern.as_deref())
                    .and_then(|(mut segments, pattern)| {
                        let title = if table_site(site) {
                            let row = modifiers.rows.as_ref()?.get(index)?;
                            render_test_row(pattern, row, modifiers.whole_rows)?
                        } else {
                            pattern.to_owned()
                        };
                        segments.push(title);
                        Some(segments)
                    });
            let full_title = table_site(site)
                .then(|| {
                    let row = modifiers.rows.as_ref()?.get(index)?;
                    display_test_row(pattern.as_deref()?, row, modifiers.whole_rows)
                })
                .flatten()
                .and_then(|title| display_title(&title))
                .or_else(|| record.full_title.clone());
            let row_class = (|| {
                if !row_prefix_known {
                    return None;
                }
                let row = modifiers.rows.as_ref()?.get(index)?;
                let identity = normal::row_identity::identity(parsed, &row.value, function?)?;
                let (arguments, fields) =
                    normal::titles::row(parsed, &row.value, modifiers.whole_rows);
                let rendered = (setting.dialect.render_row_title)(
                    pattern.as_deref()?,
                    &arguments,
                    fields.as_ref(),
                    row.index,
                )?;
                if name.as_ref()?.last()? != &rendered {
                    return None;
                }
                Some(digest(&identity))
            })();
            // A row whose every parameter is bound is the test it would be written out as.
            let chain = if table && rows[index].1 {
                &modifiers.untabled
            } else {
                &modifiers.chain
            };
            let key = format!(
                "{chain}{fails}\u{1}{options}\u{1}{unbound}\u{1}{normal_body}\u{1}{context}"
            );
            let form = callback_form.token();
            let parameterization_eligible =
                trace.complete && !parameterization_uncertain && trace.ordinary_events != 0;
            let shape = (parameterization_eligible || run.diagnostics).then(|| {
                let bound = modifiers
                    .rows
                    .as_ref()
                    .and_then(|rows| rows.get(index))
                    .and_then(|row| normalizer.bind_row_shape(parameters.as_deref()?, &row.value))
                    .map(|(bound, _)| bound)
                    .unwrap_or_default();
                normalizer.take_truncated();
                let shape = normalizer.shape(body, bound, *every);
                let complete = !normalizer.take_truncated();
                (shape, complete)
            });
            let parameterization_key = parameterization_eligible.then(|| {
                let shape = &shape.as_ref().expect("eligible shape was rendered").0;
                digest(&format!(
                    "{chain}{fails}\u{1}{options}\u{1}{unbound}\u{1}{shape}\u{1}{context}"
                ))
            });
            let views = run.diagnostics.then(|| {
                let (shape, complete) = shape.as_ref().expect("diagnostic shape was rendered");
                Box::new(records::Views {
                    body: digest(&normal_body),
                    shape: complete.then(|| digest(shape)),
                    trace: trace
                        .complete
                        .then(|| digest(&format!("{:?}", trace.events))),
                    context: digest(&format!("{context}\u{1}{module_context}")),
                    modifiers: digest(&format!("{chain}{fails}\u{1}{options}\u{1}{unbound}")),
                })
            });
            tests.push(TestCase {
                views,
                row: modifiers.rows.as_ref().map(|rows| rows[index].index),
                row_class,
                key: digest(&key),
                module_key: digest(&format!("{key}\u{1}{module_context}")),
                body: digest(&normal_body),
                parameterization_key,
                trace: trace.events,
                ordinary_events: trace.ordinary_events,
                trace_complete: trace.complete,
                similarity_key: digest(&format!(
                    "{chain}{fails}\u{1}{options}\u{1}{unbound}\u{1}{context}\u{1}{form}"
                )),
                subsumption_key: (!expects_failure
                    && !(setting.dialect.placeholder)(&parsed.tree, traced_body)
                    && (!table_site(site) || modifiers.rows.is_some()))
                .then(|| digest(&format!("{chain}\u{1}{unbound}\u{1}{context}\u{1}{form}"))),
                callback_form,
                name,
                full_title,
                inert: record.inert,
                ..record.clone()
            });
        }
    }

    // What a wrapper call, a module load or an escaped registration function registers is spelled
    // in another module: one excluded case per site, since how many tests it is is not known.
    let wrappers: Vec<_> = found
        .sites
        .iter()
        .filter(|site| {
            site.kind == SiteKind::Wrapper(Reach::Tasks)
                && !admitted.contains(&site.node)
                && !unregistered(&mut normalizer, site.node)
        })
        .map(|site| site.position)
        .collect();
    let elsewhere = wrappers
        .into_iter()
        .chain(found.loads.iter().copied())
        .chain(found.escapes.iter().map(|escape| escape.offset));
    for start in elsewhere {
        tests.push(unkeyed_case(
            at(start, start),
            None,
            Some(Exclusion::RegisteredElsewhere),
        ));
    }
    // Benchmarks run and are counted, one excluded case each, until they are compared.
    for site in found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Bench))
    {
        if unregistered(&mut normalizer, site.node) {
            continue;
        }
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
    // A suite table whose rows normalization proved empty registers nothing, like a test table;
    // every other suite declaration is a container.
    let mut containers = 0;
    for site in found
        .sites
        .iter()
        .filter(|site| site.kind == SiteKind::Registers(Registers::Suite))
    {
        if unregistered(&mut normalizer, site.node) {
            continue;
        }
        let empty = tree
            .child(site.node, "callee")
            .and_then(|callee| normalizer.modifiers(callee, site.member).rows)
            .is_some_and(|rows| rows.is_empty());
        if empty {
            empty_tables += 1;
        } else {
            containers += 1;
        }
    }
    let suites = suites::collect(
        parsed,
        &found,
        &structure,
        &contexts,
        &tests,
        &mut normalizer,
        (setting, source),
    );
    let mut helper_sources = BTreeMap::new();
    let mut imported_hooks = Vec::new();
    let scheduling_safe = expanded.calls.iter().all(|call| call.scheduling_safe);
    let mut imported_diagnostic_hooks = Vec::new();
    let imported_tests: Vec<_> = expanded
        .calls
        .iter()
        .filter(|expansion| {
            expansion
                .tests
                .iter()
                .any(|test| !test.inert && test.exclusion.is_none())
        })
        .map(|expansion| expansion.call)
        .collect();
    let admitted_calls: Vec<_> = expanded
        .calls
        .iter()
        .map(|expansion| expansion.call)
        .collect();
    for expansion in expanded.calls {
        tests.extend(expansion.tests);
        helper_sources.extend(expansion.sources);
        imported_hooks.extend(expansion.hooks);
        imported_diagnostic_hooks.extend(expansion.diagnostic_hooks);
    }
    let imported_context = lifecycle_context(&imported_hooks);
    if !imported_context.is_empty() {
        contextualize_imported_tests(&mut tests, &imported_context);
    }
    let hooks = hooks::collect(
        parsed,
        &found,
        &structure,
        &tests,
        &mut normalizer,
        (setting.dialect, source),
        (
            &imported_hooks,
            &imported_tests,
            &admitted_calls,
            scheduling_safe,
        ),
    );
    for (path, diagnostic) in &mut imported_diagnostic_hooks {
        diagnostic.effect_key = hooks
            .declarations
            .iter()
            .enumerate()
            .find_map(|(index, hook)| {
                (hook.path.as_ref() == Some(path)
                    && hook.span == diagnostic.span
                    && hooks.chains.iter().any(|chain| chain.contains(&index)))
                .then(|| hook.key.as_deref().map(digest))
                .flatten()
            });
    }
    let diagnostic_hooks = if run.diagnostics {
        records::hooks(
            parsed,
            &found,
            &hooks,
            &mut normalizer,
            setting.dialect,
            source,
            &lines,
        )
    } else {
        Vec::new()
    };
    let diagnostic_path = run.sources.relative(path, setting.root).unwrap_or_else(|| {
        path.strip_prefix(setting.root)
            .unwrap_or(path)
            .to_path_buf()
    });
    Ok(FileCorpus {
        diagnostic_hooks: diagnostic_hooks
            .into_iter()
            .map(|hook| (diagnostic_path.clone(), hook))
            .chain(imported_diagnostic_hooks)
            .collect(),
        context_refusals: [
            found.direct_eval.then_some(Exclusion::DynamicBindings),
            // Taken once per file, after every module its reading followed.
            (frame.is_none() && inventory::take_unread_imports())
                .then_some(Exclusion::UnreadableImport),
            // A refused manifest leaves the read partial, whether the inventory or a later
            // resolution needed it, and whether or not the file has a test to leave out.
            (frame.is_none() && (lang::take_manifest_refusal() || manifest_refused))
                .then_some(Exclusion::UnreadableManifest),
            found.non_hoisted_mock.then_some(Exclusion::NonHoistedMock),
            found
                .unreadable_mock_factory
                .then_some(Exclusion::UnreadableMockFactory),
            found
                .unreadable_fixture
                .then_some(Exclusion::UnreadableFixture),
        ]
        .into_iter()
        .flatten()
        .collect(),
        helper_sources: helper_sources.into_iter().collect(),
        hooks,
        suites,
        tests,
        sites,
        empty_tables,
        containers,
        parse_errors: parsed.parse_errors,
    })
}

/// Context surrounding imported registrations belongs to their executions, even though their
/// declarations were normalized in another module.
fn lifecycle_context(imported: &[runners::ImportedHook]) -> String {
    let context: Vec<_> = imported
        .iter()
        .map(|imported| {
            format!(
                "{:?}\u{1}{}",
                imported.hook.timing,
                imported.hook.key.as_deref().unwrap_or("unknown")
            )
        })
        .collect();
    context.join("\u{1}")
}

/// Prevent tests under an imported lifecycle from borrowing equivalence across another caller.
fn contextualize_imported_tests(tests: &mut [TestCase], context: &str) {
    let contextualize = |key: u64| digest(&format!("{key}\u{1}{context}"));
    for test in tests {
        test.key = contextualize(test.key);
        test.module_key = contextualize(test.module_key);
        test.parameterization_key = test.parameterization_key.map(contextualize);
        test.similarity_key = contextualize(test.similarity_key);
        test.subsumption_key = test.subsumption_key.map(contextualize);
        if let Some(views) = &mut test.views {
            views.context = contextualize(views.context);
        }
    }
}

/// The engine inside the shell's file loop: what it read, summed over the run.
#[derive(Debug, Default)]
pub struct Engine {
    /// Hook declarations and chains per relative file.
    hooks: Vec<(PathBuf, Hooks)>,
    run: Run,
    /// Tests read, rows counted one each.
    pub tests: usize,
    /// The tests the quality gate's denominator counts: see [`TestCase::gate_eligible`].
    pub eligible: usize,
    /// Tests left unresolved: excluded for a reason that has a report code.
    pub unresolved: usize,
    /// Proven empty table declarations, independently of executable test counts.
    pub empty_tables: usize,
    /// Suite containers registered, whether or not a comparable suite came of them.
    pub containers: usize,
    /// Registration sites, by the file's path relative to the root, when they are kept.
    sites: Vec<(PathBuf, SiteRecord)>,
    diagnostic_hooks: Vec<(PathBuf, records::DiagnosticHook)>,
    /// Each file's cases, by the file's path relative to the root: what the rules read once
    /// every file has been read. Retained payload and capacity feed the memory roof.
    files: Vec<(PathBuf, Vec<TestCase>)>,
    /// Comparable suite containers, retained separately from the executable population.
    suites: Vec<(PathBuf, Vec<TestCase>)>,
    /// Files every test of which is unresolved.
    blind: Vec<BlindFile>,
    /// Files with nothing to compare whose only declarations are tables proved to have no rows.
    proven_empty: Vec<PathBuf>,
    /// Owned element heaps, excluding outer vector spines and separately measured caches.
    retained: crate::memory::EngineBytes,
}

/// One file's shell-facing diagnostics and exact helper snapshots, before optional retention.
#[derive(Debug)]
pub struct FileRead {
    /// The file's exclusion entry, if any part was not comparable.
    pub excluded_file: Option<ExcludedFile>,
    /// Parsing or extraction read only part of the file.
    pub partial: bool,
    /// Exact source used by imported declarations, for directives and optional HTML snippets.
    pub helper_sources: Vec<(PathBuf, String)>,
}

/// Why a file's read did not finish.
#[derive(Debug)]
enum Failure {
    /// The thread it runs on could not be started.
    Start(String),
    /// It panicked and was recovered.
    Panicked,
}

impl Engine {
    /// An engine whose imports and resolver metadata come from one explicit source view.
    pub fn with_sources(sources: lang::Sources) -> Self {
        Self {
            run: Run::with_sources(sources),
            ..Self::default()
        }
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
        let read = self.read_with_sources(path, relative, source, setting, keep_sites, further);
        (read.excluded_file, read.partial)
    }

    /// Read with exact helper snapshots for source directives and source-aware reporters.
    /// Snapshots remain transient unless the shell explicitly retains them for a report.
    pub fn read_with_sources(
        &mut self,
        path: &Path,
        relative: &Path,
        source: &str,
        setting: Setting,
        keep_sites: bool,
        further: Vec<(ExclusionReason, String)>,
    ) -> FileRead {
        // On a stack sized for the parser: [`crate::lang::limits`].
        self.run.diagnostics = keep_sites;
        let run = &mut self.run;
        let confined = std::sync::atomic::AtomicBool::new(false);
        let confined_flag = &confined;
        let mut read = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("vitest-dedup-corpus".to_owned())
                .stack_size(crate::lang::limits::EXTRACTION_STACK_BYTES)
                .spawn_scoped(scope, move || {
                    // A tsconfig-confined resolution leaves the root through the project's own
                    // configuration; the read is partial.
                    let corpus = file(path, source, setting, run);
                    confined_flag.store(
                        crate::lang::take_confinement_refusal(),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    corpus
                })
                .map_err(|error| Failure::Start(error.to_string()))?
                .join()
                .map_err(|_| Failure::Panicked)
        });
        let confined = confined.load(std::sync::atomic::Ordering::Relaxed);
        let helper_sources = match &mut read {
            Ok(Ok(corpus)) => std::mem::take(&mut corpus.helper_sources),
            _ => Vec::new(),
        };
        // A budget refusal is named by the bound it hit.
        let limit = matches!(read, Ok(Err(Refusal::SourceLimit)))
            .then(|| crate::lang::limits::parse_refusal(source))
            .flatten();
        let (excluded_file, partial) =
            self.record(relative, read, keep_sites, further, confined, limit);
        let partial = partial || confined;
        FileRead {
            excluded_file,
            partial,
            helper_sources,
        }
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
        confined: bool,
        limit: Option<String>,
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
                    // The parser refuses only what `parse_refusal` names, so the text is there.
                    Refusal::SourceLimit => {
                        unparsed(ExclusionReason::SourceLimit, limit.unwrap_or_default())
                    }
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
                self.run.clear();
                return unparsed(
                    ExclusionReason::ExtractionFailed,
                    "the file could not be read: the analysis failed and was recovered".to_owned(),
                );
            }
        };
        self.empty_tables += corpus.empty_tables;
        self.containers += corpus.containers;
        let mut reasons: Vec<(ExclusionReason, String)> = Vec::new();
        let context_partial = !corpus.context_refusals.is_empty();
        for refusal in &corpus.context_refusals {
            if let Some((reason, detail)) = refusal.reason() {
                reasons.push((reason, detail.to_owned()));
            }
        }
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
        // A file's own refusals, before the shell's reasons (an undetermined `globals` is no part of
        // why a file has nothing to compare).
        if corpus.tests.is_empty()
            && corpus.containers == 0
            && reasons.is_empty()
            && corpus.empty_tables > 0
        {
            let path = relative.to_path_buf();
            self.retained.blind = self.retained.blind.saturating_add(path.heap_bytes());
            self.proven_empty.push(path);
        }
        reasons.extend(further);
        // Named so a comparable file read under a configuration that reaches past the root is
        // visible, not an unattributed partial count.
        if confined {
            reasons.push((
                ExclusionReason::ImportOutsideRoot,
                "an import resolved outside the analysis root through the tsconfig and was not read"
                    .to_owned(),
            ));
        }
        // Last, so it never outranks a cause that excludes a test, and not counted: only the rules
        // that read a trace leave the case out.
        if corpus
            .tests
            .iter()
            .any(|test| test.exclusion.is_none() && !test.trace_complete)
        {
            reasons.push((
                ExclusionReason::IncompleteTrace,
                "a test's behavior was not read to the end, so near-duplicate-test, subsumed-test and \
                 parameterization-candidate do not compare it"
                    .to_owned(),
            ));
        }
        use crate::memory::HeapBytes;
        let case_count = corpus.tests.len();
        self.eligible += corpus
            .tests
            .iter()
            .filter(|case| case.gate_eligible())
            .count();
        let mut corpus = corpus;
        let cases = (relative.to_path_buf(), std::mem::take(&mut corpus.tests));
        self.retained.cases = self.retained.cases.saturating_add(cases.heap_bytes());
        self.files.push(cases);
        let suites = (relative.to_path_buf(), std::mem::take(&mut corpus.suites));
        self.retained.suites = self.retained.suites.saturating_add(suites.heap_bytes());
        self.suites.push(suites);
        let hooks = (relative.to_path_buf(), std::mem::take(&mut corpus.hooks));
        self.retained.hooks = self.retained.hooks.saturating_add(hooks.heap_bytes());
        self.hooks.push(hooks);
        for hook in corpus.diagnostic_hooks {
            self.retained.hooks = self.retained.hooks.saturating_add(hook.heap_bytes());
            self.diagnostic_hooks.push(hook);
        }
        self.tests += case_count;
        self.unresolved += unresolved;
        if unresolved > 0 && unresolved == case_count {
            let blind = BlindFile {
                path: relative.to_path_buf(),
                tests: unresolved,
            };
            self.retained.blind = self.retained.blind.saturating_add(blind.heap_bytes());
            self.blind.push(blind);
        }
        if keep_sites {
            for site in corpus.sites {
                let site = (relative.to_path_buf(), site);
                self.retained.sites = self.retained.sites.saturating_add(site.heap_bytes());
                self.sites.push(site);
            }
        }
        let entry =
            exclusions::counted_file(relative.to_path_buf(), reasons, unresolved, case_count);
        (entry, corpus.parse_errors > 0 || context_partial)
    }

    /// Files, by path relative to the root, whose only declarations are statically empty tables.
    pub fn proven_empty_files(&self) -> &[PathBuf] {
        &self.proven_empty
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
