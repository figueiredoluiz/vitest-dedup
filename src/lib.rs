//! VitestDedup's reusable static-analysis library.
//!
//! The analyzer reads Vitest test files without importing or executing them. Parsing is confined
//! to [`frontend`] and, for the next core, [`lang`]; every other module consumes the owned
//! intermediate representation in [`model`] or the owned tree in [`lang`]. Knowledge of the test
//! framework is reached through [`framework`].

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
pub mod frontend;
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
pub use framework::vitest::config as vitest_config;
pub use framework::vitest::projects as vitest_projects;
