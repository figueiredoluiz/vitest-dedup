//! Point-sensitive capture values, selected literals, and owned mutation histories.
use super::*;

/// One indexed operation; parser handles remain confined to this temporary frontend cache.
#[derive(Clone)]
pub(super) enum HistoryAction {
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
pub(super) struct HistoryIndex {
    actions: Vec<(u32, u32, HistoryAction)>,
    prefixes: Vec<(BodyDigest, bool)>,
}

/// Read an initializer only when the resolved declaration binds this whole value.
/// A destructured child shares its declarator with the other children, but does not
/// receive the declarator's entire initializer as its runtime value.
pub(super) fn initializer<'a>(
    semantic: &Semantic<'a>,
    symbol: SymbolId,
) -> Option<&'a Expression<'a>> {
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
/// Resolve only a const object destructure of a fresh, primitive-valued literal.
/// This carries the selected key and value without treating the whole object as the child.
pub(super) fn selected_literal<'a>(
    semantic: &Semantic<'a>,
    symbol: SymbolId,
) -> Option<(String, &'a Expression<'a>)> {
    let nodes = semantic.nodes();
    if nodes.iter().len() == 0 {
        return None;
    }
    let declaration = nodes.get_node(semantic.scoping().symbol_declaration(symbol));
    let declarator = match declaration.kind() {
        AstKind::VariableDeclarator(declarator) => {
            let AstKind::VariableDeclaration(parent) = nodes.parent_kind(declaration.id()) else {
                return None;
            };
            (parent.kind == VariableDeclarationKind::Const).then_some(declarator)?
        }
        AstKind::VariableDeclaration(parent) if parent.kind == VariableDeclarationKind::Const => {
            parent.declarations.iter().find(|item| match &item.id {
                BindingPattern::ObjectPattern(object) => object.properties.iter().any(|property| {
                    matches!(&property.value, BindingPattern::BindingIdentifier(binding)
                        if binding.symbol_id.get() == Some(symbol))
                }),
                _ => false,
            })?
        }
        _ => return None,
    };
    let BindingPattern::ObjectPattern(pattern) = &declarator.id else {
        return None;
    };
    if pattern.rest.is_some()
        || pattern.properties.iter().any(|property| {
            property.computed || !matches!(property.value, BindingPattern::BindingIdentifier(_))
        })
    {
        return None;
    }
    let selected = pattern
        .properties
        .iter()
        .find_map(|property| match &property.value {
            BindingPattern::BindingIdentifier(binding)
                if binding.symbol_id.get() == Some(symbol) =>
            {
                property_key_name(&property.key)
            }
            _ => None,
        })?;
    let Expression::ObjectExpression(object) = declarator.init.as_ref()? else {
        return None;
    };
    let mut keys = HashSet::new();
    let mut value = None;
    for property in &object.properties {
        let ObjectPropertyKind::ObjectProperty(property) = property else {
            return None;
        };
        if property.kind != PropertyKind::Init || property.computed || property.method {
            return None;
        }
        let key = property_key_name(&property.key)?;
        if key == "__proto__"
            || !keys.insert(key.clone())
            || !matches!(
                property.value,
                Expression::BooleanLiteral(_)
                    | Expression::NullLiteral(_)
                    | Expression::NumericLiteral(_)
                    | Expression::BigIntLiteral(_)
                    | Expression::StringLiteral(_)
            )
        {
            return None;
        }
        if key == selected {
            value = Some(&property.value);
        }
    }
    Some((selected, value?))
}

impl<'a, 'b, 's, 'm> Serializer<'a, 'b, 's, 'm> {
    /// Return one explicit unknown marker and poison only this callback's comparison context.
    pub(super) fn unknown_class(&self) -> String {
        self.ctx.class_uncertain.set(true);
        "<unresolved-class-state>".to_owned()
    }

    /// Resolve a binding at the current use, honoring proven straight-line overwrites.
    pub(super) fn value_at(
        &mut self,
        symbol: SymbolId,
        position: u32,
    ) -> Option<&'a Expression<'a>> {
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
    pub(super) fn straight_line(&self, mut id: oxc_semantic::NodeId) -> bool {
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

    /// Unknown escapes and writes to a class object invalidate its modeled implementation.
    pub(super) fn class_external_mutations(
        &mut self,
        symbol: SymbolId,
        class: Span,
        position: u32,
    ) {
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

    /// Captures are placeholders in the cached body, with values supplied separately at use.
    pub(in crate::frontend::fingerprint) fn class_capture(
        &mut self,
        ident: &IdentifierReference<'a>,
        _depth: u32,
    ) -> Option<String> {
        self.capture_slot(self.class_symbol(ident)?)
    }

    /// Register a lexical capture in its enclosing summary before rendering its environment.
    pub(super) fn capture_slot(&mut self, symbol: SymbolId) -> Option<String> {
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
    pub(super) fn capture_value(&mut self, symbol: SymbolId, position: u32, depth: u32) -> String {
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
            if let Some((property, value)) = selected_literal(self.ctx.semantic, symbol) {
                return format!(
                    "(selected {} {})",
                    serde_json::to_string(&property).expect("property key serializes"),
                    self.ser_expr(value, depth + 1)
                );
            }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_ast::ast::BindingPattern;
    use oxc_semantic::SemanticBuilder;
    use std::path::Path;

    /// `selected_literal` must refuse a semantic built without nodes exactly as `initializer`
    /// and `value_at` do: `AstNodes::get_node` indexes a vector, so on an empty node table it
    /// panics rather than degrading to an unresolved capture.
    ///
    /// Every production `SemanticBuilder` sets `with_build_nodes(true)`; this pins the
    /// contract for node-less callers -- this crate's own harnesses build such semantics, and
    /// a future diagnostic path must keep costing one unresolved capture, not a panic.
    #[test]
    fn selected_literal_refuses_a_node_less_semantic_instead_of_panicking() {
        let allocator = Allocator::default();
        let source = "const { chosen } = { chosen: 1 };";
        let source_type = SourceType::from_path(Path::new("case.test.ts")).unwrap();
        let parsed = Parser::new(&allocator, source, source_type).parse();
        assert!(!parsed.fatal_error, "fixture failed to parse: {source}");
        let program = &parsed.program;
        let semantic = SemanticBuilder::new().build(program).semantic;
        assert_eq!(
            semantic.nodes().iter().len(),
            0,
            "precondition: the semantic was built without nodes"
        );

        let Statement::VariableDeclaration(declaration) = &program.body[0] else {
            panic!("expected a variable declaration");
        };
        let BindingPattern::ObjectPattern(pattern) = &declaration.declarations[0].id else {
            panic!("expected an object destructuring pattern");
        };
        let BindingPattern::BindingIdentifier(binding) = &pattern.properties[0].value else {
            panic!("expected a plain binding property");
        };
        let symbol = binding
            .symbol_id
            .get()
            .expect("scoping binds the destructured name");

        assert!(
            selected_literal(&semantic, symbol).is_none(),
            "a node-less semantic must refuse the selected literal"
        );
        assert!(
            initializer(&semantic, symbol).is_none(),
            "...and the guarded sibling keeps the same refusal contract"
        );
    }
}
