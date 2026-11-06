//! Bounded ignore inputs; matching remains the ignore crate's Gitignore implementation.

use ignore::gitignore::{gitconfig_excludes_path, Gitignore, GitignoreBuilder};
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const FILE_BYTES: usize = 1024 * 1024;
const LINE_BYTES: usize = 8192;
const FILE_LINES: usize = 8192;
const FILE_PATTERNS: usize = 4096;
const TOTAL_BYTES: usize = 8 * FILE_BYTES;
const TOTAL_LINES: usize = 32768;
const MAX_ERRORS: usize = 64;

/// One directory's source classes, ordered from highest to lowest precedence.
struct Layer {
    directory: PathBuf,
    rules: [Gitignore; 4],
    repository: bool,
}

/// Walk-local state. Only ancestor matchers remain live; budgets never reset on directory exit.
pub(super) struct Ignores {
    root: PathBuf,
    layers: Vec<Layer>,
    global: Gitignore,
    require_git: bool,
    bytes: usize,
    lines: usize,
    pub errors: Vec<String>,
}

impl Ignores {
    /// Load ancestors before walking; they cannot be protected by a per-entry preflight.
    pub fn new(root: &Path, require_git: bool) -> Self {
        let mut this = Self {
            root: root.to_path_buf(),
            layers: Vec::new(),
            global: Gitignore::empty(),
            require_git,
            bytes: 0,
            lines: 0,
            errors: Vec::new(),
        };
        // This lookup consults user/system configuration, never checked-in repository config.
        // Its result is only a path: the ignore contents still use our bounded reader.
        if let Some(path) = gitconfig_excludes_path() {
            if let Ok(cwd) = std::env::current_dir() {
                this.global = this.load_with_links(&cwd, &path, true);
            }
        }
        let directories: Vec<_> = root.ancestors().collect();
        let git_boundary = if require_git {
            directories.iter().position(|directory| {
                directory.join(".git").exists() || directory.join(".jj").exists()
            })
        } else {
            None
        };
        for (index, directory) in directories.into_iter().enumerate().rev() {
            // Git sources outside this boundary cannot contribute; custom/.ignore still can.
            this.push(
                directory,
                git_boundary.is_none_or(|boundary| index <= boundary),
            );
        }
        this
    }

    /// Bound diagnostics independently from the number of hostile inputs encountered.
    fn error(&mut self, path: &Path, error: impl std::fmt::Display) {
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(format!(
                "cannot safely load ignore input {}: {error}",
                super::relative_walk_path(path, &self.root)
            ));
        }
    }

    /// Read a complete bounded snapshot or refuse it; never compile a truncated rule set.
    fn contents(&mut self, path: &Path) -> io::Result<Option<String>> {
        self.contents_with_links(path, false)
    }

    /// Only the user-configured global source may follow links to bounded regular targets.
    fn contents_with_links(
        &mut self,
        path: &Path,
        follow_links: bool,
    ) -> io::Result<Option<String>> {
        let metadata = match if follow_links {
            fs::metadata(path)
        } else {
            fs::symlink_metadata(path)
        } {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            // The global loader historically treats unstatable/non-file paths as absent.
            Err(_) if follow_links => return Ok(None),
            Err(error) => return Err(error),
        };
        if follow_links && !metadata.is_file() {
            return Ok(None);
        }
        let admit = |metadata: &fs::Metadata| {
            if !metadata.is_file() || metadata.len() > FILE_BYTES as u64 {
                Err(io::Error::other("expected a regular file of at most 1 MiB"))
            } else {
                Ok(())
            }
        };
        admit(&metadata)?;
        if self.bytes >= TOTAL_BYTES || self.lines >= TOTAL_LINES {
            return Err(io::Error::other("aggregate ignore budget exhausted"));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // A trusted global link may change targets after stat, including to a FIFO.
            options
                .custom_flags(libc::O_NONBLOCK | if follow_links { 0 } else { libc::O_NOFOLLOW });
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Repository inputs inspect a swapped-in reparse point instead of following it.
            if !follow_links {
                options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
            }
        }
        let file = options.open(path)?;
        admit(&file.metadata()?)?;
        let limit = FILE_BYTES.min(TOTAL_BYTES - self.bytes);
        let mut bytes = Vec::new();
        file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
        self.bytes += bytes.len();
        if bytes.len() > limit {
            return Err(io::Error::other("ignore byte budget exceeded"));
        }
        let text = String::from_utf8(bytes).map_err(io::Error::other)?;
        let lines = text.lines().count();
        self.lines += lines;
        if lines > FILE_LINES || self.lines > TOTAL_LINES {
            return Err(io::Error::other("ignore line budget exceeded"));
        }
        if text.lines().any(|line| line.len() > LINE_BYTES) {
            return Err(io::Error::other("ignore line exceeds 8 KiB"));
        }
        Ok(Some(text))
    }

    /// Compile owned bytes with the dependency's syntax, BOM handling and negation semantics.
    fn load(&mut self, directory: &Path, path: &Path) -> Gitignore {
        self.load_with_links(directory, path, false)
    }

    /// Compile each source with its link policy while sharing all admission and work limits.
    fn load_with_links(&mut self, directory: &Path, path: &Path, follow_links: bool) -> Gitignore {
        let text = match self.contents_with_links(path, follow_links) {
            Ok(Some(text)) => text,
            Ok(None) => return Gitignore::empty(),
            Err(error) => {
                self.error(path, error);
                return Gitignore::empty();
            }
        };
        let mut builder = GitignoreBuilder::new(directory);
        let patterns = text
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .count();
        if patterns > FILE_PATTERNS {
            self.error(path, "ignore pattern budget exceeded");
            return Gitignore::empty();
        }
        for (index, line) in text.lines().enumerate() {
            let line = if index == 0 {
                line.trim_start_matches('\u{feff}')
            } else {
                line
            };
            if let Err(error) = builder.add_line(Some(path.to_path_buf()), line) {
                self.error(path, error);
                return Gitignore::empty();
            }
        }
        builder.build().unwrap_or_else(|error| {
            self.error(path, error);
            Gitignore::empty()
        })
    }

    /// Resolve repository metadata pointers through the same bounded reader as ignore contents.
    fn git_directory(&mut self, directory: &Path) -> Option<PathBuf> {
        let dotgit = directory.join(".git");
        let metadata = fs::symlink_metadata(&dotgit).ok()?;
        let gitdir = if metadata.is_dir() {
            dotgit
        } else {
            match self.contents(&dotgit) {
                Ok(Some(text)) => {
                    let Some(path) = text.trim().strip_prefix("gitdir: ") else {
                        self.error(&dotgit, "invalid gitdir pointer");
                        return None;
                    };
                    directory.join(path)
                }
                Ok(None) => return None,
                Err(error) => {
                    self.error(&dotgit, error);
                    return None;
                }
            }
        };
        let common = gitdir.join("commondir");
        match self.contents(&common) {
            Ok(Some(text)) => Some(gitdir.join(text.trim())),
            Ok(None) => Some(gitdir),
            Err(error) => {
                self.error(&common, error);
                None
            }
        }
    }

    /// Build one directory layer; separate classes preserve custom/.ignore precedence globally.
    fn push(&mut self, directory: &Path, git_rules: bool) {
        let custom = self.load(
            directory,
            &directory.join(super::VITEST_DEDUP_IGNORE_FILENAME),
        );
        let plain = self.load(directory, &directory.join(".ignore"));
        let git = if git_rules {
            self.load(directory, &directory.join(".gitignore"))
        } else {
            Gitignore::empty()
        };
        let repository = directory.join(".git").exists() || directory.join(".jj").exists();
        let exclude = if git_rules {
            self.git_directory(directory)
                .map(|gitdir| self.load(directory, &gitdir.join("info/exclude")))
                .unwrap_or_else(Gitignore::empty)
        } else {
            Gitignore::empty()
        };
        self.layers.push(Layer {
            directory: directory.to_path_buf(),
            rules: [custom, plain, git, exclude],
            repository,
        });
    }

    /// Match each class nearest-first; Git rules stop at the nearest repository boundary.
    fn decision(&self, path: &Path, is_dir: bool) -> Option<bool> {
        for class in 0..4 {
            for layer in self.layers.iter().rev() {
                let matched = layer.rules[class].matched(path, is_dir);
                if !matched.is_none() {
                    return Some(matched.is_whitelist());
                }
                if class >= 2 && self.require_git && layer.repository {
                    break;
                }
            }
        }
        let matched = self.global.matched(path, is_dir);
        (!matched.is_none()).then(|| matched.is_whitelist())
    }

    /// Apply parent rules before loading a directory's own files; never read a pruned subtree.
    pub fn keep(&mut self, entry: &ignore::DirEntry) -> bool {
        let path = entry.path();
        while self
            .layers
            .last()
            .is_some_and(|layer| !path.starts_with(&layer.directory) || path == layer.directory)
        {
            self.layers.pop();
        }
        let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
        if entry.depth() > 0 {
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            #[cfg(windows)]
            let hidden = {
                use std::os::windows::fs::MetadataExt;
                hidden
                    || entry
                        .metadata()
                        .is_ok_and(|metadata| metadata.file_attributes() & 2 != 0)
            };
            let keep = self.decision(path, is_dir).unwrap_or(!hidden);
            if !keep {
                return false;
            }
        }
        if is_dir {
            self.push(path, true);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run the same public discovery/configuration path for each ignore-input fixture.
    fn discover(root: &Path) -> super::super::DiscoveredFiles {
        super::super::discover(&super::super::tests::load(root), None).unwrap()
    }

    /// Create one visible input independently of the ignore source under test.
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("a.test.ts"), "").unwrap();
        dir
    }

    /// Build the same FIFO fixture for repository and global-source admission controls.
    #[cfg(unix)]
    fn make_fifo(path: &Path) {
        let output = std::process::Command::new("mkfifo")
            .arg(path)
            .output()
            .expect("create FIFO fixture");
        assert!(output.status.success(), "{:?}", output.stderr);
    }

    /// Ordinary matching and refusal both retain an exact, externally visible file set.
    #[test]
    fn bounded_ignore_discovery_retains_files_and_reports_rejections() {
        for name in [
            ".gitignore",
            ".ignore",
            super::super::VITEST_DEDUP_IGNORE_FILENAME,
        ] {
            let dir = fixture();
            let root = dir.path();
            let path = root.join(name);
            for text in [
                "x".repeat(FILE_BYTES + 1),
                "#".repeat(LINE_BYTES + 1),
                "\n".repeat(FILE_LINES + 1),
                "ignored\n".repeat(FILE_PATTERNS + 1),
                "[z-a]\n".to_owned(),
            ] {
                fs::write(&path, text).unwrap();
                let result = discover(root);
                assert_eq!(result.files.len(), 1, "{name}: {:?}", result.errors);
                assert!(result.incomplete && result.traversal_failed, "{name}");
                assert!(result.errors.iter().any(|error| error.contains(name)));
            }
            fs::write(&path, "a.test.ts\n").unwrap();
            let result = discover(root);
            assert!(result.files.is_empty(), "{name}");
            assert!(!result.incomplete, "{:?}", result.errors);
            fs::write(&path, "a.test.ts\n!a.test.ts\n").unwrap();
            let result = discover(root);
            assert_eq!(result.files.len(), 1);
            assert!(!result.incomplete);
        }
    }

    /// Aggregate work and diagnostics cannot grow with every directory's rule payload.
    #[test]
    fn bounded_ignore_aggregate_budget_and_diagnostics() {
        let dir = fixture();
        let root = dir.path();
        let mut ignores = Ignores::new(root, true);
        let path = root.join("rules");
        fs::write(&path, "#comment\n").unwrap();
        ignores.bytes = TOTAL_BYTES - 2;
        assert!(ignores
            .contents(&path)
            .unwrap_err()
            .to_string()
            .contains("byte budget"));
        ignores.bytes = 0;
        ignores.lines = TOTAL_LINES;
        assert!(ignores
            .contents(&path)
            .unwrap_err()
            .to_string()
            .contains("aggregate"));
        ignores.lines = TOTAL_LINES - 1;
        fs::write(&path, "#a\n#b\n").unwrap();
        assert!(ignores.contents(&path).is_err());
        for _ in 0..100 {
            ignores.error(&path, "refused");
        }
        assert_eq!(ignores.errors.len(), MAX_ERRORS);
    }

    /// Root, ancestor and nested symlinks must never be read, even when their target is regular.
    #[test]
    #[cfg(unix)]
    fn bounded_ignore_symlinks_and_fifos_are_never_opened() {
        use std::os::unix::fs::symlink;
        for location in ["", "sub", "parent"] {
            for name in [
                ".gitignore",
                ".ignore",
                super::super::VITEST_DEDUP_IGNORE_FILENAME,
            ] {
                let dir = fixture();
                let base = dir.path();
                let root = if location == "parent" {
                    base.join("project")
                } else {
                    base.to_path_buf()
                };
                fs::create_dir_all(root.join("sub")).unwrap();
                fs::write(root.join("sub/a.test.ts"), "").unwrap();
                let parent = if location == "sub" {
                    root.join("sub")
                } else {
                    base.to_path_buf()
                };
                let path = parent.join(name);
                symlink("/dev/zero", &path).unwrap();
                let result = discover(&root);
                assert!(result.incomplete && result.traversal_failed);
                assert!(result
                    .files
                    .iter()
                    .any(|file| file.path.ends_with("sub/a.test.ts")));
                fs::remove_file(&path).unwrap();
                make_fifo(&path);
                let mut ignores = Ignores::new(&root, true);
                assert!(ignores.contents(&path).is_err());
            }
        }
    }

    /// Ignore rules cannot force reads inside explicitly excluded directories.
    #[test]
    fn bounded_ignore_pruned_directories_are_not_loaded() {
        let dir = fixture();
        let root = dir.path();
        fs::create_dir(root.join("node_modules")).unwrap();
        fs::write(
            root.join("node_modules/.ignore"),
            "x".repeat(FILE_BYTES + 1),
        )
        .unwrap();
        let result = discover(root);
        assert_eq!(result.files.len(), 1);
        assert!(!result.incomplete);
    }

    /// Irrelevant Git ancestors consume no budget; applicable ancestor source classes still do.
    #[test]
    fn bounded_ignore_loading_stops_at_the_repository_boundary() {
        for marker in [".git", ".jj"] {
            let dir = tempfile::tempdir().unwrap();
            let outer = dir.path();
            let root = outer.join("project");
            fs::create_dir_all(outer.join(".git/info")).unwrap();
            fs::create_dir(&root).unwrap();
            if marker == ".git" {
                fs::create_dir(root.join(marker)).unwrap();
            } else {
                fs::write(root.join(marker), "").unwrap();
            }
            fs::write(root.join("a.test.ts"), "").unwrap();
            let baseline = Ignores::new(&root, true);
            for source in [".gitignore", ".git/info/exclude"] {
                let path = outer.join(source);
                fs::write(&path, "# irrelevant\n".repeat(1000)).unwrap();
                let after = Ignores::new(&root, true);
                assert_eq!((after.bytes, after.lines), (baseline.bytes, baseline.lines));
                fs::write(&path, "#".repeat(LINE_BYTES + 1)).unwrap();
                let result = discover(&root);
                assert_eq!(result.files.len(), 1);
                assert!(!result.incomplete, "{:?}", result.errors);
                fs::remove_file(path).unwrap();
            }
            for source in [".ignore", super::super::VITEST_DEDUP_IGNORE_FILENAME] {
                let path = outer.join(source);
                fs::write(&path, "#".repeat(LINE_BYTES + 1)).unwrap();
                let result = discover(&root);
                assert_eq!(result.files.len(), 1);
                assert!(result.incomplete);
                fs::remove_file(path).unwrap();
            }
            if marker == ".git" {
                fs::remove_dir(root.join(marker)).unwrap();
            } else {
                fs::remove_file(root.join(marker)).unwrap();
            }
            fs::write(outer.join(".gitignore"), "blocked.test.ts\n").unwrap();
            fs::write(root.join("blocked.test.ts"), "").unwrap();
            let result = discover(&root);
            assert_eq!(
                result.files.len(),
                1,
                "parent Git rules still apply when it owns the project"
            );
            assert!(!result.incomplete);
        }
    }

    /// Global non-files stay absent; following a regular target never bypasses size admission.
    #[test]
    #[cfg(unix)]
    fn bounded_global_ignore_links_still_require_regular_bounded_targets() {
        use std::os::unix::fs::symlink;
        let dir = fixture();
        let root = dir.path();
        let link = root.join("global-ignore");
        let target = root.join("rules");
        fs::write(&target, "a.test.ts\n").unwrap();
        symlink(&target, &link).unwrap();
        let mut ignores = Ignores::new(root, true);
        assert!(
            ignores.contents(&link).is_err(),
            "repository links stay refused"
        );
        assert_eq!(
            ignores.contents_with_links(&link, true).unwrap().as_deref(),
            Some("a.test.ts\n")
        );
        fs::write(&target, "x".repeat(FILE_BYTES + 1)).unwrap();
        assert!(ignores.contents_with_links(&link, true).is_err());
        fs::remove_file(&target).unwrap();
        symlink("/dev/zero", &target).unwrap();
        assert!(ignores.contents_with_links(&link, true).unwrap().is_none());
        fs::remove_file(&target).unwrap();
        make_fifo(&target);
        assert!(ignores.contents_with_links(&link, true).unwrap().is_none());
        fs::remove_file(&target).unwrap();
        symlink(&link, &target).unwrap();
        assert!(ignores.contents_with_links(&link, true).unwrap().is_none());
    }

    /// Every source, including repository excludes, shares byte and type admission.
    #[test]
    fn bounded_ignore_repository_pointers_and_invalid_inputs() {
        let dir = fixture();
        let root = dir.path();
        fs::create_dir(root.join(".git/info")).unwrap();
        let path = root.join(".git/info/exclude");
        fs::write(&path, "#".repeat(LINE_BYTES + 1)).unwrap();
        let result = discover(root);
        assert_eq!(result.files.len(), 1);
        assert!(result.incomplete);
        fs::write(&path, "a.test.ts\n").unwrap();
        let result = discover(root);
        assert!(result.files.is_empty());
        assert!(!result.incomplete);
        let mut ignores = Ignores::new(root, true);
        fs::write(&path, [0xff]).unwrap();
        assert!(ignores.contents(&path).is_err());
        assert!(ignores.contents(&root.join(".git")).is_err());
        fs::remove_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git"), "invalid pointer").unwrap();
        assert!(Ignores::new(root, true)
            .errors
            .iter()
            .any(|error| error.contains("gitdir")));
        fs::create_dir(root.join("metadata")).unwrap();
        fs::write(root.join(".git"), "gitdir: metadata\n").unwrap();
        fs::write(root.join("metadata/commondir"), ".\n").unwrap();
        fs::create_dir(root.join("metadata/info")).unwrap();
        fs::write(root.join("metadata/info/exclude"), "a.test.ts\n").unwrap();
        let result = discover(root);
        assert!(result.files.is_empty());
        assert!(!result.incomplete, "{:?}", result.errors);
    }
}
