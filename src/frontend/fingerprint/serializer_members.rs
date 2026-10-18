use super::*;

impl<'a, 'b, 's, 'm> Serializer<'a, 'b, 's, 'm> {
    /// Render a named member access with the receiver and optional-chain distinction.
    pub(super) fn ser_static_member(
        &mut self,
        member: &StaticMemberExpression<'a>,
        depth: u32,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(member.span);
        }
        if let Some(value) =
            self.callable_reflection(&member.object, member.property.name.as_str(), depth + 1)
        {
            return value;
        }
        if member.property.name == "name" {
            if let Some(value) = self.known_class_name(&member.object, depth + 1) {
                return value;
            }
        }
        self.class_reflection(&member.object, member.property.name.as_str(), depth + 1);
        let object = self.ser_expr(&member.object, depth + 1);
        let optional = if member.optional { "?." } else { "." };
        format!(
            "(member {object} {optional}{})",
            member.property.name.as_str()
        )
    }

    /// Render an indexed access, preserving both its key expression and optional-chain flag.
    pub(super) fn ser_computed_member(
        &mut self,
        member: &ComputedMemberExpression<'a>,
        depth: u32,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(member.span);
        }
        if let Some(value) =
            self.computed_callable_reflection(&member.object, &member.expression, depth + 1)
        {
            return value;
        }
        let object = self.ser_expr(&member.object, depth + 1);
        let property = self.ser_expr(&member.expression, depth + 1);
        // `?.[k]` is not `[k]`: the first yields `undefined` on a nullish receiver, the second
        // throws. Two tests differing only in that guard are different tests, and without this
        // they serialized identically and were reported as `duplicate-test` at ERROR severity.
        //
        // Both siblings already recorded it -- `ser_static_member` emits `.b` against `?.b`, and
        // `ser_call` emits `(call` against `(call?.` -- so this was the one member form that did
        // not, which is the "fixed at one site, not its siblings" shape this repository keeps
        // producing rather than a decision anyone made.
        let optional = if member.optional { "?" } else { "" };
        format!("(index{optional} {object} {property})")
    }

    /// Render a private-field access with its receiver and private member name.
    pub(super) fn ser_private_field(
        &mut self,
        member: &PrivateFieldExpression<'a>,
        depth: u32,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(member.span);
        }
        self.private_callable_escape(member);
        let object = self.ser_expr(&member.object, depth + 1);
        let name = self.private_name(member.field.name.as_str());
        format!("(member {object} .#{name})")
    }

    /// A JSX element, structurally: name, attributes in source order, children in source order.
    ///
    /// Why structural and not the verbatim slice [`Serializer::opaque`] used to emit here: JSX in a
    /// test body is an argument to `render(...)`/`mount(...)`, and what the test observes is the
    /// RENDERED element -- which a formatter's reindenting does not change, and which an attribute
    /// literal participates in as a *value*. The slice made the comparison textual (reindent broke
    /// every match) and hid attribute literals from `structural`'s kind-collapse (the
    /// `parameterization-candidate` family could never see them). Every sub-part reuses the
    /// serializer that already exists for it: component names go through the same
    /// reference-identity machinery plain identifiers do (so two different components imported
    /// from one module stay distinct, and the same component resolved from two files can match),
    /// attribute literals through [`Serializer::literal`] (so alpha keeps the value and structural
    /// keeps the kind), and expression children/attribute values through [`Serializer::ser_expr`].
    ///
    /// `self_closing` is deliberately NOT recorded: `<Button />` and `<Button></Button>` render
    /// identically, which is the relation this serializer claims.
    ///
    /// Kept out of [`Serializer::ser_expr`]'s match for the stack-headroom reason
    /// [`Serializer::ser_new`]'s doc records, like every other wide arm.
    pub(super) fn ser_jsx_element(&mut self, element: &JSXElement<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(element.span);
        }
        let name = self.ser_jsx_name(&element.opening_element.name, depth + 1);
        let type_arguments =
            self.ser_type_arguments(element.opening_element.type_arguments.as_deref());
        let attributes: Vec<String> = element
            .opening_element
            .attributes
            .iter()
            .map(|attribute| self.ser_jsx_attribute_item(attribute, depth + 1))
            .collect();
        let children = self.ser_jsx_children(&element.children, depth + 1);
        let mut parts = Vec::with_capacity(2 + attributes.len());
        parts.push(format!("{name}{type_arguments}"));
        parts.extend(attributes);
        parts.push(children);
        format!("(jsx-elem {})", parts.join(" "))
    }

    /// A JSX fragment: children only, since fragments carry no name or attributes.
    pub(super) fn ser_jsx_fragment(&mut self, fragment: &JSXFragment<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(fragment.span);
        }
        format!(
            "(jsx-frag {})",
            self.ser_jsx_children(&fragment.children, depth + 1)
        )
    }

    /// An element name. The two identifier variants are disjoint by JSX's own grammar (a lowercase
    /// tag is an intrinsic element, a capitalized one a component reference), and are kept distinct
    /// by prefix so `<div>` can never collide with a local binding spelled the same way.
    fn ser_jsx_name(&mut self, name: &JSXElementName<'a>, depth: u32) -> String {
        match name {
            JSXElementName::Identifier(identifier) => {
                format!("intrinsic:{}", identifier.name.as_str())
            }
            JSXElementName::IdentifierReference(ident) => {
                format!("component:{}", self.ctx.reference(ident))
            }
            JSXElementName::NamespacedName(namespaced) => format!(
                "jsx-ns:{}:{}",
                namespaced.namespace.name.as_str(),
                namespaced.name.name.as_str()
            ),
            JSXElementName::MemberExpression(member) => {
                let object = self.ser_jsx_member_object(&member.object, depth + 1);
                format!("(jsx-member {object} .{})", member.property.name.as_str())
            }
            JSXElementName::ThisExpression(_) => {
                // `<this.foo />` reads the enclosing function's `this`, exactly as `this.foo`
                // outside JSX would -- see [`Serializer::this_or_arguments`].
                self.this_or_arguments = true;
                "this".to_owned()
            }
        }
    }

    /// Render nested JSX member objects using component reference identities.
    fn ser_jsx_member_object(
        &mut self,
        object: &JSXMemberExpressionObject<'a>,
        depth: u32,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(object.span());
        }
        match object {
            // `component:` for the same reason [`Serializer::ser_jsx_name`]'s identifier-reference
            // arm carries it: this identifier is a component reference, and the prefix keeps it
            // from colliding with the same name read as an intrinsic.
            JSXMemberExpressionObject::IdentifierReference(ident) => {
                format!("component:{}", self.ctx.reference(ident))
            }
            JSXMemberExpressionObject::MemberExpression(member) => {
                let object = self.ser_jsx_member_object(&member.object, depth + 1);
                format!("(jsx-member {object} .{})", member.property.name.as_str())
            }
            JSXMemberExpressionObject::ThisExpression(_) => {
                self.this_or_arguments = true;
                "this".to_owned()
            }
        }
    }

    /// One attribute: `(jsx-attr <name> <value>)`, or `(jsx-attr <name>)` for the valueless
    /// boolean form (`<button disabled>`), or a spread. Attribute names are JSX identifiers --
    /// identifier characters plus `-` -- so a name can never contain the spaces and parentheses
    /// of the value's serialization and the two cannot run together.
    fn ser_jsx_attribute_item(&mut self, attribute: &JSXAttributeItem<'a>, depth: u32) -> String {
        match attribute {
            JSXAttributeItem::Attribute(attribute) => {
                let name = match &attribute.name {
                    JSXAttributeName::Identifier(identifier) => identifier.name.as_str().to_owned(),
                    JSXAttributeName::NamespacedName(namespaced) => format!(
                        "{}:{}",
                        namespaced.namespace.name.as_str(),
                        namespaced.name.name.as_str()
                    ),
                };
                match &attribute.value {
                    None => format!("(jsx-attr {name})"),
                    Some(value) => format!(
                        "(jsx-attr {name} {})",
                        self.ser_jsx_attribute_value(value, depth + 1)
                    ),
                }
            }
            JSXAttributeItem::SpreadAttribute(spread) => format!(
                "(jsx-spread-attr {})",
                self.ser_expr(&spread.argument, depth + 1)
            ),
        }
    }

    /// Serialize JSX string, expression, element and fragment attribute values in the current
    /// fingerprint mode.
    fn ser_jsx_attribute_value(&mut self, value: &JSXAttributeValue<'a>, depth: u32) -> String {
        match value {
            // A plain string attribute is where the `parameterization-candidate` family was
            // blind: routed through `literal`, so `alpha` keeps the value (`variant="primary"` vs
            // `variant="secondary"` differ) while `structural` collapses to the kind.
            JSXAttributeValue::StringLiteral(literal) => {
                self.literal("string", literal.value.as_str())
            }
            JSXAttributeValue::ExpressionContainer(container) => {
                match container.expression.as_expression() {
                    Some(expression) => self.ser_expr(expression, depth + 1),
                    // `attr={}` is not a value a renderer can observe differently from no value.
                    None => "(jsx-empty)".to_owned(),
                }
            }
            JSXAttributeValue::Element(element) => self.ser_jsx_element(element, depth + 1),
            JSXAttributeValue::Fragment(fragment) => self.ser_jsx_fragment(fragment, depth + 1),
        }
    }

    /// Children, space-joined. Whitespace-only text children contribute NOTHING rather than a
    /// normalized-empty string, because JSX itself drops them: the whitespace between a tag and
    /// its first nested element is not a child any renderer observes.
    fn ser_jsx_children(&mut self, children: &[JSXChild<'a>], depth: u32) -> String {
        let parts: Vec<String> = children
            .iter()
            .filter_map(|child| self.ser_jsx_child(child, depth + 1))
            .collect();
        parts.join(" ")
    }

    /// Render a meaningful JSX child, declining empty text and empty expression containers.
    fn ser_jsx_child(&mut self, child: &JSXChild<'a>, depth: u32) -> Option<String> {
        match child {
            JSXChild::Text(text) => {
                let normalized = normalize_jsx_text(text.value.as_str());
                (!normalized.is_empty())
                    .then(|| format!("(jsx-text {})", self.literal("string", &normalized)))
            }
            JSXChild::Element(element) => Some(self.ser_jsx_element(element, depth + 1)),
            JSXChild::Fragment(fragment) => Some(self.ser_jsx_fragment(fragment, depth + 1)),
            // An empty expression container `{}` renders nothing, so it is not a child.
            JSXChild::ExpressionContainer(container) => container
                .expression
                .as_expression()
                .map(|expression| format!("(jsx-expr {})", self.ser_expr(expression, depth + 1))),
            JSXChild::Spread(spread) => Some(format!(
                "(jsx-spread-children {})",
                self.ser_expr(&spread.expression, depth + 1)
            )),
        }
    }

    /// An optional-chain fragment: the `?.`-linked element list of `a?.b.c()`.
    pub(super) fn ser_chain(&mut self, chain: &ChainExpression<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(chain.span);
        }
        match &chain.expression {
            ChainElement::CallExpression(call) => self.ser_call(call, depth + 1),
            ChainElement::TSNonNullExpression(non_null) => {
                self.ser_expr(&non_null.expression, depth + 1)
            }
            ChainElement::StaticMemberExpression(member) => {
                self.ser_static_member(member, depth + 1)
            }
            ChainElement::ComputedMemberExpression(member) => {
                self.ser_computed_member(member, depth + 1)
            }
            ChainElement::PrivateFieldExpression(member) => {
                self.ser_private_field(member, depth + 1)
            }
        }
    }

    /// `(call callee args...)`, with an imported-helper call site rendered as the helper's own
    /// body instead when `try_inline_imported_helper` admits it (`alpha` only).
    pub(super) fn ser_call(&mut self, call: &CallExpression<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(call.span);
        }
        if let Some(inlined) = self.try_inline_imported_helper(call, depth) {
            return inlined;
        }
        let callee = self.ser_expr(&call.callee, depth + 1);
        let type_arguments = self.ser_type_arguments(call.type_arguments.as_deref());
        let arguments: Vec<String> = call
            .arguments
            .iter()
            .map(|argument| self.ser_argument(argument, depth + 1))
            .collect();
        let optional = if call.optional { "?." } else { "" };
        format!(
            "(call{optional} {callee}{type_arguments} {})",
            arguments.join(" ")
        )
    }

    /// Imported-helper inlining, the `alpha`-only half (see `helpers.rs` for the other half:
    /// which file and declaration an import names, and the guards). A call whose callee is a
    /// bare identifier resolving to an import binding renders the *helper's own body* in place of
    /// the call, with each parameter read rendered as the argument text this pass already
    /// serialized for it -- so two tests differing only by calling equivalent helpers compare by
    /// what the calls do rather than by which exports they name.
    ///
    /// **`alpha` only, deliberately.** `structural` is `parameterization-candidate`'s key, and
    /// its remedy -- a `test.each` table in the *test file* -- can only parameterize literals
    /// written in the test bodies. Inlined into `structural`, a pair whose differing literals
    /// live inside two helper modules (measured: this exact fixture,
    /// `duplicate-test-imported-helper-differs`) would collapse to one table proposal whose
    /// suggested fix is impossible to write, which is the same reason type arguments survive in
    /// `structural` (see [`Serializer::ser_type_arguments`]). `structural` therefore keeps the
    /// pre-inlining rendering byte-for-byte, and the rule's population is untouched by this
    /// feature.
    ///
    /// Every refusal is silent and lands on today's exact rendering (an `id:free:import:...`
    /// call), because a refusal is an under-match, not a defect: the framework itself
    /// (`specifier == "vitest"`, the same text the registry keys its own framework check on), an
    /// unresolvable import, spread arguments, an arity the helper's parameters cannot bind
    /// exactly, or a helper file/export shape `helpers.rs` does not model. See that module for
    /// why each guard exists.
    ///
    /// Arguments render *before* the helper is asked for its body, so a refused inline wastes
    /// serialization but never changes the outcome: canonical naming is assigned on first
    /// encounter and re-encounter reuses it, so rendering the arguments twice is idempotent.
    fn try_inline_imported_helper(
        &mut self,
        call: &CallExpression<'a>,
        depth: u32,
    ) -> Option<String> {
        if !self.ctx.inline_imported_helpers || matches!(self.mode, Mode::Structural) {
            return None;
        }
        let Expression::Identifier(ident) = &call.callee else {
            return None;
        };
        // Resolve the callee to *the import binding* by symbol identity, not by name -- the same
        // resolution `free_identity` performs for the identity rendering, so a local shadowing
        // an import's name can never inline through the import's origin.
        let symbol_id = ident.reference_id.get().and_then(|reference_id| {
            self.ctx
                .semantic
                .scoping()
                .get_reference(reference_id)
                .symbol_id()
        })?;
        if !self
            .ctx
            .semantic
            .scoping()
            .symbol_flags(symbol_id)
            .is_import()
        {
            return None;
        }
        let origin = self.ctx.imports.get(ident.name.as_str())?;
        let has_spread = call
            .arguments
            .iter()
            .any(|argument| matches!(argument, Argument::SpreadElement(_)));
        if !crate::frontend::helpers::would_inline(origin, call.arguments.len(), has_spread) {
            return None;
        }
        let arguments: Vec<String> = call
            .arguments
            .iter()
            .map(|argument| self.ser_argument(argument, depth + 1))
            .collect();
        let rendered =
            crate::frontend::helpers::render_inline(origin, &arguments, self.mode, depth)?;
        if rendered.class_unresolved {
            self.ctx.class_uncertain.set(true);
            return None;
        }
        // Accounting: only a render that actually happened counts as inlined (see
        // `FingerprintContext::inlined_imports`). A refusal here leaves the module unrecorded,
        // and the callee then renders as an identity through `reference` below, which records
        // the other half.
        self.ctx.inlined_imports.insert(origin.component.clone());
        Some(rendered.text)
    }

    /// `new Foo<T>(...)`.
    ///
    /// Lives in its own method rather than inline in [`Serializer::ser_expr`]'s match, like
    /// [`Serializer::ser_call`] and the member-expression arms already do. That is load-bearing,
    /// not tidiness: `ser_expr` recurses to [`MAX_DEPTH`], an unoptimized build gives every arm's
    /// locals their own stack slot in one shared frame, and the frame was already close enough to
    /// a default test thread's stack that adding a couple of `String` locals to three arms
    /// overflowed it (caught by `ser_expr_depth_guard_bounds_an_ast_deeper_than_the_parser_could_produce`).
    /// Keeping wide arms out of `ser_expr` keeps that headroom.
    pub(super) fn ser_new(&mut self, new_expression: &NewExpression<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(new_expression.span);
        }
        let callee = self.ser_expr(&new_expression.callee, depth + 1);
        let type_arguments = self.ser_type_arguments(new_expression.type_arguments.as_deref());
        let arguments: Vec<String> = new_expression
            .arguments
            .iter()
            .map(|argument| self.ser_argument(argument, depth + 1))
            .collect();
        format!("(new {callee}{type_arguments} {})", arguments.join(" "))
    }

    /// `` tag<T>`...` `` -- see [`Serializer::ser_new`] on why this is not inline.
    pub(super) fn ser_tagged_template(
        &mut self,
        tagged: &TaggedTemplateExpression<'a>,
        depth: u32,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(tagged.span);
        }
        let tag = self.ser_expr(&tagged.tag, depth + 1);
        let type_arguments = self.ser_type_arguments(tagged.type_arguments.as_deref());
        let quasi = self.ser_template(&tagged.quasi, depth + 1);
        format!("(tagged {tag}{type_arguments} {quasi})")
    }

    /// `foo<string>` with no call: an instantiation expression is the one construct here whose
    /// *entire* content is a type-argument list, so unwrapping it to its inner expression (as the
    /// other TS-only wrappers are unwrapped, correctly, because they carry no identity at all)
    /// erased the only thing it says. It is kept as its own node rather than folded into the inner
    /// expression's serialization so that `foo` and `foo<string>` -- genuinely different
    /// expressions -- can never render alike.
    pub(super) fn ser_instantiation(
        &mut self,
        instantiation: &TSInstantiationExpression<'a>,
        depth: u32,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(instantiation.span);
        }
        let expression = self.ser_expr(&instantiation.expression, depth + 1);
        let type_arguments = self.ser_type_arguments(Some(&instantiation.type_arguments));
        format!("(instantiate {expression}{type_arguments})")
    }

    /// Render an expression or spread call argument, retaining opaque unsupported forms.
    pub(super) fn ser_argument(&mut self, argument: &Argument<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(argument.span());
        }
        match argument.as_expression() {
            Some(expression) => self.ser_expr(expression, depth + 1),
            None => match argument {
                Argument::SpreadElement(spread) => {
                    format!("(spread {})", self.ser_expr(&spread.argument, depth + 1))
                }
                _ => self.opaque(argument.span()),
            },
        }
    }

    /// Render array values, spreads and holes without conflating their shapes.
    pub(super) fn ser_array_element(
        &mut self,
        element: &ArrayExpressionElement<'a>,
        depth: u32,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(element.span());
        }
        match element.as_expression() {
            Some(expression) => self.ser_expr(expression, depth + 1),
            None => match element {
                ArrayExpressionElement::SpreadElement(spread) => {
                    format!("(spread {})", self.ser_expr(&spread.argument, depth + 1))
                }
                ArrayExpressionElement::Elision(_) => "<hole>".to_owned(),
                _ => self.opaque(element.span()),
            },
        }
    }

    /// Render object key/value pairs and spread properties in source order.
    pub(super) fn ser_object_property(
        &mut self,
        property: &ObjectPropertyKind<'a>,
        depth: u32,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(property.span());
        }
        match property {
            ObjectPropertyKind::ObjectProperty(object_property) => {
                let oxc_ast::ast::ObjectProperty {
                    node_id: _,
                    span: _,
                    kind,
                    key,
                    value,
                    method,
                    shorthand,
                    computed,
                    ..
                } = &**object_property;
                // Computed and shorthand __proto__ keys create ordinary own properties.
                let prototype_setter = !computed
                    && !shorthand
                    && property_key_name(key).as_deref() == Some("__proto__");
                let key = self.ser_property_key(key, depth + 1);
                let value = self.ser_expr(value, depth + 1);
                let descriptor = match (kind, *method) {
                    (oxc_ast::ast::PropertyKind::Init, false) if prototype_setter => {
                        "prop-prototype"
                    }
                    (oxc_ast::ast::PropertyKind::Init, false) => "prop",
                    (oxc_ast::ast::PropertyKind::Init, true) => "prop-method",
                    (oxc_ast::ast::PropertyKind::Get, _) => "prop-get",
                    (oxc_ast::ast::PropertyKind::Set, _) => "prop-set",
                };
                format!("({descriptor} {key} {value})")
            }
            ObjectPropertyKind::SpreadProperty(spread) => {
                format!("(spread {})", self.ser_expr(&spread.argument, depth + 1))
            }
        }
    }

    /// Serializes the write-target of an assignment.
    ///
    /// Only identifiers and member expressions are modeled explicitly; destructuring assignment
    /// targets (`[a, b] = arr`, `({a} = obj)`, as opposed to a `const`/`let` destructuring
    /// *declaration*, which goes through `ser_binding_pattern` instead) fall back to raw text.
    /// This is a narrow, rare construct in test bodies, and raw text keeps the fallback safe.
    pub(super) fn ser_assignment_target(
        &mut self,
        target: &AssignmentTarget<'a>,
        depth: u32,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(target.span());
        }
        // `AssignmentTarget` carries every `SimpleAssignmentTarget` variant inline, so the two
        // families were serialized by two matches listing the same eight arms. Adding the four TS
        // wrappers to both is what made that a clone large enough to take production Rust to its
        // duplication ceiling -- and it is the same defect shape those wrappers were added to fix:
        // one construct handled twice, in two places that can drift apart. Delegating means there
        // is one list to get right.
        match target.as_simple_assignment_target() {
            Some(simple) => self.ser_simple_assignment_target(simple, depth),
            // What remains is the destructuring family (array and object patterns), which this
            // module does not model.
            None => self.opaque(target.span()),
        }
    }

    /// `UpdateExpression::argument` is a `SimpleAssignmentTarget`, a strict subset of
    /// `AssignmentTarget`'s variants; reuse the same handling for the variants they share.
    pub(super) fn ser_simple_assignment_target(
        &mut self,
        target: &SimpleAssignmentTarget<'a>,
        depth: u32,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(target.span());
        }
        match target {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) => {
                format!("id:{}", self.ctx.reference(ident))
            }
            SimpleAssignmentTarget::StaticMemberExpression(member) => {
                self.ser_static_member(member, depth + 1)
            }
            SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                self.ser_computed_member(member, depth + 1)
            }
            SimpleAssignmentTarget::PrivateFieldExpression(member) => {
                self.ser_private_field(member, depth + 1)
            }
            // The same four TS wrappers `ser_expr` already unwraps in READ position. Without
            // these, `(o.b as any)++` rendered `<opaque:o.b as any>` while a bare `o.b` in the
            // same file rendered its member path -- one construct transparent when read and
            // opaque when written, so a type assertion on the left-hand side silently stopped
            // two otherwise-identical bodies from matching. Type syntax carries no runtime
            // identity, which is the whole reason the read side unwraps it.
            SimpleAssignmentTarget::TSAsExpression(as_expression) => {
                self.ser_expr(&as_expression.expression, depth + 1)
            }
            SimpleAssignmentTarget::TSSatisfiesExpression(satisfies) => {
                self.ser_expr(&satisfies.expression, depth + 1)
            }
            SimpleAssignmentTarget::TSNonNullExpression(non_null) => {
                self.ser_expr(&non_null.expression, depth + 1)
            }
            SimpleAssignmentTarget::TSTypeAssertion(assertion) => {
                self.ser_expr(&assertion.expression, depth + 1)
            } // No catch-all: with the four TS wrappers above, `SimpleAssignmentTarget` is matched
              // exhaustively, and clippy says so (`unreachable pattern`). Leaving a dead
              // `_ => opaque` here would be a fallback that can never run pretending to be a
              // safety net -- and would silently swallow a variant a future oxc adds, which is
              // exactly what an exhaustive match is for.
        }
    }

    /// Render template quasis and embedded expressions, preserving quasi text even in
    /// structural mode.
    pub(super) fn ser_template(&mut self, template: &TemplateLiteral<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(template.span);
        }
        // Quasi text is left intact even in structural mode: templates are string composition,
        // not a single literal value, and collapsing them risks merging templates with
        // meaningfully different static text. Documented under-match.
        let mut parts = Vec::new();
        for (index, quasi) in template.quasis.iter().enumerate() {
            parts.push(format!("`{}`", quasi.value.raw.as_str()));
            if let Some(expression) = template.expressions.get(index) {
                parts.push(self.ser_expr(expression, depth + 1));
            }
        }
        format!("(template {})", parts.join(" "))
    }

    /// Serialize supported expression structure using the current mode, with conservative
    /// fallback for depth or unsupported syntax.
    pub(super) fn ser_expr(&mut self, expr: &Expression<'a>, depth: u32) -> String {
        #[cfg(test)]
        record_stack_low_water_mark();
        if depth > MAX_DEPTH {
            return self.degraded(expr.span());
        }
        match expr {
            Expression::BooleanLiteral(literal) => {
                self.literal("boolean", if literal.value { "true" } else { "false" })
            }
            Expression::NullLiteral(_) => self.literal("null", "null"),
            Expression::NumericLiteral(literal) => self.literal(
                "number",
                literal
                    .raw
                    .as_ref()
                    .map(|raw| raw.as_str())
                    .unwrap_or_default(),
            ),
            Expression::BigIntLiteral(literal) => {
                // A distinct `bigint` kind, not folded into `number`: an audit prompted by the
                // kind-erasure bug above found this module already had the same collision
                // one level down -- `100n` and `100` share digit text, and sharing the `number`
                // kind tag too would let them collide the same way `true`/`'true'` did. BigInt
                // and Number are different runtime types with different arithmetic, so even
                // `structural` keeps them apart rather than folding into the original five
                // literal kinds; that is strictly more precision, never less.
                self.literal("bigint", literal.value.as_str())
            }
            Expression::RegExpLiteral(literal) => self.literal(
                "regex",
                literal
                    .raw
                    .as_ref()
                    .map(|raw| raw.as_str())
                    .unwrap_or_default(),
            ),
            Expression::StringLiteral(literal) => self.literal("string", literal.value.as_str()),
            Expression::TemplateLiteral(template) => self.ser_template(template, depth + 1),
            Expression::Identifier(ident) => self.reference_or_literal(ident, depth),
            Expression::Super(_) => "super".to_owned(),
            Expression::ThisExpression(_) => {
                self.this_or_arguments = true;
                "this".to_owned()
            }
            Expression::ArrayExpression(array) => {
                let elements: Vec<String> = array
                    .elements
                    .iter()
                    .map(|element| self.ser_array_element(element, depth + 1))
                    .collect();
                format!("(array {})", elements.join(" "))
            }
            Expression::ObjectExpression(object) => {
                let properties: Vec<String> = object
                    .properties
                    .iter()
                    .map(|property| self.ser_object_property(property, depth + 1))
                    .collect();
                format!("(object {})", properties.join(" "))
            }
            Expression::ArrowFunctionExpression(arrow) => {
                self.ser_callable(&FunctionLike::Arrow(arrow), depth + 1)
            }
            Expression::FunctionExpression(function) => {
                self.ser_callable(&FunctionLike::Function(function), depth + 1)
            }
            Expression::CallExpression(call) => self.ser_call(call, depth + 1),
            Expression::NewExpression(new_expression) => self.ser_new(new_expression, depth + 1),
            Expression::ClassExpression(class) => self.ser_class(class, depth + 1),
            Expression::AssignmentExpression(assignment) => format!(
                "(assign {} {} {})",
                assignment.operator.as_str(),
                self.ser_assignment_target(&assignment.left, depth + 1),
                self.ser_expr(&assignment.right, depth + 1)
            ),
            Expression::BinaryExpression(binary) => format!(
                "(bin {} {} {})",
                binary.operator.as_str(),
                self.ser_expr(&binary.left, depth + 1),
                self.ser_expr(&binary.right, depth + 1)
            ),
            Expression::LogicalExpression(logical) => format!(
                "(logic {} {} {})",
                logical.operator.as_str(),
                self.ser_expr(&logical.left, depth + 1),
                self.ser_expr(&logical.right, depth + 1)
            ),
            Expression::UnaryExpression(unary) => {
                format!(
                    "(unary {} {})",
                    unary.operator.as_str(),
                    self.ser_expr(&unary.argument, depth + 1)
                )
            }
            Expression::UpdateExpression(update) => {
                let target = self.ser_simple_assignment_target(&update.argument, depth + 1);
                format!(
                    "(update {} {} {target})",
                    update.operator.as_str(),
                    update.prefix
                )
            }
            Expression::ConditionalExpression(conditional) => format!(
                "(cond {} {} {})",
                self.ser_expr(&conditional.test, depth + 1),
                self.ser_expr(&conditional.consequent, depth + 1),
                self.ser_expr(&conditional.alternate, depth + 1)
            ),
            Expression::SequenceExpression(sequence) => {
                let parts: Vec<String> = sequence
                    .expressions
                    .iter()
                    .map(|expression| self.ser_expr(expression, depth + 1))
                    .collect();
                format!("(seq {})", parts.join(" "))
            }
            Expression::AwaitExpression(await_expression) => {
                format!(
                    "(await {})",
                    self.ser_expr(&await_expression.argument, depth + 1)
                )
            }
            Expression::YieldExpression(yield_expression) => match &yield_expression.argument {
                Some(argument) => format!(
                    "(yield{} {})",
                    if yield_expression.delegate { "*" } else { "" },
                    self.ser_expr(argument, depth + 1)
                ),
                None => format!(
                    "(yield{})",
                    if yield_expression.delegate { "*" } else { "" }
                ),
            },
            Expression::ChainExpression(chain) => self.ser_chain(chain, depth + 1),
            Expression::ParenthesizedExpression(parenthesized) => {
                // Parens carry no runtime identity; unwrapping avoids spurious mismatches between
                // `x` and `(x)`.
                self.ser_expr(&parenthesized.expression, depth + 1)
            }
            Expression::TSAsExpression(as_expression) => {
                self.ser_expr(&as_expression.expression, depth + 1)
            }
            Expression::TSSatisfiesExpression(satisfies) => {
                self.ser_expr(&satisfies.expression, depth + 1)
            }
            Expression::TSNonNullExpression(non_null) => {
                self.ser_expr(&non_null.expression, depth + 1)
            }
            Expression::TSTypeAssertion(assertion) => {
                self.ser_expr(&assertion.expression, depth + 1)
            }
            Expression::TSInstantiationExpression(instantiation) => {
                self.ser_instantiation(instantiation, depth + 1)
            }
            Expression::TaggedTemplateExpression(tagged) => {
                self.ser_tagged_template(tagged, depth + 1)
            }
            Expression::StaticMemberExpression(member) => self.ser_static_member(member, depth + 1),
            Expression::ComputedMemberExpression(member) => {
                self.ser_computed_member(member, depth + 1)
            }
            Expression::PrivateFieldExpression(member) => self.ser_private_field(member, depth + 1),
            Expression::PrivateInExpression(membership) => {
                let name = self.private_name(membership.left.name.as_str());
                let object = self.ser_expr(&membership.right, depth + 1);
                format!("(private-in #{name} {object})")
            }

            // A dynamic `import()` names a module, and a module identity is exactly what this
            // scheme anchors everywhere else. Falling through to `opaque` rendered the verbatim
            // source slice instead, so two files each doing `await import('./recipes')` compared
            // EQUAL however differently their `./recipes` resolved -- the same defect
            // `fixtures/recall/cross-file-duplicate-same-specifier-different-module/` pins for
            // the STATIC form, which `import_origins` closed and this path never saw. Measured:
            // 380 of the corpus's 13,781 test files contain `await import(`.
            //
            // Only a static string specifier can be resolved; `import(name)` keeps the
            // conservative fallback, because the module it names is not knowable here.
            Expression::ImportExpression(import) => match &import.source {
                Expression::StringLiteral(specifier) => format!(
                    "(import {})",
                    specifier_identity(&self.ctx.file, specifier.value.as_str())
                ),
                _ => self.opaque(expr.span()),
            },
            // JSX is serialized structurally (`ser_jsx_element`/`ser_jsx_fragment`): the verbatim
            // slice this arm used to emit made the comparison textual, so a formatter reindenting
            // one element broke a match between two bodies that render identically, and a literal
            // inside an attribute was not a literal to the structural digest at all.
            Expression::JSXElement(element) => self.ser_jsx_element(element, depth + 1),
            Expression::JSXFragment(fragment) => self.ser_jsx_fragment(fragment, depth + 1),
            // Decorators, `import.meta`/`new.target`, private `in`, V8 intrinsics, and any other
            // exotic expression form: raw text. `opaque` emits the verbatim source slice, so
            // textually different forms DO compare unequal; what it loses is alpha-renaming and
            // import anchoring inside the slice, and it is whitespace-sensitive.
            _ => self.opaque(expr.span()),
        }
    }
}
