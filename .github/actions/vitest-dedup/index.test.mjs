import { test, describe } from "node:test";
import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { createHash } from "node:crypto";

import {
  analyzerEnvironment,
  attestationIdentity,
  buildArguments,
  describeExitCode,
  detectLibc,
  encodeAnnotationData,
  errorAnnotation,
  expectedDigestFromChecksumFile,
  extractOutputs,
  isValidReleaseVersion,
  isValidReportShape,
  releaseTarget,
  resolveJsonReportPath,
  resolveReportPath,
  resolveReporters,
  runAction,
  verifyChecksum,
  verifyAttestation,
  warningAnnotation,
} from "./index.js";

/** Runs `body` with `console.log` captured and returns everything it logged, one line per call. */
function captureStdout(body) {
  const originalLog = console.log;
  const logged = [];
  console.log = (...logArgs) => logged.push(logArgs.join(" "));
  try {
    body();
  } finally {
    console.log = originalLog;
  }
  return logged.join("\n");
}

/**
 * Parses the `name<<delimiter\nvalue\ndelimiter\n` records `setOutput` writes to `GITHUB_OUTPUT`
 * into a plain `{ name: value }` map, so runAction tests can assert on published outputs without
 * depending on write order or exact formatting.
 */
function parseGithubOutput(content) {
  const lines = content.split("\n");
  const outputs = {};
  let i = 0;
  while (i < lines.length) {
    const header = lines[i];
    const match = /^(.+)<<(.+)$/.exec(header ?? "");
    if (!match) {
      i += 1;
      continue;
    }
    const [, name, delimiter] = match;
    const valueLines = [];
    i += 1;
    while (i < lines.length && lines[i] !== delimiter) {
      valueLines.push(lines[i]);
      i += 1;
    }
    outputs[name] = valueLines.join("\n");
    i += 1; // skip the delimiter line
  }
  return outputs;
}

describe("releaseTarget", () => {
  const targets = {
    "darwin-arm64": { platform: "darwin", arch: "arm64", binaryName: "vitest-dedup" },
    "linux-x64-gnu": { platform: "linux", arch: "x64", libc: "gnu", binaryName: "vitest-dedup" },
    "linux-x64-musl": { platform: "linux", arch: "x64", libc: "musl", binaryName: "vitest-dedup" },
    "win32-x64": { platform: "win32", arch: "x64", binaryName: "vitest-dedup.exe" },
  };

  test("resolves a non-linux platform without needing libc", () => {
    const target = releaseTarget("darwin", "arm64", undefined, targets);
    assert.equal(target.key, "darwin-arm64");
  });

  test("resolves linux by libc: gnu", () => {
    const target = releaseTarget("linux", "x64", "gnu", targets);
    assert.equal(target.key, "linux-x64-gnu");
  });

  test("resolves linux by libc: musl", () => {
    const target = releaseTarget("linux", "x64", "musl", targets);
    assert.equal(target.key, "linux-x64-musl");
  });

  test("returns undefined for an unsupported platform/arch, naming nothing to guess with", () => {
    const target = releaseTarget("freebsd", "riscv64", undefined, targets);
    assert.equal(target, undefined);
  });
});

describe("detectLibc", () => {
  test("reports gnu when glibcVersionRuntime is present", () => {
    const report = { getReport: () => ({ header: { glibcVersionRuntime: "2.35" } }) };
    assert.equal(detectLibc(report), "gnu");
  });

  test("reports musl when glibcVersionRuntime is absent", () => {
    const report = { getReport: () => ({ header: {} }) };
    assert.equal(detectLibc(report), "musl");
  });

  test("falls back to musl when the report API throws", () => {
    const report = {
      getReport: () => {
        throw new Error("no report available");
      },
    };
    assert.equal(detectLibc(report), "musl");
  });
});

describe("buildArguments", () => {
  for (const value of [undefined, "", "false", "FALSE", "TRUE", true, "true"]) {
    test(`forwards no-metrics only for exact true string: ${JSON.stringify(value)}`, () => {
      assert.equal(buildArguments({ path: ".", noMetrics: value }).includes("--no-metrics"), value === "true");
    });
  }
  test("uses defaults when every input is empty/absent", () => {
    const args = buildArguments({
      path: "",
      config: "",
      threshold: "",
      reporters: "",
      output: "",
      rule: "",
      globals: "",
      failOnIncomplete: "",
    });
    assert.deepEqual(args, [".", "--reporters", "terminal,json"]);
  });

  test("includes every flag when every input is set", () => {
    const args = buildArguments({
      path: "src",
      config: ".vitest-dedup.json",
      threshold: "5",
      reporters: "terminal",
      output: "reports/out",
      rule: "duplicate-test=warning",
      globals: "true",
      failOnIncomplete: "true",
    });
    assert.deepEqual(args, [
      "src",
      "--config",
      ".vitest-dedup.json",
      "--threshold",
      "5",
      "--reporters",
      "terminal,json",
      "--output",
      "reports/out",
      "--rule",
      "duplicate-test=warning",
      "--globals",
      "--fail-on-incomplete",
    ]);
  });

  test("force-adds json to a terminal-only reporters input without dropping terminal", () => {
    const args = buildArguments({ path: ".", reporters: "terminal" });
    const reportersIndex = args.indexOf("--reporters");
    assert.equal(args[reportersIndex + 1], "terminal,json");
  });

  test("does not duplicate json when the user already asked for it", () => {
    const args = buildArguments({ path: ".", reporters: "json,terminal" });
    const reportersIndex = args.indexOf("--reporters");
    assert.equal(args[reportersIndex + 1], "json,terminal");
  });

  for (const [separator, rule] of [
    ["commas", "duplicate-test=warning,near-duplicate-test=error"],
    ["newlines", "duplicate-test=warning\nnear-duplicate-test=error\n"],
  ]) {
    test(`splits --rule on ${separator} into one flag per entry`, () => {
      const args = buildArguments({ path: ".", rule });
      const ruleOccurrences = args.reduce((acc, value, index) => (value === "--rule" ? [...acc, args[index + 1]] : acc), []);
      assert.deepEqual(ruleOccurrences, ["duplicate-test=warning", "near-duplicate-test=error"]);
    });
  }

  test("omits threshold, config, and output when empty", () => {
    const args = buildArguments({ path: ".", config: "", threshold: "", output: "" });
    assert.ok(!args.includes("--config"));
    assert.ok(!args.includes("--threshold"));
    assert.ok(!args.includes("--output"));
  });

  for (const value of ["false", "", undefined]) {
    test(`does not pass --globals or --fail-on-incomplete for input ${JSON.stringify(value)}`, () => {
      const args = buildArguments({ path: ".", globals: value, failOnIncomplete: value });
      assert.ok(!args.includes("--globals"));
      assert.ok(!args.includes("--fail-on-incomplete"));
    });
  }

  for (const value of ["read", "TRUE", "1"]) {
    test(`rejects unsupported --fail-on-incomplete input ${JSON.stringify(value)}`, () => {
      assert.throws(() => buildArguments({ path: ".", failOnIncomplete: value }), /invalid fail-on-incomplete input/);
    });
  }

  test('passes --globals and --fail-on-incomplete only for the exact string "true"', () => {
    const args = buildArguments({ path: ".", globals: "true", failOnIncomplete: "true" });
    assert.ok(args.includes("--globals"));
    assert.ok(args.includes("--fail-on-incomplete"));
  });

  test("forwards the selective fail-on-incomplete modes as =-values", () => {
    // Copilot review, PR #30 round 5: the CLI's selective modes were unreachable through the
    // Action -- only "true" was forwarded, so "reads"/"limits" were silently dropped.
    for (const mode of ["any", "reads", "limits"]) {
      const args = buildArguments({ path: ".", failOnIncomplete: mode });
      assert.ok(args.includes(`--fail-on-incomplete=${mode}`), `${mode}: ${args}`);
      assert.ok(!args.includes("--fail-on-incomplete"), `${mode}: no bare form`);
    }
  });

  test("forwards the baseline, fail-on-new and changed-since inputs", () => {
    const args = buildArguments({
      path: ".",
      baseline: "vitest-dedup-baseline.json",
      failOnNew: "0",
      changedSince: "origin/main",
    });
    assert.deepEqual(args.slice(args.indexOf("--baseline")), [
      "--baseline",
      "vitest-dedup-baseline.json",
      "--fail-on-new",
      "0",
      "--changed-since",
      "origin/main",
    ]);
  });

  test("omits the baseline, fail-on-new and changed-since flags when their inputs are empty", () => {
    // `--fail-on-new 0` is a meaningful setting and "" is not, so the check has to be on emptiness
    // rather than falsiness -- a `!inputs.failOnNew` test would drop the strictest limit there is.
    const args = buildArguments({ path: ".", baseline: "", failOnNew: "", changedSince: "" });
    assert.ok(!args.includes("--baseline"));
    assert.ok(!args.includes("--fail-on-new"));
    assert.ok(!args.includes("--changed-since"));
  });

  test("forwards the memory budget inputs, and omits them when unset", () => {
    const args = buildArguments({ path: ".", maxMemory: "512", noMemoryCap: "true" });
    assert.deepEqual(args.slice(args.indexOf("--max-memory")), ["--max-memory", "512", "--no-memory-cap"]);
    const unset = buildArguments({ path: ".", maxMemory: "", noMemoryCap: "false" });
    assert.ok(!unset.includes("--max-memory"));
    assert.ok(!unset.includes("--no-memory-cap"));
  });

  test("passes --fail-on-new 0, which is the strictest limit and not an absent one", () => {
    const args = buildArguments({ path: ".", baseline: "b.json", failOnNew: "0" });
    assert.deepEqual(args.slice(args.indexOf("--fail-on-new")), ["--fail-on-new", "0"]);
  });
});

describe("resolveJsonReportPath", () => {
  test("resolves the .json path relative to cwd", () => {
    const path = resolveJsonReportPath("/work/repo", "reports/vitest-dedup");
    assert.equal(path, "/work/repo/reports/vitest-dedup.json");
  });
});

describe("resolveReportPath", () => {
  test("resolves an arbitrary extension relative to cwd", () => {
    const path = resolveReportPath("/work/repo", "reports/vitest-dedup", "html");
    assert.equal(path, "/work/repo/reports/vitest-dedup.html");
  });
});

describe("resolveReporters", () => {
  test("defaults to terminal when empty/absent", () => {
    assert.deepEqual(resolveReporters(""), ["terminal"]);
    assert.deepEqual(resolveReporters(undefined), ["terminal"]);
  });

  test("splits on commas and newlines", () => {
    assert.deepEqual(resolveReporters("json,html"), ["json", "html"]);
    assert.deepEqual(resolveReporters("json\nhtml"), ["json", "html"]);
  });

  test("does not force-add json (that is buildArguments's job, not resolveReporters's)", () => {
    assert.deepEqual(resolveReporters("html"), ["html"]);
  });
});

describe("extractOutputs", () => {
  const realisticReport = {
    schemaVersion: "0.1",
    testsAnalyzed: 49,
    filesAnalyzed: 21,
    incomplete: false,
    findings: [
      { rule: "duplicate-test-name", severity: "warning" },
      { rule: "duplicate-test", severity: "error" },
      { rule: "duplicate-test", severity: "error" },
    ],
  };

  test("counts findings by severity and passes through corpus size", () => {
    const outputs = extractOutputs(realisticReport, 1);
    assert.deepEqual(outputs, {
      "exit-code": "1",
      findings: "3",
      errors: "2",
      warnings: "1",
      "tests-analyzed": "49",
      "files-analyzed": "21",
      incomplete: "false",
      baselined: "0",
      "new-findings": "0",
    });
  });

  test("a report without a modes object still reports 0 baselined / 0 new", () => {
    // Forward/backward compatibility: `modes` was added after the first release, so a report
    // written by an older pinned binary has no such key. Defaulting to 0 keeps the two outputs
    // present and numeric rather than publishing "undefined" into a workflow expression.
    const outputs = extractOutputs({ ...realisticReport }, 1);
    assert.equal(outputs.baselined, "0");
    assert.equal(outputs["new-findings"], "0");
  });

  test("splits the finding total against the baseline without changing the total", () => {
    const outputs = extractOutputs(
      {
        ...realisticReport,
        modes: { baselineApplied: true, baselined: 2, newFindings: 1 },
      },
      1,
    );
    assert.equal(outputs.findings, "3", "a baseline changes what gates, not what was found");
    assert.equal(outputs.errors, "2");
    assert.equal(outputs.baselined, "2");
    assert.equal(outputs["new-findings"], "1");
  });

  test('surfaces incomplete: true as the string "true"', () => {
    const outputs = extractOutputs({ ...realisticReport, incomplete: true }, 0);
    assert.equal(outputs.incomplete, "true");
  });

  test("omits every finding-derived output when the report is missing, rather than reporting zeros", () => {
    // A missing report (the binary failed before writing one, or it was deleted upfront and never
    // replaced -- see runAction) must not be indistinguishable from a genuinely clean run. Zeros
    // would be exactly that; omitting the outputs is the only way to make the failure visible to a
    // consumer that only looks at `errors`/`warnings`/`findings`.
    const outputs = extractOutputs(null, 2);
    assert.deepEqual(outputs, { "exit-code": "2" });
  });
});

describe("isValidReportShape", () => {
  const realisticReport = {
    schemaVersion: "0.1",
    testsAnalyzed: 49,
    filesAnalyzed: 21,
    incomplete: false,
    findings: [{ rule: "duplicate-test", severity: "error" }],
  };

  test("accepts a well-formed report", () => {
    assert.ok(isValidReportShape(realisticReport));
  });

  test("accepts a well-formed report with zero findings (a genuinely clean run)", () => {
    assert.ok(isValidReportShape({ ...realisticReport, findings: [] }));
  });

  // Each of these is a report that would otherwise sail past `existsSync`/`JSON.parse` and get
  // read by `extractOutputs` as a genuine zero-finding run: exactly the "file that says nothing"
  // failure mode this check exists to catch.
  test("rejects an empty object", () => {
    assert.ok(!isValidReportShape({}));
  });

  test("rejects null", () => {
    assert.ok(!isValidReportShape(null));
  });

  test("rejects a report missing schemaVersion", () => {
    const { schemaVersion, ...rest } = realisticReport;
    assert.ok(!isValidReportShape(rest));
  });

  test("rejects a report whose findings is not an array", () => {
    assert.ok(!isValidReportShape({ ...realisticReport, findings: "none" }));
  });

  test("rejects a report whose testsAnalyzed is not a number", () => {
    assert.ok(!isValidReportShape({ ...realisticReport, testsAnalyzed: "49" }));
  });

  test("rejects a report whose filesAnalyzed is not a number", () => {
    assert.ok(!isValidReportShape({ ...realisticReport, filesAnalyzed: null }));
  });
});

describe("describeExitCode", () => {
  test("0 is clean and does not fail the step", () => {
    assert.deepEqual(describeExitCode(0), { failed: false, message: null });
  });

  test("1 fails the step and names it a quality-gate failure", () => {
    const { failed, message } = describeExitCode(1);
    assert.equal(failed, true);
    assert.match(message, /quality gate/i);
  });

  test("2 fails the step and names it an operational error, distinctly from 1", () => {
    const { failed, message } = describeExitCode(2);
    assert.equal(failed, true);
    assert.match(message, /operational error/i);
  });

  test("1 and 2 never produce the same message", () => {
    assert.notEqual(describeExitCode(1).message, describeExitCode(2).message);
  });
});

describe("checksum verification", () => {
  let dir;

  test("expectedDigestFromChecksumFile parses a sha256sum-style line", () => {
    const digest = "a".repeat(64);
    assert.equal(expectedDigestFromChecksumFile(`${digest}  vitest-dedup-1.0.0-linux-x64-gnu.tar.gz\n`), digest);
  });

  test("expectedDigestFromChecksumFile rejects a malformed checksum file", () => {
    assert.throws(() => expectedDigestFromChecksumFile("not a digest"), /sha256 digest/);
  });

  test("verifyChecksum passes when the digest matches", () => {
    dir = mkdtempSync(join(tmpdir(), "vitest-dedup-action-test-"));
    const archive = join(dir, "archive.tar.gz");
    const checksumFile = join(dir, "archive.tar.gz.sha256");
    const content = Buffer.from("release bytes");
    writeFileSync(archive, content);
    const digest = createHash("sha256").update(content).digest("hex");
    writeFileSync(checksumFile, `${digest}  archive.tar.gz\n`);

    assert.doesNotThrow(() => verifyChecksum(archive, checksumFile));
    rmSync(dir, { recursive: true, force: true });
  });

  test("verifyChecksum fails on mismatch and deletes the archive", () => {
    dir = mkdtempSync(join(tmpdir(), "vitest-dedup-action-test-"));
    const archive = join(dir, "archive.tar.gz");
    const checksumFile = join(dir, "archive.tar.gz.sha256");
    writeFileSync(archive, Buffer.from("release bytes"));
    const wrongDigest = "0".repeat(64);
    writeFileSync(checksumFile, `${wrongDigest}  archive.tar.gz\n`);

    assert.throws(() => verifyChecksum(archive, checksumFile), (error) => {
      assert.match(error.message, new RegExp(wrongDigest));
      assert.match(error.message, /[0-9a-f]{64}/); // actual digest is also reported
      return true;
    });

    assert.equal(existsSync(archive), false, "the archive must be deleted on checksum mismatch");
    rmSync(dir, { recursive: true, force: true });
  });
});

describe("annotation encoding", () => {
  // GitHub reads `::error::`/`::warning::` lines out of the job's raw stdout. Both annotations
  // carry text that ultimately traces back to the analyzed repository (paths, messages), which is
  // not necessarily trustworthy. An unencoded newline or `%` in that text must not be readable as
  // workflow-command syntax by the runner.

  test("encodeAnnotationData escapes %, CR, and LF per GitHub's documented scheme", () => {
    assert.equal(encodeAnnotationData("100%"), "100%25");
    assert.equal(encodeAnnotationData("line1\rline2"), "line1%0Dline2");
    assert.equal(encodeAnnotationData("line1\nline2"), "line1%0Aline2");
  });

  test("encodes % before CR/LF so the escape sequences themselves are not double-encoded", () => {
    assert.equal(encodeAnnotationData("100%\n"), "100%25%0A");
  });

  test("errorAnnotation encodes an embedded newline so it cannot forge a second workflow command", () => {
    let out = "";
    errorAnnotation("first line\n::error::forged command", (line) => {
      out = line;
    });
    assert.equal(out, "::error::first line%0A::error::forged command");
    assert.equal(out.split("\n").length, 1, "the emitted line must never actually contain a newline");
  });

  test("warningAnnotation encodes a literal percent sign in the message", () => {
    let out = "";
    warningAnnotation("path coverage dropped to 50%", (line) => {
      out = line;
    });
    assert.equal(out, "::warning::path coverage dropped to 50%25");
  });

  test("errorAnnotation encodes an embedded carriage return", () => {
    let out = "";
    errorAnnotation("a\rb", (line) => {
      out = line;
    });
    assert.equal(out, "::error::a%0Db");
  });
});

describe("isValidReleaseVersion", () => {
  // Must agree exactly with release.yml's `^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$` -- a value
  // this function accepts but that workflow would reject can never correspond to a real
  // published release, so it is guaranteed to 404 rather than fail with a clear error up front.
  test("accepts a bare major.minor.patch", () => {
    assert.ok(isValidReleaseVersion("1.2.3"));
  });

  test("accepts a dot-separated prerelease identifier", () => {
    assert.ok(isValidReleaseVersion("1.2.3-rc.1"));
  });

  test("rejects a build-metadata suffix, which release.yml's pattern does not permit", () => {
    assert.ok(!isValidReleaseVersion("1.2.3+build"));
  });

  test("rejects a hyphen inside the prerelease identifier, which release.yml's pattern does not permit", () => {
    assert.ok(!isValidReleaseVersion("1.2.3-rc-1"));
  });

  test("rejects a non-semver string", () => {
    assert.ok(!isValidReleaseVersion("not-a-version"));
  });
});

describe("runAction", () => {
  // Each case owns its workspace and cleanup; only subprocess/output plumbing is shared.
  /**
   * Create a temporary Action workspace and fake-binary runner with report/output paths;
   * register cleanup with the test.
   */
  function actionFixture(t) {
    const dir = mkdtempSync(join(tmpdir(), "vitest-dedup-action-run-"));
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    const reportsDir = join(dir, "reports");
    mkdirSync(reportsDir, { recursive: true });
    const reportPath = join(reportsDir, "vitest-dedup.json");
    const githubOutputPath = join(dir, "github_output");
    writeFileSync(githubOutputPath, "");
    return {
      dir,
      reportPath,
      run(script, inputs = {}) {
        const binaryPath = join(dir, "fake-binary.mjs");
        writeFileSync(binaryPath, `#!/usr/bin/env node\n${script}\n`);
        chmodSync(binaryPath, 0o755);
        return runAction({
          cwd: dir,
          env: { VITEST_DEDUP_BINARY: binaryPath, GITHUB_OUTPUT: githubOutputPath, ...inputs },
        });
      },
      /** Run without a supplied binary, so the Action takes its install path. */
      runUnpinned(inputs = {}) {
        return runAction({ cwd: dir, env: { GITHUB_OUTPUT: githubOutputPath, ...inputs } });
      },
      outputs: () => parseGithubOutput(readFileSync(githubOutputPath, "utf8")),
    };
  }

  for (const value of ["false", "true"]) {
    test(`reads no-metrics action input ${value} and forwards it to the binary`, async (t) => {
      const fixture = actionFixture(t);
      const argvPath = join(fixture.dir, "argv.json");
      const report = { schemaVersion: "0.1", testsAnalyzed: 1, filesAnalyzed: 1, incomplete: false, findings: [] };
      const script = reportWriter(fixture.reportPath, report,
        `writeFileSync(${JSON.stringify(argvPath)}, JSON.stringify(process.argv.slice(2)));`);
      assert.equal(await fixture.run(script, { "INPUT_NO-METRICS": value }), 0);
      assert.equal(JSON.parse(readFileSync(argvPath, "utf8")).includes("--no-metrics"), value === "true");
    });
  }

  // Keep the payload and optional extra writes visible in each case; this only quotes a script.
  const reportWriter = (reportPath, report, extraScript = "") => [
    'import { writeFileSync } from "node:fs";',
    `writeFileSync(${JSON.stringify(reportPath)}, ${JSON.stringify(JSON.stringify(report))});`,
    extraScript,
    "process.exit(0);",
  ].join("\n");

  test("the analyzer process never receives the token input", async (t) => {
    // The runner exposes every input as INPUT_*; only gh needs the token.
    const previous = process.env.INPUT_TOKEN;
    process.env.INPUT_TOKEN = "workflow-token";
    t.after(() => {
      if (previous === undefined) delete process.env.INPUT_TOKEN;
      else process.env.INPUT_TOKEN = previous;
    });
    const fixture = actionFixture(t);
    const seenPath = join(fixture.dir, "seen.json");
    const report = { schemaVersion: "0.1", testsAnalyzed: 1, filesAnalyzed: 1, incomplete: false, findings: [] };
    const script = reportWriter(fixture.reportPath, report,
      `writeFileSync(${JSON.stringify(seenPath)}, JSON.stringify({ token: process.env.INPUT_TOKEN ?? null, path: Boolean(process.env.PATH) }));`);
    assert.equal(await fixture.run(script), 0);
    assert.deepEqual(JSON.parse(readFileSync(seenPath, "utf8")), { token: null, path: true });
    assert.equal(fixture.outputs().attestation, undefined, "no attestation output for a supplied binary");
  });

  test("a stale report from an earlier run is deleted upfront and never published when the binary fails before writing a fresh one", async (t) => {
    const fixture = actionFixture(t);
    writeFileSync(
      fixture.reportPath,
      JSON.stringify({
        testsAnalyzed: 999,
        filesAnalyzed: 999,
        incomplete: false,
        findings: [{ rule: "duplicate-test", severity: "error" }],
      }),
    );

    // The binary fails operationally before it can write a new report.
    const exitCode = await fixture.run("process.exit(2);");
    assert.equal(exitCode, 2);
    assert.equal(existsSync(fixture.reportPath), false, "the stale report must be deleted before the binary runs");
    const outputs = fixture.outputs();
    assert.equal(outputs["exit-code"], "2");
    assert.equal(outputs.findings, undefined, "no finding counts may be published without a fresh report");
    assert.equal(outputs["json-report"], undefined, "json-report must not point at a report that was not produced");
  });

  test("a missing report with a native exit code of 0 is escalated to an operational failure, not published as clean", async (t) => {
    const fixture = actionFixture(t);
    // A1: the native process says "clean" but writes nothing. It must not pass silently with
    // every finding output omitted, which would look like a genuine zero-finding run.
    const exitCode = await fixture.run("process.exit(0);");
    assert.equal(exitCode, 2, "a native 0 with no report must be surfaced as an operational failure");
    const outputs = fixture.outputs();
    assert.equal(outputs["exit-code"], "2");
    assert.equal(outputs.findings, undefined, "no finding counts may be published without a fresh report");
    assert.equal(outputs["json-report"], undefined);
  });

  test("a parseable but malformed report is escalated to an operational failure, even when the binary exited 0", async (t) => {
    const fixture = actionFixture(t);
    // {} passes existsSync and JSON.parse while still being an untrusted report schema.
    const exitCode = await fixture.run(reportWriter(fixture.reportPath, {}));
    assert.equal(exitCode, 2, "a malformed report must be an operational failure, not a clean run");
    const outputs = fixture.outputs();
    assert.equal(outputs["exit-code"], "2");
    assert.equal(outputs.findings, undefined, "no finding counts may be published from an untrusted report");
    assert.equal(outputs["json-report"], fixture.reportPath, "the path is still published so an operator can inspect the file");
  });

  test("an unparseable report is an operational failure that still publishes exit-code and the report path", async (t) => {
    const fixture = actionFixture(t);
    // Truncated mid-write: the file exists, so existsSync passes, and JSON.parse throws.
    const script = [
      'import { writeFileSync } from "node:fs";',
      `writeFileSync(${JSON.stringify(fixture.reportPath)}, '{"schemaVersion": "0.1", "findings": [');`,
      "process.exit(1);",
    ].join("\n");
    const exitCode = await fixture.run(script);
    assert.equal(exitCode, 2, "an unreadable report must not keep the native gate code");
    const outputs = fixture.outputs();
    assert.equal(outputs["exit-code"], "2");
    assert.equal(outputs.findings, undefined, "no finding counts may be published from an unreadable report");
    assert.equal(outputs["json-report"], fixture.reportPath);
  });

  test("a failed install still publishes exit-code 2 before the error propagates", async (t) => {
    const fixture = actionFixture(t);
    // The version is rejected before any network access.
    await assert.rejects(
      fixture.runUnpinned({ INPUT_VERSION: "1.0.0+build" }),
      /invalid release version/,
    );
    const outputs = fixture.outputs();
    assert.equal(outputs["exit-code"], "2");
    assert.deepEqual(Object.keys(outputs), ["exit-code"], "nothing but the exit code can be stated truthfully");
  });

  test("an unwritable output file does not mask the original failure", async (t) => {
    const fixture = actionFixture(t);
    // A directory cannot be appended to, so the fallback publish itself fails.
    await assert.rejects(
      fixture.runUnpinned({ INPUT_VERSION: "1.0.0+build", GITHUB_OUTPUT: dirname(fixture.reportPath) }),
      /invalid release version/,
    );
  });

  test("a failure before the install, such as an unremovable stale report, still publishes exit-code 2", async (t) => {
    const fixture = actionFixture(t);
    // A non-empty directory at the report path makes the upfront cleanup throw.
    mkdirSync(join(fixture.reportPath, "nested"), { recursive: true });
    await assert.rejects(fixture.run("process.exit(0);"));
    assert.deepEqual(fixture.outputs(), { "exit-code": "2" });
  });

  test("names the expected path and the output input when the html reporter was selected but wrote nothing", async (t) => {
    const fixture = actionFixture(t);
    const outputPrefix = "reports/vitest-dedup";
    const htmlPath = join(fixture.dir, `${outputPrefix}.html`);
    const report = { schemaVersion: "0.1", testsAnalyzed: 1, filesAnalyzed: 1, incomplete: false, findings: [] };
    // Only JSON is written, simulating an HTML reporter failure after JSON succeeded.
    const originalLog = console.log;
    const logged = [];
    console.log = (...logArgs) => logged.push(logArgs.join(" "));
    let exitCode;
    try {
      exitCode = await fixture.run(reportWriter(fixture.reportPath, report), {
        INPUT_REPORTERS: "html",
        INPUT_OUTPUT: outputPrefix,
      });
    } finally {
      console.log = originalLog;
    }

    assert.equal(exitCode, 0);
    const outputs = fixture.outputs();
    assert.equal(outputs["html-report"], undefined);
    const warning = logged.find((line) => line.includes("::warning::") && line.includes("html reporter"));
    assert.ok(warning, "expected an html-report warning annotation");
    assert.ok(warning.includes(htmlPath), "warning must name the exact expected path");
    assert.ok(warning.includes(outputPrefix), "warning must name the output input it was derived from");
  });

  test("publishes the fresh report this run produced, not a stale one left over from before", async (t) => {
    const fixture = actionFixture(t);
    writeFileSync(
      fixture.reportPath,
      JSON.stringify({ testsAnalyzed: 1, filesAnalyzed: 1, incomplete: false, findings: [] }),
    );
    const freshReport = {
      schemaVersion: "0.1",
      testsAnalyzed: 5,
      filesAnalyzed: 2,
      incomplete: false,
      findings: [{ rule: "duplicate-test", severity: "warning" }],
    };
    const exitCode = await fixture.run(reportWriter(fixture.reportPath, freshReport));
    assert.equal(exitCode, 0);
    const outputs = fixture.outputs();
    assert.equal(outputs.findings, "1");
    assert.equal(outputs.warnings, "1");
    assert.equal(outputs["tests-analyzed"], "5");
    assert.equal(outputs["json-report"], fixture.reportPath);
  });

  test("publishes html-report when the html reporter was selected and wrote a file", async (t) => {
    const fixture = actionFixture(t);
    const outputPrefix = "reports/vitest-dedup";
    const htmlPath = join(fixture.dir, `${outputPrefix}.html`);
    const report = { schemaVersion: "0.1", testsAnalyzed: 1, filesAnalyzed: 1, incomplete: false, findings: [] };
    const exitCode = await fixture.run(reportWriter(fixture.reportPath, report,
      `writeFileSync(${JSON.stringify(htmlPath)}, "<html></html>");`), {
      INPUT_REPORTERS: "html",
      INPUT_OUTPUT: outputPrefix,
    });
    assert.equal(exitCode, 0);
    const outputs = fixture.outputs();
    assert.equal(outputs["html-report"], htmlPath);
  });

  test("does not publish html-report when the html reporter was not selected", async (t) => {
    const fixture = actionFixture(t);
    const outputPrefix = "reports/vitest-dedup";
    const report = { schemaVersion: "0.1", testsAnalyzed: 1, filesAnalyzed: 1, incomplete: false, findings: [] };
    const exitCode = await fixture.run(reportWriter(fixture.reportPath, report), { INPUT_OUTPUT: outputPrefix });
    assert.equal(exitCode, 0);
    const outputs = fixture.outputs();
    assert.equal(outputs["html-report"], undefined);
  });
});

describe("verifyAttestation", () => {
  // The digest and the attestation answer different questions. The digest proves the bytes were
  // not corrupted in transit; only the attestation, signed by GitHub's OIDC identity rather than
  // by the release pipeline itself, says anything about where the bytes came from. These tests
  // pin that an unavailable verifier degrades loudly while a FAILED one is always fatal.

  /** Runs a passing verification with `token` and returns the environment given to the verify call. */
  function verifyEnvironment(token) {
    const envs = [];
    const capture = (_cmd, args, options) => {
      if (args[0] !== "--version") envs.push(options.env);
      return { status: 0 };
    };
    verifyAttestation("/tmp/a.tar.gz", "o/r", "0.2.1", capture, token);
    assert.equal(envs.length, 1, "exactly one verify call");
    return envs[0];
  }

  test("reports unavailable, without throwing, when the gh CLI is absent", () => {
    const missing = () => ({ error: new Error("ENOENT"), status: null });
    assert.equal(verifyAttestation("/tmp/a.tar.gz", "o/r", "0.2.1", missing), "unavailable");
  });

  test("reports verified when gh confirms the attestation", () => {
    const ok = () => ({ status: 0 });
    assert.equal(verifyAttestation("/tmp/a.tar.gz", "o/r", "0.2.1", ok), "verified");
  });

  test("throws when gh is present and verification fails", () => {
    // The dangerous case: the archive's digest matched, so verifyChecksum was happy, but the
    // artifact's origin could not be established. Executing it anyway would defeat the point.
    const failing = (_cmd, args) =>
      args[0] === "--version" ? { status: 0 } : { status: 1, stderr: "no attestation found" };
    assert.throws(
      () => verifyAttestation("/tmp/a.tar.gz", "o/r", "0.2.1", failing),
      /attestation failed/,
      "a failed verification must never be downgraded to a warning",
    );
  });

  test("passes the repository through to gh so verification is scoped", () => {
    // Verifying without --repo would accept an attestation from any repository.
    let seen = null;
    const capture = (_cmd, args) => {
      if (args[0] !== "--version") seen = args;
      return { status: 0 };
    };
    verifyAttestation("/tmp/a.tar.gz", "figueiredoluiz/vitest-dedup", "0.2.1", capture);
    assert.deepEqual(seen, [
      "attestation", "verify", "/tmp/a.tar.gz",
      ...attestationIdentity("figueiredoluiz/vitest-dedup", "0.2.1"),
    ]);
  });

  test("gives the token to the verify call as GH_TOKEN", () => {
    // gh refuses API calls without credentials, and a JavaScript action's environment carries no
    // token, so the token input is the only way verification can run on a hosted runner.
    const env = verifyEnvironment("token-value");
    assert.equal(env.GH_TOKEN, "token-value");
    assert.equal(env.PATH, process.env.PATH, "the rest of the environment is kept");
  });

  test("replaces a differently-cased GH_TOKEN instead of adding a second one", (t) => {
    // On Windows `Gh_Token` and `GH_TOKEN` are one variable; keeping both leaves the child's value
    // to chance.
    process.env.Gh_Token = "stale";
    t.after(() => delete process.env.Gh_Token);
    const env = verifyEnvironment("token-value");
    assert.deepEqual(Object.keys(env).filter((name) => name.toUpperCase() === "GH_TOKEN"), ["GH_TOKEN"]);
    assert.equal(env.GH_TOKEN, "token-value");
  });

  test("injects no GH_TOKEN when the token is empty", () => {
    assert.equal(verifyEnvironment(""), process.env);
  });

  test("reports unavailable, without throwing, when gh is not authenticated", () => {
    // Exit 4 is gh's documented "authentication required": the verifier could not run, which the
    // advisory policy tolerates loudly. It is not a verification result.
    const unauthenticated = (_cmd, args) => (args[0] === "--version" ? { status: 0 } : { status: 4 });
    const warnings = captureStdout(() =>
      assert.equal(verifyAttestation("/tmp/a.tar.gz", "o/r", "0.2.1", unauthenticated, ""), "unavailable"),
    );
    assert.match(warnings, /::warning::.*not authenticated/);
  });

  test("still throws on a non-auth failure when a token was given", () => {
    // A bad or insufficient token makes gh exit 1 (HTTP 401/403), not 4: that is a failed
    // verification and must stay fatal.
    const rejected = (_cmd, args) =>
      args[0] === "--version" ? { status: 0 } : { status: 1, stderr: "HTTP 401: Bad credentials" };
    assert.throws(() => verifyAttestation("/tmp/a.tar.gz", "o/r", "0.2.1", rejected, "bad"), /attestation failed/);
  });

  test("throws when gh reports authentication required although a token was given", () => {
    // With a token supplied, exit 4 means it did not authenticate: a failure, not an absent verifier.
    const unauthenticated = (_cmd, args) => (args[0] === "--version" ? { status: 0 } : { status: 4 });
    assert.throws(
      () => verifyAttestation("/tmp/a.tar.gz", "o/r", "0.2.1", unauthenticated, "token-value"),
      /attestation failed/,
    );
  });
});

describe("attestationIdentity", () => {
  // `--repo` alone accepts an attestation from any workflow or ref in the repository; the release
  // identity is the release workflow building the version's tag. Measured on the published
  // releases: all are signed by release.yml; 0.2.0 and later record refs/tags/v<version>, while
  // 0.1.0 records refs/heads/main.
  test("requires the release workflow and the version's tag", () => {
    assert.deepEqual(attestationIdentity("o/r", "0.2.1"), [
      "--repo", "o/r",
      "--signer-workflow", "o/r/.github/workflows/release.yml",
      "--source-ref", "refs/tags/v0.2.1",
    ]);
  });

  test("binds a prerelease to its own tag", () => {
    assert.ok(attestationIdentity("o/r", "1.0.0-rc.1").join(" ").endsWith("--source-ref refs/tags/v1.0.0-rc.1"));
  });

  test("checks 0.1.0, built from main, by signer workflow only", () => {
    assert.deepEqual(attestationIdentity("o/r", "0.1.0"), [
      "--repo", "o/r",
      "--signer-workflow", "o/r/.github/workflows/release.yml",
    ]);
  });
});

describe("analyzerEnvironment", () => {
  test("removes every credential variable and keeps everything else", () => {
    const credentials = {
      INPUT_TOKEN: "a",
      GH_TOKEN: "b",
      GITHUB_TOKEN: "c",
      GH_ENTERPRISE_TOKEN: "d",
      GITHUB_ENTERPRISE_TOKEN: "e",
      ACTIONS_RUNTIME_TOKEN: "f",
      ACTIONS_ID_TOKEN_REQUEST_TOKEN: "g",
    };
    for (const name of Object.keys(credentials)) {
      const env = analyzerEnvironment({ [name]: "secret", INPUT_PATH: ".", PATH: "/bin" });
      assert.deepEqual(env, { INPUT_PATH: ".", PATH: "/bin" }, `${name} must not reach the analyzer`);
    }
    const input = { ...credentials, PATH: "/bin" };
    analyzerEnvironment(input);
    assert.equal(input.GH_TOKEN, "b", "the caller's environment is not mutated");
  });

  test("matches credential names case-insensitively", () => {
    // Windows environment names are case-insensitive, so `Gh_Token` reaches a child as GH_TOKEN.
    const env = analyzerEnvironment({ Gh_Token: "a", actions_runtime_token: "b", Input_Token: "c", Path: "C:\\bin" });
    assert.deepEqual(env, { Path: "C:\\bin" });
  });
});
