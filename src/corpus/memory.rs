//! Retained next-engine ownership, excluding per-file parser/normalizer scratch.
use super::{Engine, Exclusion, Hook, Hooks, Run, SiteRecord, TestCase};
use crate::memory::{EngineBytes, HeapBytes};
use crate::model::BlindFile;

impl HeapBytes for Exclusion {
    /// Every exclusion is an inline tag; adding a payload requires an accounting decision.
    fn heap_bytes(&self) -> usize {
        match self {
            Self::RegisteredInsideFunction
            | Self::EmptyBody
            | Self::NoCallback
            | Self::UnreadableCallback
            | Self::OrderDependentState
            | Self::TruncatedBody
            | Self::NamedCallback
            | Self::Benchmark
            | Self::RegisteredElsewhere => 0,
        }
    }
}

impl HeapBytes for TestCase {
    /// Account every owned case field, including display names and retained trace capacity.
    fn heap_bytes(&self) -> usize {
        let Self {
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
            trace_complete,
            similarity_key,
            subsumption_key,
            callback_form,
            inert,
            exclusion,
        } = self;
        [
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
            trace_complete.heap_bytes(),
            similarity_key.heap_bytes(),
            subsumption_key.heap_bytes(),
            callback_form.heap_bytes(),
            inert.heap_bytes(),
            exclusion.heap_bytes(),
        ]
        .into_iter()
        .fold(0, usize::saturating_add)
    }
}

impl HeapBytes for Hook {
    /// A hook retains only its effect key; location and phase fields remain accounted inline.
    fn heap_bytes(&self) -> usize {
        let Self {
            span,
            timing,
            label,
            scope,
            key,
        } = self;
        [
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
            unresolved,
            sites,
            files,
            suites,
            blind,
            retained,
        } = self;
        let Run { modules, exports } = run;
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
            hooks: retained.hooks.saturating_add(
                hooks
                    .capacity()
                    .saturating_mul(size_of::<(std::path::PathBuf, Hooks)>()),
            ),
            sites: retained.sites.saturating_add(
                sites
                    .capacity()
                    .saturating_mul(size_of::<(std::path::PathBuf, SiteRecord)>()),
            ),
            blind: retained
                .blind
                .saturating_add(blind.capacity().saturating_mul(size_of::<BlindFile>())),
            modules: modules.heap_bytes(),
            exports: exports.heap_bytes(),
            scalars: tests.heap_bytes().saturating_add(unresolved.heap_bytes()),
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
        crate::memory::MemoryReport::next_engine(
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
