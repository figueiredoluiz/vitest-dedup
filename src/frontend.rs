//! The only module tree permitted to import `oxc_*`.
//!
//! Everything above this boundary consumes [`crate::model`]. Keeping the parser contained is what
//! makes the pre-1.0 oxc pin survivable: a breaking release is a rewrite of this subtree, not of
//! the analyzer.

mod extract;
mod fingerprint;
mod helpers;
mod registry;
mod resolver;

use crate::discovery::SourceFile;
use crate::model::TestCase;
use std::path::Path;

pub use registry::{take_cell_census, Globals};
pub use resolver::{take_resolution_gaps, ResolutionGaps, TsconfigGap};

/// Everything one parsed file contributed.
#[derive(Clone, Debug, Default)]
pub struct FileExtraction {
    /// Test cases in source order.
    pub tests: Vec<TestCase>,
    /// Exact source bytes for imported declaration locations, for snippets and suppressions.
    /// Only helpers that actually contributed declarations are retained, once per path.
    pub helper_sources: std::collections::BTreeMap<std::path::PathBuf, String>,
    /// Set when the file could not be fully understood, so absence of a finding proves nothing.
    pub incomplete: bool,
    /// Human-readable reasons the extraction was incomplete.
    pub diagnostics: Vec<String>,
    /// The closed reason code of each entry of `diagnostics`, index-parallel.
    pub reasons: Vec<crate::model::ExclusionReason>,
    /// Refusals of a kind `diagnostics` already carries one sentence for: the diagnostic cap keeps
    /// the list short, but each further distinct reason is kept here.
    pub further_refusals: Vec<(crate::model::ExclusionReason, String)>,
    /// Set when this file contributed zero tests, extraction found nothing wrong with it
    /// (`diagnostics` is empty), and every `test`/`describe` declaration this extractor did find
    /// was a statically known-empty `.each`/`.for` table (`test.each([])`, `describe.each([])`).
    /// Vitest itself runs such a table zero times -- reporting "No test suite found in file" for
    /// it, not a clean pass -- so zero tests here is a proven, correct answer, not a sign that
    /// extraction missed something. `main.rs`'s "discovered a file but extracted no tests" guard
    /// (which otherwise blames `test.globals`) uses this to say the right thing instead: never to
    /// suppress the guard itself, since a zero-test `globals: true` file this analyzer failed to
    /// recognize would look identical to a caller that only checked "tests is empty".
    pub proven_empty: bool,
}

/// Parses one source file and extracts its Vitest test cases.
///
/// Import resolution is not confined to any root; the CLI uses [`extract_within`].
///
/// Never panics on malformed input: a file that fails to parse yields an empty extraction with
/// `incomplete` set, matching the product rule that operational degradation stays visible.
pub fn extract(source: &str, file: &SourceFile, globals: Globals) -> FileExtraction {
    extract::extract(source, file, globals)
}

/// Like [`extract()`], but a tsconfig `paths`/`baseUrl` resolution that leaves `root` is refused:
/// the file it names is not opened, and the extraction records a diagnostic and is `incomplete`.
/// Relative imports and package resolution, which no tsconfig decides, are unaffected.
pub fn extract_within(
    source: &str,
    file: &SourceFile,
    globals: Globals,
    root: &Path,
) -> FileExtraction {
    extract::extract_within(source, file, globals, root)
}

/// Extracts a historical caller without reading present-day imported declarations.
/// Until helper blobs can be resolved in that same revision, called imports are refused and
/// their registration scopes are unresolved; the revision baseline is explicitly partial.
pub fn extract_from_revision(source: &str, file: &SourceFile, globals: Globals) -> FileExtraction {
    extract::extract_with_imports(source, file, globals, false)
}
