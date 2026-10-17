//! Binding-aware class summaries and point-sensitive owned capture environments.
//!
//! Class bodies normalize once per class and mode. References carry a compact strong digest,
//! a nominal class slot and an environment digest; repeated `new` expressions do not clone
//! class bodies. All state here dies with its callback, before the owned IR is retained.
use super::*;
use oxc_ast::ast::{
    AssignmentExpression, Class, ClassElement, MethodDefinitionKind, ReturnStatement,
};
use oxc_ast::AstKind;

/// One cached body, separated from the capture values read at a particular construction.
#[derive(Clone)]
struct ClassSummary {
    digest: String,
    captures: Vec<SymbolId>,
    definition: String,
    escapes_this: bool,
}

/// A class whose receiver leaves a method can expose its runtime name indirectly.
#[derive(Default)]
struct EscapedThis {
    found: bool,
    depth: u32,
}

impl<'a> Visit<'a> for EscapedThis {
    fn visit_expression(&mut self, expression: &Expression<'a>) {
        if self.found {
            return;
        }
        if self.depth >= MAX_DEPTH {
            self.found = true;
            return;
        }
        self.depth += 1;
        walk::walk_expression(self, expression);
        self.depth -= 1;
    }

    fn visit_statement(&mut self, statement: &Statement<'a>) {
        if self.found {
            return;
        }
        if self.depth >= MAX_DEPTH {
            self.found = true;
            return;
        }
        self.depth += 1;
        walk::walk_statement(self, statement);
        self.depth -= 1;
    }

    fn visit_assignment_expression(&mut self, assignment: &AssignmentExpression<'a>) {
        self.found |= matches!(assignment.right, Expression::ThisExpression(_));
        if !self.found {
            walk::walk_assignment_expression(self, assignment);
        }
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        self.found |= call.arguments.iter().any(|argument| {
            matches!(
                argument.as_expression(),
                Some(Expression::ThisExpression(_))
            )
        });
        if !self.found {
            walk::walk_call_expression(self, call);
        }
    }

    fn visit_new_expression(&mut self, new: &NewExpression<'a>) {
        self.found |= new.arguments.iter().any(|argument| {
            matches!(
                argument.as_expression(),
                Some(Expression::ThisExpression(_))
            )
        });
        if !self.found {
            walk::walk_new_expression(self, new);
        }
    }

    fn visit_return_statement(&mut self, statement: &ReturnStatement<'a>) {
        self.found |= matches!(statement.argument, Some(Expression::ThisExpression(_)));
        if !self.found {
            walk::walk_return_statement(self, statement);
        }
    }
}

/// One indexed operation; parser handles remain confined to this temporary frontend cache.
#[derive(Clone)]
enum HistoryAction {
    Class {
        class: oxc_semantic::NodeId,
        site: oxc_semantic::NodeId,
        phase: String,
    },
    Call(oxc_semantic::NodeId),
    Assignment(oxc_semantic::NodeId),
    Unknown,
}

/// Ordered operation sites and the strong digest of every already-rendered prefix.
#[derive(Default)]
struct HistoryIndex {
    actions: Vec<(u32, u32, HistoryAction)>,
    prefixes: Vec<(BodyDigest, bool)>,
}

/// One bounded lexical index; nested declarations shadow only their own private names.
#[derive(Default)]
struct PrivateIndex {
    declarations: HashMap<String, oxc_semantic::NodeId>,
    references: HashMap<String, Vec<oxc_semantic::NodeId>>,
    shadows: Vec<HashSet<String>>,
    bytes: usize,
    overflow: bool,
}

impl<'a> Visit<'a> for PrivateIndex {
    /// Traverse each class subtree once, bounding nested scopes and temporary strings.
    fn visit_class(&mut self, class: &Class<'a>) {
        if self.shadows.len() >= 16 || self.overflow {
            self.overflow = true;
            return;
        }
        let mut names = HashSet::new();
        if !self.shadows.is_empty() {
            for element in &class.body.body {
                if let Some(PropertyKey::PrivateIdentifier(name)) = element.property_key() {
                    self.bytes += name.name.len();
                    names.insert(name.name.to_string());
                }
            }
        }
        if self.bytes > 1024 * 1024 {
            self.overflow = true;
            return;
        }
        self.shadows.push(names);
        walk::walk_class(self, class);
        self.shadows.pop();
    }

    /// Index unshadowed private references using their existing semantic node handles.
    fn visit_private_field_expression(&mut self, field: &PrivateFieldExpression<'a>) {
        if self.overflow {
            return;
        }
        if !self
            .shadows
            .iter()
            .any(|names| names.contains(field.field.name.as_str()))
        {
            self.bytes += std::mem::size_of::<oxc_semantic::NodeId>();
            if !self.references.contains_key(field.field.name.as_str()) {
                self.bytes += field.field.name.len();
            }
            if self.bytes > 1024 * 1024 {
                self.overflow = true;
                return;
            }
            self.references
                .entry(field.field.name.to_string())
                .or_default()
                .push(field.node_id.get());
        }
        walk::walk_private_field_expression(self, field);
    }
}

/// Per-callback temporary summaries; neither parser nodes nor cache strings enter the IR.
#[derive(Default)]
pub(super) struct ClassCache {
    bodies: HashMap<(u32, bool), ClassSummary>,
    bytes: usize,
    nominals: HashMap<u32, usize>,
    histories: HashMap<(SymbolId, u32, bool), HistoryIndex>,
    class_mutations: HashMap<SymbolId, Option<u32>>,
    writes: HashMap<SymbolId, Vec<oxc_semantic::NodeId>>,
    callable_metadata: HashMap<SymbolId, bool>,
    instance_bindings: HashMap<SymbolId, String>,
    private_members: HashMap<u32, PrivateIndex>,
    private_stability: HashMap<(u32, String), bool>,
}

/// Read an initializer only when the resolved declaration binds this whole value.
/// A destructured child shares its declarator with the other children, but does not
/// receive the declarator's entire initializer as its runtime value.
fn initializer<'a>(semantic: &Semantic<'a>, symbol: SymbolId) -> Option<&'a Expression<'a>> {
    if semantic.nodes().iter().len() == 0 {
        return None;
    }
    let declares_whole_value = |declarator: &VariableDeclarator<'a>| {
        matches!(
            &declarator.id,
            BindingPattern::BindingIdentifier(binding)
                if binding.symbol_id.get() == Some(symbol)
        )
    };
    match semantic
        .nodes()
        .get_node(semantic.scoping().symbol_declaration(symbol))
        .kind()
    {
        AstKind::VariableDeclarator(declaration) if declares_whole_value(declaration) => {
            declaration.init.as_ref()
        }
        AstKind::VariableDeclaration(declaration) => declaration
            .declarations
            .iter()
            .find(|item| declares_whole_value(item))
            .and_then(|item| item.init.as_ref()),
        _ => None,
    }
}

impl<'a, 'b, 's, 'm> Serializer<'a, 'b, 's, 'm> {
    /// Save declaration identities before behavior collection can reorder first observations.
    pub(super) fn record_instance_binding(&self, symbol: SymbolId, binding: &str) {
        if self.ctx.expand_classes
            || !self.ctx.class_regions.is_empty()
            || !matches!(
                initializer(self.ctx.semantic, symbol),
                Some(Expression::NewExpression(_))
            )
        {
            return;
        }
        let mut cache = self.ctx.class_cache.borrow_mut();
        if cache.instance_bindings.contains_key(&symbol) {
            return;
        }
        let bytes = binding.len() + std::mem::size_of::<SymbolId>();
        if cache.bytes.saturating_add(bytes) > 4 * 1024 * 1024 {
            drop(cache);
            self.unknown_class();
            return;
        }
        cache.bytes += bytes;
        cache.instance_bindings.insert(symbol, binding.to_owned());
    }

    /// Prefer enclosing capture slots, then declaration identity over first-use numbering.
    fn instance_binding(&mut self, symbol: SymbolId) -> String {
        if let Some(slot) = self.capture_slot(symbol) {
            return slot;
        }
        if self.ctx.class_regions.is_empty() {
            return self
                .ctx
                .class_cache
                .borrow()
                .instance_bindings
                .get(&symbol)
                .cloned()
                .unwrap_or_else(|| self.unknown_class());
        }
        canonical_local(
            &mut self.ctx.canonical,
            &mut self.ctx.next_local,
            self.ctx.local_prefix,
            symbol,
        )
    }

    /// Resolve one reference through the semantic graph, independent of local spelling.
    fn class_symbol(&self, ident: &IdentifierReference<'a>) -> Option<SymbolId> {
        ident
            .reference_id
            .get()
            .and_then(|id| self.ctx.semantic.scoping().get_reference(id).symbol_id())
    }

    /// Reuse one nominal slot without conflating separately declared classes.
    fn class_nominal(&self, class: &Class<'a>) -> usize {
        let mut cache = self.ctx.class_cache.borrow_mut();
        let next = cache.nominals.len();
        *cache.nominals.entry(class.span.start).or_insert(next)
    }

    /// Return one explicit unknown marker and poison only this callback's comparison context.
    fn unknown_class(&self) -> String {
        self.ctx.class_uncertain.set(true);
        "<unresolved-class-state>".to_owned()
    }

    /// Resolve a binding at the current use, honoring proven straight-line overwrites.
    fn value_at(&mut self, symbol: SymbolId, position: u32) -> Option<&'a Expression<'a>> {
        if self.ctx.semantic.nodes().iter().len() == 0 {
            self.unknown_class();
            return None;
        }
        let mut value = initializer(self.ctx.semantic, symbol);
        let cached = self.ctx.class_cache.borrow().writes.get(&symbol).cloned();
        let writes = cached.unwrap_or_else(|| {
            let mut writes: Vec<_> = self
                .ctx
                .semantic
                .scoping()
                .get_resolved_references(symbol)
                .filter(|reference| {
                    reference.is_write() && self.enclosing_class(reference.node_id()).is_none()
                })
                .map(|reference| reference.node_id())
                .collect();
            writes.sort_by_key(|id| self.ctx.semantic.nodes().get_node(*id).kind().span().start);
            self.ctx
                .class_cache
                .borrow_mut()
                .writes
                .insert(symbol, writes.clone());
            writes
        });
        let index = writes.partition_point(|id| {
            self.ctx.semantic.nodes().get_node(*id).kind().span().start < position
        });
        if let Some(id) = index.checked_sub(1).map(|index| writes[index]) {
            let AstKind::AssignmentExpression(assignment) =
                self.ctx.semantic.nodes().parent_kind(id)
            else {
                self.unknown_class();
                return None;
            };
            if assignment.operator.as_str() != "=" || !self.straight_line(assignment.node_id.get())
            {
                self.unknown_class();
                return None;
            }
            if self
                .ctx
                .semantic
                .scoping()
                .get_resolved_references(symbol)
                .any(|read| {
                    read.is_read()
                        && within(
                            self.ctx
                                .semantic
                                .nodes()
                                .get_node(read.node_id())
                                .kind()
                                .span(),
                            assignment.right.span(),
                        )
                })
            {
                self.unknown_class();
                return None;
            }
            value = Some(&assignment.right);
        }
        value
    }

    /// Only direct callback statements have a known execution order at this use site.
    fn straight_line(&self, mut id: oxc_semantic::NodeId) -> bool {
        for _ in 0..MAX_DEPTH {
            let parent = self.ctx.semantic.nodes().parent_node(id);
            if parent.kind().span() == self.ctx.body_span {
                return true;
            }
            match parent.kind() {
                AstKind::ExpressionStatement(_)
                | AstKind::VariableDeclarator(_)
                | AstKind::VariableDeclaration(_)
                | AstKind::FunctionBody(_) => {}
                _ => return false,
            }
            if parent.id() == id {
                return false;
            }
            id = parent.id();
        }
        false
    }

    /// Reuse the per-file semantic parent index; never search all nodes for containment.
    fn enclosing_class(&self, mut id: oxc_semantic::NodeId) -> Option<&'a Class<'a>> {
        for _ in 0..MAX_DEPTH {
            let parent = self.ctx.semantic.nodes().parent_node(id);
            if let AstKind::Class(class) = parent.kind() {
                return Some(class);
            }
            if parent.id() == id || parent.kind().span() == self.ctx.body_span {
                return None;
            }
            id = parent.id();
        }
        None
    }

    /// Recognize candidate class bindings without poisoning ordinary scalar/object references.
    fn class_candidate(&self, symbol: SymbolId, seen: &mut HashSet<SymbolId>, depth: u32) -> bool {
        if depth > MAX_DEPTH || !seen.insert(symbol) || self.ctx.semantic.nodes().iter().len() == 0
        {
            return false;
        }
        let declaration = self
            .ctx
            .semantic
            .nodes()
            .get_node(self.ctx.semantic.scoping().symbol_declaration(symbol))
            .kind();
        if matches!(declaration, AstKind::Class(_)) {
            return true;
        }
        initializer(self.ctx.semantic, symbol)
            .is_some_and(|value| self.class_candidate_expression(value, seen, depth + 1))
    }

    /// Erased initializer wrappers preserve alias provenance with the same cycle/depth budget.
    fn class_candidate_expression(
        &self,
        expression: &Expression<'a>,
        seen: &mut HashSet<SymbolId>,
        depth: u32,
    ) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        match expression {
            Expression::ClassExpression(_) => true,
            Expression::Identifier(ident) => self
                .class_symbol(ident)
                .is_some_and(|symbol| self.class_candidate(symbol, seen, depth + 1)),
            Expression::ParenthesizedExpression(inner) => {
                self.class_candidate_expression(&inner.expression, seen, depth + 1)
            }
            Expression::TSAsExpression(inner) => {
                self.class_candidate_expression(&inner.expression, seen, depth + 1)
            }
            Expression::TSSatisfiesExpression(inner) => {
                self.class_candidate_expression(&inner.expression, seen, depth + 1)
            }
            Expression::TSNonNullExpression(inner) => {
                self.class_candidate_expression(&inner.expression, seen, depth + 1)
            }
            _ => false,
        }
    }

    /// Follow binding-owned class values, including immutable aliases and proven reassignment.
    pub(super) fn class_reference(
        &mut self,
        ident: &IdentifierReference<'a>,
        depth: u32,
    ) -> Option<String> {
        let symbol = self.class_symbol(ident)?;
        if !is_local_symbol(self.ctx.semantic, self.ctx.body_span, symbol)
            || !self.class_candidate(symbol, &mut HashSet::new(), depth)
        {
            return None;
        }
        let position = self
            .ctx
            .class_uses
            .last()
            .copied()
            .unwrap_or(ident.span.start);
        let class = self.resolve_class_symbol(symbol, position, &mut HashSet::new(), depth)?;
        self.class_external_mutations(symbol, class.span, position);
        self.ctx.class_uses.push(position);
        let value = self.ser_class(class, depth + 1);
        self.ctx.class_uses.pop();
        Some(value)
    }

    /// Unknown escapes and writes to a class object invalidate its modeled implementation.
    fn class_external_mutations(&mut self, symbol: SymbolId, class: Span, position: u32) {
        let cached = self
            .ctx
            .class_cache
            .borrow()
            .class_mutations
            .get(&symbol)
            .copied();
        let first = match cached {
            Some(first) => first,
            None => {
                let mut owners = vec![symbol];
                let mut seen = HashSet::new();
                let mut first = None;
                while let Some(owner) = owners.pop() {
                    if !seen.insert(owner) {
                        continue;
                    }
                    if seen.len() > 256 {
                        first = Some(0);
                        break;
                    }
                    for reference in self.ctx.semantic.scoping().get_resolved_references(owner) {
                        let node = self.ctx.semantic.nodes().get_node(reference.node_id());
                        if within(node.kind().span(), class) {
                            continue;
                        }
                        let parent = self.ctx.semantic.nodes().parent_node(node.id());
                        let bad = match parent.kind() {
                            AstKind::NewExpression(_)
                            | AstKind::Class(_)
                            | AstKind::BinaryExpression(_) => false,
                            AstKind::VariableDeclarator(declaration) => {
                                if let BindingPattern::BindingIdentifier(binding) = &declaration.id
                                {
                                    if let Some(alias) = binding.symbol_id.get() {
                                        owners.push(alias);
                                    }
                                    false
                                } else {
                                    true
                                }
                            }
                            AstKind::StaticMemberExpression(_)
                            | AstKind::ComputedMemberExpression(_) => {
                                let mut outer = self.ctx.semantic.nodes().parent_node(parent.id());
                                for _ in 0..MAX_DEPTH {
                                    if !matches!(
                                        outer.kind(),
                                        AstKind::StaticMemberExpression(_)
                                            | AstKind::ComputedMemberExpression(_)
                                    ) {
                                        break;
                                    }
                                    outer = self.ctx.semantic.nodes().parent_node(outer.id());
                                }
                                match outer.kind() {
                                    AstKind::AssignmentExpression(_)
                                    | AstKind::UpdateExpression(_) => true,
                                    AstKind::CallExpression(call) => {
                                        !self.known_read_only_assertion(call)
                                    }
                                    _ => false,
                                }
                            }
                            AstKind::AssignmentExpression(assignment) => {
                                if assignment.operator.as_str() != "="
                                    || !self.straight_line(assignment.node_id.get())
                                {
                                    true
                                } else if let AssignmentTarget::AssignmentTargetIdentifier(alias) =
                                    &assignment.left
                                {
                                    if let Some(symbol) = self.class_symbol(alias) {
                                        if is_local_symbol(
                                            self.ctx.semantic,
                                            self.ctx.body_span,
                                            symbol,
                                        ) {
                                            owners.push(symbol);
                                            false
                                        } else {
                                            true
                                        }
                                    } else {
                                        true
                                    }
                                } else {
                                    true
                                }
                            }
                            _ => true,
                        };
                        if bad {
                            let point = parent.kind().span().start;
                            first = Some(first.map_or(point, |old: u32| old.min(point)));
                        }
                    }
                }
                self.ctx
                    .class_cache
                    .borrow_mut()
                    .class_mutations
                    .insert(symbol, first);
                first
            }
        };
        if first.is_some_and(|point| point < position) {
            self.unknown_class();
        }
    }

    /// Resolve aliases without mixing distinct declarations or following cycles indefinitely.
    fn resolve_class_symbol(
        &mut self,
        symbol: SymbolId,
        position: u32,
        seen: &mut HashSet<SymbolId>,
        depth: u32,
    ) -> Option<&'a Class<'a>> {
        if depth > MAX_DEPTH || !seen.insert(symbol) {
            self.unknown_class();
            return None;
        }
        let declaration = self
            .ctx
            .semantic
            .nodes()
            .get_node(self.ctx.semantic.scoping().symbol_declaration(symbol))
            .kind();
        if let AstKind::Class(class) = declaration {
            if self.ctx.semantic.scoping().symbol_is_mutated(symbol) {
                self.unknown_class();
            }
            return Some(class);
        }
        let init = self.value_at(symbol, position)?;
        self.resolve_class_expression(init, position, seen, depth + 1)
    }

    /// Unwrap syntax-only expressions while retaining the owning declaration.
    fn resolve_class_expression<'c>(
        &mut self,
        expression: &'c Expression<'a>,
        position: u32,
        seen: &mut HashSet<SymbolId>,
        depth: u32,
    ) -> Option<&'c Class<'a>> {
        if depth > MAX_DEPTH {
            self.unknown_class();
            return None;
        }
        match expression {
            Expression::ClassExpression(class) => Some(class),
            Expression::Identifier(ident) => self
                .class_symbol(ident)
                .and_then(|symbol| self.resolve_class_symbol(symbol, position, seen, depth + 1)),
            Expression::ParenthesizedExpression(inner) => {
                self.resolve_class_expression(&inner.expression, position, seen, depth + 1)
            }
            Expression::TSAsExpression(inner) => {
                self.resolve_class_expression(&inner.expression, position, seen, depth + 1)
            }
            Expression::TSSatisfiesExpression(inner) => {
                self.resolve_class_expression(&inner.expression, position, seen, depth + 1)
            }
            Expression::TSNonNullExpression(inner) => {
                self.resolve_class_expression(&inner.expression, position, seen, depth + 1)
            }
            _ => None,
        }
    }

    /// Classify constructor provenance before point-sensitive instance writes can degrade it.
    fn owned_constructor_candidate(&self, expression: &Expression<'a>, depth: u32) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        match expression {
            Expression::ClassExpression(_) => true,
            Expression::Identifier(ident) => self.class_symbol(ident).is_some_and(|symbol| {
                is_local_symbol(self.ctx.semantic, self.ctx.body_span, symbol)
                    && self.class_candidate(symbol, &mut HashSet::new(), depth + 1)
            }),
            Expression::ParenthesizedExpression(inner) => {
                self.owned_constructor_candidate(&inner.expression, depth + 1)
            }
            Expression::TSAsExpression(inner) => {
                self.owned_constructor_candidate(&inner.expression, depth + 1)
            }
            Expression::TSSatisfiesExpression(inner) => {
                self.owned_constructor_candidate(&inner.expression, depth + 1)
            }
            Expression::TSNonNullExpression(inner) => {
                self.owned_constructor_candidate(&inner.expression, depth + 1)
            }
            _ => false,
        }
    }

    /// Find the current direct construction for a locally initialized instance binding.
    fn owned_instance_new(
        &mut self,
        symbol: SymbolId,
        position: u32,
    ) -> Option<&'a NewExpression<'a>> {
        if !is_local_symbol(self.ctx.semantic, self.ctx.body_span, symbol) {
            return None;
        }
        let Expression::NewExpression(initial) = initializer(self.ctx.semantic, symbol)? else {
            return None;
        };
        if !self.owned_constructor_candidate(&initial.callee, 0) {
            return None;
        }
        let value = self.value_at(symbol, position)?;
        let writes = self
            .ctx
            .class_cache
            .borrow()
            .writes
            .get(&symbol)
            .cloned()
            .unwrap_or_default();
        if writes.iter().any(|id| {
            let AstKind::AssignmentExpression(assignment) =
                self.ctx.semantic.nodes().parent_kind(*id)
            else {
                return true;
            };
            !self.straight_line(assignment.node_id.get())
        }) {
            self.unknown_class();
            return None;
        }
        match value {
            Expression::NewExpression(new) => Some(new),
            _ => None,
        }
    }

    /// A whole instance passed to application code can expose its nominal class name.
    pub(super) fn instance_name_observed(&self, ident: &IdentifierReference<'a>) -> bool {
        if self.ctx.semantic.nodes().iter().len() == 0 {
            return false;
        }
        let Some(reference_id) = ident.reference_id.get() else {
            return true;
        };
        let reference = self.ctx.semantic.scoping().get_reference(reference_id);
        let parent = self.ctx.semantic.nodes().parent_node(reference.node_id());
        let AstKind::CallExpression(call) = parent.kind() else {
            return matches!(
                parent.kind(),
                AstKind::NewExpression(_) | AstKind::ReturnStatement(_)
            );
        };
        if let Expression::StaticMemberExpression(member) = &call.callee {
            if matches!(
                member.property.name.as_str(),
                "toBe" | "toBeDefined" | "toBeTruthy" | "toBeFalsy"
            ) {
                if let Expression::CallExpression(base) = &member.object {
                    if self.known_read_only_assertion(base) {
                        return false;
                    }
                }
            }
        }
        if !self.known_read_only_assertion(call) {
            return true;
        }
        let matcher = self.ctx.semantic.nodes().parent_node(parent.id());
        !matches!(matcher.kind(), AstKind::StaticMemberExpression(member)
            if matches!(member.property.name.as_str(), "toBe" | "toBeDefined" | "toBeTruthy" | "toBeFalsy"))
    }

    /// An instance receiver retains the class and argument environment of its construction.
    pub(super) fn class_instance_reference(
        &mut self,
        ident: &IdentifierReference<'a>,
        depth: u32,
    ) -> Option<String> {
        let symbol = self.class_symbol(ident)?;
        if self.ctx.capture_stack.contains(&symbol) {
            return None;
        }
        let new = self.owned_instance_new(symbol, ident.span.start)?;
        let class = self.resolve_class_expression(
            &new.callee,
            new.span.start,
            &mut HashSet::new(),
            depth + 1,
        )?;
        if let Expression::Identifier(class_ident) = &new.callee {
            if let Some(class_symbol) = self.class_symbol(class_ident) {
                self.class_external_mutations(class_symbol, class.span, ident.span.start);
            }
        }
        let observed_name = if self.instance_name_observed(ident) {
            let name = class.id.as_ref().map_or_else(
                || self.unknown_class(),
                |binding| self.literal("string", binding.name.as_str()),
            );
            format!(" name={name}")
        } else {
            String::new()
        };
        self.ctx.capture_stack.push(symbol);
        self.ctx.class_uses.push(ident.span.start);
        let live = self.ser_class(class, depth + 1);
        self.ctx.class_uses.pop();
        let construction = self.ser_new(new, depth + 1);
        self.ctx.capture_stack.pop();
        let binding = self.instance_binding(symbol);
        Some(format!(
            "(instance {binding}{observed_name} {construction} live={live})"
        ))
    }

    /// Captures are placeholders in the cached body, with values supplied separately at use.
    pub(super) fn class_capture(
        &mut self,
        ident: &IdentifierReference<'a>,
        _depth: u32,
    ) -> Option<String> {
        self.capture_slot(self.class_symbol(ident)?)
    }

    /// Register a lexical capture in its enclosing summary before rendering its environment.
    fn capture_slot(&mut self, symbol: SymbolId) -> Option<String> {
        let region = *self.ctx.class_regions.last()?;
        if within(self.ctx.semantic.scoping().symbol_span(symbol), region)
            || !is_local_symbol(self.ctx.semantic, self.ctx.body_span, symbol)
        {
            return None;
        }
        let frame = self.ctx.capture_frames.last_mut()?;
        let index = frame
            .iter()
            .position(|candidate| *candidate == symbol)
            .unwrap_or_else(|| {
                let index = frame.len();
                frame.push(symbol);
                index
            });
        Some(format!("capture:{index}"))
    }

    /// Retain the enclosing binding alongside its captured value and mutation history.
    fn capture_value(&mut self, symbol: SymbolId, position: u32, depth: u32) -> String {
        if let Some(slot) = self.capture_slot(symbol) {
            // The enclosing summary supplies state at use; eager history here can re-enter it.
            return slot;
        }
        let binding = canonical_local(
            &mut self.ctx.canonical,
            &mut self.ctx.next_local,
            self.ctx.local_prefix,
            symbol,
        );
        let value = self.capture_contents(symbol, position, depth);
        format!("(capture {binding} {value})")
    }

    /// Render a captured value and its ordered owned mutations preceding this construction.
    fn capture_contents(&mut self, symbol: SymbolId, position: u32, depth: u32) -> String {
        if depth > MAX_DEPTH || self.ctx.capture_stack.contains(&symbol) {
            return self.unknown_class();
        }
        if let Some(value) = self.ctx.arg_substitution.get(&symbol).cloned() {
            self.ctx.consumed_args.insert(symbol);
            return value;
        }
        if let Some((kind, value)) = self.ctx.row_substitution.get(&symbol).cloned() {
            return self.literal(kind, &value);
        }
        let Some(init) = self.value_at(symbol, position) else {
            if let Some((callable, _, _)) =
                self.resolve_callable_symbol(symbol, position, &mut HashSet::new(), depth + 1)
            {
                self.ctx.capture_stack.push(symbol);
                let value = self.owned_callable(&callable, position, depth + 1);
                let history =
                    self.capture_history(symbol, callable.span(), true, position, depth + 1);
                self.ctx.capture_stack.pop();
                return format!("(owned {value} history={history})");
            }
            return self.unknown_class();
        };
        let closure = matches!(
            init,
            Expression::FunctionExpression(_) | Expression::ArrowFunctionExpression(_)
        );
        if expression_has_effect(init, 0) && !closure && !self.owned_mock(init) {
            return self.unknown_class();
        }
        self.ctx.capture_stack.push(symbol);
        let value = if closure {
            self.owned_closure(init, position, depth + 1)
        } else {
            self.ser_expr(init, depth + 1)
        };
        let heap = matches!(
            init,
            Expression::ArrayExpression(_) | Expression::ObjectExpression(_)
        ) || closure
            || self.owned_mock(init);
        let history = self.capture_history(symbol, init.span(), heap, position, depth + 1);
        self.ctx.capture_stack.pop();
        format!("(owned {value} history={history})")
    }

    /// Classify each captured binding's reference graph once, preserving a single event order.
    fn history_index(&mut self, symbol: SymbolId, init: Span, heap: bool) -> HistoryIndex {
        let mut actions = Vec::new();
        let mut phases = HashSet::new();
        for reference in self.ctx.semantic.scoping().get_resolved_references(symbol) {
            let node = self.ctx.semantic.nodes().get_node(reference.node_id());
            if let Some(class) = self.enclosing_class(node.id()) {
                if !phases.insert((class.span.start, self.capture_phase_key(node.id()))) {
                    continue;
                }
                actions.extend(self.capture_execution_sites(class, node.id()));
                continue;
            }
            if !heap || node.kind().span().start <= init.end {
                continue;
            }
            let mut parent = self.ctx.semantic.nodes().parent_node(node.id());
            match parent.kind() {
                AstKind::StaticMemberExpression(_) | AstKind::ComputedMemberExpression(_) => {
                    parent = self.ctx.semantic.nodes().parent_node(parent.id())
                }
                AstKind::CallExpression(call) if self.known_read_only_assertion(call) => continue,
                _ => {
                    actions.push((
                        parent.kind().span().end,
                        parent.kind().span().start,
                        HistoryAction::Unknown,
                    ));
                    continue;
                }
            }
            let action = match parent.kind() {
                AstKind::CallExpression(call) if self.known_read_only_assertion(call) => continue,
                AstKind::CallExpression(call) if self.straight_line(call.node_id.get()) => {
                    HistoryAction::Call(call.node_id.get())
                }
                AstKind::AssignmentExpression(assignment)
                    if self.straight_line(assignment.node_id.get()) =>
                {
                    HistoryAction::Assignment(assignment.node_id.get())
                }
                _ => HistoryAction::Unknown,
            };
            actions.push((parent.kind().span().end, parent.kind().span().start, action));
        }
        actions.retain(|(_, start, _)| *start > init.end);
        actions.sort_by_key(|(end, start, _)| (*end, *start));
        actions.dedup_by_key(|(end, start, _)| (*end, *start));
        if actions.len() > 4096 {
            actions.truncate(4096);
            if let Some(last) = actions.last_mut() {
                last.2 = HistoryAction::Unknown;
            }
        }
        HistoryIndex {
            actions,
            prefixes: Vec::new(),
        }
    }

    /// Extend a cached strong prefix only for newly executed operations, then look up older uses.
    fn capture_history(
        &mut self,
        symbol: SymbolId,
        init: Span,
        heap: bool,
        position: u32,
        depth: u32,
    ) -> String {
        let key = (symbol, init.end, self.mode == Mode::Structural);
        let cached = self.ctx.class_cache.borrow_mut().histories.remove(&key);
        let mut index = match cached {
            Some(index) => index,
            None => {
                let index = self.history_index(symbol, init, heap);
                let bytes = index
                    .actions
                    .iter()
                    .map(|(_, _, action)| {
                        std::mem::size_of::<(u32, u32, HistoryAction)>()
                            + std::mem::size_of::<(BodyDigest, bool)>()
                            + match action {
                                HistoryAction::Class { phase, .. } => phase.len(),
                                _ => 0,
                            }
                    })
                    .sum::<usize>();
                let mut cache = self.ctx.class_cache.borrow_mut();
                if cache.bytes.saturating_add(bytes) > 4 * 1024 * 1024 {
                    drop(cache);
                    return self.unknown_class();
                }
                cache.bytes += bytes;
                index
            }
        };
        let required = index.actions.partition_point(|(end, _, _)| *end < position);
        let previous = self
            .ctx
            .arg_substitution
            .insert(symbol, "<owned-state-input>".to_owned());
        while index.prefixes.len() < required {
            let (_end, start, action) = index.actions[index.prefixes.len()].clone();
            let before = self.ctx.class_uncertain.get();
            let operation = match action {
                HistoryAction::Call(id) => match self.ctx.semantic.nodes().get_node(id).kind() {
                    AstKind::CallExpression(call) => self.ser_call(call, depth + 1),
                    _ => self.unknown_class(),
                },
                HistoryAction::Assignment(id) => {
                    match self.ctx.semantic.nodes().get_node(id).kind() {
                        AstKind::AssignmentExpression(assignment) => format!(
                            "(write {} {} {})",
                            assignment.operator.as_str(),
                            self.ser_assignment_target(&assignment.left, depth + 1),
                            self.ser_expr(&assignment.right, depth + 1)
                        ),
                        _ => self.unknown_class(),
                    }
                }
                HistoryAction::Class { class, site, phase } => {
                    match self.ctx.semantic.nodes().get_node(class).kind() {
                        AstKind::Class(class) if phase == "definition" => self
                            .class_summary(class, depth + 1)
                            .map_or_else(|| self.unknown_class(), |summary| summary.definition),
                        AstKind::Class(class) => {
                            match self.ctx.semantic.nodes().get_node(site).kind() {
                                AstKind::NewExpression(new) if self.straight_line(site) => self
                                    .class_operation(
                                        class,
                                        symbol,
                                        start,
                                        &new.arguments,
                                        &phase,
                                        depth + 1,
                                    ),
                                AstKind::CallExpression(call) if self.straight_line(site) => self
                                    .class_operation(
                                        class,
                                        symbol,
                                        start,
                                        &call.arguments,
                                        &phase,
                                        depth + 1,
                                    ),
                                _ => self.unknown_class(),
                            }
                        }
                        _ => self.unknown_class(),
                    }
                }
                HistoryAction::Unknown => self.unknown_class(),
            };
            let (previous_digest, previous_unknown) = index
                .prefixes
                .last()
                .copied()
                .unwrap_or((BodyDigest::default(), false));
            let digest = BodyDigest::of(
                &serde_json::to_string(&(previous_digest, BodyDigest::of(&operation)))
                    .expect("history prefix serializes"),
            );
            let unknown = previous_unknown
                || (!before && self.ctx.class_uncertain.get())
                || matches!(operation.as_str(), "<unresolved-class-state>");
            index.prefixes.push((digest, unknown));
        }
        if let Some(value) = previous {
            self.ctx.arg_substitution.insert(symbol, value);
        } else {
            self.ctx.arg_substitution.remove(&symbol);
        }
        let (digest, unknown) = required
            .checked_sub(1)
            .map_or((BodyDigest::default(), false), |index_| {
                index.prefixes[index_]
            });
        self.ctx
            .class_cache
            .borrow_mut()
            .histories
            .insert(key, index);
        if unknown {
            self.unknown_class();
        }
        serde_json::to_string(&digest).expect("history digest serializes")
    }

    /// Recognize Vitest's fresh mock by resolved import ownership, not local spelling.
    fn owned_mock(&self, expression: &Expression<'a>) -> bool {
        let Expression::CallExpression(call) = expression else {
            return false;
        };
        let Expression::StaticMemberExpression(member) = &call.callee else {
            return false;
        };
        let Expression::Identifier(ident) = &member.object else {
            return false;
        };
        member.property.name == "fn"
            && self.ctx.import_origin_of(ident).is_some_and(|origin| {
                origin.specifier == "vitest" && matches!(origin.exported.as_str(), "vi" | "vitest")
            })
    }

    /// Normalize a stable closure with the same binding-aware capture frames as a class body.
    fn owned_closure(&mut self, expression: &Expression<'a>, position: u32, depth: u32) -> String {
        let callable = match expression {
            Expression::ArrowFunctionExpression(arrow) => FunctionLike::Arrow(arrow),
            Expression::FunctionExpression(function) => FunctionLike::Function(function),
            _ => return self.unknown_class(),
        };
        self.owned_callable(&callable, position, depth + 1)
    }

    /// Share bounded lexical environments across declared and expression callables.
    fn owned_callable(
        &mut self,
        callable: &FunctionLike<'a, '_>,
        position: u32,
        depth: u32,
    ) -> String {
        let mut child = Serializer::new(self.ctx.fresh_for_pass(), self.mode);
        child.ctx.canonical.clear();
        child.ctx.next_local = 0;
        child.ctx.local_prefix = "F";
        child.ctx.class_regions.push(callable.span());
        child.ctx.capture_frames.push(Vec::new());
        let body = child.ser_callable(callable, depth + 1);
        let captures = child
            .ctx
            .capture_frames
            .pop()
            .expect("closure capture frame");
        let environment: Vec<_> = captures
            .iter()
            .map(|capture| self.capture_value(*capture, position, depth + 1))
            .collect();
        format!(
            "(owned-closure {} environment={})",
            serde_json::to_string(&BodyDigest::of(&body)).expect("closure digest serializes"),
            serde_json::to_string(&environment).expect("closure environment serializes")
        )
    }

    /// A resolved Vitest assertion observes a capture without escaping it to application code.
    fn known_read_only_assertion(&self, call: &CallExpression<'a>) -> bool {
        let Expression::Identifier(ident) = &call.callee else {
            return false;
        };
        self.ctx
            .import_origin_of(ident)
            .is_some_and(|origin| origin.specifier == "vitest" && origin.exported == "expect")
    }

    /// Encode one proven class operation with the target state left as an owned symbolic input.
    fn class_operation(
        &mut self,
        class: &Class<'a>,
        target: SymbolId,
        point: u32,
        arguments: &[Argument<'a>],
        phase: &str,
        depth: u32,
    ) -> String {
        let Some(summary) = self.class_summary(class, depth + 1) else {
            return self.unknown_class();
        };
        self.ctx.class_uses.push(point);
        let environment: Vec<_> = summary
            .captures
            .iter()
            .map(|symbol| {
                if *symbol == target {
                    "<owned-state-input>".to_owned()
                } else {
                    self.capture_value(*symbol, point, depth + 1)
                }
            })
            .collect();
        let arguments: Vec<_> = arguments
            .iter()
            .map(|argument| self.ser_argument(argument, depth + 1))
            .collect();
        self.ctx.class_uses.pop();
        let nominal = self.class_nominal(class);
        let descriptor = format!(
            "{phase} class=C{nominal} body={} env={} args={}",
            summary.digest,
            serde_json::to_string(&environment).expect("operation environment serializes"),
            serde_json::to_string(&arguments).expect("operation arguments serialize")
        );
        serde_json::to_string(&BodyDigest::of(&descriptor)).expect("operation digest serializes")
    }

    /// Preserve one class-call operation's lexical range and execution phase.
    fn class_call_history(
        class: &Class<'a>,
        call: &CallExpression<'a>,
        phase: &str,
    ) -> (u32, u32, HistoryAction) {
        (
            call.span.end,
            call.span.start,
            HistoryAction::Class {
                class: class.node_id.get(),
                site: call.node_id.get(),
                phase: phase.to_owned(),
            },
        )
    }

    /// Multiple reads in one class phase share one indexed execution graph.
    fn capture_phase_key(&self, mut id: oxc_semantic::NodeId) -> u32 {
        let span = self.ctx.semantic.nodes().get_node(id).kind().span();
        for _ in 0..MAX_DEPTH {
            let parent = self.ctx.semantic.nodes().parent_node(id);
            match parent.kind() {
                AstKind::MethodDefinition(item) => {
                    if item.computed && within(span, item.key.span()) {
                        return 1;
                    }
                    return if item.kind == MethodDefinitionKind::Constructor {
                        0
                    } else {
                        item.span.start
                    };
                }
                AstKind::PropertyDefinition(item) => {
                    return u32::from(
                        item.r#static || (item.computed && within(span, item.key.span())),
                    )
                }
                AstKind::StaticBlock(_) => return 1,
                AstKind::Class(_) => return 1,
                _ => {}
            }
            if parent.id() == id {
                break;
            }
            id = parent.id();
        }
        span.start
    }

    /// Follow class and instance aliases to the executions linked to this captured binding.
    fn capture_execution_sites(
        &self,
        class: &Class<'a>,
        mut id: oxc_semantic::NodeId,
    ) -> Vec<(u32, u32, HistoryAction)> {
        let mut phase = "construction".to_owned();
        let mut method = None;
        let mut is_static = false;
        for _ in 0..MAX_DEPTH {
            let parent = self.ctx.semantic.nodes().parent_node(id);
            match parent.kind() {
                AstKind::MethodDefinition(item) => {
                    if item.computed
                        && within(
                            self.ctx.semantic.nodes().get_node(id).kind().span(),
                            item.key.span(),
                        )
                    {
                        phase = "definition".to_owned();
                    } else if item.kind != MethodDefinitionKind::Constructor {
                        method = property_key_name(&item.key);
                        is_static = item.r#static;
                        phase = format!("method:{}", method.as_deref().unwrap_or("<dynamic>"));
                    }
                    break;
                }
                AstKind::PropertyDefinition(field) => {
                    if field.r#static {
                        phase = "definition".to_owned();
                    }
                    break;
                }
                AstKind::StaticBlock(_) => {
                    phase = "definition".to_owned();
                    break;
                }
                AstKind::Class(_) => break,
                _ => {}
            }
            if parent.id() == id {
                break;
            }
            id = parent.id();
        }
        if phase == "definition" {
            return vec![(
                class.span.end,
                class.span.start,
                HistoryAction::Class {
                    class: class.node_id.get(),
                    site: class.node_id.get(),
                    phase,
                },
            )];
        }
        let mut owners = Vec::new();
        if let Some(symbol) = class
            .id
            .as_ref()
            .and_then(|binding| binding.symbol_id.get())
        {
            owners.push(symbol);
        }
        let mut node = self.ctx.semantic.nodes().parent_node(class.node_id.get());
        for _ in 0..16 {
            match node.kind() {
                AstKind::VariableDeclarator(declaration) => {
                    if let BindingPattern::BindingIdentifier(binding) = &declaration.id {
                        if let Some(symbol) = binding.symbol_id.get() {
                            owners.push(symbol);
                        }
                    }
                    break;
                }
                AstKind::ParenthesizedExpression(_)
                | AstKind::TSAsExpression(_)
                | AstKind::TSSatisfiesExpression(_) => {
                    node = self.ctx.semantic.nodes().parent_node(node.id())
                }
                _ => break,
            }
        }
        let mut seen = HashSet::new();
        let mut operations = Vec::new();
        while let Some(owner) = owners.pop() {
            if !seen.insert(owner) {
                continue;
            }
            if seen.len() > 256 {
                operations.push((class.span.end, class.span.start, HistoryAction::Unknown));
                break;
            }
            for reference in self.ctx.semantic.scoping().get_resolved_references(owner) {
                let parent = self.ctx.semantic.nodes().parent_node(reference.node_id());
                match parent.kind() {
                    AstKind::VariableDeclarator(declaration) => {
                        Self::record_class_alias(&mut owners, &declaration.id);
                    }
                    AstKind::NewExpression(new) => {
                        if method.is_none() {
                            operations.push((
                                new.span.end,
                                new.span.start,
                                HistoryAction::Class {
                                    class: class.node_id.get(),
                                    site: new.node_id.get(),
                                    phase: "construction".to_owned(),
                                },
                            ));
                        } else if !is_static {
                            let owner_node =
                                self.ctx.semantic.nodes().parent_node(new.node_id.get());
                            if let AstKind::VariableDeclarator(declaration) = owner_node.kind() {
                                if let BindingPattern::BindingIdentifier(binding) = &declaration.id
                                {
                                    if let Some(instance) = binding.symbol_id.get() {
                                        for use_ in self
                                            .ctx
                                            .semantic
                                            .scoping()
                                            .get_resolved_references(instance)
                                        {
                                            let member = self
                                                .ctx
                                                .semantic
                                                .nodes()
                                                .parent_node(use_.node_id());
                                            if let AstKind::StaticMemberExpression(member_expr) =
                                                member.kind()
                                            {
                                                if method.as_deref()
                                                    != Some(member_expr.property.name.as_str())
                                                {
                                                    continue;
                                                }
                                                if let AstKind::CallExpression(call) = self
                                                    .ctx
                                                    .semantic
                                                    .nodes()
                                                    .parent_kind(member.id())
                                                {
                                                    operations.push(Self::class_call_history(
                                                        class, call, &phase,
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    AstKind::StaticMemberExpression(member)
                        if is_static
                            && method.as_deref() == Some(member.property.name.as_str()) =>
                    {
                        if let AstKind::CallExpression(call) =
                            self.ctx.semantic.nodes().parent_kind(parent.id())
                        {
                            operations.push(Self::class_call_history(class, call, &phase));
                        }
                    }
                    _ => {}
                }
            }
        }
        operations
    }

    /// Resolve only class objects owned by this callback, including immutable aliases.
    fn class_object<'c>(
        &mut self,
        object: &'c Expression<'a>,
        depth: u32,
    ) -> Option<&'c Class<'a>> {
        if depth > MAX_DEPTH || self.ctx.semantic.nodes().iter().len() == 0 {
            return None;
        }
        match object {
            Expression::Identifier(ident) => {
                let symbol = self.class_symbol(ident)?;
                if !is_local_symbol(self.ctx.semantic, self.ctx.body_span, symbol)
                    || !self.class_candidate(symbol, &mut HashSet::new(), depth + 1)
                {
                    return None;
                }
                let class = self.resolve_class_symbol(
                    symbol,
                    ident.span.start,
                    &mut HashSet::new(),
                    depth + 1,
                )?;
                self.class_external_mutations(symbol, class.span, ident.span.start);
                Some(class)
            }
            Expression::ClassExpression(class) => Some(class),
            Expression::StaticMemberExpression(member) if member.property.name == "constructor" => {
                let class = self.instance_class(&member.object, depth + 1)?;
                if self.intrinsic_instance_constructor(class, depth + 1) {
                    Some(class)
                } else {
                    self.unknown_class();
                    None
                }
            }
            Expression::ParenthesizedExpression(inner) => {
                self.class_object(&inner.expression, depth + 1)
            }
            _ => None,
        }
    }

    /// Find the class creating a directly owned instance without evaluating its environment.
    fn instance_class<'c>(
        &mut self,
        object: &'c Expression<'a>,
        depth: u32,
    ) -> Option<&'c Class<'a>> {
        if depth > MAX_DEPTH {
            return None;
        }
        match object {
            Expression::NewExpression(new) => self.resolve_class_expression(
                &new.callee,
                new.span.start,
                &mut HashSet::new(),
                depth + 1,
            ),
            Expression::Identifier(ident) => {
                let symbol = self.class_symbol(ident)?;
                let new = self.owned_instance_new(symbol, ident.span.start)?;
                self.resolve_class_expression(
                    &new.callee,
                    new.span.start,
                    &mut HashSet::new(),
                    depth + 1,
                )
            }
            Expression::ParenthesizedExpression(inner) => {
                self.instance_class(&inner.expression, depth + 1)
            }
            _ => None,
        }
    }

    /// A known constructor slot cannot be assumed when fields or dynamic keys can override it.
    fn intrinsic_instance_constructor(&mut self, class: &Class<'a>, depth: u32) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        for element in &class.body.body {
            let (key, computed, is_static, constructor) = match element {
                ClassElement::MethodDefinition(item) => (
                    &item.key,
                    item.computed,
                    item.r#static,
                    item.kind == MethodDefinitionKind::Constructor,
                ),
                ClassElement::PropertyDefinition(item) => {
                    (&item.key, item.computed, item.r#static, false)
                }
                ClassElement::AccessorProperty(item) => {
                    (&item.key, item.computed, item.r#static, false)
                }
                _ => continue,
            };
            if is_static || constructor {
                continue;
            }
            let name = property_key_name(key);
            if name.as_deref() == Some("constructor") || (computed && name.is_none()) {
                return false;
            }
        }
        if let Some(heritage) = &class.heritage {
            let Some(base) = self.resolve_class_expression(
                &heritage.expression,
                class.span.start,
                &mut HashSet::new(),
                depth + 1,
            ) else {
                return false;
            };
            return self.intrinsic_instance_constructor(base, depth + 1);
        }
        true
    }

    /// Resolve a proven callable and its original observable name through owned aliases.
    fn resolve_callable_symbol(
        &mut self,
        symbol: SymbolId,
        position: u32,
        seen: &mut HashSet<SymbolId>,
        depth: u32,
    ) -> Option<(FunctionLike<'a, 'a>, String, SymbolId)> {
        if depth > MAX_DEPTH || !seen.insert(symbol) || self.ctx.semantic.nodes().iter().len() == 0
        {
            return None;
        }
        let declaration = self
            .ctx
            .semantic
            .nodes()
            .get_node(self.ctx.semantic.scoping().symbol_declaration(symbol))
            .kind();
        if let Some(value) = self.value_at(symbol, position) {
            match value {
                Expression::Identifier(ident) => {
                    let target = self.class_symbol(ident)?;
                    self.resolve_callable_symbol(target, position, seen, depth + 1)
                }
                Expression::PrivateFieldExpression(field) => {
                    let (_, declaration) = self.private_declaration(field)?;
                    let name = self.private_callable_name(field)?;
                    let callable = match self.ctx.semantic.nodes().get_node(declaration).kind() {
                        AstKind::MethodDefinition(method)
                            if method.kind == MethodDefinitionKind::Method =>
                        {
                            FunctionLike::Function(&method.value)
                        }
                        AstKind::PropertyDefinition(property) => match property.value.as_ref()? {
                            Expression::ArrowFunctionExpression(arrow) => {
                                FunctionLike::Arrow(arrow)
                            }
                            Expression::FunctionExpression(function) => {
                                FunctionLike::Function(function)
                            }
                            _ => return None,
                        },
                        _ => return None,
                    };
                    Some((callable, name, symbol))
                }
                Expression::ArrowFunctionExpression(arrow) => Some((
                    FunctionLike::Arrow(arrow),
                    self.ctx.semantic.scoping().symbol_name(symbol).to_owned(),
                    symbol,
                )),
                Expression::FunctionExpression(function) => Some((
                    FunctionLike::Function(function),
                    function.id.as_ref().map_or_else(
                        || self.ctx.semantic.scoping().symbol_name(symbol).to_owned(),
                        |binding| binding.name.to_string(),
                    ),
                    symbol,
                )),
                _ => None,
            }
        } else if let AstKind::Function(function) = declaration {
            let previous_write = self
                .ctx
                .semantic
                .scoping()
                .get_resolved_references(symbol)
                .any(|reference| {
                    reference.is_write()
                        && self
                            .ctx
                            .semantic
                            .nodes()
                            .get_node(reference.node_id())
                            .kind()
                            .span()
                            .start
                            < position
                });
            if previous_write || function.body.is_none() {
                return None;
            }
            Some((
                FunctionLike::Function(function),
                function.id.as_ref()?.name.to_string(),
                symbol,
            ))
        } else {
            None
        }
    }

    /// Reject metadata writes or escapes through any alias while permitting ordinary invocation.
    fn callable_metadata_stable(&mut self, symbol: SymbolId) -> bool {
        let cached = self
            .ctx
            .class_cache
            .borrow()
            .callable_metadata
            .get(&symbol)
            .copied();
        if let Some(stable) = cached {
            return stable;
        }
        if self.ctx.class_cache.borrow().callable_metadata.len() >= 256 {
            return false;
        }
        let stable = self.inspect_callable_metadata(symbol);
        self.ctx
            .class_cache
            .borrow_mut()
            .callable_metadata
            .insert(symbol, stable);
        stable
    }

    /// Record a simple binding alias, retaining the caller's policy for destructuring.
    fn record_class_alias(owners: &mut Vec<SymbolId>, pattern: &BindingPattern<'a>) -> bool {
        if let BindingPattern::BindingIdentifier(binding) = pattern {
            if let Some(alias) = binding.symbol_id.get() {
                owners.push(alias);
            }
            true
        } else {
            false
        }
    }

    /// Inspect one alias graph once, including writes deferred within callable/class bodies.
    fn inspect_callable_metadata(&self, symbol: SymbolId) -> bool {
        let mut owners = vec![symbol];
        let mut seen = HashSet::new();
        while let Some(owner) = owners.pop() {
            if !seen.insert(owner) {
                continue;
            }
            if seen.len() > 256 {
                return false;
            }
            for reference in self.ctx.semantic.scoping().get_resolved_references(owner) {
                let node = self.ctx.semantic.nodes().get_node(reference.node_id());
                let parent = self.ctx.semantic.nodes().parent_node(node.id());
                match parent.kind() {
                    AstKind::VariableDeclarator(declaration) => {
                        if !Self::record_class_alias(&mut owners, &declaration.id) {
                            return false;
                        }
                    }
                    AstKind::CallExpression(call)
                        if call.callee.span() == node.kind().span()
                            || self.known_read_only_assertion(call) => {}
                    AstKind::StaticMemberExpression(_) | AstKind::ComputedMemberExpression(_) => {
                        let outer = self.ctx.semantic.nodes().parent_node(parent.id());
                        if matches!(
                            outer.kind(),
                            AstKind::AssignmentExpression(_)
                                | AstKind::UpdateExpression(_)
                                | AstKind::UnaryExpression(_)
                        ) || matches!(outer.kind(), AstKind::CallExpression(call) if !self.known_read_only_assertion(call))
                        {
                            return false;
                        }
                    }
                    AstKind::AssignmentExpression(_) if reference.is_write() => return false,
                    _ => return false,
                }
            }
        }
        true
    }

    /// Identify an indexed named receiver without imposing callable semantics on ordinary objects.
    fn callable_receiver(&self, object: &Expression<'a>) -> Option<(SymbolId, u32)> {
        if self.ctx.semantic.nodes().iter().len() == 0 {
            return None;
        }
        let Expression::Identifier(ident) = object else {
            return None;
        };
        Some((self.class_symbol(ident)?, ident.span.start))
    }

    /// Build one bounded private declaration/reference graph per lexical class.
    fn index_private_members(&mut self, class: &Class<'a>) {
        if self
            .ctx
            .class_cache
            .borrow()
            .private_members
            .contains_key(&class.span.start)
        {
            return;
        }
        let mut index = PrivateIndex::default();
        for element in &class.body.body {
            if let Some(PropertyKey::PrivateIdentifier(name)) = element.property_key() {
                let id = match element {
                    ClassElement::MethodDefinition(item) => item.node_id.get(),
                    ClassElement::PropertyDefinition(item) => item.node_id.get(),
                    ClassElement::AccessorProperty(item) => item.node_id.get(),
                    _ => continue,
                };
                index.bytes += name.name.len() + std::mem::size_of::<oxc_semantic::NodeId>();
                index.declarations.insert(name.name.to_string(), id);
                if index.bytes > 1024 * 1024 {
                    index.overflow = true;
                    break;
                }
            }
        }
        if !index.overflow {
            index.visit_class(class);
        }
        let mut cache = self.ctx.class_cache.borrow_mut();
        if cache.private_members.len() >= 256
            || cache.bytes.saturating_add(index.bytes) > 4 * 1024 * 1024
        {
            drop(cache);
            self.unknown_class();
            return;
        }
        cache.bytes += index.bytes;
        cache.private_members.insert(class.span.start, index);
    }

    /// Resolve lexical private declarations, including references to an outer private scope.
    fn private_declaration(
        &mut self,
        field: &PrivateFieldExpression<'a>,
    ) -> Option<(u32, oxc_semantic::NodeId)> {
        if self.ctx.semantic.nodes().iter().len() == 0 {
            self.unknown_class();
            return None;
        }
        let mut id = field.node_id.get();
        for _ in 0..MAX_DEPTH {
            let node = self.ctx.semantic.nodes().parent_node(id);
            if let AstKind::Class(class) = node.kind() {
                self.index_private_members(class);
                let cache = self.ctx.class_cache.borrow();
                let index = cache.private_members.get(&class.span.start)?;
                if index.overflow {
                    drop(cache);
                    self.unknown_class();
                    return None;
                }
                if let Some(declaration) = index.declarations.get(field.field.name.as_str()) {
                    return Some((class.span.start, *declaration));
                }
            }
            if node.id() == id {
                break;
            }
            id = node.id();
        }
        self.unknown_class();
        None
    }

    /// Any replacement, metadata mutation or escape invalidates initializer-derived names.
    fn private_metadata_stable(&mut self, class: u32, name: &str) -> bool {
        let key = (class, name.to_owned());
        if let Some(stable) = self
            .ctx
            .class_cache
            .borrow()
            .private_stability
            .get(&key)
            .copied()
        {
            return stable;
        }
        if self.ctx.class_cache.borrow().private_stability.len() >= 256 {
            return false;
        }
        let references = self
            .ctx
            .class_cache
            .borrow()
            .private_members
            .get(&class)
            .and_then(|index| index.references.get(name))
            .cloned()
            .unwrap_or_default();
        let stable = references.iter().all(|id| {
            let node = self.ctx.semantic.nodes().get_node(*id);
            let parent = self.ctx.semantic.nodes().parent_node(*id);
            match parent.kind() {
                AstKind::CallExpression(call) => {
                    call.callee.span() == node.kind().span() || self.known_read_only_assertion(call)
                }
                AstKind::StaticMemberExpression(_) | AstKind::ComputedMemberExpression(_) => {
                    let outer = self.ctx.semantic.nodes().parent_kind(parent.id());
                    let invocation = match parent.kind() {
                        AstKind::StaticMemberExpression(member) => {
                            matches!(member.property.name.as_str(), "call" | "apply")
                        }
                        AstKind::ComputedMemberExpression(member) => self
                            .callable_property_key(&member.expression, 0)
                            .is_some_and(|key| matches!(key.as_str(), "call" | "apply")),
                        _ => false,
                    };
                    match outer {
                        AstKind::CallExpression(call) => {
                            invocation && call.callee.span() == parent.kind().span()
                        }
                        AstKind::AssignmentExpression(_)
                        | AstKind::UpdateExpression(_)
                        | AstKind::UnaryExpression(_) => false,
                        _ => true,
                    }
                }
                AstKind::VariableDeclarator(declaration) => match &declaration.id {
                    BindingPattern::BindingIdentifier(binding) => binding
                        .symbol_id
                        .get()
                        .is_some_and(|alias| self.callable_metadata_stable(alias)),
                    _ => false,
                },
                _ => false,
            }
        });
        self.ctx
            .class_cache
            .borrow_mut()
            .private_stability
            .insert(key, stable);
        stable
    }

    /// Stable owned aliases of data initializers are ordinary properties, not callable reflection.
    fn private_data_initializer(&self, value: &Expression<'a>, depth: u32) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        match value {
            Expression::ObjectExpression(_)
            | Expression::ArrayExpression(_)
            | Expression::StringLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_) => true,
            Expression::Identifier(ident) => self.class_symbol(ident).is_some_and(|symbol| {
                !self.ctx.semantic.scoping().symbol_is_mutated(symbol)
                    && initializer(self.ctx.semantic, symbol)
                        .is_some_and(|value| self.private_data_initializer(value, depth + 1))
            }),
            _ => false,
        }
    }

    /// Distinguish callable private storage from ordinary private data before reflecting on it.
    fn private_callable_name(&mut self, field: &PrivateFieldExpression<'a>) -> Option<String> {
        let (class, declaration) = self.private_declaration(field)?;
        let node = self.ctx.semantic.nodes().get_node(declaration).kind();
        let value = match node {
            AstKind::MethodDefinition(method) if method.kind == MethodDefinitionKind::Method => {
                if !self.private_metadata_stable(class, field.field.name.as_str()) {
                    return Some(self.unknown_class());
                }
                return Some(format!("#{}", field.field.name));
            }
            AstKind::PropertyDefinition(property) => property.value.as_ref(),
            AstKind::AccessorProperty(property) => property.value.as_ref(),
            _ => return Some(self.unknown_class()),
        };
        let name = match value {
            Some(Expression::ArrowFunctionExpression(_)) => format!("#{}", field.field.name),
            Some(Expression::FunctionExpression(function)) => function.id.as_ref().map_or_else(
                || format!("#{}", field.field.name),
                |id| id.name.to_string(),
            ),
            Some(value) if self.private_data_initializer(value, 0) => return None,
            _ => return Some(self.unknown_class()),
        };
        if !self.private_metadata_stable(class, field.field.name.as_str()) {
            return Some(self.unknown_class());
        }
        Some(name)
    }

    /// An escaping or replaced known private callable cannot silently lose its observed identity.
    pub(super) fn private_callable_escape(&mut self, field: &PrivateFieldExpression<'a>) {
        let Some((class, declaration)) = self.private_declaration(field) else {
            return;
        };
        let callable = match self.ctx.semantic.nodes().get_node(declaration).kind() {
            AstKind::MethodDefinition(method) => method.kind == MethodDefinitionKind::Method,
            AstKind::PropertyDefinition(property) => matches!(
                property.value.as_ref(),
                Some(Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_))
            ),
            AstKind::AccessorProperty(property) => matches!(
                property.value.as_ref(),
                Some(Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_))
            ),
            _ => false,
        };
        if callable && !self.private_metadata_stable(class, field.field.name.as_str()) {
            self.unknown_class();
        }
    }

    /// Observe private callable names while leaving ordinary data-property reads unchanged.
    fn private_callable_reflection(
        &mut self,
        object: &Expression<'a>,
        property: &str,
    ) -> Option<String> {
        if !matches!(property, "name" | "toString") {
            return None;
        }
        let Expression::PrivateFieldExpression(field) = object else {
            return None;
        };
        let name = self.private_callable_name(field)?;
        Some(
            if property == "toString" || name == "<unresolved-class-state>" {
                self.unknown_class()
            } else {
                self.literal("string", &name)
            },
        )
    }

    /// Keep explicit private dependency uncertainty distinct from an ordinary object refusal.
    fn private_alias_uncertainty(&self, mut symbol: SymbolId) -> bool {
        if !self.ctx.class_uncertain.get() {
            return false;
        }
        let mut seen = HashSet::new();
        for _ in 0..MAX_DEPTH {
            if !seen.insert(symbol) {
                return false;
            }
            match initializer(self.ctx.semantic, symbol) {
                Some(Expression::PrivateFieldExpression(_)) => return true,
                Some(Expression::Identifier(ident)) => match self.class_symbol(ident) {
                    Some(next) => symbol = next,
                    None => return false,
                },
                _ => return false,
            }
        }
        false
    }

    /// Observe callable names only on demand; raw source reflection remains explicitly unknown.
    pub(super) fn callable_reflection(
        &mut self,
        object: &Expression<'a>,
        property: &str,
        depth: u32,
    ) -> Option<String> {
        if !matches!(property, "name" | "toString") {
            return None;
        }
        if let Some(value) = self.private_callable_reflection(object, property) {
            return Some(value);
        }
        let (symbol, position) = self.callable_receiver(object)?;
        let before = self.ctx.class_uncertain.get();
        let Some((_, name, owner)) =
            self.resolve_callable_symbol(symbol, position, &mut HashSet::new(), depth + 1)
        else {
            if self.private_alias_uncertainty(symbol) {
                return Some(self.unknown_class());
            }
            // A mutable ordinary object's `.name` is not callable reflection.
            let declaration = self
                .ctx
                .semantic
                .nodes()
                .get_node(self.ctx.semantic.scoping().symbol_declaration(symbol))
                .kind();
            if !matches!(declaration, AstKind::Function(_))
                && !matches!(
                    initializer(self.ctx.semantic, symbol),
                    Some(
                        Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
                    )
                )
            {
                self.ctx.class_uncertain.set(before);
                return None;
            }
            return Some(self.unknown_class());
        };
        if property == "toString"
            || name == "<unresolved-class-state>"
            || !self.callable_metadata_stable(owner)
        {
            return Some(self.unknown_class());
        }
        Some(self.literal("string", &name))
    }

    /// Resolve stable string keys without freezing mutable keys at class-definition time.
    fn callable_property_key(&self, key: &Expression<'a>, depth: u32) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        match key {
            Expression::StringLiteral(key) => Some(key.value.to_string()),
            Expression::Identifier(ident) => {
                let symbol = self.class_symbol(ident)?;
                if self.ctx.semantic.scoping().symbol_is_mutated(symbol) {
                    return None;
                }
                self.callable_property_key(initializer(self.ctx.semantic, symbol)?, depth + 1)
            }
            _ => None,
        }
    }

    /// Unknown computed callable properties can expose a name or source and cannot alpha-match.
    pub(super) fn computed_callable_reflection(
        &mut self,
        object: &Expression<'a>,
        key: &Expression<'a>,
        depth: u32,
    ) -> Option<String> {
        if let Expression::PrivateFieldExpression(field) = object {
            if self.private_callable_name(field).is_some() {
                return match self.callable_property_key(key, depth + 1) {
                    Some(key) => self.private_callable_reflection(object, &key),
                    None => Some(self.unknown_class()),
                };
            }
        }
        let (symbol, position) = self.callable_receiver(object)?;
        let before = self.ctx.class_uncertain.get();
        if self
            .resolve_callable_symbol(symbol, position, &mut HashSet::new(), depth + 1)
            .is_none()
        {
            if self.private_alias_uncertainty(symbol) {
                return Some(self.unknown_class());
            }
            self.ctx.class_uncertain.set(before);
            return None;
        }
        match self.callable_property_key(key, depth + 1) {
            Some(key) => self.callable_reflection(object, &key, depth + 1),
            None => Some(self.unknown_class()),
        }
    }

    /// Preserve known observed names as values while leaving dynamic reflective state unknown.
    pub(super) fn known_class_name(
        &mut self,
        object: &Expression<'a>,
        depth: u32,
    ) -> Option<String> {
        let class = self.class_object(object, depth + 1)?;
        for element in &class.body.body {
            let (key, computed, is_static, value) = match element {
                ClassElement::MethodDefinition(item) => {
                    (&item.key, item.computed, item.r#static, None)
                }
                ClassElement::PropertyDefinition(item) => {
                    (&item.key, item.computed, item.r#static, item.value.as_ref())
                }
                ClassElement::AccessorProperty(item) => {
                    (&item.key, item.computed, item.r#static, item.value.as_ref())
                }
                _ => continue,
            };
            if !is_static {
                continue;
            }
            let name = property_key_name(key);
            if computed && name.is_none() {
                return Some(self.unknown_class());
            }
            if name.as_deref() == Some("name") {
                return Some(match value {
                    Some(value) if super::super::registry::const_fold_tagged(value).is_some() => {
                        self.ser_expr(value, depth + 1)
                    }
                    _ => self.unknown_class(),
                });
            }
        }
        if let Some(binding) = &class.id {
            return Some(self.literal("string", binding.name.as_str()));
        }
        let mut parent = self.ctx.semantic.nodes().parent_node(class.node_id.get());
        for _ in 0..16 {
            match parent.kind() {
                AstKind::VariableDeclarator(declaration) => {
                    return Some(match &declaration.id {
                        BindingPattern::BindingIdentifier(binding) => {
                            self.literal("string", binding.name.as_str())
                        }
                        _ => self.unknown_class(),
                    });
                }
                AstKind::ParenthesizedExpression(_)
                | AstKind::TSAsExpression(_)
                | AstKind::TSSatisfiesExpression(_) => {
                    parent = self.ctx.semantic.nodes().parent_node(parent.id())
                }
                AstKind::NewExpression(_) | AstKind::StaticMemberExpression(_) => {
                    return Some(self.literal("string", ""))
                }
                _ => return Some(self.unknown_class()),
            }
        }
        Some(self.unknown_class())
    }

    /// Reflection exposing erased local names is explicitly unresolved, rather than alpha-equal.
    pub(super) fn class_reflection(&mut self, object: &Expression<'a>, property: &str, depth: u32) {
        if property == "constructor" {
            if let Some(class) = self.instance_class(object, depth + 1) {
                if self.intrinsic_instance_constructor(class, depth + 1) {
                    return;
                }
            }
        }
        if !matches!(property, "name" | "constructor" | "toString" | "prototype") {
            return;
        }
        let local = match object {
            Expression::Identifier(ident) => {
                self.class_reference(ident, depth + 1).is_some()
                    || (property == "constructor"
                        && self.class_instance_reference(ident, depth + 1).is_some())
            }
            Expression::NewExpression(new) if property == "constructor" => self
                .resolve_class_expression(
                    &new.callee,
                    new.span.start,
                    &mut HashSet::new(),
                    depth + 1,
                )
                .is_some(),
            _ => false,
        };
        if local {
            self.unknown_class();
        }
    }

    /// Cache normalized implementation separately from nominal identity and point-sensitive state.
    pub(super) fn ser_class(&mut self, class: &Class<'a>, depth: u32) -> String {
        if let Some(index) = self
            .ctx
            .class_regions
            .iter()
            .position(|span| *span == class.span)
        {
            return format!("class-self:{index}");
        }
        if depth > MAX_DEPTH
            || self.ctx.class_regions.len() >= 16
            || self.ctx.semantic.nodes().iter().len() == 0
        {
            return self.unknown_class();
        }
        let nominal = self.class_nominal(class);
        let Some(summary) = self.class_summary(class, depth + 1) else {
            return self.unknown_class();
        };
        let observed_name = if summary.escapes_this {
            let Some(binding) = &class.id else {
                return self.unknown_class();
            };
            format!(" name={}", self.literal("string", binding.name.as_str()))
        } else {
            String::new()
        };
        let position = self
            .ctx
            .class_uses
            .last()
            .copied()
            .unwrap_or(class.span.start);
        let environment: Vec<_> = summary
            .captures
            .iter()
            .map(|symbol| self.capture_value(*symbol, position, depth + 1))
            .collect();
        let environment = BodyDigest::of(
            &serde_json::to_string(&environment).expect("class environment serializes"),
        );
        format!(
            "(class C{nominal}{observed_name} body={} definition={} environment={})",
            summary.digest,
            serde_json::to_string(&BodyDigest::of(&summary.definition))
                .expect("definition digest serializes"),
            serde_json::to_string(&environment).expect("environment digest serializes")
        )
    }
    /// Compute or reuse the bounded body summary without expanding a captured environment.
    fn class_summary(&mut self, class: &Class<'a>, depth: u32) -> Option<ClassSummary> {
        let key = (class.span.start, self.mode == Mode::Structural);
        let cached = self.ctx.class_cache.borrow().bodies.get(&key).cloned();
        let summary = match cached {
            Some(summary) => summary,
            None => {
                if self.ctx.class_cache.borrow().bodies.len() >= 256 {
                    self.unknown_class();
                    return None;
                }
                let mut child = Serializer::new(self.ctx.fresh_for_pass(), self.mode);
                child.ctx.canonical.clear();
                child.ctx.next_local = 0;
                child.ctx.local_prefix = "K";
                child.ctx.expand_classes = true;
                child.ctx.class_regions.push(class.span);
                child.ctx.capture_frames.push(Vec::new());
                let body = child.ser_class_body(class, depth + 1);
                let captures = child.ctx.capture_frames.pop().expect("class capture frame");
                child.ctx.class_regions.pop();
                let definition = child
                    .class_definition_effects(class, depth + 1)
                    .unwrap_or_default();
                let mut escape = EscapedThis::default();
                escape.visit_class(class);
                let mut cache = self.ctx.class_cache.borrow_mut();
                if body.len() > 1024 * 1024
                    || cache.bytes.saturating_add(body.len()) > 4 * 1024 * 1024
                {
                    drop(cache);
                    self.unknown_class();
                    return None;
                }
                cache.bytes += body.len();
                let summary = ClassSummary {
                    digest: serde_json::to_string(&BodyDigest::of(&body))
                        .expect("class digest serializes"),
                    captures,
                    definition,
                    escapes_this: escape.found,
                };
                cache.bodies.insert(key, summary.clone());
                summary
            }
        };
        Some(summary)
    }

    /// Render class-owned execution without mistaking its this/arguments for callback bindings.
    fn class_inner(&mut self, render: impl FnOnce(&mut Self) -> String) -> String {
        let dependent = self.this_or_arguments;
        let outer = self.outermost_this_dependent;
        let value = render(self);
        self.this_or_arguments = dependent;
        self.outermost_this_dependent = outer;
        value
    }

    /// Preserve ordinary-field versus auto-accessor kind with shared ordered value rendering.
    fn class_field(&mut self, element: &ClassElement<'a>, depth: u32) -> String {
        let (kind, value, declared) = match element {
            ClassElement::PropertyDefinition(item) => ("field", item.value.as_ref(), item.declare),
            ClassElement::AccessorProperty(item) => ("accessor", item.value.as_ref(), false),
            _ => return self.unknown_class(),
        };
        if element.has_decorator() || declared {
            self.ctx.class_uncertain.set(true);
        }
        let key = self.ser_property_key(element.property_key().expect("field key"), depth + 1);
        let value = self.class_inner(|this| {
            value.map_or_else(
                || "undefined".to_owned(),
                |value| this.ser_expr(value, depth + 1),
            )
        });
        format!(
            "({kind} static={} computed={} key={key} value={value})",
            element.r#static(),
            element.computed(),
        )
    }

    /// Computed keys execute before static values; instance values remain deferred.
    fn class_field_effects(
        &mut self,
        kind: &str,
        key: &PropertyKey<'a>,
        value: Option<&Expression<'a>>,
        flags: (bool, bool),
        depth: u32,
    ) -> Vec<String> {
        let mut effects = Vec::new();
        if flags.1 {
            effects.push(format!("key:{}", self.ser_property_key(key, depth + 1)));
        }
        if flags.0 {
            effects.push(self.class_inner(|this| {
                format!(
                    "{kind}:{}={}",
                    this.ser_property_key(key, depth + 1),
                    value.map_or_else(
                        || "undefined".to_owned(),
                        |value| this.ser_expr(value, depth + 1)
                    )
                )
            }));
        }
        effects
    }

    /// Encode every supported runtime class dimension in source order, with scoped private names.
    fn ser_class_body(&mut self, class: &Class<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH || self.ctx.class_regions.len() >= 16 {
            self.ctx.class_uncertain.set(true);
            return "<unresolved-class>".to_owned();
        }
        if !class.decorators.is_empty() || class.declare || class.r#abstract {
            self.ctx.class_uncertain.set(true);
        }
        let heritage = class.heritage.as_ref().map_or_else(
            || "none".to_owned(),
            |heritage| self.ser_expr(&heritage.expression, depth + 1),
        );
        let mut private = HashMap::new();
        for element in &class.body.body {
            let key = match element {
                ClassElement::MethodDefinition(item) => Some(&item.key),
                ClassElement::PropertyDefinition(item) => Some(&item.key),
                ClassElement::AccessorProperty(item) => Some(&item.key),
                _ => None,
            };
            if let Some(PropertyKey::PrivateIdentifier(name)) = key {
                let next = private.len();
                private
                    .entry(name.name.to_string())
                    .or_insert_with(|| format!("P{next}"));
            }
        }

        self.ctx.private_names.push(private);
        let mut elements = Vec::new();
        for element in &class.body.body {
            let rendered = match element {
                ClassElement::MethodDefinition(item) => {
                    if !item.decorators.is_empty()
                        || item.value.params.items.iter().any(|p| {
                            !p.decorators.is_empty()
                                || ((p.accessibility.is_some() || p.readonly || p.r#override)
                                    && !matches!(p.pattern, BindingPattern::BindingIdentifier(_)))
                        })
                    {
                        self.ctx.class_uncertain.set(true);
                    }
                    let key = self.ser_property_key(&item.key, depth + 1);
                    let parameter_properties: Vec<_> = item
                        .value
                        .params
                        .items
                        .iter()
                        .enumerate()
                        .filter_map(|(index, parameter)| {
                            if parameter.accessibility.is_none()
                                && !parameter.readonly
                                && !parameter.r#override
                            {
                                return None;
                            }
                            let BindingPattern::BindingIdentifier(binding) = &parameter.pattern
                            else {
                                return Some(self.unknown_class());
                            };
                            Some(format!(
                                "(parameter-property index={index} key={})",
                                binding.name
                            ))
                        })
                        .collect();
                    let body = self.class_inner(|this| {
                        this.ser_callable(&FunctionLike::Function(&item.value), depth + 1)
                    });
                    format!(
                        "(method kind={:?} static={} computed={} key={key} optional={} {} {body})",
                        item.kind,
                        item.r#static,
                        item.computed,
                        item.optional,
                        parameter_properties.join(" ")
                    )
                }
                ClassElement::PropertyDefinition(_) | ClassElement::AccessorProperty(_) => {
                    self.class_field(element, depth)
                }
                ClassElement::StaticBlock(item) => self.class_inner(|this| {
                    format!("(static-block {})", this.ser_stmts(&item.body, depth + 1))
                }),
                ClassElement::TSIndexSignature(_) => {
                    self.ctx.class_uncertain.set(true);
                    "<unresolved-class-element>".to_owned()
                }
            };
            elements.push(rendered);
        }
        self.ctx.private_names.pop();

        format!("(class extends={heritage} {})", elements.join(" "))
    }

    /// Definition-time effects are ordered independently from deferred instance initialization.
    pub(super) fn class_definition_effects(
        &mut self,
        class: &Class<'a>,
        depth: u32,
    ) -> Option<String> {
        self.ctx.class_regions.push(class.span);
        self.ctx.capture_frames.push(Vec::new());
        let mut effects = Vec::new();
        if let Some(heritage) = &class.heritage {
            effects.push(format!(
                "heritage:{}",
                self.ser_expr(&heritage.expression, depth + 1)
            ));
        }
        for element in &class.body.body {
            match element {
                ClassElement::StaticBlock(item) => effects.push(self.class_inner(|this| {
                    format!("static-block:{}", this.ser_stmts(&item.body, depth + 1))
                })),
                ClassElement::MethodDefinition(item) if item.computed => effects.push(format!(
                    "key:{}",
                    self.ser_property_key(&item.key, depth + 1)
                )),
                ClassElement::PropertyDefinition(item) => effects.extend(self.class_field_effects(
                    "static-field",
                    &item.key,
                    item.value.as_ref(),
                    (item.r#static, item.computed),
                    depth,
                )),
                ClassElement::AccessorProperty(item) => effects.extend(self.class_field_effects(
                    "static-accessor",
                    &item.key,
                    item.value.as_ref(),
                    (item.r#static, item.computed),
                    depth,
                )),
                _ => {}
            }
        }
        let captures = self
            .ctx
            .capture_frames
            .pop()
            .expect("definition capture frame");
        self.ctx.class_regions.pop();
        let environment: Vec<_> = captures
            .iter()
            .map(|symbol| self.capture_value(*symbol, class.span.start, depth + 1))
            .collect();
        (!effects.is_empty()).then(|| {
            format!(
                "class-eval:{} environment={}",
                serde_json::to_string(&effects).expect("class effects serialize"),
                serde_json::to_string(&environment).expect("definition environment serializes")
            )
        })
    }
}
