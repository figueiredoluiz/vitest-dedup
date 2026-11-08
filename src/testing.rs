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
    use proptest::test_runner::{Config, RngSeed, TestRunner};
    use std::cell::Cell;
    let mut config = Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    };
    if matches!(config.rng_seed, RngSeed::Random) {
        config.rng_seed = RngSeed::Fixed(PROPERTY_SEED);
    }
    let passed = Cell::new(0_u32);
    let result = TestRunner::new(config).run(&strategy, |value| {
        check(value)?;
        passed.set(passed.get() + 1);
        Ok(())
    });
    if let Err(error) = result {
        panic!("{error}");
    }
    assert!(
        passed.get() >= cases,
        "{} of {cases} cases ran",
        passed.get()
    );
}
