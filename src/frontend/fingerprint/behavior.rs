use super::*;

// ---------------------------------------------------------------------------------------------
// behavior events
// ---------------------------------------------------------------------------------------------

/// Collects the ordered behavior signature: canonical callee paths, assertions, awaits, and
/// control-flow markers, in source order.
pub(super) struct BehaviorCollector<'a, 'b, 's, 'm> {
    /// Everything both traversals know about the file, plus the shared canonical-naming state;
    /// see [`FingerprintContext`].
    pub(super) ctx: FingerprintContext<'a, 'b, 's, 'm>,
    pub(super) events: Vec<String>,
}

impl<'a, 'b, 's, 'm> BehaviorCollector<'a, 'b, 's, 'm> {
    /// Initialize a behavior collector with the supplied fingerprint context and no events.
    pub(super) fn new(ctx: FingerprintContext<'a, 'b, 's, 'm>) -> Self {
        Self {
            ctx,
            events: Vec::new(),
        }
    }

    /// Collect ordered behavior events from a callable body or concise arrow expression.
    pub(super) fn run(mut self, body: &FunctionLike<'a, '_>) -> Vec<String> {
        match body {
            FunctionLike::Arrow(arrow) => match &arrow.body {
                ArrowFunctionBody::FunctionBody(function_body) => {
                    for statement in &function_body.statements {
                        self.walk_stmt(statement, false, 0);
                    }
                }
                other => {
                    if let Some(expression) = other.as_expression() {
                        // `() => <call>().then(callback)` -- the expression-bodied-arrow twin of
                        // the `return <call>().then(callback)` case `walk_stmt` handles; see
                        // `then_continuation`'s doc.
                        if let Expression::CallExpression(call) = expression {
                            if let Some((awaited, continuation)) = Self::then_continuation(call) {
                                self.push(false, "await".to_owned());
                                self.walk_expr(awaited, false, 1);
                                for statement in continuation {
                                    self.walk_stmt(statement, false, 1);
                                }
                                return self.events;
                            }
                        }
                        self.walk_expr(expression, false, 0);
                    }
                }
            },
            FunctionLike::Function(function) => {
                if let Some(function_body) = &function.body {
                    for statement in &function_body.statements {
                        self.walk_stmt(statement, false, 0);
                    }
                }
            }
        }
        self.events
    }

    /// Whether a `const` declarator's own binding is fully absorbed by
    /// [`DataflowSubst::Inline`] -- only ever true for a plain `BindingIdentifier` (destructuring
    /// never registers an `Inline` entry; see `analyze_dataflow`), so this deliberately does not
    /// recurse into `ObjectPattern`/`ArrayPattern` the way `Serializer::binding_fully_substituted`
    /// does for row substitution: a destructured property's own read is canonicalized at its read
    /// site, but its *declaration* statement (`const { a } = o`) contributes no event either way
    /// (see the module doc's "Traversal shape" -- a bare identifier read, which is all `o`
    /// contributes here, was never an event to begin with), so there is nothing to suppress.
    fn declarator_fully_inlined(&self, id: &BindingPattern<'a>) -> bool {
        matches!(id, BindingPattern::BindingIdentifier(binding)
        if binding.symbol_id.get().is_some_and(|symbol_id| {
            matches!(
                self.ctx.dataflow.get(&symbol_id),
                Some(DataflowSubst::Inline { .. })
            )
        }))
    }

    /// Recognizes `<call>().then(callback)`: a single awaited call chained straight into its own
    /// continuation, expressed as a promise chain instead of `await`. Returns the awaited call
    /// expression and the callback's own statement list when the shape qualifies for the
    /// `await`/`.then` canonicalization; `None` for anything else -- every arm below is a
    /// precondition, not a parsing convenience. The
    /// transformation's stated precondition is "a single awaited call, with no interleaving", and
    /// each arm is one half of checking it:
    ///
    /// - `call.arguments` must be a single argument: a second argument is a rejection handler
    ///   (`.then(onFulfilled, onRejected)`), and that shape observes a DIFFERENT code path than a
    ///   bare `await` ever can (an `await` that rejects throws, it does not call a second
    ///   function), so it is never equivalent -- the one precondition this transformation was
    ///   required to verify rather than assume.
    /// - the object being `.then`-ed must itself be a plain `CallExpression`, not another `.then`
    ///   chain (`x().then(a).then(b)`) -- chaining means the continuation depends on what `a`
    ///   returns, which is not "a single awaited call".
    /// - the callback must be a zero-parameter arrow or `function` expression with a block body:
    ///   a parameter would receive the resolved value, which this module does not attempt to
    ///   correlate with anything (a missed finding, not a soundness risk); an expression-bodied
    ///   arrow (`.then(() => x)`) has no statement list to splice, so it is left alone too.
    fn then_continuation<'x>(
        call: &'x CallExpression<'a>,
    ) -> Option<(&'x Expression<'a>, &'x [Statement<'a>])> {
        let Expression::StaticMemberExpression(member) = &call.callee else {
            return None;
        };
        if member.property.name.as_str() != "then" {
            return None;
        }
        let Expression::CallExpression(inner) = &member.object else {
            return None;
        };
        if let Expression::StaticMemberExpression(inner_member) = &inner.callee {
            if inner_member.property.name.as_str() == "then" {
                return None; // a chained `.then().then()`: not "a single awaited call".
            }
        }
        let [argument] = call.arguments.as_slice() else {
            return None; // zero, or two-or-more (a rejection handler): never equivalent.
        };
        let callback = argument.as_expression()?;
        let statements: &'x [Statement<'a>] = match callback {
            Expression::ArrowFunctionExpression(arrow)
                if arrow.params.items.is_empty() && arrow.params.rest.is_none() =>
            {
                match &arrow.body {
                    ArrowFunctionBody::FunctionBody(function_body) => &function_body.statements,
                    _ => return None,
                }
            }
            Expression::FunctionExpression(function)
                if function.params.items.is_empty() && function.params.rest.is_none() =>
            {
                function.body.as_ref()?.statements.as_slice()
            }
            _ => return None,
        };
        Some((&member.object, statements))
    }

    /// Append a behavior event, prefixing it when its execution is deferred.
    pub(super) fn push(&mut self, deferred: bool, event: String) {
        self.events.push(if deferred {
            format!("deferred-{event}")
        } else {
            event
        });
    }

    /// Collect statement behavior within the depth bound, carrying deferred-execution state
    /// into nested expressions.
    fn walk_stmt(&mut self, statement: &Statement<'a>, deferred: bool, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        match statement {
            Statement::BlockStatement(block) => {
                for inner in &block.body {
                    self.walk_stmt(inner, deferred, depth + 1);
                }
            }
            Statement::ExpressionStatement(expression_statement) => {
                self.walk_expr(&expression_statement.expression, deferred, depth + 1);
            }
            Statement::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    if let Some(init) = &declarator.init {
                        // A local fully absorbed into "single-use local inlining" (see
                        // `DataflowSubst::Inline`) contributes no event of its own here -- its one
                        // read accounts for this same initializer expression instead, at that read
                        // site: rendered into the assertion's shape when the read is an assertion
                        // subject/value, and re-walked for its own events by `walk_inlined_read`
                        // everywhere else. Walking it here too would double-count it (e.g. its own
                        // `call:` event, once here and once at the read), which is exactly the
                        // mismatch that would stop the inlined and the written-out-twice bodies
                        // from converging.
                        if self.declarator_fully_inlined(&declarator.id) {
                            continue;
                        }
                        self.walk_expr(init, deferred, depth + 1);
                    }
                }
            }
            Statement::ClassDeclaration(class) => {
                self.walk_class_definition(class, deferred, depth + 1)
            }
            Statement::IfStatement(if_statement) => {
                self.push(deferred, "if".to_owned());
                self.walk_expr(&if_statement.test, deferred, depth + 1);
                self.walk_stmt(&if_statement.consequent, deferred, depth + 1);
                if let Some(alternate) = &if_statement.alternate {
                    self.walk_stmt(alternate, deferred, depth + 1);
                }
            }
            Statement::ForStatement(for_statement) => {
                if let Some(init) = &for_statement.init {
                    if let ForStatementInit::VariableDeclaration(declaration) = init {
                        for declarator in &declaration.declarations {
                            if let Some(value) = &declarator.init {
                                self.walk_expr(value, deferred, depth + 1);
                            }
                        }
                    } else if let Some(expression) = init.as_expression() {
                        self.walk_expr(expression, deferred, depth + 1);
                    }
                }
                self.push(deferred, "loop".to_owned());
                if let Some(test) = &for_statement.test {
                    self.walk_expr(test, deferred, depth + 1);
                }
                if let Some(update) = &for_statement.update {
                    self.walk_expr(update, deferred, depth + 1);
                }
                self.walk_stmt(&for_statement.body, deferred, depth + 1);
            }
            Statement::ForInStatement(for_in) => {
                self.push(deferred, "loop".to_owned());
                self.walk_expr(&for_in.right, deferred, depth + 1);
                self.walk_stmt(&for_in.body, deferred, depth + 1);
            }
            Statement::ForOfStatement(for_of) => {
                // Sync loop kinds deliberately share one token. Async iteration changes
                // protocol selection and suspension behavior, so it gets its own token.
                let protocol = if for_of.r#await { "loop-await" } else { "loop" };
                self.push(deferred, protocol.to_owned());
                self.walk_expr(&for_of.right, deferred, depth + 1);
                self.walk_stmt(&for_of.body, deferred, depth + 1);
            }
            Statement::WhileStatement(while_statement) => {
                self.push(deferred, "loop".to_owned());
                self.walk_expr(&while_statement.test, deferred, depth + 1);
                self.walk_stmt(&while_statement.body, deferred, depth + 1);
            }
            Statement::DoWhileStatement(do_while) => {
                self.push(deferred, "loop".to_owned());
                self.walk_stmt(&do_while.body, deferred, depth + 1);
                self.walk_expr(&do_while.test, deferred, depth + 1);
            }
            Statement::TryStatement(try_statement) => {
                self.push(deferred, "try".to_owned());
                for inner in &try_statement.block.body {
                    self.walk_stmt(inner, deferred, depth + 1);
                }
                if let Some(handler) = &try_statement.handler {
                    for inner in &handler.body.body {
                        self.walk_stmt(inner, deferred, depth + 1);
                    }
                }
                if let Some(finalizer) = &try_statement.finalizer {
                    for inner in &finalizer.body {
                        self.walk_stmt(inner, deferred, depth + 1);
                    }
                }
            }
            Statement::SwitchStatement(switch_statement) => {
                self.walk_expr(&switch_statement.discriminant, deferred, depth + 1);
                for case in &switch_statement.cases {
                    if let Some(test) = &case.test {
                        self.walk_expr(test, deferred, depth + 1);
                    }
                    for inner in &case.consequent {
                        self.walk_stmt(inner, deferred, depth + 1);
                    }
                }
            }
            Statement::ReturnStatement(return_statement) => {
                // `return <call>().then(callback)` -- see `then_continuation`'s doc: a resolved
                // promise chain returned from a test body is what Vitest itself awaits, so this
                // canonicalizes to the same event shape a bare `await <call>(); <callback body>`
                // would produce, rather than the ordinary `return` + `call:...then` + a `deferred-`
                // callback body this would otherwise collect.
                if let Some(Expression::CallExpression(call)) = &return_statement.argument {
                    if let Some((awaited, continuation)) = Self::then_continuation(call) {
                        self.push(deferred, "await".to_owned());
                        self.walk_expr(awaited, deferred, depth + 1);
                        for inner in continuation {
                            self.walk_stmt(inner, deferred, depth + 1);
                        }
                        return;
                    }
                }
                self.push(deferred, "return".to_owned());
                if let Some(argument) = &return_statement.argument {
                    self.walk_expr(argument, deferred, depth + 1);
                }
            }
            Statement::ThrowStatement(throw_statement) => {
                self.push(deferred, "throw".to_owned());
                self.walk_expr(&throw_statement.argument, deferred, depth + 1);
            }
            Statement::LabeledStatement(labeled) => {
                self.walk_stmt(&labeled.body, deferred, depth + 1)
            }
            Statement::FunctionDeclaration(function) => {
                if let Some(function_body) = &function.body {
                    for inner in &function_body.statements {
                        self.walk_stmt(inner, true, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }

    /// Collect expression behavior while distinguishing enclosing execution from deferred
    /// nested callables.
    pub(super) fn walk_expr(&mut self, expr: &Expression<'a>, deferred: bool, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        match expr {
            Expression::CallExpression(call) => self.walk_call(call, deferred, depth + 1),
            Expression::ClassExpression(class) => {
                self.walk_class_definition(class, deferred, depth + 1)
            }
            Expression::NewExpression(new_expression) => {
                self.walk_new(expr, new_expression, deferred, depth + 1)
            }
            Expression::AwaitExpression(await_expression) => {
                self.push(deferred, "await".to_owned());
                self.walk_expr(&await_expression.argument, deferred, depth + 1);
            }
            // A nested function/arrow used as a value (a callback passed elsewhere, a helper
            // assigned to a variable) is retained as syntactic evidence but not promoted to
            // directly-executed behavior: this body may never run, or may run many times, so its
            // calls and assertions are marked `deferred-` rather than treated as equivalent to
            // the enclosing test's own top-level behavior. This module treats every nested
            // function-like uniformly this way, unlike CukeDedup's narrower "only if passed as a
            // call argument" heuristic -- a deliberate simplification given how rarely a bare,
            // never-invoked nested function declaration appears in a test body.
            Expression::ArrowFunctionExpression(arrow) => match &arrow.body {
                ArrowFunctionBody::FunctionBody(function_body) => {
                    for statement in &function_body.statements {
                        self.walk_stmt(statement, true, depth + 1);
                    }
                }
                other => {
                    if let Some(inner) = other.as_expression() {
                        self.walk_expr(inner, true, depth + 1);
                    }
                }
            },
            Expression::FunctionExpression(function) => {
                if let Some(function_body) = &function.body {
                    for statement in &function_body.statements {
                        self.walk_stmt(statement, true, depth + 1);
                    }
                }
            }
            Expression::StaticMemberExpression(member) => {
                self.walk_member_value(member, deferred, depth + 1)
            }
            Expression::ComputedMemberExpression(member) => {
                self.walk_expr(&member.object, deferred, depth + 1);
                self.walk_expr(&member.expression, deferred, depth + 1);
            }
            Expression::PrivateFieldExpression(member) => {
                self.walk_expr(&member.object, deferred, depth + 1)
            }
            Expression::BinaryExpression(binary) => {
                self.walk_expr(&binary.left, deferred, depth + 1);
                self.walk_expr(&binary.right, deferred, depth + 1);
            }
            Expression::LogicalExpression(logical) => {
                self.walk_expr(&logical.left, deferred, depth + 1);
                self.walk_expr(&logical.right, deferred, depth + 1);
            }
            Expression::UnaryExpression(unary) => {
                self.walk_expr(&unary.argument, deferred, depth + 1)
            }
            Expression::ConditionalExpression(conditional) => {
                self.walk_expr(&conditional.test, deferred, depth + 1);
                self.walk_expr(&conditional.consequent, deferred, depth + 1);
                self.walk_expr(&conditional.alternate, deferred, depth + 1);
            }
            Expression::AssignmentExpression(assignment) => {
                let target = self.assignment_target_shape(&assignment.left, depth + 1);
                let value = self.shape(&assignment.right, Mode::Alpha, depth + 1);
                self.push(
                    deferred,
                    format!(
                        "assign:{} target={target} value={value}",
                        assignment.operator.as_str()
                    ),
                );
            }
            Expression::UpdateExpression(update) => {
                let target = self.simple_assignment_target_shape(&update.argument, depth + 1);
                self.push(
                    deferred,
                    format!(
                        "update:{}{} target={target}",
                        if update.prefix { "prefix" } else { "postfix" },
                        update.operator.as_str()
                    ),
                );
            }
            Expression::SequenceExpression(sequence) => {
                for inner in &sequence.expressions {
                    self.walk_expr(inner, deferred, depth + 1);
                }
            }
            Expression::TemplateLiteral(template) => {
                for expression in &template.expressions {
                    self.walk_expr(expression, deferred, depth + 1);
                }
            }
            Expression::ArrayExpression(array) => {
                for element in &array.elements {
                    if let Some(inner) = element.as_expression() {
                        self.walk_expr(inner, deferred, depth + 1);
                    }
                }
            }
            Expression::ObjectExpression(object) => {
                for property in &object.properties {
                    if let ObjectPropertyKind::ObjectProperty(object_property) = property {
                        self.walk_expr(&object_property.value, deferred, depth + 1);
                    }
                }
            }
            Expression::ParenthesizedExpression(parenthesized) => {
                self.walk_expr(&parenthesized.expression, deferred, depth + 1)
            }
            Expression::TSAsExpression(as_expression) => {
                self.walk_expr(&as_expression.expression, deferred, depth + 1)
            }
            Expression::TSSatisfiesExpression(satisfies) => {
                self.walk_expr(&satisfies.expression, deferred, depth + 1)
            }
            Expression::TSNonNullExpression(non_null) => {
                self.walk_expr(&non_null.expression, deferred, depth + 1)
            }
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => self.walk_call(call, deferred, depth + 1),
                ChainElement::TSNonNullExpression(non_null) => {
                    self.walk_expr(&non_null.expression, deferred, depth + 1)
                }
                ChainElement::StaticMemberExpression(member) => {
                    self.walk_member_value(member, deferred, depth + 1)
                }
                ChainElement::ComputedMemberExpression(member) => {
                    self.walk_expr(&member.object, deferred, depth + 1);
                    self.walk_expr(&member.expression, deferred, depth + 1);
                }
                ChainElement::PrivateFieldExpression(member) => {
                    self.walk_expr(&member.object, deferred, depth + 1)
                }
            },
            // A bare identifier is not itself behavior -- except when it is the single read of a
            // local whose declaration `walk_stmt` skipped as fully inlined, in which case the
            // initializer's own events belong *here*. See `walk_inlined_read`.
            Expression::Identifier(ident) => self.walk_inlined_read(ident, deferred, depth),
            _ => {}
        }
    }

    /// Walks a static member expression standing in **value** position -- a statement of its own,
    /// an operand, an argument, an initializer: anywhere the member access is the finished
    /// expression rather than the thing about to be called.
    ///
    /// A member access is not behavior, so ordinarily this is just a descent into the object. The
    /// exception is the chai-style property assertion (see [`Self::property_chain_event`]), whose
    /// entire meaning is the member names this descent would otherwise walk straight past. When
    /// the chain is recognized the event is emitted and the walk resumes at the base
    /// `expect(...)` call, **skipping the member links in between**: descending into
    /// `member.object` instead would re-enter here on `expect(spy).to.not.have.been` and emit a
    /// shortened chain event for every remaining link, so one assertion would contribute as many
    /// events as it has properties.
    ///
    /// Value position is the whole condition. A member chain in *callee* position reaches
    /// [`Self::walk_under_members`] instead, which descends past the links without consulting
    /// this: an `expect(x).to.have.property('a')` already emits
    /// `call:global:expect.to.have.property` carrying those same names, and emitting a chain
    /// event for its `expect(x).to.have` prefix would count one assertion twice.
    ///
    /// A *declined* chain descends the same way, and for a sharper reason: re-entering here on
    /// the object would offer this recognizer every proper prefix of a chain it has already
    /// refused, and a prefix can qualify where the whole does not. `expect(x).to['be'].ok` would
    /// have reported `assert:chain:expect.to`, and a chain one link past
    /// [`MAX_ASSERTION_CHAIN`] would have reported the longest accepted prefix of itself -- a
    /// bound that truncates instead of declining. A chain is recognized whole or not at all.
    fn walk_member_value(
        &mut self,
        member: &StaticMemberExpression<'a>,
        deferred: bool,
        depth: u32,
    ) {
        if let Some((event, base_call)) = self.property_chain_event(member, depth) {
            self.push(deferred, event);
            self.walk_call(base_call, deferred, depth + 1);
            return;
        }
        self.walk_under_members(&member.object, deferred, depth);
    }

    /// Walks whatever sits *underneath* a member chain, emitting nothing for the links
    /// themselves. Reached from the two positions where a chain's member names must not be
    /// offered to [`Self::property_chain_event`]: the callee of a call, whose own `call:` event
    /// already names them, and the object of a member access this recognizer declined.
    ///
    /// This is what `walk_expr` did by construction before [`Self::walk_member_value`] existed:
    /// every member arm it could reach only ever recursed into the object, so a chain was
    /// already descended without emitting anything of its own. Spelling that descent out here
    /// keeps it that way now that a static member access in value position can emit an event,
    /// and keeps the [`MAX_DEPTH`] accounting identical to the recursion it replaces -- one level
    /// per link, degrading at the same depth.
    pub(super) fn walk_under_members(
        &mut self,
        callee: &Expression<'a>,
        deferred: bool,
        depth: u32,
    ) {
        let mut current = callee;
        let mut depth = depth;
        loop {
            if depth > MAX_DEPTH {
                return;
            }
            current = match current {
                Expression::StaticMemberExpression(member) => &member.object,
                Expression::PrivateFieldExpression(member) => &member.object,
                Expression::ComputedMemberExpression(member) => {
                    self.walk_expr(&member.expression, deferred, depth + 1);
                    &member.object
                }
                Expression::ParenthesizedExpression(parenthesized) => &parenthesized.expression,
                Expression::TSNonNullExpression(non_null) => &non_null.expression,
                Expression::TSAsExpression(as_expression) => &as_expression.expression,
                Expression::TSSatisfiesExpression(satisfies) => &satisfies.expression,
                other => return self.walk_expr(other, deferred, depth),
            };
            depth += 1;
        }
    }

    /// Re-emits a single-use local's inlined initializer at its one **read** site.
    ///
    /// `walk_stmt` skips a fully-inlined declarator's initializer (see `declarator_fully_inlined`)
    /// on the premise that the read renders it instead. That premise held for exactly one read
    /// position -- an assertion's subject or expected value, which `shape` renders through a
    /// `Serializer` that consults [`DataflowSubst`]. Every other read position (a call argument, a
    /// fluent receiver, a `return` operand) reaches this collector's own traversal, where a bare
    /// `Expression::Identifier` emits nothing at all, so the initializer's `call:` events were
    /// dropped outright and two tests building genuinely different subjects collided on an equal
    /// `behavior` multiset -- a real `near-duplicate-test` false positive, distilled in
    /// `fixtures/observed/observed-single-use-local-erases-behavior.test.ts`.
    ///
    /// Re-emitting **at the read site**, rather than un-skipping the declaration, is what keeps
    /// the two forms this canonicalization exists to equate actually equal. The whole point of
    /// single-use inlining is that `const x = f(); g(x)` must look like `g(f())`, and `g(f())`
    /// emits `call:g` *then* `call:f` -- the enclosing call first, its argument second. Walking
    /// the initializer back at the declaration would emit `call:f` first, restoring the multiset
    /// but inverting the sequence; `near-duplicate-test` keys on a sorted multiset and would not
    /// notice, but `subsumed-test` keys on an ordered subsequence and would stop matching the pair
    /// it matches today. It would also over-correct in the assertion-subject position, where the
    /// inlined body emits *no* event for the initializer at all (`walk_call` treats a whole
    /// assertion as one atomic event and never descends into its subject), so an extra `call:f`
    /// from the declaration would split `fixtures/duplicates/near-extracted-local-positive.test.ts`
    /// apart. Emitting at the read site gets both right for free: a read inside an assertion
    /// subject is never reached (the assertion already returned), and a read anywhere else lands
    /// in exactly the position the inlined expression would have occupied.
    ///
    /// Only [`DataflowSubst::Inline`] is re-emitted. A [`DataflowSubst::Member`] read stands for
    /// `object.property`, and a member read of a free object contributes no event in either form;
    /// a [`DataflowSubst::DestructuredProperty`] read stands for the local itself, which was never
    /// an event either -- that entry only decorates how the local *renders* where it already
    /// appears.
    fn walk_inlined_read(&mut self, ident: &IdentifierReference<'a>, deferred: bool, depth: u32) {
        // Copy the map reference out of `self` first: it is a `'m` borrow independent of this
        // `&mut self`, so holding an entry from it across the `walk_expr` call below is fine.
        let dataflow = self.ctx.dataflow;
        let Some(symbol_id) = ident.reference_id.get().and_then(|reference_id| {
            self.ctx
                .semantic
                .scoping()
                .get_reference(reference_id)
                .symbol_id()
        }) else {
            return;
        };
        if let Some(DataflowSubst::Inline { init, .. }) = dataflow.get(&symbol_id) {
            self.walk_expr(init, deferred, depth + 1);
        }
    }

    /// Render an alpha assignment target and retain the serializer's canonical-binding updates.
    fn assignment_target_shape(&mut self, target: &AssignmentTarget<'a>, depth: u32) -> String {
        let mut serializer = self.shape_serializer(Mode::Alpha);
        let result = serializer.ser_assignment_target(target, depth);
        self.ctx.canonical = serializer.ctx.canonical;
        self.ctx.next_local = serializer.ctx.next_local;
        result
    }

    /// Render a simple alpha assignment target and preserve the updated canonical-binding
    /// state.
    fn simple_assignment_target_shape(
        &mut self,
        target: &SimpleAssignmentTarget<'a>,
        depth: u32,
    ) -> String {
        let mut serializer = self.shape_serializer(Mode::Alpha);
        let result = serializer.ser_simple_assignment_target(target, depth);
        self.ctx.canonical = serializer.ctx.canonical;
        self.ctx.next_local = serializer.ctx.next_local;
        result
    }
}
