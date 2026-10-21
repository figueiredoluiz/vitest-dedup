//! A parsed helper frame borrows only its own AST. Caller state and returned declarations are
//! owned; no symbol ID or parameter binding crosses the boundary.
use super::*;
use crate::frontend::fingerprint::ImportOrigin;
use crate::frontend::helpers::export_contract::resolve_export_chain;
use crate::frontend::helpers::{self, Callable, Export};
use crate::frontend::resolver::{self, Resolved};
use oxc_ast::ast::{Declaration, ExportDefaultDeclarationKind, ImportOrExportKind};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(super) struct RunnerImport {
    origin: ImportOrigin,
    erased: bool,
    local: String,
}

pub(super) struct ExpansionState {
    pub(super) active: Vec<(PathBuf, u32)>,
    left: usize,
    /// Declarations this file may still emit (see [`MAX_DECLARATIONS_PER_FILE`]).
    declarations_left: usize,
    /// Literal loop iterations this file may still unroll (see [`MAX_UNROLLED_ITERATIONS`]).
    iterations_left: usize,
    /// Whether a declaration or an iteration was refused, for [`Self::budget_diagnostics`].
    declarations_refused: bool,
    iterations_refused: bool,
    /// Whether importing a module (and everything it imports) runs no unmodeled setup, by path.
    closure: HashMap<PathBuf, bool>,
}

/// Takes `units` from `left`, or marks `refused` when fewer are left. A refusal is final: a later,
/// smaller request is refused too, so a truncated file stays a prefix of its full extraction.
fn admit(left: &mut usize, refused: &mut bool, units: usize) -> bool {
    if *refused || *left < units {
        *refused = true;
        return false;
    }
    *left -= units;
    true
}

/// Most modules one import-closure check may visit before it refuses.
const MAX_IMPORT_CLOSURE: usize = 32;

impl Default for ExpansionState {
    /// Starts an empty active-call stack with the shared per-file expansion budget.
    fn default() -> Self {
        Self {
            active: Vec::new(),
            left: MAX_HELPER_EXPANSIONS,
            declarations_left: emission_limit(MAX_DECLARATIONS_PER_FILE),
            iterations_left: emission_limit(MAX_UNROLLED_ITERATIONS),
            declarations_refused: false,
            iterations_refused: false,
            closure: HashMap::new(),
        }
    }
}

/// The per-file emission limit, or a smaller test-only override set on the extracting thread.
fn emission_limit(limit: usize) -> usize {
    #[cfg(test)]
    if let Some(limit) = EMISSION_LIMIT_OVERRIDE.with(std::cell::Cell::get) {
        return limit;
    }
    limit
}

#[cfg(test)]
thread_local! {
    /// Test-only replacement for both emission limits, so a truncation can be compared against
    /// a full run without generating ten thousand declarations.
    pub(super) static EMISSION_LIMIT_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

impl ExpansionState {
    /// Spends one declaration, or records the refusal once the file's budget is spent. Nothing is
    /// admitted after a refused iteration either, so a truncated file is always a prefix of its
    /// full extraction.
    pub(super) fn admit_declaration(&mut self) -> bool {
        !self.iterations_refused
            && admit(
                &mut self.declarations_left,
                &mut self.declarations_refused,
                1,
            )
    }

    /// Spends `elements` loop iterations at once, or records the refusal when fewer remain.
    pub(super) fn admit_iterations(&mut self, elements: usize) -> bool {
        admit(
            &mut self.iterations_left,
            &mut self.iterations_refused,
            elements,
        )
    }

    /// How many declarations may still be emitted; none once either budget refused.
    pub(super) fn declarations_remaining(&self) -> usize {
        if self.iterations_refused || self.declarations_refused {
            0
        } else {
            self.declarations_left
        }
    }

    /// Records a declaration refusal for a table too long for the remaining budget.
    pub(super) fn refuse_declarations(&mut self) {
        self.declarations_refused = true;
    }

    /// One diagnostic per emission budget this file ran out of.
    pub(super) fn budget_diagnostics(&self) -> Vec<(ExclusionReason, String)> {
        let mut diagnostics = Vec::new();
        if self.declarations_refused {
            diagnostics.push((
                ExclusionReason::WalkLimit,
                format!(
                    "the file declares more than {MAX_DECLARATIONS_PER_FILE} tests and suites; \
                 declarations past the limit were not counted"
                ),
            ));
        }
        if self.iterations_refused {
            diagnostics.push((
                ExclusionReason::WalkLimit,
                format!(
                    "the file unrolls more than {MAX_UNROLLED_ITERATIONS} loop iterations; \
                 declarations past the limit were not counted"
                ),
            ));
        }
        diagnostics
    }

    /// Spends an admission and guards a local helper against recursion and excessive depth.
    pub(super) fn begin(&mut self, identity: (PathBuf, u32)) -> Result<(), Refusal> {
        self.spend()?;
        self.enter(identity)
    }

    /// Consumes an admission across both registration passes, refusing an exhausted budget.
    fn spend(&mut self) -> Result<(), Refusal> {
        if self.left == 0 {
            return Err(refusal(
                ExclusionReason::HelperLimit,
                "this file's helper-expansion budget is spent",
            ));
        }
        self.left -= 1;
        Ok(())
    }

    /// Pushes a unique active declaration identity within the shared nesting limit.
    fn enter(&mut self, identity: (PathBuf, u32)) -> Result<(), Refusal> {
        if self.active.contains(&identity) {
            return Err(refusal(
                ExclusionReason::HelperLimit,
                "it calls itself, directly or through another helper",
            ));
        }
        if self.active.len() >= MAX_HELPER_EXPANSION_DEPTH {
            return Err(refusal(
                ExclusionReason::HelperLimit,
                "helper calls are nested deeper than this extractor expands",
            ));
        }
        self.active.push(identity);
        Ok(())
    }
}

/// Maps caller symbols to imported export provenance.
/// Retains erased bindings solely so attempted runtime calls are explicitly refused.
pub(super) fn collect_imports(program: &Program, file: &Path) -> HashMap<SymbolId, RunnerImport> {
    let mut imports = HashMap::new();
    for statement in &program.body {
        let Statement::ImportDeclaration(import) = statement else {
            continue;
        };
        let specifier = import.source.value.as_str();
        for binding in import.specifiers.iter().flatten() {
            let (local, exported) = match binding {
                ImportDeclarationSpecifier::ImportSpecifier(binding) => {
                    (&binding.local, binding.imported.name().to_string())
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(binding) => {
                    (&binding.local, "default".to_owned())
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(binding) => {
                    (&binding.local, "*".to_owned())
                }
            };
            let Some(symbol) = local.symbol_id.get() else {
                continue;
            };
            let erased = matches!(import.import_kind, ImportOrExportKind::Type)
                || matches!(binding, ImportDeclarationSpecifier::ImportSpecifier(binding) if matches!(binding.import_kind, ImportOrExportKind::Type));
            if specifier == "vitest" && !erased {
                continue;
            }
            let resolved = if erased {
                None
            } else {
                match resolver::resolve(file, specifier) {
                    Resolved::Module(path) => Some(path),
                    Resolved::Unresolved => None,
                }
            };
            imports.insert(
                symbol,
                RunnerImport {
                    erased,
                    local: local.name.as_str().to_owned(),
                    origin: ImportOrigin {
                        component: if erased {
                            String::new()
                        } else {
                            super::super::fingerprint::specifier_identity(file, specifier)
                        },
                        exported,
                        specifier: specifier.to_owned(),
                        resolved,
                    },
                },
            );
        }
    }
    imports
}

impl<'a> Walker<'a> {
    /// Resolves direct imported calls and namespace/function invocation candidates.
    /// Ordinary named/default object methods do not invoke the imported runner.
    fn runner_origin(&self, callee: &Expression) -> Option<RunnerImport> {
        let (identifier, member) = match callee {
            Expression::Identifier(identifier) => (&**identifier, None),
            Expression::StaticMemberExpression(member) => {
                let Expression::Identifier(identifier) = &member.object else {
                    return None;
                };
                (&**identifier, Some(member.property.name.as_str()))
            }
            _ => return None,
        };
        let symbol = identifier
            .reference_id
            .get()
            .and_then(|reference| self.semantic.scoping().get_reference(reference).symbol_id());
        let mut binding = match symbol {
            Some(symbol) => self.runner_imports.get(&symbol)?.clone(),
            None => self
                .runner_imports
                .values()
                .find(|binding| binding.erased && binding.local == identifier.name.as_str())?
                .clone(),
        };
        if binding.origin.exported == "*" {
            binding.origin.exported = member.unwrap_or("*").to_owned();
        } else if member.is_some_and(|member| !matches!(member, "call" | "apply")) {
            // A property method on an imported value does not invoke the exported runner.
            // Function.call/apply are invocation forms, but remain explicitly unsupported.
            return None;
        }
        Some(binding)
    }

    /// Expands one registration-time import call or records a bounded scope refusal.
    pub(super) fn imported_helper_call(&mut self, call: &CallExpression<'a>, prepare: bool) {
        let Some(origin) = self.runner_origin(&call.callee) else {
            return;
        };
        if origin.erased {
            self.refuse_helper_expansion(refusal(
                ExclusionReason::HelperExportUnreadable,
                "a type-only import has no runtime runner binding",
            ));
            return;
        }
        // A declaration imported from a competing runner is not an opaque Vitest helper. Its
        // binding already proves which runtime owns the call, so trying to resolve its installed
        // package body can only manufacture an incomplete diagnostic on a non-Vitest file. Keep
        // unknown packages refused: custom installed helpers can register Vitest tests, which is
        // the C11 boundary this distinction must not weaken.
        if matches!(
            origin.origin.specifier.as_str(),
            "node:test" | "@jest/globals" | "@playwright/test"
        ) {
            return;
        }
        if !self.allow_imported {
            self.refuse_helper_expansion(refusal(
                ExclusionReason::HelperSourcesUnavailable,
                "historical helper sources are unavailable; present-day helper bodies cannot populate a revision baseline",
            ));
            return;
        }
        let result = self.read_imported_runner(&origin.origin, call, prepare);
        match result {
            Ok(tests) if !prepare => self.tests.extend(tests),
            Ok(_) => {}
            Err(reason) => self.refuse_helper_expansion(reason),
        }
    }

    /// Reads an admitted first-party export in its own parser and lexical frame.
    /// Checks module effects, callable shape, mocks and resource bounds before returning
    /// owned records with caller context and helper source identity.
    fn read_imported_runner(
        &mut self,
        origin: &ImportOrigin,
        call: &CallExpression<'a>,
        prepare: bool,
    ) -> Result<Vec<TestCase>, Refusal> {
        let admission = self.expansion.borrow_mut().spend();
        admission?;
        let start = origin.resolved.as_ref().ok_or(refusal(
            ExclusionReason::HelperExportUnreadable,
            "a called named import could not be resolved",
        ))?;
        let chain = resolve_export_chain(start, &origin.exported)
            .map_err(|_| refusal(ExclusionReason::HelperExportUnreadable, "a called named import's first-party export could not be read within the resolution/parser bounds"))?;
        let terminal = &chain.terminal;
        if self.mocks_unresolved
            || self.mocked_modules.iter().any(|mock| {
                mock.specifier.as_ref() == origin.component
                    || chain.modules.iter().any(|path| {
                        mock.specifier.as_ref()
                            == super::super::fingerprint::specifier_identity(
                                path,
                                &path.to_string_lossy(),
                            )
                    })
            })
        {
            return Err(refusal(
                ExclusionReason::RunnerMocked,
                "the runner or its re-export chain is mocked or mocking is unresolved",
            ));
        }

        for path in &chain.modules {
            if helpers::with_parsed_file(path, |program, _| {
                Some(module_is_inert(
                    program,
                    path,
                    &chain.modules,
                    &mut |file, specifier| import_is_inert(&self.expansion, file, specifier),
                ))
            }) != Some(true)
            {
                self.hook_scopes[0].unresolved = true;
                for case in &mut self.tests {
                    case.context.unresolved = true;
                }
                return Err(refusal(
                    ExclusionReason::HelperSetupEffects,
                    "a module in the runner's re-export chain has unsupported setup effects",
                ));
            }
        }
        helpers::with_parsed_file(&terminal.path, |program, source| {
            let semantic_return = SemanticBuilder::new().with_build_nodes(true).build(program);
            if !semantic_return.diagnostics.is_empty() {
                return Some(Err(refusal(
                    ExclusionReason::HelperExportUnreadable,
                    "the helper's lexical bindings could not be resolved",
                )));
            }
            let Some(Export::Local(callable)) = helpers::find_export(program, &terminal.export)
            else {
                return Some(Err(refusal(
                    ExclusionReason::HelperExportUnreadable,
                    "the helper's export changed during extraction",
                )));
            };
            let (language, module_flavor) = crate::discovery::classify_source_file(&terminal.path)?;
            let file =
                SourceFile::without_project_context(terminal.path.clone(), language, module_flavor);
            let semantic = &semantic_return.semantic;
            let mut child = Walker::new(source, &file, program, semantic, self.globals);
            if child.mocks_unresolved {
                self.hook_scopes[0].unresolved = true;
                for case in &mut self.tests {
                    case.context.unresolved = true;
                }
                return Some(Err(refusal(
                    ExclusionReason::RunnerMocked,
                    "the helper has unresolved mocking effects",
                )));
            }
            let mut registry = Registry::new(program, self.globals);
            // Defaults, rest and destructuring evaluate or bind more than a positional value, and
            // evaluated call arguments can themselves register tests even when the body is inert.
            // Only plain parameters and arguments that fold (or are inert) are admitted.
            let parameters = match &callable {
                Callable::Function(function) => &function.params,
                Callable::Arrow(arrow) => &arrow.params,
            };
            if parameters.rest.is_some()
                || parameters.items.iter().any(|parameter| {
                    parameter.initializer.is_some()
                        || !matches!(parameter.pattern, BindingPattern::BindingIdentifier(_))
                })
            {
                return Some(Err(refusal(
                    ExclusionReason::UnsupportedExpression,
                    "the imported runner has defaulted, rest or destructured parameters",
                )));
            }
            // The pre-scan does not unroll loops, so a loop variable cannot fold there. It still
            // scans the runner for hooks, which apply to the caller's scope whatever the titles
            // are; the declaration walk holds the bindings and decides whether the call is admitted.
            let unproved_arguments = call.arguments.iter().any(|argument| {
                argument.as_expression().is_none_or(|expression| {
                    !inert_expression(expression) && self.fold_expr(expression).is_none()
                })
            });
            if unproved_arguments && !prepare {
                return Some(Err(refusal(
                    ExclusionReason::HelperEffectsUnproved,
                    "the imported call has spread or unproved argument evaluation effects",
                )));
            }
            if matches!(&callable, Callable::Function(function) if function.body.is_none()) {
                return Some(Err(refusal(
                    ExclusionReason::HelperExportUnreadable,
                    "the exported helper is a bodyless or overload signature",
                )));
            }
            // A written parameter no longer holds the call-site value, so it is refused.
            // The caller folds the arguments; the helper's own symbols receive the text.
            if parameters.items.iter().any(|parameter| {
                matches!(&parameter.pattern, BindingPattern::BindingIdentifier(binding)
                if binding.symbol_id.get().is_none_or(|symbol| {
                    semantic.scoping().symbol_is_mutated(symbol)
                }))
            }) {
                return Some(Err(refusal(
                    ExclusionReason::HelperEffectsUnproved,
                    "the imported runner writes to a parameter",
                )));
            }
            child
                .param_bindings
                .extend(self.fold_parameter_arguments(parameters, call));
            let (statements, concise) = match &callable {
                Callable::Function(function) => (
                    function
                        .body
                        .as_ref()
                        .map(|body| body.statements.as_slice())
                        .unwrap_or(&[]),
                    None,
                ),
                Callable::Arrow(arrow) => match &arrow.body {
                    ArrowFunctionBody::FunctionBody(body) => (body.statements.as_slice(), None),
                    body => (&[][..], body.as_expression()),
                },
            };
            // `test.extend` bindings must exist before the shape check resolves `t(...)` calls.
            for statement in statements {
                if let Statement::VariableDeclaration(declaration) = statement {
                    for declarator in &declaration.declarations {
                        child.register_variable_declarator(declarator, &mut registry);
                    }
                }
            }
            let shape = if unproved_arguments {
                Ok(true)
            } else if let Some(expression) = concise {
                if inert_expression(expression) {
                    Ok(false)
                } else {
                    child
                        .imported_expression_shape(expression, &registry)
                        .map(|shape| shape != Shape::Inert)
                }
            } else {
                child.imported_statements_shape(statements, &registry)
            };
            if !matches!(&call.callee, Expression::Identifier(_)) {
                return Some(Err(refusal(
                    ExclusionReason::HelperExportUnreadable,
                    "imported runners require a direct named call",
                )));
            }
            match shape {
                Ok(false) => return Some(Ok(Vec::new())),
                Err(reason) => return Some(Err(reason)),
                Ok(true) => {}
            }
            let (start, asynchronous, declaration) = match &callable {
                Callable::Function(function) => (
                    function.span.start,
                    function.r#async || function.generator,
                    (function.r#type == FunctionType::FunctionDeclaration).then_some(&function.id),
                ),
                Callable::Arrow(arrow) => (arrow.span.start, arrow.r#async, None),
            };
            if asynchronous {
                return Some(Err(refusal(
                    ExclusionReason::UnsupportedExpression,
                    "this imported runner is async or a generator",
                )));
            }
            // A `const` initializer cannot be reassigned. A function declaration's binding can,
            // unless it is an anonymous default export that no local name reaches.
            if let Some(id) = declaration {
                match id.as_ref().and_then(|id| id.symbol_id.get()) {
                    Some(symbol) if semantic.scoping().symbol_is_mutated(symbol) => {
                        return Some(Err(refusal(
                            ExclusionReason::HelperExportUnreadable,
                            "the exported runner can be reassigned",
                        )));
                    }
                    None if id.is_some() || terminal.export != "default" => {
                        return Some(Err(refusal(
                            ExclusionReason::HelperExportUnreadable,
                            "the exported runner can be reassigned",
                        )));
                    }
                    _ => {}
                }
            }

            let identity = (
                terminal
                    .path
                    .canonicalize()
                    .unwrap_or_else(|_| terminal.path.clone()),
                start,
            );
            if let Err(reason) = self.expansion.borrow_mut().enter(identity) {
                return Some(Err(reason));
            }
            child.expansion = Rc::clone(&self.expansion);
            child.suite_path = self.suite_path.clone();
            child.suite_modifiers = self.suite_modifiers.clone();
            child.hook_scopes = self.hook_scopes.clone();
            child.unresolved_suite_depth = self.unresolved_suite_depth;
            child.mocked_modules = self.mocked_modules.clone();
            child.mocks_unresolved = self.mocks_unresolved;
            child.module_setup = self.module_setup.clone();
            child.registration_file = Some(
                self.registration_file
                    .as_ref()
                    .unwrap_or(&self.file.path)
                    .clone(),
            );
            // Configuration is owned by the caller, while source and lexical symbols stay local.
            let mut child_file = file.clone();
            child_file.config_scope = self.file.config_scope.clone();
            child.file = &child_file;
            let hooks_before = hook_count(child.hook_scopes.last());
            if prepare {
                // Hooks join the calling scope, which the declaration walk then already holds.
                child.prepare_body(statements, concise, &mut registry);
            } else {
                // The caller's pre-scan registered this call's hooks; the helper's own pass only
                // needs its fixture bindings, so its hooks land in a scratch scope.
                let held = std::mem::take(
                    child
                        .hook_scopes
                        .last_mut()
                        .expect("root hook scope always present"),
                );
                child.prepare_body(statements, concise, &mut registry);
                *child
                    .hook_scopes
                    .last_mut()
                    .expect("root hook scope always present") = held;
                match concise {
                    Some(Expression::CallExpression(call)) => {
                        child.classify_call(call, &mut registry);
                    }
                    _ => child.walk_statement_list(statements, &mut registry),
                }
            }
            self.expansion.borrow_mut().active.pop();
            if !child.diagnostics.is_empty() {
                // The nested refusal's own code, not a stand-in: the export resolved fine.
                let nested = child
                    .helper_refusal_reasons
                    .first()
                    .copied()
                    .unwrap_or(ExclusionReason::HelperExportUnreadable);
                // The nested cause travels with the code: the child's own first refusal names what
                // could not be read, so the outer sentence says that instead of only that
                // something failed.
                let text = child.first_helper_refusal_text.as_deref().map_or(
                    std::borrow::Cow::Borrowed("an imported runner could not be fully extracted"),
                    |inner| {
                        std::borrow::Cow::Owned(format!(
                            "an imported runner could not be fully extracted: {inner}"
                        ))
                    },
                );
                // The child may have refused for more than one reason; the caller records the first
                // as its own refusal, so the rest are carried along for the file's other reasons.
                for (reason, sentence) in std::mem::take(&mut child.further_refusals) {
                    if self
                        .further_refusals
                        .iter()
                        .all(|(held, _)| *held != reason)
                    {
                        self.further_refusals.push((reason, sentence));
                    }
                }
                return Some(Err(Refusal {
                    reason: nested,
                    text,
                }));
            }
            let registers_hooks = prepare && hook_count(child.hook_scopes.last()) > hooks_before;
            if registers_hooks {
                *self
                    .hook_scopes
                    .last_mut()
                    .expect("root hook scope always present") = child
                    .hook_scopes
                    .last()
                    .expect("root hook scope always present")
                    .clone();
            }
            if !child.tests.is_empty() || registers_hooks {
                if !self.helper_sources.contains_key(&terminal.path)
                    && self.helper_sources.values().map(String::len).sum::<usize>() as u64
                        + source.len() as u64
                        > crate::discovery::MAX_SOURCE_FILE_BYTES
                {
                    return Some(Err(refusal(
                        ExclusionReason::HelperLimit,
                        "this file's retained helper sources exceed the source byte budget",
                    )));
                }
                self.helper_sources
                    .entry(terminal.path.clone())
                    .or_insert_with(|| source.to_owned());
            }
            // Sources a nested runner retained in its own frame belong to this caller's budget.
            for (path, text) in std::mem::take(&mut child.helper_sources) {
                if !self.helper_sources.contains_key(&path)
                    && self.helper_sources.values().map(String::len).sum::<usize>() as u64
                        + text.len() as u64
                        > crate::discovery::MAX_SOURCE_FILE_BYTES
                {
                    return Some(Err(refusal(
                        ExclusionReason::HelperLimit,
                        "this file's retained helper sources exceed the source byte budget",
                    )));
                }
                self.helper_sources.entry(path).or_insert(text);
            }
            Some(Ok(child.tests))
        })
        .ok_or(refusal(
            ExclusionReason::HelperExportUnreadable,
            "the helper could not be parsed within the source size/nesting bounds",
        ))?
    }

    /// Pre-scans a runner body for hooks and fixture bindings: a statement list or the single
    /// expression of a concise arrow.
    fn prepare_body(
        &mut self,
        statements: &[Statement<'a>],
        concise: Option<&Expression<'a>>,
        registry: &mut Registry,
    ) {
        match concise {
            Some(Expression::CallExpression(call)) => self.prepare_call(call, registry),
            _ => self.prepare_statements(statements, registry),
        }
    }

    /// Distinguishes static registrations from inert bodies and unproved effects.
    /// Refuses control flow and setup/fixture effects before declaration walking.
    fn imported_statements_shape(
        &self,
        statements: &[Statement<'a>],
        registry: &Registry,
    ) -> Result<bool, Refusal> {
        let mut registers = false;
        let mut opaque = false;
        for (index, statement) in statements.iter().enumerate() {
            match statement {
                Statement::ExpressionStatement(statement) => {
                    match self.imported_expression_shape(&statement.expression, registry)? {
                        Shape::Inert => opaque = true,
                        Shape::Hook | Shape::Declares => registers = true,
                    }
                }
                Statement::VariableDeclaration(declaration) => {
                    if declaration.declarations.iter().any(|declaration| {
                        declaration.init.as_ref().is_some_and(|expression| {
                            !inert_expression(expression)
                                && !self.is_fixture_binding(declaration, registry)
                        })
                    }) {
                        return Err(refusal(
                            ExclusionReason::HelperSetupEffects,
                            "the imported runner has unproved setup or fixture effects",
                        ));
                    }
                }
                Statement::FunctionDeclaration(_)
                | Statement::EmptyStatement(_)
                | Statement::TSTypeAliasDeclaration(_)
                | Statement::TSInterfaceDeclaration(_) => {}
                Statement::ReturnStatement(statement)
                    if !registers
                        && index + 1 == statements.len()
                        && statement.argument.as_ref().is_none_or(inert_expression) => {}
                _ => {
                    return Err(refusal(
                        ExclusionReason::HelperEffectsUnproved,
                        "the imported runner has unsupported control flow or registration effects",
                    ))
                }
            }
        }
        if registers && opaque {
            return Err(refusal(
                ExclusionReason::HelperSetupEffects,
                "the imported runner has unproved setup or fixture effects",
            ));
        }
        Ok(registers)
    }

    /// Recognizes `const t = test.extend({...})` whose fixtures all read as static values.
    /// A readable fixture table has no evaluation effects; anything else resolves as unresolved.
    fn is_fixture_binding(&self, declarator: &VariableDeclarator<'a>, registry: &Registry) -> bool {
        let (BindingPattern::BindingIdentifier(_), Some(init @ Expression::CallExpression(_))) =
            (&declarator.id, &declarator.init)
        else {
            return false;
        };
        self.resolve_call_callee(init, registry)
            .is_some_and(|resolved| {
                resolved.root == "test"
                    && resolved.steps.is_empty()
                    && !resolved.fixtures_unresolved
            })
    }

    /// Proves a local shadowing callable cannot register tests or add setup effects.
    fn proved_inert_call(&self, callee: &Expression) -> bool {
        let Expression::Identifier(identifier) = callee else {
            return false;
        };
        let Some(symbol) = identifier
            .reference_id
            .get()
            .and_then(|reference| self.semantic.scoping().get_reference(reference).symbol_id())
        else {
            return false;
        };
        let AstKind::VariableDeclarator(declaration) =
            self.semantic.symbol_declaration(symbol).kind()
        else {
            return false;
        };
        if self.semantic.scoping().symbol_is_mutated(symbol) {
            return false;
        }
        let Some(Expression::ArrowFunctionExpression(arrow)) = &declaration.init else {
            return false;
        };
        if arrow.r#async
            || arrow.params.rest.is_some()
            || arrow.params.items.iter().any(|parameter| {
                parameter.initializer.is_some()
                    || !matches!(parameter.pattern, BindingPattern::BindingIdentifier(_))
            })
        {
            return false;
        }
        match &arrow.body {
            ArrowFunctionBody::FunctionBody(body) => {
                body.statements.iter().all(|statement| match statement {
                    Statement::EmptyStatement(_) => true,
                    Statement::ReturnStatement(statement) => {
                        statement.argument.as_ref().is_none_or(inert_expression)
                    }
                    _ => false,
                })
            }
            body => body.as_expression().is_some_and(inert_expression),
        }
    }

    /// Recognizes static Vitest calls using the helper's own lexical registry.
    /// Unsupported titles, callbacks, modifiers and nested runner calls are refused.
    fn imported_expression_shape(
        &self,
        expression: &Expression<'a>,
        registry: &Registry,
    ) -> Result<Shape, Refusal> {
        let Expression::CallExpression(call) = expression else {
            return Err(refusal(
                ExclusionReason::UnsupportedExpression,
                "the imported runner has an unsupported registration expression",
            ));
        };
        let Some(resolved) = self.resolve_call_callee(&call.callee, registry) else {
            // A nested call is expanded by the same walk under the shared budget and active-call
            // stack, which applies this shape check to its body; a refusal there surfaces as a
            // diagnostic on this frame, so the whole runner is refused with its scope tainted.
            if self.resolve_local_helper(&call.callee).is_some()
                || self.runner_origin(&call.callee).is_some()
            {
                return Ok(Shape::Declares);
            }
            if let Expression::Identifier(identifier) = &call.callee {
                if self.is_free_reference(identifier)
                    && (canonicalize_export_name(identifier.name.as_str()).is_some()
                        || hook_kind(identifier.name.as_str()).is_some())
                {
                    return Err(refusal(
                        ExclusionReason::UnsupportedExpression,
                        "an ambient framework call is unresolved in the caller's globals mode",
                    ));
                }
            }
            if self.proved_inert_call(&call.callee) {
                return Ok(Shape::Inert);
            }
            return Err(refusal(
                ExclusionReason::HelperEffectsUnproved,
                "a call inside the imported runner has unproved registration effects",
            ));
        };
        let hook = match resolved.steps.as_slice() {
            [] => hook_kind(resolved.root).is_some(),
            [ChainStep::Member(name)] => hook_kind(name).is_some(),
            _ => false,
        };
        if hook {
            let inline = call.arguments.first().and_then(as_function_like).is_some();
            let options_inert = call
                .arguments
                .get(1)
                .is_none_or(|argument| argument.as_expression().is_some_and(inert_expression));
            if !inline || !options_inert || call.arguments.len() > 2 {
                return Err(refusal(
                    ExclusionReason::UnsupportedExpression,
                    "an imported hook needs an inline callback and an inert timeout",
                ));
            }
            return Ok(Shape::Hook);
        }
        if !matches!(resolved.root, "test" | "describe")
            || !resolved.steps.is_empty()
            || resolved.fixtures_unresolved
        {
            return Err(refusal(
                ExclusionReason::UnsupportedExpression,
                "imported modified registrations and unresolved fixtures are not supported yet",
            ));
        }
        let (title, _, callback) = split_declaration_args(&call.arguments);
        if call.arguments.len() != 2 {
            return Err(refusal(
                ExclusionReason::UnsupportedExpression,
                "imported registration options and extra arguments are not supported yet",
            ));
        }
        if !matches!(self.extract_title(title), Title::Static(_))
            || callback.and_then(as_function_like).is_none()
        {
            return Err(refusal(
                ExclusionReason::UnsupportedExpression,
                "the imported runner needs a static title and an inline callback",
            ));
        }
        if resolved.root == "describe" {
            let function = callback
                .and_then(as_function_like)
                .expect("inline callback checked above");
            let asynchronous = match &function {
                FunctionLike::Arrow(arrow) => arrow.r#async,
                FunctionLike::Function(function) => function.r#async || function.generator,
            };
            if asynchronous
                || !function.params().items.is_empty()
                || function.params().rest.is_some()
            {
                return Err(refusal(
                    ExclusionReason::UnsupportedExpression,
                    "imported suite callbacks must be synchronous and have no parameters",
                ));
            }
            match describe_body(callback) {
                Some(DescribeBody::Block(statements)) => {
                    self.imported_statements_shape(statements, registry)?;
                }
                Some(DescribeBody::Concise(expression)) => {
                    self.imported_expression_shape(expression, registry)?;
                }
                None => {
                    return Err(refusal(
                        ExclusionReason::UnsupportedExpression,
                        "the imported suite callback could not be read",
                    ))
                }
            }
        }
        Ok(Shape::Declares)
    }
}

/// What one registration-time call in an imported runner contributes.
#[derive(PartialEq)]
enum Shape {
    /// A proved ordinary call with no registration effects.
    Inert,
    /// A hook that joins the calling scope.
    Hook,
    /// A `test`/`describe` declaration.
    Declares,
}

/// Counts the hooks one scope holds, to detect what an imported runner added.
fn hook_count(scope: Option<&ScopeHooks>) -> usize {
    scope.map_or(0, |scope| {
        scope.before_all.len()
            + scope.before_each.len()
            + scope.after_each.len()
            + scope.after_all.len()
    })
}

/// Admits literals and function values whose evaluation alone has no effects.
fn inert_expression(expression: &Expression) -> bool {
    matches!(
        expression,
        Expression::StringLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::ArrowFunctionExpression(_)
            | Expression::FunctionExpression(_)
    )
}

/// Checks module execution for unmodeled setup effects.
/// Allows erased declarations, inert values, Vitest imports and validated re-export hops.
fn module_is_inert(
    program: &Program,
    file: &Path,
    modules: &[PathBuf],
    imports_inert: &mut dyn FnMut(&Path, &str) -> bool,
) -> bool {
    program.body.iter().all(|statement| match statement {
        Statement::ImportDeclaration(import) => {
            matches!(import.import_kind, ImportOrExportKind::Type)
                || import.specifiers.as_ref().is_some_and(|specifiers| {
                    !specifiers.is_empty()
                        && specifiers.iter().all(|specifier| {
                            matches!(specifier,
                            ImportDeclarationSpecifier::ImportSpecifier(specifier)
                                if matches!(specifier.import_kind, ImportOrExportKind::Type))
                        })
                })
                || import.source.value.as_str() == "vitest"
                || imports_inert(file, import.source.value.as_str())
        }
        Statement::ExportFromDeclaration(export) => {
            if matches!(export.export_kind, ImportOrExportKind::Type)
                || (!export.specifiers.is_empty()
                    && export
                        .specifiers
                        .iter()
                        .all(|specifier| matches!(specifier.export_kind, ImportOrExportKind::Type)))
            {
                return true;
            }
            match resolver::resolve(file, export.source.value.as_str()) {
                Resolved::Module(path) => {
                    modules.contains(&path) || imports_inert(file, export.source.value.as_str())
                }
                Resolved::Unresolved => false,
            }
        }
        Statement::FunctionDeclaration(_)
        | Statement::TSTypeAliasDeclaration(_)
        | Statement::TSInterfaceDeclaration(_)
        | Statement::ExportNamedDeclaration(_)
        | Statement::EmptyStatement(_) => true,
        Statement::ExportDefaultDeclaration(export) => matches!(
            &export.declaration,
            ExportDefaultDeclarationKind::FunctionDeclaration(_)
                | ExportDefaultDeclarationKind::ArrowFunctionExpression(_)
                | ExportDefaultDeclarationKind::FunctionExpression(_)
                | ExportDefaultDeclarationKind::TSInterfaceDeclaration(_)
        ),
        Statement::ExportDeclaration(export) => match &export.declaration {
            Declaration::FunctionDeclaration(_)
            | Declaration::TSTypeAliasDeclaration(_)
            | Declaration::TSInterfaceDeclaration(_) => true,
            oxc_ast::ast::Declaration::VariableDeclaration(declaration) => declaration
                .declarations
                .iter()
                .all(|declaration| declaration.init.as_ref().is_none_or(inert_expression)),
            _ => false,
        },
        Statement::VariableDeclaration(declaration) => declaration
            .declarations
            .iter()
            .all(|declaration| declaration.init.as_ref().is_none_or(inert_expression)),
        _ => false,
    })
}

/// Whether importing `specifier` from `importer` runs no unmodeled setup, memoized per module.
/// A first-party module is inert when its own statements are and so is everything it imports.
fn import_is_inert(state: &RefCell<ExpansionState>, importer: &Path, specifier: &str) -> bool {
    let Resolved::Module(path) = resolver::resolve(importer, specifier) else {
        return false;
    };
    if let Some(verdict) = state.borrow().closure.get(&path) {
        return *verdict;
    }
    let verdict = closure_is_inert(&path, &mut Vec::new());
    state.borrow_mut().closure.insert(path, verdict);
    verdict
}

/// Walks one module's imports depth-first. A revisit is a cycle that adds no effects; installed
/// packages, unreadable files and closures over the visit limit are not inert.
fn closure_is_inert(path: &Path, visited: &mut Vec<PathBuf>) -> bool {
    if visited.iter().any(|seen| seen == path) {
        return true;
    }
    if visited.len() >= MAX_IMPORT_CLOSURE {
        return false;
    }
    visited.push(path.to_path_buf());
    let verdict = helpers::with_parsed_file(path, |program, _| {
        Some(module_is_inert(
            program,
            path,
            &[],
            &mut |file, specifier| match resolver::resolve(file, specifier) {
                Resolved::Module(next) => closure_is_inert(&next, visited),
                Resolved::Unresolved => false,
            },
        ))
    });
    verdict == Some(true)
}
