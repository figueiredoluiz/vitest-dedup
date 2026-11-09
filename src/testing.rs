//! Shared test fixtures: an owned project written to disk and read through the layers, and the
//! one runner every property test goes through.
//!
//! Both harness families that read a fixture project — the normal forms' and the behavior
//! trace's — start from the same shape: files on disk, one parse, one inventory, one normalizer.
//! The setup lives here once so the two cannot drift.

use crate::framework::vitest::DIALECT;
use crate::inventory::{inventory, Inventory, Modules};
use crate::lang::{parse, Parsed};
use crate::normal::{Exports, Normalizer, Setting, Structure};
use std::fs;
use std::path::Path;

/// Runs `with` over a project holding `files`, with `main` written as its test file and read
/// through the inventory and a normalizer over it. `globals` decides whether a bare `test` call
/// registers.
pub(crate) fn project<T>(
    main: &str,
    files: &[(&str, &str)],
    globals: bool,
    with: impl for<'a> FnOnce(&'a Path, &'a Parsed, &'a Inventory, &'a mut Normalizer<'a>) -> T,
) -> T {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    let path = dir.path().join("a.test.tsx");
    fs::write(&path, main).unwrap();
    let parsed = parse(&path, main).unwrap();
    let found = inventory(
        &path,
        main,
        &parsed,
        crate::inventory::Setting {
            dialect: &DIALECT,
            globals,
            root: dir.path(),
        },
        &mut Modules::default(),
    );
    let structure = Structure::of(&parsed, &found);
    let exports = Exports::default();
    let setting = Setting {
        dialect: &DIALECT,
        root: dir.path(),
        exports: &exports,
    };
    let mut normalizer = Normalizer::new(&path, &parsed, setting, &structure);
    with(dir.path(), &parsed, &found, &mut normalizer)
}

/// The seed a property runs under unless `PROPTEST_RNG_SEED` names another, so CI and a local run
/// draw the same cases.
const PROPERTY_SEED: u64 = 0x5eed_7e57;

/// The seed a run uses: the one `PROPTEST_RNG_SEED` set, or [`PROPERTY_SEED`] when none was.
fn seed(configured: proptest::test_runner::RngSeed) -> proptest::test_runner::RngSeed {
    use proptest::test_runner::RngSeed;
    match configured {
        RngSeed::Random => RngSeed::Fixed(PROPERTY_SEED),
        named => named,
    }
}

/// Checks `check` on `cases` values drawn from `strategy`, shrinking any failure to a minimal
/// input, and fails unless every one of the `cases` ran and passed.
///
/// Failures are not persisted: a counterexample becomes an owned test or corpus case instead.
pub(crate) fn property<S>(
    cases: u32,
    strategy: S,
    check: impl Fn(S::Value) -> Result<(), proptest::test_runner::TestCaseError>,
) where
    S: proptest::strategy::Strategy,
    S::Value: std::fmt::Debug,
{
    use proptest::test_runner::{Config, TestRunner};
    use std::cell::Cell;
    assert!(cases > 0, "a property that runs no case measures nothing");
    let defaults = Config::default();
    let config = Config {
        cases,
        failure_persistence: None,
        rng_seed: seed(defaults.rng_seed),
        ..defaults
    };
    let passed = Cell::new(0_u32);
    let result = TestRunner::new(config).run(&strategy, |value| {
        check(value)?;
        passed.set(passed.get() + 1);
        Ok(())
    });
    finish(
        result.map_err(|error| error.to_string()),
        passed.get(),
        cases,
    );
}

/// Fails a property run that found a counterexample, or that ran fewer than `cases` cases. Kept
/// out of the generic runner so both outcomes are measured as one function.
fn finish(result: Result<(), String>, ran: u32, cases: u32) {
    if let Err(error) = result {
        panic!("{error}");
    }
    // Not reachable through `TestRunner`, which errors before passing fewer cases than asked; it
    // guards the configuration above.
    assert!(ran >= cases, "{ran} of {cases} cases ran");
}

#[cfg(test)]
mod tests {
    use super::{property, seed, PROPERTY_SEED};
    use proptest::test_runner::RngSeed;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    /// With no seed configured a run uses the fixed one; a seed `PROPTEST_RNG_SEED` named wins.
    #[test]
    fn a_named_seed_overrides_the_fixed_one() {
        assert_eq!(seed(RngSeed::Random), RngSeed::Fixed(PROPERTY_SEED));
        assert_eq!(seed(RngSeed::Fixed(7)), RngSeed::Fixed(7));
    }

    /// The message `run` panics with; a run that returns fails the calling test.
    fn panic_message(run: impl FnOnce()) -> String {
        let payload = catch_unwind(AssertUnwindSafe(run)).expect_err("the property passed");
        let text = payload.downcast_ref::<String>().map(String::as_str);
        text.or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or_default()
            .to_owned()
    }

    /// A failing property panics with its input shrunk to the smallest that still fails.
    #[test]
    fn a_failing_property_reports_its_shrunk_input() {
        let message = panic_message(|| {
            property(256, 0_u32..1_000, |value| {
                proptest::prop_assert!(value < 10);
                Ok(())
            })
        });
        assert!(message.contains("minimal failing input: 10"), "{message}");
    }

    /// A check every value passes.
    fn holds(_: u32) -> Result<(), proptest::test_runner::TestCaseError> {
        Ok(())
    }

    /// Asking for no cases is refused, since it would pass having measured nothing; the same
    /// property asked for one case passes.
    #[test]
    fn a_property_with_no_cases_is_refused() {
        let message = panic_message(|| property(0, 0_u32..1, holds));
        assert!(message.contains("runs no case"), "{message}");
        property(1, 0_u32..1, holds);
    }
}
