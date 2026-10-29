//! The test-framework seam.
//!
//! Everything specific to one test framework is reached through [`Framework`]: the names of its
//! test API, described as data by a [`Dialect`], and its default file globs. Code outside a
//! framework's adapter module reads framework-neutral concepts (suite, test, hook, chain member)
//! and never spells an API name; the boundary test in this module enforces that for the neutral
//! modules. Vitest, in [`vitest`], is the one adapter.

pub mod vitest;

/// What a registration call registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Registers {
    /// A test.
    Test,
    /// A suite, whose callback registers more tests, suites and hooks.
    Suite,
    /// A benchmark.
    Bench,
    /// A lifecycle hook.
    Hook(HookTiming),
}

/// A lifecycle hook, by when it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookTiming {
    /// Once before the tests of its scope.
    BeforeAll,
    /// Before each test of its scope.
    BeforeEach,
    /// After each test of its scope.
    AfterEach,
    /// Once after the tests of its scope.
    AfterAll,
    /// Around each test of its scope.
    AroundEach,
    /// Around all the tests of its scope.
    AroundAll,
    /// After the current test finishes, registered while it runs.
    OnTestFinished,
    /// After the current test fails, registered while it runs.
    OnTestFailed,
}

/// A name that registers a test, a suite, a benchmark or a hook.
#[derive(Debug)]
pub struct Registration {
    /// The name as written.
    pub name: &'static str,
    /// The alias-free spelling every alias of this registration shares.
    pub canonical: &'static str,
    /// What it registers.
    pub registers: Registers,
}

/// What a member chained onto a registration (`test.skip`, `describe.each(rows)`) means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chain {
    /// Never runs.
    Skip,
    /// Runs while unfocused siblings are skipped.
    Focus,
    /// A placeholder that never runs.
    Todo,
    /// Passes when its body fails.
    ExpectedFailure,
    /// May run alongside its siblings.
    Concurrent,
    /// Runs in order even when its scope is concurrent.
    Sequential,
    /// Runs its children in random order.
    Shuffle,
    /// Skipped when its argument is truthy.
    SkipWhen,
    /// Runs only when its argument is truthy.
    RunWhen,
    /// Registers one copy per table row.
    Table(TableRows),
    /// Builds a new registration function with fixtures.
    FixtureExtension,
    /// Changes fixture values for the scope it is called in.
    FixtureScoping,
}

/// How a table row reaches the registered callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableRows {
    /// An array row is spread into the callback's arguments.
    Spread,
    /// The row is passed whole as the first argument.
    Whole,
}

/// A member that can be chained onto a registration.
#[derive(Debug)]
pub struct ChainMember {
    /// The member name.
    pub name: &'static str,
    /// What it means.
    pub meaning: Chain,
}

/// A name the framework's API exposes besides registrations, such as a utility namespace.
#[derive(Debug)]
pub struct Namespace {
    /// The name as written.
    pub name: &'static str,
    /// The alias-free spelling every alias shares.
    pub canonical: &'static str,
}

/// One framework's test API, described as data.
#[derive(Debug)]
pub struct Dialect {
    /// Modules whose imports are the API.
    pub api_modules: &'static [&'static str],
    /// The `import.meta` member that carries the API in source files, when the framework has one.
    pub in_source_member: Option<&'static str>,
    /// Names that register tests, suites, benchmarks and hooks.
    pub registrations: &'static [Registration],
    /// Other API names: utility namespaces and the assertion entry point.
    pub namespaces: &'static [Namespace],
    /// Members that can be chained onto a registration.
    pub chain: &'static [ChainMember],
    /// Keys of a registration's options object that change only how a test is scheduled, never
    /// what it checks (a timeout, retries).
    pub scheduling_options: &'static [&'static str],
    /// Text a matcher's name contains when its assertion records values in a form that prints
    /// constructor and function names (snapshots).
    pub name_observing_matchers: &'static [&'static str],
}

impl Dialect {
    /// The registration `name` stands for, if any.
    pub fn registration(&self, name: &str) -> Option<&'static Registration> {
        let registrations: &'static [Registration] = self.registrations;
        registrations
            .iter()
            .find(|registration| registration.name == name)
    }

    /// The hook `name` registers, if it registers one.
    pub fn hook(&self, name: &str) -> Option<HookTiming> {
        match self.registration(name)?.registers {
            Registers::Hook(hook) => Some(hook),
            _ => None,
        }
    }

    /// What chaining `name` means, if it is a chain member.
    pub fn chain_member(&self, name: &str) -> Option<Chain> {
        self.chain
            .iter()
            .find(|member| member.name == name)
            .map(|member| member.meaning)
    }

    /// The canonical spelling of the non-registration API name `name`, if it is one.
    pub fn namespace(&self, name: &str) -> Option<&'static str> {
        self.namespaces
            .iter()
            .find(|namespace| namespace.name == name)
            .map(|namespace| namespace.canonical)
    }

    /// Whether `module` is one of the modules the API is imported from.
    pub fn is_api_module(&self, module: &str) -> bool {
        self.api_modules.contains(&module)
    }

    /// Every name this dialect declares.
    pub fn names(&self) -> impl Iterator<Item = &'static str> {
        let registrations: &'static [Registration] = self.registrations;
        let namespaces: &'static [Namespace] = self.namespaces;
        let chain: &'static [ChainMember] = self.chain;
        self.api_modules
            .iter()
            .copied()
            .chain(self.in_source_member)
            .chain(registrations.iter().map(|registration| registration.name))
            .chain(namespaces.iter().map(|namespace| namespace.name))
            .chain(chain.iter().map(|member| member.name))
            .chain(self.scheduling_options.iter().copied())
            .chain(self.name_observing_matchers.iter().copied())
    }
}

/// The file globs a framework selects test files with when its configuration does not say.
#[derive(Debug)]
pub struct FileDefaults {
    /// Test files.
    pub include: &'static [&'static str],
    /// Type-test files, consulted only when type checking is enabled.
    pub typecheck_include: &'static [&'static str],
    /// Benchmark files.
    pub benchmark_include: &'static [&'static str],
    /// Paths excluded from benchmark files.
    pub benchmark_exclude: &'static [&'static str],
    /// Paths excluded from test files.
    pub exclude: &'static [&'static str],
}

/// A test framework, as the analyzer needs to know it.
pub trait Framework: Sync {
    /// Its test API.
    fn dialect(&self) -> &'static Dialect;
    /// Its default file globs.
    fn file_defaults(&self) -> &'static FileDefaults;
}

/// The framework the analyzer reads.
pub fn current() -> &'static dyn Framework {
    &vitest::Vitest
}

#[cfg(test)]
mod tests;
