//! Configuration loading and precedence.
//!
//! Precedence, lowest to highest:
//!
//! 1. Built-in defaults ([`crate::discovery::DEFAULT_INCLUDE_GLOBS`],
//!    [`crate::discovery::DEFAULT_BENCHMARK_INCLUDE_GLOBS`],
//!    [`crate::discovery::DEFAULT_EXCLUDE_GLOBS`], every rule at its `default_severity()`,
//!    threshold `0`).
//! 2. `.vitest-dedup.json`, found by walking upward from the analysis root to the filesystem
//!    root, or an explicit file passed via [`ConfigOverrides::config_file`] (`--config`).
//! 3. [`ConfigOverrides`] built from CLI flags. These always win: an operator invoking the tool
//!    interactively needs to be able to override a checked-in project file without editing it.

use crate::model::{Rule, Severity};
use anyhow::{bail, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The configuration filename auto-discovered above the analysis root.
const CONFIG_FILE_NAME: &str = ".vitest-dedup.json";

/// Byte limit on the configuration file itself.
///
/// Configuration is untrusted input in the same way source files are: a huge or hostile config
/// should fail with a clear message rather than let `serde_json` allocate against it unbounded.
pub const MAX_CONFIG_FILE_BYTES: u64 = 1024 * 1024;

/// Every rule this build knows about.
///
/// Deliberately an alias for [`Rule::ALL`] rather than a second list. A previous version declared
/// its own array here, and both it and `Rule::ALL` documented themselves as "the single place"
/// rules are enumerated. They agreed only by luck: adding a variant to `Rule::ALL` alone would
/// have left the rule unparseable from `.vitest-dedup.json` and un-overridable from `--rule`, with
/// no compile error and no test failure. That is precisely the silent-narrowing defect this
/// analyzer exists to find in other people's code.
const ALL_RULES: &[Rule] = Rule::ALL;

/// Parses a stable rule id (as returned by [`Rule::id`]) into its `Rule` value.
///
/// # Errors
/// Returns a message naming every valid id when `id` does not match one, so a typo in
/// `.vitest-dedup.json` or `--rule` is actionable rather than a bare "invalid value".
pub fn parse_rule_id(id: &str) -> Result<Rule, String> {
    ALL_RULES
        .iter()
        .copied()
        .find(|rule| rule.id() == id)
        .ok_or_else(|| {
            let known = ALL_RULES
                .iter()
                .map(|rule| rule.id())
                .collect::<Vec<_>>()
                .join(", ");
            format!("unknown rule `{id}` (expected one of: {known})")
        })
}

/// Parses a severity string using the same spelling [`Severity`] round-trips through JSON
/// (`off`, `warning`, `error`).
///
/// A free function rather than `FromStr` on `Severity` because `Severity` lives in `model.rs`,
/// which this crate's architecture keeps free of configuration-string parsing.
pub fn parse_severity(value: &str) -> Result<Severity, String> {
    match value {
        "off" => Ok(Severity::Off),
        "warning" => Ok(Severity::Warning),
        "error" => Ok(Severity::Error),
        _ => Err(format!(
            "invalid severity `{value}` (expected one of: off, warning, error)"
        )),
    }
}

/// The on-disk shape of `.vitest-dedup.json`.
///
/// `deny_unknown_fields` turns a typo'd top-level key into a load error instead of a silently
/// ignored setting. `rules` is deserialized as raw strings on both sides (not `Rule`/`Severity`
/// directly) so [`parse_rule_id`] and [`parse_severity`] give one consistent, actionable error
/// message regardless of which key or value was wrong -- serde's own generated message for an
/// unrecognized enum variant would work but would not match the CLI's `--rule` error wording.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    include: Option<Vec<String>>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
    #[serde(default)]
    threshold: Option<i64>,
    #[serde(default)]
    rules: Option<BTreeMap<String, String>>,
}

/// Command-line (or embedder-supplied) overrides applied after file configuration.
///
/// See the module-level doc comment for the full precedence order. Only fields that correspond
/// to something [`Config`] actually stores are here: reporters, output location, and
/// `--fail-on-incomplete` are CLI concerns the orchestrator reads directly from parsed arguments,
/// not part of the analyzed configuration surface this module owns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigOverrides {
    /// Explicit configuration file, bypassing upward auto-discovery entirely.
    pub config_file: Option<PathBuf>,
    /// Replacement quality-gate threshold percentage (0-100).
    pub threshold: Option<u8>,
    /// Rule severities to apply after file configuration.
    pub rules: BTreeMap<Rule, Severity>,
}

/// Fully resolved and validated analyzer configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
    /// Canonicalized analysis root every relative path -- discovered files included -- resolves
    /// against.
    pub root: PathBuf,
    /// Glob patterns selecting which files [`crate::discovery::discover`] walks over.
    pub include: Vec<String>,
    /// Whether [`Config::include`] came from the user rather than the built-in defaults.
    ///
    /// The distinction decides who wins when a Vitest project declares its own, broader `include`.
    /// An explicit user choice is an instruction and must not be widened behind their back; the
    /// built-in defaults are only a guess at what a test file looks like, and a project that says
    /// otherwise knows better.
    pub include_is_explicit: bool,
    /// Glob patterns pruned from discovery, in addition to `.gitignore` / `.vitest-dedupignore`.
    pub exclude: Vec<String>,
    /// Effective severity for each rule.
    pub rules: BTreeMap<Rule, Severity>,
    /// Quality-gate threshold percentage (0-100). Findings and thresholds are combined by the
    /// analysis layer; this module only stores and range-validates the number.
    pub threshold: u8,
    /// Human-readable origin of the effective configuration (`"built-in defaults"`, a path, ...),
    /// surfaced in diagnostics so a confusing effective config is easy to trace back.
    pub source: String,
}

impl Config {
    /// Loads configuration for `root`: built-in defaults, then file configuration (explicit or
    /// auto-discovered), then `overrides`. See the module doc comment for precedence.
    pub fn load(root: &Path, overrides: ConfigOverrides) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("cannot access analysis root {}", root.display()))?;
        if !root.is_dir() {
            bail!("analysis root {} is not a directory", root.display());
        }

        let mut config = Self::defaults(root.clone());

        let explicit_path = overrides.config_file.as_ref().map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                root.join(path)
            }
        });
        let (raw, source_path) = if let Some(path) = explicit_path {
            (Some(read_raw_config(&path)?), Some(path))
        } else {
            discover_config_file(&root)?
        };

        if let Some(raw) = raw {
            config.apply_raw(raw)?;
            config.source = source_path
                .as_ref()
                .map(|path| display_relative(path, &root))
                .unwrap_or_else(|| CONFIG_FILE_NAME.to_owned());
        }

        config.apply_overrides(overrides);
        config.validate()?;
        Ok(config)
    }

    /// Build configuration for the supplied root using test/benchmark discovery defaults and
    /// each rule's default severity.
    fn defaults(root: PathBuf) -> Self {
        Self {
            root,
            include: crate::discovery::DEFAULT_INCLUDE_GLOBS
                .iter()
                .chain(crate::discovery::DEFAULT_BENCHMARK_INCLUDE_GLOBS)
                .map(|pattern| (*pattern).to_owned())
                .collect(),
            exclude: crate::discovery::DEFAULT_EXCLUDE_GLOBS
                .iter()
                .map(|pattern| (*pattern).to_owned())
                .collect(),
            rules: ALL_RULES
                .iter()
                .map(|rule| (*rule, rule.default_severity()))
                .collect(),
            include_is_explicit: false,
            threshold: 0,
            source: "built-in defaults".to_owned(),
        }
    }

    /// Apply present config-file fields and rule overrides, rejecting invalid thresholds, rule
    /// names or severity names.
    fn apply_raw(&mut self, raw: RawConfig) -> Result<()> {
        if let Some(include) = raw.include {
            self.include = include;
            self.include_is_explicit = true;
        }
        if let Some(exclude) = raw.exclude {
            self.exclude = exclude;
        }
        if let Some(threshold) = raw.threshold {
            self.threshold = validate_threshold_range(threshold)?;
        }
        if let Some(rules) = raw.rules {
            for (id, severity) in rules {
                let rule = parse_rule_id(&id).map_err(anyhow::Error::msg)?;
                let severity = parse_severity(&severity).map_err(anyhow::Error::msg)?;
                self.rules.insert(rule, severity);
            }
        }
        Ok(())
    }

    /// Apply an optional CLI threshold and extend configured rule severities with CLI
    /// overrides.
    fn apply_overrides(&mut self, overrides: ConfigOverrides) {
        if let Some(threshold) = overrides.threshold {
            self.threshold = threshold;
        }
        self.rules.extend(overrides.rules);
    }

    /// Reject thresholds above 100 and invalid include/exclude globs before discovery begins.
    fn validate(&self) -> Result<()> {
        if self.threshold > 100 {
            bail!(
                "threshold must be between 0 and 100, got {}",
                self.threshold
            );
        }
        // Malformed globs are caught here, at load time, rather than surfacing later as a
        // discovery-time failure deep in a walk that had already done useful work.
        compile_glob_set(&self.include, "include")?;
        compile_glob_set(&self.exclude, "exclude")?;
        Ok(())
    }

    /// The configured severity for `rule`, falling back to the rule's own default when neither
    /// file configuration nor CLI overrides mention it.
    pub fn severity(&self, rule: Rule) -> Severity {
        self.rules
            .get(&rule)
            .copied()
            .unwrap_or_else(|| rule.default_severity())
    }
}

/// Compiles glob patterns into a matcher, with an actionable error naming the offending pattern.
///
/// `pub(crate)` because both this module (to validate at load time) and `discovery` (to actually
/// apply the patterns) need identical compilation semantics; sharing one function guarantees a
/// pattern that passes validation here is the same pattern discovery will later match with.
///
/// A single leading `./` is normalized away here, once, for every pattern channel that flows
/// through this function -- `include`, `exclude`, `includeSource`, and the per-project channels
/// alike. Every matcher is applied to paths made relative to the analysis root (or the project's
/// base directory), and `'./pkg/**'` does not match `'pkg/a.spec.ts'` under those semantics, so a
/// config author who writes the prefix the way shell users do silently matched nothing: the
/// 2026-09-27 scan measured a one-character include of this shape costing a repository all 30 of
/// its test files, and an `exclude` of the same shape letting 568 of another repository's 733
/// finding locations through (backlog S1). Exactly one leading `./`, and nothing else: a `./` in
/// any later position (`pkg/./x`) or after a doubled slash (`.//pkg`) is left alone, matching the
/// precedent of `vitest_projects::expand_project_path`'s own strip, so normalization cannot
/// silently widen what a pattern claims.
pub(crate) fn compile_glob_set(patterns: &[String], kind: &str) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let stripped = pattern.strip_prefix("./").unwrap_or(pattern);
        builder.add(
            Glob::new(stripped)
                .with_context(|| format!("invalid {kind} glob pattern `{pattern}`"))?,
        );
    }
    builder
        .build()
        .with_context(|| format!("failed to build {kind} glob set"))
}

/// Convert a signed threshold to u8 only when it lies in the inclusive range 0 through 100.
fn validate_threshold_range(value: i64) -> Result<u8> {
    u8::try_from(value)
        .ok()
        .filter(|value| *value <= 100)
        .ok_or_else(|| anyhow::anyhow!("threshold must be between 0 and 100, got {value}"))
}

/// Reads and parses `.vitest-dedup.json`, enforcing [`MAX_CONFIG_FILE_BYTES`] with a bounded
/// reader rather than trusting a `stat()` taken before the read.
fn read_raw_config(path: &Path) -> Result<RawConfig> {
    let file = File::open(path)
        .with_context(|| format!("failed to read configuration file {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect configuration file {}", path.display()))?;
    if metadata.len() > MAX_CONFIG_FILE_BYTES {
        bail!(
            "configuration file {} is {} bytes, exceeding the {}-byte limit",
            path.display(),
            metadata.len(),
            MAX_CONFIG_FILE_BYTES
        );
    }
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_CONFIG_FILE_BYTES) as usize);
    file.take(MAX_CONFIG_FILE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read configuration file {}", path.display()))?;
    if bytes.len() as u64 > MAX_CONFIG_FILE_BYTES {
        bail!(
            "configuration file {} exceeds the {}-byte limit",
            path.display(),
            MAX_CONFIG_FILE_BYTES
        );
    }
    let text = String::from_utf8(bytes)
        .with_context(|| format!("configuration file {} is not valid UTF-8", path.display()))?;
    serde_json::from_str::<RawConfig>(&text)
        .with_context(|| format!("failed to parse configuration file {}", path.display()))
}

/// Walks upward from `root` looking for `.vitest-dedup.json`, stopping at the first match or at
/// the filesystem root.
///
/// This project has no workspace/monorepo-root concept for *its own* config file: Vitest
/// project/workspace boundaries are resolved by [`crate::vitest_config`] and
/// [`crate::vitest_projects`], and nothing has yet taught this walk to respect them. So the
/// filesystem root is the only boundary that is always
/// correct; a workspace-aware module can stop the walk earlier once it exists. A file found this
/// way that fails to parse is a fatal error rather than a skip-and-keep-looking: a config typo
/// must never silently fall back to "no config found."
fn discover_config_file(root: &Path) -> Result<(Option<RawConfig>, Option<PathBuf>)> {
    let mut current = Some(root);
    while let Some(directory) = current {
        let candidate = directory.join(CONFIG_FILE_NAME);
        if candidate.is_file() {
            return Ok((Some(read_raw_config(&candidate)?), Some(candidate)));
        }
        current = directory.parent();
    }
    Ok((None, None))
}

/// Display a path relative to root when possible, with forward slashes even on Windows.
fn display_relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Check configuration: defaults apply when no config file exists.
    #[test]
    fn defaults_apply_when_no_config_file_exists() {
        let directory = tempfile::tempdir().unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert_eq!(config.threshold, 0);
        assert_eq!(config.source, "built-in defaults");
        assert_eq!(config.severity(Rule::DuplicateTest), Severity::Error);
        assert_eq!(config.severity(Rule::DuplicateTestName), Severity::Warning);
    }

    /// Check configuration: unknown top level key is a load error not a silent ignore.
    #[test]
    fn unknown_top_level_key_is_a_load_error_not_a_silent_ignore() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".vitest-dedup.json"),
            r#"{"thresholdd":10}"#,
        )
        .unwrap();
        let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
        assert!(error.to_string().contains("failed to parse"), "{error:#}");
    }

    /// Check configuration: typo'd rule name is an error not a silently disabled rule.
    #[test]
    fn typo_d_rule_name_is_an_error_not_a_silently_disabled_rule() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".vitest-dedup.json"),
            r#"{"rules":{"duplicate-tets":"off"}}"#,
        )
        .unwrap();
        let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
        assert!(error.to_string().contains("unknown rule `duplicate-tets`"));
    }

    /// Check configuration: invalid severity string is an actionable error.
    #[test]
    fn invalid_severity_string_is_an_actionable_error() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".vitest-dedup.json"),
            r#"{"rules":{"duplicate-test":"critical"}}"#,
        )
        .unwrap();
        let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
        assert!(error.to_string().contains("invalid severity `critical`"));
    }

    /// Check configuration: threshold out of range is rejected at 101 and negative.
    #[test]
    fn threshold_out_of_range_is_rejected_at_101_and_negative() {
        let directory = tempfile::tempdir().unwrap();
        for (body, needle) in [
            (r#"{"threshold":101}"#, "between 0 and 100"),
            (r#"{"threshold":-1}"#, "between 0 and 100"),
        ] {
            fs::write(directory.path().join(".vitest-dedup.json"), body).unwrap();
            let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
            assert!(error.to_string().contains(needle), "{error:#}");
        }
        fs::write(
            directory.path().join(".vitest-dedup.json"),
            r#"{"threshold":100}"#,
        )
        .unwrap();
        assert_eq!(
            Config::load(directory.path(), ConfigOverrides::default())
                .unwrap()
                .threshold,
            100
        );
    }

    /// Check configuration: malformed glob is an operational error.
    #[test]
    fn malformed_glob_is_an_operational_error() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".vitest-dedup.json"),
            r#"{"include":["["]}"#,
        )
        .unwrap();
        let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
        assert!(error.to_string().contains("invalid include glob pattern"));
    }

    /// Check configuration: config file over the size limit is rejected.
    #[test]
    fn config_file_over_the_size_limit_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let padding = "0".repeat(MAX_CONFIG_FILE_BYTES as usize);
        fs::write(
            directory.path().join(".vitest-dedup.json"),
            format!(r#"{{"threshold":{padding}}}"#),
        )
        .unwrap();
        let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
        assert!(error.to_string().contains("exceeding the"), "{error:#}");
    }

    /// Check configuration: cli overrides win over file configuration.
    #[test]
    fn cli_overrides_win_over_file_configuration() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".vitest-dedup.json"),
            r#"{"threshold":10,"rules":{"duplicate-test":"warning"}}"#,
        )
        .unwrap();
        let mut rules = BTreeMap::new();
        rules.insert(Rule::DuplicateTest, Severity::Error);
        let config = Config::load(
            directory.path(),
            ConfigOverrides {
                threshold: Some(30),
                rules,
                ..ConfigOverrides::default()
            },
        )
        .unwrap();
        assert_eq!(config.threshold, 30);
        assert_eq!(config.severity(Rule::DuplicateTest), Severity::Error);
    }

    /// Check configuration: auto discovery walks upward from the analysis root.
    #[test]
    fn auto_discovery_walks_upward_from_the_analysis_root() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".vitest-dedup.json"),
            r#"{"threshold":42}"#,
        )
        .unwrap();
        let nested = directory.path().join("packages/app/src");
        fs::create_dir_all(&nested).unwrap();

        let config = Config::load(&nested, ConfigOverrides::default()).unwrap();
        assert_eq!(config.threshold, 42);
        // The config file lives above `root`, not beneath it, so there is no meaningful
        // root-relative form to show; the full path is the honest answer.
        assert!(config.source.ends_with(".vitest-dedup.json"));
    }

    /// Check configuration: nearest config file wins over a more distant ancestor.
    #[test]
    fn nearest_config_file_wins_over_a_more_distant_ancestor() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".vitest-dedup.json"),
            r#"{"threshold":10}"#,
        )
        .unwrap();
        let nested = directory.path().join("packages/app");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join(".vitest-dedup.json"), r#"{"threshold":20}"#).unwrap();

        let config = Config::load(&nested, ConfigOverrides::default()).unwrap();
        assert_eq!(config.threshold, 20);
    }

    /// Check configuration: explicit config file bypasses auto discovery.
    #[test]
    fn explicit_config_file_bypasses_auto_discovery() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".vitest-dedup.json"),
            r#"{"threshold":10}"#,
        )
        .unwrap();
        let explicit = directory.path().join("team-policy.json");
        fs::write(&explicit, r#"{"threshold":55}"#).unwrap();

        let config = Config::load(
            directory.path(),
            ConfigOverrides {
                config_file: Some(explicit),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();
        assert_eq!(config.threshold, 55);
    }

    /// Check configuration: missing explicit config file is a fatal error.
    #[test]
    fn missing_explicit_config_file_is_a_fatal_error() {
        let directory = tempfile::tempdir().unwrap();
        let error = Config::load(
            directory.path(),
            ConfigOverrides {
                config_file: Some(directory.path().join("missing.json")),
                ..ConfigOverrides::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("failed to read"));
    }

    /// Check configuration: analysis root must be a directory.
    #[test]
    fn analysis_root_must_be_a_directory() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("not-a-directory");
        fs::write(&file, "x").unwrap();
        let error = Config::load(&file, ConfigOverrides::default()).unwrap_err();
        assert!(error.to_string().contains("not a directory"));
    }

    /// `exclude` in the file replaces the built-in prune list outright rather than extending it,
    /// and -- unlike `include` -- it does not set `include_is_explicit`, because the two answer
    /// different questions: `include_is_explicit` decides whether a Vitest project may widen the
    /// file selection, and a project widening `include` is unrelated to what this configuration
    /// asked to prune.
    #[test]
    fn exclude_from_the_file_replaces_the_built_in_prune_list_without_claiming_include() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".vitest-dedup.json"),
            r#"{"exclude":["**/generated/**"]}"#,
        )
        .unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert_eq!(config.exclude, vec!["**/generated/**".to_owned()]);
        assert!(
            !config.include_is_explicit,
            "excluding files is not a statement about which files count as tests"
        );
        assert_eq!(
            config.include,
            Config::load(
                tempfile::tempdir().unwrap().path(),
                ConfigOverrides::default()
            )
            .unwrap()
            .include,
            "the default include globs must survive an exclude-only configuration"
        );
    }

    /// `validate()` is the only thing standing between an out-of-range *override* and the quality
    /// gate: `ConfigOverrides::threshold` is a `u8`, so the file-level `validate_threshold_range`
    /// never sees it, and every value from 101 to 255 would otherwise be stored as a threshold no
    /// percentage can ever reach -- a gate that can never fail. The bound is probed one past the
    /// edge (101) and at it (100), never comfortably inside.
    #[test]
    fn an_out_of_range_threshold_override_is_rejected_even_though_the_file_parser_never_sees_it() {
        let directory = tempfile::tempdir().unwrap();
        for threshold in [101u8, 255u8] {
            let error = Config::load(
                directory.path(),
                ConfigOverrides {
                    threshold: Some(threshold),
                    ..ConfigOverrides::default()
                },
            )
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("between 0 and 100, got {threshold}")),
                "{error:#}"
            );
        }
        assert_eq!(
            Config::load(
                directory.path(),
                ConfigOverrides {
                    threshold: Some(100),
                    ..ConfigOverrides::default()
                },
            )
            .unwrap()
            .threshold,
            100,
            "100 is in range and must load"
        );
    }

    /// The byte limit is enforced twice, and the second check is the one that matters: the first
    /// trusts `metadata().len()`, taken before the read, and a file whose reported size
    /// understates its content would sail past it. `/dev/zero` is that file -- it stats as zero
    /// bytes and yields as many as asked for -- so this reaches the post-read guard the way a
    /// growing file or a pipe would, without a race to lose.
    ///
    /// Without the second check the oversized content reaches `serde_json` and the operator gets
    /// a parse error about a megabyte of NUL bytes instead of the limit that was crossed.
    #[cfg(unix)]
    #[test]
    fn a_file_whose_stat_size_understates_its_content_is_still_bounded_after_the_read() {
        let directory = tempfile::tempdir().unwrap();
        let zero = Path::new("/dev/zero");
        assert_eq!(
            fs::metadata(zero).unwrap().len(),
            0,
            "this test needs a file that stats smaller than it reads"
        );
        let error = Config::load(
            directory.path(),
            ConfigOverrides {
                config_file: Some(zero.to_path_buf()),
                ..ConfigOverrides::default()
            },
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("exceeds the {MAX_CONFIG_FILE_BYTES}-byte limit")),
            "{error:#}"
        );
    }

    // ---------------------------------------------------------------------------------------------
    // S1 (2026-09-27 scan): a leading `./` in a glob matched nothing
    // ---------------------------------------------------------------------------------------------

    /// The one-character repro and its exclude sibling, stated at the chokepoint every pattern
    /// channel flows through. `'./pkg/**'` must match exactly what `'pkg/**'` matches, because
    /// matchers are applied to root-relative paths and the prefix is how shell users spell the
    /// same thing. End to end this is pinned twice more: recall case
    /// `duplicate-test-include-leading-dot-slash` and `vitest_projects::tests::
    /// a_leading_dot_slash_in_an_include_glob_matches_the_same_files_as_without`.
    #[test]
    fn a_leading_dot_slash_prefix_matches_the_same_files_as_the_bare_glob() {
        let prefixed = compile_glob_set(&["./pkg/**/*.spec.ts".to_owned()], "include").unwrap();
        let bare = compile_glob_set(&["pkg/**/*.spec.ts".to_owned()], "include").unwrap();
        for path in ["pkg/api.spec.ts", "pkg/nested/deep/api.spec.ts"] {
            assert!(
                prefixed.is_match(path),
                "`{path}` must match the ./-prefixed include"
            );
            assert!(bare.is_match(path), "`{path}` must match the bare include");
        }
        assert!(!prefixed.is_match("elsewhere.spec.ts"));

        // The same normalization on the exclude side, where the scan measured 568 of a
        // repository's 733 finding locations slipping through a `./`-prefixed exclude.
        let exclude = compile_glob_set(&["./playground/**".to_owned()], "exclude").unwrap();
        assert!(exclude.is_match("playground/assets/a.test.ts"));
        assert!(!exclude.is_match("src/a.test.ts"));
    }

    /// The boundary beside the fix: a `./` anywhere but the very front is NOT normalized --
    /// folding `pkg/./x` to `pkg/x`, or `.//pkg` to `pkg`, would let the strip silently widen
    /// what a pattern claims. Mirrors `vitest_projects::tests::
    /// a_dot_slash_in_a_non_initial_segment_is_not_stripped` for this site.
    #[test]
    fn a_dot_slash_in_a_non_initial_position_is_not_normalized_here_either() {
        for pattern in [".//pkg/**/*.ts", "pkg/./**/*.ts", "../pkg/**/*.ts"] {
            let compiled = compile_glob_set(&[pattern.to_owned()], "include").unwrap();
            assert!(
                !compiled.is_match("pkg/a.ts"),
                "`{pattern}` folded to `pkg/**` and stopped being its own pattern"
            );
        }
    }
}
