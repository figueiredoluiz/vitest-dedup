use super::*;
use crate::inventory::{
    ImportLimit, ImportLimitKind, MAX_DEPTH, MAX_IMPORT_ANALYSES, MAX_IMPORT_ANALYSIS_PASSES,
    MAX_IMPORT_SOURCE_BYTES,
};
use std::path::PathBuf;

/// Assert the first refusal and every counter that explains it.
fn assert_limit(
    found: &Inventory,
    kind: ImportLimitKind,
    module: &str,
    depth: usize,
    analyses: usize,
    bytes: usize,
) {
    assert!(found.imports_limited);
    let expected = ImportLimit {
        kind,
        module: PathBuf::from(module),
        depth,
        analyses,
        bytes,
    };
    assert_eq!(found.import_limit, Some(expected.clone()));
    let bound = match kind {
        ImportLimitKind::Depth => "depth",
        ImportLimitKind::Analyses => "module-analysis count",
        ImportLimitKind::SourceBytes => "source-byte work",
    };
    assert_eq!(
        expected.detail(),
        format!(
            "import analysis reached its {bound} limit at {module} (depth {depth}, {analyses} analyses, {bytes} source bytes), so imported effects and registrations are unknown"
        )
    );
}

/// Copy the permanent runtime/corpus case into an isolated project.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/recall/import-depth-cut");
    for entry in fs::read_dir(fixture).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
        }
    }
    dir
}

/// Read a root using the same persistent cache that later test files will see.
fn read(dir: &Path, source: &str, modules: &mut Modules) -> Inventory {
    let path = dir.join("root.test.ts");
    fs::write(&path, source).unwrap();
    let parsed = parse(&path, source).unwrap();
    inventory(
        &path,
        source,
        &parsed,
        Setting {
            dialect: &DIALECT,
            globals: false,
            root: dir,
        },
        modules,
    )
}

/// A depth refusal stops duplicate-import amplification without poisoning shallower proofs.
#[test]
fn owned_import_cut_stops_work_and_recovers_from_nearer_root() {
    let dir = project();
    let source = fs::read_to_string(dir.path().join("deep.test.ts")).unwrap();
    let control = fs::read_to_string(dir.path().join("control.test.ts")).unwrap();
    let mut modules = Modules::default();
    for _ in 0..2 {
        let found = read(dir.path(), &source, &mut modules);
        assert_eq!(found.sites.len(), 2);
        assert_limit(
            &found,
            ImportLimitKind::Depth,
            &format!("m{MAX_DEPTH}.ts"),
            MAX_DEPTH + 1,
            MAX_DEPTH,
            modules.work.bytes,
        );
        assert_eq!(modules.work.analyses, MAX_DEPTH);
        assert!(modules.active.is_empty());
        for depth in 0..MAX_DEPTH {
            assert!(modules
                .cache
                .get(&(dir.path().join(format!("m{depth}.ts")), false))
                .is_none());
        }
        let found = read(dir.path(), &control, &mut modules);
        assert_eq!(found.sites.len(), 2);
        assert!(!found.imports_limited);
        assert!(found.import_limit.is_none());
        assert_eq!(modules.work.analyses, 0);
    }
    let near = source.replace("'./m0'", "'./m1'");
    let found = read(dir.path(), &near, &mut modules);
    assert_eq!((found.sites.len(), found.loads.len()), (2, 1));
    assert!(found.import_limit.is_none());
    assert_eq!(modules.work.analyses, MAX_DEPTH);
    // Completed cache entries are evidence even when reused from a farther root.
    let found = read(dir.path(), &source, &mut modules);
    assert_eq!((found.sites.len(), found.loads.len()), (2, 1));
    assert!(found.import_limit.is_none());
    assert_eq!(modules.work.analyses, 1);
}

/// Re-export routes share the same stop, while erased imports do not spend import work.
#[test]
fn import_limits_cover_reexports_but_not_erased_imports() {
    let dir = project();
    let source = fs::read_to_string(dir.path().join("deep.test.ts")).unwrap();
    for prefix in ["export * from", "export { value } from"] {
        for depth in 0..MAX_DEPTH {
            fs::write(
                dir.path().join(format!("m{depth}.ts")),
                format!("{prefix} './m{}';\nimport './m{}';\n", depth + 1, depth + 1),
            )
            .unwrap();
        }
        let leaf = dir.path().join(format!("m{MAX_DEPTH}.ts"));
        fs::write(&leaf, "export const value = 1;").unwrap();
        let mut modules = Modules::default();
        let found = read(dir.path(), &source, &mut modules);
        assert_eq!(found.sites.len(), 2);
        assert_eq!(
            found.import_limit.as_ref().map(|limit| limit.kind),
            Some(ImportLimitKind::Depth)
        );
        assert_eq!(modules.work.analyses, MAX_DEPTH);
    }
    let erased = source.replace("import './m0'", "import type { value } from './m0'");
    let mut modules = Modules::default();
    let found = read(dir.path(), &erased, &mut modules);
    assert_eq!(found.sites.len(), 2);
    assert!(found.import_limit.is_none());
    assert_eq!(modules.work.analyses, 0);
}

/// A flat import root for measuring exact analysis-pass boundaries.
fn fanout(dir: &Path, modules: usize, content: &str) -> String {
    let mut source = String::new();
    for n in 0..modules {
        fs::write(dir.join(format!("m{n}.ts")), content).unwrap();
        source.push_str(&format!("import './m{n}';\n"));
    }
    source.push_str("import { test } from 'vitest'; test('visible', () => {});");
    source
}

/// Count limits admit three progressing passes, reset per root, and preserve completed proofs.
#[test]
fn import_analysis_count_boundary_keeps_cached_proofs() {
    let dir = tempfile::tempdir().unwrap();
    let module = "export const value = 1;";
    let source = fanout(
        dir.path(),
        MAX_IMPORT_ANALYSES * MAX_IMPORT_ANALYSIS_PASSES + 1,
        module,
    );
    let mut modules = Modules::default();
    let exact_pass = source
        .lines()
        .take(MAX_IMPORT_ANALYSES)
        .chain(std::iter::once(
            "import { test } from 'vitest'; test('visible', () => {});",
        ))
        .collect::<Vec<_>>()
        .join("\n");
    let found = read(dir.path(), &exact_pass, &mut modules);
    assert_eq!(found.sites.len(), 1);
    assert!(found.import_limit.is_none());
    assert_eq!(modules.work.analyses, MAX_IMPORT_ANALYSES);

    let second_pass = source
        .lines()
        .take(MAX_IMPORT_ANALYSES + 1)
        .chain(std::iter::once(
            "import { test } from 'vitest'; test('visible', () => {});",
        ))
        .collect::<Vec<_>>()
        .join("\n");
    let found = read(dir.path(), &second_pass, &mut modules);
    assert_eq!(found.sites.len(), 1);
    assert!(found.import_limit.is_none());
    assert_eq!(modules.work.analyses, 1);

    let mut cold = Modules::default();
    let found = read(dir.path(), &second_pass, &mut cold);
    assert_eq!(found.sites.len(), 1);
    assert!(found.import_limit.is_none());
    assert_eq!(cold.work.prior_analyses, MAX_IMPORT_ANALYSES);
    assert_eq!(cold.work.analyses, 1);
    assert!(cold.active.is_empty());

    let exact_total = source
        .lines()
        .take(MAX_IMPORT_ANALYSES * MAX_IMPORT_ANALYSIS_PASSES)
        .chain(std::iter::once(
            "import { test } from 'vitest'; test('visible', () => {});",
        ))
        .collect::<Vec<_>>()
        .join("\n");
    let mut exact = Modules::default();
    let found = read(dir.path(), &exact_total, &mut exact);
    assert!(found.import_limit.is_none());
    assert_eq!(
        exact.work.prior_analyses + exact.work.analyses,
        MAX_IMPORT_ANALYSES * MAX_IMPORT_ANALYSIS_PASSES
    );

    let mut limited = Modules::default();
    let found = read(dir.path(), &source, &mut limited);
    assert_eq!(found.sites.len(), 1);
    assert_limit(
        &found,
        ImportLimitKind::Analyses,
        &format!("m{}.ts", MAX_IMPORT_ANALYSES * MAX_IMPORT_ANALYSIS_PASSES),
        1,
        MAX_IMPORT_ANALYSES * MAX_IMPORT_ANALYSIS_PASSES,
        MAX_IMPORT_ANALYSES * MAX_IMPORT_ANALYSIS_PASSES * module.len(),
    );
    assert_eq!(
        limited.work.prior_analyses,
        MAX_IMPORT_ANALYSES * (MAX_IMPORT_ANALYSIS_PASSES - 1)
    );
    assert_eq!(limited.work.analyses, MAX_IMPORT_ANALYSES);
    assert!(limited.active.is_empty());
    assert!(read(dir.path(), &source, &mut limited)
        .import_limit
        .is_none());
    assert_eq!(limited.work.analyses, 1);
}

/// A pass with any unreadable import cannot use its completed siblings to license a retry.
#[test]
fn import_analysis_retry_requires_only_reusable_progress() {
    let dir = tempfile::tempdir().unwrap();
    let valid = "export const value = 1;";
    let source = fanout(dir.path(), MAX_IMPORT_ANALYSES + 1, valid);
    fs::write(
        dir.path().join(format!("m{}.ts", MAX_IMPORT_ANALYSES - 1)),
        "export {",
    )
    .unwrap();
    let mut modules = Modules::default();
    let found = read(dir.path(), &source, &mut modules);
    assert_limit(
        &found,
        ImportLimitKind::Analyses,
        &format!("m{MAX_IMPORT_ANALYSES}.ts"),
        1,
        MAX_IMPORT_ANALYSES,
        (MAX_IMPORT_ANALYSES - 1) * valid.len() + "export {".len(),
    );
    assert_eq!(modules.work.prior_analyses, 0);
    assert!(!modules.work.retry_safe);
    assert!(crate::inventory::take_unread_imports());
}

/// Source work is admitted at the exact byte boundary and refused before parsing one more byte.
#[test]
fn import_source_byte_boundary_is_aggregate() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.ts", "b.ts"] {
        fs::write(
            dir.path().join(name),
            " ".repeat(MAX_IMPORT_SOURCE_BYTES / 2),
        )
        .unwrap();
    }
    fs::write(dir.path().join("c.ts"), " ").unwrap();
    let source =
        "import './a'; import './b'; import { test } from 'vitest'; test('visible', () => {});";
    let mut modules = Modules::default();
    let found = read(dir.path(), source, &mut modules);
    assert_eq!(found.sites.len(), 1);
    assert!(found.import_limit.is_none());
    assert_eq!(modules.work.bytes, MAX_IMPORT_SOURCE_BYTES);
    let mut cold = Modules::default();
    let found = read(dir.path(), &format!("{source} import './c';"), &mut cold);
    assert_eq!(found.sites.len(), 1);
    assert_limit(
        &found,
        ImportLimitKind::SourceBytes,
        "c.ts",
        1,
        3,
        MAX_IMPORT_SOURCE_BYTES,
    );
    assert_eq!(cold.work.bytes, MAX_IMPORT_SOURCE_BYTES);
    assert_eq!(cold.work.analyses, 3);
    assert!(cold.active.is_empty());
}

/// Retry passes share one source-byte budget rather than multiplying it.
#[test]
fn import_retry_does_not_reset_source_byte_work() {
    let dir = tempfile::tempdir().unwrap();
    let chunk = " ".repeat(MAX_IMPORT_SOURCE_BYTES / MAX_IMPORT_ANALYSES);
    let source = fanout(dir.path(), MAX_IMPORT_ANALYSES + 1, &chunk);
    fs::write(dir.path().join(format!("m{MAX_IMPORT_ANALYSES}.ts")), " ").unwrap();
    let mut modules = Modules::default();
    let found = read(dir.path(), &source, &mut modules);
    assert_limit(
        &found,
        ImportLimitKind::SourceBytes,
        &format!("m{MAX_IMPORT_ANALYSES}.ts"),
        1,
        MAX_IMPORT_ANALYSES + 1,
        MAX_IMPORT_SOURCE_BYTES,
    );
    assert_eq!(modules.work.prior_analyses, MAX_IMPORT_ANALYSES);
    assert_eq!(modules.work.analyses, 1);
}

/// A cycle consumes no additional analysis attempt and does not itself trigger a new refusal.
#[test]
fn import_cycle_retains_existing_within_bound_handling() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.ts"), "import './b';").unwrap();
    fs::write(dir.path().join("b.ts"), "import './a';").unwrap();
    let source = "import './a'; import { test } from 'vitest'; test('visible', () => {});";
    let mut modules = Modules::default();
    let found = read(dir.path(), source, &mut modules);
    assert_eq!(found.sites.len(), 1);
    assert!(found.import_limit.is_none());
    assert_eq!(modules.work.analyses, 2);
    assert!(modules.active.is_empty());
}

/// Local and exported runner readers consume the recorded import refusal before making a plan.
#[test]
fn runner_plans_refuse_an_incomplete_import_inventory() {
    use crate::inventory::runners::{self, Refusal};
    let dir = project();
    fs::write(
        dir.path().join(format!("m{MAX_DEPTH}.ts")),
        "export const value = 1;",
    )
    .unwrap();
    let source = "import './m0'; import { test } from 'vitest'; export function run() { test('held', () => {}); }";
    let path = dir.path().join("root.test.ts");
    let mut modules = Modules::default();
    let found = read(dir.path(), source, &mut modules);
    assert_eq!(found.sites.len(), 1);
    assert_eq!(
        found.import_limit.as_ref().map(|limit| limit.kind),
        Some(ImportLimitKind::Depth)
    );
    let parsed = parse(&path, source).unwrap();
    let callee = parsed
        .tree
        .nodes()
        .find(|&node| {
            parsed.tree.kind(node) == "Identifier" && parsed.tree.text(node, "name") == Some("run")
        })
        .unwrap();
    assert!(matches!(
        runners::local(&parsed, &found, callee),
        Err(Refusal::Limit)
    ));
    let setting = Setting {
        dialect: &DIALECT,
        globals: false,
        root: dir.path(),
    };
    assert!(matches!(
        runners::template(&crate::lang::Sources::default(), &path, "run", setting),
        Err(Refusal::Limit)
    ));
    fs::write(&path, source.replace("'./m0'", "'./m1'")).unwrap();
    let plan = runners::template(&crate::lang::Sources::default(), &path, "run", setting).unwrap();
    assert_eq!(plan.steps.len(), 1);
}
