//! Retained-heap accounting for the intermediate representation.
//!
//! This module answers one question and refuses to answer it vaguely: **how many bytes of heap
//! does the analyzer still hold once extraction is done?** That number is the one that scales with
//! the corpus. A peak memory measurement does not answer it -- see [`MemoryReport::peak_rss_bytes`]
//! -- and `size_of` does not either, since every interesting cost in [`crate::model`] sits behind
//! a `String`, a `Vec`, or an `Arc<str>`.
//!
//! # What "retained" means here
//!
//! [`HeapBytes::heap_bytes`] reports the bytes of heap allocation **reachable from** a value,
//! excluding the value's own inline footprint. A `TestCase` sitting in a `Vec<TestCase>` has its
//! inline `size_of::<TestCase>()` paid for by that vector's backing array, which
//! [`FieldBytes::spine`] accounts for once, from the vector's *capacity*. So for the whole IR:
//!
//! ```text
//! retained = tests.capacity() * size_of::<TestCase>()  +  sum over tests of test.heap_bytes()
//! ```
//!
//! and nothing is counted twice.
//!
//! # Capacity, never length
//!
//! Every `String`, `PathBuf`, and `Vec` contributes its **capacity**, not its length. This is not
//! a rounding detail: the 947 MB -> 149 MB regression this module exists to prevent a repeat of
//! was `push_str` growth doubling a buffer past what it held, which a length-based accounting
//! reports as free. A collection that has reserved 4 KB to hold 40 bytes costs 4 KB, and this
//! module says 4 KB.
//!
//! # `Arc<str>`: proportional apportionment
//!
//! [`crate::model::Hook::alpha_source`], [`crate::model::MockFactory::Factory`], and
//! [`crate::model::MockedModule::specifier`] are `Arc<str>` precisely because one declaration is
//! copied into thousands of tests. That leaves the accounting with a choice, and both obvious
//! answers are wrong in a way that would be invisible in the output:
//!
//! - **Charge every holder the full allocation.** On a 24k-test corpus one `beforeEach` body is
//!   reachable from ~40,000 hook-chain memberships, so this reports tens of gigabytes for a few
//!   megabytes of actual heap -- a number three orders of magnitude off that would quietly mean
//!   nothing.
//! - **Charge the first holder and skip the rest.** Exact, but needs a pointer-identity set
//!   threaded through every call, which makes `heap_bytes` non-local (a value's cost would depend
//!   on what was measured before it) and allocates a corpus-sized hash set to measure a corpus.
//!
//! This module charges each holder an **equal share**: `allocation / Arc::strong_count`, rounded
//! up. The shares over all holders sum back to the allocation, so the total is exact rather than
//! an estimate, and `heap_bytes` stays a pure function of the value in hand.
//!
//! Two honest caveats, stated rather than hidden:
//!
//! - Rounding up over-counts a shared allocation by at most `holders - 1` bytes. On the real
//!   corpus that is a few kilobytes against ~150 MB.
//! - If some holder of the same allocation lives **outside** the measured graph, its share is not
//!   in the total, so the total under-counts by exactly that outside share. That is the intended
//!   reading: the question is what the IR owns, and a string the IR shares with a cache elsewhere
//!   is only partly the IR's cost.
//!
//! # Why this cannot rot
//!
//! Every implementation in this module **destructures** its type (`let Self { a, b, c } = self;`)
//! instead of reading named fields, and every enum match lists every variant with no `_` arm.
//! Adding a field to an IR struct, or a variant to an IR enum, is therefore a compile error until
//! someone says what it costs. The same discipline runs one level up: [`FieldBytes::add_test`]
//! destructures [`TestCase`] and [`FieldBytes::total`] destructures itself, so a new field cannot
//! be added to the IR without also being placed in the published per-field breakdown, and the
//! breakdown cannot stop summing to the reported total.
//!
//! Every field is routed through `HeapBytes` even when it obviously costs nothing (a `u32`, a
//! `bool`), which is the second half of the guard: a field whose type has no `HeapBytes`
//! implementation does not compile either, so a newly introduced heap-holding type cannot be
//! waved through as "probably free".

use crate::model::{
    Context, DeclarationKind, EachRows, EachTable, FixtureBinding, FixtureValue, Hook, HookKind,
    Location, MockFactory, MockedModule, Modifier, Position, Span, TestCase, Title,
};
use std::path::PathBuf;
use std::sync::Arc;

/// The heap a value keeps alive, in bytes, not counting the value's own inline footprint.
///
/// Implementations must destructure rather than read named fields, and must match enum variants
/// exhaustively without a `_` arm. See the module documentation for why that is the whole point
/// of this trait rather than a style preference.
pub trait HeapBytes {
    /// Bytes of heap allocation reachable from `self`.
    ///
    /// Counts capacity, not length. Shared `Arc<str>` text is apportioned across its holders --
    /// see the module documentation.
    fn heap_bytes(&self) -> usize;
}

/// Implements [`HeapBytes`] as zero for types that are entirely inline.
///
/// Spelled out per type rather than as a blanket `impl<T: Copy>`: a blanket implementation would
/// silently absorb a future `Copy` type that did own heap (none exist today, but the guard costs
/// nothing), and would collide with the collection implementations below.
macro_rules! inline_only {
    ($($type:ty),+ $(,)?) => {
        $(
            impl HeapBytes for $type {
                /// Inline-only values retain no separately owned heap bytes.
                fn heap_bytes(&self) -> usize {
                    0
                }
            }
        )+
    };
}

inline_only!(bool, u32, u64, u128, usize, Option<u32>);

impl HeapBytes for String {
    /// Capacity, not length. See the module documentation for why the difference is the entire
    /// reason this accounting exists.
    fn heap_bytes(&self) -> usize {
        self.capacity()
    }
}

impl HeapBytes for PathBuf {
    /// Capacity of the underlying `OsString`, for the same reason [`String`] reports capacity.
    fn heap_bytes(&self) -> usize {
        self.capacity()
    }
}

impl<T: HeapBytes> HeapBytes for Vec<T> {
    /// The backing array at its reserved capacity, plus whatever the live elements point at.
    ///
    /// Only `len` elements exist, so only `len` elements can own heap; the remaining
    /// `capacity - len` slots are uninitialized memory that still costs its inline size. Both
    /// halves are counted, which is why a vector that grew and then shrank still reports the
    /// buffer it never gave back.
    fn heap_bytes(&self) -> usize {
        self.capacity() * size_of::<T>() + self.iter().map(HeapBytes::heap_bytes).sum::<usize>()
    }
}

impl HeapBytes for Arc<str> {
    /// This holder's share of the shared allocation. See the module documentation for why a share
    /// rather than the whole thing or nothing.
    fn heap_bytes(&self) -> usize {
        let holders = Arc::strong_count(self).max(1);
        arc_str_allocation(self.len()).div_ceil(holders)
    }
}

/// The heap an `Arc<str>` of `len` bytes occupies, header included.
///
/// `Arc`'s allocation is its private `ArcInner`: a strong count, a weak count, then the payload,
/// laid out and padded to pointer alignment. Two counters are 16 bytes on every platform this
/// tool builds for, and omitting them would under-report a corpus holding thousands of short
/// shared strings by more than the strings themselves cost.
fn arc_str_allocation(len: usize) -> usize {
    (2 * size_of::<usize>() + len).next_multiple_of(align_of::<usize>())
}

impl HeapBytes for Position {
    /// Count the position fields, which contain no heap payload.
    fn heap_bytes(&self) -> usize {
        let Self { line, column } = self;
        line.heap_bytes() + column.heap_bytes()
    }
}

impl HeapBytes for Span {
    /// Account for span endpoints and byte offsets without adding scalar storage to heap
    /// totals.
    fn heap_bytes(&self) -> usize {
        let Self {
            start,
            end,
            start_byte,
            end_byte,
        } = self;
        start.heap_bytes() + end.heap_bytes() + start_byte.heap_bytes() + end_byte.heap_bytes()
    }
}

impl HeapBytes for Location {
    /// Sum retained path allocation and span heap accounting for this location.
    fn heap_bytes(&self) -> usize {
        let Self { path, span } = self;
        path.heap_bytes() + span.heap_bytes()
    }
}

impl HeapBytes for Title {
    /// Count the text payload held by the active title variant.
    fn heap_bytes(&self) -> usize {
        match self {
            Self::Static(text) | Self::Dynamic(text) | Self::Parameterized(text) => {
                text.heap_bytes()
            }
        }
    }
}

impl HeapBytes for DeclarationKind {
    /// Declaration kinds are payload-free and retain no heap allocation.
    fn heap_bytes(&self) -> usize {
        match self {
            Self::Test | Self::Suite | Self::Benchmark => 0,
        }
    }
}

impl HeapBytes for Modifier {
    /// Count condition/unknown text or each-table payloads; fixed modifier variants contribute
    /// zero.
    fn heap_bytes(&self) -> usize {
        match self {
            Self::Skip
            | Self::Only
            | Self::Todo
            | Self::Fails
            | Self::Concurrent
            | Self::Sequential
            | Self::Shuffle => 0,
            Self::SkipIf(condition) | Self::RunIf(condition) | Self::Unknown(condition) => {
                condition.heap_bytes()
            }
            Self::Each(table) => table.heap_bytes(),
        }
    }
}

impl HeapBytes for EachTable {
    /// Account for retained rows in the each-table representation.
    fn heap_bytes(&self) -> usize {
        let Self { is_for, rows } = self;
        is_for.heap_bytes() + rows.heap_bytes()
    }
}

impl HeapBytes for EachRows {
    /// Count literal row storage or the unreadable table identity payload.
    fn heap_bytes(&self) -> usize {
        match self {
            Self::Literal(rows) => rows.heap_bytes(),
            Self::Unreadable(identity) => identity.heap_bytes(),
        }
    }
}

impl HeapBytes for HookKind {
    /// Hook-kind variants contain no heap payload.
    fn heap_bytes(&self) -> usize {
        match self {
            Self::BeforeAll | Self::BeforeEach | Self::AfterEach | Self::AfterAll => 0,
        }
    }
}

impl HeapBytes for Hook {
    /// Sum the hook location, fingerprint and retained alpha-source heap payloads.
    fn heap_bytes(&self) -> usize {
        let Self {
            kind,
            location,
            body,
            idempotent,
            alpha_source,
        } = self;
        kind.heap_bytes()
            + location.heap_bytes()
            + body.heap_bytes()
            + idempotent.heap_bytes()
            + alpha_source.heap_bytes()
    }
}

impl HeapBytes for MockFactory {
    /// Count explicit factory source text; automock and unresolved markers allocate no heap
    /// payload.
    fn heap_bytes(&self) -> usize {
        match self {
            Self::Automock | Self::Unresolved => 0,
            Self::Factory(source) => source.heap_bytes(),
        }
    }
}

impl HeapBytes for MockedModule {
    /// Sum retained mock specifier and factory payloads.
    fn heap_bytes(&self) -> usize {
        let Self { specifier, factory } = self;
        specifier.heap_bytes() + factory.heap_bytes()
    }
}

impl HeapBytes for FixtureValue {
    /// Count setup/literal fixture evidence, with zero heap bytes for an unresolved marker.
    fn heap_bytes(&self) -> usize {
        match self {
            Self::Setup(text) | Self::Literal(text) => text.heap_bytes(),
            Self::Unresolved => 0,
        }
    }
}

impl HeapBytes for FixtureBinding {
    /// Count the fixture name and its retained body evidence.
    fn heap_bytes(&self) -> usize {
        let Self { name, body } = self;
        name.heap_bytes() + body.heap_bytes()
    }
}

impl HeapBytes for Context {
    /// Delegates to [`ContextFieldBytes`] so the total and the published breakdown are computed
    /// from one destructuring of [`Context`] and can never disagree.
    fn heap_bytes(&self) -> usize {
        ContextFieldBytes::of(self).total()
    }
}

impl HeapBytes for TestCase {
    /// Delegates to [`FieldBytes`] for the same reason [`Context`] delegates: one destructuring,
    /// one answer. `spine` is zero here -- a lone `TestCase` has no backing array of its own.
    fn heap_bytes(&self) -> usize {
        let mut totals = FieldBytes::default();
        totals.add_test(self);
        totals.total()
    }
}

/// The retained bytes of a [`Context`], split by the field that holds them.
///
/// Every field of [`Context`] has a counterpart here, and [`Self::of`] destructures `Context` to
/// fill them, so a new context field cannot reach the IR without being given a bucket.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextFieldBytes {
    /// The hook chain: each [`Hook`]'s inline size, its location path, its body digests, and this
    /// holder's share of its shared `alpha_source` text.
    pub hooks: usize,
    /// `vi.mock` registrations surviving into this context.
    pub mocked_modules: usize,
    /// `test.extend` fixture bindings.
    pub fixtures: usize,
    /// Anchored module identities the body's free identifiers resolve to.
    pub imports: usize,
    /// Module-scope awaited setup calls and ambient-state calls, held as normalized source text --
    /// plus `config_scope`, the per-directory config's `test` object text. All three share this
    /// bucket rather than adding one: each is a normalized setup/config-derived string, the total
    /// is unaffected, and the published `"memory"` record's shape stays as consumers recorded it.
    /// The name is narrower than the contents, which is recorded here rather than left for a
    /// reader of the report to discover.
    pub module_setup: usize,
    /// Context fields that hold no heap of their own. Always zero, and reported anyway: it is the
    /// visible proof that the breakdown covers every field rather than the ones someone
    /// remembered.
    pub scalars: usize,
}

impl ContextFieldBytes {
    /// Measures one [`Context`].
    pub fn of(context: &Context) -> Self {
        // Destructured, not field-accessed. Adding a field to `Context` breaks this line until it
        // is given a bucket above -- which is the whole enforcement mechanism. See the module doc.
        let Context {
            hooks,
            mocked_modules,
            fixtures,
            imports,
            module_setup,
            config_scope,
            unresolved,
        } = context;
        Self {
            hooks: hooks.heap_bytes(),
            mocked_modules: mocked_modules.heap_bytes(),
            fixtures: fixtures.heap_bytes(),
            imports: imports.heap_bytes(),
            module_setup: module_setup.heap_bytes()
                + config_scope.as_ref().map_or(0, |scope| scope.heap_bytes()),
            scalars: unresolved.heap_bytes(),
        }
    }

    /// Adds another context's bytes into this accumulator.
    pub fn add(&mut self, other: Self) {
        let Self {
            hooks,
            mocked_modules,
            fixtures,
            imports,
            module_setup,
            scalars,
        } = other;
        self.hooks += hooks;
        self.mocked_modules += mocked_modules;
        self.fixtures += fixtures;
        self.imports += imports;
        self.module_setup += module_setup;
        self.scalars += scalars;
    }

    /// The sum of every bucket.
    pub fn total(&self) -> usize {
        // Destructured for the same reason `of` is: a new bucket must be added to the total, and
        // the compiler is what says so.
        let Self {
            hooks,
            mocked_modules,
            fixtures,
            imports,
            module_setup,
            scalars,
        } = self;
        hooks + mocked_modules + fixtures + imports + module_setup + scalars
    }
}

/// The retained bytes of a corpus of [`TestCase`]s, split by the field that holds them.
///
/// The buckets are **disjoint** and sum to exactly the reported total, so a consumer can both
/// attribute a regression to a structure and check that the attribution is complete. That is why
/// `spine` and `scalars` are here alongside the six fields that carry the interesting cost: a
/// breakdown that only listed the interesting fields would not add up, and a gate could not tell
/// "this field grew" from "a field was added and nobody accounted for it".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FieldBytes {
    /// [`TestCase::body`]: the three normalized views, now fixed-width digests plus one
    /// `Vec<EventDigest>` per body.
    pub body: usize,
    /// [`TestCase::context`], broken down further by [`ContextFieldBytes`].
    pub context: ContextFieldBytes,
    /// [`TestCase::suite_path`]: the enclosing suite titles, copied into every descendant test.
    pub suite_path: usize,
    /// [`TestCase::title`].
    pub title: usize,
    /// [`TestCase::modifiers`], including any `.each` table rows recovered into them.
    pub modifiers: usize,
    /// Declaration location and the optional calling-file path retained by [`TestCase`].
    pub location: usize,
    /// The `Vec<TestCase>` backing array at its reserved capacity: `capacity *
    /// size_of::<TestCase>()`.
    ///
    /// Counted once for the whole corpus rather than per test, because that is how it is
    /// allocated. It is the one bucket that is not reachable *from* a `TestCase`, and leaving it
    /// out would under-report a 24k-test corpus by several megabytes of memory that is
    /// unmistakably retained.
    pub spine: usize,
    /// [`TestCase`] fields that hold no heap of their own. Always zero; reported for the reason
    /// [`ContextFieldBytes::scalars`] is.
    pub scalars: usize,
}

impl FieldBytes {
    /// Adds one test's bytes into this accumulator.
    ///
    /// Does not touch [`Self::spine`], which is a property of the corpus vector rather than of
    /// any test; [`MemoryReport::post_extraction`] sets it.
    pub fn add_test(&mut self, test: &TestCase) {
        // Destructured, not field-accessed: adding a field to `TestCase` breaks this line until
        // the new field is given a bucket. See the module doc -- this is the enforcement, and a
        // `test.location` style read here would quietly let a new `String` field cost 100 MB
        // while every number this module publishes stayed the same.
        let TestCase {
            location,
            registration_file,
            kind,
            suite_path,
            title,
            modifiers,
            context,
            body,
            has_body,
            table_row_class,
            row_cells_fully_observable,
        } = test;
        self.location +=
            location.heap_bytes() + registration_file.as_ref().map_or(0, HeapBytes::heap_bytes);
        self.suite_path += suite_path.heap_bytes();
        self.title += title.heap_bytes();
        self.modifiers += modifiers.heap_bytes();
        self.context.add(ContextFieldBytes::of(context));
        self.body += body.heap_bytes();
        self.scalars += kind.heap_bytes()
            + has_body.heap_bytes()
            + table_row_class.heap_bytes()
            + row_cells_fully_observable.heap_bytes();
    }

    /// The sum of every bucket, which is the run's `retainedBytes`.
    pub fn total(&self) -> usize {
        // Destructured for the same reason `add_test` is.
        let Self {
            body,
            context,
            suite_path,
            title,
            modifiers,
            location,
            spine,
            scalars,
        } = self;
        body + context.total() + suite_path + title + modifiers + location + spine + scalars
    }
}

/// Which point in the pipeline a [`MemoryReport`] describes.
///
/// A closed enum rather than a string so a new phase gets a compile-checked identifier here and in
/// the reporter, instead of a literal typed twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPhase {
    /// Immediately after extraction, with the whole `Vec<TestCase>` live and before `analyze`
    /// takes ownership of it.
    ///
    /// The number that matters: this is the working set the analysis phase then operates over,
    /// and it is the one that scales linearly with test count. There is deliberately no
    /// `post-analysis` counterpart today -- `crate::analysis::analyze` consumes the IR and
    /// [`crate::model::AnalysisResult`] retains no [`TestCase`], so a second record measuring this
    /// same field set would be a row of zeros. A row of zeros is worse than an absent record: a
    /// budget gate ratcheting on it would be ratcheting on nothing and would report success
    /// forever. When a phase exists that genuinely retains IR, it gets a variant here.
    PostExtraction,
}

impl MemoryPhase {
    /// The stable identifier published in the record's `phase` field.
    pub fn id(self) -> &'static str {
        match self {
            Self::PostExtraction => "post-extraction",
        }
    }
}

/// One phase's retained-heap accounting, as published on the `jsonl` stream by `--debug-memory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryReport {
    /// Which point in the pipeline this describes.
    pub phase: MemoryPhase,
    /// Executable declarations analyzed, matching the summary record's `testsAnalyzed` exactly.
    ///
    /// The denominator of [`Self::bytes_per_test`]. It counts executable declarations, while
    /// [`Self::declarations_retained`] counts every `TestCase` in the vector (suites included):
    /// the per-test budget this instrumentation exists to govern is expressed per *test*, so the
    /// denominator has to be the number every other surface of this tool calls `testsAnalyzed`,
    /// not a larger one that would flatter the ratio.
    pub tests_analyzed: usize,
    /// Files parsed, matching the summary record's `filesAnalyzed`.
    pub files_analyzed: usize,
    /// Total retained heap for this phase: exactly [`FieldBytes::total`].
    pub retained_bytes: usize,
    /// `retained_bytes / tests_analyzed`, truncated, and `0` when no test was analyzed.
    ///
    /// Zero rather than absent on an empty corpus, deliberately. A `null` here would make
    /// `record.bytesPerTest > budget` evaluate false in every language a gate is likely to be
    /// written in, so a run that measured nothing would sail through a budget check. A consumer
    /// must floor [`Self::tests_analyzed`] before reading this number -- the same
    /// work-performed floor every other gate in this repository carries.
    pub bytes_per_test: usize,
    /// Where the bytes are, disjoint and summing to [`Self::retained_bytes`].
    pub fields: FieldBytes,
    /// How many [`TestCase`] values are retained, suites and benchmarks included.
    ///
    /// Not the same as [`Self::tests_analyzed`], and published so the difference is visible rather
    /// than something a reader has to infer from a ratio that looks slightly off.
    pub declarations_retained: usize,
    /// Peak resident set size of the whole process, in bytes, when the platform can report it.
    ///
    /// **A process high-water mark, never a measurement of what this record accounts for.** RSS
    /// includes every transient the run has touched and not yet returned to the OS: on macOS
    /// libmalloc does not eagerly return freed per-file parser arenas, so peak RSS absorbs parser
    /// churn that was dropped long before this record was built. Two external reviewers have
    /// already read a peak RSS figure as though it were retained bytes and drawn the wrong
    /// conclusion from it, which is why the published record carries an explicit notice alongside
    /// the number instead of leaving the distinction to a doc comment.
    ///
    /// A **run-level** number, not a phase-level one: the high-water mark at the moment the record
    /// was written, which covers every phase up to that point rather than only the phase the rest
    /// of this record measures. See [`Self::with_peak_rss`] for why sampling it with the phase was
    /// wrong.
    ///
    /// It is reported **beside** [`Self::retained_bytes`] and is comparable to neither it nor any
    /// budget, in either direction. It is not an upper bound on retained bytes: untouched reserved
    /// capacity is counted there (a `Vec` that reserved 41,194 slots to hold 33,036 pays for all
    /// 41,194) and pages that were reserved and never written are not resident here, so a
    /// capacity-counted total can legitimately exceed a high-water mark of resident pages. The two
    /// numbers are not two measurements of one thing, which is the reason to compare budgets
    /// against `retained_bytes` and never against this -- and the reason nothing in this project
    /// asserts an ordering between them.
    ///
    /// Supplied by the caller rather than read here: the library is `#![forbid(unsafe_code)]` and
    /// peak RSS has no safe portable API, so the binary reads it and hands it in. `None` on a
    /// platform with no reader wired up.
    pub peak_rss_bytes: Option<u64>,
}

impl MemoryReport {
    /// Measures the extracted IR.
    ///
    /// `tests` is taken as `&Vec<TestCase>` rather than as a slice on purpose: the vector's
    /// *capacity* is part of what is being measured (see [`FieldBytes::spine`]), and a slice
    /// cannot report it. Measuring a slice would silently drop however much the vector over-
    /// reserved while growing -- which is the same length-versus-capacity blind spot this whole
    /// module exists to close.
    #[allow(clippy::ptr_arg)]
    pub fn post_extraction(
        tests: &Vec<TestCase>,
        tests_analyzed: usize,
        files_analyzed: usize,
        peak_rss_bytes: Option<u64>,
    ) -> Self {
        let mut fields = FieldBytes {
            spine: tests.capacity() * size_of::<TestCase>(),
            ..FieldBytes::default()
        };
        for test in tests {
            fields.add_test(test);
        }
        let retained_bytes = fields.total();
        Self {
            phase: MemoryPhase::PostExtraction,
            tests_analyzed,
            files_analyzed,
            retained_bytes,
            // Checked division, not a guarded branch that could be written the other way round:
            // an empty corpus reports 0, and a consumer is told in this field's doc to floor
            // `tests_analyzed` before comparing.
            bytes_per_test: retained_bytes.checked_div(tests_analyzed).unwrap_or(0),
            fields,
            declarations_retained: tests.len(),
            peak_rss_bytes,
        }
    }

    /// Replaces the peak RSS reading, for a caller that samples it later than the phase itself.
    ///
    /// Exists because the two numbers on this record answer questions with different time spans,
    /// and sampling them together got the comparison wrong. `retained_bytes` is a fact about one
    /// instant; peak RSS is a high-water mark, and a high-water mark sampled *during* the run is
    /// not the run's peak. Reading it at extraction time on a real 24k-test corpus reported a
    /// smaller peak than reading it at the end, and a peak that is not the run's peak is simply
    /// the wrong number under the field's own name. The orchestrator therefore builds the report
    /// when the phase ends and stamps the high-water mark when it writes it.
    ///
    /// Note what this is *not* an argument for: the fix is about sampling the field correctly, not
    /// about restoring any ordering against `retained_bytes`. There is none -- see
    /// [`Self::peak_rss_bytes`].
    #[must_use]
    pub fn with_peak_rss(mut self, peak_rss_bytes: Option<u64>) -> Self {
        self.peak_rss_bytes = peak_rss_bytes;
        self
    }
}

/// The default retained-memory budget, in MiB (`--max-memory`).
pub const DEFAULT_MEMORY_BUDGET_MIB: u64 = 2_048;

/// Retained-memory accounting and a current-RSS backstop, checked at file boundaries.
///
/// The figure is the one [`MemoryReport::post_extraction`] reports as `retained_bytes` -- the
/// corpus vector's spine plus each test's [`HeapBytes::heap_bytes`] -- plus any source text the run
/// retains for its reports. Each file is charged once, when its tests are ingested, so checking
/// costs nothing proportional to the corpus.
#[derive(Debug, Clone, Copy)]
pub struct MemoryRoof {
    /// The budget in bytes, or `None` when the roof is disabled.
    budget_bytes: Option<u64>,
    /// Heap and retained text charged so far, excluding the corpus spine.
    charged: u64,
}

impl MemoryRoof {
    /// A roof of `budget_bytes`, or none.
    #[must_use]
    pub fn new(budget_bytes: Option<u64>) -> Self {
        Self {
            budget_bytes,
            charged: 0,
        }
    }

    /// Charges one file's tests, before they join the corpus, and the text retained for it.
    pub fn charge(&mut self, tests: &[TestCase], retained_text: usize) {
        let heap: usize = tests.iter().map(HeapBytes::heap_bytes).sum();
        self.charged = self
            .charged
            .saturating_add(u64::try_from(heap.saturating_add(retained_text)).unwrap_or(u64::MAX));
    }

    /// Check retained bytes and, if still within budget, sample current process RSS once.
    /// RSS has a separate ceiling of 1.5 times the budget. A missing sample preserves the
    /// accounting roof; a disabled roof never calls the reader. Both comparisons are strict.
    #[allow(clippy::ptr_arg)]
    #[must_use]
    pub fn exceeded_with_rss(
        &self,
        corpus: &Vec<TestCase>,
        resident: impl FnOnce() -> Option<u64>,
    ) -> bool {
        let Some(budget) = self.budget_bytes else {
            return false;
        };
        self.exceeded(corpus)
            || resident().is_some_and(|rss| rss > budget.saturating_add(budget / 2))
    }

    /// Whether `corpus`, with everything charged so far, holds more than the budget.
    ///
    /// Takes `&Vec` for its capacity, as [`MemoryReport::post_extraction`] does.
    #[allow(clippy::ptr_arg)]
    #[must_use]
    pub fn exceeded(&self, corpus: &Vec<TestCase>) -> bool {
        let spine = corpus.capacity().saturating_mul(size_of::<TestCase>());
        let held = self
            .charged
            .saturating_add(u64::try_from(spine).unwrap_or(u64::MAX));
        self.budget_bytes.is_some_and(|budget| held > budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Fingerprints, Position};

    /// Build a source-location fixture with a fixed one-line span and the supplied path.
    fn location(path: &str) -> Location {
        Location {
            path: PathBuf::from(path),
            span: Span {
                start: Position { line: 1, column: 1 },
                end: Position { line: 1, column: 9 },
                start_byte: 0,
                end_byte: 8,
            },
        }
    }

    /// Builds a fresh `String` with `extra` bytes of reserved capacity beyond `text`.
    ///
    /// `String::clone()` allocates to fit its length, not its source's capacity -- reusing one
    /// padded string across several assertions via `.clone()` silently drops the padding this
    /// helper exists to keep. Callers that need the same padded value twice must call this twice
    /// with the same arguments rather than clone the result.
    fn padded(text: &str, extra: usize) -> String {
        let mut owned = String::from(text);
        owned.reserve_exact(extra);
        owned
    }

    /// Build a minimal executable test fixture with the supplied static title and empty
    /// context.
    fn test_case(title: &str) -> TestCase {
        TestCase {
            registration_file: None,
            location: location("a.test.ts"),
            kind: DeclarationKind::Test,
            suite_path: Vec::new(),
            title: Title::Static(title.to_owned()),
            modifiers: Vec::new(),
            context: Context::default(),
            body: Fingerprints::default(),
            has_body: true,
            table_row_class: None,
            row_cells_fully_observable: true,
        }
    }

    /// The property the last regression turned on: a buffer that grew past what it holds costs
    /// what it reserved, and a length-based accounting would report the smaller number and miss
    /// it entirely.
    #[test]
    fn a_string_costs_its_capacity_not_its_length() {
        let mut text = String::from("abc");
        text.reserve_exact(1_000);
        assert!(
            text.capacity() >= 1_003,
            "the test needs excess capacity to be measuring anything"
        );
        assert_eq!(
            text.heap_bytes(),
            text.capacity(),
            "a String must report its reserved buffer"
        );
        assert!(
            text.heap_bytes() > text.len(),
            "length-based accounting is exactly what missed the 947 MB regression: len={} \
             capacity={}",
            text.len(),
            text.capacity()
        );
    }

    /// The same property one level up, through a real IR value rather than a bare `String`.
    #[test]
    fn an_ir_value_reports_reserved_capacity_through_the_whole_graph() {
        let mut lean = test_case("a");
        let mut padded = test_case("a");
        let Title::Static(text) = &mut padded.title else {
            unreachable!("built as Static above")
        };
        text.reserve_exact(4_096);

        // Same length, same content, same everything except a reserved buffer nobody is using.
        assert_eq!(lean.full_title(), padded.full_title());
        assert!(
            padded.heap_bytes() >= lean.heap_bytes() + 4_096,
            "excess capacity inside a Title must surface in the TestCase's own accounting: \
             lean={} padded={}",
            lean.heap_bytes(),
            padded.heap_bytes()
        );

        // And through a Vec's backing array, which is the other half of the same blind spot.
        lean.suite_path.reserve_exact(64);
        assert!(
            lean.heap_bytes() >= 64 * size_of::<Title>(),
            "a Vec must report its reserved backing array even while empty"
        );
    }

    /// Shared text is charged once in total, not once per holder. A per-holder charge would have
    /// reported this corpus at four times its real cost.
    #[test]
    fn shared_text_is_apportioned_rather_than_multiplied() {
        let text = "x".repeat(4_000);
        let allocation = arc_str_allocation(text.len());
        let shared: Arc<str> = Arc::from(text.as_str());
        let holders: Vec<Arc<str>> = (0..4).map(|_| Arc::clone(&shared)).collect();

        let apportioned: usize = std::iter::once(&shared)
            .chain(holders.iter())
            .map(HeapBytes::heap_bytes)
            .sum();
        assert_eq!(
            Arc::strong_count(&shared),
            5,
            "the test needs five holders to be measuring apportionment"
        );
        assert!(
            apportioned >= allocation && apportioned <= allocation + 5,
            "the shares of one allocation must sum back to it (up to rounding up per holder): \
             apportioned={apportioned} allocation={allocation}"
        );
        assert!(
            shared.heap_bytes() < allocation,
            "one holder of a five-way share must not be charged the whole allocation"
        );
    }

    /// The published breakdown must account for every byte it reports as the total, or a consumer
    /// cannot tell a field that grew from a field nobody accounted for.
    #[test]
    fn the_field_breakdown_sums_to_the_reported_total() {
        let tests = vec![test_case("a"), test_case("b"), test_case("c")];
        let report = MemoryReport::post_extraction(&tests, 3, 1, None);

        let FieldBytes {
            body,
            context,
            suite_path,
            title,
            modifiers,
            location,
            spine,
            scalars,
        } = report.fields;
        let summed =
            body + context.total() + suite_path + title + modifiers + location + spine + scalars;
        assert_eq!(
            summed, report.retained_bytes,
            "fields must be disjoint and exhaustive: {:?}",
            report.fields
        );
        assert!(
            spine >= 3 * size_of::<TestCase>(),
            "the corpus vector's own backing array is retained memory and must be counted"
        );
    }

    /// The arithmetic a gate reads off the record, including the empty-corpus case that a
    /// division would panic on and a `null` would let slip through a budget check.
    #[test]
    fn bytes_per_test_is_the_quotient_and_survives_an_empty_corpus() {
        let tests = vec![test_case("a"), test_case("b")];
        let report = MemoryReport::post_extraction(&tests, 2, 1, None);
        assert_eq!(report.bytes_per_test, report.retained_bytes / 2);

        let empty: Vec<TestCase> = Vec::new();
        let empty_report = MemoryReport::post_extraction(&empty, 0, 0, None);
        assert_eq!(
            empty_report.bytes_per_test, 0,
            "an empty corpus must report a number a gate can compare, not a panic and not a null"
        );
        assert_eq!(empty_report.retained_bytes, 0);
    }

    #[test]
    fn the_context_breakdown_attributes_a_hook_chain_to_hooks() {
        let mut test = test_case("a");
        test.context.hooks.push(Hook {
            kind: HookKind::BeforeEach,
            location: location("a.test.ts"),
            body: Fingerprints::default(),
            idempotent: false,
            alpha_source: Arc::from("x".repeat(2_000).as_str()),
        });
        let tests = vec![test];
        let report = MemoryReport::post_extraction(&tests, 1, 1, None);

        assert!(
            report.fields.context.hooks >= 2_000,
            "a hook's retained body text must be attributable to the hooks bucket: {:?}",
            report.fields.context
        );
        assert_eq!(
            report.fields.context.mocked_modules, 0,
            "and must not leak into a sibling bucket"
        );
    }

    #[test]
    fn phase_identifiers_are_stable() {
        assert_eq!(MemoryPhase::PostExtraction.id(), "post-extraction");
    }

    /// Every no-payload flag variant costs nothing, and every variant carrying analyzer-recovered
    /// text or a table attributes it. A `Modifier` that fell back to reporting zero for a payload
    /// variant (the same shape as an accidental `_ => 0` arm) would be invisible in every other
    /// test here, since none of them construct a non-empty `modifiers` vector.
    #[test]
    fn modifier_variants_attribute_their_payload_and_flags_cost_nothing() {
        for modifier in [
            Modifier::Skip,
            Modifier::Only,
            Modifier::Todo,
            Modifier::Fails,
            Modifier::Concurrent,
            Modifier::Sequential,
            Modifier::Shuffle,
        ] {
            assert_eq!(
                modifier.heap_bytes(),
                0,
                "a bare modifier flag carries no heap of its own: {modifier:?}"
            );
        }

        let condition_cost = padded("process.env.CI", 300).heap_bytes();
        assert_eq!(
            Modifier::SkipIf(padded("process.env.CI", 300)).heap_bytes(),
            condition_cost,
            "skipIf must attribute its retained condition text"
        );
        assert_eq!(
            Modifier::RunIf(padded("process.env.CI", 300)).heap_bytes(),
            condition_cost,
            "runIf must attribute its retained condition text"
        );
        assert_eq!(
            Modifier::Unknown(padded("process.env.CI", 300)).heap_bytes(),
            condition_cost,
            "an unknown chained modifier must attribute its retained text"
        );

        let mut rows = vec!["a".to_owned()];
        rows.reserve_exact(64);
        let rows_cost = rows.heap_bytes();
        let table = EachTable {
            is_for: true,
            rows: EachRows::Literal(rows),
        };
        assert_eq!(
            Modifier::Each(table).heap_bytes(),
            rows_cost,
            "an each modifier must attribute its recovered table, not report zero like the flags"
        );
    }

    /// The two halves of what `.each`/`.for` can recover are unrelated data (rows vs. an
    /// identity string) and must be attributed independently -- neither arm should silently
    /// compute the other's number.
    #[test]
    fn each_table_and_each_rows_attribute_the_recovered_rows_or_the_unreadable_identity() {
        let mut rows = vec!["a".to_owned(), "b".to_owned()];
        rows.reserve_exact(32);
        let literal_cost = rows.heap_bytes();
        let literal_table = EachTable {
            is_for: false,
            rows: EachRows::Literal(rows),
        };
        assert_eq!(
            literal_table.heap_bytes(),
            literal_cost,
            "a recovered table's rows must be attributed, and is_for costs nothing extra"
        );

        let mut identity = String::from("getCasesA()");
        identity.reserve_exact(500);
        let identity_cost = identity.heap_bytes();
        let unreadable_table = EachTable {
            is_for: true,
            rows: EachRows::Unreadable(identity),
        };
        assert_eq!(
            unreadable_table.heap_bytes(),
            identity_cost,
            "an unreadable table costs its retained identity string, not the rows it never had"
        );
        assert_ne!(
            literal_table.heap_bytes(),
            unreadable_table.heap_bytes(),
            "the two EachRows arms must compute independently rather than collapsing to one number"
        );
    }

    /// `MockFactory` and `MockedModule` each sum more than one field; a bug that drops one of them
    /// (e.g. `MockedModule::heap_bytes` returning only `specifier.heap_bytes()`) would be invisible
    /// unless a factory that costs something is actually attached and measured.
    #[test]
    fn mock_factory_and_mocked_module_attribute_the_specifier_and_the_factory_source() {
        assert_eq!(MockFactory::Automock.heap_bytes(), 0);
        assert_eq!(MockFactory::Unresolved.heap_bytes(), 0);

        let text = "x".repeat(3_000);
        let sole_holder: Arc<str> = Arc::from(text.as_str());
        let expected_factory_cost = arc_str_allocation(sole_holder.len());
        assert_eq!(
            MockFactory::Factory(sole_holder).heap_bytes(),
            expected_factory_cost,
            "a factory with a single holder must be charged the whole allocation"
        );

        let specifier_text = "resolved:/a/b.ts";
        let specifier: Arc<str> = Arc::from(specifier_text);
        let expected_specifier_cost = arc_str_allocation(specifier.len());
        let module_without_factory = MockedModule {
            specifier,
            factory: MockFactory::Automock,
        };
        assert_eq!(
            module_without_factory.heap_bytes(),
            expected_specifier_cost,
            "an automocked module costs only its specifier and must not add anything for the factory"
        );

        let specifier2: Arc<str> = Arc::from(specifier_text);
        let expected_specifier_cost2 = arc_str_allocation(specifier2.len());
        let text2 = "y".repeat(2_000);
        let source2: Arc<str> = Arc::from(text2.as_str());
        let expected_factory_cost2 = arc_str_allocation(source2.len());
        let module_with_factory = MockedModule {
            specifier: specifier2,
            factory: MockFactory::Factory(source2),
        };
        assert_eq!(
            module_with_factory.heap_bytes(),
            expected_specifier_cost2 + expected_factory_cost2,
            "specifier and factory costs must both be present, not one replacing the other"
        );
    }

    /// `FixtureValue` carries text in two of its three variants, and `FixtureBinding` sums its own
    /// name against that body -- a dropped `+` on either field would go unnoticed without a case
    /// that gives both fields a distinguishable, non-zero cost.
    #[test]
    fn fixture_value_and_fixture_binding_attribute_the_body_and_the_name() {
        assert_eq!(FixtureValue::Unresolved.heap_bytes(), 0);

        let setup_cost = padded("setup", 800).heap_bytes();
        assert_eq!(
            FixtureValue::Setup(padded("setup", 800)).heap_bytes(),
            setup_cost,
            "a setup fixture must attribute its fingerprint text"
        );

        let literal_cost = padded("literal", 400).heap_bytes();
        assert_eq!(
            FixtureValue::Literal(padded("literal", 400)).heap_bytes(),
            literal_cost,
            "a literal fixture must attribute its fingerprint text"
        );

        let name_cost = padded("db", 200).heap_bytes();
        let binding = FixtureBinding {
            name: padded("db", 200),
            body: FixtureValue::Setup(padded("setup", 800)),
        };
        assert_eq!(
            binding.heap_bytes(),
            name_cost + setup_cost,
            "the binding must sum its own name and its body's cost, not just one of them"
        );
    }

    /// `Context`'s own `HeapBytes` impl and the `ContextFieldBytes` breakdown a report publishes
    /// for it are documented to be computed from one destructuring so they can never disagree --
    /// this is the test that would notice if a future edit duplicated the logic and let the two
    /// drift apart. It also exercises `Context::heap_bytes` itself, which no other test in this
    /// module calls: `FieldBytes::add_test` reaches `ContextFieldBytes::of` directly and never
    /// goes through the trait method.
    #[test]
    fn context_heap_bytes_agrees_with_its_published_field_breakdown() {
        let context = Context {
            hooks: vec![Hook {
                kind: HookKind::BeforeEach,
                location: location("a.test.ts"),
                body: Fingerprints::default(),
                idempotent: false,
                alpha_source: Arc::from("x".repeat(1_500).as_str()),
            }],
            mocked_modules: vec![MockedModule {
                specifier: Arc::from("resolved:/a.ts"),
                factory: MockFactory::Automock,
            }],
            fixtures: vec![FixtureBinding {
                name: "db".to_owned(),
                body: FixtureValue::Literal("42".to_owned()),
            }],
            imports: ["resolved:/b.ts", "unresolved:pkg"].map(Arc::from).to_vec(),
            module_setup: Vec::new(),
            config_scope: None,
            unresolved: true,
        };

        assert_eq!(
            context.heap_bytes(),
            ContextFieldBytes::of(&context).total(),
            "Context's own HeapBytes impl must agree with the breakdown a report publishes for it"
        );
        assert!(
            context.heap_bytes() > 0,
            "the test needs a non-trivial context, or the two sides could agree by both being zero"
        );
    }

    /// Replacing the peak RSS reading must touch only that field -- the bug this guards against is
    /// sampling it correctly but wiring the setter to leave the stale value in place, or to reset
    /// unrelated fields along with it.
    #[test]
    fn with_peak_rss_replaces_only_the_peak_rss_reading() {
        let tests = vec![test_case("a")];
        let report = MemoryReport::post_extraction(&tests, 1, 1, Some(111));
        assert_eq!(report.peak_rss_bytes, Some(111));

        let updated = report.with_peak_rss(Some(999));
        assert_eq!(
            updated.peak_rss_bytes,
            Some(999),
            "with_peak_rss must replace the reading"
        );
        assert_eq!(
            updated.retained_bytes, report.retained_bytes,
            "replacing the peak RSS reading must not disturb the bytes it was computed alongside"
        );
        assert_eq!(updated.fields, report.fields);
        assert_eq!(updated.tests_analyzed, report.tests_analyzed);

        let cleared = updated.with_peak_rss(None);
        assert_eq!(
            cleared.peak_rss_bytes, None,
            "must also accept clearing the reading"
        );
    }

    /// The `inline_only!` scalars cost nothing regardless of their value -- not just at zero,
    /// which a `size_of::<Self>()` mistake could coincidentally satisfy for a zeroed value. `usize`
    /// is included even though nothing in today's IR is a bare `usize` field (every collection
    /// reports its cost through `capacity()` arithmetic directly, never through this impl) --
    /// it is reachable, just not from production code, so it is tested directly rather than left
    /// unmeasured.
    #[test]
    fn bare_scalars_report_zero_heap_regardless_of_value() {
        assert_eq!(true.heap_bytes(), 0);
        assert_eq!(false.heap_bytes(), 0);
        assert_eq!(0u32.heap_bytes(), 0);
        assert_eq!(u32::MAX.heap_bytes(), 0);
        assert_eq!(0u64.heap_bytes(), 0);
        assert_eq!(u64::MAX.heap_bytes(), 0);
        assert_eq!(0u128.heap_bytes(), 0);
        assert_eq!(u128::MAX.heap_bytes(), 0);
        assert_eq!(0usize.heap_bytes(), 0);
        assert_eq!(usize::MAX.heap_bytes(), 0);
    }

    /// `Title`'s three variants share one match arm bound to the same text; only `Static` is
    /// exercised by [`test_case`], so `Dynamic` and `Parameterized` are otherwise never
    /// constructed anywhere in this module's tests.
    #[test]
    fn title_dynamic_and_parameterized_variants_also_attribute_their_text() {
        let cost = padded("computed name", 700).heap_bytes();

        assert_eq!(
            Title::Dynamic(padded("computed name", 700)).heap_bytes(),
            cost
        );
        assert_eq!(
            Title::Parameterized(padded("computed name", 700)).heap_bytes(),
            cost
        );
        assert_eq!(
            Title::Static(padded("computed name", 700)).heap_bytes(),
            cost
        );
    }

    /// The roof holds exactly its budget and stops one byte past it; without a budget it never
    /// stops.
    #[test]
    fn the_roof_stops_one_byte_past_its_budget() {
        let corpus: Vec<TestCase> = Vec::new();
        let mut roof = MemoryRoof::new(Some(100));
        roof.charge(&[], 100);
        assert!(!roof.exceeded(&corpus), "equal to the budget is within it");
        roof.charge(&[], 1);
        assert!(roof.exceeded(&corpus));

        let mut uncapped = MemoryRoof::new(None);
        uncapped.charge(&[], usize::MAX);
        assert!(!uncapped.exceeded(&corpus));
    }

    /// Current RSS is a separate 1.5x backstop, including exact and overflowing thresholds.
    #[test]
    fn rss_backstop_preserves_thresholds_and_accounting() {
        let corpus = Vec::new();
        let roof = MemoryRoof::new(Some(100));
        for resident in [None, Some(149), Some(150)] {
            assert!(!roof.exceeded_with_rss(&corpus, || resident));
        }
        assert!(roof.exceeded_with_rss(&corpus, || Some(151)));
        assert!(!MemoryRoof::new(Some(3)).exceeded_with_rss(&corpus, || Some(4)));
        assert!(MemoryRoof::new(Some(3)).exceeded_with_rss(&corpus, || Some(5)));
        assert!(!MemoryRoof::new(Some(u64::MAX)).exceeded_with_rss(&corpus, || Some(u64::MAX)));
        let mut charged = roof;
        charged.charge(&[], 101);
        assert!(charged.exceeded_with_rss(&corpus, || None));
        assert!(!MemoryRoof::new(None).exceeded_with_rss(&corpus, || {
            panic!("an uncapped run must not sample RSS")
        }));
        let mut samples = 0;
        assert!(!roof.exceeded_with_rss(&corpus, || {
            samples += 1;
            Some(1)
        }));
        assert_eq!(samples, 1);
    }

    /// The charge is the figure `--debug-memory` reports: the corpus spine plus each test's heap.
    #[test]
    fn the_roof_charges_what_debug_memory_reports() {
        let mut corpus = vec![test_case(&padded("a title", 4_000))];
        let retained = MemoryReport::post_extraction(&corpus, 1, 1, None).retained_bytes as u64;
        let mut roof = MemoryRoof::new(Some(retained));
        roof.charge(&corpus, 0);
        assert!(!roof.exceeded(&corpus));
        corpus.reserve_exact(1);
        assert!(roof.exceeded(&corpus), "the spine's capacity counts too");
    }
}
