// The behavior-runtime gate's judgments, driven without spawning anything: the suite injects the
// expectations and a fake Vitest run, so every rejection is asserted here rather than only by a
// run that happens to hit it. The real spawn is exercised by the gate itself in `npm run check`.

import assert from "node:assert/strict";
import test from "node:test";

import { runCli } from "./check-behavior-runtime.mjs";

const EXPECTED = {
  cases: {
    "returned chain runs as part of the test": "fail",
    "awaited twin": "fail",
    "unreturned chain is not awaited": "pass",
  },
};

/** A fake Vitest report: one entry per named case, spelled as the JSON reporter spells them. */
function report(outcomes) {
  return {
    testResults: [
      {
        assertionResults: outcomes.map(([fullName, status]) => ({ fullName, status })),
      },
    ],
  };
}

/** The run matching `EXPECTED`: two fails, one pass — the shapes the fixture really has. */
const agreeingRun = () => ({
  status: 1,
  stderr: "",
  report: report([
    ["returned chain runs as part of the test", "failed"],
    ["awaited twin", "failed"],
    ["unreturned chain is not awaited", "passed"],
  ]),
});

test("a run that agrees with every pinned outcome passes", () => {
  const result = runCli({ expected: EXPECTED, run: agreeingRun });
  assert.equal(result.status, 0);
  assert.ok(result.lines.at(-1).startsWith("ok: 3 runtime case(s)"));
});

test("a case whose outcome disagrees fails, naming both sides", () => {
  const run = () => {
    const agreed = agreeingRun();
    agreed.report.testResults[0].assertionResults[2].status = "failed";
    return agreed;
  };
  const result = runCli({ expected: EXPECTED, run });
  assert.equal(result.status, 1);
  assert.ok(
    result.lines.some((line) => line.includes("unreturned chain is not awaited: expected pass")),
    result.lines.join("\n"),
  );
});

test("a case vitest did not report fails, and a reported case missing from the expectations fails too", () => {
  const run = () => {
    const agreed = agreeingRun();
    agreed.report.testResults[0].assertionResults.pop();
    agreed.report.testResults[0].assertionResults.push({
      fullName: "case the expectations forgot",
      status: "passed",
    });
    return agreed;
  };
  const result = runCli({ expected: EXPECTED, run });
  assert.equal(result.status, 1);
  assert.ok(
    result.lines.some((line) => line.includes("no result; vitest reported 3 cases")),
    result.lines.join("\n"),
  );
  assert.ok(
    result.lines.some((line) => line.includes("case the expectations forgot: reported but not in")),
    result.lines.join("\n"),
  );
});

test("a run measuring only passes or only fails fails the floor, even when every outcome agrees", () => {
  const onlyPasses = () => ({
    status: 0,
    stderr: "",
    report: report([
      ["returned chain runs as part of the test", "passed"],
      ["awaited twin", "passed"],
      ["unreturned chain is not awaited", "passed"],
    ]),
  });
  const result = runCli({ expected: EXPECTED, run: onlyPasses });
  assert.equal(result.status, 1);
  assert.ok(
    result.lines.some((line) => line.includes("measured 3 pass(es) and 0 fail(s)")),
    result.lines.join("\n"),
  );
});

test("a vitest that wrote no report fails with the spawn's stderr, whatever the exit was", () => {
  const crashed = () => ({ status: 1, stderr: "dyld: Library not loaded", report: undefined });
  const result = runCli({ expected: EXPECTED, run: crashed });
  assert.equal(result.status, 1);
  assert.match(result.lines[0], /produced no report \(exit 1\): dyld: Library not loaded/);
});

test("an outcome the reporter spells some other way is neither pass nor fail, and floors the run", () => {
  const run = () => {
    const agreed = agreeingRun();
    agreed.report.testResults[0].assertionResults[0].status = "skipped";
    return agreed;
  };
  const result = runCli({ expected: EXPECTED, run });
  assert.equal(result.status, 1);
  assert.ok(
    result.lines.some((line) => line.includes("expected fail, vitest reported skipped")),
    result.lines.join("\n"),
  );
  // A skipped case counts toward neither side of the floor, and with one pass and one fail left
  // the floor still holds; the mismatch alone is what fails this run.
  assert.ok(
    !result.lines.some((line) => line.includes("a run that measured nothing")),
    result.lines.join("\n"),
  );
});
