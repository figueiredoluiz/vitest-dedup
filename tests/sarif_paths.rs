//! SARIF URI encoding through discovery, analysis and every structured file reporter.

#[path = "support/sarif_uri.rs"]
mod sarif_uri;

const SOURCE: &str = include_str!("../fixtures/duplicates/sarif-path-locations.test.ts");

/// Each filename retains the owned duplicate finding and both participants; only SARIF encodes it.
#[test]
fn sarif_paths_preserve_owned_findings_and_other_reporter_paths() {
    let cases = [
        ("src/plain.test.ts", "src/plain.test.ts"),
        (
            "src/[id]/a b#c%41.test.ts",
            "src/%5Bid%5D/a%20b%23c%2541.test.ts",
        ),
        ("src/a%41.test.ts", "src/a%2541.test.ts"),
        ("src/aA.test.ts", "src/aA.test.ts"),
        (
            "src/日本語 café.test.ts",
            "src/%E6%97%A5%E6%9C%AC%E8%AA%9E%20caf%C3%A9.test.ts",
        ),
        (
            "src/a+b&c=d,e;f$g.test.ts",
            "src/a%2Bb%26c%3Dd%2Ce%3Bf%24g.test.ts",
        ),
    ];
    for (path, expected_uri) in cases {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, SOURCE).unwrap();
        let output = assert_cmd::Command::cargo_bin("vitest-dedup")
            .unwrap()
            .current_dir(dir.path())
            .arg(".")
            .args(["--reporters", "jsonl,json,html,sarif", "--output", "report"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{path}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let read = |extension: &str| {
            std::fs::read_to_string(dir.path().join(format!("report.{extension}"))).unwrap()
        };
        let json: serde_json::Value = serde_json::from_str(&read("json")).unwrap();
        assert_eq!(json["testsAnalyzed"], 2, "{path}");
        assert_eq!(json["findings"].as_array().unwrap().len(), 1, "{path}");
        let finding = &json["findings"][0];
        assert_eq!(finding["rule"], "duplicate-test");
        assert_eq!(finding["verdict"], "redundant");

        let stdout = String::from_utf8(output.stdout).unwrap();
        let records: Vec<serde_json::Value> = stdout
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["type"], "finding");
        assert_eq!(records[1]["type"], "summary");
        let html = read("html");
        let island = html
            .split_once(r#"<script type="application/json" id="report-data">"#)
            .unwrap()
            .1
            .split_once("</script>")
            .unwrap()
            .0;
        let html: serde_json::Value = serde_json::from_str(island).unwrap();
        let sarif: serde_json::Value = serde_json::from_str(&read("sarif")).unwrap();
        let results = sarif["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        let result = &results[0];
        assert_eq!(result["ruleId"], finding["rule"]);
        assert_eq!(
            result["partialFingerprints"][vitest_dedup::model::FINGERPRINT_VERSION],
            finding["fingerprint"]
        );
        assert_eq!(result["locations"].as_array().unwrap().len(), 1);
        assert_eq!(result["relatedLocations"].as_array().unwrap().len(), 1);
        assert_eq!(finding["locations"].as_array().unwrap().len(), 2);
        for (index, location) in [&result["locations"][0], &result["relatedLocations"][0]]
            .into_iter()
            .enumerate()
        {
            let physical = &location["physicalLocation"];
            let uri = physical["artifactLocation"]["uri"].as_str().unwrap();
            assert_eq!(uri, expected_uri);
            assert_eq!(sarif_uri::decode_path(uri), path);
            assert_eq!(
                std::fs::read_to_string(dir.path().join(sarif_uri::decode_path(uri))).unwrap(),
                SOURCE
            );
            for plain in [
                &finding["locations"][index],
                &records[0]["locations"][index],
                &html["findings"][0]["locations"][index],
            ] {
                assert_eq!(plain["path"], path);
            }
            let plain = &finding["locations"][index];
            for (sarif_key, json_key) in [
                ("startLine", "line"),
                ("startColumn", "column"),
                ("endLine", "endLine"),
                ("endColumn", "endColumn"),
            ] {
                assert_eq!(physical["region"][sarif_key], plain[json_key]);
            }
            assert_eq!(location["message"]["text"], finding["titles"][index]);
        }
    }
}
