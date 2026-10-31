// Runtime oracle for the behavior trace's await/.then splice: the claims the canonicalization
// rests on, measured against the pinned Vitest rather than assumed from reading.
//
// The collection oracle compares what Vitest collects; it cannot see an outcome. The claims this
// gate checks are outcome claims — a returned chain's continuation is the test's own behavior, an
// unreturned chain is not awaited, and a rejection handler observes a path no `await` reaches —
// so `fixtures/behavior-runtime/cases.test.ts` runs under the real Vitest and each named test's
// status must equal the one `expected.json` pins.
//
// Floors (a gate that measured nothing fails): every case in `expected.json` matched a result,
// the run carried at least one pass and one fail, and every reported case is expected — a Vitest
// that silently reported nothing, or a fixture whose expectations were left behind, would
// otherwise read as agreement.
//
// Usage: `node scripts/check/check-behavior-runtime.mjs`
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";

import { runAsGate } from "./lib/gate-main.mjs";

// The gate reads its fixture and the pinned Vitest from the checkout root.
const ROOT = resolve(import.meta.dirname, "..", "..");
const DIR = "fixtures/behavior-runtime";
const require = createRequire(join(ROOT, "package.json"));
const { version: vitestVersion } = require("vitest/package.json");

/**
 * Runs the fixture under the pinned Vitest and returns what the reporter wrote, with the spawn's
 * exit and stderr for a run that wrote no report. Split from [`runCli`] so the suite can drive
 * the judgments without spawning anything.
 *
 * @returns {{ report: object | undefined, status: number | undefined, stderr: string }}
 */
function runVitest() {
  const work = mkdtempSync(join(tmpdir(), "vitest-dedup-behavior-runtime-"));
  try {
    const result = spawnSync(
      process.execPath,
      [
        join(ROOT, "node_modules", "vitest", "vitest.mjs"),
        "run",
        "--reporter=json",
        "--outputFile",
        join(work, "out.json"),
        resolve(ROOT, DIR, "cases.test.ts"),
      ],
      { cwd: ROOT, encoding: "utf8", maxBuffer: 64 << 20, timeout: 300_000 },
    );
    const report = readReport(join(work, "out.json"));
    return { report, status: result.status, stderr: result.stderr ?? "" };
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

/** The reporter's parsed output, or `undefined` when the run wrote none. */
function readReport(path) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    return undefined;
  }
}

/**
 * The gate's judgments, as a status and the lines a reader sees. `expected` and `run` are
 * injected by the suite; the default reads the fixture's expectations and runs the real Vitest.
 *
 * @returns {{ status: number, lines: string[] }}
 */
export function runCli({ expected = readExpected(), run = runVitest } = {}) {
  const { report, status, stderr } = run();
  // Checked whatever the exit was: the expected run itself exits 1 (two cases must fail), and a
  // crash that wrote no report must name Vitest's stderr, not surface as a missing-file error.
  if (report === undefined) {
    return {
      status: 1,
      lines: [`vitest run produced no report (exit ${status}): ${stderr}`.slice(-2000)],
    };
  }
  const outcomes = new Map();
  for (const file of report.testResults ?? []) {
    for (const assertion of file.assertionResults ?? []) {
      outcomes.set(assertion.fullName ?? assertion.title, assertion.status);
    }
  }
  const lines = [`vitest ${vitestVersion}, node ${process.version}`];
  const failures = [];
  let passes = 0;
  let fails = 0;
  for (const [name, wanted] of Object.entries(expected.cases)) {
    const got = outcomes.get(name);
    if (got === undefined) {
      failures.push(`${name}: no result; vitest reported ${outcomes.size} cases`);
      continue;
    }
    const normalized = got === "passed" ? "pass" : got === "failed" ? "fail" : got;
    if (normalized !== wanted) {
      failures.push(`${name}: expected ${wanted}, vitest reported ${got}`);
    }
    lines.push(`  ${got}: ${name}`);
    if (normalized === "pass") passes += 1;
    if (normalized === "fail") fails += 1;
  }
  const extra = [...outcomes.keys()].filter((name) => !(name in expected.cases));
  for (const name of extra) {
    failures.push(
      `${name}: reported but not in expected.json; the fixture and its expectations move together`,
    );
  }
  if (passes === 0 || fails === 0) {
    failures.push(
      `the run measured ${passes} pass(es) and ${fails} fail(s); a run that measured nothing proves nothing`,
    );
  }
  lines.push(
    failures.length === 0
      ? `ok: ${outcomes.size} runtime case(s) measured under vitest ${vitestVersion}.`
      : `FAIL: ${failures.length} runtime case(s) disagree.`,
  );
  return { status: failures.length === 0 ? 0 : 1, lines: [...lines, ...failures] };
}

/** The fixture's pinned outcomes. */
function readExpected() {
  return JSON.parse(readFileSync(join(ROOT, DIR, "expected.json"), "utf8"));
}

runAsGate(import.meta.url, () => runCli(), "behavior trace runtime claims");
