//! JavaScript and TypeScript as the analyzer reads them: an owned syntax tree, its scopes, module
//! resolution and positions.
//!
//! The only module tree that imports an `oxc_*` crate. The tree is the ESTree serialization of oxc's AST, with parent links
//! and node ids; everything above reads that published vocabulary through [`Tree`] and [`Scopes`]
//! and knows nothing of tests.

mod build;
mod completion;
pub(crate) mod js_text;
pub(crate) mod limits;
pub(crate) mod manifest_fs;
pub(crate) mod packages;
mod resolve;
mod scalar;
mod scopes;
mod sources;
mod syntax;
pub mod text;
mod tree;

pub use completion::completes_inertly;
pub(crate) use completion::primitive;
pub use resolve::{
    classify_tsconfig_gap, confinement_refusals, manifest_refusals, resolve, resolve_file,
    take_confinement_refusal, take_manifest_refusal, take_package_gaps, take_tsconfig_gaps,
    ResolutionGaps, TsconfigGap,
};
pub use scalar::Scalar;
pub use scopes::{Declaration, Imported, Name, Reference, Scopes, Symbol, SymbolId};
pub(crate) use sources::query;
pub use sources::{SourceKind, SourceTree, Sources};
pub use syntax::{bound_symbols, export_name, member_name, property_name, uses_receiver};
pub use tree::{Field, NodeId, Span, Tree};

use std::path::Path;

/// A parsed module.
#[derive(Debug)]
pub struct Parsed {
    /// The syntax tree.
    pub tree: Tree,
    /// Its bindings and references.
    pub scopes: Scopes,
    /// Errors the parser recovered from; the tree holds what parsed.
    pub parse_errors: usize,
}

/// Why a module has no tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The file extension is not a JavaScript or TypeScript source type.
    UnsupportedSource,
    /// The parser could not recover a program.
    Fatal,
    /// The source is past the size or nesting the parser is given.
    SourceLimit,
}

/// Parses `source`, read from `path`, whose extension decides the language.
pub fn parse(path: &Path, source: &str) -> Result<Parsed, Refusal> {
    // Parsing, serialization and tree building recurse with the source's nesting: see
    // [`limits::PARSE_STACK_BYTES`].
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("vitest-dedup-parse".to_owned())
            .stack_size(limits::PARSE_STACK_BYTES)
            .spawn_scoped(scope, || build::parse(path, source))
            .map_err(|_| Refusal::Fatal)?
            .join()
            .unwrap_or(Err(Refusal::Fatal))
    })
}

/// Line starts of a source, for turning byte offsets into positions.
#[derive(Debug)]
pub struct Lines {
    starts: Vec<u32>,
}

impl Lines {
    /// The line starts of `source`.
    pub fn new(source: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            source
                .bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(index, _)| index as u32 + 1),
        );
        Self { starts }
    }

    /// The 1-based line of a byte offset.
    pub fn line(&self, offset: u32) -> u32 {
        match self.starts.binary_search(&offset) {
            Ok(exact) => exact as u32 + 1,
            Err(next) => next as u32,
        }
    }

    /// The 1-based line and UTF-16 column of a byte offset into `source`, the unit Vitest reports.
    pub fn position(&self, source: &str, offset: u32) -> (u32, u32) {
        let line = self.line(offset);
        let start = self.starts[line as usize - 1] as usize;
        let column = source
            .get(start..offset as usize)
            .unwrap_or_default()
            .encode_utf16()
            .count();
        (line, column as u32 + 1)
    }
}

#[cfg(test)]
mod tests;
