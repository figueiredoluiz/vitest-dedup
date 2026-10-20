//! Confinement of configuration-derived paths to the analysis root.
//!
//! Paths named by a Vitest or TypeScript configuration (project entries, `extends`, `mergeConfig`
//! imports, a project's `root`/`dir`, tsconfig `paths`) come from the analyzed repository, which
//! is untrusted input. Each one is checked here, after canonicalization, before anything is read
//! through it, so `..` segments, absolute paths and symlinks cannot reach files outside the tree
//! being analyzed.

use std::path::{Component, Path, PathBuf};

/// Where a path lies relative to the analysis root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confinement {
    /// The path exists and its canonical form is under the root. Carries that canonical form.
    Inside(PathBuf),
    /// The path does not exist, but would lie under the root. Carries the best-effort resolved
    /// form: the deepest existing ancestor canonicalized, the remainder normalized lexically.
    Missing(PathBuf),
    /// The path resolves outside the root, whether or not it exists.
    Outside,
}

/// Classifies `path` against `root`. Both are canonicalized when they exist, so a root reached
/// through a symlink, and a path spelled through it, compare equal; comparison is component-wise
/// on the canonical forms, so it does not depend on the platform's verbatim prefix.
pub fn confine(root: &Path, path: &Path) -> Confinement {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if let Ok(canonical) = path.canonicalize() {
        return if canonical.starts_with(&root) {
            Confinement::Inside(canonical)
        } else {
            Confinement::Outside
        };
    }
    let resolved = soft_canonicalize(path);
    if resolved.starts_with(&root) {
        Confinement::Missing(resolved)
    } else {
        Confinement::Outside
    }
}

/// Whether the existing `path` is under `root` after canonicalization.
pub fn contains(root: &Path, path: &Path) -> bool {
    matches!(confine(root, path), Confinement::Inside(_))
}

/// Canonicalizes the longest prefix of `path` that exists, `..` and symlinks included, then
/// appends the remaining components lexically. Only the part that does not exist is folded
/// textually, so a symlinked directory followed by `..` is resolved by the filesystem.
fn soft_canonicalize(path: &Path) -> PathBuf {
    let components: Vec<Component> = path.components().collect();
    for split in (1..=components.len()).rev() {
        let prefix: PathBuf = components[..split].iter().collect();
        if let Ok(canonical) = prefix.canonicalize() {
            let remainder: PathBuf = components[split..].iter().collect();
            return normalize_lexically(&canonical.join(remainder));
        }
    }
    normalize_lexically(path)
}

/// Removes `.` components and folds `name/..` pairs without touching the filesystem. A leading
/// `..` that has nothing to fold against is kept, so a relative escape stays visible.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(component),
            },
            other => out.push(other),
        }
    }
    out.iter().collect()
}

/// Creates a symlink at `link` pointing to `target`, or returns `false` (printing why) where the
/// platform or its permissions refuse, such as a Windows account without the symlink privilege.
/// A caller that needs the link returns early, so the skip is visible in the test output.
#[cfg(test)]
pub(crate) fn try_symlink(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let created = if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
    match created {
        Ok(()) => true,
        Err(error) => {
            eprintln!("skipped: cannot create a symlink here: {error}");
            false
        }
    }
}

/// Runs `body` only if the symlink could be created, so a test that needs one skips visibly
/// rather than failing where the platform refuses.
#[cfg(test)]
pub(crate) fn with_symlink(target: &Path, link: &Path, body: impl FnOnce()) {
    if try_symlink(target, link) {
        body();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Check path confinement: a dotted path that folds back inside stays inside.
    #[test]
    fn dotted_path_that_folds_back_inside_is_inside() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("root");
        fs::create_dir_all(root.join("pkg")).unwrap();
        assert!(matches!(
            confine(&root, &root.join("pkg/../pkg")),
            Confinement::Inside(_)
        ));
        assert!(contains(&root, &root.join("./pkg")));
    }

    /// Check path confinement: parent traversal and absolute paths leave the root, existing or not.
    #[test]
    fn parent_traversal_and_absolute_paths_are_outside() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("root");
        let sibling = directory.path().join("sibling");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        assert_eq!(
            confine(&root, &root.join("../sibling")),
            Confinement::Outside
        );
        assert_eq!(confine(&root, &sibling), Confinement::Outside);
        assert_eq!(
            confine(&root, &root.join("../absent")),
            Confinement::Outside
        );
        assert_eq!(
            confine(&root, &root.join("a/../../absent")),
            Confinement::Outside
        );
        assert_eq!(confine(&root, directory.path()), Confinement::Outside);
    }

    /// Check path confinement: a path that does not exist yet but lies under the root is
    /// `Missing`, not refused.
    #[test]
    fn absent_path_under_the_root_is_missing_not_outside() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("root");
        fs::create_dir_all(&root).unwrap();
        assert!(matches!(
            confine(&root, &root.join("not/yet/../there")),
            Confinement::Missing(_)
        ));
    }

    /// Check path confinement: a symlink resolving outside is refused; one staying inside is not.
    #[test]
    fn symlinks_are_judged_by_their_target() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("root");
        let sibling = directory.path().join("sibling");
        fs::create_dir_all(root.join("real")).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        with_symlink(&sibling, &root.join("escape"), || {
            assert!(try_symlink(&root.join("real"), &root.join("alias")));
            assert_eq!(confine(&root, &root.join("escape")), Confinement::Outside);
            assert!(contains(&root, &root.join("alias")));
        });
    }

    /// Check path confinement: a root reached through a symlink accepts paths spelled either way.
    #[test]
    fn root_reached_through_a_symlink_accepts_both_spellings() {
        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real-root");
        let alias = directory.path().join("alias-root");
        fs::create_dir_all(real.join("pkg")).unwrap();
        with_symlink(&real, &alias, || {
            assert!(contains(&alias, &real.join("pkg")));
            assert!(contains(&real, &alias.join("pkg")));
            assert!(contains(&alias, &alias.join("pkg")));
        });
    }

    /// Check path confinement: a nonexistent path behind a symlinked directory and `..` is judged
    /// where the operating system sends `..`. Unix resolves the symlink first, so `link/../missing`
    /// lands beside the link's target; Windows folds `..` textually before it looks at symlinks, so
    /// the same spelling opens `root/missing`. Either way the judgment matches the path that would be
    /// opened.
    #[test]
    fn dotdot_after_a_symlinked_directory_follows_the_filesystem() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("root");
        let deep = directory.path().join("sibling/deep");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&deep).unwrap();
        with_symlink(&deep, &root.join("link"), || {
            let beside_the_link = confine(&root, &root.join("link/../missing"));
            if cfg!(windows) {
                assert!(matches!(beside_the_link, Confinement::Missing(_)));
            } else {
                assert_eq!(beside_the_link, Confinement::Outside);
            }
            assert!(matches!(
                confine(&root, &root.join("link/../../root/missing")),
                Confinement::Missing(_)
            ));
        });
    }

    /// Check lexical normalization: folds `.` and `name/..`, keeps an unfoldable leading `..`.
    #[test]
    fn lexical_normalization_folds_only_what_it_can() {
        assert_eq!(
            normalize_lexically(Path::new("a/./b/../c")),
            PathBuf::from("a/c")
        );
        assert_eq!(
            normalize_lexically(Path::new("../a")),
            PathBuf::from("../a")
        );
        assert_eq!(
            normalize_lexically(Path::new("a/../../b")),
            PathBuf::from("../b")
        );
    }
}
