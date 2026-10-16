//! CLI baseline output-path contracts.

use vitest_dedup::modes::load_baseline;

/// A bare baseline filename uses the caller's working directory without changing global cwd.
#[test]
fn a_bare_baseline_filename_is_written_in_the_cli_working_directory() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("a.test.ts"),
        "import {test, expect} from 'vitest'; test('a', () => expect(1).toBe(1));\n",
    )
    .unwrap();
    let output = assert_cmd::Command::cargo_bin("vitest-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(directory.path())
        .args(["--baseline", "baseline.json", "--update-baseline"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        load_baseline(&directory.path().join("baseline.json"))
            .unwrap()
            .total(),
        0
    );
}
