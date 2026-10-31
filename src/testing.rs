//! Shared test fixtures: an owned project written to disk and read through the layers.
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
