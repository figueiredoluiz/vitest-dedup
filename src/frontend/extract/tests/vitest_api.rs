use super::*;

/// Analyze owned files with extraction and eligibility asserted before judging final findings.
fn analyze_sources(
    directory: &Path,
    sources: &[(&str, String)],
    globals: Globals,
    expected_tests: usize,
    unresolved: usize,
) -> Vec<crate::model::Finding> {
    let mut tests = Vec::new();
    for (name, source) in sources {
        let file = SourceFile::without_project_context(
            directory.join(name),
            crate::discovery::SourceLanguage::TypeScript,
            ModuleFlavor::Ambiguous,
        );
        tests.extend(extract(source, &file, globals).tests);
    }
    assert_eq!(
        tests
            .iter()
            .filter(|test| test.kind != DeclarationKind::Suite)
            .count(),
        expected_tests
    );
    assert_eq!(
        tests
            .iter()
            .filter(|test| test.kind != DeclarationKind::Suite && !test.context.is_comparable())
            .count(),
        unresolved
    );
    let config =
        crate::config::Config::load(directory, crate::config::ConfigOverrides::default()).unwrap();
    crate::analysis::analyze(
        tests,
        &config,
        sources.len(),
        crate::model::IncompleteCauses::default(),
    )
    .unwrap()
    .findings
}

/// The runtime oracle runs these exact files: direct mocks retain a positive, aliases whose
/// timing is unproved refuse comparison, and ambient effects never merge with the plain file.
#[test]
fn vitest_api_owned_outcomes() {
    for (name, files, unresolved, pair) in [
        ("named", 3, 0, Some(["alias.test.ts", "vi.test.ts"])),
        ("renamed", 4, 2, None),
        ("namespace-dynamic", 3, 1, None),
        ("vitest-environment", 2, 0, None),
        ("insource", 2, 0, None),
        ("vi-positive", 3, 0, Some(["alias.test.ts", "vi.test.ts"])),
        (
            "shadowed-environment",
            2,
            0,
            Some(["alias.test.ts", "plain.test.ts"]),
        ),
    ] {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/recall")
            .join(format!("vitest-api-{name}"));
        let owned: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".test.ts"))
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read_to_string(entry.path()).unwrap(),
                )
            })
            .collect();
        let sources: Vec<_> = owned
            .iter()
            .map(|(name, source)| (name.as_str(), source.clone()))
            .collect();
        let findings = analyze_sources(&directory, &sources, Globals::Disabled, files, unresolved);
        assert_cross_file_pair(&findings, pair.as_ref().map(|pair| pair.as_slice()));
    }
}

/// Renamed imports, namespace members, configured globals and immutable in-source bindings
/// carry ambient state, while identical setup remains comparable.
#[test]
fn vitest_api_ambient_binding_matrix() {
    for (binding, receiver, globals) in [
        (
            "import { vitest } from 'vitest';",
            "vitest",
            Globals::Disabled,
        ),
        (
            "import { vitest as tools } from 'vitest';",
            "tools",
            Globals::Disabled,
        ),
        (
            "import { vi as tools } from 'vitest';",
            "tools",
            Globals::Disabled,
        ),
        (
            "import * as framework from 'vitest';",
            "framework.vitest",
            Globals::Disabled,
        ),
        ("", "vitest", Globals::Enabled),
        (
            "const { vitest: tools } = import.meta.vitest;",
            "tools",
            Globals::Disabled,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let body =
            "test('reads environment', () => expect(process.env.U12_API_OWNED).toBe('mocked'));";
        let plain = format!("import {{ test, expect }} from 'vitest'; {body}");
        let with_setup = format!("import {{ test, expect }} from 'vitest'; {binding}\n{receiver}.stubEnv('U12_API_OWNED', 'mocked');\n{body}");
        let findings = analyze_sources(
            directory.path(),
            &[
                ("a.test.ts", with_setup.clone()),
                ("b.test.ts", with_setup),
                ("plain.test.ts", plain),
            ],
            globals,
            3,
            0,
        );
        assert_cross_file_pair(&findings, Some(&["a.test.ts", "b.test.ts"]));
    }
}

/// Canonical API identity is not proof of the raw syntax Vitest's hoisting transform sees.
#[test]
fn vitest_api_mock_timing_matrix() {
    for (binding, receiver) in [
        ("import { vi as tools } from 'vitest';", "tools"),
        ("import { vitest as tools } from 'vitest';", "tools"),
        ("import * as framework from 'vitest';", "framework.vi"),
        ("import * as framework from 'vitest';", "framework.vitest"),
        ("const { vitest: tools } = import.meta.vitest;", "tools"),
    ] {
        for method in ["mock", "unmock", "doMock", "doUnmock"] {
            let source = format!("import {{ test, expect }} from 'vitest'; {binding} {receiver}.{method}('dep', () => ({{value: 1}})); test('a', () => expect(1).toBe(1)); test('b', () => expect(1).toBe(1));");
            assert_refused_mock(source);
        }
    }
}

/// Only proven framework reset methods are idempotent; local names cannot license a hook finding.
#[test]
fn vitest_api_reset_binding_matrix() {
    for (binding, receiver, globals, redundant) in [
        ("import { vitest } from 'vitest';", "vitest", Globals::Disabled, true),
        ("import { vitest as tools } from 'vitest';", "tools", Globals::Disabled, true),
        ("import * as framework from 'vitest';", "framework.vitest", Globals::Disabled, true),
        ("", "vitest", Globals::Enabled, true),
        ("const { vitest: tools } = import.meta.vitest;", "tools", Globals::Disabled, true),
        ("const vitest = {clearAllMocks() { counter++ }}; let counter = 0;", "vitest", Globals::Disabled, false),
        ("let { vitest: tools } = import.meta.vitest; tools = {clearAllMocks() { counter++ }}; let counter = 0;", "tools", Globals::Disabled, false),
    ] {
        let source = format!("import {{ test, expect, describe, beforeEach }} from 'vitest'; {binding}\ndescribe('outer', () => {{ beforeEach(() => {{ {receiver}.clearAllMocks(); }}); describe('inner', () => {{ beforeEach(() => {{ {receiver}.clearAllMocks(); }}); test('reads state', () => expect(1).toBe(1)); }}); }});");
        let directory = tempfile::tempdir().unwrap();
        let findings = analyze_sources(directory.path(), &[("a.test.ts", source)], globals, 1, 0);
        assert_eq!(findings.iter().filter(|finding| finding.rule == crate::model::Rule::RedundantHook).count(), usize::from(redundant), "{binding}: {findings:?}");
    }
}

/// Parentheses, comments and optional/computed calls do not license hoisted mock context.
#[test]
fn vitest_api_unproved_hoisting_syntax() {
    for statement in [
        "vitest['mock']('dep', () => ({}));",
        "vitest?.mock('dep', () => ({}));",
        "vitest.mock?.('dep', () => ({}));",
        "(vitest).mock('dep', () => ({}));",
        "vitest /* comment */ .mock('dep', () => ({}));",
        "vitest.mock /* comment */ ('dep', () => ({}));",
    ] {
        let source = format!("import {{ test, expect, vitest }} from 'vitest'; {statement} test('a', () => expect(1).toBe(1)); test('b', () => expect(1).toBe(1));");
        assert_refused_mock(source);
    }
}

/// Framework assertion chains also carry setup; alias support must not erase existing effects.
#[test]
fn vitest_api_assertion_setup_retention() {
    let directory = tempfile::tempdir().unwrap();
    let plain = "import {test, expect} from 'vitest'; test('reads', () => expect(1).toBe(1));";
    let setup = format!("expect(1).toBe(1); {plain}");
    let findings = analyze_sources(
        directory.path(),
        &[
            ("a.test.ts", setup.clone()),
            ("b.test.ts", setup),
            ("plain.test.ts", plain.to_owned()),
        ],
        Globals::Disabled,
        3,
        0,
    );
    assert_cross_file_pair(&findings, Some(&["a.test.ts", "b.test.ts"]));
}

/// Match the complete cross-file participant set, so a retained positive cannot hide a false pair.
fn assert_cross_file_pair(findings: &[crate::model::Finding], expected: Option<&[&str]>) {
    let duplicates: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == crate::model::Rule::CrossFileDuplicate)
        .collect();
    assert_eq!(
        duplicates.len(),
        usize::from(expected.is_some()),
        "{findings:?}"
    );
    if let Some(expected) = expected {
        let mut names: Vec<_> = duplicates[0]
            .locations
            .iter()
            .map(|location| {
                location
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(names, expected, "{findings:?}");
    }
}

/// Unproved timing must leave both extracted tests visibly ineligible and emit no duplicate.
fn assert_refused_mock(source: String) {
    let directory = tempfile::tempdir().unwrap();
    let findings = analyze_sources(
        directory.path(),
        &[("a.test.ts", source)],
        Globals::Disabled,
        2,
        2,
    );
    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != crate::model::Rule::DuplicateTest),
        "{findings:?}"
    );
}
