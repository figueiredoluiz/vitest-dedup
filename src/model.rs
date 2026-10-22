//! The parser-independent intermediate representation and finding model.
//!
//! Everything above `crate::frontend` consumes these types. No `oxc_*` type may appear here: the
//! whole point of the boundary is that the analysis layer is testable from hand-built values and
//! survives a frontend replacement.

use crate::memory::HeapBytes;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// Serializes an `Arc<str>` field exactly as the `String` it replaced.
///
/// `serde`'s own `Arc` support is behind its `rc` feature, and enabling a feature to move three
/// fields is a heavier change than eleven lines. What matters is that the encoding is unchanged:
/// these fields are read back out inside `analysis`'s `ContextKey`, whose canonical JSON is
/// published as [`TestFingerprint::context_key`], so a different encoding here would silently
/// move a value consumers have already recorded.
mod arc_str {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::sync::Arc;

    /// Serialize one shared string as a Serde string value.
    pub(super) fn serialize<S: Serializer>(
        value: &Arc<str>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(value)
    }

    /// Deserialize a string value into a shared string, propagating deserializer errors.
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<str>, D::Error> {
        Ok(Arc::from(String::deserialize(deserializer)?.as_str()))
    }
}

/// [`arc_str`] for a `Vec<Arc<str>>`: encodes as a plain JSON array of strings, so the published
/// `contextKey` built from [`Context::imports`] is byte-identical to the `Vec<String>` encoding
/// it replaced.
mod arc_str_vec {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::Arc;

    /// Serialize shared strings as a sequence of string values.
    pub(super) fn serialize<S: Serializer>(
        value: &[Arc<str>],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .iter()
            .map(|entry| &**entry)
            .collect::<Vec<_>>()
            .serialize(serializer)
    }

    /// Deserialize a string sequence into shared-string elements.
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Arc<str>>, D::Error> {
        let strings = Vec::<String>::deserialize(deserializer)?;
        Ok(strings
            .iter()
            .map(|entry| Arc::from(entry.as_str()))
            .collect())
    }
}

/// A source position, 1-indexed for display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Position {
    /// 1-indexed line.
    pub line: u32,
    /// 1-indexed UTF-16 column, matching editor and SARIF conventions.
    pub column: u32,
}

/// A half-open source range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Span {
    /// Inclusive start.
    pub start: Position,
    /// Exclusive end.
    pub end: Position,
    /// Byte offset of `start` in the source, for span-based edits.
    pub start_byte: u32,
    /// Byte offset of `end` in the source.
    pub end_byte: u32,
}

/// A location in a specific file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Location {
    /// Path relative to the analysis root.
    pub path: PathBuf,
    /// Range within that file.
    pub span: Span,
}

/// How a test or suite title was written in source.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Title {
    /// A string or no-substitution template literal with a known value.
    Static(String),
    /// A template literal or expression whose value is not statically known.
    ///
    /// The payload is a normalized shape such as `` `user ${_} logs in` `` so that two dynamic
    /// titles built the same way still compare equal, without claiming to know the runtime text.
    Dynamic(String),
    /// An `each`/`for` title pattern, retained with its interpolation placeholders intact.
    Parameterized(String),
}

impl Title {
    /// The text to show in reports.
    pub fn display(&self) -> &str {
        match self {
            Self::Static(value) | Self::Dynamic(value) | Self::Parameterized(value) => value,
        }
    }

    /// Whether the title's runtime value is statically known.
    pub fn is_static(&self) -> bool {
        matches!(self, Self::Static(_))
    }
}

/// Which callable declared the node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeclarationKind {
    /// `test` or `it`.
    Test,
    /// `describe` or `suite`.
    Suite,
    /// `bench`.
    Benchmark,
}

/// A modifier applied through a chained property on a declaration.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Modifier {
    /// `.skip`
    Skip,
    /// `.only`
    Only,
    /// `.todo`
    Todo,
    /// `.fails`
    Fails,
    /// `.concurrent`
    Concurrent,
    /// `.sequential`
    Sequential,
    /// `.shuffle`
    Shuffle,
    /// `.skipIf(condition)`, retaining the normalized condition source.
    SkipIf(String),
    /// `.runIf(condition)`, retaining the normalized condition source.
    RunIf(String),
    /// `.each(table)` or `.for(table)`.
    Each(EachTable),
    /// A chained property this version does not model. Retained so unknown chains are visible
    /// rather than silently dropped.
    Unknown(String),
}

/// A statically recovered `each`/`for` table.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EachTable {
    /// Whether the declaration used `.for` rather than `.each`.
    pub is_for: bool,
    /// What the analyzer could recover about the table.
    pub rows: EachRows,
}

/// What the analyzer could recover about a `.each`/`.for` table.
///
/// This field used to be `Option<Vec<String>>`, with `None` standing for "the table is an
/// identifier, call, or spread the analyzer refused to guess at". That collapsed every unreadable
/// table to the same value -- `.each(getCasesA())` and `.each(getCasesB())` were indistinguishable
/// -- the same "two things that mean different things stored as one value" defect this file has
/// named [`MockFactory`] and [`FixtureValue`] for already. The guard that made this safe in
/// practice lived one layer up, not here: the frontend also marks the whole test's
/// [`Context::unresolved`] whenever it emits a table it cannot read, and every rule filters on
/// [`Context::is_comparable`] before grouping -- so no rule has ever actually compared two `None`
/// tables against each other. That guard stays (removing it is its own change, not this one); what
/// changes here is that the field itself stops lying to whatever reads it next.
///
/// [`Unreadable`](Self::Unreadable) carries what is honestly knowable about the table
/// *expression* even when its *values* are not: a literal folds to its own kind-tagged value, a
/// bare identifier resolves to the symbol it names (so two sibling scopes that each declare their
/// own same-named table variable are never conflated), and anything else -- a call, a spread, a
/// malformed tagged-template header -- falls back to its own source text. That is enough to tell
/// two genuinely different unreadable tables apart while never claiming to know a row it could not
/// read: an `Unreadable` value can never equal a `Literal` one (different variants), and two
/// `Unreadable` values compare equal only when what was recoverable about each expression is
/// identical.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EachRows {
    /// One normalized entry per row: the table was a statically known array literal (or
    /// tagged-template) the analyzer could fully enumerate.
    Literal(Vec<String>),
    /// The table's row values could not be read. Carries an identity for the table expression
    /// itself -- never a guess at its rows.
    Unreadable(String),
}

/// A lifecycle hook attached to a suite or file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hook {
    /// Which hook was registered.
    pub kind: HookKind,
    /// Where it was registered.
    pub location: Location,
    /// Fingerprints of the hook body, compared the same way test bodies are.
    pub body: Fingerprints,
    /// Whether every statement of this body is a provably idempotent operation.
    ///
    /// `redundant-hook`'s claim is "deleting one of these loses no coverage", and on the
    /// 2026-09-27 scan it was wrong all 10 times it was made -- because a repeated
    /// NON-idempotent statement is not redundant setup, it is the behaviour. `false` is the
    /// conservative answer and the default for everything the frontend cannot argue about; see
    /// `extract::hook_body_is_idempotent` for the three shapes that earn `true`.
    pub idempotent: bool,
    /// The hook body's `alpha` text, in the serialized form [`Fingerprints::alpha`] digests.
    ///
    /// Retained for exactly one reason, and only on hooks: `analysis`'s `ContextKey` publishes a
    /// hash of its own canonical JSON as [`TestFingerprint::context_key`], and that JSON contains
    /// each hook's `alpha` **text**. Reducing hook `alpha` to a digest with nothing else changed
    /// would therefore silently move every `contextKey` a consumer has already recorded -- a
    /// change to a published record, not an internal one, and one the record's `schemaVersion`
    /// would have to announce. Keeping the text here is what makes the digest change invisible
    /// from outside.
    ///
    /// Never compared. `Context::is_equivalent`, `ContextKey`'s own equality, and
    /// `redundant-hook` all read [`Fingerprints::alpha`]; this field only ever gets serialized.
    ///
    /// Shared rather than owned. One hook declaration is attached to every test in the scopes
    /// under it, so `extract`'s `effective_hooks` copies the whole [`Hook`] into each test's
    /// [`Context::hooks`] -- 39,576 copies of a few thousand declarations on a 24k-test corpus,
    /// which with an owned `String` meant 39,576 copies of this text. `Arc<str>` makes that copy
    /// a refcount bump and leaves one allocation per declaration. Equality and ordering still
    /// compare the text, so nothing above this type can tell the difference.
    #[serde(with = "arc_str")]
    pub alpha_source: Arc<str>,
}

/// The lifecycle hooks an extractor recognizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HookKind {
    /// `beforeAll`
    BeforeAll,
    /// `beforeEach`
    BeforeEach,
    /// `afterEach`
    AfterEach,
    /// `afterAll`
    AfterAll,
}

impl HookKind {
    /// The source-level spelling, used as a participant title by `redundant-hook` findings: a
    /// hook has no title of its own, and the kind is the honest name for "the `beforeEach` at
    /// this location".
    pub fn label(&self) -> &'static str {
        match self {
            Self::BeforeAll => "beforeAll",
            Self::BeforeEach => "beforeEach",
            Self::AfterEach => "afterEach",
            Self::AfterAll => "afterAll",
        }
    }
}

/// The three normalized views of a function body that rules compare.
///
/// Mirrors CukeDedup's `HandlerFingerprint`, with one deliberate difference: normalization is
/// driven by resolved symbols rather than by matching identifier text, because a test body plus
/// its hook chain and helpers spans more than one scope.
///
/// `alpha` is declared first among the string fields deliberately: the derived `Ord`/`PartialOrd`
/// compare fields in declaration order, and `TestCase`'s own derived `Ord` reaches into `body`
/// (this struct) as a tie-break among `.each`/`.for` rows that share one declaration `Location`
/// and title (see `TestCase`'s doc and `main.rs`'s sort). `alpha` is the first field a row-level
/// comparison can actually distinguish on, since row substitution changes `alpha`/`structural`
/// per row.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Fingerprints {
    /// Digest of the serialized AST with locals renamed by resolved symbol; literals preserved.
    ///
    /// A [`BodyDigest`], not the text: see that type for why nothing needs the text and what the
    /// collision probability is.
    pub alpha: BodyDigest,
    /// Digest of `alpha` with literal values replaced by their kind. Drives parameterization
    /// findings.
    pub structural: BodyDigest,
    /// Ordered behavior events: canonical callee paths, assertions, awaits, control flow, each
    /// reduced to an [`EventDigest`]. See [`BehaviorTrace`].
    pub behavior: BehaviorTrace,
    /// Whether the body is empty or a recognized stub, and therefore not worth comparing.
    pub trivial: bool,
    /// Whether this body's callback form changes its meaning, and which form it is.
    ///
    /// Not part of [`Fingerprints::behavior`], deliberately: a callback form is not something the
    /// test *does*, it is a property of the function enclosing what it does, and putting it in an
    /// event multiset both misdescribes it and destroys the one control that proves two bodies
    /// perform identical work. It belongs with `context` and `modifiers` -- identity a
    /// behaviour-keyed rule must agree on before it compares events at all.
    pub callback_form: CallbackForm,
}

/// Whether an arrow and a `function` body mean the same thing here, and which one was written.
///
/// An arrow does not bind `this`/`arguments` and a `function` does, so a body that reads either
/// means different things in the two forms -- `this.value = 5` writes to a different object.
/// `alpha` has always recorded this; `behavior` could not, so `near-duplicate-test` reported two
/// such tests as the same test. `duplicate-test` was never exposed, because it keys on `alpha`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum CallbackForm {
    /// The body reads neither `this` nor `arguments`, so the two forms are interchangeable.
    ///
    /// The overwhelmingly common case, and the reason rewriting an arrow as a `function` is a
    /// cosmetic difference this tool sees through rather than reports.
    #[default]
    Interchangeable,
    /// An arrow whose body reads `this`/`arguments`, which it does not bind.
    Arrow,
    /// A `function` whose body reads `this`/`arguments`, which it does bind.
    Function,
}

/// What a `vi.mock` registration supplies in place of the real module.
///
/// A tri-state, not an `Option`. Review found `Option<String>` here conflating "automocked" with
/// "we could not read the factory" — the third instance of that same defect in this file. A value
/// that means two things cannot be compared correctly under any equality.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MockFactory {
    /// `vi.mock('./x')` with no factory. Two of these are genuinely the same behavior.
    Automock,
    /// An inline factory, carrying its alpha-normalized fingerprint.
    ///
    /// Shared for the same reason as [`Hook::alpha_source`]: a file's mock registrations are
    /// copied into every test in that file, so an owned `String` here was duplicated once per
    /// test rather than once per `vi.mock` call.
    Factory(#[serde(with = "arc_str")] Arc<str>),
    /// A factory the analyzer could not read, such as an imported or computed one.
    ///
    /// Producing this **must** also set [`Context::unresolved`], which removes the test from
    /// comparison entirely. Equality on this variant is therefore never load-bearing.
    Unresolved,
}

/// One surviving `vi.mock` registration -- one that no later `vi.unmock` in the file cancelled.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MockedModule {
    /// An anchored identity for the module specifier passed to `vi.mock`, **not** the raw
    /// argument text.
    ///
    /// A relative specifier (`./dep`) is file-relative: the same text in two different
    /// directories names two different modules. Review found this field storing the literal
    /// `vi.mock` argument verbatim, which made `cross-file-duplicate` compare `Context`s as
    /// equivalent across files that mocked unrelated modules -- the same "unique in one scope,
    /// used in a wider one" defect `src/frontend/fingerprint.rs`'s free-import identity exists to
    /// avoid, applied to the one field in this struct that was never routed through it.
    /// `src/frontend/fingerprint.rs::specifier_identity` (the same scheme `import_origins` uses)
    /// is what this field is built from: `resolved:<canonical path>` when the specifier resolves
    /// to a real file, `unresolved:<anchored specifier>` when it does not -- prefixed distinctly
    /// so the two can never collide -- and a bare/package specifier (`vitest`, `@scope/pkg`)
    /// stays file-independent either way, since it names the same thing regardless of which file
    /// imports it.
    ///
    /// Shared for the same reason as [`MockFactory::Factory`].
    #[serde(with = "arc_str")]
    pub specifier: Arc<str>,
    /// What replaces the module.
    pub factory: MockFactory,
}

/// What a `test.extend` fixture supplies.
///
/// Tri-state for the same reason as [`MockFactory`]: review found this `Option` conflating "a
/// plain literal value" with "unreadable", which made `base.extend({ db: makeRealDb() })` compare
/// equal to `base.extend({ db: 42 })`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FixtureValue {
    /// A setup function, carrying its alpha-normalized fingerprint.
    Setup(String),
    /// A non-function value, carrying a fingerprint of the value itself.
    Literal(String),
    /// A value the analyzer could not read. Must also set [`Context::unresolved`].
    Unresolved,
}

/// One fixture supplied by `test.extend`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FixtureBinding {
    /// The fixture's name as destructured in the test callback.
    pub name: String,
    /// What the fixture provides.
    pub body: FixtureValue,
}

/// The ambient facts that make two bodies comparable.
///
/// Two byte-identical test bodies under different hook chains, mocks, or fixtures are not
/// duplicates. This is the precision lever with no CukeDedup analogue.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Context {
    /// Hook bodies that run for this test, outermost first.
    pub hooks: Vec<Hook>,
    /// Modules this file has mocked, sorted: the net effect of its module-top-level `vi.mock` and
    /// `vi.unmock` calls, folded last-in-source-order-wins per specifier, so a specifier that is
    /// mocked and then unmocked is absent here exactly as it is absent from Vitest's registry.
    /// Never includes the non-hoisted `vi.doMock`/`vi.doUnmock`, which taint the file instead --
    /// see `collect_mocked_modules` in `src/frontend/extract.rs`.
    pub mocked_modules: Vec<MockedModule>,
    /// Fixtures in scope from `test.extend`, sorted.
    pub fixtures: Vec<FixtureBinding>,
    /// Anchored module identities the body's free identifiers resolve to, sorted.
    ///
    /// This field used to store the raw specifier text (`./helpers`) rather than an identity
    /// anchored to a resolved module -- the same "unique in one scope, wrong in a wider one"
    /// defect [`MockedModule::specifier`]'s doc catalogs the rest of the instances of. It was
    /// sound only because every rule that reads `Context` also requires `body.alpha` equality
    /// first, and `alpha` already carries the properly anchored `free:import:resolved:…`
    /// identity for anything the body actually references -- so this coarser field could never
    /// decide a comparison the finer one had not already decided. A future context-only rule
    /// (comparing setup without also comparing bodies) would have made that exploitable, which is
    /// why it is fixed rather than left as a documented assumption: each entry here is now built
    /// through the same `crate::frontend::fingerprint::specifier_identity` scheme
    /// `MockedModule::specifier` uses, `resolved:<canonical path>` when the specifier names a
    /// real file and `unresolved:<anchored specifier>` when it does not -- prefixed distinctly so
    /// the two families can never collide, and a bare/package specifier stays file-independent
    /// either way.
    ///
    /// Shared text: every test in a file that references the same import holds one `Arc` to one
    /// allocation of its identity, built once per file by the extractor. Identities are long
    /// (`resolved:` plus a canonical absolute path) and repeated across most tests of a file, so
    /// per-test copies were the largest single retention cost (`--debug-memory`'s `imports`
    /// bucket). The context key and equivalence read the text, which is unchanged.
    #[serde(with = "arc_str_vec")]
    pub imports: Vec<Arc<str>>,
    /// Module-scope **awaited** calls, in source order, normalized to their own source text.
    ///
    /// S6 mechanism 3, measured on `nuxt-i18n`: a file whose module top level runs
    /// `await setup({ rootDir: './fixtures/alpha' })` boots a different fixture application than
    /// one running `await setup({ rootDir: './fixtures/beta' })`, and every test in the file
    /// inherits that. Nothing recorded it, so two such files compared equal and
    /// `cross-file-duplicate` linked them -- one of the three mechanisms behind that rule's 6.7%
    /// precision (1 of 15).
    ///
    /// **Awaited** is the admission rule, and it is narrow on purpose. A module-scope call that
    /// is not awaited is ordinary program text -- including, since S11, a helper call that
    /// *declares tests*, whose declarations are already modelled as tests rather than as ambient
    /// setup. Treating every module-scope call as context would fold those helper calls into the
    /// key of the very tests they declare. `await` at module top level is a file saying "nothing
    /// below runs until this finishes", which is what ambient setup means.
    ///
    /// The payload is source text, not an identity like [`Context::imports`] carries, because the
    /// discriminating part here is the *argument literal* (`'./fixtures/alpha'`) rather than a
    /// module anyone could resolve. Two spellings of the same setup therefore compare unequal --
    /// which suppresses a comparison rather than inventing one, the safe direction for a field
    /// whose whole job is to stop two files being called equal.
    pub module_setup: Vec<String>,
    /// The nearest per-directory config scope governing this test, when one exists.
    ///
    /// S6 mechanism 2: a `vitest.config.*` beside a test directory decides how those tests run,
    /// and two byte-identical files under `environment: 'node'` and `environment: 'happy-dom'`
    /// are not duplicates. Carries the config's own `test` object normalized, so identical
    /// configs still compare equal and only genuinely different ones separate; `None` when
    /// nothing nearer than the analysis root governs the file, which leaves the context exactly
    /// as it was before this field existed.
    pub config_scope: Option<String>,
    /// Set when some ambient fact could not be resolved, making this context unsafe to compare.
    ///
    /// Equality on this struct stays lawful and reflexive; the "unknown never matches" rule is
    /// enforced by filtering on [`Context::is_comparable`] before grouping, not by a value that
    /// compares unequal to itself.
    pub unresolved: bool,
}

impl Context {
    /// Whether this context is resolved well enough to participate in comparison at all.
    ///
    /// Rules must filter on this **before** grouping. A context carrying an unresolved fact could
    /// coincidentally match another, and a duplicate finding asserted on facts the analyzer could
    /// not read is exactly the false positive this product refuses to emit.
    pub fn is_comparable(&self) -> bool {
        !self.unresolved
    }

    /// Whether two contexts are equivalent enough for their bodies to be compared directly.
    ///
    /// No rule calls this: grouping needs a hashable key, so `analysis.rs`'s `ContextKey` is what
    /// every rule actually groups on. This is the executable specification that key is checked
    /// against -- `the_grouping_key_agrees_with_context_equivalence_on_every_pair` asserts the two
    /// answer identically across a context matrix. Kept rather than deleted for that reason: with
    /// it gone, `ContextKey`'s correctness would rest on a comment.
    ///
    /// Purely structural and reflexive. It does not consult [`Context::is_comparable`], because a
    /// non-reflexive equivalence would be unusable as a grouping key.
    ///
    /// It compares every field of [`Context`] except `unresolved`. That is not a style preference:
    /// the agreement test only proves what its matrix varies, and it hardcoded `module_setup` and
    /// `config_scope` at their defaults for two releases, so both fields reached `ContextKey`
    /// without reaching this predicate and the test stayed green. A field added to `Context` must
    /// be added here and given a row in that matrix, or the executable specification silently
    /// stops specifying it.
    pub fn is_equivalent(&self, other: &Self) -> bool {
        self.mocked_modules == other.mocked_modules
            && self.fixtures == other.fixtures
            && self.imports == other.imports
            // Compared in `ContextKey` order, and every field of `Context` except `unresolved`
            // must appear here. `module_setup` and `config_scope` were each added to the key and
            // not to this predicate, so two contexts the analyzer separates were reported
            // equivalent by the specification that key is checked against.
            && self.module_setup == other.module_setup
            && self.config_scope == other.config_scope
            && self.hooks.len() == other.hooks.len()
            && self
                .hooks
                .iter()
                .zip(&other.hooks)
                .all(|(left, right)| left.kind == right.kind && left.body.alpha == right.body.alpha)
    }
}

/// One extracted test case.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TestCase {
    /// Where the declaration appears.
    pub location: Location,
    /// Calling file that registers this declaration, when different from its source file.
    ///
    /// `None` preserves ordinary extraction without retaining a second copy of its path.
    /// Source locations still identify declarations; rule file scope and configuration belong
    /// to the caller. Imported extraction supplies this field in a later delivery step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_file: Option<PathBuf>,
    /// Which callable declared it.
    pub kind: DeclarationKind,
    /// Enclosing suite titles, outermost first.
    pub suite_path: Vec<Title>,
    /// The declaration's own title.
    pub title: Title,
    /// Chained modifiers, in source order.
    pub modifiers: Vec<Modifier>,
    /// The ambient facts that gate comparison.
    pub context: Context,
    /// Normalized views of the callback body.
    pub body: Fingerprints,
    /// Whether the declaration has a callback at all. `.todo` forms do not.
    pub has_body: bool,
    /// Declaration-local equality class when this case is one resolved row of a statically
    /// enumerated `test.each`/`test.for` or `describe.each`/`describe.for` table.
    ///
    /// Rows of one declaration share its `location`, but shared location alone does not mean
    /// "table rows": a `for...of` over a literal array is unrolled per iteration by the
    /// frontend (so per-iteration titles can fold), producing several cases at one location
    /// that are loop iterations, not rows -- and the loop variable is not substituted into the
    /// body, so iterations of a body that branches on it can look identical while deleting one
    /// would delete real coverage. Extraction assigns the same class only after exact comparison
    /// of kind-tagged row values, so the rule can compare suite rows too without retaining that
    /// canonical text in every case. Unlike a test-row callback, a suite body's fingerprint
    /// cannot substitute the row values used by its nested declarations.
    pub table_row_class: Option<u32>,
    /// Whether this row's callback can observe cells beyond the ones it binds by position.
    ///
    /// S7's body half turns on this one question. `duplicate-each-row` claims "identical rendered
    /// title AND substituted body", and both halves can be true of two rows that genuinely
    /// differ -- but only when the difference is somewhere the callback can still read.
    ///
    /// - `false`: a uniform ARRAY table whose callback takes a fixed parameter list with no rest
    ///   element. Vitest spreads the row, the callback binds the first N cells, and any cell past
    ///   N is **provably dropped**. `[1, 2]` and `[1, 3]` under `(value) => ...` both bind
    ///   `value = 1` and really are the same case twice -- verified against Vitest 5 and pinned by
    ///   `fixtures/observed/observed-uniform-each-table.test.ts`.
    /// - `true`: the callback takes a REST parameter (`(head, ...rest)`), so every cell reaches
    ///   it; or the row is an OBJECT, which binds whole, so every member does. These are the two
    ///   shapes behind 17 of this rule's 41 false positives -- an observed display-utility suite
    ///   and an application framework's form-utility suite respectively.
    ///
    /// When `true`, the rule additionally requires [`TestCase::table_row_class`] equality, so two
    /// rows differing anywhere the callback can see are not claimed as one case. When `false` it
    /// must NOT, or the dropped-cell true positive above is lost -- that fixture's header
    /// predicted exactly that failure ("it fails if a future fix ... is implemented by giving up
    /// on row expansion"), and a first attempt at this change duly broke it.
    pub row_cells_fully_observable: bool,
}

impl TestCase {
    /// File owning this registration, independently of the declaration's source location.
    pub fn registration_file(&self) -> &std::path::Path {
        self.registration_file
            .as_deref()
            .unwrap_or(&self.location.path)
    }

    /// Suite path and title joined the way Vitest composes a full test name.
    pub fn full_title(&self) -> String {
        let mut parts: Vec<&str> = self.suite_path.iter().map(Title::display).collect();
        parts.push(self.title.display());
        parts.join(" > ")
    }

    /// Whether the case is declared in a form that will never execute.
    pub fn is_inert(&self) -> bool {
        self.modifiers
            .iter()
            .any(|modifier| matches!(modifier, Modifier::Skip | Modifier::Todo))
    }
}

/// A rule identifier. Stable kebab-case names, never numeric codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rule {
    /// Same alpha-normalized body under an equivalent context.
    DuplicateTest,
    /// Same full title within one file.
    DuplicateTestName,
    /// Same structure under an equivalent context, differing only in literal values.
    ParameterizationCandidate,
    /// Bodies that serve the same purpose written differently, under an equivalent context.
    ///
    /// The rule this product exists for: text-level copy-paste detection cannot link two bodies
    /// whose identifiers and spelling differ; this finds tests whose behavior converges despite
    /// differing source. The fixture corpus is the evidence for that claim -- its positives are
    /// exactly pairs a token-level comparison would not connect.
    NearDuplicateTest,
    /// The same test body in two different files.
    ///
    /// `Warning` by default since 2026-09-23, on an exhaustive adjudication: 9 findings over
    /// 15,980 tests on the pinned external corpus, all 9 genuine copy-paste duplicates. The
    /// `Off` default it replaces rested on a hypothesis (duplication across unit and integration
    /// layers is frequently deliberate) that the corpus contains no instance of -- untested
    /// rather than refuted, which is why this is a `Warning` and not an `Error`.
    CrossFileDuplicate,
    /// One test's behavior events are a strict ordered subsequence of another's.
    ///
    /// A review prompt, not a deletion order: the analyzer establishes the subset relation, not
    /// that the larger test's extra observations are irrelevant.
    SubsumedTest,
    /// A whole `describe` block duplicated within one file, reported once instead of per-test.
    DuplicateSuite,
    /// Two rows of one parameterized test, benchmark, or suite carry the same case.
    DuplicateEachRow,
    /// A nested lifecycle hook repeats the work an ancestor hook already performs.
    RedundantHook,
}

/// What a finding claims, and therefore what the correct remedy is.
///
/// "Duplicate" conflated three different claims with three different remedies, which produced a
/// direct contradiction in planning: boundary values like `0` and `1` were simultaneously required
/// to produce no finding (they are deliberate coverage) and to be collapsed into a `test.each`
/// table. Both are right, because they are different verdicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Both tests observe the same behavior; deleting one loses no coverage.
    ///
    /// Note the remedy is not always deletion: when one test's title names a subject its body
    /// never exercises, the fix is to repair that test, and deleting it would cement the gap.
    Redundant,
    /// Both cases are needed, but belong in one table. The remedy preserves every case.
    Consolidatable,
    /// A naming collision rather than a behavioral claim.
    NameCollision,
    /// High similarity without established equivalence. Needs a human to decide.
    ///
    /// Kept distinct from `Redundant` on purpose: collapsing it there would claim the analyzer
    /// proved something it only suspects, and collapsing it into "no finding" would discard the
    /// case this product exists to surface.
    SimilarUncertain,
}

impl Rule {
    /// Every rule the analyzer implements.
    ///
    /// Exhaustive by construction: the `match` in [`Rule::id`] fails to compile when a variant is
    /// added, and this constant is the single place configuration, documentation, and reporters
    /// enumerate rules from. A second hand-maintained list elsewhere would let a new rule be
    /// silently unconfigurable, which is the same silent-narrowing failure this project exists to
    /// find in other people's code.
    pub const ALL: &'static [Self] = &[
        Self::DuplicateTest,
        Self::DuplicateTestName,
        Self::ParameterizationCandidate,
        Self::NearDuplicateTest,
        Self::CrossFileDuplicate,
        Self::SubsumedTest,
        Self::DuplicateSuite,
        Self::DuplicateEachRow,
        Self::RedundantHook,
    ];

    /// What this rule claims about the tests it reports, which determines the correct remedy.
    pub fn verdict(&self) -> Verdict {
        match self {
            Self::DuplicateTest => Verdict::Redundant,
            Self::ParameterizationCandidate => Verdict::Consolidatable,
            Self::NearDuplicateTest => Verdict::SimilarUncertain,
            Self::CrossFileDuplicate => Verdict::Redundant,
            Self::DuplicateTestName => Verdict::NameCollision,
            // The subset relation is established, but "the smaller test adds no coverage" is a
            // judgement about the larger test's extra observations that only a human can make --
            // the same necessary-not-sufficient position `near-duplicate-test` holds.
            Self::SubsumedTest => Verdict::SimilarUncertain,
            // A copied block is redundant exactly the way a copied test is: alpha-equality of the
            // whole block is a proof.
            Self::DuplicateSuite => Verdict::Redundant,
            Self::DuplicateEachRow => Verdict::Redundant,
            Self::RedundantHook => Verdict::Redundant,
        }
    }

    /// Resolves a stable identifier back to its rule.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|rule| rule.id() == id)
    }

    /// The stable identifier used in configuration and machine reports.
    pub fn id(&self) -> &'static str {
        match self {
            Self::DuplicateTest => "duplicate-test",
            Self::DuplicateTestName => "duplicate-test-name",
            Self::ParameterizationCandidate => "parameterization-candidate",
            Self::NearDuplicateTest => "near-duplicate-test",
            Self::CrossFileDuplicate => "cross-file-duplicate",
            Self::SubsumedTest => "subsumed-test",
            Self::DuplicateSuite => "duplicate-suite",
            Self::DuplicateEachRow => "duplicate-each-row",
            Self::RedundantHook => "redundant-hook",
        }
    }

    /// The severity applied when configuration does not override it.
    pub fn default_severity(&self) -> Severity {
        match self {
            Self::DuplicateTest => Severity::Error,
            Self::DuplicateTestName => Severity::Warning,
            // Collapsing a matrix into `test.each` is a readability judgement, not a defect.
            Self::ParameterizationCandidate => Severity::Warning,
            // A near-duplicate is a review prompt, not a defect. Error severity is reserved for
            // the provable case, and a similarity measure is never provable.
            Self::NearDuplicateTest => Severity::Warning,
            // On by default on measured evidence: 9 findings over 15,980 tests of pinned external
            // corpus (0.06 per 100), adjudicated exhaustively at 9/9 `redundant` -- a duplicated
            // `shallowReadonly` section across two files of a Vue framework core, one body shared
            // by two hook tests in a React monorepo, an array test stranded in a
            // schema-validation library, per-locale copied boilerplate. Warning rather than Error because the
            // hypothesis the former `Off` default rested on -- deliberate duplication across a
            // unit and an integration layer -- has no instance in that corpus, so it is untested
            // rather than refuted, and a human should confirm before either side is deleted.
            Self::CrossFileDuplicate => Severity::Warning,
            // The last rule that ships Off, and on measured precision rather than measured volume
            // alone: an adjudicated 40-finding sample ruled 30% of its findings `distinct`, and on
            // 12 of the 14 actionable ones the right remedy is `test.each`, not the deletion this
            // rule proposes. The volume is the second argument, not the first -- a Warning default
            // added ~2,900 findings on the pinned corpus, 83% of one project's from two
            // homogeneous parametric suites whose per-case tests are deliberate coverage, because
            // subsumption is a partial order and those files go quadratic. Until the call-argument
            // erasure behind the false positives is fixed, the rule stays opt-in.
            Self::SubsumedTest => Severity::Off,
            // A copied suite is provable, but reporting a whole block at error severity would
            // dominate the report; the per-test evidence is one finding away if wanted.
            Self::DuplicateSuite => Severity::Warning,
            // A repeated table row is a copy-paste slip, not a design disagreement.
            Self::DuplicateEachRow => Severity::Warning,
            // Redundant setup costs runtime, not correctness.
            Self::RedundantHook => Severity::Warning,
        }
    }

    /// A one-line statement of what this rule detects, for a driver's `rules[].shortDescription`
    /// (SARIF) and anywhere else a rule needs a stable, human-readable summary. Mirrors the
    /// "Meaning" column of the rule table in `README.md`, which this method is the single owned
    /// source for -- `docs_contract.rs` checks the table against `Rule::default_severity` and
    /// `Rule::id`, but has no way to check free-text prose against itself, so this and that table
    /// can still drift in wording; they cannot drift in which rules exist, because both are driven
    /// from [`Rule::ALL`].
    pub fn short_description(&self) -> &'static str {
        match self {
            Self::DuplicateTest => {
                "Two tests have the same normalized body under an equivalent context."
            }
            Self::DuplicateTestName => "Two tests in one file share a full title.",
            Self::ParameterizationCandidate => {
                "Two tests have the same structure and differ only in literal values."
            }
            Self::NearDuplicateTest => {
                "Two tests execute the same behavior events under an equivalent context, but \
                 their normalized bodies differ."
            }
            Self::CrossFileDuplicate => {
                "The same duplicate-test criteria, without the single-file restriction."
            }
            Self::SubsumedTest => {
                "One test's behavior events are a strict ordered subsequence of another's."
            }
            Self::DuplicateSuite => "A whole `describe` block is duplicated within one file.",
            Self::DuplicateEachRow => {
                "Two rows of one parameterized test, benchmark, or suite carry the same case."
            }
            Self::RedundantHook => {
                "A nested lifecycle hook repeats the work an ancestor hook already performs."
            }
        }
    }

    /// A fuller statement of what this rule claims and what follows from that claim, for a
    /// driver's `rules[].fullDescription` (SARIF). States the same claim [`Rule::verdict`]
    /// encodes as a closed enum, in prose, so a reader of the driver metadata alone (without a
    /// result in hand) already knows what remedy each rule implies.
    pub fn full_description(&self) -> &'static str {
        match self {
            Self::DuplicateTest => {
                "Both tests observe the same behavior; deleting one loses no coverage. The \
                 remedy is not always deletion: when one test's title names a subject its body \
                 never exercises, the fix is repairing that test, and deleting it would cement \
                 the gap."
            }
            Self::DuplicateTestName => {
                "A naming collision, not a behavioral claim: two tests in one file share a full \
                 title. The remedy is to rename one, not to delete either."
            }
            Self::ParameterizationCandidate => {
                "Both cases are needed, but belong in one `test.each` table. The remedy \
                 consolidates them while keeping every row -- deleting one would lose coverage."
            }
            Self::NearDuplicateTest => {
                "High similarity without established equivalence: this analyzer detects that two \
                 bodies execute the same behavior events, sometimes in a different order or \
                 through a supported normalization, but cannot verify every precondition that \
                 would make them interchangeable. A review prompt, not a proof -- a human \
                 decides before either test is touched. What that review usually decides is \
                 consolidation, not deletion: on adjudicated findings the common remedy is \
                 merging the cases into one `test.each` table or shared helper that keeps every \
                 one of them, and deletion is right in a minority."
            }
            Self::CrossFileDuplicate => {
                "The same criteria as `duplicate-test`, applied across file boundaries instead of \
                 within one file. Reported at Warning rather than Error because the copy may be \
                 deliberate -- a unit and an integration layer can mirror a case on purpose -- so \
                 a human confirms before either side goes."
            }
            Self::SubsumedTest => {
                "Every step the smaller test performs, the larger one also performs, in order. \
                 The subset relation is a fact; whether the larger test's extra observations make \
                 the smaller one worthless is not -- a human decides. Never a deletion order on \
                 its own."
            }
            Self::DuplicateSuite => {
                "An entire suite was copied: title, children, and bodies. Reported once at the \
                 suite level, and the per-test findings between the two copies are suppressed \
                 rather than re-stated -- the copy is the defect, not each child pair."
            }
            Self::DuplicateEachRow => {
                "The same table row appears twice, so the same case is registered and run twice. \
                 Deleting one row loses nothing; keeping both costs runtime and implies coverage \
                 that does not exist."
            }
            Self::RedundantHook => {
                "A nested hook's body repeats what an ancestor hook of the same kind already \
                 performs, so every test under it pays for the same setup twice. Deleting the \
                 nested hook loses nothing the ancestor does not provide."
            }
        }
    }
}

/// How a finding affects the exit code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    /// Reported and ignored.
    Off,
    /// Reported without failing the run.
    Warning,
    /// Reported and fails the run.
    Error,
}

/// One reported problem, always backed by at least two locations.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Finding {
    /// Which rule produced it.
    pub rule: Rule,
    /// Effective severity after configuration.
    pub severity: Severity,
    /// Human-readable explanation.
    pub message: String,
    /// The participating tests, in deterministic order.
    pub locations: Vec<Location>,
    /// Calling files represented by each declaration participant, parallel to `locations`.
    ///
    /// Empty means every participant belongs to its source file, preserving existing reports.
    /// A collapsed declaration can represent several callers; retaining them prevents changed
    /// file filtering from dropping a finding when a non-representative caller changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registration_files: Vec<Vec<PathBuf>>,
    /// Full titles of the participating tests, parallel to `locations`.
    pub titles: Vec<String>,
    /// Whether each participating test is inert (`.skip`/`.todo`), parallel to `locations`.
    ///
    /// A structured field rather than a phrase inside [`Finding::message`]. A reporter that
    /// recovered this by matching text in the message would break silently the first time the
    /// wording changed, and nothing would fail: it would simply start reporting every participant
    /// as live. Reporters that need the distinction read it from here.
    pub inert: Vec<bool>,
    /// Whether a baseline (`--baseline`) accounts for this finding.
    ///
    /// A baselined finding is **excluded from the quality gate and from nothing else**: it is
    /// still analyzed, still sorted, still emitted by every reporter, and still labeled as a
    /// finding. Making accepted debt vanish from the report would turn a baseline into a way to
    /// stop seeing the problem — the failure mode this codebase exists to prevent — so the flag is
    /// carried on the finding rather than used to filter it out. See [`crate::modes`].
    ///
    /// `#[serde(default)]` so a JSON report written before baselines existed still deserializes.
    #[serde(default)]
    pub baselined: bool,
}

impl Finding {
    /// Calling files represented by participant `index`, with the legacy source-file fallback.
    pub fn registration_files_for(&self, index: usize) -> impl Iterator<Item = &std::path::Path> {
        let files = self.registration_files.get(index);
        files.into_iter().flatten().map(PathBuf::as_path).chain(
            self.locations
                .get(index)
                .filter(move |_| files.is_none_or(Vec::is_empty))
                .map(|location| location.path.as_path()),
        )
    }

    /// Stores sorted caller provenance only when it adds information to source locations.
    pub fn set_registration_files(&mut self, mut files: Vec<Vec<PathBuf>>) {
        debug_assert_eq!(files.len(), self.locations.len());
        for callers in &mut files {
            callers.sort();
            callers.dedup();
        }
        if files
            .iter()
            .zip(&self.locations)
            .all(|(callers, location)| {
                callers.len() == 1 && callers.first() == Some(&location.path)
            })
        {
            self.registration_files.clear();
        } else {
            self.registration_files = files;
        }
    }

    /// Whether any participating test is inert.
    pub fn has_inert_participant(&self) -> bool {
        self.inert.iter().any(|inert| *inert)
    }

    /// Full titles of the inert participants, in the order they appear in `locations`.
    pub fn inert_titles(&self) -> Vec<&str> {
        self.titles
            .iter()
            .zip(&self.inert)
            .filter(|(_, inert)| **inert)
            .map(|(title, _)| title.as_str())
            .collect()
    }

    /// A stable identity for this finding. Used as-is by the `sarif` reporter's
    /// `partialFingerprints` (`src/reporters/sarif.rs`); baseline and suppression support are
    /// still on the roadmap and will use it too.
    ///
    /// Mirrors CukeDedup's `finding_fingerprint` (`cuke-dedup/src/modes.rs`), labeled the same
    /// way (`{FINGERPRINT_VERSION}:<hash>`, the CukeDedup equivalent of
    /// `cukeDedupFingerprint/v2`): a future change to the derivation below changes every emitted
    /// fingerprint, and bumping the version segment makes that change detectable by a baseline
    /// consumer instead of silently invalidating it.
    ///
    /// # What goes in, and why
    ///
    /// The one property that matters is in the module-level tests below, not in this comment:
    /// **the same finding keeps its fingerprint across unrelated edits to the file.** That rules
    /// out anything line- or byte-offset-based -- inserting an import at the top of a file must
    /// not re-fingerprint every finding below it. So the hash input is built from exactly three
    /// things that do not move when unrelated lines do:
    ///
    /// - [`Finding::rule`]'s stable id -- two findings from different rules are never the same
    ///   finding, even if they happen to name the same tests (`duplicate-test` and
    ///   `duplicate-test-name` can both fire on one pair).
    /// - Each participant's file path, forward-slash normalized the same way every reporter
    ///   normalizes it, so a repository analyzed on Windows and on Linux CI hashes identically.
    /// - Each participant's full title (suite path + own title, already normalized the way
    ///   [`TestCase::full_title`] normalizes dynamic and parameterized titles) -- the closest
    ///   thing to CukeDedup's matcher text that a `Finding` carries. `Finding` does not carry a
    ///   participant's body fingerprint (the frontend fingerprints bodies, but that value never
    ///   survives into the report model), so title is the strongest per-participant identity
    ///   available at this layer, and in practice it is what actually varies between two
    ///   otherwise-similar findings: two duplicate pairs sharing a coincidentally-identical
    ///   normalized body almost always have distinct titles, which already gives them distinct
    ///   fingerprints without needing the body at all.
    ///
    /// Severity is deliberately excluded: lowering a rule from `error` to `warning` in
    /// configuration must not change the identity of the finding it still reports, or every
    /// baseline entry would need re-earning after a config-only change.
    ///
    /// A participant registered outside its source file additionally includes its sorted caller
    /// provenance in a JSON tuple. Legacy source-owned participants keep their exact v1 input,
    /// so existing baseline entries remain valid; the extended tuple cannot collide with the
    /// legacy spelling because it escapes every NUL the latter uses as a separator.
    /// Extended source/caller identities preserve invalid Unicode as native path units;
    /// the lossless identities also select the legacy branch, so lossy lookalikes cannot enter it.
    ///
    /// Participant path/title pairs are sorted before hashing (matching CukeDedup's
    /// `comparison_fingerprint_input` sorting its two sides) so the fingerprint does not depend
    /// on which location this analyzer happened to list first -- a `Finding`'s member order is
    /// already deterministic, but making the fingerprint order-independent as well means a future
    /// change to how members are ordered within one finding cannot silently change fingerprints.
    ///
    /// # Collisions are possible, and accepted by design
    ///
    /// Two genuinely different findings in one file can legitimately hash the same: a group of
    /// more than two tests sharing one identical (title, normalized body) is reported as a
    /// spanning set of pairwise findings (see `analysis/pairs.rs`), and if the *same* title text
    /// was copy-pasted at every one of those locations, two different pairs drawn from that group
    /// can carry the exact same (path, title) participant set. Excluding location would defeat
    /// the stability property above, and re-including it would defeat the point of a fingerprint
    /// that survives unrelated edits -- so this is the same trade CukeDedup makes, and the same
    /// one SARIF's `partialFingerprints` is documented to allow: a fingerprint narrows identity,
    /// it does not guarantee uniqueness. A consumer needing to tell two colliding findings apart
    /// (baseline "allow N occurrences of this fingerprint", the way CukeDedup's own baseline
    /// loader counts rather than assumes uniqueness) already has everything it needs in
    /// `locations` to do so.
    pub fn fingerprint(&self) -> String {
        let mut participants: Vec<String> = self
            .locations
            .iter()
            .zip(&self.titles)
            .enumerate()
            .map(|(index, (location, title))| {
                let source = fingerprint_path(&location.path);
                let mut callers: Vec<FingerprintPath> = self
                    .registration_files_for(index)
                    .map(fingerprint_path)
                    .collect();
                callers.sort();
                callers.dedup();
                if callers.len() == 1 && callers[0] == source {
                    // Existing declarations retain the exact v1 fingerprint input.
                    format!("{}\u{0}{title}", forward_slash(&location.path))
                } else {
                    // JSON escapes title delimiters, distinguishing this extended identity
                    // from every legacy participant, which always contains a literal NUL.
                    serde_json::to_string(&(source, title, callers))
                        .expect("paths and titles serialize")
                }
            })
            .collect();
        participants.sort_unstable();

        let mut input = String::from(FINGERPRINT_VERSION);
        input.push('\u{0}');
        input.push_str(self.rule.id());
        for participant in &participants {
            input.push('\u{0}');
            input.push_str(participant);
        }

        format!("{FINGERPRINT_VERSION}:{}", stable_hash(&input))
    }
}

/// The version segment embedded in every [`Finding::fingerprint`]. Bump only the trailing number
/// -- never the rest of the spelling -- when [`Finding::fingerprint`]'s derivation changes, so a
/// baseline computed against an older version reads as unmistakably stale rather than silently
/// comparing hashes produced by two different schemes.
pub const FINGERPRINT_VERSION: &str = "vitestDedupFingerprint/v1";

/// Renders `path` with forward slashes regardless of host platform.
///
/// A byte-for-byte duplicate of `crate::reporters::shared::forward_slash_path`'s separator
/// normalization (without the sanitization step that function also applies, which matters for
/// *display* safety and not for a hash of the raw bytes). Kept as its own copy rather than a call
/// into `reporters` because `model` is the layer reporters are built on: reporters importing from
/// `model` is normal layering, `model` importing from `reporters` would invert it for the sake of
/// one three-line helper.
fn forward_slash(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Lossless normalized identity for an extended fingerprint's source or caller path.
/// Unicode paths retain the published JSON string shape; native units cannot alias strings.
#[derive(PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(untagged)]
enum FingerprintPath {
    Text(String),
    Native(Vec<u32>),
}

/// Normalizes separators without replacing invalid Unicode. Native Unix bytes and Windows
/// UTF-16 units keep identity independent of Rust's unspecified internal OS-string encoding.
fn fingerprint_path(path: &std::path::Path) -> FingerprintPath {
    if let Some(text) = path.to_str() {
        return FingerprintPath::Text(text.replace('\\', "/"));
    }
    #[cfg(unix)]
    let mut units: Vec<u32> = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str()
            .as_bytes()
            .iter()
            .copied()
            .map(u32::from)
            .collect()
    };
    #[cfg(windows)]
    let mut units: Vec<u32> = {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().map(u32::from).collect()
    };
    #[cfg(not(any(unix, windows)))]
    let mut units: Vec<u32> = path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .copied()
        .map(u32::from)
        .collect();
    for unit in &mut units {
        if *unit == u32::from(b'\\') {
            *unit = u32::from(b'/');
        }
    }
    FingerprintPath::Native(units)
}

/// FNV-1a, matching CukeDedup's `stable_fingerprint` (`cuke-dedup/src/model.rs`) exactly:
/// deliberately simple and deterministic across platforms and Rust versions, which is what a
/// fingerprint meant to be compared across CI runs and machines needs.
fn stable_hash(value: &str) -> String {
    format!("{:016x}", fnv1a_64(value))
}

/// The 64-bit FNV-1a [`stable_hash`] renders. Separated only so a caller that needs the number
/// rather than its hex spelling does not have to parse the spelling back.
fn fnv1a_64(value: &str) -> u64 {
    let mut hash = FNV64_OFFSET;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV64_PRIME);
    }
    hash
}

/// One normalized view of a body (`alpha` or `structural`), retained as a fixed-width digest
/// rather than as the serialized text it was computed from.
///
/// # Why not the text
///
/// Nothing anywhere reads the *content* of `alpha` or `structural`. Every consumer compares them
/// for equality (or orders on them for determinism): they key `DuplicateTestKey`,
/// `ParameterizationKey`, `CrossFileKey` and friends in `crate::analysis`, and they are published
/// only as a hash ([`TestFingerprint`], [`HookFingerprint`]). The text itself averaged ~3.5 KB
/// per body on a 24k-test corpus and was retained for the whole run, once per test and once per
/// hook-chain membership -- 220 MB of live heap holding strings no code path ever looked inside.
///
/// # Collision risk, and why the hash is a cryptographic one
///
/// A collision here is not a missed finding; it is a **false `duplicate-test` at error severity**,
/// on someone else's repository. That threat model rules out a fast non-cryptographic hash no
/// matter how wide it is. The bodies being hashed are *attacker-influenced*: string and number
/// literals are preserved verbatim in `alpha`, so whoever writes the tests controls a large run
/// of the serialized bytes. FNV-1a -- which this field used at 128 bits in an earlier draft --
/// has an algebraically invertible step and makes no pseudorandomness claim at any width, so a
/// colliding pair can be *constructed* rather than waited for, and the birthday arithmetic simply
/// does not apply to it.
///
/// `strong` is therefore SHA-256 truncated to its leading 128 bits (`sha256_128`). Against a
/// collision-resistant hash, truncation to 128 bits leaves a best known attack at 2^64 work, and
/// accidental collisions follow the birthday bound for real: n<sup>2</sup>/2<sup>129</sup> is
/// ~9e-31 over a 25,000-body corpus and ~1.5e-25 over a ten-million-body monorepo -- below the
/// rate at which the machine computing them miscomputes something else, which is the only honest
/// bar for "negligible". Unkeyed and standard, so the value, and the ordering it induces, are
/// identical on every platform, every run, and every Rust release.
///
/// Verify-on-match (re-reading and re-serializing both bodies before emitting a finding) was
/// considered and rejected: it would make `analysis` depend on the source files again rather than
/// purely on the IR, and buy a reduction from ~1e-25.
///
/// # Two numbers, not one
///
/// `published` is **not** a second comparison key -- it exists so that reducing the text to a
/// digest changes no emitted byte. `--emit-test-fingerprints` publishes
/// `fingerprint_hash(canonical JSON of the text)` for this view, and that value cannot be
/// recovered from `strong`; computing it once here, while the text is still in hand, keeps the
/// published record identical to what the text-backed version emitted. It is included in
/// `Eq`/`Ord` only because deriving them over the whole struct is simpler to reason about than
/// deriving them over one field, and it can only ever make two digests *more* distinguishable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BodyDigest {
    /// SHA-256 over the normalized text's canonical JSON, truncated to 128 bits. The value every
    /// rule compares.
    strong: u128,
    /// 64-bit FNV-1a over the text's canonical JSON encoding: exactly the number
    /// [`fingerprint_hash`] would have produced for this view, kept so the published record does
    /// not move.
    published: u64,
}

/// FNV-1a's 64-bit offset basis.
const FNV64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a's 64-bit prime.
const FNV64_PRIME: u64 = 0x0000_0100_0000_01b3;
/// SHA-256's round constants: the first 32 bits of the fractional parts of the cube roots of the
/// first 64 primes (FIPS 180-4 section 4.2.2).
#[rustfmt::skip]
const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256's initial hash value: the first 32 bits of the fractional parts of the square roots of
/// the first eight primes (FIPS 180-4 section 5.3.3).
const SHA256_INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// A streaming SHA-256, implemented here rather than pulled in as a dependency.
///
/// Written out because the alternative is a new crate in a deliberately short dependency list for
/// ~70 lines of a fully specified, unchanging algorithm -- and because streaming is what
/// [`DigestWriter`] needs, so a one-shot convenience API would not have helped. The risk of a
/// hand-rolled hash is an implementation bug, not a design one, and that risk is retired by
/// testing against the FIPS 180-4 vectors and against a chunk-size-independence property (see
/// `sha256_matches_the_published_vectors` and `sha256_is_independent_of_chunking`): a digest that
/// reproduces the published vectors for the empty, single-block, and block-straddling cases is
/// SHA-256.
struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    buffered: usize,
    length_bytes: u64,
}

impl Sha256 {
    /// Initialize SHA-256 state with its standard constants and an empty partial block.
    fn new() -> Self {
        Self {
            state: SHA256_INIT,
            block: [0; 64],
            buffered: 0,
            length_bytes: 0,
        }
    }

    /// Buffer input bytes into the SHA-256 state, compressing each complete block and tracking
    /// total length.
    fn update(&mut self, mut bytes: &[u8]) {
        self.length_bytes = self.length_bytes.wrapping_add(bytes.len() as u64);
        while !bytes.is_empty() {
            let take = (64 - self.buffered).min(bytes.len());
            self.block[self.buffered..self.buffered + take].copy_from_slice(&bytes[..take]);
            self.buffered += take;
            bytes = &bytes[take..];
            if self.buffered == 64 {
                let block = self.block;
                self.compress(&block);
                self.buffered = 0;
            }
        }
    }

    /// Apply one SHA-256 message-schedule/compression round sequence to the supplied 64-byte
    /// block.
    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (index, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
            w[index] = u32::from_be_bytes(*chunk);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(SHA256_K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    /// Applies FIPS 180-4 padding and returns the full 256-bit digest, most significant byte
    /// first.
    fn finish(mut self) -> [u8; 32] {
        let bit_length = self.length_bytes.wrapping_mul(8);
        self.update(&[0x80]);
        // `update` counted the padding byte; the length field must describe the message only.
        self.length_bytes = 0;
        while self.buffered != 56 {
            self.update(&[0x00]);
            self.length_bytes = 0;
        }
        self.update(&bit_length.to_be_bytes());
        debug_assert_eq!(self.buffered, 0, "padding must end on a block boundary");
        let mut digest = [0u8; 32];
        for (slot, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *slot = word.to_be_bytes();
        }
        digest
    }

    /// The leading 128 bits of the digest, as one integer. Truncating a collision-resistant hash
    /// leaves a collision-resistant hash of the narrower width.
    fn finish_128(self) -> u128 {
        let digest = self.finish();
        let mut leading = [0u8; 16];
        leading.copy_from_slice(&digest[..16]);
        u128::from_be_bytes(leading)
    }
}

/// SHA-256 of `value`, truncated to 128 bits. The per-event digest [`BehaviorTrace`] stores.
fn sha256_128(value: &str) -> u128 {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hasher.finish_128()
}

/// Feeds bytes into a 64-bit FNV-1a and a SHA-256 state at once.
///
/// Exists so [`BodyDigest::of`] can take both of its hashes over the *same* canonical JSON
/// encoding in a single pass, without ever materializing that encoding: `serde_json::to_writer`
/// pushes the escaped bytes straight through here. Materializing it instead (`to_string`) meant
/// allocating and copying a second full-size buffer per body -- ~440 MB of pure churn on a
/// 24k-test corpus -- for a value read once and dropped.
///
/// The two hashes answer different questions and are deliberately different functions: `narrow`
/// reproduces a published value byte for byte and makes no correctness claim, `strong` is the one
/// every rule compares and therefore has to resist a *constructed* collision. See [`BodyDigest`].
struct DigestWriter {
    narrow: u64,
    strong: Sha256,
}

impl DigestWriter {
    /// Initialize the paired narrow/strong digest sink.
    fn new() -> Self {
        Self {
            narrow: FNV64_OFFSET,
            strong: Sha256::new(),
        }
    }
}

impl std::io::Write for DigestWriter {
    /// Feed all bytes into both digest states and report the complete buffer as consumed.
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for byte in buf {
            self.narrow ^= u64::from(*byte);
            self.narrow = self.narrow.wrapping_mul(FNV64_PRIME);
        }
        self.strong.update(buf);
        Ok(buf.len())
    }

    /// Return success because the digest sink has no external output to flush.
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A fixed-width spelling of `text`'s identity: SHA-256 truncated to 128 bits, in hex. For setup
/// facts whose full text would otherwise be held once per test in a file.
pub fn identity_digest(text: &str) -> String {
    format!("{:032x}", sha256_128(text))
}

impl BodyDigest {
    /// Digests one normalized view's serialized text.
    ///
    /// Both hashes are taken over the text's canonical JSON encoding, not over the raw text.
    /// That is forced for `published` -- it must reproduce `fingerprint_hash(canonical_json(..))`
    /// digit for digit -- and free for `strong`, since JSON encoding is injective: two texts have
    /// the same encoding exactly when they are the same text, so hashing the encoding
    /// distinguishes no fewer bodies than hashing the text would.
    pub fn of(text: &str) -> Self {
        let mut writer = DigestWriter::new();
        serde_json::to_writer(&mut writer, text).expect("a string encodes to JSON infallibly");
        Self {
            strong: writer.strong.finish_128(),
            published: writer.narrow,
        }
    }

    /// The hex spelling `--emit-test-fingerprints` publishes for this view.
    pub fn published_hex(&self) -> String {
        format!("{:016x}", self.published)
    }
}

impl Default for BodyDigest {
    /// The digest of the empty text, matching what the previous `String` field's `Default` (an
    /// empty `String`) hashed to. A declaration with no body (`crate::frontend::extract`'s
    /// `Fingerprints::default()`) must keep comparing and publishing exactly as it did.
    fn default() -> Self {
        Self::of("")
    }
}

/// One recorded behavior event, reduced to a 128-bit digest of its canonical text.
///
/// Events *must* stay individually comparable -- `analysis::is_strict_subsequence` walks two
/// lists element by element, and `near-duplicate-test` sorts one into a multiset -- so the list
/// cannot collapse into a single hash the way [`BodyDigest`] does for a whole body. Per-event
/// digests keep every one of those comparisons intact while dropping the text: 567,000 events
/// across a 24k-test corpus, averaging ~117 bytes each, held 80 MB that nothing read the content
/// of. Same hash and same width as `BodyDigest::strong` -- truncated SHA-256, for the reason
/// that type's collision section gives -- and the same claim: a collision here would manufacture
/// a `near-duplicate-test` or `subsumed-test` out of two events that are not the same event.
pub type EventDigest = u128;

/// The ordered behavior events of one body.
///
/// Holds the digests rules compare, plus the one hash the events' *text* is published as, so
/// reducing the events to digests moves no emitted byte -- the same two-numbers arrangement, for
/// the same reason, as [`BodyDigest`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BehaviorTrace {
    /// One digest per recorded event, in source order.
    events: Vec<EventDigest>,
    /// FNV-1a-64 over the canonical JSON *array of event texts*, which is exactly what
    /// [`fingerprint_hash`] was previously handed for [`TestFingerprint::behavior`]. Encoding the
    /// events as a JSON array is what keeps one event's content from being confused with a
    /// separator between events.
    published: u64,
}

impl BehaviorTrace {
    /// Digests one body's recorded events, in the order the frontend collected them.
    pub fn new(events: &[String]) -> Self {
        let mut writer = DigestWriter::new();
        serde_json::to_writer(&mut writer, events).expect("strings encode to JSON infallibly");
        Self {
            events: events.iter().map(|event| sha256_128(event)).collect(),
            published: writer.narrow,
        }
    }

    /// The digests, in source order.
    pub fn events(&self) -> &[EventDigest] {
        &self.events
    }

    /// How many events were recorded. The number the substance floors in `crate::analysis` gate
    /// on, and the number `subsumed-test`'s message quotes.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether no event was recorded at all.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// The hex spelling `--emit-test-fingerprints` publishes for this body's events.
    pub fn published_hex(&self) -> String {
        format!("{:016x}", self.published)
    }
}

impl Default for BehaviorTrace {
    /// The trace of a body with no events, which publishes the hash of `[]` -- exactly what the
    /// previous empty `Vec<String>` hashed to.
    fn default() -> Self {
        Self::new(&[])
    }
}

// Three `HeapBytes` implementations live here rather than beside their siblings in
// `crate::memory`, for one reason: `BodyDigest` and `BehaviorTrace` keep their fields private to
// this module, and the enforcement `crate::memory` is built on is *destructuring* --
// `let Self { .. } = self`, so that adding a field is a compile error until someone accounts for
// its cost. A destructuring pattern cannot name a private field from another module, so the
// choice was between weakening these types' encapsulation (which their own doc comments argue
// for at length) and putting three small implementations on this side of the boundary.
// `Fingerprints` joins them only so that all three views of a body are accounted for in one
// place. Everything else in the IR is measured in `crate::memory`.

impl HeapBytes for BodyDigest {
    /// Zero, and the destructuring is the point: this type is two integers today, and the reason
    /// it is two integers is that it used to be a ~3.5 KB `String` per body. A field that brought
    /// the text back would not compile until it was counted.
    fn heap_bytes(&self) -> usize {
        let Self { strong, published } = self;
        strong.heap_bytes() + published.heap_bytes()
    }
}

impl HeapBytes for BehaviorTrace {
    /// The per-event digest array at its reserved capacity. 567,000 events across a 24k-test
    /// corpus is the single largest `Vec` the IR holds, so this is the number to watch.
    fn heap_bytes(&self) -> usize {
        let Self { events, published } = self;
        events.heap_bytes() + published.heap_bytes()
    }
}

impl HeapBytes for Fingerprints {
    /// Sum retained fingerprint heap payloads; scalar flags and the copy-only callback form add
    /// no heap storage.
    fn heap_bytes(&self) -> usize {
        let Self {
            alpha,
            structural,
            behavior,
            trivial,
            callback_form,
        } = self;
        // A `Copy` enum with no payload: no heap of its own, counted for the same reason
        // `trivial` is -- so that adding a field here is a compile error rather than a silent
        // under-count.
        let _ = callback_form;
        alpha.heap_bytes() + structural.heap_bytes() + behavior.heap_bytes() + trivial.heap_bytes()
    }
}

/// FNV-1a over `value`, rendered as 16 lowercase hex digits — the hash every
/// [`TestFingerprint`] field carries.
///
/// The same derivation [`Finding::fingerprint`] uses, exposed because
/// `--emit-test-fingerprints` needs to hash values that live outside this module (notably
/// `analysis.rs`'s private `ContextKey`/`ModifierKey`, whose *types* must stay private while a
/// hash of their contents is published). Deliberately not labeled with
/// [`FINGERPRINT_VERSION`]: that label versions the *finding* identity a baseline stores, and
/// reusing it here would make a change to one silently invalidate consumers of the other. The
/// test-record derivation is versioned by the JSONL record's own `schemaVersion` instead.
///
/// Callers hashing anything composite must feed this a canonical JSON encoding of the whole
/// value rather than a concatenation of its parts: a fingerprint string can itself contain the
/// delimiter syntax `frontend/fingerprint.rs` encodes, which has already produced one collision
/// bug, and JSON string escaping removes the ambiguity that caused it.
pub fn fingerprint_hash(value: &str) -> String {
    stable_hash(value)
}

/// One analyzed test's normalized identity, emitted by `--emit-test-fingerprints`.
///
/// Every field a rule in `crate::analysis` keys on, reduced to a value a consumer can only
/// compare for equality. It exists for the negative-fixture isolation gate
/// (`scripts/check/check-fixture-isolation.mjs`), whose whole difficulty is that a `rule=none`
/// fixture's participants are by definition the tests **no** finding mentions: no reporter
/// enumerated them, so the gate had to approximate the analyzer's own oracle by tokenizing
/// source. A record per analyzed test — including every test in no finding — is that oracle.
///
/// # Hashes, not raw fingerprints
///
/// Every normalized view is carried as a [`fingerprint_hash`], never as the raw serialized form.
/// A consumer only ever compares these for equality; a raw `alpha` of a large body is unbounded
/// text, and this stream is read by agents with finite context windows (see
/// `crate::reporters::jsonl`'s bound on free text). The raw forms are also *not* a stable public
/// contract — they are an internal serialization whose delimiter syntax has already caused a
/// collision bug — and publishing them would invite a consumer to parse them.
///
/// `location` and `title` are analyzed-repository content and are sanitized and bounded by the
/// reporter, exactly like a finding's. The five hashes and the two booleans (`trivial`,
/// `comparable`) are tool-computed and are not.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestFingerprint {
    /// Where the test is declared. Rows of one `.each`/`.for` table share a location.
    pub location: Location,
    /// Calling file when different from the declaration's source file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_file: Option<PathBuf>,
    /// Suite path and title joined the way Vitest composes a full test name
    /// ([`TestCase::full_title`]).
    pub title: String,
    /// Hash of [`Fingerprints::alpha`] — identifiers normalized, literal values preserved.
    ///
    /// Two tests differing only in a literal have equal `structural` and differing `alpha`.
    pub alpha: String,
    /// Hash of [`Fingerprints::structural`] — `alpha` with literal values collapsed to their kind.
    pub structural: String,
    /// Hash of [`Fingerprints::behavior`], the ordered behavior events, encoded as a JSON array
    /// so no event's content can be confused with a separator between events.
    pub behavior: String,
    /// Whether the body is empty or a recognized stub ([`Fingerprints::trivial`]), and therefore
    /// excluded from comparison regardless of what the hashes above say.
    pub trivial: bool,
    /// Whether this test's [`Context`] is resolved well enough to participate in comparison
    /// ([`Context::is_comparable`]).
    ///
    /// `false` means the analyzer excluded this test from every rule up front and the record's
    /// five hashes above were never compared against anything -- the test is `not-compared`, not
    /// verified distinct. Without this field a consumer cannot tell that state apart from a test
    /// that *was* compared and genuinely matched nothing, which is exactly the
    /// not-compared-laundered-into-distinct failure this project's verdict vocabulary exists to
    /// prevent. Derived from [`Context::is_comparable`] directly, never re-derived from
    /// `unresolved` or any other field.
    pub comparable: bool,
    /// Hash of the equality key `analysis.rs` builds from [`Context`] for every context-sensitive
    /// rule (its `ContextKey`).
    ///
    /// The key's *type* stays private to that module — publishing it would make the exact set of
    /// ambient facts a rule compares into a public contract. A hash of it is all a consumer can
    /// use, and is enough to check "these two tests differ in context and in nothing else",
    /// which a source-level comparison can only approximate.
    pub context_key: String,
    /// Hash of the equality key `analysis.rs` builds from a test's modifiers (its `ModifierKey`):
    /// the subset that changes what the test *does*, not merely whether or when it runs.
    pub modifier_key: String,
}

/// One analyzed hook's normalized identity, emitted by `--emit-test-fingerprints` alongside
/// [`TestFingerprint`].
///
/// `redundant-hook` (`analysis::emit_redundant_hook`) compares hook bodies for alpha-equality but,
/// before this record existed, published nothing that let a fixture prove its two hooks were
/// withheld *for the claimed reason* rather than by accident -- the same gap `TestFingerprint`
/// closes for tests, applied to the one declaration kind that had no fingerprint record at all
/// (`not-redundant-hook-different-body.test.ts`'s `unchecked` opt-out).
///
/// # Identity: one record per declaration, not per chain membership
///
/// A [`Hook`] is attached to every test in the scopes under it, so the same declaration appears
/// once per test in [`Context::hooks`]. `analysis::hook_fingerprints` deduplicates by [`Location`]
/// -- exactly `emit_redundant_hook`'s own identity for a hook, which keys its pair map on hook
/// `Location` and never on which test's chain it walked through to find it.
///
/// # Why this is smaller than `TestFingerprint`
///
/// A `Hook` carries no title of its own ([`HookKind::label`] is the only name it has) and no
/// [`Context`] of its own -- it is a component of whichever test's chain includes it, not an
/// independently resolved unit. That asymmetry is why three of `TestFingerprint`'s seven fields
/// are not here at all, rather than present with a value that would always be the same:
///
/// - **`comparable` (omitted).** [`Context::is_comparable`] is a fact about a *test's* resolved
///   ambient state (mocks, fixtures, `.each` tables), not about a hook body -- `fingerprint`
///   always succeeds on a hook body exactly as it does on a test body, so there is no
///   "unresolved hook" state to report. Worse, the same hook declaration can sit in one test's
///   chain whose context resolved and another's that did not (fixtures differ per test even
///   under one shared suite-level hook), so "comparable" is not even a well-defined per-hook fact
///   in the current model -- it is per-(hook, test), and collapsing that to one boolean here would
///   assert something the analyzer never actually decided.
/// - **`contextKey` (omitted).** The key is built from a *test's* [`Context`] -- its hook chain,
///   mocks, fixtures, imports -- and a hook is one of the facts that key is built *from*, not a
///   thing that has its own. Publishing one here would either repeat the enclosing test's key
///   (redundant, and wrong when a hook sits under several suites with different ambient facts) or
///   invent a per-hook context nothing in `analysis.rs` computes.
/// - **`modifierKey` (omitted).** `ModifierKey` is built from [`TestCase::modifiers`]
///   (`.skip`/`.each`/`.fails`/...); a hook registration has no chained modifiers to fold into
///   one, so every hook's key would be identical and the field would tell a consumer nothing it
///   could not already assume.
///
/// `alpha`, `structural`, `behavior`, and `trivial` all survive unchanged: each is a fact about
/// the hook's own body, computed by the identical `fingerprint` pass a test body gets, and each
/// is exactly as meaningful here as there.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookFingerprint {
    /// Where the hook is registered (the call expression's span, matching
    /// [`Hook::location`] and the location a `redundant-hook` finding reports for it).
    pub location: Location,
    /// Which lifecycle hook this is. Load-bearing on its own record, not just a label:
    /// `redundant-hook` only ever compares hooks of matching [`HookKind`], so a `beforeEach` and
    /// an `afterEach` with byte-identical bodies must stay distinguishable here too, or a
    /// consumer checking this stream could "confirm" an isolation claim the analyzer itself would
    /// never make.
    pub kind: HookKind,
    /// Hash of [`Fingerprints::alpha`] -- identifiers normalized, literal values preserved. What
    /// `redundant-hook` itself compares.
    pub alpha: String,
    /// Hash of [`Fingerprints::structural`] -- `alpha` with literal values collapsed to their
    /// kind. Not read by `redundant-hook` today, but as meaningful a view of a hook body as of a
    /// test body, and the view a hook-literal isolation claim (agrees at `structural`, differs at
    /// `alpha`) needs to state itself.
    pub structural: String,
    /// Hash of [`Fingerprints::behavior`], the ordered behavior events.
    pub behavior: String,
    /// Whether the body is empty or a recognized stub ([`Fingerprints::trivial`]), and therefore
    /// excluded from `redundant-hook`'s comparison regardless of what the hashes say.
    pub trivial: bool,
}

/// What the baseline and changed-file modes did to this run's findings.
///
/// Reported rather than inferred. A consumer cannot recover these numbers by counting the findings
/// it received: the whole point of both modes is that some findings were *not* handed over
/// (withheld as untouched) or *not* counted against the gate (accepted by a baseline), and a count
/// of what arrived says nothing about either. See [`crate::modes`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeSummary {
    /// Whether a baseline was applied at all.
    ///
    /// Distinct from `baselined == 0`: "a baseline was consulted and matched nothing" and "no
    /// baseline was in use" are opposite situations, and only the first makes `new_findings`
    /// meaningful.
    pub baseline_applied: bool,
    /// Findings the baseline accounted for, and which therefore do not affect the quality gate.
    pub baselined: usize,
    /// Findings the baseline did not account for. The number `--fail-on-new` compares against.
    pub new_findings: usize,
    /// Whether the report was scoped to files changed since a revision (`--changed-since`).
    pub changed_scope: bool,
    /// Findings withheld because no participant was in a changed file.
    ///
    /// Non-zero forces [`AnalysisResult::incomplete`]: those findings are real, were found, and
    /// are not in this report, so absence of a finding here proves nothing about the rest of the
    /// repository. A changed-files run that withheld nothing is byte-for-byte a full run's report
    /// and stays complete.
    pub withheld_unchanged: usize,
    /// Findings removed because an inline suppression directive
    /// (`// vitest-dedup-disable-next-line`) sat beside a participant.
    ///
    /// Unlike `withheld_unchanged`, non-zero does NOT force [`AnalysisResult::incomplete`]: a
    /// suppression is the repository's own recorded decision, made at the code, with the reason
    /// next to it -- the finding is not missing from the report, it is answered. The count
    /// exists so the decision is visible in every report rather than silent (the same
    /// visibility rule the baselined count follows). Applied before baselining, so a suppressed
    /// finding never consumes baseline allowance; see [`crate::modes::apply_inline_suppressions`].
    pub suppressions_applied: usize,
}

/// What kind of file or directory a run needed and did not find.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrerequisiteKind {
    /// A tsconfig `extends` target that does not exist, typically a generated file.
    TsconfigExtends,
    /// A tsconfig that exists but could not be loaded (empty, unreadable or invalid).
    TsconfigUnreadable,
    /// `node_modules` is absent or empty although `package.json` lists dependencies.
    NodeModules,
}

/// A file or directory the run needed to resolve imports and did not find, with how to get it.
///
/// Detected from the resolver's own failure, so it is only reported when something stayed
/// unresolved because of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingPrerequisite {
    /// Which prerequisite this is.
    pub kind: PrerequisiteKind,
    /// The file or directory, relative to the analysis root where it lies inside it.
    pub path: String,
    /// The command that creates it, or `generate or restore this file`.
    pub fix: String,
    /// How many import resolutions failed because of it, counted per analyzed file.
    pub unresolved_imports: usize,
}

impl MissingPrerequisite {
    /// One sentence for the banner of every reporter.
    fn sentence(&self) -> String {
        let imports = self.unresolved_imports;
        let advice = match self.fix.as_str() {
            fix @ ("generate or restore this file" | "install the dependencies") => {
                format!("{}.", capitalize_first(fix))
            }
            command => format!("Run `{command}`."),
        };
        match self.kind {
            PrerequisiteKind::TsconfigExtends => format!(
                "`{}` is missing: a tsconfig extends it for path aliases and compiler options, \
                 and without it no import under that tsconfig resolves ({imports} import(s) \
                 stayed unresolved). {advice}",
                self.path
            ),
            PrerequisiteKind::TsconfigUnreadable => format!(
                "`{}` exists but could not be read as a tsconfig (unreadable or invalid), \
                 so no import under it resolves ({imports} import(s) stayed unresolved). \
                 Repair or regenerate this file.",
                self.path
            ),
            PrerequisiteKind::NodeModules => format!(
                "`{}` is missing or empty although package.json lists dependencies: \
                 imports of installed packages cannot be resolved ({imports} import(s) stayed \
                 unresolved). {advice}",
                self.path
            ),
        }
    }
}

/// `text` with its first character upper-cased.
fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Why a run could not compare every test, and how much each cause accounts for.
///
/// `incomplete` used to be one `bool` aggregated from nine unrelated conditions, and the banner
/// that read it guessed: it said "analysis hit a limit or a parse failure" unconditionally. On a
/// real 24,147-test run **neither limit was hit** -- 2,563 findings against a `MAX_FINDINGS` of
/// 10,000, no pair-budget exhaustion -- while the actual cause was 1,729 unresolved contexts. A
/// reader was told the tool had been overwhelmed by their repository when it had not been, which
/// is worse than saying nothing: it invites them to dismiss the warning as the tool being
/// undersized rather than to fix the twelve files it could not read.
///
/// So each cause is counted where it is detected, and the banner states the ones that fired.
/// [`IncompleteCauses::reasons`] is the single place that turns them into prose, shared by every
/// reporter -- two banners that describe the same flag in their own words drift, and this one
/// already had.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncompleteCauses {
    /// Directory traversal failed, so the file list itself may be short.
    pub discovery_failed: bool,
    /// A resolved Vitest project's configuration could not be read well enough to scope
    /// discovery, so a file that should have been analyzed may have been set aside.
    pub project_config_degraded: bool,
    /// Every discovered file failed to read: nothing was analyzed at all.
    pub nothing_read: bool,
    /// Files skipped for exceeding [`crate::discovery::MAX_SOURCE_FILE_BYTES`].
    pub files_too_large: usize,
    /// Files skipped because reading them failed.
    pub files_unreadable: usize,
    /// Files the frontend could not read in full.
    pub files_partially_read: usize,
    /// Files whose `globals` setting could not be determined, analyzed as `globals: false`.
    pub files_globals_undetermined: usize,
    /// Executable tests excluded from comparison because their context could not be resolved.
    ///
    /// The largest cause by far on real corpora, and the one the old banner never named.
    pub unresolved_contexts: usize,
    /// The pair-comparison budget ran out before every pair had been compared.
    pub pair_budget_exhausted: bool,
    /// `redundant-hook`'s candidate scan ran out of budget before every hook chain was inspected.
    ///
    /// Separate from [`IncompleteCauses::pair_budget_exhausted`] because they are separate
    /// budgets with separate limits, and folding them together is precisely the misattribution
    /// this struct exists to remove -- a reader told "the pair-comparison budget ran out" would
    /// go looking at `MAX_PAIR_COMPARISONS`, which was never reached.
    pub hook_scan_exhausted: bool,
    /// Files left unanalyzed because the retained-memory budget (`--max-memory`) was reached.
    pub files_over_memory_budget: usize,
    /// Findings discarded by the presentation cap.
    pub findings_truncated: usize,
    /// Findings withheld because no participant lay in a changed file.
    pub withheld_unchanged: usize,
    /// Files or directories the run needed to resolve imports and did not find.
    #[serde(default)]
    pub missing_prerequisites: Vec<MissingPrerequisite>,
}

impl IncompleteCauses {
    /// Whether any cause fired. This is the flag every report and `--fail-on-incomplete` read.
    ///
    /// Written by destructuring so that adding a field without deciding whether it makes a run
    /// incomplete is a compile error rather than a cause that silently never counts -- the same
    /// guard [`crate::memory`] uses, and for the same reason.
    #[must_use]
    pub fn any(&self) -> bool {
        let Self {
            discovery_failed,
            project_config_degraded,
            nothing_read,
            files_too_large,
            files_unreadable,
            files_partially_read,
            files_globals_undetermined,
            unresolved_contexts,
            pair_budget_exhausted,
            hook_scan_exhausted,
            files_over_memory_budget,
            findings_truncated,
            withheld_unchanged,
            missing_prerequisites,
        } = self;
        *discovery_failed
            || *project_config_degraded
            || *nothing_read
            || !missing_prerequisites.is_empty()
            || *pair_budget_exhausted
            || *hook_scan_exhausted
            || *files_too_large > 0
            || *files_unreadable > 0
            || *files_partially_read > 0
            || *files_globals_undetermined > 0
            || *unresolved_contexts > 0
            || *files_over_memory_budget > 0
            || *findings_truncated > 0
            || *withheld_unchanged > 0
    }

    /// Whether a cause fired that means THE ANALYZED PROJECT could not be fully read: files
    /// the run could not open, finish, or attribute; tests whose context could not be resolved;
    /// or a discovery/config step that set material aside. The CI response this class asks for
    /// is on the repository being analyzed (fix access, split an oversized file, resolve the
    /// config), not on this tool's limits.
    ///
    /// C10's granularity decision, taken as two classes rather than ten flags: `reads` and
    /// `limits` are the two responses a pipeline can actually automate, and
    /// `--fail-on-incomplete` with no value keeps meaning "any" for everything else.
    ///
    /// Every field is enumerated (the ignored ones underscore-bound), never `..`: a new cause
    /// must fail THIS destructure too, not only [`IncompleteCauses::any`]'s, so adding a field
    /// forces a class decision everywhere at compile time (Copilot review, PR #30 round 2).
    /// Deliberately NOT counted: the budget causes and the truncation cap (this tool's limits,
    /// [`IncompleteCauses::hit_limits`]) and `withheld_unchanged` (a run-mode policy,
    /// countable only by the flag's default `any`).
    #[must_use]
    pub fn failed_reads(&self) -> bool {
        let Self {
            discovery_failed,
            project_config_degraded,
            nothing_read,
            files_too_large,
            files_unreadable,
            files_partially_read,
            files_globals_undetermined,
            unresolved_contexts,
            pair_budget_exhausted: _pair_budget_exhausted,
            hook_scan_exhausted: _hook_scan_exhausted,
            files_over_memory_budget: _files_over_memory_budget,
            findings_truncated: _findings_truncated,
            withheld_unchanged: _withheld_unchanged,
            missing_prerequisites,
        } = self;
        *discovery_failed
            || *project_config_degraded
            || *nothing_read
            || !missing_prerequisites.is_empty()
            || *files_too_large > 0
            || *files_unreadable > 0
            || *files_partially_read > 0
            || *files_globals_undetermined > 0
            || *unresolved_contexts > 0
    }

    /// Whether a cause fired that means THE ANALYZER'S OWN BOUNDED WORK was capped: budgets or
    /// presentation limits this tool chose, not anything the analyzed repository did. The CI
    /// response this class asks for is raising this tool's limits or splitting the run.
    ///
    /// Every field is enumerated on the same terms as [`IncompleteCauses::failed_reads`]:
    /// a new cause forces a class decision here at compile time too. Deliberately NOT counted:
    /// every read-side cause, and `withheld_unchanged` (a run-mode policy).
    #[must_use]
    pub fn hit_limits(&self) -> bool {
        let Self {
            discovery_failed: _discovery_failed,
            project_config_degraded: _project_config_degraded,
            nothing_read: _nothing_read,
            files_too_large: _files_too_large,
            files_unreadable: _files_unreadable,
            files_partially_read: _files_partially_read,
            files_globals_undetermined: _files_globals_undetermined,
            unresolved_contexts: _unresolved_contexts,
            pair_budget_exhausted,
            hook_scan_exhausted,
            files_over_memory_budget,
            findings_truncated,
            withheld_unchanged: _withheld_unchanged,
            missing_prerequisites: _missing_prerequisites,
        } = self;
        *pair_budget_exhausted
            || *hook_scan_exhausted
            || *files_over_memory_budget > 0
            || *findings_truncated > 0
    }

    /// The prose banner every renderer prints, one sentence per cause that fired,
    /// largest-blast-radius first.
    ///
    /// Ordered so the first line is the one most likely to explain a surprising report: nothing
    /// analyzed at all, then a short file list, then the hard limits (which mean the tool was
    /// genuinely undersized and the old banner claimed unconditionally), then per-file and
    /// per-test coverage gaps. Every sentence carries its own number, because "some tests could
    /// not be compared" and "1,729 tests could not be compared" prompt different actions.
    #[must_use]
    pub fn reasons(&self) -> Vec<String> {
        let mut reasons = Vec::new();
        if self.nothing_read {
            reasons.push(
                "every discovered file failed to read, so nothing was analyzed at all".to_string(),
            );
        }
        if self.discovery_failed {
            reasons.push(
                "directory traversal failed, so files may be missing from this run entirely"
                    .to_string(),
            );
        }
        if self.project_config_degraded {
            reasons.push(
                "a Vitest project's configuration could not be read well enough to scope \
                 discovery, so a file that should have been analyzed may have been set aside"
                    .to_string(),
            );
        }
        reasons.extend(
            self.missing_prerequisites
                .iter()
                .map(MissingPrerequisite::sentence),
        );
        if self.pair_budget_exhausted {
            reasons.push(
                "the pair-comparison budget ran out before every pair had been compared"
                    .to_string(),
            );
        }
        if self.hook_scan_exhausted {
            reasons.push(
                "the `redundant-hook` scan budget ran out before every hook chain had been \
                 inspected"
                    .to_string(),
            );
        }
        if self.files_over_memory_budget > 0 {
            reasons.push(format!(
                "{} file(s) were not analyzed because the memory budget was reached",
                self.files_over_memory_budget
            ));
        }
        if self.findings_truncated > 0 {
            reasons.push(format!(
                "{} finding(s) were dropped by the display cap",
                self.findings_truncated
            ));
        }
        if self.unresolved_contexts > 0 {
            reasons.push(format!(
                "{} test(s) were excluded from comparison because their context could not be \
                 resolved",
                self.unresolved_contexts
            ));
        }
        if self.files_partially_read > 0 {
            reasons.push(format!(
                "{} file(s) could not be read in full",
                self.files_partially_read
            ));
        }
        if self.files_unreadable > 0 {
            reasons.push(format!(
                "{} file(s) were skipped because reading them failed",
                self.files_unreadable
            ));
        }
        if self.files_too_large > 0 {
            reasons.push(format!(
                "{} file(s) were skipped for exceeding the source-size limit",
                self.files_too_large
            ));
        }
        if self.files_globals_undetermined > 0 {
            reasons.push(format!(
                "{} file(s) had an undetermined `globals` setting and were analyzed as `globals: \
                 false`, so an in-source test there may have been missed",
                self.files_globals_undetermined
            ));
        }
        if self.withheld_unchanged > 0 {
            reasons.push(format!(
                "{} finding(s) lying entirely within unchanged files were withheld",
                self.withheld_unchanged
            ));
        }
        reasons
    }
}

/// One file in which **every** executable test was excluded from comparison.
///
/// Carries the test count as well as the path because a bounded list has to show the files that
/// matter most, and "this file contributed 129 tests' worth of nothing" is a different problem
/// from a single unresolved test. Sorting by count is what makes a truncated list honest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlindFile {
    /// Path relative to the analysis root, as every other reported path is.
    pub path: PathBuf,
    /// How many executable tests the file declared, all of them excluded.
    pub tests: usize,
}

/// Why a file was excluded from comparison, in whole or in part.
///
/// A closed set with one code per refusal site, created where the refusal happens (the frontend, or
/// `main.rs` for files that were never parsed) so no consumer ever parses a sentence to learn the
/// cause. The kebab-case spelling is the machine contract in the `json` and `jsonl` reports;
/// variant order is the tie-break when a file has several reasons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExclusionReason {
    /// An imported runner's export could not be resolved, read, or proved to be the same function.
    HelperExportUnreadable,
    /// An imported runner has module, setup or fixture effects that could not be proved inert.
    HelperSetupEffects,
    /// A call, argument or parameter write inside an imported runner has unproved effects.
    HelperEffectsUnproved,
    /// A helper-expansion budget, recursion or depth limit was reached.
    HelperLimit,
    /// Helper bodies are unavailable in a historical revision baseline.
    HelperSourcesUnavailable,
    /// The runner, or a module on its re-export chain, is mocked or mocking is unresolved.
    RunnerMocked,
    /// A title, callback, option or registration expression is outside the supported shapes.
    UnsupportedExpression,
    /// A `vi.mock` factory could not be read.
    MockFactoryUnreadable,
    /// A `test.extend` fixture value could not be read.
    FixtureUnreadable,
    /// Some other ambient fact of the test (dynamic title, unreadable `.each` table, unreadable
    /// nested config) could not be resolved.
    UnresolvedContext,
    /// A statement form the walker does not enter (`with`) may hide declarations.
    UnsupportedStatement,
    /// The parser reported errors; the file was read as far as it parsed.
    ParseError,
    /// The source type is unsupported.
    UnsupportedSource,
    /// A lexical nesting or syntax-unit limit refused the file.
    SourceLimit,
    /// Extraction could not run or panicked and was recovered.
    ExtractionFailed,
    /// A per-file declaration or loop-unrolling budget was reached.
    WalkLimit,
    /// An import resolved outside the analysis root and was not read.
    ImportOutsideRoot,
    /// The governing project's `globals` setting could not be determined.
    GlobalsUndetermined,
    /// The file exceeds the source-size limit and was not read.
    FileTooLarge,
    /// Project configuration could not be read, so no project could be matched to the file and it
    /// was skipped.
    ProjectConfigUnreadable,
    /// The file could not be read.
    Unreadable,
    /// The memory budget was reached before the file was analyzed.
    MemoryBudget,
}

impl ExclusionReason {
    /// Every code, in the order that breaks ties between a file's reasons.
    pub const ALL: [Self; 22] = [
        Self::HelperExportUnreadable,
        Self::HelperSetupEffects,
        Self::HelperEffectsUnproved,
        Self::HelperLimit,
        Self::HelperSourcesUnavailable,
        Self::RunnerMocked,
        Self::UnsupportedExpression,
        Self::MockFactoryUnreadable,
        Self::FixtureUnreadable,
        Self::UnresolvedContext,
        Self::UnsupportedStatement,
        Self::ParseError,
        Self::UnsupportedSource,
        Self::SourceLimit,
        Self::ExtractionFailed,
        Self::WalkLimit,
        Self::ImportOutsideRoot,
        Self::GlobalsUndetermined,
        Self::FileTooLarge,
        Self::ProjectConfigUnreadable,
        Self::Unreadable,
        Self::MemoryBudget,
    ];

    /// The stable kebab-case spelling used in the reports.
    pub fn code(self) -> &'static str {
        match self {
            Self::HelperExportUnreadable => "helper-export-unreadable",
            Self::HelperSetupEffects => "helper-setup-effects",
            Self::HelperEffectsUnproved => "helper-effects-unproved",
            Self::HelperLimit => "helper-limit",
            Self::HelperSourcesUnavailable => "helper-sources-unavailable",
            Self::RunnerMocked => "runner-mocked",
            Self::UnsupportedExpression => "unsupported-expression",
            Self::MockFactoryUnreadable => "mock-factory-unreadable",
            Self::FixtureUnreadable => "fixture-unreadable",
            Self::UnresolvedContext => "unresolved-context",
            Self::UnsupportedStatement => "unsupported-statement",
            Self::ParseError => "parse-error",
            Self::UnsupportedSource => "unsupported-source",
            Self::SourceLimit => "source-limit",
            Self::ExtractionFailed => "extraction-failed",
            Self::WalkLimit => "walk-limit",
            Self::ImportOutsideRoot => "import-outside-root",
            Self::GlobalsUndetermined => "globals-undetermined",
            Self::FileTooLarge => "file-too-large",
            Self::ProjectConfigUnreadable => "project-config-unreadable",
            Self::Unreadable => "unreadable",
            Self::MemoryBudget => "memory-budget",
        }
    }
}

/// One file excluded from comparison in whole or in part, for the machine reports.
///
/// Files that were never parsed (`file-too-large`, `unreadable`, ...) carry `tests_total == 0`: their
/// test count is unknown, not zero.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcludedFile {
    /// Path relative to the analysis root.
    pub path: PathBuf,
    /// The first reason recorded for the file.
    pub reason: ExclusionReason,
    /// Every other distinct reason, in [`ExclusionReason`] order.
    pub other_reasons: Vec<ExclusionReason>,
    /// A sentence naming the cause of `reason`; for a refused helper, the one the terminal prints.
    pub detail: String,
    /// Executable tests of the file excluded from comparison.
    pub tests_excluded: usize,
    /// Executable tests the file declared; `0` when the file was not parsed.
    pub tests_total: usize,
}

impl ExcludedFile {
    /// Whether every executable test of the file was excluded (membership in `blind_files`).
    pub fn every_test_excluded(&self) -> bool {
        self.tests_total > 0 && self.tests_excluded == self.tests_total
    }
}

/// Files and tests attributed to one primary [`ExclusionReason`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExclusionTally {
    /// The primary reason.
    pub reason: ExclusionReason,
    /// Files whose primary reason it is.
    pub files: usize,
    /// Excluded tests in those files.
    pub tests: usize,
}

/// Run-level totals over every excluded file, counted before any list cap.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExclusionSummary {
    /// Files excluded in whole or in part.
    pub files: usize,
    /// Excluded tests in them.
    pub tests: usize,
    /// Files counted above but left out of [`AnalysisResult::excluded_files`] by its cap.
    pub omitted: usize,
    /// Primary reasons, most files first, then most tests, then code order.
    pub reasons: Vec<ExclusionTally>,
}

/// The outcome of one analysis run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AnalysisResult {
    /// Findings in deterministic order.
    pub findings: Vec<Finding>,
    /// How many test cases were discovered.
    pub tests_analyzed: usize,
    /// How many files were parsed.
    pub files_analyzed: usize,
    /// Why absence of a finding proves nothing, and how much each cause accounts for.
    ///
    /// Replaces a bare `bool`. See [`IncompleteCauses`] for why a flag that could not say which
    /// of its nine causes fired was actively misleading rather than merely terse.
    pub incomplete_causes: IncompleteCauses,
    /// How many tests were excluded from comparison because their context could not be resolved.
    ///
    /// A structured field rather than a line of prose on stderr. The external corpus gate needs
    /// this number to tell a real precision improvement from coverage quietly narrowing, and it
    /// was previously recovering it by regex over a human-readable diagnostic — so rewording that
    /// sentence would have silently broken the gate. `incomplete` says *that* coverage degraded;
    /// this says *how much*, which is the difference between a run worth trusting and one that
    /// analyzed almost nothing.
    pub unresolved_tests: usize,
    /// Files in which **every** executable test was excluded from comparison, sorted.
    ///
    /// Distinct from [`AnalysisResult::unresolved_tests`] because the two mislead in different
    /// ways. A partially excluded file still appears in the report, and its excluded tests qualify
    /// findings the reader can see. A wholly excluded file produces no findings at all, so it never
    /// appears anywhere in the output -- and a report that never mentions a file cannot be told
    /// apart from one that examined it and found nothing.
    ///
    /// This is the `not-compared` verdict presenting as `distinct` at the file level. The corpus
    /// count alone cannot surface it: 620 excluded tests read as a small fraction of 24,147 right
    /// up until one notices they are all of the tests in 35 particular files.
    pub blind_files: Vec<BlindFile>,
    /// Every file excluded from comparison in whole or in part, sorted by path and capped at
    /// `exclusions::MAX_EXCLUDED_FILES`; [`AnalysisResult::exclusions`] counts past the cap.
    #[serde(default)]
    pub excluded_files: Vec<ExcludedFile>,
    /// Totals and the per-reason histogram behind [`AnalysisResult::excluded_files`].
    #[serde(default)]
    pub exclusions: ExclusionSummary,
    /// Unique test locations in at least one error-severity finding, **before** truncation.
    ///
    /// `threshold_passed`'s numerator must not depend on which findings survived a display cap.
    /// Findings are sorted then truncated to a retention limit, so a run producing more
    /// error-severity findings than that limit could discard enough participants that the retained
    /// percentage passes while the true rate exceeds the configured threshold -- reporting a clean
    /// gate on a corpus that failed it.
    ///
    /// Same treatment as [`AnalysisResult::had_error_finding`], and for the same reason: record the
    /// fact where it is determined rather than re-deriving it from a presentation-limited list.
    pub error_finding_locations: usize,
    /// Whether any error-severity finding was produced, **before** presentation truncation.
    ///
    /// The quality gate must not depend on which findings survived a display cap. Findings are
    /// sorted then truncated to a retention limit, so a run with more than the limit's worth of
    /// warnings ordered ahead of a single error would drop that error and exit 0 — reporting
    /// success on a corpus known to contain an error-level duplicate. Recording the outcome when
    /// it is determined, rather than re-deriving it from the retained list, makes the gate
    /// independent of presentation.
    ///
    /// Baselined findings are excluded from this and from
    /// [`AnalysisResult::error_finding_locations`], for the same reason they are excluded from the
    /// gate: a repository that has recorded its existing debt must be able to pass, or a baseline
    /// would be decorative. Both are still captured before truncation.
    pub had_error_finding: bool,
    /// What `--baseline` and `--changed-since` did to this run. See [`ModeSummary`].
    #[serde(default)]
    pub modes: ModeSummary,
}

impl AnalysisResult {
    /// Whether absence of a finding proves nothing.
    ///
    /// Derived from [`AnalysisResult::incomplete_causes`] rather than stored beside it: a `bool`
    /// kept in step with the causes by hand is two sources of truth that can disagree, and the
    /// disagreement would be silent in exactly the direction that matters -- a run with a cause
    /// recorded and the flag clear reports clean.
    #[must_use]
    pub fn incomplete(&self) -> bool {
        self.incomplete_causes.any()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a model location fixture with the requested start and a five-column/byte endpoint.
    fn location(path: &str, line: u32, column: u32, start_byte: u32) -> Location {
        Location {
            path: PathBuf::from(path),
            span: Span {
                start: Position { line, column },
                end: Position {
                    line,
                    column: column + 5,
                },
                start_byte,
                end_byte: start_byte + 5,
            },
        }
    }

    /// Build a model finding from participant locations/titles with default severity and no
    /// baseline flag.
    fn finding(rule: Rule, participants: &[(&str, u32, &str)]) -> Finding {
        Finding {
            registration_files: Vec::new(),
            rule,
            severity: rule.default_severity(),
            message: "identical bodies under an equivalent context".to_string(),
            locations: participants
                .iter()
                .map(|(path, line, _)| location(path, *line, 3, *line * 100))
                .collect(),
            titles: participants
                .iter()
                .map(|(_, _, title)| title.to_string())
                .collect(),
            inert: participants.iter().map(|_| false).collect(),
            baselined: false,
        }
    }

    // --- verdict: present and correct for every rule --------------------------------------

    #[test]
    fn every_rule_has_the_verdict_the_testing_and_corpus_plan_assigns_it() {
        assert_eq!(Rule::DuplicateTest.verdict(), Verdict::Redundant);
        assert_eq!(
            Rule::ParameterizationCandidate.verdict(),
            Verdict::Consolidatable
        );
        assert_eq!(Rule::NearDuplicateTest.verdict(), Verdict::SimilarUncertain);
        assert_eq!(Rule::CrossFileDuplicate.verdict(), Verdict::Redundant);
        assert_eq!(Rule::DuplicateTestName.verdict(), Verdict::NameCollision);
    }

    #[test]
    fn verdict_is_defined_for_every_rule_ala_exhaustively() {
        // `Rule::ALL` is the single enumeration point every configuration/documentation/reporter
        // surface is required to use; walking it here (rather than hand-listing the rules a
        // second time) means a rule added later is covered automatically instead of only
        // when someone remembers to extend this test too.
        for rule in Rule::ALL {
            let _ = rule.verdict(); // must not panic for any rule; exhaustive match enforces this
        }
        assert_eq!(
            Rule::ALL.len(),
            9,
            "update this test's rule count if a rule is added"
        );
    }

    // --- fingerprint: stability -------------------------------------------------------------

    #[test]
    fn fingerprint_is_unchanged_when_unrelated_lines_are_inserted_above_the_finding() {
        let original = finding(
            Rule::DuplicateTest,
            &[
                ("src/math.test.ts", 10, "math > adds two numbers"),
                ("src/math.test.ts", 40, "math > adds two numbers again"),
            ],
        );

        // Simulate inserting an import (and a lot of unrelated code) at the top of the file:
        // every location shifts down by the same number of lines and byte offsets, but nothing
        // about what the finding *is* changes.
        let mut shifted = original.clone();
        for location in &mut shifted.locations {
            location.span.start.line += 200;
            location.span.end.line += 200;
            location.span.start.column += 3;
            location.span.end.column += 3;
            location.span.start_byte += 8_000;
            location.span.end_byte += 8_000;
        }

        assert_eq!(
            original.fingerprint(),
            shifted.fingerprint(),
            "inserting unrelated lines above a finding must not change its fingerprint"
        );
    }

    #[test]
    fn fingerprint_is_unchanged_when_participant_order_within_the_finding_changes() {
        // `locations`/`titles` order is already deterministic by construction, but the
        // fingerprint sorts participants independently so a future change to that ordering can
        // never silently change fingerprints on its own.
        let forward = finding(
            Rule::DuplicateTest,
            &[
                ("src/a.test.ts", 10, "suite > a"),
                ("src/a.test.ts", 20, "suite > b"),
            ],
        );
        let mut reversed = forward.clone();
        reversed.locations.reverse();
        reversed.titles.reverse();

        assert_eq!(forward.fingerprint(), reversed.fingerprint());
    }

    #[test]
    fn fingerprint_is_stable_across_repeated_calls() {
        let value = finding(
            Rule::DuplicateTest,
            &[
                ("src/a.test.ts", 10, "suite > a"),
                ("src/a.test.ts", 20, "suite > b"),
            ],
        );
        assert_eq!(value.fingerprint(), value.fingerprint());
    }

    // --- fingerprint: distinctness -----------------------------------------------------------

    #[test]
    fn fingerprint_differs_when_the_rule_differs() {
        let participants = [
            ("src/a.test.ts", 10, "suite > a"),
            ("src/a.test.ts", 20, "suite > b"),
        ];
        let as_duplicate = finding(Rule::DuplicateTest, &participants);
        let as_duplicate_name = finding(Rule::DuplicateTestName, &participants);
        assert_ne!(as_duplicate.fingerprint(), as_duplicate_name.fingerprint());
    }

    #[test]
    fn fingerprint_differs_when_a_title_differs() {
        let base = finding(
            Rule::DuplicateTest,
            &[
                ("src/a.test.ts", 10, "suite > a"),
                ("src/a.test.ts", 20, "suite > b"),
            ],
        );
        let retitled = finding(
            Rule::DuplicateTest,
            &[
                ("src/a.test.ts", 10, "suite > a"),
                ("src/a.test.ts", 20, "suite > c"),
            ],
        );
        assert_ne!(base.fingerprint(), retitled.fingerprint());
    }

    #[test]
    fn fingerprint_differs_when_the_file_differs() {
        let in_a = finding(
            Rule::DuplicateTest,
            &[
                ("src/a.test.ts", 10, "suite > x"),
                ("src/a.test.ts", 20, "suite > y"),
            ],
        );
        let in_b = finding(
            Rule::DuplicateTest,
            &[
                ("src/b.test.ts", 10, "suite > x"),
                ("src/b.test.ts", 20, "suite > y"),
            ],
        );
        assert_ne!(
            in_a.fingerprint(),
            in_b.fingerprint(),
            "two files with an identically-titled duplicate pair must not collide"
        );
    }

    #[test]
    fn fingerprint_is_independent_of_severity() {
        // Lowering a rule's severity in configuration must not change the identity of the
        // finding it still reports -- otherwise every baseline entry would need re-earning after
        // a config-only change.
        let mut error = finding(
            Rule::DuplicateTest,
            &[
                ("src/a.test.ts", 10, "suite > a"),
                ("src/a.test.ts", 20, "suite > b"),
            ],
        );
        let mut warning = error.clone();
        error.severity = Severity::Error;
        warning.severity = Severity::Warning;
        assert_eq!(error.fingerprint(), warning.fingerprint());
    }

    #[test]
    fn fingerprint_embeds_the_current_version_label() {
        let value = finding(
            Rule::DuplicateTest,
            &[
                ("src/a.test.ts", 10, "suite > a"),
                ("src/a.test.ts", 20, "suite > b"),
            ],
        );
        assert!(
            value.fingerprint().starts_with(FINGERPRINT_VERSION),
            "fingerprint must be prefixed with its version label so a future derivation change \
             is detectable rather than silently invalidating a stored baseline"
        );
    }

    // --- fingerprint: the accepted collision case --------------------------------------------

    #[test]
    fn fingerprint_collides_for_two_findings_that_share_an_identical_participant_identity_set() {
        // The documented, accepted collision: a group of more than two tests sharing one
        // identical (title, normalized body) is reported as a spanning set of pairwise findings
        // (`analysis/pairs.rs`), and if the *same* title text was copy-pasted at every location,
        // two different pairs drawn from that group carry the exact same (path, title)
        // participant set. This is pinned as intentional, not a bug: see
        // `Finding::fingerprint`'s doc comment for why location cannot be folded back in without
        // defeating the stability property above it.
        let pair_one = finding(
            Rule::DuplicateTest,
            &[
                ("src/copy-paste.test.ts", 10, "suite > renders correctly"),
                ("src/copy-paste.test.ts", 20, "suite > renders correctly"),
            ],
        );
        let pair_two = finding(
            Rule::DuplicateTest,
            &[
                ("src/copy-paste.test.ts", 30, "suite > renders correctly"),
                ("src/copy-paste.test.ts", 40, "suite > renders correctly"),
            ],
        );
        assert_ne!(
            pair_one.locations, pair_two.locations,
            "sanity check: these must be genuinely different findings before comparing fingerprints"
        );
        assert_eq!(
            pair_one.fingerprint(),
            pair_two.fingerprint(),
            "two findings sharing an identical (path, title) participant set are accepted to \
             collide by design -- a consumer that must tell them apart still has `locations`"
        );
    }

    #[test]
    fn fingerprint_does_not_collide_once_one_title_is_distinguished() {
        // The same near-collision as above, but with the copy-paste habit broken on one side --
        // proves the collision above is specifically about identical titles, not an accident of
        // this test's fixture.
        let pair_one = finding(
            Rule::DuplicateTest,
            &[
                ("src/copy-paste.test.ts", 10, "suite > renders correctly"),
                ("src/copy-paste.test.ts", 20, "suite > renders correctly"),
            ],
        );
        let pair_two = finding(
            Rule::DuplicateTest,
            &[
                ("src/copy-paste.test.ts", 30, "suite > renders correctly"),
                (
                    "src/copy-paste.test.ts",
                    40,
                    "suite > renders correctly (variant)",
                ),
            ],
        );
        assert_ne!(pair_one.fingerprint(), pair_two.fingerprint());
    }

    /// The hand-written SHA-256 must *be* SHA-256, not merely a deterministic mixing function.
    /// These are the FIPS 180-4 / NIST CAVP vectors, chosen to cover each structurally distinct
    /// padding case: the empty message (padding is the whole block), a short one-block message,
    /// a 56-byte message whose length field does not fit in its own block and forces a second
    /// one, and a two-block message. Any error in the round function, the message schedule, the
    /// big-endian packing, or the length encoding moves at least one of these.
    #[test]
    fn sha256_matches_the_published_vectors() {
        for (message, expected) in [
            (
                "",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                "abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmn\
                 hijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
                "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
            ),
        ] {
            let mut hasher = Sha256::new();
            hasher.update(message.as_bytes());
            let rendered: String = hasher
                .finish()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(rendered, expected, "SHA-256 of {message:?}");
            assert_eq!(
                sha256_128(message),
                u128::from_str_radix(&expected[..32], 16).expect("hex prefix parses"),
                "the 128-bit form must be the leading half of the full digest"
            );
        }
    }

    /// A streaming hash that buffers wrongly can still reproduce the one-shot vectors above while
    /// producing a different digest when the same bytes arrive split differently -- and they do
    /// arrive split, because `serde_json::to_writer` pushes a JSON string through `DigestWriter`
    /// in several `write` calls. Every chunking of the same message must agree.
    #[test]
    fn sha256_is_independent_of_chunking() {
        let message: Vec<u8> = (0..500u32).map(|index| (index % 251) as u8).collect();
        let mut one_shot = Sha256::new();
        one_shot.update(&message);
        let expected = one_shot.finish();
        for chunk in [1usize, 7, 63, 64, 65, 128, 499] {
            let mut hasher = Sha256::new();
            for part in message.chunks(chunk) {
                hasher.update(part);
            }
            assert_eq!(
                hasher.finish(),
                expected,
                "digest must not depend on how the message was split (chunk size {chunk})"
            );
        }
    }

    /// The invariant that makes reducing `alpha`/`structural` to a digest invisible from outside:
    /// `BodyDigest::published` must be *the number the old text-backed code computed*, not merely
    /// a hash of the same text. That number was `fingerprint_hash` over the text's canonical JSON
    /// (`analysis::canonical_json`), so this reproduces that derivation independently and demands
    /// equality. Hash the raw text instead of its JSON, change either FNV constant, or seed the
    /// state differently and this fails -- which is exactly what would silently move every
    /// `alpha`/`structural`/`contextKey` value `--emit-test-fingerprints` has ever published.
    #[test]
    fn body_digest_publishes_the_hash_of_the_texts_canonical_json() {
        for text in [
            "",
            "block(call(expect))",
            // Characters JSON must escape. The encoding is part of the derivation, so a digest
            // taken over the raw bytes would disagree here while agreeing above.
            "quote\" backslash \\ newline \n tab \t",
            "unicode \u{1f600} and \u{7} control",
        ] {
            let expected = fingerprint_hash(
                &serde_json::to_string(text).expect("a string encodes to JSON infallibly"),
            );
            assert_eq!(
                BodyDigest::of(text).published_hex(),
                expected,
                "published digest of {text:?} must match the text-backed derivation"
            );
        }
    }

    /// Same invariant for the behavior events, whose published hash is taken over the JSON
    /// *array* of event texts -- the encoding that keeps one event's content from being read as a
    /// separator between events.
    #[test]
    fn behavior_trace_publishes_the_hash_of_the_event_arrays_canonical_json() {
        for events in [
            vec![],
            vec!["call:expect".to_string()],
            vec![
                "call:expect".to_string(),
                "assert:toBe".to_string(),
                "await".to_string(),
            ],
            // Two events whose concatenation matches a single event's text: the JSON array is
            // what keeps these distinguishable.
            vec!["a\",\"b".to_string()],
        ] {
            let expected = fingerprint_hash(
                &serde_json::to_string(&events).expect("strings encode to JSON infallibly"),
            );
            assert_eq!(
                BehaviorTrace::new(&events).published_hex(),
                expected,
                "published digest of {events:?} must match the text-backed derivation"
            );
        }
    }

    /// A body-less declaration built from `Default` must compare and publish exactly as the
    /// empty-text/empty-event body it stands for did when these fields held a `String` and a
    /// `Vec<String>`. A derived `Default` would zero both numbers instead and silently make every
    /// such declaration compare unequal to a genuinely empty body.
    #[test]
    fn defaults_are_the_digests_of_empty_content() {
        assert_eq!(BodyDigest::default(), BodyDigest::of(""));
        assert_eq!(BehaviorTrace::default(), BehaviorTrace::new(&[]));
        assert!(BehaviorTrace::default().is_empty());
        assert_eq!(BehaviorTrace::default().len(), 0);
    }

    /// Equal text digests equally and different text does not -- the whole basis on which every
    /// duplication rule is entitled to compare digests instead of bodies. The pairs below are
    /// chosen to differ only in ways a weak or truncated hash would lose: one byte, one
    /// transposition, and a trailing character.
    #[test]
    fn digests_separate_bodies_that_differ_at_all() {
        assert_eq!(
            BodyDigest::of("call(expect)"),
            BodyDigest::of("call(expect)")
        );
        for (left, right) in [
            ("call(expect)", "call(expecx)"),
            ("call(expect)", "call(exepct)"),
            ("call(expect)", "call(expect) "),
            ("", " "),
        ] {
            assert_ne!(
                BodyDigest::of(left),
                BodyDigest::of(right),
                "{left:?} and {right:?} must not share a digest"
            );
        }
    }

    /// Events stay individually comparable after digesting -- `subsumed-test` walks two event
    /// lists element by element and `near-duplicate-test` sorts one into a multiset, so a trace
    /// must expose its events in order and must not conflate two traces that hold the same events
    /// in a different order.
    #[test]
    fn behavior_trace_keeps_events_ordered_and_individually_comparable() {
        let ordered = BehaviorTrace::new(&["a".to_string(), "b".to_string()]);
        let reversed = BehaviorTrace::new(&["b".to_string(), "a".to_string()]);
        assert_eq!(ordered.len(), 2);
        assert_eq!(
            ordered.events(),
            &[sha256_128("a"), sha256_128("b")],
            "events must be exposed in source order, one digest each"
        );
        assert_ne!(
            ordered, reversed,
            "two traces over the same events in a different order are different traces"
        );
        assert_eq!(
            {
                let mut sorted = ordered.events().to_vec();
                sorted.sort_unstable();
                sorted
            },
            {
                let mut sorted = reversed.events().to_vec();
                sorted.sort_unstable();
                sorted
            },
            "but their sorted multisets agree, which is what near-duplicate-test compares"
        );
    }

    // --- incomplete causes: which ones fired, and which ones did not ------------------------

    /// Sets one cause to a value that fires it, leaving the rest clear. Named so the tables below
    /// can hold one entry per cause without hand-listing a function type at every use.
    type CauseSetter = fn(&mut IncompleteCauses);

    /// The reason [`IncompleteCauses`] exists at all: a run that hit no limit must not claim it
    /// did. The old `bool` plus its unconditional "analysis hit a limit or a parse failure"
    /// banner is the defect this pins shut -- a clean run has no causes and therefore no prose.
    #[test]
    fn a_run_with_no_cause_is_complete_and_states_no_reason() {
        let causes = IncompleteCauses::default();
        assert!(
            !causes.any(),
            "no cause fired, so the run is not incomplete"
        );
        assert_eq!(
            causes.reasons(),
            Vec::<String>::new(),
            "a complete run must produce no banner text at all"
        );
        assert!(
            !AnalysisResult::default().incomplete(),
            "`incomplete()` is derived from the causes and must agree with them"
        );
    }

    /// [`IncompleteCauses::any`] is written by destructuring so that *adding* a field without
    /// counting it is a compile error. This pins the other half, which no compiler checks: every
    /// field that exists today does flip it. Dropping one clause from the `||` chain would make a
    /// run whose only cause was that clause report itself complete -- silence read as
    /// cleanliness, which is the failure this whole struct exists to prevent.
    ///
    /// The serialized field list is walked rather than a hand-written count, so a field added to
    /// the struct and handled in `any()` but forgotten here fails this test instead of passing it.
    #[test]
    fn every_incomplete_cause_on_its_own_makes_the_run_incomplete() {
        let flips: [(&str, CauseSetter); 14] = [
            ("discoveryFailed", |causes| causes.discovery_failed = true),
            ("projectConfigDegraded", |causes| {
                causes.project_config_degraded = true;
            }),
            ("nothingRead", |causes| causes.nothing_read = true),
            ("filesTooLarge", |causes| causes.files_too_large = 1),
            ("filesUnreadable", |causes| causes.files_unreadable = 1),
            ("filesPartiallyRead", |causes| {
                causes.files_partially_read = 1;
            }),
            ("filesGlobalsUndetermined", |causes| {
                causes.files_globals_undetermined = 1;
            }),
            ("unresolvedContexts", |causes| {
                causes.unresolved_contexts = 1;
            }),
            ("pairBudgetExhausted", |causes| {
                causes.pair_budget_exhausted = true;
            }),
            ("hookScanExhausted", |causes| {
                causes.hook_scan_exhausted = true;
            }),
            ("filesOverMemoryBudget", |causes| {
                causes.files_over_memory_budget = 1;
            }),
            ("findingsTruncated", |causes| causes.findings_truncated = 1),
            ("withheldUnchanged", |causes| causes.withheld_unchanged = 1),
            ("missingPrerequisites", |causes| {
                causes.missing_prerequisites = vec![MissingPrerequisite {
                    kind: PrerequisiteKind::NodeModules,
                    path: "node_modules".to_string(),
                    fix: "npm ci".to_string(),
                    unresolved_imports: 1,
                }];
            }),
        ];

        let serialized = serde_json::to_value(IncompleteCauses::default()).unwrap();
        let declared: Vec<&String> = serialized
            .as_object()
            .expect("IncompleteCauses serializes as an object")
            .keys()
            .collect();
        let exercised: Vec<&str> = flips.iter().map(|(name, _)| *name).collect();
        for field in &declared {
            assert!(
                exercised.contains(&field.as_str()),
                "`{field}` is a cause this test never sets; add it to `flips`"
            );
        }
        assert_eq!(
            declared.len(),
            exercised.len(),
            "every declared cause must be exercised exactly once"
        );

        for (field, set) in flips {
            let mut causes = IncompleteCauses::default();
            set(&mut causes);
            assert!(causes.any(), "`{field}` alone must make the run incomplete");
            let result = AnalysisResult {
                incomplete_causes: causes.clone(),
                ..AnalysisResult::default()
            };
            assert!(
                result.incomplete(),
                "`{field}` alone must make `AnalysisResult::incomplete()` true"
            );
            assert_eq!(
                causes.reasons().len(),
                1,
                "`{field}` alone must produce exactly one sentence, got {:?}",
                causes.reasons()
            );
        }
    }

    /// Each cause renders its own sentence and *only* its own. Asserting the whole `Vec` rather
    /// than `contains` is deliberate: the defect this replaced was a banner that named a limit on
    /// every incomplete run, so the sentences that must be **absent** are exactly what a
    /// `contains`-shaped assertion would have let through. Each counted cause also has to quote
    /// its own number -- "some tests could not be compared" and "1729 tests could not be
    /// compared" prompt different actions.
    #[test]
    fn every_cause_renders_exactly_one_sentence_that_names_only_itself() {
        let cases: [(CauseSetter, &str); 12] = [
            (
                |causes| causes.nothing_read = true,
                "every discovered file failed to read, so nothing was analyzed at all",
            ),
            (
                |causes| causes.discovery_failed = true,
                "directory traversal failed, so files may be missing from this run entirely",
            ),
            (
                |causes| causes.project_config_degraded = true,
                "a Vitest project's configuration could not be read well enough to scope \
                 discovery, so a file that should have been analyzed may have been set aside",
            ),
            (
                |causes| causes.pair_budget_exhausted = true,
                "the pair-comparison budget ran out before every pair had been compared",
            ),
            (
                |causes| causes.hook_scan_exhausted = true,
                "the `redundant-hook` scan budget ran out before every hook chain had been \
                 inspected",
            ),
            (
                |causes| causes.findings_truncated = 7,
                "7 finding(s) were dropped by the display cap",
            ),
            (
                |causes| causes.unresolved_contexts = 1729,
                "1729 test(s) were excluded from comparison because their context could not be \
                 resolved",
            ),
            (
                |causes| causes.files_partially_read = 3,
                "3 file(s) could not be read in full",
            ),
            (
                |causes| causes.files_unreadable = 4,
                "4 file(s) were skipped because reading them failed",
            ),
            (
                |causes| causes.files_too_large = 5,
                "5 file(s) were skipped for exceeding the source-size limit",
            ),
            (
                |causes| causes.files_globals_undetermined = 6,
                "6 file(s) had an undetermined `globals` setting and were analyzed as `globals: \
                 false`, so an in-source test there may have been missed",
            ),
            (
                |causes| causes.withheld_unchanged = 8,
                "8 finding(s) lying entirely within unchanged files were withheld",
            ),
        ];

        for (set, expected) in cases {
            let mut causes = IncompleteCauses::default();
            set(&mut causes);
            assert_eq!(causes.reasons(), vec![expected.to_string()]);
        }
    }

    /// C10: every cause belongs to exactly the class its CI response names, and
    /// `withheld_unchanged` -- a deliberate run-mode policy -- belongs to neither.
    #[test]
    fn incompleteness_causes_split_into_reads_and_limits() {
        let assert_class = |set: fn(&mut IncompleteCauses), reads_class: bool| {
            let mut c = IncompleteCauses::default();
            set(&mut c);
            assert_eq!(c.failed_reads(), reads_class, "{c:?}");
            assert_eq!(c.hit_limits(), !reads_class, "{c:?}");
        };
        // Reads: the analyzed project could not be fully read.
        for set in [
            (|c: &mut IncompleteCauses| c.discovery_failed = true) as fn(&mut _),
            |c: &mut IncompleteCauses| c.project_config_degraded = true,
            |c: &mut IncompleteCauses| c.nothing_read = true,
            |c: &mut IncompleteCauses| c.files_too_large = 1,
            |c: &mut IncompleteCauses| c.files_unreadable = 1,
            |c: &mut IncompleteCauses| c.files_partially_read = 1,
            |c: &mut IncompleteCauses| c.files_globals_undetermined = 1,
            |c: &mut IncompleteCauses| c.unresolved_contexts = 1,
        ] {
            assert_class(set, true);
        }
        // Limits: this tool's own bounded work was capped.
        for set in [
            (|c: &mut IncompleteCauses| c.pair_budget_exhausted = true) as fn(&mut _),
            |c: &mut IncompleteCauses| c.hook_scan_exhausted = true,
            |c: &mut IncompleteCauses| c.files_over_memory_budget = 1,
            |c: &mut IncompleteCauses| c.findings_truncated = 1,
        ] {
            assert_class(set, false);
        }
        // Deliberately in neither: changed-file withholding is a run-mode policy, countable
        // only by the flag's default `any`.
        let withheld = IncompleteCauses {
            withheld_unchanged: 1,
            ..IncompleteCauses::default()
        };
        assert!(!withheld.failed_reads() && !withheld.hit_limits());
        assert!(withheld.any());
    }

    /// A reader acts on the first line. Ordering is therefore behaviour, not presentation: the
    /// causes that invalidate the most -- nothing read, then a short file list -- have to precede
    /// the hard limits, which in turn precede the per-file and per-test gaps. A run that
    /// analyzed nothing and also truncated its findings must not lead with the display cap.
    #[test]
    fn reasons_are_ordered_largest_blast_radius_first() {
        let causes = IncompleteCauses {
            discovery_failed: true,
            project_config_degraded: true,
            nothing_read: true,
            files_too_large: 1,
            files_unreadable: 2,
            files_partially_read: 3,
            files_globals_undetermined: 4,
            unresolved_contexts: 5,
            pair_budget_exhausted: true,
            hook_scan_exhausted: true,
            files_over_memory_budget: 8,
            findings_truncated: 6,
            withheld_unchanged: 7,
            missing_prerequisites: vec![MissingPrerequisite {
                kind: PrerequisiteKind::NodeModules,
                path: "node_modules".to_string(),
                fix: "npm ci".to_string(),
                unresolved_imports: 9,
            }],
        };
        let reasons = causes.reasons();
        let leads: Vec<&str> = reasons
            .iter()
            .map(|reason| {
                reason
                    .split_once(' ')
                    .map(|(_, rest)| rest)
                    .unwrap_or(reason)
            })
            .collect();
        assert_eq!(
            reasons.len(),
            14,
            "every cause fired, so all fourteen speak"
        );
        assert!(reasons[0].starts_with("every discovered file failed to read"));
        assert!(reasons[1].starts_with("directory traversal failed"));
        assert!(reasons[2].starts_with("a Vitest project's configuration"));
        assert!(reasons[3].starts_with("`node_modules` is missing or empty"));
        assert!(reasons[4].starts_with("the pair-comparison budget ran out"));
        assert!(reasons[5].starts_with("the `redundant-hook` scan budget"));
        assert!(leads[6].starts_with("file(s) were not analyzed because the memory budget"));
        assert!(leads[7].starts_with("finding(s) were dropped by the display cap"));
        assert!(leads[8].starts_with("test(s) were excluded from comparison"));
        assert!(leads[9].starts_with("file(s) could not be read in full"));
        assert!(leads[10].starts_with("file(s) were skipped because reading them failed"));
        assert!(leads[11].starts_with("file(s) were skipped for exceeding the source-size limit"));
        assert!(leads[12].starts_with("file(s) had an undetermined `globals` setting"));
        assert!(leads[13].starts_with("finding(s) lying entirely within unchanged files"));
    }

    // --- context equivalence -----------------------------------------------------------------

    /// Build a non-idempotent hook fixture with the requested kind, alpha digest and retained
    /// source text.
    fn hook(kind: HookKind, line: u32, alpha: &str, source: &str) -> Hook {
        Hook {
            kind,
            location: location("src/a.test.ts", line, 1, line * 100),
            body: Fingerprints {
                alpha: BodyDigest::of(alpha),
                ..Fingerprints::default()
            },
            idempotent: false,
            alpha_source: Arc::from(source),
        }
    }

    /// `is_equivalent` compares the ambient facts that gate comparison, and deliberately compares
    /// *less* than the derived `PartialEq` does: two hook chains that agree on kind and `alpha`
    /// are equivalent however far apart they are written, and whatever text
    /// [`Hook::alpha_source`] happens to carry. It is also reflexive and ignores `unresolved` --
    /// "unknown never matches" is enforced by filtering on [`Context::is_comparable`] first, not
    /// by a value that compares unequal to itself, because a non-reflexive relation cannot be a
    /// grouping key.
    #[test]
    fn context_equivalence_compares_ambient_facts_and_ignores_where_a_hook_is_written() {
        let base = Context {
            hooks: vec![hook(HookKind::BeforeEach, 3, "setup()", "setup()")],
            mocked_modules: vec![MockedModule {
                specifier: Arc::from("resolved:/repo/src/dep.ts"),
                factory: MockFactory::Automock,
            }],
            fixtures: vec![FixtureBinding {
                name: "db".to_owned(),
                body: FixtureValue::Literal("42".to_owned()),
            }],
            imports: vec![Arc::from("resolved:/repo/src/helpers.ts")],
            module_setup: Vec::new(),
            config_scope: None,
            unresolved: false,
        };

        assert!(base.is_equivalent(&base), "the relation must be reflexive");

        let moved = Context {
            hooks: vec![hook(HookKind::BeforeEach, 99, "setup()", "different text")],
            ..base.clone()
        };
        assert!(
            base.is_equivalent(&moved),
            "a hook declared elsewhere, with different retained source text but the same alpha, \
             is the same ambient fact"
        );
        assert_ne!(
            base, moved,
            "and the derived equality does see that difference, which is why is_equivalent exists"
        );

        let unresolved = Context {
            unresolved: true,
            ..base.clone()
        };
        assert!(
            base.is_equivalent(&unresolved),
            "is_equivalent must not consult is_comparable"
        );
        assert!(!unresolved.is_comparable());
        assert!(base.is_comparable());

        for (what, other) in [
            (
                "a differing mock",
                Context {
                    mocked_modules: vec![MockedModule {
                        specifier: Arc::from("resolved:/repo/src/other.ts"),
                        factory: MockFactory::Automock,
                    }],
                    ..base.clone()
                },
            ),
            (
                "a differing fixture value",
                Context {
                    fixtures: vec![FixtureBinding {
                        name: "db".to_owned(),
                        body: FixtureValue::Setup("makeRealDb()".to_owned()),
                    }],
                    ..base.clone()
                },
            ),
            (
                "a differing import identity",
                Context {
                    imports: vec![Arc::from("resolved:/repo/src/elsewhere.ts")],
                    ..base.clone()
                },
            ),
            (
                "an extra hook",
                Context {
                    hooks: vec![
                        hook(HookKind::BeforeEach, 3, "setup()", "setup()"),
                        hook(HookKind::AfterEach, 9, "teardown()", "teardown()"),
                    ],
                    ..base.clone()
                },
            ),
            (
                "the same body registered on a different hook",
                Context {
                    hooks: vec![hook(HookKind::AfterEach, 3, "setup()", "setup()")],
                    ..base.clone()
                },
            ),
            (
                "a hook body that differs",
                Context {
                    hooks: vec![hook(HookKind::BeforeEach, 3, "setup(2)", "setup(2)")],
                    ..base.clone()
                },
            ),
        ] {
            assert!(
                !base.is_equivalent(&other),
                "{what} must make two contexts non-equivalent"
            );
            assert!(
                !other.is_equivalent(&base),
                "{what} must make two contexts non-equivalent in both directions"
            );
        }
    }

    // --- small accessors whose answers other layers key on ------------------------------------

    /// Only a `Static` title has a statically known runtime value. `Dynamic` and `Parameterized`
    /// both carry readable text, so a check written against the *payload* rather than the variant
    /// would call them static and let a rule key on a title it cannot actually predict.
    #[test]
    fn only_a_static_title_claims_a_statically_known_runtime_value() {
        assert!(Title::Static("adds two numbers".to_owned()).is_static());
        assert!(!Title::Dynamic("user ${_} logs in".to_owned()).is_static());
        assert!(!Title::Parameterized("adds $a and $b".to_owned()).is_static());
        assert_eq!(
            Title::Dynamic("user ${_} logs in".to_owned()).display(),
            "user ${_} logs in",
            "every variant displays its own payload, static or not"
        );
        assert_eq!(
            Title::Parameterized("adds $a and $b".to_owned()).display(),
            "adds $a and $b"
        );
    }

    /// A hook has no title, so `redundant-hook` reports this label as the participant's name. Two
    /// kinds sharing a spelling would make one finding name the wrong declaration.
    #[test]
    fn every_hook_kind_labels_itself_with_its_own_source_spelling() {
        assert_eq!(HookKind::BeforeAll.label(), "beforeAll");
        assert_eq!(HookKind::BeforeEach.label(), "beforeEach");
        assert_eq!(HookKind::AfterEach.label(), "afterEach");
        assert_eq!(HookKind::AfterAll.label(), "afterAll");
    }

    /// `from_id` is the inverse of `id`, and it is what turns a `.vitest-dedup.json` key or a
    /// `--rule` flag back into a rule. A rule whose id did not round-trip would be configurable
    /// in name only. Driven from `Rule::ALL` so a rule added later is covered without editing
    /// this test.
    #[test]
    fn rule_from_id_round_trips_every_rule_and_rejects_anything_else() {
        for rule in Rule::ALL {
            assert_eq!(
                Rule::from_id(rule.id()),
                Some(*rule),
                "`{}` must resolve back to itself",
                rule.id()
            );
        }
        assert_eq!(Rule::from_id("duplicate_test"), None);
        assert_eq!(Rule::from_id("DuplicateTest"), None);
        assert_eq!(Rule::from_id("duplicate-test "), None);
        assert_eq!(Rule::from_id(""), None);
    }

    /// The `Arc<str>` fields must encode exactly as the `String`s they replaced, in both
    /// directions. `analysis`'s `ContextKey` publishes a hash of its own canonical JSON as
    /// `TestFingerprint::context_key`, and that JSON contains a hook's `alpha_source` text -- a
    /// different encoding here (a `{"...": ...}` wrapper, say, which is what serde's own `rc`
    /// support would not give but a hand-rolled module easily could) would silently move a value
    /// consumers have already recorded.
    #[test]
    fn arc_str_fields_encode_as_plain_strings_and_read_back_unchanged() {
        let hook = hook(HookKind::BeforeEach, 3, "setup()", "call(setup)");
        let encoded = serde_json::to_string(&hook).unwrap();
        assert!(
            encoded.contains(r#""alpha_source":"call(setup)""#),
            "a hook's retained alpha text must be a bare JSON string, got {encoded}"
        );
        assert_eq!(
            serde_json::from_str::<Hook>(&encoded).unwrap(),
            hook,
            "and must read back as the same hook"
        );

        let module = MockedModule {
            specifier: Arc::from("resolved:/repo/src/dep.ts"),
            factory: MockFactory::Factory(Arc::from("() => ({ ok: true })")),
        };
        let encoded = serde_json::to_string(&module).unwrap();
        assert_eq!(
            encoded,
            r#"{"specifier":"resolved:/repo/src/dep.ts","factory":{"Factory":"() => ({ ok: true })"}}"#,
            "both shared fields are bare JSON strings, not wrapped values"
        );
        assert_eq!(
            serde_json::from_str::<MockedModule>(&encoded).unwrap(),
            module
        );
    }
}
