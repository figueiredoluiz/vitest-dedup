//! Retained ownership, excluding per-file parser/normalizer scratch.
use super::{Engine, Exclusion, Hook, Hooks, Origin, Run, SiteRecord, TestCase};
use crate::memory::{EngineBytes, HeapBytes};
use crate::model::BlindFile;

impl HeapBytes for Exclusion {
    /// Every exclusion is an inline tag; adding a payload requires an accounting decision.
    fn heap_bytes(&self) -> usize {
        match self {
            Self::RegisteredInsideFunction
            | Self::UnprovedTableRow
            | Self::UnprovedSuiteRow
            | Self::UnprovedRepeatCount
            | Self::EmptyBody
            | Self::NoCallback
            | Self::UnreadableCallback
            | Self::OrderDependentState
            | Self::TruncatedBody
            | Self::NamedCallback
            | Self::Benchmark
            | Self::RegisteredElsewhere
            | Self::UnprovedRunner
            | Self::DynamicBindings
            | Self::UnreadableImport
            | Self::ImportLimit
            | Self::UnreadableManifest
            | Self::NonHoistedMock
            | Self::UnreadableMockFactory
            | Self::UnreadableFixture
            | Self::UnresolvedTitle => 0,
        }
    }
}

impl HeapBytes for TestCase {
    /// Account every owned case field, including display names and retained trace capacity.
    fn heap_bytes(&self) -> usize {
        let Self {
            runtime_row,
            suite_rows,
            origin,
            views,
            span,
            title,
            full_title,
            name,
            row,
            row_class,
            key,
            module_key,
            body,
            parameterization_key,
            trace,
            ordinary_events,
            trace_complete,
            similarity_key,
            subsumption_key,
            callback_form,
            inert,
            bodyless,
            benchmark,
            exclusion,
        } = self;
        [
            runtime_row.heap_bytes(),
            suite_rows.heap_bytes(),
            origin.heap_bytes(),
            views
                .as_ref()
                .map_or(0, |_| size_of::<super::records::Views>()),
            span.heap_bytes(),
            title.heap_bytes(),
            full_title.heap_bytes(),
            name.heap_bytes(),
            row.heap_bytes(),
            row_class.heap_bytes(),
            key.heap_bytes(),
            module_key.heap_bytes(),
            body.heap_bytes(),
            parameterization_key.heap_bytes(),
            trace.heap_bytes(),
            ordinary_events.heap_bytes(),
            trace_complete.heap_bytes(),
            similarity_key.heap_bytes(),
            subsumption_key.heap_bytes(),
            callback_form.heap_bytes(),
            inert.heap_bytes(),
            bodyless.heap_bytes(),
            benchmark.heap_bytes(),
            exclusion.heap_bytes(),
        ]
        .into_iter()
        .fold(0, usize::saturating_add)
    }
}

impl HeapBytes for Origin {
    /// Source and registration ownership are disjoint owned path payloads.
    fn heap_bytes(&self) -> usize {
        let Self {
            path,
            registration_files,
            call,
        } = self;
        path.heap_bytes()
            .saturating_add(registration_files.heap_bytes())
            .saturating_add(call.capacity() * std::mem::size_of::<u32>())
    }
}

impl HeapBytes for Hook {
    /// A hook retains its optional helper path and effect key; locations and phases are inline.
    fn heap_bytes(&self) -> usize {
        let Self {
            path,
            registration_files,
            span,
            timing,
            label,
            scope,
            key,
        } = self;
        [
            path.heap_bytes(),
            registration_files.heap_bytes(),
            span.heap_bytes(),
            timing.heap_bytes(),
            label.heap_bytes(),
            scope.heap_bytes(),
            key.heap_bytes(),
        ]
        .into_iter()
        .fold(0, usize::saturating_add)
    }
}

impl HeapBytes for Hooks {
    /// Both declaration and lifecycle-chain containers include their reserved capacities.
    fn heap_bytes(&self) -> usize {
        let Self {
            declarations,
            chains,
        } = self;
        declarations
            .heap_bytes()
            .saturating_add(chains.heap_bytes())
    }
}

impl HeapBytes for SiteRecord {
    /// Static site kinds are borrowed; chained spelling is owned.
    fn heap_bytes(&self) -> usize {
        let Self {
            kind,
            line,
            line_from,
            column,
            chain,
        } = self;
        [
            kind.heap_bytes(),
            line.heap_bytes(),
            line_from.heap_bytes(),
            column.heap_bytes(),
            chain.heap_bytes(),
        ]
        .into_iter()
        .fold(0, usize::saturating_add)
    }
}

impl HeapBytes for BlindFile {
    /// Blind-file paths remain retained even when no comparable case survives.
    fn heap_bytes(&self) -> usize {
        let Self { path, tests } = self;
        path.heap_bytes().saturating_add(tests.heap_bytes())
    }
}

impl Engine {
    /// Constant-time disjoint accounting: payload ledgers plus live outer vector capacities.
    pub fn memory_fields(&self) -> EngineBytes {
        let Self {
            hooks,
            run,
            tests,
            eligible,
            unresolved,
            empty_tables,
            containers,
            sites,
            diagnostic_hooks,
            files,
            suites,
            blind,
            proven_empty,
            retained,
        } = self;
        let Run {
            modules,
            exports,
            sources,
            diagnostics,
        } = run;
        EngineBytes {
            cases: retained.cases.saturating_add(
                files
                    .capacity()
                    .saturating_mul(size_of::<(std::path::PathBuf, Vec<TestCase>)>()),
            ),
            suites: retained
                .suites
                .saturating_add(
                    suites
                        .capacity()
                        .saturating_mul(size_of::<(std::path::PathBuf, Vec<TestCase>)>()),
                ),
            hooks: retained
                .hooks
                .saturating_add(
                    hooks
                        .capacity()
                        .saturating_mul(size_of::<(std::path::PathBuf, Hooks)>()),
                )
                .saturating_add(diagnostic_hooks.capacity().saturating_mul(size_of::<(
                    std::path::PathBuf,
                    super::records::DiagnosticHook,
                )>())),
            sites: retained.sites.saturating_add(
                sites
                    .capacity()
                    .saturating_mul(size_of::<(std::path::PathBuf, SiteRecord)>()),
            ),
            blind: retained
                .blind
                .saturating_add(blind.capacity().saturating_mul(size_of::<BlindFile>()))
                .saturating_add(
                    proven_empty
                        .capacity()
                        .saturating_mul(size_of::<std::path::PathBuf>()),
                ),
            modules: modules.heap_bytes().saturating_add(sources.heap_bytes()),
            exports: exports.heap_bytes(),
            scalars: tests
                .heap_bytes()
                .saturating_add(eligible.heap_bytes())
                .saturating_add(unresolved.heap_bytes())
                .saturating_add(empty_tables.heap_bytes())
                .saturating_add(containers.heap_bytes())
                .saturating_add(diagnostics.heap_bytes()),
        }
    }

    /// Snapshot the retained populations once, before rule evaluation moves their ownership.
    pub fn memory_report(&self, files_analyzed: usize) -> crate::memory::MemoryReport {
        let declarations = self
            .files
            .iter()
            .chain(self.suites.iter())
            .map(|(_, cases)| cases.len())
            .sum();
        crate::memory::MemoryReport::engine(
            self.memory_fields(),
            self.tests,
            files_analyzed,
            declarations,
            None,
        )
    }

    /// Test cases are consumed only after the final memory snapshot and before rule evaluation.
    pub fn take_files(&mut self) -> Vec<(std::path::PathBuf, Vec<TestCase>)> {
        self.retained.cases = 0;
        std::mem::take(&mut self.files)
    }

    /// Suite cases remain owned by the engine while the rules read them.
    pub fn suites(&self) -> &[(std::path::PathBuf, Vec<TestCase>)] {
        &self.suites
    }

    /// Hook chains remain owned by the engine while the rules read them.
    pub fn hooks(&self) -> &[(std::path::PathBuf, Hooks)] {
        &self.hooks
    }

    /// Transfer optional diagnostic hook ownership without cloning its retained payload.
    pub fn take_diagnostic_hooks(
        &mut self,
    ) -> Vec<(std::path::PathBuf, super::records::DiagnosticHook)> {
        let mut hooks = std::mem::take(&mut self.diagnostic_hooks);
        use crate::memory::HeapBytes;
        self.retained.hooks = self
            .retained
            .hooks
            .saturating_sub(hooks.iter().map(HeapBytes::heap_bytes).sum::<usize>());
        hooks.sort_by(|a, b| (&a.0, a.1.span, a.1.kind).cmp(&(&b.0, b.1.span, b.1.kind)));
        hooks.dedup_by(|a, b| {
            if a.0 != b.0 || a.1.span != b.1.span || a.1.kind != b.1.kind {
                return false;
            }
            if a.1.effect_key != b.1.effect_key {
                b.1.effect_key = None;
            }
            true
        });
        hooks
    }

    /// Reporting takes the saved sites only after analysis has finished.
    pub fn into_sites(self) -> Vec<(std::path::PathBuf, SiteRecord)> {
        self.sites
    }
}

impl HeapBytes for Engine {
    /// The same constant-time retained estimate feeds budget checks and debug reporting.
    fn heap_bytes(&self) -> usize {
        self.memory_fields().total()
    }
}
