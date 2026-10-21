//! VitestDedup's reusable static-analysis library.
//!
//! The analyzer reads Vitest test files without importing or executing them. Parsing is confined
//! to [`frontend`]; every other module consumes the owned intermediate representation in
//! [`model`].

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod analysis;
pub mod atomic_write;
pub mod cli;
pub mod config;
pub(crate) mod confine;
pub mod discovery;
pub mod exclusions;
pub mod frontend;
pub mod memory;
pub mod model;
pub mod modes;
pub mod reporters;
pub mod vitest_config;
pub mod vitest_projects;
