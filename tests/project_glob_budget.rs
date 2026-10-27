//! Project discovery limits remain visible while owned finding controls retain their meaning.

/// Run each permanent corpus case through the actual CLI, checking eligibility before findings.
#[test]
fn project_glob_corpus_retains_findings_and_reports_refusal() {
    for (name, incomplete) in [
        ("duplicate-test-project-glob-bounded", true),
        ("duplicate-test-project-glob-retention", false),
    ] {
        let report_dir = tempfile::tempdir().unwrap();
        let prefix = report_dir.path().join("report");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/recall")
            .join(name);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("pkg")).unwrap();
        for path in ["pkg/scope.test.ts", "pkg/vitest.config.ts"] {
            std::fs::copy(fixture.join(path), root.join(path)).unwrap();
        }
        std::fs::copy(
            fixture.join("project-config.txt"),
            root.join("vitest.config.ts"),
        )
        .unwrap();
        let projects = vitest_dedup::vitest_projects::resolve_projects(root);
        assert_eq!(projects.projects.len(), usize::from(!incomplete), "{name}");
        assert_eq!(projects.truncated, incomplete, "{name}");
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .arg(root)
            .args(["--reporters", "json", "--output"])
            .arg(&prefix)
            .args(["--fail-on-incomplete", "--threshold", "100"])
            .output()
            .unwrap();
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(prefix.with_extension("json")).unwrap())
                .unwrap();
        assert_eq!(report["filesAnalyzed"], 1, "{name}");
        assert_eq!(report["testsAnalyzed"], 3, "{name}");
        assert_eq!(report["unresolvedTests"], 0, "{name}");
        let findings = report["findings"].as_array().unwrap();
        assert_eq!(findings.len(), 1, "{name}: {findings:?}");
        assert_eq!(findings[0]["rule"], "duplicate-test");
        assert_eq!(
            report["incompleteCauses"]["projectConfigDegraded"], incomplete,
            "{name}"
        );
        assert_eq!(report["incomplete"], incomplete, "{name}");
        assert_eq!(
            output.status.success(),
            !incomplete,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).contains("glob compilation budget"),
            incomplete
        );
    }
}
