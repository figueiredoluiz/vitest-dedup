//! Names the files and directories a run needed to resolve imports and did not find.
//!
//! The resolver reports why a resolution failed; [`assess`] turns the failures it collected into
//! [`MissingPrerequisite`]s with a path and a fix. It reads the filesystem only to describe the
//! cause (lockfiles, `package.json`, `node_modules`), never to decide that something is missing:
//! a tsconfig is reported only when the resolver itself failed on it, so a project that never
//! needed the file is never told about it.

use crate::frontend::{ResolutionGaps, TsconfigGap};
use crate::model::{MissingPrerequisite, PrerequisiteKind};
use std::path::{Path, PathBuf};

/// The lockfiles that imply an install command, in priority order.
const LOCKFILES: [(&str, &str); 5] = [
    ("pnpm-lock.yaml", "pnpm install"),
    ("bun.lock", "bun install"),
    ("bun.lockb", "bun install"),
    ("yarn.lock", "yarn install"),
    ("package-lock.json", "npm ci"),
];

const NUXT_CONFIGS: [&str; 5] = [
    "nuxt.config.ts",
    "nuxt.config.js",
    "nuxt.config.mjs",
    "nuxt.config.mts",
    "nuxt.config.cjs",
];

const GENERIC_FIX: &str = "generate or restore this file";
const NO_MANAGER_FIX: &str = "install the dependencies";

/// The prerequisites `gaps` shows to be missing, tsconfig entries first, ordered by path.
#[must_use]
pub fn assess(root: &Path, gaps: &ResolutionGaps) -> Vec<MissingPrerequisite> {
    // The nearest manifest at or above the root governs it, as for Node's own lookup.
    let manifest = root
        .ancestors()
        .find_map(|directory| std::fs::read_to_string(directory.join("package.json")).ok())
        .unwrap_or_default();
    let mut missing: Vec<MissingPrerequisite> = gaps
        .tsconfig
        .iter()
        .map(|(gap, count)| {
            let (kind, target) = match gap {
                TsconfigGap::Absent(target) => (PrerequisiteKind::TsconfigExtends, target),
                TsconfigGap::Unreadable(target) => (PrerequisiteKind::TsconfigUnreadable, target),
            };
            let path = display(target, root);
            let fix = match kind {
                PrerequisiteKind::TsconfigExtends if !target.is_absolute() => install_fix(root),
                PrerequisiteKind::TsconfigExtends if generated_by_nuxt(&path, root, &manifest) => {
                    "nuxt prepare".to_string()
                }
                _ => GENERIC_FIX.to_string(),
            };
            MissingPrerequisite {
                kind,
                path,
                fix,
                unresolved_imports: *count,
            }
        })
        .collect();
    missing.sort_by(|a, b| a.path.cmp(&b.path));
    if gaps.packages_not_found > 0 && lists_dependencies(&manifest) && !has_installed_packages(root)
    {
        missing.push(MissingPrerequisite {
            kind: PrerequisiteKind::NodeModules,
            path: "node_modules".to_string(),
            fix: install_fix(root),
            unresolved_imports: gaps.packages_not_found,
        });
    }
    missing
}

/// A path as the reports show it: relative to the root with forward slashes (`..` segments when
/// it lies outside, so no machine-specific prefix is printed), control characters replaced, since
/// it is repository-controlled text that reaches the terminal.
fn display(path: &Path, root: &Path) -> String {
    let relative = if path.is_absolute() {
        let shared = path
            .components()
            .zip(root.components())
            .take_while(|(a, b)| a == b)
            .count();
        if shared == 0 {
            path.to_path_buf()
        } else {
            let ups = root.components().count() - shared;
            let mut out = PathBuf::new();
            (0..ups).for_each(|_| out.push(".."));
            out.extend(path.components().skip(shared));
            out
        }
    } else {
        path.to_path_buf()
    };
    relative
        .to_string_lossy()
        .replace('\\', "/")
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// Whether the missing file sits in `.nuxt` and the project is recognizably a Nuxt one.
fn generated_by_nuxt(path: &str, root: &Path, manifest: &str) -> bool {
    path.split('/').any(|part| part == ".nuxt")
        && (NUXT_CONFIGS.iter().any(|name| root.join(name).is_file())
            || serde_json::from_str::<serde_json::Value>(manifest)
                .ok()
                .is_some_and(|value| dependency_names(&value).any(|name| name == "nuxt")))
}

/// Names listed under the dependency tables of a parsed `package.json`.
fn dependency_names(manifest: &serde_json::Value) -> impl Iterator<Item = &str> {
    ["dependencies", "devDependencies", "optionalDependencies"]
        .into_iter()
        .filter_map(|field| manifest.get(field)?.as_object())
        .flat_map(|table| table.keys().map(String::as_str))
}

/// Whether `package.json` lists at least one dependency to install.
fn lists_dependencies(manifest: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(manifest)
        .ok()
        .is_some_and(|value| dependency_names(&value).next().is_some())
}

/// Whether `node_modules` holds anything in the root or an ancestor (Node resolves upward).
fn has_installed_packages(root: &Path) -> bool {
    root.ancestors().any(|directory| {
        std::fs::read_dir(directory.join("node_modules"))
            .is_ok_and(|mut entries| entries.next().is_some())
    })
}

/// The install command the nearest lockfile implies, searching the root and its ancestors up to
/// the repository boundary.
fn install_fix(root: &Path) -> String {
    for directory in root.ancestors() {
        if let Some((_, command)) = LOCKFILES
            .iter()
            .find(|(name, _)| directory.join(name).is_file())
        {
            return (*command).to_string();
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    NO_MANAGER_FIX.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::IncompleteCauses;
    use std::fs;

    /// Builds the gaps the resolver would have recorded.
    fn gaps(tsconfig: Vec<(TsconfigGap, usize)>, packages: usize) -> ResolutionGaps {
        ResolutionGaps {
            tsconfig: tsconfig.into_iter().collect(),
            packages_not_found: packages,
        }
    }

    /// No recorded failure reports nothing, whatever the project lists.
    #[test]
    fn nothing_recorded_means_nothing_reported() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("package.json"),
            r#"{"dependencies":{"a":"1"}}"#,
        )
        .unwrap();
        assert!(assess(directory.path(), &ResolutionGaps::default()).is_empty());
    }

    /// `bun.lockb` implies `bun install`, and the count passes through.
    #[test]
    fn bun_binary_lockfile_implies_bun() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("package.json"), r#"{"dependencies":{"a":"1"}}"#).unwrap();
        fs::write(root.join("bun.lockb"), "").unwrap();
        let missing = assess(root, &gaps(vec![], 3));
        assert_eq!(missing[0].fix, "bun install");
        assert_eq!(missing[0].unresolved_imports, 3);
    }

    /// With several lockfiles the priority order decides.
    #[test]
    fn pnpm_wins_over_other_lockfiles() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("package.json"), r#"{"dependencies":{"a":"1"}}"#).unwrap();
        fs::write(root.join("package-lock.json"), "").unwrap();
        fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(assess(root, &gaps(vec![], 1))[0].fix, "pnpm install");
    }

    /// A lockfile above the repository root is not this project's.
    #[test]
    fn lockfile_search_stops_at_the_repository_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let outer = directory.path();
        fs::write(outer.join("yarn.lock"), "").unwrap();
        let root = outer.join("inner");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("package.json"), r#"{"dependencies":{"a":"1"}}"#).unwrap();
        assert_eq!(assess(&root, &gaps(vec![], 1))[0].fix, NO_MANAGER_FIX);
    }

    /// `nuxt prepare` needs a `.nuxt` path component and Nuxt evidence in the project.
    #[test]
    fn nuxt_prepare_needs_both_a_nuxt_path_and_nuxt_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"nuxt":"3"}}"#,
        )
        .unwrap();
        let nuxt = |relative: &str| {
            assess(
                root,
                &gaps(vec![(TsconfigGap::Absent(root.join(relative)), 1)], 0),
            )
            .remove(0)
        };
        assert_eq!(nuxt(".nuxt/tsconfig.json").fix, "nuxt prepare");
        assert_eq!(nuxt("a/.nuxt/tsconfig.app.json").fix, "nuxt prepare");
        assert_eq!(nuxt("base.json").fix, GENERIC_FIX);
        assert_eq!(nuxt("nuxt/tsconfig.json").fix, GENERIC_FIX);
        fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"vue":"3"}}"#,
        )
        .unwrap();
        assert_eq!(nuxt(".nuxt/tsconfig.json").fix, GENERIC_FIX);
    }

    /// A package `extends` target is fixed by installing, not by generating.
    #[test]
    fn package_extends_target_names_the_install_command() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("yarn.lock"), "").unwrap();
        let missing = assess(
            root,
            &gaps(
                vec![(TsconfigGap::Absent("@scope/tsconfig/base.json".into()), 4)],
                0,
            ),
        );
        assert_eq!(missing[0].path, "@scope/tsconfig/base.json");
        assert_eq!(missing[0].fix, "yarn install");
        assert_eq!(missing[0].kind, PrerequisiteKind::TsconfigExtends);
    }

    /// An unreadable target keeps its kind and control characters are replaced.
    #[test]
    fn unreadable_target_keeps_its_kind_and_a_hostile_path_is_made_printable() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let missing = assess(
            root,
            &gaps(
                vec![(TsconfigGap::Unreadable(root.join("a\u{1b}[2Jb.json")), 1)],
                0,
            ),
        );
        assert_eq!(missing[0].kind, PrerequisiteKind::TsconfigUnreadable);
        assert_eq!(missing[0].path, "a?[2Jb.json");
    }

    /// All three conditions are required for a `node_modules` entry.
    #[test]
    fn node_modules_needs_listed_dependencies_a_failed_package_and_no_install() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("package.json"), r#"{"name":"lib"}"#).unwrap();
        assert!(
            assess(root, &gaps(vec![], 5)).is_empty(),
            "no dependencies listed"
        );
        fs::write(root.join("package.json"), r#"{"dependencies":{"a":"1"}}"#).unwrap();
        assert!(
            assess(root, &gaps(vec![], 0)).is_empty(),
            "no package import failed"
        );
        assert_eq!(assess(root, &gaps(vec![], 1)).len(), 1);
        fs::create_dir_all(root.join("node_modules/a")).unwrap();
        assert!(assess(root, &gaps(vec![], 1)).is_empty(), "installed");
    }

    /// A prerequisite makes the run incomplete as a `reads` cause, not a `limits` one.
    #[test]
    fn an_empty_cause_set_is_complete_and_a_prerequisite_is_a_read_failure() {
        let mut causes = IncompleteCauses::default();
        assert!(!causes.any() && causes.reasons().is_empty());
        causes.missing_prerequisites = assess(
            Path::new("/nonexistent-root"),
            &gaps(
                vec![(TsconfigGap::Absent("/nonexistent-root/x.json".into()), 2)],
                0,
            ),
        );
        assert!(causes.any() && causes.failed_reads() && !causes.hit_limits());
        assert_eq!(causes.reasons().len(), 1);
    }

    /// A target outside the root is shown with `..` segments, never an absolute prefix.
    #[test]
    fn a_target_outside_the_root_is_shown_relative() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("app");
        let missing = assess(
            &root,
            &gaps(
                vec![(
                    TsconfigGap::Absent(directory.path().join("shared/base.json")),
                    1,
                )],
                0,
            ),
        );
        assert_eq!(missing[0].path, "../shared/base.json");
    }
}
