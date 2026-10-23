//! Nominal class identity, aliases, and instance-name resolution.
use super::*;
use crate::frontend::extract::registration_free::binding_may_differ_from_declaration;

impl<'a, 'b, 's, 'm> Serializer<'a, 'b, 's, 'm> {
    /// Resolve one reference through the semantic graph, independent of local spelling.
    pub(super) fn class_symbol(&self, ident: &IdentifierReference<'a>) -> Option<SymbolId> {
        ident
            .reference_id
            .get()
            .and_then(|id| self.ctx.semantic.scoping().get_reference(id).symbol_id())
    }

    /// Reuse one nominal slot without conflating separately declared classes.
    pub(super) fn class_nominal(&self, class: &Class<'a>) -> usize {
        let mut cache = self.ctx.class_cache.borrow_mut();
        let next = cache.nominals.len();
        *cache.nominals.entry(class.span.start).or_insert(next)
    }

    /// Reuse the per-file semantic parent index; never search all nodes for containment.
    pub(super) fn enclosing_class(&self, mut id: oxc_semantic::NodeId) -> Option<&'a Class<'a>> {
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
    pub(super) fn class_candidate(
        &self,
        symbol: SymbolId,
        seen: &mut HashSet<SymbolId>,
        depth: u32,
    ) -> bool {
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
    pub(in crate::frontend::fingerprint) fn class_reference(
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
            // A redeclared binding runs the same refusal as a reassigned one: the first
            // declaration is not necessarily the class the reference observes.
            if binding_may_differ_from_declaration(self.ctx.semantic, symbol) {
                self.unknown_class();
            }
            return Some(class);
        }
        let init = self.value_at(symbol, position)?;
        self.resolve_class_expression(init, position, seen, depth + 1)
    }

    /// Unwrap syntax-only expressions while retaining the owning declaration.
    pub(super) fn resolve_class_expression<'c>(
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

    /// A whole instance passed to application code can expose its nominal class name.
    pub(in crate::frontend::fingerprint) fn instance_name_observed(
        &self,
        ident: &IdentifierReference<'a>,
    ) -> bool {
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
    pub(in crate::frontend::fingerprint) fn class_instance_reference(
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

    /// Record a simple binding alias, retaining the caller's policy for destructuring.
    pub(super) fn record_class_alias(
        owners: &mut Vec<SymbolId>,
        pattern: &BindingPattern<'a>,
    ) -> bool {
        if let BindingPattern::BindingIdentifier(binding) = pattern {
            if let Some(alias) = binding.symbol_id.get() {
                owners.push(alias);
            }
            true
        } else {
            false
        }
    }

    /// Preserve known observed names as values while leaving dynamic reflective state unknown.
    pub(in crate::frontend::fingerprint) fn known_class_name(
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
                    Some(value)
                        if crate::frontend::registry::const_fold_tagged(value).is_some() =>
                    {
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
    pub(in crate::frontend::fingerprint) fn class_reflection(
        &mut self,
        object: &Expression<'a>,
        property: &str,
        depth: u32,
    ) {
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
}
