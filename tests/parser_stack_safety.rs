//! Stack-safety refusals must preserve findings from other files and remain operationally visible.
use assert_cmd::Command;
use serde_json::Value;
use std::time::Duration;

const POSITIVE: &str = "import {test,expect} from 'vitest';\n\
    test('a',()=>{expect(Math.abs(-1)).toBe(1)});\n\
    test('b',()=>{expect(Math.abs(-1)).toBe(1)});";

/// Build adversarial syntax cases that must exceed the parser safety budgets.
fn hostile_sources() -> Vec<(&'static str, String)> {
    let deep = format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000));
    vec![
        ("regex-comment", format!("const re=/a\\/*/;const x={deep};")),
        ("regex-template", format!("const re=/`/;const x={deep};")),
        ("regex-quote", format!("const re=/'/;const x={deep};")),
        ("unary", format!("const x={}1;", "!".repeat(200_000))),
        (
            "escaped-keyword",
            format!("const x={}0;", r"\u0076oid ".repeat(20_000)),
        ),
        ("control", format!("{}{{}}", "if(a)".repeat(200_000))),
        ("member", format!("x{};", ".a".repeat(20_000))),
        ("call", format!("f{};", "()".repeat(20_000))),
        (
            "label",
            format!(
                "{}{{}}",
                (0..20_000).map(|i| format!("l{i}:")).collect::<String>()
            ),
        ),
        ("arrow", format!("const f={}1;", "x=>".repeat(20_000))),
        (
            "jsx",
            format!(
                "const element={}text{};",
                "<a>".repeat(20_000),
                "</a>".repeat(20_000)
            ),
        ),
        ("type", format!("type T={}X;", "keyof ".repeat(20_000))),
    ]
}

/// Require an incomplete two-file report retaining both healthy tests and their duplicate finding.
fn assert_preserved_report(report: &Value, name: &str) {
    assert_eq!(report["testsAnalyzed"], 2, "{name}");
    assert_eq!(report["filesAnalyzed"], 2, "{name}");
    assert_eq!(report["incomplete"], true, "{name}");
    let findings = report["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{name}");
    assert_eq!(findings[0]["rule"], "duplicate-test", "{name}");
    assert_eq!(
        findings[0]["locations"].as_array().unwrap().len(),
        2,
        "{name}"
    );
}

/// Verify each hostile input exits with an operational error while preserving healthy-file findings.
#[test]
fn hostile_syntax_is_refused_without_aborting_or_losing_other_findings() {
    for (name, source) in hostile_sources() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("positive.test.ts"), POSITIVE).unwrap();
        std::fs::write(
            dir.path().join(if name == "jsx" {
                "hostile.test.tsx"
            } else {
                "hostile.test.ts"
            }),
            source,
        )
        .unwrap();
        let prefix = dir.path().join("report");
        let output = Command::cargo_bin("vitest-dedup")
            .unwrap()
            .timeout(Duration::from_secs(15))
            .arg(dir.path())
            .args(["--reporters", "json", "--output"])
            .arg(&prefix)
            .arg("--fail-on-incomplete=reads")
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value =
            serde_json::from_slice(&std::fs::read(prefix.with_extension("json")).unwrap()).unwrap();
        assert_preserved_report(&report, name);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("raw syntax budget"),
            "{name}"
        );
    }
}

/// Verify a refused imported helper leaves the report incomplete and retains healthy-file findings.
#[test]
fn imported_hostile_syntax_is_refused_without_aborting_or_losing_other_findings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("positive.test.ts"), POSITIVE).unwrap();
    std::fs::write(
        dir.path().join("caller.test.ts"),
        "import {register} from './helper';register();",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("helper.ts"),
        format!(
            "const re=/a\\/*/;const x={}1{};export function register(){{}}",
            "(".repeat(100_000),
            ")".repeat(100_000)
        ),
    )
    .unwrap();
    let prefix = dir.path().join("report");
    let output = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .timeout(Duration::from_secs(15))
        .arg(dir.path())
        .args(["--reporters", "json", "--output"])
        .arg(&prefix)
        .arg("--fail-on-incomplete=reads")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value =
        serde_json::from_slice(&std::fs::read(prefix.with_extension("json")).unwrap()).unwrap();
    assert_preserved_report(&report, "helper");
    assert!(String::from_utf8_lossy(&output.stderr).contains("helper"));
}

/// Keep bounded ambiguous syntax and wide literal data analyzable without findings or incompleteness.
#[test]
fn bounded_ambiguous_syntax_and_wide_literal_payload_remain_analyzable() {
    let dir = tempfile::tempdir().unwrap();
    let source = format!(
        "import {{test,expect}} from 'vitest';\n\
        const re=/a\\/*/; const quote=/'/; const tick=/`/; const ratio=2 / 1;\n\
        const x={}1{};\n\
        const text='{}'; /* {} */\n\
        const data=[{}];\n\
        test('bounded',()=>{{expect(data.length).toBe(20000)}});",
        "(".repeat(1_000),
        ")".repeat(1_000),
        "(".repeat(300),
        ")".repeat(300),
        vec!["1"; 20_000].join(",")
    );
    // This file intentionally includes regexes that defeat lexical masking, but remains below
    // the independent raw budget and runs on the explicit worker stack.
    std::fs::write(dir.path().join("bounded.test.ts"), source).unwrap();
    let prefix = dir.path().join("report");
    let output = Command::cargo_bin("vitest-dedup")
        .unwrap()
        .timeout(Duration::from_secs(15))
        .arg(dir.path())
        .args(["--reporters", "json", "--output"])
        .arg(&prefix)
        .arg("--fail-on-incomplete=reads")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value =
        serde_json::from_slice(&std::fs::read(prefix.with_extension("json")).unwrap()).unwrap();
    assert_eq!(report["testsAnalyzed"], 1);
    assert_eq!(report["filesAnalyzed"], 1);
    assert_eq!(report["incomplete"], false);
    assert_eq!(report["findings"].as_array().unwrap().len(), 0);
}

/// Require current and historical extraction to refuse excessive syntax without claiming a proven-empty file.
#[test]
fn current_and_revision_extraction_share_the_same_refusal_contract() {
    use vitest_dedup::discovery::{ModuleFlavor, SourceFile, SourceLanguage};
    use vitest_dedup::frontend::{extract, extract_from_revision, Globals};
    let file = SourceFile::without_project_context(
        std::path::PathBuf::from("hostile.test.ts"),
        SourceLanguage::TypeScript,
        ModuleFlavor::ForcedEsm,
    );
    let source = format!("const x={}1;", "!".repeat(200_000));
    for extraction in [
        extract(&source, &file, Globals::Disabled),
        extract_from_revision(&source, &file, Globals::Disabled),
    ] {
        assert_eq!(extraction.tests.len(), 0);
        assert!(extraction.incomplete);
        assert!(!extraction.proven_empty);
        assert!(extraction
            .diagnostics
            .iter()
            .any(|reason| reason.contains("raw syntax budget")));
    }
}

/// Exercise admitted deep syntax on the worker stack without aborting, losing the test or reporting findings.
#[test]
fn admitted_bracket_free_chains_use_the_bounded_worker_stack() {
    let cases = [
        ("member", format!("external{}();", ".member".repeat(8_000))),
        (
            "masked-parentheses",
            format!(
                "const re=/a\\/*/;const x={}1{};",
                "(".repeat(8_000),
                ")".repeat(8_000)
            ),
        ),
        ("unary", format!("const x={}1;", "void ".repeat(8_000))),
        ("control", format!("{}{{}}", "if(a)".repeat(4_000))),
        (
            "label",
            format!(
                "{}{{}}",
                (0..8_000).map(|i| format!("l{i}:")).collect::<String>()
            ),
        ),
        ("arrow", format!("const f={}1;", "x=>".repeat(4_000))),
        (
            "jsx",
            format!(
                "const x={}text{};",
                "<a>".repeat(1_500),
                "</a>".repeat(1_500)
            ),
        ),
        ("tagged", format!("const x=tag{};", "`text`".repeat(4_000))),
        ("type", format!("type T={}X;", "keyof ".repeat(8_000))),
    ];
    for (name, body) in cases {
        let dir = tempfile::tempdir().unwrap();
        let source = format!(
            "import {{test,expect}} from 'vitest';{body}\n\
             test('retained',()=>{{expect(1).toBe(1)}});"
        );
        let path = if name == "jsx" {
            "chain.test.tsx"
        } else {
            "chain.test.ts"
        };
        std::fs::write(dir.path().join(path), source).unwrap();
        let prefix = dir.path().join("report");
        let output = Command::cargo_bin("vitest-dedup")
            .unwrap()
            .timeout(Duration::from_secs(15))
            .arg(dir.path())
            .args(["--reporters", "json", "--output"])
            .arg(&prefix)
            .arg("--fail-on-incomplete=reads")
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value =
            serde_json::from_slice(&std::fs::read(prefix.with_extension("json")).unwrap()).unwrap();
        assert_eq!(report["testsAnalyzed"], 1, "{name}");
        assert_eq!(report["incomplete"], false, "{name}");
        assert_eq!(report["findings"].as_array().unwrap().len(), 0, "{name}");
    }
}
