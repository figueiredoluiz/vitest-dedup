//! Static detection of Vitest's `test.globals` setting.
//!
//! Without knowing whether `globals: true` is in effect, a
//! project that relies on it has its `test`/`describe`/`it` calls silently invisible to
//! `crate::frontend` (they are ordinary, unbound free identifiers), so the analyzer extracts
//! **zero tests**. The zero-work guard rejects that run as an operational error, but it still
//! produces no useful analysis. This module answers the `globals` question from the project's own
//! configuration instead of requiring `--globals` on every invocation.
//!
//! # Never executes the config
//!
//! This is a static analyzer; its central guarantee is that it never runs project code. Vitest
//! and Vite config files are arbitrary JavaScript/TypeScript -- the only sound way to know their
//! effective value is to run them through Vite's config loader, which this crate deliberately does
//! not do. Instead, [`detect_globals`] performs a conservative textual scan for the literal shape
//! `test: { ... globals: true ... }` and refuses to guess whenever the source does not make the
//! answer obvious (see "Why `Unknown` so often" below).
//!
//! # Precedence
//!
//! Specified by `docs/vitest-surface.md`'s "How `describe`/`test`/etc. become available" and
//! "Projects / workspace" sections.
//!
//! At the analysis root:
//!
//! 1. `vitest.workspace.{ts,mts,cts,js,mjs,cjs}` -- if present, `globals` must be resolved per
//!    workspace member (entries can set `extends: false` and diverge), which this scan does not
//!    attempt. Its mere presence is reported as [`GlobalsDetection::Unknown`].
//! 2. `vitest.config.{ts,mts,cts,js,mjs,cjs}`, tried in that extension order.
//! 3. `vite.config.{ts,mts,cts,js,mjs,cjs}`, tried in that extension order, only if no
//!    `vitest.config.*` was found -- Vitest prefers its own dedicated config file.
//! 4. Otherwise, no configuration could be located at all.
//!
//! A `test.projects` array inside whichever config file is chosen is the same per-project problem
//! as a workspace file, and is detected the same way: reported as `Unknown` rather than answered
//! for the whole repository. See "Known limitation" below.
//!
//! # Why `Unknown` so often
//!
//! A wrong [`GlobalsDetection::Enabled`] invents test discoveries in a Playwright/Jest/Mocha file
//! that merely happens to share Vitest's ambient-global names. That was verified rather than
//! assumed: Cypress, Mocha and Jasmine all provide ambient `describe`/`it` with no import to
//! anchor to, so under a repository-wide `globals: true` their files were extracted as Vitest
//! tests regardless of which runner actually owned them.
//! A wrong [`GlobalsDetection::Disabled`] silently analyzes
//! nothing in a real `globals: true` project -- the exact failure this module exists to prevent.
//! Both are worse than admitting uncertainty, so every ambiguous shape -- a non-literal value, an
//! unbalanced or unterminated source, a `test` field that is not an object literal -- resolves to
//! `Unknown` rather than a best-effort guess.
//!
//! # Avoiding false positives in the textual scan
//!
//! `globals` can appear in a comment or inside an unrelated string without meaning anything. The
//! scan never matches raw text: it first walks the source once, replacing every character that
//! lies inside a line comment, a block comment, a `'`/`"` string, or the literal-text portions of
//! a template literal with a space, while leaving the code portions -- including the `${ ... }`
//! holes inside a template literal, recursively -- untouched. Property lookups then run only
//! against that masked text, so `// globals: true` and `"globals: true"` are invisible to them:
//! there is no code there to find. An unterminated string, template literal, or block comment
//! makes the masking pass itself fail closed (`Unknown`) rather than silently misreading the rest
//! of the file.
//!
//! # Anchoring to the exported config object
//!
//! A property lookup is never a flat text search over the whole file: `test` is looked up only as
//! a *direct* property of the object literal that `export default` resolves to (unwrapping at
//! most one function call, e.g. `defineConfig({ ... })`), and `globals`/`projects` are looked up
//! only as direct properties of that `test` object. A `{ test: { globals: true } }` shape sitting
//! inside a plugin's own options, or a `globals` key nested one level deeper under some other
//! `test` sibling (e.g. `environmentOptions`), is at depth 1 or more relative to the anchor and is
//! never matched -- only the config's or `test`'s own direct properties are. When no `export
//! default` can be found, or its value is not an object literal nor a single recognized wrapper
//! call around one, resolution fails closed to `Unknown` rather than falling back to a wider,
//! unanchored search.
//!
//! # Known limitation: `test.projects` / workspaces
//!
//! Vitest resolves `globals` per project, and `test.projects` entries can set `extends: false` to
//! ignore the root config entirely (`docs/vitest-surface.md`, "Projects / workspace"). Fully
//! resolving that would mean
//! locating, reading, and interpreting every project's own config -- out of scope here. Detecting
//! the shape and saying so is in scope: a discovered `vitest.workspace.*` file, or a `projects` key
//! inside the resolved `test` object, both yield `Unknown` with a diagnostic explaining why, rather
//! than a single repo-wide answer that would be wrong for at least one project.
//!
//! # Why this module has no dependency on the rest of the crate
//!
//! `src/frontend/` is the only module tree permitted to import an `oxc_*` crate
//! (`AGENTS.md`, "Parser containment"); this module does not need one, since it never builds an
//! AST -- a disciplined textual scan is sufficient for the narrow question it answers, and staying
//! off `oxc_*` avoids even raising the containment question. It also does not import
//! `crate::config` or `crate::model`: at the time this module was written it was not yet wired
//! into `src/lib.rs`, so keeping it self-contained (`std` plus `tempfile` in tests) let it be
//! verified in an isolated scratch crate. [`MAX_CONFIG_SOURCE_BYTES`] intentionally duplicates the
//! value of `crate::config::MAX_CONFIG_FILE_BYTES` rather than importing it; the two are expected
//! to be kept in sync by inspection since both encode the same "config is untrusted input" policy.

use std::fs::File;
use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Byte limit on a Vitest/Vite configuration file this scan will read.
///
/// Matches [`crate::config::MAX_CONFIG_FILE_BYTES`]'s value and rationale: configuration is
/// untrusted input in the same way source files are, so a huge or hostile file should fail with a
/// clear message rather than let this scan allocate against it unbounded.
pub const MAX_CONFIG_SOURCE_BYTES: u64 = 1024 * 1024;

/// Candidate config-file extensions, tried in this order.
///
/// Matches the extension family used everywhere else Vitest looks for a config file
/// (`docs/vitest-surface.md`, "How `describe`/`test`/etc. become available" and "Projects /
/// workspace"): `.ts`/`.js` plus their forced-module-system
/// `.mts`/`.cts`/`.mjs`/`.cjs` siblings. `vitest.config.json` is documented as not part of the
/// root-config lookup, so no `json` entry is included.
pub(crate) const CONFIG_EXTENSIONS: [&str; 6] = ["ts", "mts", "cts", "js", "mjs", "cjs"];

/// Outcome of statically resolving whether a Vitest project's effective configuration sets
/// `test.globals` to `true`.
///
/// Three outcomes, not two, because `Unknown` is not the same claim as `Disabled`: this analyzer
/// never executes the project's configuration, so "a config was found and it says false" and "no
/// usable answer could be produced" must stay distinguishable. Collapsing the second into the
/// first would recreate the zero-tests failure this module exists to prevent: a caller that treats
/// `Unknown` as `Disabled` will extract zero tests from a `globals: true` project and make the run
/// fail operationally instead of analyzing it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GlobalsDetection {
    /// A configuration file was found and statically shown to set `test.globals` to `true`.
    Enabled {
        /// The configuration file the value was read from.
        config: PathBuf,
    },
    /// A configuration file was found and does not set `test.globals` to `true`: either the
    /// `globals` key is absent from the `test` object (Vitest's own documented default is
    /// `false`), or it is present and its value is the literal `false`.
    Disabled {
        /// The configuration file inspected.
        config: PathBuf,
    },
    /// Could not be determined. Returned instead of a guess whenever guessing could be wrong in
    /// either direction -- see the module-level "Why `Unknown` so often" section for the full
    /// list of cases this covers (no config found, unreadable or oversized, a non-literal
    /// `globals` value, `test.projects` / a workspace file, or a source shape the scan cannot
    /// safely parse).
    Unknown {
        /// Human-readable explanation of why resolution stopped, so a caller can surface a
        /// specific warning instead of a bare "unknown".
        reason: String,
    },
}

/// Detects whether the Vitest project rooted at `root` resolves `test.globals` to `true`.
///
/// Reads at most one configuration file (see the module-level precedence order) and never
/// executes it. See the module documentation for the full precedence order, the false-positive
/// avoidance strategy, and the `test.projects` / workspace limitation.
pub fn detect_globals(root: &Path) -> GlobalsDetection {
    detect_globals_with_projects(root, None)
}

/// [`detect_globals`] for a caller that has already resolved the project set.
///
/// `resolve_projects` walks globs under a budget; the orchestrator resolves it before asking
/// about `globals`, so re-resolving here would double that walk on every run for an answer it
/// already holds. Passing `None` resolves on demand, which is what the unit tests and any other
/// caller want.
pub fn detect_globals_with_projects(
    root: &Path,
    projects: Option<&crate::vitest_projects::ProjectSet>,
) -> GlobalsDetection {
    if let Some(workspace) = find_workspace_file(root) {
        return GlobalsDetection::Unknown {
            reason: format!(
                "found {}: Vitest resolves `globals` per workspace member, and members can set \
                 `extends: false` to diverge from the root config, so a single repo-wide answer \
                 would be wrong for at least one of them",
                display_path(&workspace, root)
            ),
        };
    }

    let Some(config_path) = find_project_config(root) else {
        return GlobalsDetection::Unknown {
            reason: format!(
                "no vitest.config.* or vite.config.* found at {}",
                root.display()
            ),
        };
    };

    let source = match read_bounded(&config_path) {
        Ok(source) => source,
        Err(reason) => return GlobalsDetection::Unknown { reason },
    };

    let Some(masked) = mask_non_code(&source) else {
        return GlobalsDetection::Unknown {
            reason: format!(
                "{} contains an unterminated string, template literal, or comment; the scan \
                 cannot safely tell code from text past that point",
                display_path(&config_path, root)
            ),
        };
    };

    let config_object = match locate_config_object(&masked) {
        ConfigObject::NoExport => {
            return GlobalsDetection::Unknown {
                reason: format!(
                    "{} has no `export default` assignment this scan recognizes, so there is no \
                     exported configuration object to anchor the `test` lookup to",
                    display_path(&config_path, root)
                ),
            }
        }
        ConfigObject::NonLiteral => {
            return GlobalsDetection::Unknown {
                reason: format!(
                    "{}'s default export is not an object literal, nor a single function call \
                     wrapping one (e.g. `defineConfig({{ ... }})`); its shape cannot be read \
                     without executing it",
                    display_path(&config_path, root)
                ),
            }
        }
        ConfigObject::Malformed => {
            return GlobalsDetection::Unknown {
                reason: format!(
                    "{}'s default export starts an object literal that never reaches a matching \
                     closing brace in this scan; treating it as malformed rather than guessing at \
                     its contents",
                    display_path(&config_path, root)
                ),
            }
        }
        ConfigObject::Found(open, close) => (open, close),
    };

    match locate_test_object(&masked, config_object.0 + 1..config_object.1) {
        TestObject::Absent => GlobalsDetection::Disabled {
            config: config_path,
        },
        TestObject::NonLiteral => GlobalsDetection::Unknown {
            reason: format!(
                "{} sets `test` to something other than an object literal (e.g. a variable, a \
                 spread, or a function call); its contents cannot be read without executing it",
                display_path(&config_path, root)
            ),
        },
        TestObject::Malformed => GlobalsDetection::Unknown {
            reason: format!(
                "{} has a `test: {{` that never reaches a matching closing brace in this scan; \
                 treating it as malformed rather than guessing at its contents",
                display_path(&config_path, root)
            ),
        },
        TestObject::Found(open, close) => {
            let body = open + 1..close;
            if property_present(&masked, body.clone(), "projects") {
                // Not an automatic `Unknown` any more. Bailing on the mere *presence* of
                // `test.projects` was 23 of the 29 globals-detection failures on the 2026-09-27
                // 117-repository scan (S3), and globals detection failing costs 11.9% of all
                // tests on that corpus (S4) -- the largest repository alone loses 63% of its tests, because a
                // `globals: true` project analyzed as `globals: false` extracts nothing from an
                // ambient `describe`.
                //
                // The original reasoning was sound and is preserved below: entries resolve their
                // own configs and can `extends: false`, so a repo-wide answer *can* be wrong for
                // at least one project. What was wrong was treating "can be wrong" as "is
                // unknowable". When every resolved project agrees, the repo-wide answer is that
                // agreement, and nothing is being guessed.
                return projects_globals(root, &config_path, projects);
            }
            match boolean_property(&masked, body, "globals") {
                None => GlobalsDetection::Disabled {
                    config: config_path,
                },
                Some(None) => GlobalsDetection::Unknown {
                    reason: format!(
                        "{} sets `test.globals` to a non-literal expression (not the bare word \
                         `true` or `false`); its runtime value cannot be read without executing it",
                        display_path(&config_path, root)
                    ),
                },
                Some(Some(true)) => GlobalsDetection::Enabled {
                    config: config_path,
                },
                Some(Some(false)) => GlobalsDetection::Disabled {
                    config: config_path,
                },
            }
        }
    }
}

/// The repo-wide `test.globals` answer for a config that declares `test.projects`, or `Unknown`
/// when no single answer is defensible.
///
/// Unanimity is the whole rule. Vitest resolves `globals` per project, so a repo-wide answer is
/// only ever a *summary* of the per-project answers -- legitimate exactly when they all say the
/// same thing, and never otherwise. Four ways that summary is refused, each of them a case where
/// answering would mean guessing:
///
/// - an entry this scan could not resolve into a project at all (`has_unresolved_entries`), or a
///   set cut short by the project/glob budget (`truncated`): the projects we cannot see could
///   disagree with the ones we can;
/// - no project resolved, though entries were declared -- a glob matching nothing, a referenced
///   config that is not there. The root's own `test.globals` is NOT the answer here: the entries
///   were meant to govern, and that they resolved to nothing is a fact about this scan, not
///   permission to fall back;
/// - any project whose own `globals` is `Unknown`, which is that project's way of saying the same
///   thing one level down;
/// - projects that disagree, which is the case the original bail was written for and the one this
///   function still refuses.
///
/// `extends: false` needs no special handling here: `resolve_projects` has already applied it
/// when computing each project's effective `globals`, so a project that opted out of the root
/// config arrives with its own answer and is folded like any other.
fn projects_globals(
    root: &Path,
    config_path: &Path,
    projects: Option<&crate::vitest_projects::ProjectSet>,
) -> GlobalsDetection {
    let resolved;
    let set = match projects {
        Some(set) => set,
        None => {
            resolved = crate::vitest_projects::resolve_projects(root);
            &resolved
        }
    };

    let refuse = |why: &str| GlobalsDetection::Unknown {
        reason: format!(
            "{} declares `test.projects` and {why}: each entry resolves its own config and can \
             set `extends: false`, so a single repo-wide answer would be wrong for at least one \
             project",
            display_path(config_path, root)
        ),
    };

    if set.has_unresolved_entries {
        return refuse("at least one entry could not be resolved into a project");
    }
    if set.truncated {
        return refuse("the project map was truncated before every entry was read");
    }
    if set.projects.is_empty() {
        return refuse("no entry resolved to a project");
    }

    let mut answer: Option<bool> = None;
    for project in &set.projects {
        let enabled = match &project.globals {
            GlobalsDetection::Enabled { .. } => true,
            GlobalsDetection::Disabled { .. } => false,
            // `Unknown`, and any future variant: `GlobalsDetection` is `#[non_exhaustive]`, so an
            // unrecognized answer must not be folded as agreement with anything.
            _ => return refuse("at least one project's own `globals` could not be determined"),
        };
        match answer {
            None => answer = Some(enabled),
            Some(previous) if previous == enabled => {}
            Some(_) => return refuse("its projects disagree about `test.globals`"),
        }
    }

    match answer {
        Some(true) => GlobalsDetection::Enabled {
            config: config_path.to_path_buf(),
        },
        Some(false) => GlobalsDetection::Disabled {
            config: config_path.to_path_buf(),
        },
        // Unreachable: the emptiness check above already returned. Refusing rather than
        // `unreachable!()` keeps a future reshaping of that check from turning a logic slip into
        // a panic in a binary whose whole contract is to degrade honestly.
        None => refuse("no entry resolved to a project"),
    }
}

/// Renders `path` relative to `root` for diagnostics, falling back to the full path when `path`
/// does not live under `root` (it always does today, since both come from the same lookup, but a
/// fallback is cheaper than a second way for this function to fail).
pub(crate) fn display_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Looks for a legacy `vitest.workspace.*` file at `root`.
///
/// Only `.ts` and `.js` are confirmed by `docs/vitest-surface.md`'s "Projects / workspace"; the
/// other four extensions
/// are checked anyway on the principle that missing a real workspace file would silently produce
/// a wrong `Enabled`/`Disabled` answer, whereas a false-positive match here only ever produces the
/// always-safe `Unknown`.
pub(crate) fn find_workspace_file(root: &Path) -> Option<PathBuf> {
    CONFIG_EXTENSIONS.iter().find_map(|extension| {
        let candidate = root.join(format!("vitest.workspace.{extension}"));
        candidate.is_file().then_some(candidate)
    })
}

/// The normalized text of a config file's own `test` object, for use as a scope identity.
///
/// S6 mechanism 2. A `vitest.config.*` sitting beside a test directory governs how those tests
/// run -- a docs framework ships one per `__tests__/<scenario>/`, an SSR component library sits
/// at `environment: 'node'`, a bundler keeps five specs under a playground tree -- and the scan
/// read none of them. Whose repositories those are is recorded in the local provenance ledger. Two byte-identical files under `environment: 'node'` and
/// `environment: 'happy-dom'` therefore shared one context and `cross-file-duplicate` linked
/// them.
///
/// CONTENT, not path. Keying on the config's location would separate two directories whose
/// configs are identical, which is a recall loss for no gain; keying on its `test` object means
/// identical configs still compare equal and only genuinely different ones separate.
///
/// The text is used RAW -- deliberately, after three review rounds each found a different lexical
/// class that a whitespace normalizer here got wrong:
///
/// - `split_whitespace()` collapsed spaces inside string literals, merging
///   `include: ['a  b.test.ts']` with `include: ['a b.test.ts']`.
/// - A literal-aware scanner still collapsed spaces inside REGEX literals, merging
///   `testNamePattern: /a  b/` with `/a b/`. Both are significant to Vitest.
/// - That scanner also inserted a space only where the input already had one, so
///   `{environment:'node'}` and `{ environment: 'node' }` stayed separate anyway -- it never
///   delivered the formatting-independence that justified it.
///
/// Normalizing formatting without changing meaning requires a JavaScript lexer. `AGENTS.md`
/// confines every `oxc_*` crate to `src/frontend/`, so this module cannot have one, and a
/// hand-rolled approximation is unsound in the direction that COSTS: collapsing something
/// significant merges two differently-configured directories into one `cross-file-duplicate`
/// group, which is a false positive. Using the text raw is unsound only in the safe direction --
/// two configs that differ in formatting alone over-separate, suppressing a finding.
///
/// So the recorded limit is: reformatting a config changes its scope. Under a repo-wide formatter
/// that costs nothing, and a copy-pasted scenario config is byte-identical in practice.
///
/// `None` means exactly one thing: the config was READ and has no `test` object, so it says
/// nothing about how tests run. Every other outcome is `Err` -- an unreadable file, an
/// unterminated string, no recognized `export default` (a CommonJS `module.exports`, for
/// instance), or a `test` value that is not an object literal. The caller must treat `Err` as
/// unresolved rather than as "no config": a config that exists and cannot be read is exactly
/// where guessing is wrong, and the first version of this function returned `Ok(None)` for
/// `NoExport`, which silently ignored every CommonJS config's environment.
pub(crate) fn config_test_scope(path: &Path) -> Result<Option<String>, String> {
    let source = read_bounded(path)?;
    let Some(masked) = mask_non_code(&source) else {
        return Err(format!(
            "{} contains an unterminated string, template literal, or comment",
            path.display()
        ));
    };
    let config_object = match locate_config_object(&masked) {
        ConfigObject::Found(open, close) => (open, close),
        // `NoExport` is UNREADABLE, not "no scope". Returning `Ok(None)` made
        // `nearest_config_scope` keep walking, so the file compared as if the analysis root
        // governed it -- and a CommonJS config (`module.exports = { test: { environment: … } }`,
        // with `.cjs` in `CONFIG_EXTENSIONS`) had its environment silently ignored, bringing back
        // the very false positive this scope exists to remove. Reproduced before the fix.
        //
        // It is also what `detect_globals_with_projects` in this file already does with
        // `NoExport`: report it as unknown rather than as an answer.
        ConfigObject::NoExport => {
            return Err(format!(
                "{} has no `export default` this scan recognizes",
                path.display()
            ))
        }
        _ => {
            return Err(format!(
                "{}'s default export cannot be read",
                path.display()
            ))
        }
    };
    match locate_test_object(&masked, config_object.0 + 1..config_object.1) {
        TestObject::Absent => Ok(None),
        TestObject::Found(open, close) => {
            let original: Vec<char> = source.chars().collect();
            let text: String = original[open..=close.min(original.len() - 1)]
                .iter()
                .collect();
            Ok(Some(text))
        }
        _ => Err(format!("{}'s `test` value cannot be read", path.display())),
    }
}

/// Find the first supported root-level Vitest config, falling back to a Vite config in
/// extension order.
pub(crate) fn find_project_config(root: &Path) -> Option<PathBuf> {
    CONFIG_EXTENSIONS
        .iter()
        .find_map(|extension| {
            let candidate = root.join(format!("vitest.config.{extension}"));
            candidate.is_file().then_some(candidate)
        })
        .or_else(|| {
            CONFIG_EXTENSIONS.iter().find_map(|extension| {
                let candidate = root.join(format!("vite.config.{extension}"));
                candidate.is_file().then_some(candidate)
            })
        })
}

/// Reads `path` with the same bounded-read discipline as `crate::config::read_raw_config`: the
/// size check races a `take()` limit rather than trusting a `stat()` taken before the read.
pub(crate) fn read_bounded(path: &Path) -> Result<String, String> {
    let file =
        File::open(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
    if metadata.len() > MAX_CONFIG_SOURCE_BYTES {
        return Err(format!(
            "{} is {} bytes, exceeding the {}-byte limit this scan enforces",
            path.display(),
            metadata.len(),
            MAX_CONFIG_SOURCE_BYTES
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_CONFIG_SOURCE_BYTES) as usize);
    file.take(MAX_CONFIG_SOURCE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_CONFIG_SOURCE_BYTES {
        return Err(format!(
            "{} exceeds the {}-byte limit this scan enforces",
            path.display(),
            MAX_CONFIG_SOURCE_BYTES
        ));
    }
    String::from_utf8(bytes).map_err(|_| format!("{} is not valid UTF-8", path.display()))
}

/// One frame of the masking state machine's context stack.
enum MaskFrame {
    /// Inside a template literal's literal-text portions (between its backticks and any `${`
    /// holes). Text here is masked; a `` ` `` ends it, a `${` opens a [`MaskFrame::Hole`] on top
    /// of it without popping it.
    Template,
    /// Inside a template literal's `${ ... }` hole, which is ordinary code. Carries the brace
    /// depth still open within this hole, starting at `0` right after the `${` is seen -- the
    /// ordinary `'{' => ...` arm brings it to `1` when it processes that same opening brace on
    /// the next iteration, so a nested object literal inside the hole is not mistaken for the
    /// hole's end.
    Hole(u32),
}

/// Replaces every character that lies inside a line comment, a block comment, or a `'`/`"` string
/// with a space, and every character in a template literal's literal-text portions with a space,
/// while leaving code -- including the contents of `${ ... }` holes, recursively -- untouched.
///
/// Returns `None` when the source contains an unterminated string, template literal, or block
/// comment: past that point the scan can no longer tell code from text, so it fails closed rather
/// than guessing at the rest of the file.
///
/// The returned vector has exactly as many characters as `source`; only content is replaced, so
/// every later scan can keep working in the same index space as the original.
pub(crate) fn mask_non_code(source: &str) -> Option<Vec<char>> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = chars.clone();
    let n = chars.len();
    let mut frames: Vec<MaskFrame> = Vec::new();
    let mut i = 0usize;

    while i < n {
        let in_template_text = matches!(frames.last(), Some(MaskFrame::Template));

        if !in_template_text {
            // Top-level code, or inside a `${ ... }` hole -- both are ordinary JavaScript/
            // TypeScript source and are scanned identically.
            match chars[i] {
                '/' if i + 1 < n && chars[i + 1] == '/' => {
                    out[i] = ' ';
                    out[i + 1] = ' ';
                    i += 2;
                    while i < n && chars[i] != '\n' {
                        out[i] = ' ';
                        i += 1;
                    }
                }
                '/' if i + 1 < n && chars[i + 1] == '*' => {
                    out[i] = ' ';
                    out[i + 1] = ' ';
                    i += 2;
                    let mut closed = false;
                    while i < n {
                        if chars[i] == '*' && i + 1 < n && chars[i + 1] == '/' {
                            out[i] = ' ';
                            out[i + 1] = ' ';
                            i += 2;
                            closed = true;
                            break;
                        }
                        out[i] = ' ';
                        i += 1;
                    }
                    if !closed {
                        return None;
                    }
                }
                // A `/` here is neither `//` nor `/*` (both matched above), so it is either a
                // division or the start of a REGULAR EXPRESSION literal -- and this function does
                // not mask regex literals. That is mostly harmless: a stray `.` or `$` in the
                // output changes nothing. A brace or bracket is not harmless. `/[}]/` is a valid
                // `testNamePattern`, and its unmasked `}` ends the enclosing object as far as
                // every brace-matching caller is concerned, so
                //
                //     export default { test: { testNamePattern: /[}]/, environment: 'node' } };
                //     export default { test: { testNamePattern: /[}]/, environment: 'happy-dom' } };
                //
                // both yielded the scope `{ testNamePattern: /[}` -- one scope for two different
                // environments, which is exactly the false positive S6 exists to remove -- and a
                // config whose `globals: true` fell outside the truncated object was reported
                // `Disabled` rather than `Enabled`. Both measured before this arm existed.
                //
                // Telling regex from division needs the previous significant token, i.e. a real
                // JavaScript lexer, which `AGENTS.md` keeps in `src/frontend/`. So this DETECTS
                // without CONSUMING: it looks ahead for a closing `/` on the same line and refuses
                // the whole file if anything between them could corrupt bracket matching. Guessing
                // the end of a regex and blanking it would risk blanking real code, which is the
                // direction that invents a confident wrong answer; refusing only ever yields
                // `Unknown`, which suppresses a finding.
                //
                // Deliberately over-refuses. `a / b` has no closing `/` on the line and is not
                // affected; `1 / 2, x: {y} / 3` is, and gives up a file it did not have to. Of
                // 2179 configs across the 117-repository corpus, 178 contain a code-position `/`
                // at all and 77 a brace-bearing run that reaches this refusal.
                '/' => match classify_slash_run(&chars, i) {
                    SlashRun::Unbalanced => return None,
                    // Left unmasked deliberately: balanced brackets move no depth, and blanking a
                    // run this function only guessed at is the dangerous direction.
                    SlashRun::Balanced { close } => i = close + 1,
                    SlashRun::NotARegex => i += 1,
                },
                quote @ ('\'' | '"') => {
                    out[i] = ' ';
                    i += 1;
                    let mut closed = false;
                    while i < n {
                        if chars[i] == '\\' && i + 1 < n {
                            out[i] = ' ';
                            out[i + 1] = ' ';
                            i += 2;
                            continue;
                        }
                        if chars[i] == quote {
                            out[i] = ' ';
                            i += 1;
                            closed = true;
                            break;
                        }
                        if chars[i] == '\n' {
                            // A raw, unescaped newline never legally appears inside a single- or
                            // double-quoted JS string; treat it the same as running off the end.
                            break;
                        }
                        out[i] = ' ';
                        i += 1;
                    }
                    if !closed {
                        return None;
                    }
                }
                '`' => {
                    out[i] = ' ';
                    i += 1;
                    frames.push(MaskFrame::Template);
                }
                '{' => {
                    if let Some(MaskFrame::Hole(depth)) = frames.last_mut() {
                        *depth += 1;
                    }
                    i += 1;
                }
                '}' => {
                    if let Some(MaskFrame::Hole(depth)) = frames.last_mut() {
                        *depth -= 1;
                        if *depth == 0 {
                            frames.pop();
                        }
                    }
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }

        // Template literal text mode.
        match chars[i] {
            '\\' if i + 1 < n => {
                out[i] = ' ';
                out[i + 1] = ' ';
                i += 2;
            }
            '`' => {
                out[i] = ' ';
                i += 1;
                frames.pop();
            }
            '$' if i + 1 < n && chars[i + 1] == '{' => {
                // Mask only the `$`; the `{` is real code, left untouched, and processed by the
                // ordinary `'{' => ...` arm on the next iteration (which is what brings the new
                // `Hole`'s depth from `0` to `1` -- starting it at `1` here too would double-count
                // that same brace).
                out[i] = ' ';
                i += 1;
                frames.push(MaskFrame::Hole(0));
            }
            _ => {
                out[i] = ' ';
                i += 1;
            }
        }
    }

    if frames.is_empty() {
        Some(out)
    } else {
        // Still inside a template literal or one of its holes at end of input: truncated source.
        None
    }
}

/// What a `/` in code position turns out to be, for [`mask_non_code`].
#[derive(Debug, PartialEq, Eq)]
enum SlashRun {
    /// No closing `/` before the end of the line, so this was division, not a regex literal.
    /// The `/` is an ordinary character and the scan continues after it.
    NotARegex,
    /// A regex literal ending at this index, whose brackets are all balanced. It cannot move any
    /// caller's idea of where an object or call argument ends, so it is left in the output as-is
    /// and the scan resumes past its closing slash.
    Balanced { close: usize },
    /// A regex literal whose brackets are not balanced. Unmaskable without changing meaning.
    Unbalanced,
}

/// Classifies a `/` found in code position.
///
/// `mask_non_code` does not mask regex literals, which is mostly harmless -- a stray `.` or `$` in
/// the output changes nothing. Brackets are not harmless. `/[}]/` is a valid `testNamePattern`, and
/// its unmasked `}` ends the enclosing object for every brace-matching caller, so
///
/// ```text
/// export default { test: { testNamePattern: /[}]/, environment: 'node' } };
/// export default { test: { testNamePattern: /[}]/, environment: 'happy-dom' } };
/// ```
///
/// both yielded the scope `{ testNamePattern: /[}` -- one scope for two environments, the very
/// false positive S6 removes -- while a config whose `globals: true` fell outside the truncated
/// object was reported `Disabled`. Both measured before this existed.
///
/// BALANCE, not presence. `/^(a|b)$/` and `/x{2}/` leave every depth where they found it; only a
/// run that goes negative or ends non-zero -- `/[}]/`, `/a)b/`, `/}{/` -- can corrupt anything.
/// Each pair is counted separately, because `/[}]/` is balanced in brackets and unbalanced in
/// braces, and the braces do the damage. All three matter: `{}` and `[]` bound the object a scope
/// is cut from, `()` bounds the `defineConfig( ... )` argument `detect_globals` reads -- leaving
/// `()` out was measured to return `Disabled` for a config declaring `globals: true`.
///
/// Telling a regex from a division needs the previous significant token, i.e. a JavaScript lexer,
/// which `AGENTS.md` keeps in `src/frontend/`. So this never blanks anything: a balanced run is
/// reported with its end so the caller can step over it, and everything else is refused outright.
/// Guessing a run's extent and blanking it could blank real code, which is how a confident wrong
/// answer gets invented; refusing only ever yields `Unknown`, which suppresses a finding.
///
/// Reporting the end matters as much as the verdict. Without it the scan re-examines a regex's
/// CLOSING slash as though it opened a new run, and that run swallows the rest of the line --
/// including the config's own `}` and `)` -- so every ordinary `/abc/` was refused. Measured.
///
/// Escapes are honoured, so `/\)/` is a literal parenthesis and not a depth change. A `[...]`
/// character class is tracked only so a `/` inside one does not end the run early; brackets there
/// still count, which over-refuses a class holding a lone `(` -- the safe direction.
fn classify_slash_run(chars: &[char], open: usize) -> SlashRun {
    let mut i = open + 1;
    let mut in_class = false;
    let (mut paren, mut square, mut curly) = (0i32, 0i32, 0i32);
    // Recorded rather than returned on the spot: until a closing `/` is found on this line there
    // is no regex literal here at all, and `testTimeout: 1000 / 2, globals: true }` must not be
    // refused for the `}` that legitimately ends the object after a division. Measured -- deciding
    // as soon as a depth went negative refused exactly that shape.
    let mut went_negative = false;

    while i < chars.len() {
        match chars[i] {
            // A regex literal cannot span a line; this was division.
            '\n' => return SlashRun::NotARegex,
            // An escape decides where the run ENDS -- `/a\\/b/` closes at the last slash, not the
            // escaped one -- but it does not decide what the run CONTAINS. `mask_non_code` leaves
            // the whole run in its output, and the brace matchers downstream count raw characters
            // with no idea what a regex escape is, so the `)` in `/a\\)b/` closes their
            // `defineConfig(` exactly as an unescaped one would. Measured: skipping it here
            // reported `Disabled` for a config declaring `globals: true`.
            //
            // So the escaped character is counted, and only then stepped over. `in_class` is
            // deliberately not touched: `\\[` opens no character class, so a later `/` really does
            // close the run.
            '\\' if i + 1 < chars.len() => {
                match chars[i + 1] {
                    '(' => paren += 1,
                    ')' => paren -= 1,
                    '[' => square += 1,
                    ']' => square -= 1,
                    '{' => curly += 1,
                    '}' => curly -= 1,
                    _ => {}
                }
                went_negative |= paren < 0 || square < 0 || curly < 0;
                i += 2;
                continue;
            }
            '(' => paren += 1,
            ')' => paren -= 1,
            '[' => {
                in_class = true;
                square += 1;
            }
            ']' => {
                in_class = false;
                square -= 1;
            }
            '{' => curly += 1,
            '}' => curly -= 1,
            '/' if !in_class => {
                let unbalanced = went_negative || paren != 0 || square != 0 || curly != 0;
                return if unbalanced {
                    SlashRun::Unbalanced
                } else {
                    SlashRun::Balanced { close: i }
                };
            }
            _ => {}
        }
        // A pair closing before it opened has already broken the caller's matching, whatever the
        // rest of the run does -- but only if this run turns out to be a regex at all.
        went_negative |= paren < 0 || square < 0 || curly < 0;
        i += 1;
    }

    SlashRun::NotARegex
}

/// Result of locating the object literal that a config file's `export default` resolves to.
///
/// This is the anchor the rest of the scan hangs off: only a *direct* property of this object can
/// answer the `test.globals` question, so finding it correctly is what keeps a `test` key written
/// inside a plugin's own options, or any other unrelated nested object, from ever being considered.
pub(crate) enum ConfigObject {
    /// No `export default` assignment was found at all.
    NoExport,
    /// An `export default` was found, but its value is neither an object literal nor a single
    /// function call wrapping one (e.g. `defineConfig({ ... })`).
    NonLiteral,
    /// The default export's object literal starts with `{` but no matching `}` was found.
    Malformed,
    /// The default export resolves to the object literal spanning `masked[open..=close]`.
    Found(usize, usize),
}

/// Locates the object literal that `export default` in `masked` evaluates to, per the outcomes in
/// [`ConfigObject`]. See [`locate_object_literal_or_call_argument`] for what shapes of the
/// default-exported expression are recognized.
pub(crate) fn locate_config_object(masked: &[char]) -> ConfigObject {
    let Some(value_start) = find_export_default_value_start(masked) else {
        return ConfigObject::NoExport;
    };
    locate_object_literal_or_call_argument(masked, value_start)
}

/// Finds the index of the first non-whitespace character of the expression following a top-level
/// `export default` keyword sequence in `masked`, or `None` if no such sequence is present.
///
/// Both words are bounded by identifier boundaries (so `exported` or `defaultValue` cannot match)
/// and separated only by whitespace, matching how the masked text represents real source code once
/// comments and strings have been blanked out. A module can have at most one `export default`, so
/// the first match is unambiguous.
pub(crate) fn find_export_default_value_start(masked: &[char]) -> Option<usize> {
    let n = masked.len();
    let export_word: Vec<char> = "export".chars().collect();
    let default_word: Vec<char> = "default".chars().collect();
    let mut i = 0;
    while i + export_word.len() <= n {
        if masked[i..i + export_word.len()] == export_word[..] {
            let before_ok = i == 0 || !is_ident_char(masked[i - 1]);
            let after_export = i + export_word.len();
            let export_after_ok = after_export >= n || !is_ident_char(masked[after_export]);
            if before_ok && export_after_ok {
                let mut j = after_export;
                while j < n && is_js_whitespace(masked[j]) {
                    j += 1;
                }
                if j + default_word.len() <= n
                    && masked[j..j + default_word.len()] == default_word[..]
                {
                    let after_default = j + default_word.len();
                    let default_after_ok =
                        after_default >= n || !is_ident_char(masked[after_default]);
                    if default_after_ok {
                        let mut k = after_default;
                        while k < n && is_js_whitespace(masked[k]) {
                            k += 1;
                        }
                        return Some(k);
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// Where an expression resolves to a `<open> ... <close>` literal.
///
/// The delimiter-agnostic result [`locate_delimited_literal_or_call_argument`] returns, which
/// [`ConfigObject`] (for `{ ... }`) and `vitest_projects`'s array locator (for `[ ... ]`) each
/// translate into their own vocabulary.
pub(crate) enum DelimitedLiteral {
    /// The expression resolves to the literal spanning `masked[open..=close]`.
    Found(usize, usize),
    /// The literal's opening delimiter was found but no matching close was.
    Malformed,
    /// The expression is neither a literal of this shape nor a single call wrapping one.
    NonLiteral,
}

/// Given the start of an expression, resolves it to a literal delimited by `open`/`close` when the
/// expression either *is* one directly, or is a single function call whose first argument is one.
///
/// This is the whole of [`locate_object_literal_or_call_argument`] (`{`/`}`) and of
/// `vitest_projects`'s `locate_array_literal_or_call_argument` (`[`/`]`), whose doc comment
/// already described itself as mirroring the former. The two shapes being recognized -- a direct
/// literal, a single leading `await`, or `<dotted.callee>( <literal> ... )` -- are the same
/// JavaScript shapes whichever delimiter is being looked for, and a config-scanning bug fixed for
/// one of them has to be fixed for the other, so the recognition lives here once and each caller
/// maps the outcome onto its own result type.
///
/// A single leading `await` (e.g. `await defineVitestProject({ ... })`, a real shape observed
/// inside a `test.projects` entry) is peeled off before the rest of this resolution
/// runs; `await await x` is not a real JavaScript shape worth supporting, so at most one is
/// skipped.
///
/// Anything else -- a bare identifier, a member expression on its own, a ternary, a call with no
/// arguments or a non-literal first argument -- is [`DelimitedLiteral::NonLiteral`], since reading
/// it would require executing the expression.
pub(crate) fn locate_delimited_literal_or_call_argument(
    masked: &[char],
    start: usize,
    open: char,
    close: char,
) -> DelimitedLiteral {
    let n = masked.len();
    if start >= n {
        return DelimitedLiteral::NonLiteral;
    }
    if masked[start] == open {
        return match find_matching_delim(masked, start, open, close) {
            Some(end) => DelimitedLiteral::Found(start, end),
            None => DelimitedLiteral::Malformed,
        };
    }

    if !is_ident_char(masked[start]) {
        return DelimitedLiteral::NonLiteral;
    }

    let await_word: Vec<char> = "await".chars().collect();
    if start + await_word.len() <= n
        && masked[start..start + await_word.len()] == await_word[..]
        && (start + await_word.len() >= n || !is_ident_char(masked[start + await_word.len()]))
    {
        let mut j = start + await_word.len();
        while j < n && is_js_whitespace(masked[j]) {
            j += 1;
        }
        if j < n {
            return locate_delimited_literal_or_call_argument(masked, j, open, close);
        }
    }

    // Try `<callee>( ... )`, where `<callee>` is a dotted identifier chain (`defineConfig`,
    // `vitest.defineConfig`, ...) and the call's first argument is a literal of this shape.
    let mut i = start;
    while i < n && (is_ident_char(masked[i]) || masked[i] == '.') {
        i += 1;
    }
    while i < n && is_js_whitespace(masked[i]) {
        i += 1;
    }
    if i >= n || masked[i] != '(' {
        return DelimitedLiteral::NonLiteral;
    }
    let mut j = i + 1;
    while j < n && is_js_whitespace(masked[j]) {
        j += 1;
    }
    if j >= n || masked[j] != open {
        return DelimitedLiteral::NonLiteral;
    }
    match find_matching_delim(masked, j, open, close) {
        Some(end) => DelimitedLiteral::Found(j, end),
        None => DelimitedLiteral::Malformed,
    }
}

/// [`locate_delimited_literal_or_call_argument`] for `{ ... }`, in [`ConfigObject`]'s vocabulary:
/// the object literal an expression resolves to, directly or as the first argument of a single
/// wrapping call (`defineConfig({ ... })` / `someNamespace.defineConfig({ ... })`).
///
/// `ConfigObject::NoExport` is not reachable from here -- it describes a missing `export default`,
/// which the caller has already found by the time it has an expression start to resolve.
pub(crate) fn locate_object_literal_or_call_argument(
    masked: &[char],
    start: usize,
) -> ConfigObject {
    match locate_delimited_literal_or_call_argument(masked, start, '{', '}') {
        DelimitedLiteral::Found(open, close) => ConfigObject::Found(open, close),
        DelimitedLiteral::Malformed => ConfigObject::Malformed,
        DelimitedLiteral::NonLiteral => ConfigObject::NonLiteral,
    }
}

/// Result of looking for a `test` object-literal property, direct in some enclosing range, in a
/// masked source.
pub(crate) enum TestObject {
    /// No `test` property key was found at all. Vitest's own default (`globals: false`) applies.
    Absent,
    /// A `test` property key was found, but its value is not an object-literal `{ ... }`.
    NonLiteral,
    /// A `test` property key's value starts with `{` but no matching `}` was found.
    Malformed,
    /// A `test` property key's value is the object literal spanning `masked[open..=close]`.
    Found(usize, usize),
}

/// Locates a direct `test: { ... }` property within `range`, per the outcomes in [`TestObject`].
///
/// `range` must be the *interior* of the object literal to search (i.e. excluding its own
/// wrapping `{`/`}`), so that depth `0` in [`find_direct_key_value_starts`] means "a property of
/// this object" rather than "inside the brace that opens it." Uses the *last* match when `test`
/// appears more than once directly in `range`, matching plain-JavaScript-object semantics.
pub(crate) fn locate_test_object(masked: &[char], range: Range<usize>) -> TestObject {
    let Some(&value_start) = find_direct_key_value_starts(masked, range, "test").last() else {
        return TestObject::Absent;
    };
    if value_start >= masked.len() || masked[value_start] != '{' {
        return TestObject::NonLiteral;
    }
    match find_matching_close(masked, value_start) {
        Some(close) => TestObject::Found(value_start, close),
        None => TestObject::Malformed,
    }
}

/// Finds the index of `close_ch` matching `open_ch` at `open`, by counting only that delimiter
/// pair in `masked` from `open` onward. Sound because [`mask_non_code`] has already blanked every
/// bracket that appeared inside a string, a comment, or a template literal's literal text -- every
/// bracket left in `masked` is real code, including the paired delimiters of a template literal's
/// `${ ... }` hole (see [`MaskFrame::Hole`]). Tracking only one delimiter pair (rather than all of
/// `{[(`/`}])` at once) is deliberate: a `[` array containing object-literal elements must not
/// have its own matching `]` confused by the `{`/`}` pairs nested inside those elements, and vice
/// versa -- each bracket type's nesting is independent once strings are masked out.
pub(crate) fn find_matching_delim(
    masked: &[char],
    open: usize,
    open_ch: char,
    close_ch: char,
) -> Option<usize> {
    let mut depth = 0i32;
    for (offset, &c) in masked[open..].iter().enumerate() {
        if c == open_ch {
            depth += 1;
        } else if c == close_ch {
            depth -= 1;
            if depth == 0 {
                return Some(open + offset);
            }
        }
    }
    None
}

/// Finds the index of the `}` matching the `{` at `open`. Thin wrapper over
/// [`find_matching_delim`] kept for the many existing call sites that only ever match braces.
fn find_matching_close(masked: &[char], open: usize) -> Option<usize> {
    find_matching_delim(masked, open, '{', '}')
}

/// Whether `c` can appear in a JavaScript identifier. Used only for word-boundary checks around a
/// property-key match, not full identifier validation.
pub(crate) fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Whitespace as JavaScript's tokenizer treats it, for skipping between a property key, its `:`,
/// and its value.
pub(crate) fn is_js_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{feff}')
}

/// Finds every position within `range` where `key` appears as a *direct* (depth-zero)
/// bare-identifier object-literal property key -- not nested inside any `{`, `[`, or `(` opened
/// within `range` -- returning the index of the first non-whitespace character of each match's
/// value.
///
/// `range` must be the interior of the object literal being searched (its own wrapping `{`/`}`
/// excluded), so depth `0` means "a property of this object." This is what anchors a lookup to
/// "belongs to this object" rather than "this text appears somewhere underneath it": a `globals:
/// true` written inside a nested `environmentOptions: { ... }`, inside a plugin's own options
/// object, or inside an array element, sits at depth 1 or deeper relative to `range` and is never
/// matched here -- only a `key:` that is a direct child of `range`'s enclosing object is.
///
/// Operates purely on `masked` text. A `key` occurrence inside a comment or string is invisible
/// (already replaced with spaces by [`mask_non_code`]), and cannot desynchronize the depth counter
/// either, since [`mask_non_code`] blanks every bracket character that appears inside a string or
/// comment too -- every `{`/`[`/`(` left in `masked` is real code.
/// The key-matching and depth-tracking scan shared by [`find_direct_key_value_starts`] and
/// [`find_direct_key_value_starts_precise`].
///
/// Everything in the two public wrappers except one step is the same scan, and the doc comments on
/// both already said so. The one step that differs is `value_start`, which is handed the position
/// just past the matched `key:`'s colon and `range.end` as a limit, and returns where the value
/// actually begins. `find_direct_key_value_starts` skips masked whitespace there;
/// `find_direct_key_value_starts_precise` calls [`skip_trivia`] over `original`. Keeping the scan
/// itself in one place is what guarantees the two wrappers cannot disagree about *which* keys they
/// match -- only about where a matched key's value starts, which is the whole and only intended
/// difference between them.
fn find_direct_key_value_starts_with(
    masked: &[char],
    range: Range<usize>,
    key: &str,
    value_start: impl Fn(usize, usize) -> usize,
) -> Vec<usize> {
    let key_chars: Vec<char> = key.chars().collect();
    let mut matches = Vec::new();
    if key_chars.len() > range.len() {
        return matches;
    }
    let mut depth: i32 = 0;
    let mut i = range.start;
    while i < range.end {
        match masked[i] {
            '{' | '[' | '(' => {
                depth += 1;
                i += 1;
                continue;
            }
            '}' | ']' | ')' => {
                depth -= 1;
                i += 1;
                continue;
            }
            _ => {}
        }
        if depth == 0
            && i + key_chars.len() <= range.end
            && masked[i..i + key_chars.len()] == key_chars[..]
        {
            let before_ok = i == range.start || !is_ident_char(masked[i - 1]);
            let after_idx = i + key_chars.len();
            let after_ok = after_idx >= range.end || !is_ident_char(masked[after_idx]);
            if before_ok && after_ok {
                let mut j = after_idx;
                while j < range.end && is_js_whitespace(masked[j]) {
                    j += 1;
                }
                if j < range.end && masked[j] == ':' {
                    matches.push(value_start(j + 1, range.end));
                    i = after_idx;
                    continue;
                }
            }
        }
        i += 1;
    }
    matches
}

/// Collect direct-key value offsets in a masked object range, skipping masked whitespace
/// after each colon. Use [`find_direct_key_value_starts_precise`] for string/template values.
pub(crate) fn find_direct_key_value_starts(
    masked: &[char],
    range: Range<usize>,
    key: &str,
) -> Vec<usize> {
    find_direct_key_value_starts_with(masked, range, key, |mut pos, limit| {
        while pos < limit && is_js_whitespace(masked[pos]) {
            pos += 1;
        }
        pos
    })
}

/// Skips real whitespace and `//`/`/* */` comments starting at `pos` (bounded by `limit`),
/// stopping at the first character that is neither.
///
/// Operates on `original`, not `masked`: [`mask_non_code`] blanks a string's or template's
/// interior to spaces exactly the way it blanks a comment, so a whitespace-only skip over `masked`
/// cannot tell "still skipping trivia before the value" from "already inside a string-shaped
/// value" -- it would skip straight through the string to whatever follows it. Skipping over this
/// function reads `original` directly instead, so it stops right at a string's opening quote
/// rather than past it, while still transparently skipping comments (nothing inside one is ever a
/// value this scan needs to stop on).
pub(crate) fn skip_trivia(original: &[char], mut pos: usize, limit: usize) -> usize {
    loop {
        while pos < limit && is_js_whitespace(original[pos]) {
            pos += 1;
        }
        if pos + 1 < limit && original[pos] == '/' && original[pos + 1] == '/' {
            pos += 2;
            while pos < limit && original[pos] != '\n' {
                pos += 1;
            }
            continue;
        }
        if pos + 1 < limit && original[pos] == '/' && original[pos + 1] == '*' {
            pos += 2;
            while pos + 1 < limit && !(original[pos] == '*' && original[pos + 1] == '/') {
                pos += 1;
            }
            pos = (pos + 2).min(limit);
            continue;
        }
        break;
    }
    pos
}

/// [`find_direct_key_value_starts_with`]'s scan, resolving each match's value-start position with
/// [`skip_trivia`] over `original` instead of a masked-whitespace skip.
///
/// Needed whenever the value being looked up might itself be a string or template literal, which
/// [`find_direct_key_value_starts`]'s masked-whitespace skip cannot locate correctly (see
/// [`skip_trivia`]'s docs). Kept as a separate entry point rather than changing
/// [`find_direct_key_value_starts`] itself: every caller of that function only ever looks up
/// boolean/object/array values, where the two skips agree, so this avoids changing an
/// already-covered path for a fix it does not need. The scan the two share is now literally one
/// function, so the only thing that can differ between them is this skip.
pub(crate) fn find_direct_key_value_starts_precise(
    masked: &[char],
    original: &[char],
    range: Range<usize>,
    key: &str,
) -> Vec<usize> {
    find_direct_key_value_starts_with(masked, range, key, |pos, limit| {
        skip_trivia(original, pos, limit)
    })
}

/// Whether `key` appears directly (depth-zero, see [`find_direct_key_value_starts`]) in `range` as
/// an object-literal property key. Only presence matters for a property like `projects`, whose
/// mere existence -- not its value -- is what forces [`GlobalsDetection::Unknown`].
pub(crate) fn property_present(masked: &[char], range: Range<usize>, key: &str) -> bool {
    !find_direct_key_value_starts(masked, range, key).is_empty()
}

/// Reads `key`'s value within `range` as a boolean literal, considering only direct (depth-zero)
/// occurrences of `key` and using the *last* one when it appears more than once directly -- matching
/// plain-JavaScript-object semantics, where a later duplicate key wins at runtime. A `key` written
/// inside a nested object is a different property entirely and is never considered here.
///
/// Returns `None` if `key` is absent, `Some(None)` if present but its value is not the bare word
/// `true` or `false` (e.g. a variable, `process.env.X`, or a ternary), and `Some(Some(value))`
/// otherwise.
pub(crate) fn boolean_property(
    masked: &[char],
    range: Range<usize>,
    key: &str,
) -> Option<Option<bool>> {
    let value_start = *find_direct_key_value_starts(masked, range.clone(), key).last()?;
    Some(literal_bool_at(masked, value_start, range.end))
}

/// Reads a bare `true`/`false` word starting at `pos`, bounded so e.g. `truest` does not match
/// `true`. Returns `None` for anything else -- including no text at all at `pos`.
pub(crate) fn literal_bool_at(masked: &[char], pos: usize, end: usize) -> Option<bool> {
    let matches_word = |word: &str| -> bool {
        let word_chars: Vec<char> = word.chars().collect();
        pos + word_chars.len() <= end
            && masked[pos..pos + word_chars.len()] == word_chars[..]
            && (pos + word_chars.len() >= end || !is_ident_char(masked[pos + word_chars.len()]))
    };
    if matches_word("true") {
        Some(true)
    } else if matches_word("false") {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Write a config fixture file directly beneath root, panicking on filesystem failure.
    fn write(root: &Path, name: &str, contents: &str) {
        fs::write(root.join(name), contents).unwrap();
    }

    /// Check static Vitest config: vitest config with globals true is enabled.
    #[test]
    fn vitest_config_with_globals_true_is_enabled() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({ test: { globals: true } });",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Enabled { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: vitest config with globals false is disabled.
    #[test]
    fn vitest_config_with_globals_false_is_disabled() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({ test: { globals: false } });",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Disabled { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: vitest config without globals key is disabled.
    #[test]
    fn vitest_config_without_globals_key_is_disabled() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({ test: { environment: 'node' } });",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Disabled { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: vite config is used when no vitest config exists.
    #[test]
    fn vite_config_is_used_when_no_vitest_config_exists() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vite.config.ts",
            "export default { plugins: [], test: { globals: true } };",
        );
        let detection = detect_globals(directory.path());
        match detection {
            GlobalsDetection::Enabled { config } => {
                assert_eq!(config.file_name().unwrap(), "vite.config.ts");
            }
            other => panic!("expected Enabled from vite.config.ts, got {other:?}"),
        }
    }

    /// Check static Vitest config: vitest config takes precedence over vite config.
    #[test]
    fn vitest_config_takes_precedence_over_vite_config() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vite.config.ts",
            "export default { test: { globals: true } };",
        );
        write(
            directory.path(),
            "vitest.config.ts",
            "export default { test: { globals: false } };",
        );
        let detection = detect_globals(directory.path());
        match detection {
            GlobalsDetection::Disabled { config } => {
                assert_eq!(config.file_name().unwrap(), "vitest.config.ts");
            }
            other => panic!("expected Disabled from vitest.config.ts, got {other:?}"),
        }
    }

    /// The scope separates on any textual difference, and that is the deliberate design.
    ///
    /// Three review rounds each found a different lexical class that a whitespace normalizer here
    /// got wrong -- spaces inside string literals, spaces inside REGEX literals, and token
    /// boundaries it never normalized at all. A correct normalizer needs a JavaScript lexer, which
    /// `AGENTS.md` keeps out of this module. See [`config_test_scope`].
    ///
    /// Both directions are asserted because they fail differently. Over-separation (the formatting
    /// case) suppresses a finding; under-separation merges differently-configured directories into
    /// one `cross-file-duplicate` group, which is a false positive. Only the second is a defect,
    /// so the first is pinned as a LIMIT -- if a future lexer closes it, this assertion is the one
    /// that should be deleted, and it says so.
    #[test]
    fn a_config_scope_separates_on_any_textual_difference() {
        let scope = |source: &str| {
            let directory = tempfile::tempdir().unwrap();
            write(directory.path(), "vitest.config.ts", source);
            config_test_scope(&directory.path().join("vitest.config.ts"))
                .expect("readable")
                .expect("has a `test` object")
        };

        // Whitespace inside a string literal names a different file.
        assert_ne!(
            scope("export default { test: { include: ['a  b.test.ts'] } };"),
            scope("export default { test: { include: ['a b.test.ts'] } };"),
            "those globs name different files; collapsing whitespace inside the literal merged \
             two differently-configured directories into one group"
        );

        // Whitespace inside a regular-expression literal is significant to `testNamePattern`.
        // A literal-aware scanner that tracked quotes but not `/.../` collapsed exactly this pair.
        assert_ne!(
            scope("export default { test: { testNamePattern: /a  b/ } };"),
            scope("export default { test: { testNamePattern: /a b/ } };"),
            "`/a  b/` and `/a b/` select different tests, so the two configs are not comparable"
        );

        // The ordinary case.
        assert_ne!(
            scope("export default { test: { environment: 'node' } };"),
            scope("export default { test: { environment: 'happy-dom' } };"),
            "a changed environment must separate, or the false positive S6 removes comes back"
        );

        // RECORDED LIMIT, not a requirement: reformatting changes the scope. This over-separates,
        // which suppresses a finding -- the safe direction, and the price of having no lexer here.
        assert_ne!(
            scope("export default { test: {environment:'node'} };"),
            scope("export default { test: { environment: 'node' } };"),
            "documents that formatting is not normalized; delete this if a lexer ever makes the \
             scope formatting-independent"
        );
    }

    /// A quote or a `//` inside a regular-expression literal is read, not refused.
    ///
    /// This asserts a behaviour change. `mask_non_code` tracks `'`, `"`, `` ` `` and `//`, and used
    /// to meet them inside a regex with no idea it was in one: the apostrophe in `/it's fine/`
    /// opened a string that never closed, and the `//` in `/https:\/\//` opened a comment that
    /// swallowed the closing braces. Both produced `Unknown` -- safe, but a whole directory's
    /// findings dropped for a legal config.
    ///
    /// Stepping over a balanced run fixed that as a side effect: the run is consumed before either
    /// character is examined as code. Recorded because the previous commit pinned the old
    /// behaviour as a limit, and a limit that has been closed must stop being asserted.
    ///
    /// The fail-closed machinery itself is still pinned -- by the unterminated-string case below,
    /// and by [`an_unbalanced_bracket_in_a_regex_truncates_neither_the_scope_nor_the_globals_answer`].
    #[test]
    fn a_quote_or_comment_marker_inside_a_regex_no_longer_costs_the_whole_file() {
        let outcome = |source: &str| {
            let directory = tempfile::tempdir().unwrap();
            write(directory.path(), "vitest.config.ts", source);
            (
                config_test_scope(&directory.path().join("vitest.config.ts")),
                detect_globals(directory.path()),
            )
        };

        for source in [
            // An apostrophe: formerly an unterminated string.
            "export default { test: { testNamePattern: /it's fine/, globals: true } };",
            // An escaped slash pair: formerly a line comment.
            r"export default { test: { testNamePattern: /https:\/\//, globals: true } };",
            // A balanced pair of double quotes, which happened to work even before.
            "export default { test: { testNamePattern: /say \"hi\"/, globals: true } };",
        ] {
            let (scope, globals) = outcome(source);
            assert!(
                scope.is_ok(),
                "a legal config must be read, not given up on\n  {source}\n  -> {scope:?}"
            );
            assert!(
                matches!(globals, GlobalsDetection::Enabled { .. }),
                "{source}\n  -> {globals:?}"
            );
        }

        // A genuinely unterminated string -- one no regex run covers -- still fails closed. Without
        // this, the assertions above would pass just as well if refusal had been removed entirely.
        let (scope, globals) =
            outcome("export default { test: { name: 'unclosed, globals: true } };");
        assert!(scope.is_err(), "{scope:?}");
        assert!(
            matches!(globals, GlobalsDetection::Unknown { .. }),
            "{globals:?}"
        );
    }

    /// `Ok(None)` means one thing only: the config was READ and has no `test` object.
    ///
    /// Every other outcome is `Err`. The first version returned `Ok(None)` for an unrecognized
    /// export, which made `nearest_config_scope` keep walking and silently ignored a CommonJS
    /// config's environment -- the false positive the scope exists to remove. These refusal arms
    /// had no in-process coverage: the fixtures that exercise them run the release binary, so
    /// `cargo llvm-cov` could not see them and the module sat under its floor.
    #[test]
    fn a_config_this_scan_cannot_read_is_an_error_not_an_absent_scope() {
        let scope = |source: &str| {
            let directory = tempfile::tempdir().unwrap();
            write(directory.path(), "vitest.config.ts", source);
            config_test_scope(&directory.path().join("vitest.config.ts"))
        };

        // `Ok(None)` means exactly one thing: the config was READ and has no `test` object. Every
        // other outcome is `Err`, because a config that exists and cannot be read is where guessing
        // is wrong -- the defect that made a CommonJS config's environment silently ignored.
        assert!(
            scope("export default { test: { environment: 'node' } };")
                .unwrap()
                .is_some(),
            "the readable case, so the refusals below are not passing for the wrong reason"
        );
        assert!(
            scope("export default { other: 1 };").unwrap().is_none(),
            "read, and genuinely says nothing about how tests run"
        );
        assert!(
            scope("const config = { test: {} };\nexport default config;").is_err(),
            "a default export that is an identifier cannot be read as an object literal"
        );
        assert!(
            scope("export default { test: someSharedTestConfig };").is_err(),
            "a `test` value that is not an object literal cannot be read either"
        );

        // `detect_globals` has its own refusal for the same shape, reached by a different route.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default { test: { globals: true };",
        );
        assert!(
            matches!(
                detect_globals(directory.path()),
                GlobalsDetection::Unknown { .. }
            ),
            "a `test: {{` that never closes is malformed, and a malformed config is Unknown"
        );
    }

    /// Every escaped bracket form counts, not just the two that were reported.
    ///
    /// `classify_slash_run` steps over an escape but still counts the character it hides, because
    /// `mask_non_code` leaves the run in its output and the matchers downstream have never heard of
    /// regex escapes. Two forms were asserted when the arm was written -- `\)` and `\}` -- which
    /// left four of the six counting arms uncovered. They are one `match` apart, so the one that
    /// gets forgotten in a future edit is whichever nobody wrote a case for.
    #[test]
    fn every_escaped_bracket_in_a_regex_is_counted() {
        let refuses = |pattern: &str| {
            let directory = tempfile::tempdir().unwrap();
            write(
                directory.path(),
                "vitest.config.ts",
                &format!(
                    "export default {{ test: {{ testNamePattern: {pattern}, globals: true }} }};"
                ),
            );
            config_test_scope(&directory.path().join("vitest.config.ts")).is_err()
        };

        for pattern in [
            r"/a\(b/", r"/a\)b/", r"/a\[b/", r"/a\]b/", r"/a\{b/", r"/a\}b/",
        ] {
            assert!(
                refuses(pattern),
                "{pattern} leaves an unbalanced bracket in the masker's output, whatever the regex \
                 engine makes of the escape"
            );
        }
        // Balanced escaped pairs are fine, which is what says the counting is counting.
        assert!(
            !refuses(r"/a\(b\)c/"),
            "an escaped pair balances and must still be read"
        );
    }

    /// An unbalanced bracket inside a regular-expression literal must not truncate anything.
    ///
    /// `mask_non_code` does not mask regex literals, so their brackets reach every brace-matching
    /// caller as raw characters. `/[}]/` is a valid `testNamePattern`, and measured before the fix
    /// both of these produced the scope `{ testNamePattern: /[}` -- one scope for two different
    /// environments, which is the false positive S6 exists to remove:
    ///
    /// ```text
    /// export default { test: { testNamePattern: /[}]/, environment: 'node' } };
    /// export default { test: { testNamePattern: /[}]/, environment: 'happy-dom' } };
    /// ```
    ///
    /// Three siblings are asserted alongside it, none of them in the review that reported the
    /// brace, because a fix applied to the reported site and not its siblings is this repository's
    /// most repeated defect:
    ///
    /// - The same truncation pushed later keys out of the object, so a config declaring
    ///   `globals: true` was reported `Disabled` -- a confident wrong answer in a different
    ///   subsystem, from the same character.
    /// - `()` corrupts `defineConfig( ... )` the way `{}` corrupts the object literal. `/a)b/`
    ///   returned `Disabled` for `globals: true` until parentheses were counted too.
    /// - An ESCAPED bracket counts. `/a\)b/` is one literal paren to a regex engine, but
    ///   `mask_non_code` leaves it in its output and the paren matcher downstream has never heard
    ///   of regex escapes. Skipping it reported `Disabled` for `globals: true`.
    #[test]
    fn an_unbalanced_bracket_in_a_regex_truncates_neither_the_scope_nor_the_globals_answer() {
        let outcome = |source: String| {
            let directory = tempfile::tempdir().unwrap();
            write(directory.path(), "vitest.config.ts", &source);
            (
                config_test_scope(&directory.path().join("vitest.config.ts")),
                detect_globals(directory.path()),
            )
        };
        let bare = |body: &str| format!("export default {{ test: {{ {body} }} }};");
        let wrapped = |body: &str| {
            format!(
                "import {{ defineConfig }} from 'vitest/config';\n\
                 export default defineConfig({{ test: {{ {body} }} }});"
            )
        };

        // The reported pair: two environments that must never share one readable scope.
        let node = outcome(bare("testNamePattern: /[}]/, environment: 'node'"));
        let happy = outcome(bare("testNamePattern: /[}]/, environment: 'happy-dom'"));
        assert!(
            node.0.is_err() && happy.0.is_err(),
            "an object whose boundary cannot be trusted must be unresolved, not a scope other \
             directories can match against: {:?} / {:?}",
            node.0,
            happy.0
        );

        // Every shape that could corrupt a matcher must reach `Unknown`, never a confident answer,
        // and each of these declares `globals: true` -- so `Disabled` here is measurably wrong.
        for source in [
            bare("testNamePattern: /[}]/, globals: true"),
            bare("testNamePattern: /}{/, globals: true"),
            wrapped("testNamePattern: /a)b/, globals: true"),
            wrapped("testNamePattern: /a(b/, globals: true"),
            wrapped(r"testNamePattern: /a\)b/, globals: true"),
            wrapped(r"testNamePattern: /a\}b/, globals: true"),
        ] {
            let (_, globals) = outcome(source.clone());
            assert!(
                matches!(globals, GlobalsDetection::Unknown { .. }),
                "`globals: true` is declared here but the bracket structure is untrustworthy; a \
                 confident answer changes which calls count as tests\n  {source}\n  -> {globals:?}"
            );
        }
    }

    /// The refusal above must cost only what it has to.
    ///
    /// It gives up the whole file, so an over-broad rule would quietly turn ordinary configs into
    /// `Unknown` and drop findings. Measured over the 2179 `vite`/`vitest` configs in the
    /// 117-repository corpus: the rule refuses **1**, against a baseline of 0 -- `vitest`'s own
    /// `packages/ui/vite.config.ts`, whose HTML-extraction regex carries an unbalanced `(`, which
    /// is precisely the corruption being refused. These cases stand for the other 2178.
    ///
    /// This is the anti-vacuity half of the pair. Two earlier versions of the rule passed the test
    /// above and failed here: refusing on the mere PRESENCE of a bracket rejected 77 corpus
    /// configs, and judging depth before confirming a closing slash rejected `1000 / 2` followed by
    /// the object's own `}`. Balanced brackets move no depth and must be read normally.
    #[test]
    fn a_regex_whose_brackets_balance_is_still_read_and_division_is_not_mistaken_for_one() {
        let outcome = |source: String| {
            let directory = tempfile::tempdir().unwrap();
            write(directory.path(), "vitest.config.ts", &source);
            (
                config_test_scope(&directory.path().join("vitest.config.ts")),
                detect_globals(directory.path()),
            )
        };
        let wrapped = |body: &str| {
            format!(
                "import {{ defineConfig }} from 'vitest/config';\n\
                 export default defineConfig({{ test: {{ {body} }} }});"
            )
        };

        for body in [
            // No bracket at all.
            "testNamePattern: /abc/, globals: true",
            // Balanced, one pair of each kind.
            "testNamePattern: /^(a|b)$/, globals: true",
            "testNamePattern: /x{2}/, globals: true",
            "testNamePattern: /[a-z]+/, globals: true",
            // The shape the corpus actually carries.
            r"testNamePattern: /\.[jt]sx$/, globals: true",
            // An escaped slash does not end the run early, so the real closing slash is found.
            r"testNamePattern: /a\/b/, globals: true",
            // Division, not a regex: no closing slash on the line. The `}` that follows belongs to
            // the object and must not be read as a bracket inside a literal.
            "testTimeout: 1000 / 2, globals: true",
            // Two divisions on one line do look like a regex between them -- and are harmless,
            // because what is between them balances.
            "testTimeout: 1000 / 2, hookTimeout: 8 / 4, globals: true",
            // TWO regex literals on one line, the shape the corpus carries as `vue(/\.vue$/)`.
            // A run that is stepped over cannot have its closing slash re-read as the opener of a
            // new one. If it is, the phantom run spans from the first pattern's end to the second
            // pattern's start -- `), bar(` -- whose leading `)` goes negative, and a legal config
            // is refused. This is what pins the `close + 1` step; a pair with a `[` between the
            // patterns does NOT pin it, because `[` opens a character class that swallows both
            // slashes and the phantom run never terminates.
            "globals: true }, plugins: [foo(/a/), bar(/b/)] , x: { y: 1",
        ] {
            let source = wrapped(body);
            let (scope, globals) = outcome(source.clone());
            assert!(scope.is_ok(), "{source}\n  -> {scope:?}");
            assert!(
                matches!(globals, GlobalsDetection::Enabled { .. }),
                "this config is readable and declares `globals: true`; refusing it drops every \
                 finding in the directory\n  {source}\n  -> {globals:?}"
            );
        }
    }

    /// A balanced run reports the index of its CLOSING slash, and the caller steps past it.
    ///
    /// Not a detail: without it the scan re-examined that closing slash as though it opened a new
    /// run, and the new run swallowed the rest of the line -- including the config's own `}` and
    /// `)` -- so *every* ordinary `/abc/` was refused. Measured, and the reason this is asserted on
    /// the index rather than only through behaviour.
    #[test]
    fn a_balanced_regex_run_reports_where_it_closes() {
        let chars: Vec<char> = "x = /a(b)c/;".chars().collect();
        let open = chars.iter().position(|&c| c == '/').unwrap();
        assert_eq!(
            classify_slash_run(&chars, open),
            SlashRun::Balanced { close: 10 },
            "the run must end at the closing slash, not at the end of the line"
        );
        assert_eq!(chars[10], '/', "index 10 is the closing slash");

        // No closing slash on the line: division, and the `/` is an ordinary character.
        let division: Vec<char> = "a / b\n".chars().collect();
        assert_eq!(classify_slash_run(&division, 2), SlashRun::NotARegex);

        let unbalanced: Vec<char> = "/[}]/".chars().collect();
        assert_eq!(classify_slash_run(&unbalanced, 0), SlashRun::Unbalanced);
    }

    /// Check static Vitest config: projects with no resolved entries are unknown, not a repo-wide guess.
    #[test]
    fn projects_declaration_is_unknown_not_a_repo_wide_guess() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default { test: { globals: true, projects: ['packages/*'] } };",
        );
        let detection = detect_globals(directory.path());
        match detection {
            GlobalsDetection::Unknown { reason } => {
                assert!(reason.contains("projects"), "{reason}")
            }
            other => panic!("expected Unknown for a `projects` config, got {other:?}"),
        }
    }

    /// Check static Vitest config: workspace file forces unknown even with a root config
    /// present.
    #[test]
    fn workspace_file_forces_unknown_even_with_a_root_config_present() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default { test: { globals: true } };",
        );
        write(
            directory.path(),
            "vitest.workspace.ts",
            "export default ['packages/*'];",
        );
        let detection = detect_globals(directory.path());
        match detection {
            GlobalsDetection::Unknown { reason } => {
                assert!(reason.contains("workspace"), "{reason}")
            }
            other => panic!("expected Unknown for a workspace file, got {other:?}"),
        }
    }

    /// Check static Vitest config: globals word inside a comment does not trigger.
    #[test]
    fn globals_word_inside_a_comment_does_not_trigger() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default {\n  // globals: true\n  test: { environment: 'node' },\n};",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Disabled { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: globals word inside a string literal does not trigger.
    #[test]
    fn globals_word_inside_a_string_literal_does_not_trigger() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default { test: { setupFiles: ['globals: true'] } };",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Disabled { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: no config file at all is unknown.
    #[test]
    fn no_config_file_at_all_is_unknown() {
        let directory = tempfile::tempdir().unwrap();
        let detection = detect_globals(directory.path());
        match detection {
            GlobalsDetection::Unknown { reason } => {
                assert!(reason.contains("no vitest.config"), "{reason}");
            }
            other => panic!("expected Unknown with no config file, got {other:?}"),
        }
    }

    /// The size limit, probed at the edge rather than comfortably past it.
    ///
    /// `oversized_config_file_is_unknown` below writes `MAX_CONFIG_SOURCE_BYTES + 1` bytes of
    /// padding *on top of* a config, so it lands far above the limit and cannot tell `>` from `>=`.
    /// A mutation run reported exactly that: `read_bounded`'s
    /// `metadata.len() > MAX_CONFIG_SOURCE_BYTES` survives becoming `>=`, because nothing sat on
    /// the boundary. These two cases are one byte apart across it, which is the only arrangement
    /// that constrains the comparison itself.
    #[test]
    fn a_config_of_exactly_the_limit_is_read_and_one_byte_more_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let base = "export default { test: { globals: true } }; //";
        let pad = |total: u64| {
            let mut contents = String::from(base);
            contents.push_str(&" ".repeat(total as usize - base.len()));
            assert_eq!(contents.len() as u64, total, "the fixture must be exact");
            contents
        };

        // Exactly at the limit: accepted, and the value behind it still read.
        write(
            directory.path(),
            "vitest.config.ts",
            &pad(MAX_CONFIG_SOURCE_BYTES),
        );
        let at_limit = detect_globals(directory.path());
        assert!(
            matches!(at_limit, GlobalsDetection::Enabled { .. }),
            "a config of exactly MAX_CONFIG_SOURCE_BYTES must be read, got {at_limit:?}"
        );

        // One byte over: refused, and the reason names the limit rather than the parse.
        write(
            directory.path(),
            "vitest.config.ts",
            &pad(MAX_CONFIG_SOURCE_BYTES + 1),
        );
        match detect_globals(directory.path()) {
            GlobalsDetection::Unknown { reason } => {
                assert!(reason.contains("exceeding"), "{reason}");
            }
            other => panic!("one byte over the limit must be Unknown, got {other:?}"),
        }
    }

    /// `display_path` had nothing asserting its output: mutated to return `String::new()` for every
    /// input, the whole suite passed. It is only used in diagnostics, which is exactly why no other
    /// test notices -- and a diagnostic that names no file is a diagnostic nobody can act on.
    #[test]
    fn display_path_relativizes_under_the_root_and_falls_back_outside_it() {
        let root = Path::new("/repo");
        assert_eq!(
            display_path(&root.join("vitest.config.ts"), root),
            "vitest.config.ts"
        );
        assert_eq!(
            display_path(&root.join("packages/app/vitest.config.ts"), root),
            "packages/app/vitest.config.ts"
        );
        // Not under `root`: the full path, not an empty string and not a panic.
        assert_eq!(
            display_path(Path::new("/elsewhere/vitest.config.ts"), root),
            "/elsewhere/vitest.config.ts"
        );
        // The root itself relativizes to nothing, which is the one case where empty is correct.
        assert_eq!(display_path(root, root), "");
    }

    /// Check static Vitest config: oversized config file is unknown.
    #[test]
    fn oversized_config_file_is_unknown() {
        let directory = tempfile::tempdir().unwrap();
        let padding = " ".repeat(MAX_CONFIG_SOURCE_BYTES as usize + 1);
        write(
            directory.path(),
            "vitest.config.ts",
            &format!("export default {{ test: {{ globals: true }} }}; //{padding}"),
        );
        let detection = detect_globals(directory.path());
        match detection {
            GlobalsDetection::Unknown { reason } => {
                assert!(reason.contains("exceeding"), "{reason}")
            }
            other => panic!("expected Unknown for an oversized config, got {other:?}"),
        }
    }

    /// Check static Vitest config: malformed source with an unterminated string is unknown.
    #[test]
    fn malformed_source_with_an_unterminated_string_is_unknown() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default { test: { setupFiles: ['unterminated } };",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Unknown { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: non literal globals value is unknown.
    #[test]
    fn non_literal_globals_value_is_unknown() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default { test: { globals: process.env.CI ? true : false } };",
        );
        let detection = detect_globals(directory.path());
        match detection {
            GlobalsDetection::Unknown { reason } => {
                assert!(reason.contains("non-literal"), "{reason}")
            }
            other => panic!("expected Unknown for a non-literal `globals` value, got {other:?}"),
        }
    }

    /// Check static Vitest config: non object test value is unknown.
    #[test]
    fn non_object_test_value_is_unknown() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default { test: sharedConfig.test };",
        );
        let detection = detect_globals(directory.path());
        match detection {
            GlobalsDetection::Unknown { reason } => {
                assert!(reason.contains("object literal"), "{reason}")
            }
            other => panic!("expected Unknown for a non-literal `test` value, got {other:?}"),
        }
    }

    /// Check static Vitest config: template literal hole does not confuse brace matching.
    #[test]
    fn template_literal_hole_does_not_confuse_brace_matching() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default { test: { name: `env-${ { x: 1 }.x }`, globals: true } };",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Enabled { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: globals inside a plugin option is not mistaken for the real
    /// config.
    #[test]
    fn globals_inside_a_plugin_option_is_not_mistaken_for_the_real_config() {
        // Copilot PR #1 finding C1, failure case 1: a `test: { globals: true }` shape appearing
        // inside a plugin's own options must not be promoted to the repo-wide answer. The real
        // config sets nothing, so Vitest's own default (`globals: false`) applies.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({\n  plugins: [somePlugin({ test: { globals: true } })],\n});",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Disabled { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: globals under a sibling key does not override the direct
    /// test globals.
    #[test]
    fn globals_under_a_sibling_key_does_not_override_the_direct_test_globals() {
        // Copilot PR #1 finding C1, failure case 2: `environmentOptions.globals` is a different
        // property from `test.globals` and must not be read as if it were the same one -- doing
        // so previously reported the exact opposite of what the project configured.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({ test: { globals: false, environmentOptions: { globals: true } } });",
        );
        let detection = detect_globals(directory.path());
        match detection {
            GlobalsDetection::Disabled { .. } => {}
            other => {
                panic!("expected Disabled from the direct `test.globals: false`, got {other:?}")
            }
        }
    }

    /// Check static Vitest config: deeply nested globals under a plugin does not leak into the
    /// direct test object.
    #[test]
    fn deeply_nested_globals_under_a_plugin_does_not_leak_into_the_direct_test_object() {
        // A decoy two levels deeper than the single-level plugin-option case above, to pin that
        // depth tracking isn't accidentally only correct for one level of nesting.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({\n  plugins: [createPlugin({ options: { test: { globals: true } } })],\n  test: { environment: 'node' },\n});",
        );
        let detection = detect_globals(directory.path());
        match detection {
            GlobalsDetection::Disabled { .. } => {}
            other => panic!(
                "expected Disabled: the plugin's nested `test.globals` must not be read as the \
                 project's own, got {other:?}"
            ),
        }
    }

    /// Check static Vitest config: direct test globals true still wins over a decoy elsewhere
    /// in the file.
    #[test]
    fn direct_test_globals_true_still_wins_over_a_decoy_elsewhere_in_the_file() {
        // The legitimate case must still be detected once a plugin-option decoy is anchored away:
        // a real `test: { globals: true }` at the config's own top level is Enabled regardless of
        // an unrelated nested `globals` sitting inside a plugin's options.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({\n  plugins: [somePlugin({ test: { globals: false } })],\n  test: { globals: true },\n});",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Enabled { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: define config wrapped call with a decoy plugin option is
    /// still correctly enabled.
    #[test]
    fn define_config_wrapped_call_with_a_decoy_plugin_option_is_still_correctly_enabled() {
        // The `defineConfig({ ... })` function-call wrapper shape, combined with a decoy so the
        // test also exercises the call-argument resolution path end to end.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({ plugins: [decoy({ test: { globals: false } })], test: { globals: true } });",
        );
        let detection = detect_globals(directory.path());
        match detection {
            GlobalsDetection::Enabled { config } => {
                assert_eq!(config.file_name().unwrap(), "vitest.config.ts");
            }
            other => panic!("expected Enabled from the defineConfig-wrapped call, got {other:?}"),
        }
    }

    /// Check static Vitest config: duplicate globals key uses the last occurrence.
    #[test]
    fn duplicate_globals_key_uses_the_last_occurrence() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default { test: { globals: false, globals: true } };",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Enabled { .. }),
            "{detection:?}"
        );
    }

    // --- Shapes the `export default` anchor must refuse -----------------------------------------
    //
    // These assert through `matches!` with a guard on `reason` rather than a `match` with a
    // `panic!` arm: the two check the same two things (the variant, and what the diagnostic says),
    // and the `matches!` form does not leave a never-executed arm behind. Per this gate's caveat 1
    // such an arm counts against this file's measured coverage forever, which is a bad trade for a
    // panic message that `assert!`'s own `{detection:?}` already carries.

    /// Check static Vitest config: a config assigned to a variable without export default is
    /// unknown.
    #[test]
    fn a_config_assigned_to_a_variable_without_export_default_is_unknown() {
        // CommonJS shape. There is no `export default` to anchor to, so the scan must refuse
        // rather than fall back to an unanchored search -- which would find the `globals: true`
        // sitting in `config` and report `Enabled` for a file the anchor never resolved.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "const config = { test: { globals: true } };\nmodule.exports = config;\n",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(&detection, GlobalsDetection::Unknown { reason } if reason.contains("no `export default`")),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: a word merely starting with default is not the default
    /// export.
    #[test]
    fn a_word_merely_starting_with_default_is_not_the_default_export() {
        // `defaultTheme` starts with `default` but is a different identifier. Without the
        // identifier-boundary check after the keyword the scan would anchor at the `Theme;`
        // following it, resolve that to a non-literal expression, and report `Unknown` for a
        // config whose real `export default` on the next line says `globals: true`.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export defaultTheme;\nexport default defineConfig({ test: { globals: true } });\n",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Enabled { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: a named export before the default one does not anchor the
    /// scan.
    #[test]
    fn a_named_export_before_the_default_one_does_not_anchor_the_scan() {
        // The sibling of the boundary check above: here the word after `export` is not `default`
        // at all. The scan has to keep looking rather than anchor at `const`, and the decoy
        // `globals: false` inside the named export must have no effect on the answer.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export const shared = { test: { globals: false } };\n\
             export default defineConfig({ test: { globals: true } });\n",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Enabled { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: an arrow function default export is unknown rather than a
    /// guess.
    #[test]
    fn an_arrow_function_default_export_is_unknown_rather_than_a_guess() {
        // `export default () => ({ ... })` is a real Vite/Vitest config shape (the callback form
        // that receives the mode/command). Its object literal is only produced by *running* it, so
        // the scan must refuse -- not reach past the `(` and read the literal inside.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default () => ({ test: { globals: true } });",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(&detection, GlobalsDetection::Unknown { reason } if reason.contains("not an object literal")),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: export default with nothing after it is unknown.
    #[test]
    fn export_default_with_nothing_after_it_is_unknown() {
        // A truncated file: the keyword sequence is the last thing in it, so the expression start
        // lands exactly at end-of-input. The scan must answer `Unknown`, not index past the end.
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "vitest.config.ts", "export default");
        let detection = detect_globals(directory.path());
        assert!(
            matches!(&detection, GlobalsDetection::Unknown { reason } if reason.contains("not an object literal")),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: export default await with nothing after it is unknown.
    #[test]
    fn export_default_await_with_nothing_after_it_is_unknown() {
        // The `await` peel-off must not run off the end of the source when the awaited expression
        // is missing: `await` is skipped only when something actually follows it.
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "vitest.config.ts", "export default await");
        let detection = detect_globals(directory.path());
        assert!(
            matches!(&detection, GlobalsDetection::Unknown { reason } if reason.contains("not an object literal")),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: whitespace around a wrapping calls argument is still read.
    #[test]
    fn whitespace_around_a_wrapping_calls_argument_is_still_read() {
        // `defineConfig ( { ... } )` is unusual but legal, and a reformat can produce it. The
        // callee scan has to skip whitespace both before the `(` and after it; if either skip
        // were missing this would resolve to `NonLiteral` and a `globals: true` project would be
        // reported as `Unknown`.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig (\n  { test: { globals: true } }\n);",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Enabled { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: an object literal default export that never closes is
    /// unknown.
    #[test]
    fn an_object_literal_default_export_that_never_closes_is_unknown() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default { test: { globals: true };",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(&detection, GlobalsDetection::Unknown { reason } if reason.contains("never reaches a matching")),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: a define config argument that never closes is unknown.
    #[test]
    fn a_define_config_argument_that_never_closes_is_unknown() {
        // The same defect as the bare-literal case above, reached through the wrapping-call arm --
        // the sibling site that a fix applied only to the direct-literal path would miss.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({ test: { globals: true };",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(&detection, GlobalsDetection::Unknown { reason } if reason.contains("never reaches a matching")),
            "{detection:?}"
        );
    }

    // --- Masking: what the scan must not read as code ------------------------------------------

    /// Check static Vitest config: globals true inside a block comment does not trigger.
    #[test]
    fn globals_true_inside_a_block_comment_does_not_trigger() {
        // The commented-out setting sits at depth zero of the real `test` object, so nothing but
        // the block-comment masking keeps it from being read as the live value. Deleting that arm
        // turns this project from `Disabled` into `Enabled`.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({ test: { /* globals: true, */ environment: 'node' } });",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Disabled { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: an unterminated block comment is unknown.
    #[test]
    fn an_unterminated_block_comment_is_unknown() {
        // Past an unclosed `/*` the scan can no longer tell code from text, so it fails closed for
        // the whole file rather than reporting the `globals: true` it read before it.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({ test: { globals: true } });\n/* trailing\n",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(&detection, GlobalsDetection::Unknown { reason } if reason.contains("unterminated")),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: a raw newline inside a quoted string is treated as
    /// unterminated.
    #[test]
    fn a_raw_newline_inside_a_quoted_string_is_treated_as_unterminated() {
        // A raw newline never legally appears inside a `'`/`"` string, so the masking pass treats
        // it exactly like running off the end of the file: `Unknown`, not a best-effort read of
        // the `globals: true` that precedes it.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({ test: { globals: true, name: 'oops\n' } });",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(&detection, GlobalsDetection::Unknown { reason } if reason.contains("unterminated")),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: an escaped backtick does not end a template literal.
    #[test]
    fn an_escaped_backtick_does_not_end_a_template_literal() {
        // If the escape were not consumed as a pair, the `\` would be masked as ordinary template
        // text and the backtick after it would close the template -- promoting the `{` that
        // follows to live code, unbalancing the brace scan, and turning this `Enabled` config into
        // `Unknown`.
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            r#"export default defineConfig({
  test: { globals: true },
  define: { BANNER: `a\`{ b` },
});"#,
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Enabled { .. }),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: an unterminated template literal is unknown.
    #[test]
    fn an_unterminated_template_literal_is_unknown() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({ test: { globals: true } });\nconst banner = `oops\n",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(&detection, GlobalsDetection::Unknown { reason } if reason.contains("unterminated")),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: a config file that is not valid utf8 is unknown.
    #[test]
    fn a_config_file_that_is_not_valid_utf8_is_unknown() {
        // `read_bounded` refuses the file rather than lossily decoding it: a replacement character
        // landing inside an identifier would change which properties the scan believes it sees.
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("vitest.config.ts"),
            b"export default { test: { globals: true } }; // \xff\xfe",
        )
        .unwrap();
        let detection = detect_globals(directory.path());
        assert!(
            matches!(&detection, GlobalsDetection::Unknown { reason } if reason.contains("not valid UTF-8")),
            "{detection:?}"
        );
    }

    /// Check static Vitest config: locate test object reports malformed when the test brace
    /// never closes.
    #[test]
    fn locate_test_object_reports_malformed_when_the_test_brace_never_closes() {
        // Called directly, because no config file can reach this arm: `detect_globals` only looks
        // for `test` *inside* an object literal whose own closing `}` it has already matched, and
        // brace depth cannot fall from that literal's opening level to zero without passing
        // through the level that closes an inner `test: {` on the way. The arm still has to answer
        // `Malformed` rather than `Found` with a fabricated end, so it is pinned here instead of
        // left unmeasured.
        let masked: Vec<char> = "test: {".chars().collect();
        let len = masked.len();
        assert!(
            matches!(locate_test_object(&masked, 0..len), TestObject::Malformed),
            "an unclosed `test: {{` must be Malformed, not Found"
        );
    }

    // ---------------------------------------------------------------------------------------------
    // Rows specified by the 2026-09-27 tests-implementation plan (S3/S4 block)
    // ---------------------------------------------------------------------------------------------

    /// R21: with exactly one `test.projects` entry there is one repo-wide answer to
    /// `test.globals` -- the entry's own -- and `detect_globals` must read it. Today the mere
    /// presence of `projects` bails to `Unknown`, which is the 23-of-29 failure S3 names; the
    /// flag is ignored by every test until the fix teaches the single-entry case.
    #[test]
    fn detect_globals_reads_globals_from_a_single_projects_entry() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({ test: { projects: [{ test: { name: 'unit', globals: true } }] } });",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Enabled { .. }),
            "one entry carries one answer: {detection:?}"
        );
    }

    /// The unanimity fold must preserve a unanimous `false`, not merely detect the positive case.
    /// This also keeps a future implementation from treating any resolved project as evidence for
    /// ambient globals.
    #[test]
    fn detect_globals_reads_disabled_globals_from_a_single_projects_entry() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({ test: { projects: [{ test: { name: 'unit', globals: false } }] } });",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Disabled { .. }),
            "one disabled entry carries one negative answer: {detection:?}"
        );
    }

    /// R22: the negative twin of R21. Two entries that DISAGREE about `globals` have no single
    /// repo-wide answer, and folding to "take the first entry" would silently scope the wrong
    /// mode over every other project's files. Stays `Unknown` today and must stay `Unknown` after
    /// the single-entry fix.
    #[test]
    fn detect_globals_stays_unknown_when_two_projects_entries_disagree() {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "vitest.config.ts",
            "export default defineConfig({ test: { projects: [{ test: { name: 'a', globals: true } }, { test: { name: 'b', globals: false } }] } });",
        );
        let detection = detect_globals(directory.path());
        assert!(
            matches!(detection, GlobalsDetection::Unknown { .. }),
            "disagreeing entries have no single repo-wide answer: {detection:?}"
        );
    }
}
