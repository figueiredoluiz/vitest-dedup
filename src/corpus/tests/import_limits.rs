use super::*;

/// Read the permanent collection-oracle input, including its hidden registrations.
fn sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/recall/import-depth-cut");
    fs::read_dir(root)
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().unwrap().is_file())
        .map(|entry| {
            (
                entry.file_name().into_string().unwrap(),
                fs::read_to_string(entry.path()).unwrap(),
            )
        })
        .collect()
}

/// Read the owned dependency graph with one deliberate edit to its importing test file.
fn root_variant(edit: impl FnOnce(&str) -> String) -> FileCorpus {
    let mut sources = sources();
    let (_, source) = sources
        .iter_mut()
        .find(|(path, _)| path == "deep.test.ts")
        .unwrap();
    *source = edit(source);
    let files: Vec<_> = sources
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();
    corpora(&files, &["deep.test.ts"]).remove(0)
}

/// Assert the one retained duplicate after the caller has counted and checked its cases.
fn assert_duplicate(path: &str, corpus: FileCorpus, titles: [&str; 2]) {
    let result = owned_findings(path, corpus);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].rule, crate::model::Rule::DuplicateTest);
    assert_eq!(result.findings[0].titles, titles);
}

/// A limited root stays refused across cache reuse; independent duplicate findings survive.
#[test]
fn owned_import_cut_refuses_comparisons_and_retains_control() {
    let sources = sources();
    let files: Vec<_> = sources
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_str()))
        .collect();
    for order in [
        ["deep.test.ts", "control.test.ts", "deep.test.ts"],
        ["control.test.ts", "deep.test.ts", "deep.test.ts"],
    ] {
        let corpora = corpora(&files, &order);
        for (path, corpus) in order.into_iter().zip(&corpora) {
            assert_eq!(corpus.tests.len(), 2);
            if path == "deep.test.ts" {
                assert_eq!(corpus.context_refusals, [Exclusion::ImportLimit]);
                let limit = corpus.import_limit.as_ref().unwrap();
                assert_eq!(limit.kind, crate::inventory::ImportLimitKind::Depth);
                assert_eq!(limit.module, Path::new("m32.ts"));
                assert_eq!((limit.depth, limit.analyses), (33, 32));
                assert!(corpus
                    .tests
                    .iter()
                    .all(|test| test.exclusion == Some(Exclusion::ImportLimit)));
                assert!(owned_findings(path, corpus.clone()).findings.is_empty());
            } else {
                assert!(corpus.context_refusals.is_empty());
                assert!(corpus.tests.iter().all(|test| test.exclusion.is_none()));
                assert_duplicate(
                    path,
                    corpus.clone(),
                    ["independent first", "independent second"],
                );
            }
        }
    }
}

/// The missing import closure is reported even without a visible local registration.
#[test]
fn import_cut_without_direct_tests_keeps_context_refusal() {
    let corpus = root_variant(|_| "import './m0';".to_owned());
    assert!(corpus.tests.is_empty());
    assert_eq!(corpus.context_refusals, [Exclusion::ImportLimit]);
    assert!(corpus.import_limit.is_some());
}

/// An ordinary runner call cannot restore comparisons after the caller hits its import bound.
#[test]
fn import_cut_refuses_runner_expansion() {
    let mut sources = sources();
    let (_, source) = sources
        .iter_mut()
        .find(|(path, _)| path == "deep.test.ts")
        .unwrap();
    source.push_str("\nimport { run } from './runner'\nrun()\n");
    sources.push((
        "runner.ts".to_owned(),
        "import {test} from 'vitest';export function run(){test('expanded',()=>{})}".to_owned(),
    ));
    let files: Vec<_> = sources
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect();
    let corpus = corpora(&files, &["deep.test.ts"]).remove(0);
    assert_eq!(corpus.context_refusals, [Exclusion::ImportLimit]);
    assert_eq!(corpus.tests.len(), 2, "{:?}", corpus.tests);
    assert!(corpus
        .tests
        .iter()
        .all(|test| test.exclusion == Some(Exclusion::ImportLimit)));
}

/// The engine prefers bounded-work metadata but retains the stable fallback for synthetic corpora.
#[test]
fn import_cut_report_uses_specific_detail_and_keeps_the_fallback() {
    let report = |import_limit| {
        let mut engine = Engine::default();
        let (entry, partial) = engine.record(
            Path::new("deep.test.ts"),
            Ok(Ok(FileCorpus {
                context_refusals: vec![Exclusion::ImportLimit],
                import_limit,
                ..FileCorpus::default()
            })),
            false,
            Vec::new(),
            false,
            None,
        );
        assert!(partial);
        entry.unwrap().detail
    };

    let detail = report(Some(crate::inventory::ImportLimit {
        kind: crate::inventory::ImportLimitKind::Depth,
        module: Path::new("m32.ts").to_path_buf(),
        depth: 33,
        analyses: 32,
        bytes: 942,
    }));
    assert_eq!(
        detail,
        "import analysis reached its depth limit at m32.ts (depth 33, 32 analyses, 942 source bytes), so imported effects and registrations are unknown"
    );

    assert_eq!(
        report(None),
        "import analysis reached its depth or aggregate work limit, so imported effects and registrations are unknown"
    );
}

/// Erased imports consume no work and preserve same-file comparison eligibility.
#[test]
fn erased_deep_import_keeps_duplicate_control() {
    let corpus =
        root_variant(|source| source.replace("import './m0'", "import type { value } from './m0'"));
    assert_eq!(corpus.tests.len(), 2);
    assert!(corpus.context_refusals.is_empty());
    assert!(corpus.import_limit.is_none());
    assert!(corpus.tests.iter().all(|test| test.exclusion.is_none()));
    assert_duplicate("deep.test.ts", corpus, ["visible first", "visible second"]);
}

/// A wide but shallow helper graph becomes comparable only after its completed proofs are reused.
#[test]
fn bounded_import_retry_recovers_same_file_findings() {
    let mut root = String::new();
    let mut sources = Vec::new();
    for module in 0..65 {
        let path = format!("m{module}.ts");
        root.push_str(&format!("import './m{module}';\n"));
        sources.push((path, format!("export const value{module} = {module};")));
    }
    root.push_str(
        "import { expect, test } from 'vitest';\n\
         test('recovered first', () => { expect(1).toBe(1) });\n\
         test('recovered second', () => { expect(1).toBe(1) });\n",
    );
    sources.push(("wide.test.ts".to_owned(), root));
    let files: Vec<_> = sources
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect();
    let corpus = corpora(&files, &["wide.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 2);
    assert!(corpus.context_refusals.is_empty());
    assert!(corpus.import_limit.is_none());
    assert!(corpus.tests.iter().all(|test| test.exclusion.is_none()));
    assert_duplicate(
        "wide.test.ts",
        corpus,
        ["recovered first", "recovered second"],
    );
}

/// Repeated parser refusals leave an unreadable import without spending the source ceiling again.
#[test]
fn repeated_source_refusal_recovers_owned_findings() {
    let payload = format!(
        "import {{ test }} from 'vitest'; test('hidden registration', () => {{}});\n\
         export const value = '{}{}';",
        ":".repeat(40_000),
        "x".repeat(1_500_000),
    );
    let control = include_str!("../../../fixtures/recall/import-depth-cut/control.test.ts");
    for reexport in [false, true] {
        for distinct in [false, true] {
            let mut sources = Vec::new();
            let mut root = String::new();
            for index in 0..6 {
                let module = if distinct { index } else { 0 };
                if distinct || index == 0 {
                    sources.push((format!("large{module}.ts"), payload.clone()));
                }
                let relay = if reexport {
                    format!("export {{ value }} from './large{module}';")
                } else {
                    format!("import './large{module}';")
                };
                sources.push((format!("relay{index}.ts"), relay));
                root.push_str(&format!("import './relay{index}';\n"));
            }
            root.push_str(control);
            sources.push(("root.test.ts".to_owned(), root));
            let files: Vec<_> = sources
                .iter()
                .map(|(path, source)| (path.as_str(), source.as_str()))
                .collect();
            for corpus in corpora(&files, &["root.test.ts", "root.test.ts"]) {
                assert_eq!(
                    corpus.tests.len(),
                    2,
                    "reexport={reexport}, distinct={distinct}"
                );
                assert!(corpus
                    .context_refusals
                    .contains(&Exclusion::UnreadableImport));
                if distinct {
                    let limit = corpus.import_limit.as_ref().unwrap();
                    assert_eq!(limit.kind, crate::inventory::ImportLimitKind::SourceBytes);
                    assert_eq!(limit.module, Path::new("large5.ts"));
                    assert!(corpus
                        .tests
                        .iter()
                        .all(|test| test.exclusion == Some(Exclusion::ImportLimit)));
                    assert!(owned_findings("root.test.ts", corpus).findings.is_empty());
                } else {
                    assert_eq!(corpus.context_refusals, [Exclusion::UnreadableImport]);
                    assert!(corpus.import_limit.is_none());
                    assert!(corpus.tests.iter().all(|test| test.exclusion.is_none()));
                    assert_duplicate(
                        "root.test.ts",
                        corpus,
                        ["independent first", "independent second"],
                    );
                }
            }
        }
    }
}

/// A repeated ancestor of an unreadable dependency preserves both eligible and distinct tests.
#[test]
fn unreadable_diamond_preserves_comparison_outcomes() {
    let catalog = format!("export const entries = '{}';\n", ":".repeat(40_000));
    let source = concat!(
        "import './left';\n",
        "import './right';\n",
        "import { expect, test } from 'vitest';\n",
        "test('first', () => expect(42).toBe(42));\n",
        "test('second', () => expect(42).toBe(42));\n",
        "test('distinct', () => expect(42).not.toBe(0));\n",
    );
    let files = [
        ("catalog.ts", catalog.as_str()),
        (
            "shared.ts",
            "import './catalog';\nexport const value = 42;\n",
        ),
        ("left.ts", "import './shared';\n"),
        ("right.ts", "export { value } from './shared';\n"),
        ("diamond.test.ts", source),
    ];
    let corpus = corpora(&files, &["diamond.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 3);
    assert_eq!(corpus.context_refusals, [Exclusion::UnreadableImport]);
    assert!(corpus.import_limit.is_none());
    assert!(corpus.tests.iter().all(|test| test.exclusion.is_none()));
    assert_duplicate("diamond.test.ts", corpus, ["first", "second"]);
}

/// The acyclic import graph used by the recall and collection gates.
const DIAMOND: [(&str, &str); 5] = [
    (
        "catalog.ts",
        include_str!("../../../fixtures/recall/import-diamond-recovery/catalog.ts"),
    ),
    (
        "shared.ts",
        include_str!("../../../fixtures/recall/import-diamond-recovery/shared.ts"),
    ),
    (
        "left.ts",
        include_str!("../../../fixtures/recall/import-diamond-recovery/left.ts"),
    ),
    (
        "right.ts",
        include_str!("../../../fixtures/recall/import-diamond-recovery/right.ts"),
    ),
    (
        "diamond.test.ts",
        include_str!("../../../fixtures/recall/import-diamond-recovery/diamond.test.ts"),
    ),
];

/// A dense acyclic import graph recovers a comparison while retaining its diagnostic.
#[test]
fn source_refusal_diamond_recovers_comparisons() {
    for corpus in corpora(&DIAMOND, &["diamond.test.ts", "diamond.test.ts"]) {
        assert_eq!(corpus.tests.len(), 3);
        assert!(corpus
            .context_refusals
            .contains(&Exclusion::UnreadableImport));
        assert!(corpus.import_limit.is_none(), "{:?}", corpus.import_limit);
        assert_eq!(corpus.context_refusals, [Exclusion::UnreadableImport]);
        assert!(corpus.tests.iter().all(|test| test.exclusion.is_none()));
        assert_duplicate("diamond.test.ts", corpus, ["first", "second"]);
    }
}

/// A replay at the depth limit must still refuse its child one level farther down.
#[test]
fn source_refusal_diamond_preserves_the_child_depth_boundary() {
    let mut sources = DIAMOND
        .map(|(path, source)| (path.to_owned(), source.to_owned()))
        .to_vec();
    let (_, root) = sources
        .iter_mut()
        .find(|(path, _)| path == "diamond.test.ts")
        .unwrap();
    *root = root.replace("import './left'\nimport './right'", "import './relay0'");
    for depth in 0..31 {
        let next = if depth == 30 {
            "shared".to_owned()
        } else {
            format!("relay{}", depth + 1)
        };
        sources.push((format!("relay{depth}.ts"), format!("import './{next}';")));
    }
    let files: Vec<_> = sources
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect();
    let corpus = corpora(&files, &["diamond.test.ts"]).remove(0);
    assert_eq!(corpus.tests.len(), 3);
    assert!(corpus
        .context_refusals
        .contains(&Exclusion::UnreadableImport));
    let limit = corpus.import_limit.as_ref().unwrap();
    assert_eq!(limit.kind, crate::inventory::ImportLimitKind::Depth);
    assert_eq!(limit.module, Path::new("catalog.ts"));
    assert_eq!(limit.depth, 33);
    assert!(corpus
        .tests
        .iter()
        .all(|test| test.exclusion == Some(Exclusion::ImportLimit)));
    assert!(owned_findings("diamond.test.ts", corpus)
        .findings
        .is_empty());
}
