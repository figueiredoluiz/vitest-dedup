use super::*;

impl<'a, 'b, 's, 'm> BehaviorCollector<'a, 'b, 's, 'm> {
    /// Collect only definition-time effects; inert methods do not increase substance counts.
    pub(super) fn walk_class_definition(
        &mut self,
        class: &oxc_ast::ast::Class<'a>,
        deferred: bool,
        depth: u32,
    ) {
        let mut serializer =
            Serializer::new(self.ctx.fresh_for_pass_with(self.ctx.dataflow), Mode::Alpha);
        serializer.ctx.canonical = self.ctx.canonical.clone();
        serializer.ctx.next_local = self.ctx.next_local;
        if let Some(event) = serializer.class_definition_effects(class, depth + 1) {
            self.push(deferred, event);
        }
        self.ctx.canonical = serializer.ctx.canonical;
        self.ctx.next_local = serializer.ctx.next_local;
    }

    /// Retain constructor identity and argument shapes, then collect their nested effects.
    pub(super) fn walk_new(
        &mut self,
        expression: &Expression<'a>,
        new_expression: &NewExpression<'a>,
        deferred: bool,
        depth: u32,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        let shape = self.shape(expression, Mode::Alpha, depth + 1);
        self.push(deferred, format!("new:{shape}"));
        self.walk_under_members(&new_expression.callee, deferred, depth + 1);
        self.walk_arguments(&new_expression.arguments, deferred, depth + 1);
    }

    /// Collect one atomic assertion event or ordinary call behavior, respecting nesting and
    /// deferred execution.
    pub(super) fn walk_call(&mut self, call: &CallExpression<'a>, deferred: bool, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        if let Some(event) = self.assertion_event(call, depth) {
            self.push(deferred, event);
            // The whole assertion is treated as one atomic behavior event, matching CukeDedup's
            // `assertion_behavior_event`: its subject and expected value are already captured in
            // the event string, so descending further would count the subject's own calls (e.g.
            // `page.locator(...)` inside `expect(page.locator(...)).toBeVisible()`) a second time.
            return;
        }
        let path = self.call_path(&call.callee, depth + 1);
        // `near-duplicate-test` and `subsumed-test` key on these events, not on `alpha`, so the
        // type arguments have to reach the event text too: without them
        // `assertType<string>(value)` and `assertType<number>(value)` emit the identical
        // `call:global:assertType` and the two rules cannot tell one type-level assertion from
        // another even though `alpha` now can.
        let type_arguments =
            type_argument_fragment(self.ctx.source, call.type_arguments.as_deref());
        let arguments = self.call_argument_fragment(call, depth);
        self.push(deferred, format!("call:{path}{type_arguments}{arguments}"));
        self.walk_under_members(&call.callee, deferred, depth + 1);
        self.walk_arguments(&call.arguments, deferred, depth + 1);
    }

    /// Collect call and constructor argument effects, retaining spread and traversal depth.
    fn walk_arguments(&mut self, arguments: &[Argument<'a>], deferred: bool, depth: u32) {
        for argument in arguments {
            if let Some(inner) = argument.as_expression() {
                self.walk_expr(inner, deferred, depth);
            } else if let Argument::SpreadElement(spread) = argument {
                self.walk_expr(&spread.argument, deferred, depth);
            }
        }
    }

    /// The data arguments of a call, rendered into its behavior event.
    ///
    /// Without these, `wrapper.find('.name-text')` and `wrapper.find('.unit-value')` emit the
    /// identical `call:global:wrapper.find`, and two tests querying different things carry the
    /// same behavior multiset -- a `near-duplicate-test` false positive on tests that do
    /// different work. Measured on a 24,147-test codebase, that erasure was the largest single
    /// cause of the rule's 27% false-positive rate.
    ///
    /// This is the same argument [`type_argument_fragment`] already makes one level up: the two
    /// rules that key on `behavior` cannot tell apart what the event text does not mention.
    ///
    /// Three decisions, each costing discrimination rather than risking a wrong match:
    ///
    /// * Rendered through [`BehaviorCollector::shape`] in [`Mode::Alpha`], so the dataflow
    ///   canonicalization applies. That is what keeps single-use local inlining working:
    ///   `const sel = '.a'; find(sel)` renders its argument as `'.a'`, exactly like `find('.a')`,
    ///   and the two stay near-duplicates as the rule intends. `Alpha` rather than `Structural`
    ///   because the *value* is the discriminating part -- `Structural` would render both
    ///   selectors as `<string>` and change nothing.
    /// * A function argument renders as its normalized callable shape. Mock/setup helpers often
    ///   take a callback whose returned object is the branch being tested; if every callback is
    ///   just `fn`, `registerEndpoint('/x', () => ({type: 'a'}))` and the same endpoint returning
    ///   `{type: 'b'}` collapse before any later assertion can tell them apart.
    /// * A spread renders as its argument shape with an explicit `...` prefix. The value is still
    ///   available here, and erasing it would make `render(...adminProps)` and
    ///   `render(...guestProps)` indistinguishable.
    ///
    /// JSON supplies an unambiguous boundary between serialized arguments. A plain join would let
    /// an argument containing the separator forge another argument list, which could manufacture
    /// equality in the two rules that trust these event strings.
    fn call_argument_fragment(&mut self, call: &CallExpression<'a>, depth: u32) -> String {
        if call.arguments.is_empty() {
            return String::new();
        }
        let shapes: Vec<String> = call
            .arguments
            .iter()
            .map(|argument| match argument {
                Argument::SpreadElement(spread) => {
                    format!(
                        "...{}",
                        self.shape(&spread.argument, Mode::Alpha, depth + 1)
                    )
                }
                _ => match argument.as_expression() {
                    // A nested call renders in full, deliberately. Collapsing it to a
                    // placeholder was tried and loses the case that matters most: a
                    // project-owned helper whose argument is a *built subject*
                    // (`expectMethodMatch(z.string())` against
                    // `expectMethodMatch(z.string().min(5))`) then emits the identical event for
                    // both, and every refinement is reported as subsuming the primitive it
                    // refines. The duplication with the argument's own event is the price of
                    // telling those two apart, and `shape` renders an inlined single-use local
                    // exactly as it renders the call written inline, so the extraction
                    // equivalence survives it.
                    Some(expression) => self.shape(expression, Mode::Alpha, depth + 1),
                    None => "?".to_owned(),
                },
            })
            .collect();
        format!(
            " args={}",
            serde_json::to_string(&shapes).expect("argument shapes serialize infallibly")
        )
    }

    /// Flattens a member/call chain into a canonical dotted path.
    ///
    /// A fluent call such as `page.locator(x).click()` produces the event
    /// `call:global:page.locator.click` for the outer call -- unlike CukeDedup's
    /// `static_call_receiver`, which deliberately drops an intermediate call's own method name
    /// from the receiver chain (relying on that inner call getting its own separate event when
    /// the traversal reaches it), this keeps the full fluent path on the outer event, since oxc's
    /// typed `CallExpression` makes recursing through `callee` uniform regardless of nesting.
    /// `walk_call`'s subsequent walk into `call.callee` still visits the inner `CallExpression`
    /// separately, so `page.locator(x)` also gets its own `call:global:page.locator` event -- the
    /// two are additive, not a replacement for each other. Either way the outer event is
    /// "recognizably about `.click`", which is what callers need to spot a fluent action.
    /// The root identifier of a call path, carrying a destructured binding's property key when it
    /// has one. This is the callee-position half of [`DataflowSubst::DestructuredProperty`]:
    /// without it
    /// `const { fetchUser } = useApi(); await fetchUser(1)` and
    /// `const { deleteUser } = useApi(); await deleteUser(1)` both emit `call:L0(<number:1>)`,
    /// the same erasure the read position had, one event kind over. It reaches receivers too --
    /// `const { api } = useThing(); api.get()` renders `call:L0:prop:api.get` -- because
    /// `call_path` recurses through a member expression's object into this same arm.
    fn path_identity(&mut self, ident: &IdentifierReference<'a>) -> String {
        let mut serializer = self.shape_serializer(Mode::Alpha);
        if let Some(value) = serializer
            .class_reference(ident, 0)
            .or_else(|| serializer.class_instance_reference(ident, 0))
        {
            self.ctx.canonical = serializer.ctx.canonical;
            self.ctx.next_local = serializer.ctx.next_local;
            return value;
        }
        self.ctx.canonical = serializer.ctx.canonical;
        self.ctx.next_local = serializer.ctx.next_local;
        let substitution = ident
            .reference_id
            .get()
            .and_then(|reference_id| {
                self.ctx
                    .semantic
                    .scoping()
                    .get_reference(reference_id)
                    .symbol_id()
            })
            .and_then(|symbol_id| self.ctx.dataflow.get(&symbol_id).cloned());
        match substitution {
            Some(DataflowSubst::Member { object, property }) => {
                format!("{}.{}", self.call_path(object, 0), property)
            }
            Some(DataflowSubst::DestructuredProperty { property }) => {
                format!("{}:prop:{property}", self.ctx.reference(ident))
            }
            Some(DataflowSubst::Inline { .. }) | None => self.ctx.reference(ident),
        }
    }

    /// Render an anchored callee path, retaining raw source for unsupported callee forms or
    /// when the depth limit is exceeded.
    fn call_path(&mut self, callee: &Expression<'a>, depth: u32) -> String {
        if depth > MAX_DEPTH {
            return self.ctx.slice(callee.span()).to_owned();
        }
        match callee {
            Expression::Identifier(ident) => self.path_identity(ident),
            Expression::ThisExpression(_) => "this".to_owned(),
            Expression::Super(_) => "super".to_owned(),
            Expression::ParenthesizedExpression(parenthesized) => {
                self.call_path(&parenthesized.expression, depth + 1)
            }
            Expression::TSAsExpression(as_expression) => {
                self.call_path(&as_expression.expression, depth + 1)
            }
            Expression::TSNonNullExpression(non_null) => {
                self.call_path(&non_null.expression, depth + 1)
            }
            Expression::StaticMemberExpression(member) => {
                format!(
                    "{}.{}",
                    self.call_path(&member.object, depth + 1),
                    member.property.name.as_str()
                )
            }
            Expression::ComputedMemberExpression(member) => {
                format!("{}[]", self.call_path(&member.object, depth + 1))
            }
            Expression::PrivateFieldExpression(member) => {
                format!(
                    "{}.#{}",
                    self.call_path(&member.object, depth + 1),
                    member.field.name.as_str()
                )
            }
            // An inner call in a fluent chain contributes its own type arguments to the outer
            // path, so `expectTypeOf<A>().toEqualTypeOf<B>()` and `expectTypeOf<C>().toEqualTypeOf<B>()`
            // produce different outer events rather than relying solely on the separate event
            // `walk_call` emits for the inner call.
            Expression::CallExpression(call) => format!(
                "{}{}",
                self.call_path(&call.callee, depth + 1),
                type_argument_fragment(self.ctx.source, call.type_arguments.as_deref())
            ),
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => format!(
                    "{}{}",
                    self.call_path(&call.callee, depth + 1),
                    type_argument_fragment(self.ctx.source, call.type_arguments.as_deref())
                ),
                ChainElement::TSNonNullExpression(non_null) => {
                    self.call_path(&non_null.expression, depth + 1)
                }
                ChainElement::StaticMemberExpression(member) => {
                    format!(
                        "{}.{}",
                        self.call_path(&member.object, depth + 1),
                        member.property.name.as_str()
                    )
                }
                ChainElement::ComputedMemberExpression(member) => {
                    format!("{}[]", self.call_path(&member.object, depth + 1))
                }
                ChainElement::PrivateFieldExpression(member) => {
                    format!(
                        "{}.#{}",
                        self.call_path(&member.object, depth + 1),
                        member.field.name.as_str()
                    )
                }
            },
            _ => format!("<{}>", self.ctx.slice(callee.span())),
        }
    }

    /// Recognizes the `expect(...)` call an assertion chain hangs off, and reports how it was
    /// spelled: `""` for a bare `expect(subject)`, `"soft"` or `"poll"` for
    /// `expect.soft(subject)` / `expect.poll(subject)`. `None` for any other call, including a
    /// local shadowing the name (see [`is_expect_identifier`]) and `expect.<anything else>(...)`.
    ///
    /// Shared by the two chain recognizers -- [`Self::assertion_event`] for the call-terminated
    /// form and [`Self::property_chain_event`] for the chai property form -- so that a new
    /// spelling of the base is taught to both at once. The base is the half the two forms agree
    /// on; what follows it is the half they differ on, and each keeps its own loop for that.
    fn expect_base(&self, call: &CallExpression<'a>) -> Option<&'static str> {
        match &call.callee {
            Expression::Identifier(ident) => {
                is_expect_identifier(self.ctx.semantic, self.ctx.body_span, ident).then_some("")
            }
            Expression::StaticMemberExpression(member) => {
                let spelling = match member.property.name.as_str() {
                    "soft" => "soft",
                    "poll" => "poll",
                    _ => return None,
                };
                match &member.object {
                    Expression::Identifier(ident)
                        if is_expect_identifier(self.ctx.semantic, self.ctx.body_span, ident) =>
                    {
                        Some(spelling)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Recognizes a chai-style property-chain assertion -- `expect(spy).to.not.have.been.calledOnce`
    /// -- and returns its event text together with the `expect(...)` call the chain hangs off.
    ///
    /// The whole meaning of such an assertion lives in member names: there is no call node after
    /// the `expect(...)`, so [`Self::walk_call`] never sees it and [`Self::assertion_event`] never
    /// runs. Before this existed the chain contributed *nothing* to `behavior`, and
    /// `expect(spy).to.not.have.been.calledOnce` against `expect(spy).to.have.been.calledTwice` --
    /// opposite polarity and a different assertion -- compared as the same behavior events in the
    /// same order, because all that reached the trace was the shared `vi.fn(); spy(); spy()`
    /// setup. That was a `near-duplicate-test` false positive adjudicated on `vitest`'s own
    /// `test/unit/test/chai-style-assertions.test.ts` (:111 against :419), distilled in
    /// `fixtures/duplicates/near-chai-property-chain-differs.test.ts`.
    ///
    /// The event is **additive**: the caller still walks the base `expect(...)` call afterwards,
    /// so every event a chai-style body produced before is produced now too and only the chain
    /// event is new. That is deliberate. Making the chain atomic the way `assertion_event` is
    /// would have dropped the `call:` event carrying the subject, and the subject is rendered in
    /// `Mode::Alpha` there -- literals and all -- where an atomic assertion event renders its
    /// subject in `Mode::Structural`. Two chains that differ only in a subject literal
    /// (`expect(build(1)).to.be.ok` against `expect(build(2)).to.be.ok`) would then have
    /// collapsed, trading a fixed false positive for a new one. For the same reason the event
    /// carries no `subject=` of its own: the `call:` event emitted immediately after it already
    /// carries the subject, more precisely than this event could.
    ///
    /// The chain is recorded **verbatim**, chai's no-op language links (`to`, `be`, `been`,
    /// `have`, ...) included. Normalizing them away would equate `to.be.true` with `is.true` and
    /// win a handful of duplicate reports, but it needs a hardcoded vocabulary of one assertion
    /// library's filler words, and any word missing from that list would silently become
    /// load-bearing. Keeping every link costs recall on chains spelled differently and cannot
    /// cost precision, which is the side of that trade this defect is on. `not` needs no special
    /// handling under this rule: it is a link like any other, so polarity travels for free.
    ///
    /// `None` for anything that is not rooted in an `expect(...)` call, and for a chain longer
    /// than [`MAX_ASSERTION_CHAIN`] links. The root must be the call *syntactically*: a chain
    /// hung off a local (`const e = expect(x); e.to.be.ok`) is declined even when
    /// [`DataflowSubst::Inline`] would substitute that local elsewhere, because this walk reads
    /// the member chain directly rather than through a [`Serializer`]. That is the ordinary
    /// under-matching this module biases toward -- the chain contributes no event, exactly as
    /// before -- not a wrong one.
    pub(super) fn property_chain_event<'x>(
        &self,
        member: &'x StaticMemberExpression<'a>,
        depth: u32,
    ) -> Option<(String, &'x CallExpression<'a>)> {
        if depth > MAX_DEPTH {
            return None;
        }
        let mut links = vec![member.property.name.as_str()];
        let base_call = unwind_member_chain(&member.object, &mut links)?;
        let qualifier = match self.expect_base(base_call)? {
            "" => "expect".to_owned(),
            spelling => format!("expect.{spelling}"),
        };
        links.reverse();
        Some((
            format!("assert:chain:{qualifier}.{}", links.join(".")),
            base_call,
        ))
    }

    /// Recognizes `expect(subject)[.not][.resolves|.rejects].matcher(...expected)`, including
    /// `expect.soft`/`expect.poll`. Returns `None` for anything else, in which case the call is
    /// treated as an ordinary call event instead.
    fn assertion_event(&mut self, call: &CallExpression<'a>, depth: u32) -> Option<String> {
        let member = match &call.callee {
            Expression::StaticMemberExpression(member) => member,
            _ => return None,
        };
        let matcher = member.property.name.as_str();
        let mut links = Vec::new();
        let base_call = unwind_member_chain(&member.object, &mut links)?;
        // Unlike a chai property chain, the links here are a closed set: anything else between
        // `expect(...)` and the matcher means this is not an assertion at all, and the call falls
        // through to an ordinary call event. Validated after the unwind rather than during it,
        // which changes nothing -- either way an unrecognized link declines the whole chain.
        let mut modifiers: Vec<&'static str> = Vec::with_capacity(links.len());
        for name in links.iter().rev() {
            modifiers.push(match *name {
                "not" => "not",
                "resolves" => "resolves",
                "rejects" => "rejects",
                _ => return None,
            });
        }
        match self.expect_base(base_call)? {
            "" => {}
            spelling => modifiers.insert(0, spelling),
        }
        let subject = base_call
            .arguments
            .first()
            .and_then(|argument| argument.as_expression());
        let subject_shape = subject.map_or_else(
            || "()".to_owned(),
            |expression| self.shape(expression, Mode::Structural, depth + 1),
        );
        let value_parts: Vec<String> = call
            .arguments
            .iter()
            .filter_map(|argument| argument.as_expression())
            .map(|expression| self.shape(expression, Mode::Alpha, depth + 1))
            .collect();
        let qualifier = if modifiers.is_empty() {
            "expect".to_owned()
        } else {
            format!("expect.{}", modifiers.join("."))
        };
        // Both halves of the chain can carry explicit type arguments (`expect<T>(x)` and the
        // matcher's own `.toEqualTypeOf<U>()`), and each is attached to the half that wrote it.
        let subject_type_arguments =
            type_argument_fragment(self.ctx.source, base_call.type_arguments.as_deref());
        let matcher_type_arguments =
            type_argument_fragment(self.ctx.source, call.type_arguments.as_deref());
        Some(format!(
            "assert:{qualifier}{subject_type_arguments}.{matcher}{matcher_type_arguments}\
             (subject={subject_shape}; value={})",
            value_parts.join(", ")
        ))
    }

    /// Renders an assertion's subject or expected value using the same identifier/literal
    /// handling as the main serializer, without threading a second `Serializer` instance through
    /// the whole traversal. The temporarily-built `Serializer` borrows this collector's own
    /// canonical map and counter and hands them back afterward, so canonical names assigned while
    /// rendering an assertion's shape are visible to later assertions in the same body too.
    pub(super) fn shape_serializer(&mut self, mode: Mode) -> Serializer<'a, 'b, 's, 'm> {
        let mut ctx = self.ctx.fresh_for_pass();
        // The canonical names this collector assigned while rendering an earlier event's shape
        // must stay visible to later ones, so the temporary carries this collector's own state
        // and `shape` below is the return leg.
        ctx.canonical = std::mem::take(&mut self.ctx.canonical);
        ctx.next_local = self.ctx.next_local;
        ctx.expand_classes = true;
        Serializer {
            ctx,
            mode,
            // This serializer renders one behavior event's shape, not a whole callable. Any `this`
            // it meets is attributed by the `ser_callable` frame that owns this body, so the flag
            // starts clean here and is discarded with the temporary.
            this_or_arguments: false,
            outermost_this_dependent: false,
        }
    }

    /// Render an expression in the requested mode and merge canonical-binding state back into
    /// the collector.
    pub(super) fn shape(&mut self, expr: &Expression<'a>, mode: Mode, depth: u32) -> String {
        let mut serializer = self.shape_serializer(mode);
        let result = serializer.ser_expr(expr, depth);
        self.ctx.canonical = serializer.ctx.canonical;
        self.ctx.next_local = serializer.ctx.next_local;
        result
    }
}
