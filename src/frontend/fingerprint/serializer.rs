use super::*;

/// Produces a deterministic S-expression-shaped serialization of a callback body.
///
/// One `Serializer` is used for exactly one of `alpha`/`structural`; its `canonical` map
/// is local to that one pass. Because the traversal order is a pure function of the AST plus the
/// (shared, read-only) `Semantic`, the `alpha` and `structural` passes independently assign the
/// same canonical names to the same symbols without needing to share state.
pub(super) struct Serializer<'a, 'b, 's, 'm> {
    /// Everything both traversals know about the file, plus the shared canonical-naming state;
    /// see [`FingerprintContext`].
    pub(super) ctx: FingerprintContext<'a, 'b, 's, 'm>,
    pub(super) mode: Mode,
    /// Whether the callable currently being serialized reads `this` or `arguments`.
    ///
    /// Arrow and `function` callbacks are deliberately collapsed to one serialization -- Vitest
    /// accepts either, and rewriting one as the other is the single most common cosmetic
    /// difference between two copies of the same test (`near-arrow-vs-function-equivalence`
    /// pins that collapse). The collapse is only sound while the body uses neither `this` nor
    /// `arguments`: those are precisely the constructs an arrow does *not* bind itself, so two
    /// otherwise byte-identical bodies mean different things across the two forms. Collapsing
    /// them anyway reported a `duplicate-test` at Error severity on correct code.
    ///
    /// Tracked as state rather than a pre-pass because `this` binds to the nearest enclosing
    /// *non-arrow* function: a `this` inside a nested arrow belongs to the function around it,
    /// so the flag propagates outward through arrows and stops at a `function`. See
    /// [`Serializer::ser_callable`].
    pub(super) this_or_arguments: bool,
    /// Whether the OUTERMOST callable this serializer ran over was `this`/`arguments`-dependent.
    ///
    /// Not the same as [`Serializer::this_or_arguments`] after the run: `ser_callable` *absorbs*
    /// the flag for a `function` (it binds its own `this`), so the post-state reads `false` for
    /// exactly the body that is most dependent on it. This records the decision at the moment it
    /// is made, for the one callable that matters -- the test body itself.
    pub(super) outermost_this_dependent: bool,
}

impl<'a, 'b, 's, 'm> Serializer<'a, 'b, 's, 'm> {
    /// One serializer for exactly one pass over one body.
    pub(super) fn new(ctx: FingerprintContext<'a, 'b, 's, 'm>, mode: Mode) -> Self {
        Self {
            ctx,
            mode,
            this_or_arguments: false,
            outermost_this_dependent: false,
        }
    }

    /// The whole callable as one text: signature, then statements.
    pub(super) fn run(&mut self, body: &FunctionLike<'a, '_>) -> String {
        self.ser_callable(body, 0)
    }

    /// [`Serializer::run`], also reporting whether the body binds meaning to `this`/`arguments`.
    ///
    /// `behavior` needs the same answer `alpha` computes, and computing it twice is how two
    /// views of one body drift apart. Taking it from the alpha pass costs nothing: the body is
    /// serialized exactly as often as before. Borrowed (`&mut self`, unlike the by-value shape
    /// these methods once had) so the caller can also read the pass's import accounting when it
    /// finishes -- see `fingerprint`'s use of [`FingerprintContext::fully_inlined_imports`].
    pub(super) fn run_observing_this(&mut self, body: &FunctionLike<'a, '_>) -> (String, bool) {
        let text = self.ser_callable(body, 0);
        (text, self.outermost_this_dependent)
    }

    /// Raw-text fallback for anything not explicitly modeled. Two different opaque constructs
    /// never accidentally collapse (they compare by literal text, idents and all). The text alone
    /// cannot say what its names mean, so [`Serializer::raw_fallback`] adds that. See the module
    /// doc.
    pub(super) fn opaque(&mut self, span: Span) -> String {
        self.raw_fallback("opaque", span)
    }

    /// Retain a depth-limit marker and the original source slice rather than claiming a
    /// complete rendering, with the same additions as [`Serializer::opaque`].
    pub(super) fn degraded(&mut self, span: Span) -> String {
        self.raw_fallback("max-depth", span)
    }

    /// The verbatim source under `span`, plus what the rest of the body knows about it.
    ///
    /// A slice keeps the original spelling of every name, while the body around it is alpha-renamed,
    /// row-substituted and import-anchored. Two bodies whose slices read alike can therefore mean
    /// different things: `[x] = [1]` assigns whichever variable `x` names, and `config.v` reads
    /// whichever `config` is in scope. So the slice is followed by the identity of each binding it
    /// uses from outside the slice ([`Serializer::raw_binding_identity`]), and a slice that uses
    /// `this`, `arguments` or `new.target` of a function outside the slice marks the body as
    /// dependent on its callback form.
    fn raw_fallback(&mut self, marker: &str, span: Span) -> String {
        if !self.ctx.class_regions.is_empty() {
            self.ctx.class_uncertain.set(true);
        }
        // Without a node table the slice cannot be checked, so the body is not compared.
        if self.ctx.semantic.nodes().is_empty() {
            self.ctx.class_uncertain.set(true);
        }
        if self.ctx.raw_slice_binds_this(span) {
            self.this_or_arguments = true;
        }
        let identities: Vec<String> = self
            .ctx
            .raw_slice_references(span)
            .into_iter()
            .map(|ident| format!("{}={}", ident.name, self.raw_binding_identity(ident)))
            .collect();
        let suffix = if identities.is_empty() {
            String::new()
        } else {
            format!("|{}", identities.join(","))
        };
        format!("<{marker}:{}{suffix}>", self.ctx.slice(span))
    }

    /// What a binding used inside a raw slice stands for: the value a `.each` row substitutes for
    /// it, otherwise its identity (canonical local slot, outer declaration or import).
    /// Deliberately not [`Serializer::reference_or_literal`]: that also expands classes and
    /// dataflow, which recurse and mark the body uncertain, while a name in a raw slice only needs
    /// to be tied to the binding it means -- the binding's own contents are rendered where it is
    /// declared. An inlined helper's parameter is never marked consumed by a raw slice, so the
    /// call's arguments stay visible in the inlined wrapper.
    fn raw_binding_identity(&mut self, ident: &IdentifierReference<'a>) -> String {
        let symbol = ident.reference_id.get().and_then(|reference| {
            self.ctx
                .semantic
                .scoping()
                .get_reference(reference)
                .symbol_id()
        });
        if let Some(symbol) = symbol {
            if let Some((kind, raw)) = self.ctx.row_substitution.get(&symbol).cloned() {
                return self.literal(kind, &raw);
            }
        }
        format!("id:{}", self.ctx.reference(ident))
    }

    /// An explicit type-argument list, rendered identically in all three modes.
    ///
    /// `Mode::Structural` collapses a literal's *value* to its kind so that
    /// `parameterization-candidate` can propose a `test.each` table whose rows supply the values
    /// that differ. A type argument is not a value: `assertType<string>` and `assertType<number>`
    /// cannot be consolidated into two rows of a table, because a table cell is a runtime value
    /// and a type argument is erased before any value exists. Collapsing type arguments in
    /// `structural` would therefore not enable a real consolidation, it would only manufacture a
    /// `parameterization-candidate` finding whose suggested fix is impossible to write -- and it
    /// would do the same to `behavior`'s assertion *subject* shape, which is rendered in
    /// `structural` too ([`BehaviorCollector::assertion_event`]), re-opening the very erasure
    /// this models its way out of one rule lower down. So every mode keeps the type arguments.
    pub(super) fn ser_type_arguments(
        &self,
        type_arguments: Option<&TSTypeParameterInstantiation<'a>>,
    ) -> String {
        type_argument_fragment(self.ctx.source, type_arguments)
    }

    /// Renders an `IdentifierReference` in *value* (read) position: a row-substituted parameter
    /// renders exactly as [`Serializer::literal`] would render the same value written directly as
    /// a literal in source, and a dataflow-substituted local (see [`DataflowSubst`]) renders
    /// exactly as its substituted expression would render if written directly at this position --
    /// both deliberately *not* wrapped in the `id:` prefix every other identifier reference gets,
    /// because that prefix exists precisely to keep an identifier visibly distinct from what it
    /// stands in for, and a substituted reference must be indistinguishable from the real thing.
    /// Only a resolved local symbol can have a substitution at all -- nothing this module
    /// resolves to `free`/`global` was ever a parameter of this body or a dataflow-canonicalized
    /// local (`analyze_dataflow` only ever registers a body-local symbol).
    pub(super) fn reference_or_literal(
        &mut self,
        ident: &IdentifierReference<'a>,
        depth: u32,
    ) -> String {
        // `arguments` is the other construct an arrow does not bind (see `this_or_arguments`).
        // It reaches here as an ordinary identifier reference; a *resolved* symbol by that name
        // is some user's own binding and carries none of the meaning, so only the free form --
        // the real `arguments` object -- counts.
        if ident.name.as_str() == "arguments"
            && ident
                .reference_id
                .get()
                .and_then(|reference_id| {
                    self.ctx
                        .semantic
                        .scoping()
                        .get_reference(reference_id)
                        .symbol_id()
                })
                .is_none()
        {
            self.this_or_arguments = true;
        }
        if self.ctx.expand_classes {
            if let Some(class) = self.class_reference(ident, depth + 1) {
                return class;
            }
            if let Some(instance) = self.class_instance_reference(ident, depth + 1) {
                return instance;
            }
        } else if self.instance_name_observed(ident) {
            if let Some(instance) = self.class_instance_reference(ident, depth + 1) {
                return instance;
            }
        }
        if let Some(value) = self.class_capture(ident, depth + 1) {
            return value;
        }
        let symbol_id = ident.reference_id.get().and_then(|reference_id| {
            self.ctx
                .semantic
                .scoping()
                .get_reference(reference_id)
                .symbol_id()
        });
        if let Some(symbol_id) = symbol_id {
            if let Some(fragment) = self.ctx.arg_substitution.get(&symbol_id).cloned() {
                self.ctx.consumed_args.insert(symbol_id);
                return fragment;
            }
            if let Some((kind, raw)) = self.ctx.row_substitution.get(&symbol_id).cloned() {
                return self.literal(kind, &raw);
            }
            let dataflow = self.ctx.dataflow.get(&symbol_id).cloned();
            if let Some(dataflow) = dataflow {
                return match dataflow {
                    DataflowSubst::Inline { init, .. } => self.ser_expr(init, depth + 1),
                    DataflowSubst::Member { object, property } => {
                        let object_repr = self.ser_expr(object, depth + 1);
                        format!("(member {object_repr} .{property})")
                    }
                    DataflowSubst::DestructuredProperty { property } => {
                        format!("id:{}:prop:{property}", self.ctx.reference(ident))
                    }
                };
            }
        }
        format!("id:{}", self.ctx.reference(ident))
    }

    /// Allocate or reuse a canonical binding identity in this serialization context.
    fn binding(&mut self, name: &str, symbol_id: Option<SymbolId>) -> String {
        let binding = binding_identity(
            &mut self.ctx.canonical,
            &mut self.ctx.next_local,
            self.ctx.local_prefix,
            symbol_id,
            name,
        );
        if let Some(symbol) = symbol_id {
            self.record_instance_binding(symbol, &binding);
        }
        binding
    }

    /// Renders a literal, always including its `kind` so two literals of different types that
    /// happen to render the same raw text (`true` the boolean vs. `'true'` the string; `1000` the
    /// number vs. `'1000'` the string) can never collide, and so an empty-content literal (`''`)
    /// can never vanish entirely and become indistinguishable from *no* literal at all -- both
    /// were real, adjudicator-found `duplicate-test` false positives (`expect(parse(true))` vs.
    /// `expect(parse('true'))`; `expect(parse(['']))` vs. `expect(parse([]))`, since `''` used to
    /// serialize to nothing, making a one-element array of it byte-identical to an empty array).
    /// This also keeps `alpha` from drifting toward `structural`: two bodies that differ only in
    /// a literal's *value* should be indistinguishable under `alpha` only when they also share
    /// the literal's *kind* -- that is what makes `structural`'s further collapse (which does
    /// erase kind, deliberately) the meaningful step between `duplicate-test` and
    /// `parameterization-candidate`, rather than `alpha` already doing `structural`'s job.
    ///
    /// `raw` is escaped before being framed by `<kind:...>` (see [`escape_literal_delimiters`]).
    /// Without escaping, `raw` is attacker/author-controlled text (a string literal's own
    /// contents) that can itself contain the `<`/`>`/space grammar this serializer uses to
    /// separate sibling nodes, so one literal whose *value* spells out that grammar can render
    /// byte-identical to a sequence of several sibling literals -- e.g. an array holding the
    /// single string `"x> <string:y"` and an array holding the two strings `"x"` and `"y"` both
    /// serialize their elements to `<string:x> <string:y>` without escaping, joined identically
    /// by `ser_array_element`'s `" "`.join, producing the same `alpha` for genuinely different
    /// code (this was a real, self-inflicted `duplicate-test` false positive at Error severity,
    /// introduced by the kind-tagging fix above). Escaping -- rather than length-prefixing -- is
    /// chosen because it is a strictly local, one-line change at the single call site that
    /// produces every literal's raw text, it keeps the serialized form readable for debugging
    /// (a length prefix would require counting bytes by eye to find the payload boundary), and it
    /// composes for free with every other literal-producing call site (`ser_template`'s quasis
    /// are deliberately left unescaped raw text by contrast -- see that function's own comment --
    /// because they are never compared as a bare kind-delimited unit the way `literal` values
    /// are).
    pub(super) fn literal(&self, kind: &'static str, raw: &str) -> String {
        match self.mode {
            Mode::Alpha => {
                format!("<{kind}:{}>", escape_literal_delimiters(raw))
            }
            Mode::Structural => format!("<{kind}>"),
        }
    }

    /// Serialize a callable's flags, parameters and body while scoping this/arguments
    /// observations to its binding behavior.
    pub(super) fn ser_callable(&mut self, body: &FunctionLike<'a, '_>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(body.span());
        }
        let asyncness = if body.is_async() { "async " } else { "" };
        let generator = if body.is_generator() { "*" } else { "" };
        let params = self.ser_params(body.params(), depth + 1);
        // Scope the `this`/`arguments` observation to this callable. An arrow does not bind
        // either, so whatever it saw still belongs to the enclosing function and is propagated
        // back below; a `function` does bind both, so what it saw stops here and must not make
        // its *parent* look `this`-dependent.
        let enclosing_saw = self.this_or_arguments;
        self.this_or_arguments = false;
        let body_text = match body {
            FunctionLike::Arrow(arrow) => match &arrow.body {
                ArrowFunctionBody::FunctionBody(function_body) => {
                    self.ser_stmts(&function_body.statements, depth + 1)
                }
                other => match other.as_expression() {
                    Some(expression) => self.ser_expr(expression, depth + 1),
                    None => self.opaque(other.span()),
                },
            },
            FunctionLike::Function(function) => match &function.body {
                Some(function_body) => self.ser_stmts(&function_body.statements, depth + 1),
                None => "(no-body)".to_owned(),
            },
        };
        let this_dependent = self.this_or_arguments;
        if depth == 0 {
            self.outermost_this_dependent = this_dependent;
        }
        self.this_or_arguments = match body {
            // Propagate: the arrow did not bind what it saw, its enclosing function did.
            FunctionLike::Arrow(_) => enclosing_saw || this_dependent,
            // Absorb: a `function` binds its own `this`/`arguments`.
            FunctionLike::Function(_) => enclosing_saw,
        };
        // `fn` when the two forms really are interchangeable -- which keeps every existing
        // fingerprint byte-identical -- and the specific form only when it changes meaning.
        let form = if !this_dependent {
            "fn"
        } else {
            match body {
                FunctionLike::Arrow(_) => "arrow-fn",
                FunctionLike::Function(_) => "function-fn",
            }
        };
        format!("({asyncness}{generator}{form} ({params}) {body_text})")
    }

    /// Whether every identifier `pattern` declares is a row-substituted parameter -- meaning
    /// `pattern` should be omitted from the *declaration* side of the fingerprint entirely
    /// (`ser_params`, and a destructured property inside `ser_binding_pattern`'s `ObjectPattern`
    /// arm), not merely have its own read occurrences replaced.
    ///
    /// This matters because a substituted parameter is no longer meaningfully "a declared
    /// parameter" for comparison purposes: its value is now fixed by the row, exactly like a
    /// hand-written literal is fixed by the source that wrote it, and a hand-written duplicate of
    /// a substituted row never declares that parameter at all (`(w, h, expected) => expect(area(w,
    /// h)).toBe(expected)` substituted for row `[2, 3, 6]` must match `() =>
    /// expect(area(2, 3)).toBe(6)`, which has zero parameters, not three canonical locals sitting
    /// unused in the signature). Leaving a fully-substituted parameter in the printed signature
    /// would make every substituted row permanently un-matchable against the hand-written form
    /// row substitution exists to link, defeating the whole feature.
    ///
    /// Only `BindingIdentifier` (always: substituted or not, by direct lookup) and `ObjectPattern`
    /// (recursively: every property, only when there is no rest) can ever be "fully substituted",
    /// because those are the only two shapes [`resolve_row_substitution`] ever populates the map
    /// from; anything else (`AssignmentPattern`, `ArrayPattern`) is never substituted at all and
    /// this always returns `false` for it, which is the correct, safe default -- it can only cause
    /// a parameter to stay visible in the signature (a false negative on matching), never the
    /// unsound direction of erasing a parameter this module did not actually substitute.
    fn binding_fully_substituted(&self, pattern: &BindingPattern<'a>) -> bool {
        match pattern {
            BindingPattern::BindingIdentifier(id) => id
                .symbol_id
                .get()
                .is_some_and(|symbol_id| self.ctx.row_substitution.contains_key(&symbol_id)),
            BindingPattern::ObjectPattern(object) => {
                object.rest.is_none()
                    && !object.properties.is_empty()
                    && object
                        .properties
                        .iter()
                        .all(|property| self.binding_fully_substituted(&property.value))
            }
            BindingPattern::AssignmentPattern(_) | BindingPattern::ArrayPattern(_) => false,
        }
    }

    /// Render formal parameters and defaults, omitting patterns fully replaced by row
    /// substitutions.
    fn ser_params(&mut self, params: &FormalParameters<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(params.span);
        }
        let mut parts: Vec<String> = Vec::with_capacity(params.items.len());
        for item in &params.items {
            // See `binding_fully_substituted`'s doc: a fully-substituted parameter is omitted
            // from the signature entirely, not printed with its substituted value inline --
            // `reference_or_literal` already renders every *use* of it that way; the declaration
            // site itself must vanish, matching a hand-written duplicate that never declared it.
            if self.binding_fully_substituted(&item.pattern) {
                continue;
            }
            let part = match &item.initializer {
                Some(initializer) => format!(
                    "(default {} {})",
                    self.ser_binding_pattern(&item.pattern, depth + 1),
                    self.ser_expr(initializer, depth + 1)
                ),
                None => self.ser_binding_pattern(&item.pattern, depth + 1),
            };
            parts.push(part);
        }
        if let Some(rest) = &params.rest {
            parts.push(format!(
                "(rest {})",
                self.ser_binding_pattern(&rest.rest.argument, depth + 1)
            ));
        }
        parts.join(" ")
    }

    /// Serialize binding identifiers and destructuring/default patterns within the depth limit.
    fn ser_binding_pattern(&mut self, pattern: &BindingPattern<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(pattern.span());
        }
        match pattern {
            BindingPattern::BindingIdentifier(id) => {
                self.binding(id.name.as_str(), id.symbol_id.get())
            }
            BindingPattern::AssignmentPattern(assignment) => format!(
                "(default {} {})",
                self.ser_binding_pattern(&assignment.left, depth + 1),
                self.ser_expr(&assignment.right, depth + 1)
            ),
            BindingPattern::ObjectPattern(object) => {
                let mut parts: Vec<String> = Vec::with_capacity(object.properties.len());
                for property in &object.properties {
                    // See `binding_fully_substituted`'s doc: a substituted destructured property
                    // is omitted from the printed pattern entirely, matching a hand-written
                    // duplicate that would only destructure the fields it still needs.
                    if self.binding_fully_substituted(&property.value) {
                        continue;
                    }
                    parts.push(format!(
                        "(prop {} {})",
                        self.ser_property_key(&property.key, depth + 1),
                        self.ser_binding_pattern(&property.value, depth + 1)
                    ));
                }
                if let Some(rest) = &object.rest {
                    parts.push(format!(
                        "(rest {})",
                        self.ser_binding_pattern(&rest.argument, depth + 1)
                    ));
                }
                format!("(object-pattern {})", parts.join(" "))
            }
            BindingPattern::ArrayPattern(array) => {
                let mut parts: Vec<String> = array
                    .elements
                    .iter()
                    .map(|element| match element {
                        Some(element) => self.ser_binding_pattern(element, depth + 1),
                        None => "<hole>".to_owned(),
                    })
                    .collect();
                if let Some(rest) = &array.rest {
                    parts.push(format!(
                        "(rest {})",
                        self.ser_binding_pattern(&rest.argument, depth + 1)
                    ));
                }
                format!("(array-pattern {})", parts.join(" "))
            }
        }
    }

    /// Keep a private member's lexical owner distinct from a nested class's local slot.
    pub(super) fn private_name(&self, original: &str) -> String {
        self.ctx
            .private_names
            .iter()
            .rev()
            .enumerate()
            .find_map(|(depth, names)| names.get(original).map(|name| (depth, name)))
            .map_or_else(
                || original.to_owned(),
                |(depth, name)| {
                    if depth == 0 {
                        name.clone()
                    } else {
                        format!("{name}@outer{depth}")
                    }
                },
            )
    }

    /// Render static/private property names or expression-valued keys, retaining opaque
    /// unsupported syntax.
    pub(super) fn ser_property_key(&mut self, key: &PropertyKey<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(key.span());
        }
        if let Some(expression) = key.as_expression() {
            return format!("[{}]", self.ser_expr(expression, depth + 1));
        }
        match key {
            PropertyKey::StaticIdentifier(id) => format!("prop:{}", id.name.as_str()),
            PropertyKey::PrivateIdentifier(id) => {
                format!("prop:#{}", self.private_name(id.name.as_str()))
            }
            _ => self.opaque(key.span()),
        }
    }

    /// Serialize statements in order into a block fragment, retaining a degradation marker at
    /// the depth limit.
    pub(super) fn ser_stmts(&mut self, statements: &[Statement<'a>], depth: u32) -> String {
        if depth > MAX_DEPTH {
            let span = statements
                .first()
                .map(GetSpan::span)
                .unwrap_or(Span::new(0, 0));
            return self.degraded(span);
        }
        let parts: Vec<String> = statements
            .iter()
            .map(|statement| self.ser_stmt(statement, depth + 1))
            .collect();
        format!("(block {})", parts.join(" "))
    }

    /// Serialize one statement while preserving its control flow and resolved local bindings.
    /// Function declarations emit their canonical identity before parameters, so references
    /// retain their target even when hoisting places a call before the declaration.
    fn ser_stmt(&mut self, statement: &Statement<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(statement.span());
        }
        match statement {
            Statement::BlockStatement(block) => self.ser_stmts(&block.body, depth + 1),
            Statement::ExpressionStatement(expression_statement) => {
                format!(
                    "(stmt {})",
                    self.ser_expr(&expression_statement.expression, depth + 1)
                )
            }
            Statement::EmptyStatement(_) => "(empty)".to_owned(),
            Statement::DebuggerStatement(_) => "(debugger)".to_owned(),
            Statement::VariableDeclaration(declaration) => {
                self.ser_variable_declaration(declaration, depth)
            }
            Statement::FunctionDeclaration(function) => {
                // Keep the declaration-to-reference link, including when a hoisted call was
                // encountered first. Registering without emitting the identity would still
                // merge calls to different declarations before those declarations appear.
                let name = function.id.as_ref().map_or_else(
                    || "<anonymous>".to_owned(),
                    |id| self.binding(id.name.as_str(), id.symbol_id.get()),
                );
                format!(
                    "(function-decl {name} {})",
                    self.ser_callable(&FunctionLike::Function(function), depth + 1)
                )
            }
            Statement::ClassDeclaration(class) => {
                if let Some(id) = &class.id {
                    self.binding(id.name.as_str(), id.symbol_id.get());
                }
                self.ser_class(class, depth + 1)
            }
            Statement::IfStatement(if_statement) => {
                let test = self.ser_expr(&if_statement.test, depth + 1);
                let consequent = self.ser_stmt(&if_statement.consequent, depth + 1);
                match &if_statement.alternate {
                    Some(alternate) => {
                        format!(
                            "(if {test} {consequent} {})",
                            self.ser_stmt(alternate, depth + 1)
                        )
                    }
                    None => format!("(if {test} {consequent})"),
                }
            }
            Statement::ForStatement(for_statement) => {
                let init = match &for_statement.init {
                    Some(ForStatementInit::VariableDeclaration(declaration)) => {
                        self.ser_variable_declaration(declaration, depth)
                    }
                    Some(other) => match other.as_expression() {
                        Some(expression) => self.ser_expr(expression, depth + 1),
                        None => self.opaque(other.span()),
                    },
                    None => "()".to_owned(),
                };
                let test = for_statement
                    .test
                    .as_ref()
                    .map_or_else(|| "()".to_owned(), |test| self.ser_expr(test, depth + 1));
                let update = for_statement.update.as_ref().map_or_else(
                    || "()".to_owned(),
                    |update| self.ser_expr(update, depth + 1),
                );
                let body = self.ser_stmt(&for_statement.body, depth + 1);
                format!("(for {init} {test} {update} {body})")
            }
            Statement::ForInStatement(for_in) => {
                let left = self.ser_for_left(&for_in.left, depth + 1);
                let right = self.ser_expr(&for_in.right, depth + 1);
                let body = self.ser_stmt(&for_in.body, depth + 1);
                format!("(for-in {left} {right} {body})")
            }
            Statement::ForOfStatement(for_of) => {
                // Async iteration selects a distinct protocol, including awaiting values when
                // wrapping a synchronous iterator. Preserve that choice in the loop identity.
                let head = if for_of.r#await {
                    "for-await-of"
                } else {
                    "for-of"
                };
                let left = self.ser_for_left(&for_of.left, depth + 1);
                let right = self.ser_expr(&for_of.right, depth + 1);
                let body = self.ser_stmt(&for_of.body, depth + 1);
                format!("({head} {left} {right} {body})")
            }
            Statement::WhileStatement(while_statement) => {
                format!(
                    "(while {} {})",
                    self.ser_expr(&while_statement.test, depth + 1),
                    self.ser_stmt(&while_statement.body, depth + 1)
                )
            }
            Statement::DoWhileStatement(do_while) => {
                format!(
                    "(do-while {} {})",
                    self.ser_stmt(&do_while.body, depth + 1),
                    self.ser_expr(&do_while.test, depth + 1)
                )
            }
            Statement::ReturnStatement(return_statement) => match &return_statement.argument {
                Some(argument) => format!("(return {})", self.ser_expr(argument, depth + 1)),
                None => "(return)".to_owned(),
            },
            Statement::ThrowStatement(throw_statement) => {
                format!(
                    "(throw {})",
                    self.ser_expr(&throw_statement.argument, depth + 1)
                )
            }
            Statement::TryStatement(try_statement) => {
                let block = self.ser_stmts(&try_statement.block.body, depth + 1);
                let handler = match &try_statement.handler {
                    Some(handler) => self.ser_catch(handler, depth + 1),
                    None => "()".to_owned(),
                };
                let finalizer = match &try_statement.finalizer {
                    Some(finalizer) => self.ser_stmts(&finalizer.body, depth + 1),
                    None => "()".to_owned(),
                };
                format!("(try {block} {handler} {finalizer})")
            }
            Statement::SwitchStatement(switch_statement) => {
                let discriminant = self.ser_expr(&switch_statement.discriminant, depth + 1);
                let cases: Vec<String> = switch_statement
                    .cases
                    .iter()
                    .map(|case| self.ser_switch_case(case, depth + 1))
                    .collect();
                format!("(switch {discriminant} {})", cases.join(" "))
            }
            Statement::BreakStatement(break_statement) => match &break_statement.label {
                Some(label) => format!("(break {})", label.name.as_str()),
                None => "(break)".to_owned(),
            },
            Statement::ContinueStatement(continue_statement) => match &continue_statement.label {
                Some(label) => format!("(continue {})", label.name.as_str()),
                None => "(continue)".to_owned(),
            },
            Statement::LabeledStatement(labeled) => {
                format!(
                    "(label {} {})",
                    labeled.label.name.as_str(),
                    self.ser_stmt(&labeled.body, depth + 1)
                )
            }
            // Class declarations, TS-only declaration forms, and module declarations are rare
            // inside a Vitest callback body; rendered as raw text rather than modeled in depth.
            _ => self.opaque(statement.span()),
        }
    }

    /// A `var`/`let`/`const`/`using` declaration, as a statement or as a `for` initializer --
    /// the same construct in both positions, so one serializer for both.
    fn ser_variable_declaration(
        &mut self,
        declaration: &VariableDeclaration<'a>,
        depth: u32,
    ) -> String {
        let kind = match declaration.kind {
            VariableDeclarationKind::Var => "var",
            VariableDeclarationKind::Let => "let",
            VariableDeclarationKind::Const => "const",
            VariableDeclarationKind::Using => "using",
            VariableDeclarationKind::AwaitUsing => "await-using",
        };
        let declarators = self.ser_declarators(declaration, depth);
        format!("({kind} {})", declarators.join(" "))
    }

    /// Serialize supported loop-left declarations and assignment targets; keep unsupported
    /// target forms opaque and emit a degradation marker beyond the depth limit.
    fn ser_for_left(&mut self, left: &ForStatementLeft<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(left.span());
        }
        match left {
            ForStatementLeft::VariableDeclaration(declaration) => {
                let declarators = self.ser_declarators(declaration, depth);
                format!("(decl {})", declarators.join(" "))
            }
            ForStatementLeft::AssignmentTargetIdentifier(ident) => {
                format!("id:{}", self.ctx.reference(ident))
            }
            ForStatementLeft::StaticMemberExpression(member) => {
                self.ser_static_member(member, depth + 1)
            }
            ForStatementLeft::ComputedMemberExpression(member) => {
                self.ser_computed_member(member, depth + 1)
            }
            ForStatementLeft::PrivateFieldExpression(member) => {
                self.ser_private_field(member, depth + 1)
            }
            // Destructuring into an existing binding (`for ([a, b] of pairs)` reassigning, as
            // opposed to `for (const [a, b] of pairs)` declaring) is rare in test bodies; raw
            // text is the safe fallback.
            _ => self.opaque(left.span()),
        }
    }

    /// Render every declarator of a declaration; only `const` ones may drop unselected siblings.
    fn ser_declarators(
        &mut self,
        declaration: &VariableDeclaration<'a>,
        depth: u32,
    ) -> Vec<String> {
        let is_const = declaration.kind == VariableDeclarationKind::Const;
        declaration
            .declarations
            .iter()
            .map(|declarator| self.ser_declarator(declarator, depth + 1, is_const))
            .collect()
    }

    /// Render a declared binding pattern and any initializer expression.
    fn ser_declarator(
        &mut self,
        declarator: &VariableDeclarator<'a>,
        depth: u32,
        is_const: bool,
    ) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(declarator.span);
        }
        let pattern = self.ser_binding_pattern(&declarator.id, depth + 1);
        match &declarator.init {
            Some(init) => {
                let selected = if is_const {
                    self.ser_selected_object_init(&declarator.id, init, depth + 1)
                } else {
                    None
                };
                match selected {
                    Some(selected) => format!("(decl {pattern} {selected})"),
                    None => format!("(decl {pattern} {})", self.ser_expr(init, depth + 1)),
                }
            }
            None => format!("(decl {pattern})"),
        }
    }

    /// Render a const object destructure's initializer keeping only the properties its pattern
    /// selects: a property no binding can read is dead data, so `{ v: 1, w: 5 }` under
    /// `const { v } = ...` serializes like `{ v: 1 }` -- the declaration-side half of the
    /// invariance the class-capture path's `(selected key value)` rendering already carries.
    ///
    /// Refuses -- falling back to the full literal -- every shape whose dropped parts could
    /// matter: non-static or repeated keys, spreads, accessors, methods, shorthand,
    /// `__proto__`, rest or non-identifier pattern properties, non-`const` declarations, and
    /// any unselected value that is not provably inert data ([`pure_literal`]), and any selected
    /// key the literal does not define itself. An exact-match
    /// object, with nothing to drop, renders byte-identically to the full form.
    fn ser_selected_object_init(
        &mut self,
        pattern: &BindingPattern<'a>,
        init: &Expression<'a>,
        depth: u32,
    ) -> Option<String> {
        let BindingPattern::ObjectPattern(pattern) = pattern else {
            return None;
        };
        if pattern.rest.is_some()
            || pattern.properties.is_empty()
            || pattern.properties.iter().any(|property| {
                property.computed || !matches!(property.value, BindingPattern::BindingIdentifier(_))
            })
        {
            return None;
        }
        let selected: HashSet<String> = pattern
            .properties
            .iter()
            .map(|property| property_key_name(&property.key))
            .collect::<Option<_>>()?;
        let Expression::ObjectExpression(object) = init else {
            return None;
        };
        let mut seen = HashSet::new();
        let mut present = 0;
        let mut kept: Vec<&ObjectPropertyKind<'a>> = Vec::new();
        for kind in &object.properties {
            let ObjectPropertyKind::ObjectProperty(property) = kind else {
                return None;
            };
            let key = property_key_name(&property.key)?;
            if property.kind != PropertyKind::Init
                || property.computed
                || property.method
                || property.shorthand
                || key == "__proto__"
                || !seen.insert(key.clone())
            {
                return None;
            }
            if selected.contains(&key) {
                present += 1;
                kept.push(kind);
            } else if !pure_literal(&property.value, 0, false) {
                return None;
            }
        }
        // A selected key the literal lacks falls through to the prototype chain, where an
        // inherited getter can read the very siblings being dropped.
        if present != selected.len() {
            return None;
        }
        let properties: Vec<String> = kept
            .iter()
            .map(|property| self.ser_object_property(property, depth))
            .collect();
        Some(format!("(object {})", properties.join(" ")))
    }

    /// Render the optional catch binding and ordered handler statements.
    fn ser_catch(&mut self, clause: &CatchClause<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(clause.span);
        }
        let param = match &clause.param {
            Some(param) => self.ser_binding_pattern(&param.pattern, depth + 1),
            None => "()".to_owned(),
        };
        format!(
            "(catch {param} {})",
            self.ser_stmts(&clause.body.body, depth + 1)
        )
    }

    /// Render a switch label or default marker followed by its consequent statements.
    fn ser_switch_case(&mut self, case: &SwitchCase<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.degraded(case.span);
        }
        let test = case.test.as_ref().map_or_else(
            || "default".to_owned(),
            |test| self.ser_expr(test, depth + 1),
        );
        let consequent: Vec<String> = case
            .consequent
            .iter()
            .map(|statement| self.ser_stmt(statement, depth + 1))
            .collect();
        format!("(case {test} {})", consequent.join(" "))
    }
}
