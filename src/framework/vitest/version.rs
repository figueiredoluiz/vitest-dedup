//! The Vitest major a test file imports, read from installed or declared package metadata.
//!
//! An installed `node_modules/vitest` above the file decides, as Node's resolution would; only
//! when none is installed does the nearest `package.json` declaring `vitest` with a one-major
//! range answer. Nothing outside the analysis root is read, and anything unreadable is unknown.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The Vitest majors that export a top-level `bench`, as measured: 4.1.12 does, 5.0.5 does not.
const BENCH_MAJORS: &[u64] = &[4];

/// Per-directory answers, so a tree of test files reads each manifest once.
#[derive(Debug)]
pub struct Majors {
    root: PathBuf,
    installed: HashMap<PathBuf, Option<Answer>>,
    declared: HashMap<PathBuf, Option<Answer>>,
}

/// What one directory's metadata says: a major, or present but unreadable.
type Answer = Option<u64>;

impl Majors {
    /// Answers for files under `root`.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            installed: HashMap::new(),
            declared: HashMap::new(),
        }
    }

    /// The major of the `vitest` that `file` imports, when its metadata proves one.
    pub fn of(&mut self, file: &Path) -> Option<u64> {
        let directory = file.parent()?;
        match self.nearest(directory, Source::Installed) {
            Some(answer) => answer,
            None => self.nearest(directory, Source::Declared).flatten(),
        }
    }

    /// Whether `file` imports a Vitest whose top-level `bench` exists.
    pub fn exports_bench(&mut self, file: &Path) -> bool {
        self.of(file)
            .is_some_and(|major| BENCH_MAJORS.contains(&major))
    }

    /// The nearest directory's answer from `source`, walking up to the root: `None` when no
    /// directory in between has that kind of metadata at all.
    fn nearest(&mut self, directory: &Path, source: Source) -> Option<Answer> {
        if !directory.starts_with(&self.root) {
            return None;
        }
        let cache = match source {
            Source::Installed => &self.installed,
            Source::Declared => &self.declared,
        };
        if let Some(answer) = cache.get(directory) {
            return *answer;
        }
        let answer = source.read(directory).or_else(|| {
            directory
                .parent()
                .and_then(|parent| self.nearest(parent, source))
        });
        match source {
            Source::Installed => &mut self.installed,
            Source::Declared => &mut self.declared,
        }
        .insert(directory.to_path_buf(), answer);
        answer
    }
}

/// Which metadata a directory is asked for.
#[derive(Clone, Copy, Debug)]
enum Source {
    /// `node_modules/vitest/package.json`'s `version`.
    Installed,
    /// The directory's own `package.json` dependency range on `vitest`.
    Declared,
}

impl Source {
    /// This directory's answer, `None` when it has no such metadata.
    fn read(self, directory: &Path) -> Option<Answer> {
        let path = match self {
            Self::Installed => directory.join("node_modules/vitest/package.json"),
            Self::Declared => directory.join("package.json"),
        };
        if std::fs::symlink_metadata(&path).is_err() {
            return None;
        }
        let manifest = crate::package_json::read(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        let Some(manifest) = manifest else {
            // Present but unreadable: the nearest metadata says nothing provable.
            return Some(None);
        };
        match self {
            Self::Installed => Some(manifest["version"].as_str().and_then(major)),
            Self::Declared => {
                let range = ["dependencies", "devDependencies", "peerDependencies"]
                    .iter()
                    .find_map(|field| manifest[field]["vitest"].as_str())?;
                Some(major(range))
            }
        }
    }
}

/// The one major a version or range admits: `4.1.12`, `^4.1.0`, `~4.1`, `4.x`, `=4`,
/// `4.0.0-beta.1`. The whole text is read: a set of ranges (`||`, a space, a hyphen range), a
/// comparator, a tag or a protocol (`workspace:`, `npm:`, `catalog:`) is `None`.
fn major(range: &str) -> Option<u64> {
    let range = range.trim();
    let range = range.strip_prefix(['^', '~', '=']).unwrap_or(range);
    let range = range.strip_prefix('v').unwrap_or(range);
    // A prerelease or build suffix follows the version core, and must be identifiers only: a space,
    // `||` or a comparator after it is another range. Before it, the parts of the core reject them.
    let (core, suffix) = range.split_at(range.find(['-', '+']).unwrap_or(range.len()));
    if !suffix.is_empty()
        && (suffix.len() == 1
            || !suffix[1..]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+')))
    {
        return None;
    }
    let mut parts = core.split('.');
    let major = parts.next()?;
    if major.is_empty() || !major.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let rest: Vec<&str> = parts.collect();
    let valid = rest.len() <= 2
        && rest.iter().all(|part| {
            *part == "x"
                || *part == "*"
                || (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        });
    valid.then(|| major.parse().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a range that admits one major has one; everything else is unknown.
    #[test]
    fn a_range_has_a_major_only_when_it_admits_one() {
        for (range, expected) in [
            ("4.1.12", Some(4)),
            ("^4.1.0", Some(4)),
            ("~4.1", Some(4)),
            ("4.x", Some(4)),
            ("=4.0.0", Some(4)),
            ("v4.0.0", Some(4)),
            ("4.0.0-beta.1", Some(4)),
            ("4.1.0+build.7", Some(4)),
            ("^4.0.0-beta.1 || ^5.0.0", None),
            ("4.0.0-beta.1||5", None),
            ("4.0.0 - 5.0.0", None),
            ("^4.0.0 ^5.0.0", None),
            ("4.0.0-", None),
            ("4.0.0-beta!", None),
            ("5.0.5", Some(5)),
            (">=4", None),
            ("4 || 5", None),
            ("*", None),
            ("latest", None),
            ("workspace:^4.0.0", None),
            ("npm:vitest@4.1.12", None),
            ("catalog:", None),
            ("4.1.2.3", None),
            ("", None),
        ] {
            assert_eq!(major(range), expected, "{range:?}");
        }
    }

    /// An installed package decides over any declaration; without one, the nearest declaration
    /// does; outside the root, or unreadable, nothing is known.
    #[test]
    fn installed_wins_then_the_nearest_declaration() {
        let root = tempfile::tempdir().unwrap();
        let write = |relative: &str, text: &str| {
            let path = root.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("package.json", r#"{"devDependencies":{"vitest":"^5.0.0"}}"#);
        write(
            "a/package.json",
            r#"{"devDependencies":{"vitest":"^4.1.0"}}"#,
        );
        write("a/x.bench.ts", "");
        write("b/x.bench.ts", "");
        let mut majors = Majors::new(root.path());
        assert_eq!(majors.of(&root.path().join("a/x.bench.ts")), Some(4));
        assert_eq!(majors.of(&root.path().join("b/x.bench.ts")), Some(5));
        assert!(majors.exports_bench(&root.path().join("a/x.bench.ts")));
        assert!(!majors.exports_bench(&root.path().join("b/x.bench.ts")));

        write("node_modules/vitest/package.json", r#"{"version":"5.0.5"}"#);
        let mut majors = Majors::new(root.path());
        assert_eq!(majors.of(&root.path().join("a/x.bench.ts")), Some(5));
        write("node_modules/vitest/package.json", "not json");
        let mut majors = Majors::new(root.path());
        assert_eq!(majors.of(&root.path().join("a/x.bench.ts")), None);

        let mut outside = Majors::new(&root.path().join("a"));
        assert_eq!(outside.of(&root.path().join("b/x.bench.ts")), None);
    }
}
