//! VitestDedup's reusable static-analysis library.
//!
//! The analyzer reads Vitest test files without importing or executing them. Parsing is confined
//! to [`frontend`]; every other module consumes the owned intermediate representation in
//! [`model`]. Knowledge of the test framework is reached through [`framework`].

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod analysis;
pub mod atomic_write;
pub mod cli;
pub mod config;
pub(crate) mod confine;
pub mod discovery;
pub mod exclusions;
pub mod framework;
pub mod frontend;
pub mod memory;
pub mod model;
pub mod modes;
pub(crate) mod package_json;
pub mod prerequisites;
pub mod reporters;
pub use framework::vitest::config as vitest_config;
pub use framework::vitest::projects as vitest_projects;
