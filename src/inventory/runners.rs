//! Owned registration templates, before a caller binds their inputs and context.
//!
//! A wrapper's reach says that it may register, not that expanding it is safe. This reader
//! records a bounded, ordered body and validates the modules that loading it evaluates.
//! Instantiation must still prove argument binding, nested calls and caller context.

use super::{inventory, Inventory, Modules, Setting, SiteKind};
use crate::framework::Registers;
use crate::lang::{self, Declaration, Name, NodeId, Parsed, Sources, SymbolId};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

mod returned;
pub use returned::{returned_template, ReturnedTemplate};

/// Why a runner template could not be established.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Source or a project dependency is unavailable from the selected source authority.
    Source,
    /// The parser could not read the complete module.
    Parse,
    /// The export is missing, type-only, ambiguous or cyclic.
    Export,
    /// The binding is mutable or does not name a supported synchronous function.
    Callable,
    /// Parameter binding requires defaults, rest or destructuring.
    Parameters,
    /// Importing a module may execute unaccounted effects or registrations.
    ModuleEffects,
    /// The body is not a closed sequence of supported registration operations.
    Body,
    /// An export, module or syntax walk exceeded its bound.
    Limit,
}

/// One operation that runs when a template is invoked, in execution order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// A framework registration, including a suite whose children follow it.
    Registration(NodeId, Registers),
    /// A nested wrapper call, which needs a separate invocation proof.
    Call(NodeId),
}

/// One plain positional parameter, including bounded object destructuring.
#[derive(Clone, Debug)]
pub struct Parameter {
    /// Values bound from this actual argument.
    pub bindings: Vec<ParameterBinding>,
}

/// One symbol selected from a positional value.
#[derive(Clone, Debug)]
pub struct ParameterBinding {
    /// The template-local binding receiving the selected value.
    pub symbol: SymbolId,
    /// Static object keys below the positional argument.
    pub path: Vec<String>,
    /// A literal/default expression in this template's tree.
    pub default: Option<NodeId>,
}

/// A first-party callable's owned source and ordered registration template.
///
/// This is not an admitted invocation: parameters are symbols in this module, not caller
/// values, and [`Step::Call`] requires another bounded proof before any findings can be emitted.
#[derive(Debug)]
pub struct Template {
    /// The module containing the callable, under the selected source authority.
    pub path: PathBuf,
    /// Exact bytes used to parse it, also the authority for locations and source directives.
    pub source: String,
    /// Owned tree and bindings; no parser-library node escapes this record.
    pub parsed: Parsed,
    /// The function or arrow invoked.
    pub function: NodeId,
    /// Plain positional bindings, in parameter order.
    pub parameters: Vec<Parameter>,
    /// Reachable registration operations; statements after an unconditional return are absent.
    pub steps: Vec<Step>,
    /// Project modules whose import-time behavior was checked, including re-export hops.
    pub dependencies: Vec<PathBuf>,
    /// Export-resolution steps charged to the invocation's shared depth budget.
    pub resolution_depth: usize,
    /// Every source read by this proof, for the caller's shared unique-source budget.
    pub dependency_bytes: Vec<(PathBuf, usize)>,
}

/// A first-party callable whose module and export path are proved, before its body is interpreted.
pub struct CallableTemplate {
    /// The resolved module.
    pub path: PathBuf,
    /// Exact parsed source.
    pub source: String,
    /// Owned tree and bindings.
    pub parsed: Parsed,
    /// The resolved implementation function.
    pub function: NodeId,
    /// Every eagerly evaluated project dependency.
    pub dependencies: Vec<PathBuf>,
    /// Export-resolution steps charged to the caller.
    pub resolution_depth: usize,
    /// Every source read by the proof.
    pub dependency_bytes: Vec<(PathBuf, usize)>,
}

/// A callable plan within an already checked module; symbols belong to that module alone.
#[derive(Clone)]
pub(crate) struct Plan {
    /// The invocation boundary, within this module's tree.
    pub function: NodeId,
    /// Plain positional parameter bindings, in call order.
    pub parameters: Vec<Parameter>,
    /// Reachable operations, in execution order.
    pub steps: Vec<Step>,
}

/// A returned-object plan within an already checked module.
pub(crate) struct ReturnedPlan {
    /// The invocation boundary, within this module's tree.
    pub function: NodeId,
    /// Plain positional parameter bindings, in call order.
    pub parameters: Vec<Parameter>,
    /// The inert object expression returned on the proved path.
    pub object: NodeId,
    /// Inert declarations evaluated once for this factory invocation.
    pub setup: Vec<NodeId>,
    /// Actual work spent proving inert setup and its returned object.
    pub work: usize,
}

/// Read a local callable without guessing that a function-local const has been initialized.
fn local_binding(parsed: &Parsed, found: &Inventory, callee: NodeId) -> Result<(), Refusal> {
    if found.import_limit.is_some() {
        return Err(Refusal::Limit);
    }
    let symbol = parsed
        .scopes
        .symbol_at(parsed.tree.span(callee).start)
        .ok_or(Refusal::Callable)?;
    let binding = parsed.scopes.symbol(symbol);
    if matches!(binding.declaration, Declaration::Variable(_)) && !binding.top_level {
        return Err(Refusal::Callable);
    }
    Ok(())
}

/// Build a direct runner plan from its checked local binding.
pub(crate) fn local(parsed: &Parsed, found: &Inventory, callee: NodeId) -> Result<Plan, Refusal> {
    local_binding(parsed, found, callee)?;
    let function = callable(parsed, callee, None)?;
    let parameters = parameters(parsed, function, found.direct_eval)?;
    let mut steps = Vec::new();
    let reader = BodyReader { parsed, found };
    reader.body(function, &mut steps, &mut 2000)?;
    Ok(Plan {
        function,
        parameters,
        steps,
    })
}

/// Read a local callable whose only runtime work before returning is inert construction.
pub(crate) fn returned_local(
    parsed: &Parsed,
    found: &Inventory,
    callee: NodeId,
    budget: usize,
) -> Result<ReturnedPlan, Refusal> {
    local_binding(parsed, found, callee)?;
    let tree = &parsed.tree;
    let before = tree.span(callee).start;
    let function = callable(parsed, callee, Some(before))?;
    let function_span = tree.span(function);
    if parsed.scopes.symbols().any(|symbol| {
        let binding = parsed.scopes.symbol(symbol);
        matches!(
            binding.declaration,
            Declaration::Variable(_) | Declaration::Class(_)
        ) && !function_span.contains(binding.span)
            && binding
                .references
                .iter()
                .any(|reference| function_span.contains(reference.span))
            && parsed
                .scopes
                .declaration_node(symbol)
                .is_none_or(|node| tree.span(node).end > before)
    }) {
        return Err(Refusal::Callable);
    }
    let parameters = returned_parameters(parsed, function, found.direct_eval)?;
    let (object, setup, work) =
        returned::returned_object(parsed, function, &parameters[..], budget)?;
    Ok(ReturnedPlan {
        function,
        parameters,
        object,
        setup,
        work,
    })
}

/// Read a named or default first-party runner through one explicit source authority.
///
/// Every re-export hop and eagerly loaded project dependency is checked before returning a
/// template. Uncertainty returns a refusal, never a partial registration sequence.
pub fn template(
    sources: &Sources,
    path: &Path,
    export: &str,
    setting: Setting<'_>,
) -> Result<Template, Refusal> {
    let resolved = resolved_template(sources, path, export, setting)?;
    let parameters = parameters(
        &resolved.parsed,
        resolved.function,
        resolved.found.direct_eval,
    )?;
    let mut steps = Vec::new();
    let body = BodyReader {
        parsed: &resolved.parsed,
        found: &resolved.found,
    };
    body.body(resolved.function, &mut steps, &mut 2000)?;
    let mut dependencies: Vec<_> = resolved.dependencies.into_iter().collect();
    dependencies.sort();
    Ok(Template {
        path: resolved.path,
        source: resolved.source,
        parsed: resolved.parsed,
        function: resolved.function,
        parameters,
        steps,
        dependencies,
        resolution_depth: resolved.resolution_depth,
        dependency_bytes: resolved.dependency_bytes.into_iter().collect(),
    })
}

/// Read a named or default first-party callable that returns one inert object value.
pub(crate) fn factory_template(
    sources: &Sources,
    path: &Path,
    export: &str,
    setting: Setting<'_>,
    budget: usize,
) -> Result<ReturnedTemplate<Parameter>, Refusal> {
    let resolved = resolved_template(sources, path, export, setting)?;
    let parameters = returned_parameters(
        &resolved.parsed,
        resolved.function,
        resolved.found.direct_eval,
    )?;
    let (object, setup, work) =
        returned::returned_object(&resolved.parsed, resolved.function, &parameters[..], budget)?;
    let mut dependencies: Vec<_> = resolved.dependencies.into_iter().collect();
    dependencies.sort();
    Ok(ReturnedTemplate {
        path: resolved.path,
        source: resolved.source,
        parsed: resolved.parsed,
        function: resolved.function,
        parameters,
        object,
        setup,
        work,
        dependencies,
        resolution_depth: resolved.resolution_depth,
        dependency_bytes: resolved.dependency_bytes.into_iter().collect(),
    })
}

/// Resolve a first-party callable while retaining its exact implementation for a specialized
/// bounded body proof.
pub fn callable_template(
    sources: &Sources,
    path: &Path,
    export: &str,
    setting: Setting<'_>,
) -> Result<CallableTemplate, Refusal> {
    let resolved = resolved_template(sources, path, export, setting)?;
    let mut dependencies: Vec<_> = resolved.dependencies.into_iter().collect();
    dependencies.sort();
    Ok(CallableTemplate {
        path: resolved.path,
        source: resolved.source,
        parsed: resolved.parsed,
        function: resolved.function,
        dependencies,
        resolution_depth: resolved.resolution_depth,
        dependency_bytes: resolved.dependency_bytes.into_iter().collect(),
    })
}

/// Resolved source/module facts, before parameter and registration-body admission.
struct ResolvedTemplate {
    path: PathBuf,
    source: String,
    parsed: Parsed,
    function: NodeId,
    found: Inventory,
    dependencies: HashSet<PathBuf>,
    resolution_depth: usize,
    dependency_bytes: HashMap<PathBuf, usize>,
}

/// Resolve a callable and retain the completed dependency and inventory proof.
fn resolved_template(
    sources: &Sources,
    path: &Path,
    export: &str,
    setting: Setting<'_>,
) -> Result<ResolvedTemplate, Refusal> {
    let mut reader = Reader {
        sources,
        setting,
        checked: HashSet::new(),
        active: HashSet::new(),
        read_paths: HashSet::new(),
        read_sizes: HashMap::new(),
        bytes: 0,
        visits: 0,
    };
    let mut at = (path.to_path_buf(), export.to_owned());
    let mut hops = HashSet::new();
    for depth in 1..=8 {
        if !hops.insert(at.clone()) {
            return Err(Refusal::Export);
        }
        let (source, parsed) = reader.read(&at.0)?;
        reader.module(&at.0, &parsed)?;
        match exported(&parsed, &at.1)? {
            Export::Here(function) => {
                let mut modules = Modules::with_sources(sources.clone());
                let found = inventory(&at.0, &source, &parsed, setting, &mut modules);
                if found.import_limit.is_some() {
                    return Err(Refusal::Limit);
                }
                return Ok(ResolvedTemplate {
                    path: at.0,
                    source,
                    parsed,
                    function,
                    found,
                    dependencies: reader.checked,
                    resolution_depth: depth,
                    dependency_bytes: reader.read_sizes,
                });
            }
            Export::From(specifier, name) => {
                at = (
                    sources
                        .resolve(&at.0, &specifier, setting.root)
                        .ok_or(Refusal::Source)?,
                    name,
                );
            }
        }
    }
    Err(Refusal::Limit)
}

/// One bounded reader for an import closure; its scratch trees never join the retained corpus.
struct Reader<'a> {
    sources: &'a Sources,
    setting: Setting<'a>,
    checked: HashSet<PathBuf>,
    active: HashSet<PathBuf>,
    read_paths: HashSet<PathBuf>,
    read_sizes: HashMap<PathBuf, usize>,
    bytes: usize,
    visits: usize,
}

impl Reader<'_> {
    /// Read only from the selected authority, refusing recovered parse errors too.
    fn read(&mut self, path: &Path) -> Result<(String, Parsed), Refusal> {
        let source = self.sources.read(path).ok_or(Refusal::Source)?;
        if self.read_paths.insert(path.to_path_buf()) {
            self.read_sizes.insert(path.to_path_buf(), source.len());
            self.bytes = self.bytes.saturating_add(source.len());
            if self.bytes > 8 * 1024 * 1024 {
                return Err(Refusal::Limit);
            }
        }
        let parsed = lang::parse(path, &source).map_err(|_| Refusal::Parse)?;
        if parsed.parse_errors != 0 {
            return Err(Refusal::Parse);
        }
        Ok((source, parsed))
    }

    /// Prove the eagerly evaluated module closure inert; a cycle cannot borrow an unfinished proof.
    fn module(&mut self, path: &Path, parsed: &Parsed) -> Result<(), Refusal> {
        if self.checked.contains(path) {
            return Ok(());
        }
        if super::setup_access(parsed, self.setting) {
            return Err(Refusal::ModuleEffects);
        }
        self.visits += 1;
        if self.visits > 64 {
            return Err(Refusal::Limit);
        }
        if !self.active.insert(path.to_path_buf()) {
            return Err(Refusal::ModuleEffects);
        }
        let tree = &parsed.tree;
        for statement in tree.list(tree.root(), "body").into_iter().flatten() {
            let kind = tree.kind(statement);
            if tree.text(statement, "importKind") == Some("type")
                || tree.text(statement, "exportKind") == Some("type")
            {
                continue;
            }
            if matches!(kind, "ImportDeclaration" | "ExportNamedDeclaration") {
                if let Some(specifier) = tree
                    .child(statement, "source")
                    .and_then(|node| tree.text(node, "value"))
                {
                    let specifiers: Vec<_> = tree
                        .list(statement, "specifiers")
                        .into_iter()
                        .flatten()
                        .filter(|&specifier| {
                            tree.text(specifier, "importKind") != Some("type")
                                && tree.text(specifier, "exportKind") != Some("type")
                        })
                        .collect();
                    if !tree.list(statement, "specifiers").is_empty() && specifiers.is_empty() {
                        continue;
                    }
                    if !self.setting.dialect.is_api_module(specifier) {
                        // Link every runtime binding, even an unused one. Only callable exports
                        // have an owned linkage proof here; data and namespaces remain refused.
                        let target = self
                            .sources
                            .resolve(path, specifier, self.setting.root)
                            .ok_or(Refusal::Source)?;
                        let (_, dependency) = self.read(&target)?;
                        self.module(&target, &dependency)?;
                        for specifier in specifiers {
                            let name = match (kind, tree.kind(specifier)) {
                                ("ImportDeclaration", "ImportDefaultSpecifier") => {
                                    "default".to_owned()
                                }
                                ("ImportDeclaration", "ImportSpecifier") => {
                                    let imported =
                                        tree.child(specifier, "imported").ok_or(Refusal::Export)?;
                                    lang::export_name(tree, imported)
                                }
                                ("ImportDeclaration", _) => return Err(Refusal::Export),
                                _ => {
                                    let local =
                                        tree.child(specifier, "local").ok_or(Refusal::Export)?;
                                    lang::export_name(tree, local)
                                }
                            };
                            self.export_exists(&target, &dependency, &name, 0)?;
                        }
                    }
                    continue;
                }
                if kind == "ExportNamedDeclaration" {
                    for specifier in tree.list(statement, "specifiers").into_iter().flatten() {
                        if tree.text(specifier, "exportKind") == Some("type") {
                            continue;
                        }
                        let local = tree.child(specifier, "local").ok_or(Refusal::Export)?;
                        if parsed
                            .scopes
                            .top_level(&lang::export_name(tree, local))
                            .is_none()
                        {
                            return Err(Refusal::Export);
                        }
                    }
                }
            }
            let declaration =
                if matches!(kind, "ExportNamedDeclaration" | "ExportDefaultDeclaration") {
                    tree.child(statement, "declaration")
                } else {
                    Some(statement)
                };
            if declaration.is_some_and(|node| !inert_declaration(parsed, node)) {
                return Err(Refusal::ModuleEffects);
            }
        }
        self.active.remove(path);
        self.checked.insert(path.to_path_buf());
        Ok(())
    }

    /// A re-export must actually link, even when the selected callable uses another export.
    fn export_exists(
        &mut self,
        path: &Path,
        parsed: &Parsed,
        name: &str,
        depth: usize,
    ) -> Result<(), Refusal> {
        if depth >= 8 {
            return Err(Refusal::Limit);
        }
        match exported(parsed, name)? {
            Export::Here(_) => Ok(()),
            Export::From(source, name) => {
                if self.setting.dialect.is_api_module(&source) {
                    return (self.setting.dialect.registration(&name).is_some()
                        || self
                            .setting
                            .dialect
                            .assertions
                            .iter()
                            .any(|entry| entry.name == name))
                    .then_some(())
                    .ok_or(Refusal::Export);
                }
                let target = self
                    .sources
                    .resolve(path, &source, self.setting.root)
                    .ok_or(Refusal::Source)?;
                if !self.checked.contains(&target) {
                    return Err(Refusal::ModuleEffects);
                }
                let (_, dependency) = self.read(&target)?;
                self.export_exists(&target, &dependency, &name, depth + 1)
            }
        }
    }
}

/// The export denotes a local function or a named re-export hop.
enum Export {
    Here(NodeId),
    From(String, String),
}

/// Read the export's binding, retaining initialization and mutability checks for aliases.
fn exported(parsed: &Parsed, name: &str) -> Result<Export, Refusal> {
    let tree = &parsed.tree;
    let mut answer = None;
    for statement in tree.list(tree.root(), "body").into_iter().flatten() {
        if tree.text(statement, "exportKind") == Some("type") {
            continue;
        }
        let found = match tree.kind(statement) {
            "ExportDefaultDeclaration" if name == "default" => {
                let declaration = tree
                    .child(statement, "declaration")
                    .ok_or(Refusal::Export)?;
                Some(Export::Here(callable(
                    parsed,
                    declaration,
                    Some(tree.span(statement).start),
                )?))
            }
            "ExportNamedDeclaration" => {
                let specifier = tree
                    .list(statement, "specifiers")
                    .into_iter()
                    .flatten()
                    .find(|&specifier| {
                        tree.text(specifier, "exportKind") != Some("type")
                            && tree
                                .child(specifier, "exported")
                                .is_some_and(|node| lang::export_name(tree, node) == name)
                    });
                if let Some(specifier) = specifier {
                    let local = tree.child(specifier, "local").ok_or(Refusal::Export)?;
                    if let Some(source) = tree
                        .child(statement, "source")
                        .and_then(|node| tree.text(node, "value"))
                    {
                        Some(Export::From(
                            source.to_owned(),
                            lang::export_name(tree, local),
                        ))
                    } else {
                        let symbol = parsed
                            .scopes
                            .top_level(&lang::export_name(tree, local))
                            .ok_or(Refusal::Export)?;
                        Some(Export::Here(symbol_callable(
                            parsed,
                            symbol,
                            None,
                            &mut HashSet::new(),
                        )?))
                    }
                } else if let Some(declaration) = tree.child(statement, "declaration") {
                    let symbol = parsed.scopes.top_level(name).filter(|&symbol| {
                        tree.span(declaration)
                            .contains(parsed.scopes.symbol(symbol).span)
                    });
                    symbol
                        .map(|symbol| {
                            symbol_callable(parsed, symbol, None, &mut HashSet::new())
                                .map(Export::Here)
                        })
                        .transpose()?
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(found) = found {
            if answer.is_some() {
                return Err(Refusal::Export);
            }
            answer = Some(found);
        }
    }
    answer.ok_or(Refusal::Export)
}

/// Resolve a callable expression, without executing calls or treating objects as functions.
pub(crate) fn callable(
    parsed: &Parsed,
    node: NodeId,
    before: Option<u32>,
) -> Result<NodeId, Refusal> {
    let tree = &parsed.tree;
    let node = tree.inner(node);
    match tree.kind(node) {
        "FunctionDeclaration" | "FunctionExpression" | "ArrowFunctionExpression" => {
            if let Some(symbol) = tree
                .child(node, "id")
                .and_then(|id| parsed.scopes.symbol_at(tree.span(id).start))
            {
                immutable(parsed, symbol)?;
            }
            Ok(node)
        }
        "Identifier" => {
            let symbol = parsed
                .scopes
                .symbol_at(tree.span(node).start)
                .ok_or(Refusal::Callable)?;
            symbol_callable(parsed, symbol, before, &mut HashSet::new())
        }
        _ => Err(Refusal::Callable),
    }
}

/// Follow immutable local aliases once each; an import alias is not a local export definition.
fn symbol_callable(
    parsed: &Parsed,
    symbol: SymbolId,
    before: Option<u32>,
    active: &mut HashSet<SymbolId>,
) -> Result<NodeId, Refusal> {
    if active.len() >= 64 {
        return Err(Refusal::Limit);
    }
    if !active.insert(symbol) {
        return Err(Refusal::Callable);
    }
    immutable(parsed, symbol)?;
    let tree = &parsed.tree;
    match parsed.scopes.symbol(symbol).declaration {
        // An overloaded function is declared first by a signature; the call runs the
        // implementation, the one declaration of the name with a body.
        Declaration::Function(node) if tree.kind(node) == "TSDeclareFunction" => tree
            .nodes()
            .find(|&candidate| {
                tree.kind(candidate) == "FunctionDeclaration"
                    && tree.child(candidate, "id").is_some_and(|id| {
                        parsed.scopes.name_at(tree.span(id).start)
                            == Some(Name::Binding(Some(symbol)))
                    })
            })
            .ok_or(Refusal::Callable),
        Declaration::Function(node) => Ok(node),
        Declaration::Variable(node) => {
            if tree
                .parent(node)
                .and_then(|parent| tree.text(parent, "kind"))
                != Some("const")
                || before.is_some_and(|before| tree.span(node).start >= before)
            {
                return Err(Refusal::Callable);
            }
            let init = tree.inner(tree.child(node, "init").ok_or(Refusal::Callable)?);
            if tree.kind(init) == "Identifier" {
                let next = parsed
                    .scopes
                    .symbol_at(tree.span(init).start)
                    .ok_or(Refusal::Callable)?;
                symbol_callable(parsed, next, Some(tree.span(node).start), active)
            } else {
                callable(parsed, init, before)
            }
        }
        _ => Err(Refusal::Callable),
    }
}

/// A binding has one runtime declaration and no recorded writes after it is initialized.
pub(crate) fn immutable(parsed: &Parsed, symbol: SymbolId) -> Result<(), Refusal> {
    let binding = parsed.scopes.symbol(symbol);
    if binding.references.iter().any(|reference| reference.write) {
        return Err(Refusal::Callable);
    }
    let declarations = runtime_bindings(parsed)
        .filter(|&binding| binding == symbol)
        .count();
    if declarations != 1 {
        return Err(Refusal::Callable);
    }
    Ok(())
}

/// Runtime declaration identities, excluding erased overload signatures.
fn runtime_bindings(parsed: &Parsed) -> impl Iterator<Item = SymbolId> + '_ {
    parsed.tree.nodes().filter_map(|node| {
        if parsed.tree.kind(node) != "Identifier"
            || parsed
                .tree
                .parent(node)
                .is_some_and(|parent| parsed.tree.kind(parent) == "TSDeclareFunction")
        {
            return None;
        }
        match parsed.scopes.name_at(parsed.tree.span(node).start) {
            Some(Name::Binding(Some(symbol))) => Some(symbol),
            _ => None,
        }
    })
}

/// A parameter has one runtime binding span and no later writes. Destructuring wrappers may expose
/// the same binding node more than once, while a hoisted redeclaration has a distinct span.
fn immutable_parameter(
    parsed: &Parsed,
    symbol: SymbolId,
    declarations: &HashMap<SymbolId, (u32, u32, bool)>,
) -> Result<(), Refusal> {
    let binding = parsed.scopes.symbol(symbol);
    if binding.references.iter().any(|reference| reference.write) {
        return Err(Refusal::Parameters);
    }
    declarations
        .get(&symbol)
        .is_some_and(|(_, _, multiple)| !multiple)
        .then_some(())
        .ok_or(Refusal::Parameters)
}

/// Unique runtime binding spans for every symbol, computed once for all flattened parameters.
fn binding_declarations(parsed: &Parsed) -> HashMap<SymbolId, (u32, u32, bool)> {
    let tree = &parsed.tree;
    let mut declarations = HashMap::new();
    for node in tree.nodes().filter(|&node| {
        tree.kind(node) == "Identifier"
            && tree
                .parent(node)
                .is_none_or(|parent| tree.kind(parent) != "TSDeclareFunction")
    }) {
        let Some(Name::Binding(Some(symbol))) = parsed.scopes.name_at(tree.span(node).start) else {
            continue;
        };
        let span = tree.span(node);
        declarations
            .entry(symbol)
            .and_modify(|(start, end, multiple)| {
                *multiple |= *start != span.start || *end != span.end;
            })
            .or_insert((span.start, span.end, false));
    }
    declarations
}

/// Only synchronous functions with plain, immutable positional parameters have binding templates.
fn parameters(
    parsed: &Parsed,
    function: NodeId,
    direct_eval: bool,
) -> Result<Vec<Parameter>, Refusal> {
    let parameters = returned_parameters(parsed, function, direct_eval)?;
    if parameters.iter().any(|parameter| {
        !matches!(
            parameter.bindings.as_slice(),
            [ParameterBinding { path, default: None, .. }] if path.is_empty()
        )
    }) {
        return Err(Refusal::Parameters);
    }
    Ok(parameters)
}

/// The shared per-function allowance for parameter proofs and owned path copies.
const MAX_PARAMETER_WORK: usize = 8 * 1024 * 1024;

/// Synchronous returned factories and leaves may destructure static object data.
pub(crate) fn returned_parameters(
    parsed: &Parsed,
    function: NodeId,
    direct_eval: bool,
) -> Result<Vec<Parameter>, Refusal> {
    if direct_eval {
        return Err(Refusal::Parameters);
    }
    let tree = &parsed.tree;
    let params = tree.list(function, "params");
    if tree.flag(function, "async")
        || tree.flag(function, "generator")
        || tree.child(function, "body").is_none()
    {
        return Err(Refusal::Callable);
    }
    if tree
        .child(function, "body")
        .is_some_and(|body| lang::uses_receiver(tree, &parsed.scopes, body))
    {
        return Err(Refusal::Callable);
    }
    if params.is_empty() {
        return Ok(Vec::new());
    }
    let nodes = tree.nodes().count();
    if params.len().saturating_mul(nodes) > MAX_PARAMETER_WORK {
        return Err(Refusal::Limit);
    }
    let declarations = binding_declarations(parsed);
    let mut left = MAX_PARAMETER_WORK;
    let parameters: Vec<_> = params
        .into_iter()
        .map(|parameter| {
            let parameter = parameter.ok_or(Refusal::Parameters)?;
            let mut bindings = Vec::new();
            parameter_bindings(
                parsed,
                parameter,
                &mut Vec::new(),
                &mut bindings,
                &declarations,
                nodes,
                &mut left,
            )?;
            if bindings.is_empty() {
                return Err(Refusal::Parameters);
            }
            Ok(Parameter { bindings })
        })
        .collect::<Result<_, _>>()?;
    let bindings = parameters
        .iter()
        .map(|parameter| parameter.bindings.len())
        .sum::<usize>();
    if bindings.saturating_mul(nodes) > MAX_PARAMETER_WORK {
        return Err(Refusal::Limit);
    }
    Ok(parameters)
}

/// Flatten static patterns, reserving shared node/copy work before owning each selection.
fn parameter_bindings(
    parsed: &Parsed,
    pattern: NodeId,
    path: &mut Vec<String>,
    bindings: &mut Vec<ParameterBinding>,
    declarations: &HashMap<SymbolId, (u32, u32, bool)>,
    nodes: usize,
    left_work: &mut usize,
) -> Result<(), Refusal> {
    let tree = &parsed.tree;
    match tree.kind(pattern) {
        "Identifier" => {
            let cost = path.iter().fold(
                nodes
                    .saturating_add(std::mem::size_of::<ParameterBinding>())
                    .saturating_add(path.len().saturating_mul(std::mem::size_of::<String>())),
                |cost, key| cost.saturating_add(key.len()),
            );
            *left_work = left_work.checked_sub(cost).ok_or(Refusal::Limit)?;
            let symbol = parsed
                .scopes
                .symbol_at(tree.span(pattern).start)
                .ok_or(Refusal::Parameters)?;
            immutable_parameter(parsed, symbol, declarations)?;
            bindings.push(ParameterBinding {
                symbol,
                path: path.clone(),
                default: None,
            });
            Ok(())
        }
        "AssignmentPattern" => {
            let left = tree.child(pattern, "left").ok_or(Refusal::Parameters)?;
            if tree.kind(left) != "Identifier" {
                return Err(Refusal::Parameters);
            }
            let before = bindings.len();
            parameter_bindings(parsed, left, path, bindings, declarations, nodes, left_work)?;
            if bindings.len() != before + 1 {
                return Err(Refusal::Parameters);
            }
            bindings[before].default = tree.child(pattern, "right");
            Ok(())
        }
        "ObjectPattern" => {
            let properties = tree.list(pattern, "properties");
            if properties.is_empty() {
                return Err(Refusal::Parameters);
            }
            let mut names = std::collections::HashSet::new();
            for property in properties.into_iter().flatten() {
                if tree.kind(property) != "Property"
                    || tree.flag(property, "computed")
                    || tree.text(property, "kind") != Some("init")
                {
                    return Err(Refusal::Parameters);
                }
                let name = lang::property_name(tree, property).ok_or(Refusal::Parameters)?;
                if name == "__proto__" || !names.insert(name) {
                    return Err(Refusal::Parameters);
                }
                path.push(name.to_owned());
                parameter_bindings(
                    parsed,
                    tree.child(property, "value").ok_or(Refusal::Parameters)?,
                    path,
                    bindings,
                    declarations,
                    nodes,
                    left_work,
                )?;
                path.pop();
            }
            Ok(())
        }
        _ => Err(Refusal::Parameters),
    }
}

/// Definition-time syntax whose evaluation runs no user code and reads no unknown value.
pub(crate) fn inert_declaration(parsed: &Parsed, node: NodeId) -> bool {
    let tree = &parsed.tree;
    if tree.erased_declaration(node) {
        return true;
    }
    match tree.kind(node) {
        "FunctionDeclaration"
        | "TSInterfaceDeclaration"
        | "TSTypeAliasDeclaration"
        | "TSDeclareFunction"
        | "EmptyStatement" => true,
        // A `using` declaration disposes its value when its scope ends, which runs code.
        "VariableDeclaration"
            if matches!(tree.text(node, "kind"), Some("const" | "let" | "var")) =>
        {
            tree.list(node, "declarations")
                .into_iter()
                .flatten()
                .all(|declaration| {
                    tree.child(declaration, "id")
                        .is_some_and(|id| tree.kind(id) == "Identifier")
                        && tree
                            .child(declaration, "init")
                            .is_none_or(|init| inert_value(parsed, init, &mut 4096))
                })
        }
        _ => inert_value(parsed, node, &mut 4096),
    }
}

/// Inert construction only: property reads, spreads, coercions and computed execution are unknown.
fn inert_value(parsed: &Parsed, node: NodeId, left: &mut usize) -> bool {
    InertProof::new(parsed, usize::MAX).value(node, left)
}

/// Reuse static binding facts within one source-local inert-expression proof.
struct InertProof<'a> {
    parsed: &'a Parsed,
    left: usize,
    limited: bool,
    symbols: HashMap<SymbolId, bool>,
    unwritten: HashMap<SymbolId, bool>,
    declarations: Option<HashMap<SymbolId, usize>>,
}

impl<'a> InertProof<'a> {
    /// Begin with an independent allowance for scans, reference checks and cached lookups.
    fn new(parsed: &'a Parsed, budget: usize) -> Self {
        Self {
            parsed,
            left: budget,
            limited: false,
            symbols: HashMap::new(),
            unwritten: HashMap::new(),
            declarations: None,
        }
    }

    /// Charge before doing proof work; exhausted work never establishes a fact.
    fn spend(&mut self, work: usize) -> bool {
        if self.limited {
            return false;
        }
        match self.left.checked_sub(work) {
            Some(left) => {
                self.left = left;
                true
            }
            None => {
                self.limited = true;
                false
            }
        }
    }

    /// Retain the existing write-first barrier and index runtime declarations once.
    fn immutable(&mut self, symbol: SymbolId) -> bool {
        if !self.spend(1) {
            return false;
        }
        if let Some(&stable) = self.symbols.get(&symbol) {
            return stable;
        }
        if !self.unwritten(symbol) {
            if !self.limited {
                self.symbols.insert(symbol, false);
            }
            return false;
        }
        if self.declarations.is_none() {
            if !self.spend(self.parsed.tree.nodes().count().saturating_mul(2)) {
                return false;
            }
            let mut declarations = HashMap::new();
            for binding in runtime_bindings(self.parsed) {
                *declarations.entry(binding).or_insert(0) += 1;
            }
            self.declarations = Some(declarations);
        }
        let stable = self
            .declarations
            .as_ref()
            .and_then(|counts| counts.get(&symbol))
            == Some(&1);
        self.symbols.insert(symbol, stable);
        stable
    }

    /// Recorded writes belong to the immutable source, independently of value evaluation.
    fn unwritten(&mut self, symbol: SymbolId) -> bool {
        if !self.spend(1) {
            return false;
        }
        if let Some(&stable) = self.unwritten.get(&symbol) {
            return stable;
        }
        let binding = self.parsed.scopes.symbol(symbol);
        if !self.spend(binding.references.len()) {
            return false;
        }
        let stable = !binding.references.iter().any(|reference| reference.write);
        self.unwritten.insert(symbol, stable);
        stable
    }

    /// Keep the existing value walk and logical expression allowance at every visit.
    fn value(&mut self, node: NodeId, left: &mut usize) -> bool {
        if !self.spend(1) {
            return false;
        }
        if *left == 0 {
            self.limited = true;
            return false;
        }
        *left -= 1;
        let parsed = self.parsed;
        let tree = &parsed.tree;
        let node = tree.inner(node);
        match tree.kind(node) {
            "Literal" | "FunctionExpression" | "ArrowFunctionExpression" => true,
            "Identifier" => {
                let Some(symbol) = parsed.scopes.symbol_at(tree.span(node).start) else {
                    return false;
                };
                if !self.immutable(symbol) {
                    return false;
                }
                match parsed.scopes.symbol(symbol).declaration {
                    Declaration::Function(_) => true,
                    Declaration::Import {
                        import, specifier, ..
                    } => {
                        tree.text(import, "importKind") != Some("type")
                            && tree.text(specifier, "importKind") != Some("type")
                    }
                    Declaration::Variable(declaration) => {
                        tree.parent(declaration)
                            .and_then(|parent| tree.text(parent, "kind"))
                            == Some("const")
                            && tree.span(declaration).end < tree.span(node).start
                            && tree
                                .child(declaration, "init")
                                .is_some_and(|init| self.value(init, left))
                    }
                    _ => false,
                }
            }
            "ArrayExpression" => tree
                .list(node, "elements")
                .into_iter()
                .all(|element| element.is_none_or(|element| self.value(element, left))),
            "ObjectExpression" => {
                tree.list(node, "properties")
                    .into_iter()
                    .flatten()
                    .all(|property| {
                        tree.kind(property) == "Property"
                            && !tree.flag(property, "computed")
                            && tree.text(property, "kind") == Some("init")
                            && lang::property_name(tree, property) != Some("__proto__")
                            && tree
                                .child(property, "value")
                                .is_some_and(|value| self.value(value, left))
                    })
            }
            _ => false,
        }
    }
}

/// Executable statements shared by factory and transform body readers, after TS erasure.
pub(crate) fn runtime_statements(tree: &lang::Tree, body: NodeId) -> Vec<NodeId> {
    tree.list(body, "body")
        .into_iter()
        .flatten()
        .filter(|&statement| {
            !tree.erased_declaration(statement) && tree.kind(statement) != "EmptyStatement"
        })
        .collect()
}

/// Read one returned-object leaf as an ordinary registration template.
pub(crate) fn returned_leaf(
    parsed: &Parsed,
    found: &Inventory,
    function: NodeId,
) -> Result<Plan, Refusal> {
    let parameters = returned_parameters(parsed, function, found.direct_eval)?;
    let mut steps = Vec::new();
    BodyReader { parsed, found }.body(function, &mut steps, &mut 2000)?;
    Ok(Plan {
        function,
        parameters,
        steps,
    })
}

/// One module's template reader over what its inventory recorded.
struct BodyReader<'a> {
    parsed: &'a Parsed,
    found: &'a Inventory,
}

impl BodyReader<'_> {
    /// Record only reachable operations; returning a registration ends this invocation's sequence.
    fn body(
        &self,
        function: NodeId,
        steps: &mut Vec<Step>,
        left: &mut usize,
    ) -> Result<(), Refusal> {
        let tree = &self.parsed.tree;
        let root = tree.child(function, "body").ok_or(Refusal::Body)?;
        let statements: Vec<NodeId> = if tree.kind(root) == "BlockStatement" {
            tree.list(root, "body").into_iter().flatten().collect()
        } else {
            vec![root]
        };
        self.sequence(&statements, steps, left).map(|_| ())
    }

    /// Read `statements` in order; whether the sequence returned, which ends the invocation.
    fn sequence(
        &self,
        statements: &[NodeId],
        steps: &mut Vec<Step>,
        left: &mut usize,
    ) -> Result<bool, Refusal> {
        let tree = &self.parsed.tree;
        for &statement in statements {
            if tree.erased_declaration(statement) {
                continue;
            }
            if *left == 0 {
                return Err(Refusal::Limit);
            }
            *left -= 1;
            let returns = tree.kind(statement) == "ReturnStatement";
            let expression = match tree.kind(statement) {
                "ExpressionStatement" => {
                    tree.child(statement, "expression").ok_or(Refusal::Body)?
                }
                "ReturnStatement" => {
                    let Some(argument) = tree.child(statement, "argument") else {
                        return Ok(true);
                    };
                    argument
                }
                "EmptyStatement"
                | "TSInterfaceDeclaration"
                | "TSTypeAliasDeclaration"
                | "FunctionDeclaration" => continue,
                // A `finally` that runs nothing and ends normally leaves the block's outcome as
                // it was; a `catch` would run only on a throw the reader does not model.
                "TryStatement" => {
                    let finalizer = tree.child(statement, "finalizer").ok_or(Refusal::Body)?;
                    if tree.child(statement, "handler").is_some()
                        || !lang::completes_inertly(tree, &self.parsed.scopes, finalizer)
                    {
                        return Err(Refusal::Body);
                    }
                    let block: Vec<NodeId> = tree
                        .child(statement, "block")
                        .map(|block| tree.list(block, "body").into_iter().flatten().collect())
                        .ok_or(Refusal::Body)?;
                    if self.sequence(&block, steps, left)? {
                        return Ok(true);
                    }
                    continue;
                }
                "VariableDeclaration" => {
                    self.declaration(statement, steps, left)?;
                    continue;
                }
                _ => statement,
            };
            let expression = tree.inner(expression);
            let site = self
                .found
                .sites
                .iter()
                .find(|site| site.node == expression)
                .ok_or(Refusal::Body)?;
            match site.kind {
                SiteKind::Registers(kind) => self.registration(site, kind, steps, left)?,
                SiteKind::Wrapper(_) => steps.push(Step::Call(expression)),
            }
            if returns {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// A registration call, and a suite's children after it.
    fn registration(
        &self,
        site: &super::Site,
        kind: Registers,
        steps: &mut Vec<Step>,
        left: &mut usize,
    ) -> Result<(), Refusal> {
        let tree = &self.parsed.tree;
        if !site.chain.is_empty() {
            return Err(Refusal::Body);
        }
        steps.push(Step::Registration(site.node, kind));
        if kind == Registers::Suite {
            let arguments = tree.list(site.node, "arguments");
            if arguments.len() != 2 {
                return Err(Refusal::Body);
            }
            let callback = tree.inner(arguments[1].ok_or(Refusal::Body)?);
            if !tree.is_function(callback) {
                return Err(Refusal::Body);
            }
            if !parameters(self.parsed, callback, self.found.direct_eval)?.is_empty() {
                return Err(Refusal::Body);
            }
            self.body(callback, steps, left)?;
        }
        Ok(())
    }

    /// A `const` whose initializer is a registration, read as that registration. A registration
    /// function built and left unused is not admitted: building one sets state on the shared chain
    /// and coerces its arguments. Nor is the in-source binding: an imported module may not receive
    /// it, and a hoisted function can read its names before it is initialized.
    fn declaration(
        &self,
        statement: NodeId,
        steps: &mut Vec<Step>,
        left: &mut usize,
    ) -> Result<(), Refusal> {
        let tree = &self.parsed.tree;
        let declarations = tree.list(statement, "declarations");
        let [Some(declarator)] = declarations.as_slice() else {
            return Err(Refusal::Body);
        };
        if tree.text(statement, "kind") != Some("const") {
            return Err(Refusal::Body);
        }
        let id = tree.child(*declarator, "id").ok_or(Refusal::Body)?;
        let init = tree.inner(tree.child(*declarator, "init").ok_or(Refusal::Body)?);
        match tree.kind(id) {
            "Identifier" => {
                if let Some(site) = self.found.sites.iter().find(|site| site.node == init) {
                    return match site.kind {
                        SiteKind::Registers(kind) => self.registration(site, kind, steps, left),
                        SiteKind::Wrapper(_) => Err(Refusal::Body),
                    };
                }
                Err(Refusal::Body)
            }
            _ => Err(Refusal::Body),
        }
    }
}

#[cfg(test)]
mod tests;
