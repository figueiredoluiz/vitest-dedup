//! Owned callable resolution and the metadata that keeps reflections distinct.
use super::*;
use crate::lang::js_text::string_text;

impl<'a, 'b, 's, 'm> Serializer<'a, 'b, 's, 'm> {
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
    pub(super) fn owned_instance_new(
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

    /// Recognize Vitest's fresh mock by resolved import ownership, not local spelling.
    pub(super) fn owned_mock(&self, expression: &Expression<'a>) -> bool {
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
    pub(super) fn owned_closure(
        &mut self,
        expression: &Expression<'a>,
        position: u32,
        depth: u32,
    ) -> String {
        let callable = match expression {
            Expression::ArrowFunctionExpression(arrow) => FunctionLike::Arrow(arrow),
            Expression::FunctionExpression(function) => FunctionLike::Function(function),
            _ => return self.unknown_class(),
        };
        self.owned_callable(&callable, position, depth + 1)
    }

    /// Share bounded lexical environments across declared and expression callables.
    pub(super) fn owned_callable(
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
    pub(super) fn known_read_only_assertion(&self, call: &CallExpression<'a>) -> bool {
        let Expression::Identifier(ident) = &call.callee else {
            return false;
        };
        self.ctx
            .import_origin_of(ident)
            .is_some_and(|origin| origin.specifier == "vitest" && origin.exported == "expect")
    }

    /// Encode one proven class operation with the target state left as an owned symbolic input.
    pub(super) fn class_operation(
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
    pub(super) fn class_call_history(
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

    /// A known constructor slot cannot be assumed when fields or dynamic keys can override it.
    pub(super) fn intrinsic_instance_constructor(&mut self, class: &Class<'a>, depth: u32) -> bool {
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
    pub(super) fn resolve_callable_symbol(
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
    pub(super) fn callable_metadata_stable(&mut self, symbol: SymbolId) -> bool {
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

    /// Observe callable names only on demand; raw source reflection remains explicitly unknown.
    pub(in crate::frontend::fingerprint) fn callable_reflection(
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
    pub(super) fn callable_property_key(&self, key: &Expression<'a>, depth: u32) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        match key {
            Expression::StringLiteral(key) => Some(string_text(key).into_owned()),
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
    pub(in crate::frontend::fingerprint) fn computed_callable_reflection(
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
}
