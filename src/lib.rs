//! VitestDedup's reusable static-analysis library.
//!
//! The analyzer reads Vitest test files without importing or executing them. Parsing is confined
//! to [`lang`]; every other module consumes the owned tree in [`lang`] or the records built over
//! it. Knowledge of the test framework is reached through [`framework`].

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod analysis;
pub mod atomic_write;
pub mod behavior;
pub mod cli;
pub mod config;
pub(crate) mod confine;
pub mod corpus;
pub mod discovery;
pub mod exclusions;
pub mod framework;
pub mod inventory;
pub mod lang;
pub mod memory;
pub mod model;
pub mod modes;
pub mod normal;
pub(crate) mod package_json;
pub mod pairs;
pub mod prerequisites;
pub mod reporters;
pub mod rules;
#[cfg(test)]
pub(crate) mod testing;
pub use framework::vitest::config as vitest_config;
pub use framework::vitest::projects as vitest_projects;
