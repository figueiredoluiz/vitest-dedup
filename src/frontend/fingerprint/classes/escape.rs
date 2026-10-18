//! Private-name indexing and `this`-escape detection for local class summaries.
use super::*;

/// A class whose receiver leaves a method can expose its runtime name indirectly.
#[derive(Default)]
pub(super) struct EscapedThis {
    pub(super) found: bool,
    pub(super) depth: u32,
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

/// One bounded lexical index; nested declarations shadow only their own private names.
#[derive(Default)]
pub(super) struct PrivateIndex {
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

impl<'a, 'b, 's, 'm> Serializer<'a, 'b, 's, 'm> {
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
    pub(super) fn private_declaration(
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
    pub(super) fn private_callable_name(
        &mut self,
        field: &PrivateFieldExpression<'a>,
    ) -> Option<String> {
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
    pub(in crate::frontend::fingerprint) fn private_callable_escape(
        &mut self,
        field: &PrivateFieldExpression<'a>,
    ) {
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
    pub(super) fn private_callable_reflection(
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
    pub(super) fn private_alias_uncertainty(&self, mut symbol: SymbolId) -> bool {
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
}
