use super::*;

// ---------------------------------------------------------------------------------------------
// alpha / structural serialization
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
/// Which of the two exact-equality views a serialization pass produces. `pub(crate)` only so
/// `helpers.rs` can thread it through to [`render_inlined_helper_body`]; no other module matches
/// on it.
pub(crate) enum Mode {
    /// Locals renamed by symbol; literals preserved.
    Alpha,
    /// Locals renamed by symbol; literals collapsed to their kind.
    Structural,
}

/// The file-level construction context and canonicalization state that [`Serializer`] and
/// [`BehaviorCollector`] share verbatim.
///
/// The two traversals answer different questions over the same body -- one renders a whole
/// callable as text, the other collects ordered behavior events -- but everything they know
/// about the *file* is identical, and so is the canonical-naming state both must advance for
/// `behavior`'s shapes and `alpha`'s text to agree on what a symbol is called. This struct exists
/// because the two used to repeat ten fields and two methods, which is one construct handled
/// twice, in two places that can drift apart.
pub(super) struct FingerprintContext<'a, 'b, 's, 'm> {
    pub(super) semantic: &'b Semantic<'a>,
    pub(super) source: &'s str,
    pub(super) imports: &'m ImportOrigins,
    /// The fingerprinted file's own path, rendered by [`path_key`]. Owned rather than borrowed:
    /// it is cheap (one small string per body), and avoids yet another lifetime parameter for a
    /// value every caller already has to compute once per `fingerprint` call.
    pub(super) file_key: String,
    /// The fingerprinted file's own path, unrendered. `file_key` is the *display* form; this is
    /// what [`specifier_identity`] needs to resolve a specifier relative to the importer, and a
    /// dynamic `import()` is the one place inside a body that carries one.
    pub(super) file: PathBuf,
    pub(super) body_span: Span,
    /// The callable's own node, the root of the run [`Self::collect_raw_sites`] scans.
    pub(super) body_node: NodeId,
    pub(super) class_uncertain: std::rc::Rc<std::cell::Cell<bool>>,
    /// What a raw-text fallback needs to know about the body, built on first use and shared by
    /// the passes of one `fingerprint` call. See [`Self::raw_slice_binds_this`] and
    /// [`Self::raw_slice_references`].
    pub(super) raw_sites: std::rc::Rc<std::cell::OnceCell<RawSites<'a>>>,
    pub(super) expand_classes: bool,
    pub(super) class_regions: Vec<Span>,
    pub(super) capture_stack: Vec<SymbolId>,
    pub(super) private_names: Vec<HashMap<String, String>>,
    pub(super) class_cache: std::rc::Rc<std::cell::RefCell<classes::ClassCache>>,
    pub(super) capture_frames: Vec<Vec<SymbolId>>,
    pub(super) class_uses: Vec<u32>,
    /// This row's substitution, already resolved to `SymbolId`s by [`resolve_row_substitution`] --
    /// empty (never populated) when `fingerprint` was not given a `.each` row substitution, or
    /// when this body's parameter shape did not qualify for one. See the module doc's "Row
    /// substitution" section; consulted only by [`Serializer::reference_or_literal`].
    pub(super) row_substitution: &'m HashMap<SymbolId, (&'static str, String)>,
    /// This body's dataflow canonicalization, computed once by [`analyze_dataflow`]. Empty for
    /// every `Serializer` `fingerprint` constructs directly (`alpha`/`structural`); only
    /// [`BehaviorCollector::shape`]'s temporary `Serializer` is ever given a populated map. See
    /// the "Dataflow canonicalization" section doc above [`DataflowSubst`]; consulted only by
    /// [`Serializer::reference_or_literal`].
    pub(super) dataflow: &'m HashMap<SymbolId, DataflowSubst<'a, 'b>>,
    /// Parameter-to-argument-fragment substitution for an inlined imported helper's body (see
    /// `helpers.rs`): the helper's parameter symbols, each mapped to the already-rendered text of
    /// the argument the call site passed. Empty for every ordinary pass -- only the `Serializer`
    /// [`render_inlined_helper_body`] constructs is ever given a populated map, and its keys are
    /// that helper file's own `SymbolId`s, never the fingerprinted body's. Consulted only by
    /// [`Serializer::reference_or_literal`], before every other substitution, on exactly the
    /// terms `row_substitution` substitutes a row's cells: a substituted read is the argument's
    /// rendering, not an `id:`-prefixed reference.
    pub(super) arg_substitution: HashMap<SymbolId, String>,
    /// The parameter symbols whose substitution [`Serializer::reference_or_literal`] has
    /// actually consumed -- reads that rendered. An inlined helper's wrapper renders every
    /// UNCONSUMED argument in an `(args ...)` tail, because an argument no read consumed is
    /// invisible to the body's text and yet still observable at runtime (`arguments[i]` indexes
    /// it): two calls differing only in an unread argument must not render alike.
    pub(super) consumed_args: HashSet<SymbolId>,
    /// Whether this pass may inline imported helper bodies at call sites (`helpers.rs`). True for
    /// the passes over a fingerprinted body; false inside an inlined helper's own rendering, so
    /// inlining is exactly one level deep -- a helper calling another imported helper renders
    /// that inner call as an ordinary `free:import` identity, which under-matches (equivalent
    /// inner helpers stay distinct) rather than recursing without a bound.
    pub(super) inline_imported_helpers: bool,
    /// Imported-helper inlining's **accounting**, recorded by the pass itself so that what
    /// extract.rs drops from `Context::imports` is what this pass *actually rendered*, never a
    /// re-derivation of the decision (an opaque-rendered region -- a class body -- hides a call
    /// from the serializer while still showing it to a reference-counting visitor, and the two
    /// disagreeing was a review-found false-merge). `inlined` collects components whose calls
    /// rendered as bodies; `identity` collects components that rendered at least once as a
    /// `free:import` identity (a refused call, a value reference, an argument read). A module is
    /// fully inlined for this body iff it is in `inlined` and not in `identity` -- see
    /// [`Self::fully_inlined_imports`].
    ///
    /// Degraded or opaque regions inside a rendered body do **not** un-report the module (a
    /// review suggestion, declined): `<opaque:...>`/`<max-depth:...>` carry byte-exact raw
    /// source, which is comparison at its strictest -- two bodies render equal through them only
    /// when the raw text is equal -- so "the content was compared directly" remains true, just
    /// by raw text instead of structure. Un-reporting would trade a real comparison for a
    /// coarser identity without removing any merge.
    pub(super) inlined_imports: HashSet<String>,
    pub(super) identity_imports: HashSet<String>,
    /// The canonical-local namespace of this pass: `L` for a pass over a fingerprinted body,
    /// `H` for an inlined helper's own rendering. Separate namespaces by construction, so a
    /// helper's own locals can never render the same name as a caller local appearing in a
    /// substituted argument fragment -- the two are numbered by different passes, and equal
    /// spellings from different allocations must not be able to make different bodies compare
    /// equal (review-hardened; the structural argument already held, the prefix removes the
    /// question).
    pub(super) local_prefix: &'static str,
    pub(super) canonical: HashMap<SymbolId, String>,
    pub(super) next_local: u32,
}

/// One identifier reference that resolves to a symbol, with where that symbol is declared.
struct RawReference<'a> {
    start: u32,
    ident: &'a IdentifierReference<'a>,
    symbol: SymbolId,
    declared: Span,
}

/// The positions a raw-text fallback searches, collected once per body. Both lists are sorted by
/// source offset so a slice finds its own entries by binary search.
#[derive(Default)]
pub(super) struct RawSites<'a> {
    /// `this`, `new.target` and a free `arguments`, each with the span of the non-arrow function
    /// that binds it (`None` when no function does).
    this_sites: Vec<(u32, Option<Span>)>,
    /// Every identifier reference that resolves to a symbol.
    references: Vec<RawReference<'a>>,
}

impl<'a, 'b, 's, 'm> FingerprintContext<'a, 'b, 's, 'm> {
    // Every parameter is a distinct piece of construction context with no natural grouping --
    // same rationale as `reference_identity`'s own `#[allow]` above.
    /// The file-level construction context from `fingerprint`'s inputs, with empty
    /// per-pass naming state.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        semantic: &'b Semantic<'a>,
        source: &'s str,
        imports: &'m ImportOrigins,
        file_key: String,
        file: PathBuf,
        body_span: Span,
        body_node: NodeId,
        row_substitution: &'m HashMap<SymbolId, (&'static str, String)>,
        dataflow: &'m HashMap<SymbolId, DataflowSubst<'a, 'b>>,
    ) -> Self {
        Self {
            semantic,
            source,
            imports,
            file_key,
            file,
            body_span,
            body_node,
            class_uncertain: std::rc::Rc::new(std::cell::Cell::new(false)),
            raw_sites: Default::default(),
            expand_classes: false,
            class_regions: Vec::new(),
            capture_stack: Vec::new(),
            private_names: Vec::new(),
            class_cache: Default::default(),
            capture_frames: Vec::new(),
            class_uses: Vec::new(),
            row_substitution,
            dataflow,
            arg_substitution: HashMap::new(),
            consumed_args: HashSet::new(),
            inline_imported_helpers: true,
            inlined_imports: HashSet::new(),
            identity_imports: HashSet::new(),
            local_prefix: "L",
            canonical: HashMap::new(),
            next_local: 0,
        }
    }

    /// A fresh pass over the same file: identical borrowed inputs, re-owned path strings, and
    /// empty canonical-naming state. Every pass must start canonical naming from zero (see
    /// [`Serializer`]'s doc for why the passes deliberately never share it), so this -- not a
    /// repeat of the construction argument list -- is how one `fingerprint` call yields three
    /// independent passes.
    pub(super) fn fresh_for_pass(&self) -> Self {
        Self {
            semantic: self.semantic,
            source: self.source,
            imports: self.imports,
            file_key: self.file_key.clone(),
            file: self.file.clone(),
            body_span: self.body_span,
            body_node: self.body_node,
            class_uncertain: self.class_uncertain.clone(),
            raw_sites: self.raw_sites.clone(),
            expand_classes: self.expand_classes,
            class_regions: self.class_regions.clone(),
            capture_stack: self.capture_stack.clone(),
            private_names: self.private_names.clone(),
            class_cache: self.class_cache.clone(),
            capture_frames: self.capture_frames.clone(),
            class_uses: self.class_uses.clone(),
            row_substitution: self.row_substitution,
            dataflow: self.dataflow,
            // Both inlining-related fields are per-pass constants in truth: an ordinary body's
            // passes never substitute helper arguments, and they all inline; an inlined helper's
            // own rendering pass is never built through here.
            arg_substitution: HashMap::new(),
            consumed_args: HashSet::new(),
            inline_imported_helpers: self.inline_imported_helpers,
            // Accounting starts empty for every pass: only the alpha pass's numbers are ever
            // reported, and only after it finishes.
            inlined_imports: HashSet::new(),
            identity_imports: HashSet::new(),
            local_prefix: "L",
            canonical: HashMap::new(),
            next_local: 0,
        }
    }

    /// [`Self::fresh_for_pass`] with `dataflow` swapped -- the one input that differs between
    /// passes, since only the behavior pass consults dataflow canonicalization (see `fingerprint`).
    pub(super) fn fresh_for_pass_with(
        &self,
        dataflow: &'m HashMap<SymbolId, DataflowSubst<'a, 'b>>,
    ) -> Self {
        let mut fresh = self.fresh_for_pass();
        fresh.dataflow = dataflow;
        fresh.expand_classes = true;
        fresh
    }

    /// The sites a raw-text fallback searches, collected on first use.
    fn raw_sites(&self) -> &RawSites<'a> {
        self.raw_sites.get_or_init(|| self.collect_raw_sites())
    }

    /// Whether the raw source under `span` uses `this`, `arguments` or `new.target` of a function
    /// that is not itself inside the text. Such a slice means different things under an arrow
    /// callback and a `function` one, and nothing else in the fingerprint can say so.
    pub(super) fn raw_slice_binds_this(&self, span: Span) -> bool {
        let sites = self.raw_sites();
        let first = sites
            .this_sites
            .partition_point(|&(start, _)| start < span.start);
        sites.this_sites[first..]
            .iter()
            .take_while(|&&(start, _)| start < span.end)
            .any(|&(_, function)| {
                function
                    .is_none_or(|function| function.start < span.start || function.end > span.end)
            })
    }

    /// The references inside the raw source under `span` to a symbol declared outside it, first
    /// occurrence of each symbol only, in source order. The caller renders them: a slice keeps
    /// the original spelling of every name, while the rest of the body is alpha-renamed, so the
    /// text alone cannot say which binding a name meant.
    pub(super) fn raw_slice_references(&self, span: Span) -> Vec<&'a IdentifierReference<'a>> {
        let sites = self.raw_sites();
        let first = sites
            .references
            .partition_point(|reference| reference.start < span.start);
        let mut seen = HashSet::new();
        sites.references[first..]
            .iter()
            .take_while(|reference| reference.start < span.end)
            .filter(|reference| {
                !(reference.declared.start >= span.start && reference.declared.end <= span.end)
            })
            .filter(|reference| seen.insert(reference.symbol))
            .map(|reference| reference.ident)
            .collect()
    }

    /// The offsets [`Self::raw_slice_binds_this`] and [`Self::raw_slice_references`] search.
    fn collect_raw_sites(&self) -> RawSites<'a> {
        let scoping = self.semantic.scoping();
        let mut sites = RawSites::default();
        // The body's nodes are one contiguous run starting at the callable's own node, and a node
        // belongs to it exactly while its parent does not precede that node. This keeps the cost
        // to the body, not the file -- a file with a thousand tests would otherwise be scanned a
        // thousand times.
        let nodes = self.semantic.nodes();
        let root = self.body_node;
        for node in nodes
            .iter_enumerated()
            .skip(root.index())
            .map_while(|(id, node)| {
                (id == root || nodes.parent_id(id).index() >= root.index()).then_some(node)
            })
        {
            let span = node.span();
            let is_this = match node.kind() {
                AstKind::ThisExpression(_) | AstKind::NewTarget(_) => true,
                AstKind::IdentifierReference(ident) => {
                    let resolved = ident
                        .reference_id
                        .get()
                        .and_then(|reference| scoping.get_reference(reference).symbol_id());
                    if let Some(symbol) = resolved {
                        sites.references.push(RawReference {
                            start: span.start,
                            ident,
                            symbol,
                            declared: scoping.symbol_span(symbol),
                        });
                    }
                    resolved.is_none() && ident.name == "arguments"
                }
                _ => false,
            };
            if is_this {
                let in_initializer = |value: &Option<Expression<'_>>| {
                    value.as_ref().is_some_and(|value| {
                        value.span().start <= span.start && span.end <= value.span().end
                    })
                };
                // `this`, `arguments` and `new.target` belong to the nearest enclosing
                // non-arrow function.
                let function = self
                    .semantic
                    .nodes()
                    .ancestor_kinds(node.id())
                    .find_map(|kind| match kind {
                        AstKind::Function(function) => Some(function.span),
                        // A static block and a field initializer bind `this` to the class and its
                        // instance; a computed key does not, it is evaluated in the outer scope.
                        AstKind::StaticBlock(block) => Some(block.span),
                        AstKind::PropertyDefinition(property)
                            if in_initializer(&property.value) =>
                        {
                            Some(property.span)
                        }
                        AstKind::AccessorProperty(property) if in_initializer(&property.value) => {
                            Some(property.span)
                        }
                        _ => None,
                    });
                sites.this_sites.push((span.start, function));
            }
        }
        sites
            .references
            .sort_unstable_by_key(|reference| reference.start);
        sites.this_sites.sort_unstable_by_key(|&(start, _)| start);
        sites
    }

    /// The source text under `span` (empty when the span is out of range).
    pub(super) fn slice(&self, span: Span) -> &'s str {
        self.source
            .get(span.start as usize..span.end as usize)
            .unwrap_or_default()
    }

    /// Renders one identifier reference: canonical local names, free/import identities, or a
    /// global name -- the single choke point every identifier rendering passes through, which is
    /// why the imported-helper inlining accounting records here (a reference that renders an
    /// import *identity* marks the module as not fully inlined).
    pub(super) fn reference(&mut self, ident: &IdentifierReference<'_>) -> String {
        // Accounting for imported-helper inlining (see the `inlined_imports`/`identity_imports`
        // doc): this is the one choke point every identifier rendering passes through, so a
        // reference to an import binding that renders as an identity -- a refused call's callee,
        // a value reference, an argument read -- is recorded here, exactly when it happens.
        if let Some(origin) = self.import_origin_of(ident) {
            self.identity_imports.insert(origin.component.clone());
        }
        reference_identity(
            self.semantic,
            self.body_span,
            self.imports,
            &self.file_key,
            &mut self.canonical,
            &mut self.next_local,
            self.local_prefix,
            ident.name.as_str(),
            ident.reference_id.get(),
        )
    }

    /// The import binding `ident` resolves to, when it resolves to one -- by symbol identity,
    /// the same dispatch `super::identity::free_identity` performs, so a local shadowing an import's name is
    /// never recorded as a reference to that import.
    pub(super) fn import_origin_of(
        &self,
        ident: &IdentifierReference<'_>,
    ) -> Option<&ImportOrigin> {
        let symbol_id = ident.reference_id.get().and_then(|reference_id| {
            self.semantic
                .scoping()
                .get_reference(reference_id)
                .symbol_id()
        })?;
        self.semantic
            .scoping()
            .symbol_flags(symbol_id)
            .is_import()
            .then(|| self.imports.get(ident.name.as_str()))
            .flatten()
    }

    /// The modules whose every reference in this pass rendered as an inlined helper body: the
    /// report `fingerprint()` hands extract.rs, which drops exactly these from
    /// `Context::imports`. Sorted, so the report is a pure function of the body.
    pub(super) fn fully_inlined_imports(&self) -> Vec<String> {
        let mut fully: Vec<String> = self
            .inlined_imports
            .difference(&self.identity_imports)
            .cloned()
            .collect();
        fully.sort();
        fully
    }
}
