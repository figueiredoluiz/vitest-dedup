// A bump is applied once, by hand, immediately before a release, and its failure mode is a tree
// that still builds while publishing the wrong version somewhere. These tests run the planner
// against in-memory fixtures shaped like the real files, then run it against the *actual*
// repository contents -- the part that catches a pattern silently ceasing to match after an
// unrelated edit to Cargo.toml or the README.
import assert from "node:assert/strict";
import test from "node:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { BUMPED_FILES, applyBump, planBump } from "../../scripts/release/bump-version.mjs";
import { TARGETS } from "../lib/targets.mjs";
import { platformManifest } from "../../scripts/release/platform-package.mjs";
import {
  cargoLockVersion,
  cargoVersion,
  releaseWorkflowVersion,
  readmeVersionSites,
} from "../../scripts/release/version-sites.mjs";

/** Build an in-memory release tree so version planning can be tested without file writes. */
const targets = Object.values(TARGETS);
const fixture = (overrides = {}) => ({
  "Cargo.toml":
    '[package]\nname = "vitest-dedup"\nversion = "0.1.0"\nedition = "2021"\n',
  "Cargo.lock":
    'version = 4\n\n[[package]]\nname = "anyhow"\nversion = "1.0.0"\n\n' +
    '[[package]]\nname = "vitest-dedup"\nversion = "0.1.0"\ndependencies = []\n',
  "package.json": `${JSON.stringify(
    {
      name: "vitest-dedup",
      version: "0.1.0",
      workspaces: ["npm/platforms/*"],
      optionalDependencies: Object.fromEntries(targets.map((target) => [target.packageName, "0.1.0"])),
    },
    null,
    2,
  )}\n`,
  "package-lock.json": `${JSON.stringify({
    name: "vitest-dedup",
    version: "0.1.0",
    packages: {
      "": {
        version: "0.1.0",
        workspaces: ["npm/platforms/*"],
        optionalDependencies: Object.fromEntries(targets.map((target) => [target.packageName, "0.1.0"])),
      },
      ...Object.fromEntries(targets.map((target) => [
        `npm/platforms/${target.packageDirectory}`,
        { name: target.packageName, version: "0.1.0" },
      ])),
      ...Object.fromEntries(targets.map((target) => [
        `node_modules/${target.packageName}`,
        { resolved: `npm/platforms/${target.packageDirectory}`, link: true },
      ])),
    },
  }, null, 2)}\n`,
  "README.md":
    "```yaml\n- uses: figueiredoluiz/vitest-dedup@v0.1.0\n  with:\n    version: 0.1.0\n```\n",
  ".github/workflows/release.yml":
    "name: Release\non:\n  workflow_dispatch:\n    inputs:\n      version:\n        description: Version to build\n        required: true\n        default: 0.1.0\n",
  ...Object.fromEntries(targets.map((target) => [
    `npm/platforms/${target.packageDirectory}/package.json`,
    `${JSON.stringify(platformManifest(target, "0.1.0"), null, 2)}\n`,
  ])),
  ...overrides,
});

async function fixtureTree(t) {
  const rootDir = await mkdtemp(join(tmpdir(), "vitest-dedup-bump-"));
  t.after(() => rm(rootDir, { recursive: true, force: true }));
  const files = fixture();
  for (const [path, content] of Object.entries(files)) {
    await mkdir(dirname(join(rootDir, path)), { recursive: true });
    await writeFile(join(rootDir, path), content);
  }
  return { rootDir, files };
}

test("the version bumper writes every site in an isolated checkout", async (t) => {
  const { rootDir } = await fixtureTree(t);
  const { previous, written } = await applyBump("0.2.0", { rootDir });
  assert.equal(previous, "0.1.0");
  assert.deepEqual([...written].sort(), [...BUMPED_FILES].sort());
  assert.equal(cargoVersion(await readFile(join(rootDir, "Cargo.toml"), "utf8")), "0.2.0");
  const lock = JSON.parse(await readFile(join(rootDir, "package-lock.json"), "utf8"));
  assert.equal(lock.version, "0.2.0");
  for (const target of targets) {
    const path = `npm/platforms/${target.packageDirectory}/package.json`;
    assert.equal(JSON.parse(await readFile(join(rootDir, path), "utf8")).version, "0.2.0");
    assert.equal(lock.packages[`npm/platforms/${target.packageDirectory}`].version, "0.2.0");
  }
});

test("a failed write restores all version files", async (t) => {
  const { rootDir, files } = await fixtureTree(t);
  let writes = 0;
  const write = async (path, content) => {
    writes += 1;
    if (writes === 4) throw new Error("injected write failure");
    await writeFile(path, content);
  };
  await assert.rejects(applyBump("0.2.0", { rootDir, write }), /injected write failure/);
  for (const [path, content] of Object.entries(files)) {
    assert.equal(await readFile(join(rootDir, path), "utf8"), content, `${path} was not restored`);
  }
});

test("a patch bump rewrites every site including the exact Action ref", () => {
  const { previous, planned } = planBump("0.1.1", fixture());
  assert.equal(previous, "0.1.0");
  assert.deepEqual([...planned.keys()].sort(), [...BUMPED_FILES].sort());

  assert.equal(cargoVersion(planned.get("Cargo.toml")), "0.1.1");
  assert.match(
    planned.get("Cargo.lock"),
    /name = "vitest-dedup"\nversion = "0\.1\.1"/,
  );
  // The unrelated dependency's version must be untouched.
  assert.match(
    planned.get("Cargo.lock"),
    /name = "anyhow"\nversion = "1\.0\.0"/,
  );

  const manifest = JSON.parse(planned.get("package.json"));
  assert.equal(manifest.version, "0.1.1");
  for (const pinned of Object.values(manifest.optionalDependencies)) {
    assert.equal(pinned, "0.1.1");
  }
  const lock = JSON.parse(planned.get("package-lock.json"));
  assert.equal(lock.version, "0.1.1");
  assert.equal(lock.packages[""].version, "0.1.1");
  for (const target of targets) {
    assert.equal(lock.packages[`npm/platforms/${target.packageDirectory}`].version, "0.1.1");
    assert.equal(JSON.parse(planned.get(`npm/platforms/${target.packageDirectory}/package.json`)).version, "0.1.1");
  }

  const readme = readmeVersionSites(planned.get("README.md"));
  assert.deepEqual(readme.versions, ["0.1.1"]);
  assert.deepEqual(
    readme.actionTags,
    ["0.1.1"],
    "the Action ref must move with the exact version input",
  );
  assert.match(planned.get(".github/workflows/release.yml"), /default: 0\.1\.1/);
});

test("a major bump moves the documented exact Action ref", () => {
  const { planned } = planBump("1.0.0", fixture());
  assert.deepEqual(readmeVersionSites(planned.get("README.md")).actionTags, ["1.0.0"]);
});

test("a prerelease bump accepts only versions the release workflow can publish", () => {
  const { planned } = planBump("0.2.0-rc.1", fixture());
  assert.deepEqual(readmeVersionSites(planned.get("README.md")), {
    versions: ["0.2.0-rc.1"],
    actionTags: ["0.2.0-rc.1"],
  });
  assert.throws(() => planBump("0.2.0-rc-1", fixture()), /not a release version/);
  assert.deepEqual(
    readmeVersionSites("- uses: figueiredoluiz/vitest-dedup@v0.2.0-rc-1\n  version: 0.2.0-rc-1\n"),
    { versions: [], actionTags: [] },
  );
});

test("malformed input is refused before anything is planned", () => {
  assert.throws(() => planBump("v0.2.0", fixture()), /not a release version/);
  assert.throws(() => planBump("0.2", fixture()), /not a release version/);
  assert.throws(() => planBump("0.1.0", fixture()), /already at 0\.1\.0/);
  const missing = fixture();
  delete missing["README.md"];
  assert.throws(
    () => planBump("0.2.0", missing),
    /needs the contents of README\.md/,
  );
});

test("a bump refuses stale platform manifests and broken workspace links", () => {
  const stale = fixture();
  const path = "npm/platforms/linux-x64-gnu/package.json";
  const manifest = JSON.parse(stale[path]);
  manifest.libc = ["musl"];
  stale[path] = `${JSON.stringify(manifest, null, 2)}\n`;
  assert.throws(() => planBump("0.2.0", stale), /differs from the target definition/);

  const broken = fixture();
  const lock = JSON.parse(broken["package-lock.json"]);
  lock.packages["node_modules/vitest-dedup-linux-x64-gnu"].resolved =
    "npm/platforms/linux-x64-musl";
  broken["package-lock.json"] = `${JSON.stringify(lock, null, 2)}\n`;
  assert.throws(() => planBump("0.2.0", broken), /not a local link/);

  const mismatched = fixture();
  const oldLock = JSON.parse(mismatched["package-lock.json"]);
  oldLock.version = "0.0.0";
  mismatched["package-lock.json"] = `${JSON.stringify(oldLock, null, 2)}\n`;
  assert.throws(() => planBump("0.2.0", mismatched), /package-lock\.json differs/);

  const staleWorkflow = fixture({ ".github/workflows/release.yml":
    fixture()[".github/workflows/release.yml"].replace("default: 0.1.0", "default: 0.0.9") });
  assert.throws(() => planBump("0.2.0", staleWorkflow), /release workflow default differs/);
});

test("every version site is found in a CRLF checkout, as Windows produces", () => {
  // Git checks these files out with CRLF on Windows by default. The patterns used to anchor on a
  // bare `\n`, so on a Windows contributor's machine `npm run bump:version` reported that
  // Cargo.toml contains no version -- on a file that plainly does. This is the regression test for
  // that, and it failed in CI before the patterns were relaxed, not only in principle.
  const crlf = (text) => text.replaceAll("\n", "\r\n");

  const cargo = crlf(
    '[package]\nname = "vitest-dedup"\nversion = "1.2.3"\nedition = "2021"\n',
  );
  assert.equal(cargoVersion(cargo), "1.2.3");

  const readme = crlf(
    "```yaml\n  - uses: figueiredoluiz/vitest-dedup@v1.2.3\n    with:\n      version: 1.2.3\n```\n",
  );
  const { versions, actionTags } = readmeVersionSites(readme);
  assert.deepEqual(
    versions,
    ["1.2.3"],
    "the CRLF README version pin should be found",
  );
  assert.deepEqual(actionTags, ["1.2.3"], "the CRLF README exact tag should be found");

  const workflow = crlf(fixture()[".github/workflows/release.yml"]);
  assert.equal(releaseWorkflowVersion(workflow), "0.1.0");
});

test("a Cargo.toml whose shape changed fails loudly instead of partially bumping", () => {
  assert.throws(
    () =>
      planBump(
        "0.2.0",
        fixture({ "Cargo.toml": '[package]\nname = "vitest-dedup"\n' }),
      ),
    /\[package\] version in Cargo\.toml was not found/,
  );
  assert.throws(
    () => planBump("0.2.0", fixture({ "README.md": "no pins here\n" })),
    /README\.md carries no Action version pin/,
  );
  assert.throws(
    () => planBump("0.2.0", fixture({
      "README.md": "- uses: figueiredoluiz/vitest-dedup@v0\n  with:\n    version: 0.1.0\n",
    })),
    /README\.md carries no exact Action tag/,
  );
});

/// This repository's own bumped files, with the version it is on and the one after it.
///
/// Shared by the two tests that run the planner against the real tree rather than a fixture.
/// Inlined in both, it was 22 duplicated lines and the duplication ratchet said so.
async function repositoryToday() {
  const files = Object.fromEntries(
    await Promise.all(
      BUMPED_FILES.map(async (path) => [
        path,
        await readFile(new URL(`../../${path}`, import.meta.url), "utf8"),
      ]),
    ),
  );
  const current = cargoVersion(files["Cargo.toml"]);
  const next = `${current.split(".").slice(0, 2).join(".")}.${Number(current.split(".")[2]) + 1}`;
  return { files, current, next };
}

test("the planner still matches this repository as it stands today", async () => {
  const { files, current, next } = await repositoryToday();

  const { previous, planned } = planBump(next, files);
  assert.equal(previous, current);
  assert.equal(cargoVersion(planned.get("Cargo.toml")), next);
  assert.equal(JSON.parse(planned.get("package.json")).version, next);
  for (const pinned of readmeVersionSites(planned.get("README.md")).versions) {
    assert.equal(pinned, next);
  }
  for (const pinned of readmeVersionSites(planned.get("README.md")).actionTags) {
    assert.equal(pinned, next);
  }
  assert.deepEqual(
    [...planned.keys()].sort(),
    [...BUMPED_FILES].sort(),
    "the planner writes exactly the files it declares, and no longer a changelog",
  );
});

// Two guards the changelog tests used to reach incidentally. They are the difference between a
// partial bump and a refused one, so they are worth asserting directly rather than as a side effect
// of something else.

test("a file carrying two version sites is refused, not half-rewritten", () => {
  const twice = fixture({
    "Cargo.lock":
      'version = 4\n\n[[package]]\nname = "vitest-dedup"\nversion = "0.1.0"\ndependencies = []\n\n' +
      '[[package]]\nname = "vitest-dedup"\nversion = "0.1.0"\ndependencies = []\n',
  });
  assert.throws(
    () => planBump("0.1.1", twice),
    /expected exactly one version site/,
    "rewriting one of two matching sites would publish a tree that disagrees with itself",
  );
});

test("the Cargo.lock entry is read by name, not by position", () => {
  const lock =
    'version = 4\n\n[[package]]\nname = "anyhow"\nversion = "9.9.9"\n\n' +
    '[[package]]\nname = "vitest-dedup"\nversion = "0.1.0"\ndependencies = []\n';
  assert.equal(
    cargoLockVersion(lock),
    "0.1.0",
    "a dependency listed first must not be mistaken for the package itself",
  );
});
