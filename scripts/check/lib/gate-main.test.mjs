// Tests for the shared gate entry point. Extracting it removed 21 duplicated lines across two
// gates, and it carries three decisions that a gate would otherwise each get right or wrong on its
// own: a throw is a failure, failures go to stderr, and an imported module does nothing at all.

import assert from "node:assert/strict";
import { mkdtempSync, symlinkSync, writeFileSync } from "node:fs";
import { realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

import { runAsGate } from "./gate-main.mjs";

const SCRIPT = "/repo/scripts/check/some-gate.mjs";
const URL_OF_SCRIPT = pathToFileURL(SCRIPT).href;

/** Collects everything `runAsGate` would have sent to a stream or to `process.exit`. */
function harness() {
  const out = [];
  const err = [];
  const exits = [];
  return {
    out,
    err,
    exits,
    io: {
      log: (line) => out.push(line),
      error: (line) => err.push(line),
      exit: (code) => exits.push(code),
    },
  };
}

test("a module that was merely imported does nothing and reports that it did nothing", () => {
  const h = harness();
  const ran = runAsGate(pathToFileURL("/repo/other.mjs").href, () => {
    throw new Error("runCli must not be called");
  }, "some", { argv: ["node", SCRIPT], ...h.io });
  assert.equal(ran, false);
  assert.deepEqual([h.out, h.err, h.exits], [[], [], []]);
});

test("an argv with no script path does nothing, rather than throwing on pathToFileURL", () => {
  const h = harness();
  const ran = runAsGate(URL_OF_SCRIPT, () => {
    throw new Error("runCli must not be called");
  }, "some", { argv: ["node"], ...h.io });
  assert.equal(ran, false);
  assert.deepEqual(h.exits, []);
});

test("a passing gate prints to stdout and exits 0", () => {
  const h = harness();
  const ran = runAsGate(URL_OF_SCRIPT, () => ({ status: 0, lines: ["first", "second"] }), "some", {
    argv: ["node", SCRIPT],
    ...h.io,
  });
  assert.equal(ran, true);
  assert.deepEqual(h.out, ["first", "second"]);
  assert.deepEqual(h.err, []);
  assert.deepEqual(h.exits, [0]);
});

test("a failing gate prints to stderr, not stdout, and exits with its status", () => {
  // `npm run check` chains gates with `&&` and is read in a terminal. A failure on stdout is
  // indistinguishable from the success it replaced.
  const h = harness();
  runAsGate(URL_OF_SCRIPT, () => ({ status: 1, lines: ["FAIL: because"] }), "some", {
    argv: ["node", SCRIPT],
    ...h.io,
  });
  assert.deepEqual(h.out, []);
  assert.deepEqual(h.err, ["FAIL: because"]);
  assert.deepEqual(h.exits, [1]);
});

test("a thrown error becomes a named failure and exit 1, never a pass", () => {
  // The case this exists for: a gate that dies partway through has not measured what it claims to.
  const h = harness();
  const ran = runAsGate(URL_OF_SCRIPT, () => {
    throw new Error("git is not on PATH");
  }, "attribution leak", { argv: ["node", SCRIPT], ...h.io });
  assert.equal(ran, true);
  assert.deepEqual(h.out, []);
  assert.equal(h.err.length, 1);
  assert.match(h.err[0], /FAIL: attribution leak gate could not complete a scan/);
  assert.match(h.err[0], /git is not on PATH/);
  assert.deepEqual(h.exits, [1], "a throw must not fall through to a zero exit");
});

test("the arguments after the script path are what runCli receives", () => {
  const h = harness();
  let seen;
  runAsGate(URL_OF_SCRIPT, (argv) => {
    seen = argv;
    return { status: 0, lines: [] };
  }, "some", { argv: ["node", SCRIPT, "--verbose", "7"], ...h.io });
  assert.deepEqual(seen, ["--verbose", "7"]);
});

/** A real script and a link to it in a fresh directory. */
function linkedScript() {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "gate-main-")));
  const real = join(directory, "gate.mjs");
  writeFileSync(real, "export {}\n");
  const link = join(directory, "link.mjs");
  symlinkSync(real, link);
  return { real, link };
}

/** Starts a gate from `argv[1] = link` whose module URL is `moduleUrl`; returns what happened. */
function startThrough(moduleUrl, link) {
  const h = harness();
  const ran = runAsGate(moduleUrl, () => ({ status: 0, lines: ["ran"] }), "some", { argv: ["node", link], ...h.io });
  return { ran, out: h.out, exits: h.exits };
}

test("a gate reached through a symlinked path still runs, because Node reports the real path", { skip: process.platform === "win32" }, () => {
  const { real, link } = linkedScript();
  const started = startThrough(pathToFileURL(real).href, link);
  assert.deepEqual([started.ran, started.out, started.exits], [true, ["ran"], [0]]);
});

test("a gate started with the link preserved still runs", { skip: process.platform === "win32" }, () => {
  const { link } = linkedScript();
  assert.equal(startThrough(pathToFileURL(link).href, link).ran, true);
});

test("a script path that does not exist is compared as written", () => {
  const h = harness();
  const ran = runAsGate(pathToFileURL("/nowhere/gate.mjs").href, () => ({ status: 0, lines: [] }), "some", {
    argv: ["node", "/nowhere/gate.mjs"],
    ...h.io,
  });
  assert.equal(ran, true);
});
