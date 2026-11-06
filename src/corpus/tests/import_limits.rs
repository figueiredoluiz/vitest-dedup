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
}

/// Erased imports consume no work and preserve same-file comparison eligibility.
#[test]
fn erased_deep_import_keeps_duplicate_control() {
    let corpus =
        root_variant(|source| source.replace("import './m0'", "import type { value } from './m0'"));
    assert_eq!(corpus.tests.len(), 2);
    assert!(corpus.context_refusals.is_empty());
    assert!(corpus.tests.iter().all(|test| test.exclusion.is_none()));
    assert_duplicate("deep.test.ts", corpus, ["visible first", "visible second"]);
}
