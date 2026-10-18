//! Class summary caching and owned class body serialization.
use super::*;

/// One cached body, separated from the capture values read at a particular construction.
#[derive(Clone)]
pub(super) struct ClassSummary {
    pub(super) digest: String,
    pub(super) captures: Vec<SymbolId>,
    pub(super) definition: String,
    pub(super) escapes_this: bool,
}

impl<'a, 'b, 's, 'm> Serializer<'a, 'b, 's, 'm> {
    /// Save declaration identities before behavior collection can reorder first observations.
    pub(in crate::frontend::fingerprint) fn record_instance_binding(
        &self,
        symbol: SymbolId,
        binding: &str,
    ) {
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
    pub(super) fn instance_binding(&mut self, symbol: SymbolId) -> String {
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

    /// Cache normalized implementation separately from nominal identity and point-sensitive state.
    pub(in crate::frontend::fingerprint) fn ser_class(
        &mut self,
        class: &Class<'a>,
        depth: u32,
    ) -> String {
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
    pub(super) fn class_summary(&mut self, class: &Class<'a>, depth: u32) -> Option<ClassSummary> {
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
    pub(in crate::frontend::fingerprint) fn class_definition_effects(
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
