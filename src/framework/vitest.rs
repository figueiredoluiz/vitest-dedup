//! The Vitest adapter: its test API as a [`Dialect`], its default file globs, and the reading of
//! its configuration files ([`config`], [`projects`]).
//!
//! The names here are the ones `docs/vitest-surface.md` documents.

use super::{
    Chain, ChainMember, Dialect, FileDefaults, Framework, HookTiming, Namespace, Registers,
    Registration, TableRows,
};

pub mod config;
pub mod projects;
mod titles;

/// Vitest.
#[derive(Debug)]
pub struct Vitest;

impl Framework for Vitest {
    fn dialect(&self) -> &'static Dialect {
        &DIALECT
    }

    fn file_defaults(&self) -> &'static FileDefaults {
        &FILE_DEFAULTS
    }
}

// Table entries are built by macros rather than `const fn`s: a `const fn` called only in a static
// runs at compile time, so coverage would report its body as never executed.

/// A registration entry.
macro_rules! registration {
    ($name:literal, $canonical:literal, $registers:expr) => {
        Registration {
            name: $name,
            canonical: $canonical,
            registers: $registers,
        }
    };
}

/// A hook registration, whose canonical spelling is its own name.
macro_rules! hook {
    ($name:literal, $hook:expr) => {
        registration!($name, $name, Registers::Hook($hook))
    };
}

/// A chain member entry.
macro_rules! member {
    ($name:literal, $meaning:expr) => {
        ChainMember {
            name: $name,
            meaning: $meaning,
        }
    };
}

/// Vitest's test API (`docs/vitest-surface.md` sections 1, 2, 4, 5, 6, 7, 8 and 12).
pub static DIALECT: Dialect = Dialect {
    render_title: titles::render,
    api_modules: &["vitest"],
    in_source_member: Some("vitest"),
    registrations: &[
        registration!("test", "test", Registers::Test),
        registration!("it", "test", Registers::Test),
        registration!("describe", "describe", Registers::Suite),
        registration!("suite", "describe", Registers::Suite),
        registration!("bench", "bench", Registers::Bench),
        hook!("beforeAll", HookTiming::BeforeAll),
        hook!("beforeEach", HookTiming::BeforeEach),
        hook!("afterEach", HookTiming::AfterEach),
        hook!("afterAll", HookTiming::AfterAll),
        hook!("aroundEach", HookTiming::AroundEach),
        hook!("aroundAll", HookTiming::AroundAll),
        hook!("onTestFinished", HookTiming::OnTestFinished),
        hook!("onTestFailed", HookTiming::OnTestFailed),
    ],
    namespaces: &[
        Namespace {
            name: "vi",
            canonical: "vi",
        },
        Namespace {
            name: "vitest",
            canonical: "vi",
        },
        Namespace {
            name: "expect",
            canonical: "expect",
        },
    ],
    chain: &[
        member!("skip", Chain::Skip),
        member!("only", Chain::Focus),
        member!("todo", Chain::Todo),
        member!("fails", Chain::ExpectedFailure),
        member!("concurrent", Chain::Concurrent),
        // [v5 CHANGE] removed in v5, but pre-v5 code still contains it.
        member!("sequential", Chain::Sequential),
        member!("shuffle", Chain::Shuffle),
        member!("skipIf", Chain::SkipWhen),
        member!("runIf", Chain::RunWhen),
        member!("each", Chain::Table(TableRows::Spread)),
        member!("for", Chain::Table(TableRows::Whole)),
        member!("extend", Chain::FixtureExtension),
        member!("override", Chain::FixtureScoping),
        member!("scoped", Chain::FixtureScoping),
    ],
    scheduling_options: &["timeout", "retry", "repeats"],
    name_observing_matchers: &["Snapshot"],
};

/// Vitest's default file globs.
pub static FILE_DEFAULTS: FileDefaults = FileDefaults {
    include: DEFAULT_INCLUDE_GLOBS,
    typecheck_include: DEFAULT_TYPECHECK_INCLUDE_GLOBS,
    benchmark_include: DEFAULT_BENCHMARK_INCLUDE_GLOBS,
    benchmark_exclude: DEFAULT_BENCHMARK_EXCLUDE_GLOBS,
    exclude: DEFAULT_EXCLUDE_GLOBS,
};

/// Default include patterns, matching Vitest's own `include` defaults.
///
/// Vitest documents its default as the extglob pattern `**/*.{test,spec}.?(c|m)[jt]s?(x)`, but
/// `globset` -- the matcher used everywhere in this project, the same choice CukeDedup made --
/// does not support extglob optional-groups (`?(...)`). Rather than approximate that syntax, this
/// enumerates the concrete extensions the optional groups expand to in practice: `js`, `jsx`,
/// `ts`, `tsx`, and their `.mjs`/`.cjs`/`.mts`/`.cts` module-flavor forms. This intentionally
/// drops combinations the extglob technically allows but nobody writes, such as `.mjsx`/`.ctsx`.
pub const DEFAULT_INCLUDE_GLOBS: &[&str] = &[
    "**/*.test.js",
    "**/*.test.jsx",
    "**/*.test.ts",
    "**/*.test.tsx",
    "**/*.test.mjs",
    "**/*.test.mts",
    "**/*.test.cjs",
    "**/*.test.cts",
    "**/*.spec.js",
    "**/*.spec.jsx",
    "**/*.spec.ts",
    "**/*.spec.tsx",
    "**/*.spec.mjs",
    "**/*.spec.mts",
    "**/*.spec.cjs",
    "**/*.spec.cts",
];

/// Default type-test include patterns, matching Vitest's `typecheck.include` default.
///
/// Vitest documents `['**/*.{test,spec}-d.?(c|m)[jt]s?(x)']` and consults it only when
/// `typecheck.enabled` is on (default off), so this list gates nothing on its own. Enumerated
/// rather than extglobbed for the reason [`DEFAULT_INCLUDE_GLOBS`] gives: `globset` does not
/// implement Vitest's optional groups, and this drops only the combinations nobody writes
/// (`.test-d.mjsx` and friends).
pub const DEFAULT_TYPECHECK_INCLUDE_GLOBS: &[&str] = &[
    "**/*.test-d.js",
    "**/*.test-d.jsx",
    "**/*.test-d.ts",
    "**/*.test-d.tsx",
    "**/*.test-d.mjs",
    "**/*.test-d.mts",
    "**/*.test-d.cjs",
    "**/*.test-d.cts",
    "**/*.spec-d.js",
    "**/*.spec-d.jsx",
    "**/*.spec-d.ts",
    "**/*.spec-d.tsx",
    "**/*.spec-d.mjs",
    "**/*.spec-d.mts",
    "**/*.spec-d.cjs",
    "**/*.spec-d.cts",
];

/// Default benchmark include patterns, matching Vitest's `benchmark.include` default.
///
/// Vitest documents `**/*.{bench,benchmark}.?(c|m)[jt]s?(x)`. As with
/// [`DEFAULT_INCLUDE_GLOBS`], enumerate the practical extensions because `globset` does not
/// implement the optional extglob groups Vitest accepts.
pub const DEFAULT_BENCHMARK_INCLUDE_GLOBS: &[&str] = &[
    "**/*.bench.js",
    "**/*.bench.jsx",
    "**/*.bench.ts",
    "**/*.bench.tsx",
    "**/*.bench.mjs",
    "**/*.bench.mts",
    "**/*.bench.cjs",
    "**/*.bench.cts",
    "**/*.benchmark.js",
    "**/*.benchmark.jsx",
    "**/*.benchmark.ts",
    "**/*.benchmark.tsx",
    "**/*.benchmark.mjs",
    "**/*.benchmark.mts",
    "**/*.benchmark.cjs",
    "**/*.benchmark.cts",
];

/// Vitest's `benchmark.exclude` defaults, expressed as directory globs for `globset`.
pub const DEFAULT_BENCHMARK_EXCLUDE_GLOBS: &[&str] = &[
    "**/node_modules/**",
    "**/dist/**",
    "**/.idea/**",
    "**/.git/**",
    "**/.cache/**",
];

/// Default exclude patterns, applied in addition to `.gitignore` / `.vitest-dedupignore`.
pub const DEFAULT_EXCLUDE_GLOBS: &[&str] = &[
    "**/node_modules/**",
    "**/dist/**",
    "**/build/**",
    "**/coverage/**",
    "**/.idea/**",
    "**/.git/**",
    "**/.cache/**",
    "**/.output/**",
    "**/.temp/**",
];
