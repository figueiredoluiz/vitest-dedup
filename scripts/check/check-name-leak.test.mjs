// Tests for the attribution-leak gate. The gate's own source states every marker it forbids, so it
// excludes itself and this file from the scan (`SELF`); these tests therefore drive the exported
// functions directly rather than grepping the real tree.

import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  GENERATED,
  GENERIC_SEGMENTS,
  IN_SCOPE_SEGMENTS,
  MARKERS,
  NAME_SOURCES,
  SELF,
  escapeTerm,
  inspect,
  lineCarries,
  namesFrom,
  runCli,
  scan,
  termsFromSlug,
  trackedFiles,
} from "./check-name-leak.mjs";

// A slug is matched three ways and each is asserted, because the sweep that motivated this gate
// found all three spellings in the tree.
test("a distinctive slug yields a full-slug term and a term per segment", () => {
  const terms = termsFromSlug("someowner/someproject");
  assert.deepEqual(terms, [
    { text: "someowner/someproject", mode: "substring" },
    { text: "someowner", mode: "substring" },
    { text: "someproject", mode: "substring" },
  ]);
});

test("a generic or short segment is degraded to attributive, never dropped", () => {
  // `core` is generic; `abc` is under four characters. Both would be unusable as substrings, and
  // both must still be searched in the one form the leak took.
  const generic = termsFromSlug("someowner/core");
  assert.deepEqual(generic.at(-1), { text: "core", mode: "attributive" });
  const short = termsFromSlug("abc-zone/abc");
  assert.deepEqual(short.at(-1), { text: "abc", mode: "attributive" });
  // Dropping it instead was the first implementation and it is what let two real leaks through.
  assert.ok(short.some((term) => term.text === "abc"));
});

test("a slug with no owner, or nothing at all, yields no bogus terms", () => {
  assert.deepEqual(termsFromSlug(""), []);
  assert.deepEqual(termsFromSlug("   "), []);
  assert.deepEqual(termsFromSlug("solo"), [{ text: "solo", mode: "substring" }]);
  // A trailing slash must not produce an empty-string term, which would match every line.
  const trailing = termsFromSlug("owner/");
  assert.ok(!trailing.some((term) => term.text === ""));
});

test("every generic segment is lowercase, so the case-folded lookup can hit it", () => {
  for (const segment of GENERIC_SEGMENTS) {
    assert.equal(segment, segment.toLowerCase(), segment);
  }
  for (const segment of IN_SCOPE_SEGMENTS) {
    assert.equal(segment, segment.toLowerCase(), segment);
  }
});

test("an in-scope tool segment yields NO bare term: only the slug and owner are swept", () => {
  // `vite` is a substring of this project's own name and the possessive of the tool's prose
  // (`Vite's config loader` is legitimate documentation), so no bare form of it is usable. The
  // slug and `vitejs` remain distinctive and stay.
  const terms = termsFromSlug("vitejs/vite");
  assert.deepEqual(terms, [
    { text: "vitejs/vite", mode: "substring" },
    { text: "vitejs", mode: "substring" },
  ]);
});

test("a STANDALONE in-scope slug is never omitted from the sweep", () => {
  // Copilot review, PR #29, High: skipping the segment unconditionally made
  // `termsFromSlug("vite")` return NO terms -- a source listing the bare slug would have been
  // silently swept as nothing, a false clean. With no slug term to retain, the segment falls
  // through to the normal classification and fails loudly instead.
  const standalone = termsFromSlug("vite");
  // Attributive, not substring: `vite` is in GENERIC_SEGMENTS (it matched 2,755 lines as a bare
  // substring), so the fallback keeps the narrow cue forms rather than the flood.
  assert.deepEqual(standalone, [{ text: "vite", mode: "attributive" }]);
  assert.equal(
    lineCarries("the vite repository repeats it", standalone[0]),
    true,
    "the retained term sweeps the repository-noun form",
  );
});

test("a substring term matches anywhere and folds case", () => {
  const sub = { text: "someproject", mode: "substring" };
  assert.equal(lineCarries("measured on SomeProject: 12 findings", sub), true);
  assert.equal(lineCarries("nothing here", sub), false);
});

test("an attributive term catches the possessive and the attribution cues, not the bare word", () => {
  const attr = { text: "core", mode: "attributive" };
  // The form the original hand-sweep leak took.
  assert.equal(lineCarries("core's real shape", attr), true);
  assert.equal(lineCarries("Core's real shape", attr), true);
  assert.equal(lineCarries("core’s real shape", attr), true, "curly apostrophe");
  // The B1 hole, closed: an attribution verb with a preposition names the repository whatever
  // case it carries.
  assert.equal(lineCarries("Measured on core: 12 findings", attr), true);
  assert.equal(lineCarries("observed in core, the scan found 4", attr), true);
  assert.equal(lineCarries("pinned against CORE @ a1b2c3d", attr), true);
  // A name followed by a repository noun is attribution whatever the verb was.
  assert.equal(lineCarries("the core repository repeats it", attr), true);
  assert.equal(lineCarries("a core project ships it", attr), true);
  // The bare word stays quiet, which is the whole reason the mode exists.
  assert.equal(lineCarries("the core of the matter", attr), false);
  assert.equal(lineCarries("scores and cores", attr), false);
  // And the cues do not fire on this repository's own prose: a verb with NO preposition was the
  // 24-line false positive (analyzed test, extracted tests, recorded next) that forced the
  // preposition to be required.
  assert.equal(lineCarries("one record per analyzed test", attr), false);
  assert.equal(lineCarries("the count of extracted tests is 5", attr), false);
  assert.equal(lineCarries("the target is recorded next to them", attr), false);
  assert.equal(lineCarries("a plain vite.config lookup", attr), false);
  // The residual hole, documented in lineCarries' doc: no listed verb, no possessive, no
  // repository noun -- `findings on core` passes. MARKERS is the compensating shape check.
  assert.equal(lineCarries("findings on core were mixed", attr), false);
});

test("a term carrying regex metacharacters is matched literally", () => {
  assert.equal(escapeTerm("a.b+c"), "a\\.b\\+c");
  const attr = { text: "a.b", mode: "attributive" };
  assert.equal(lineCarries("a.b's shape", attr), true);
  assert.equal(lineCarries("axb's shape", attr), false, "the dot must not be a wildcard");
});

test("each marker fires on the shape it names and not on ordinary prose", () => {
  const observed = MARKERS.find((marker) => marker.name === "OBSERVED ON:");
  assert.equal(observed.pattern.test("// OBSERVED ON: someproject, a/b.ts:4"), true);
  assert.equal(
    observed.pattern.test("// OBSERVED IN REAL CODE: a public library"),
    false,
    "the replacement header must not trip the rule it replaced",
  );

  const pin = MARKERS.find((marker) => marker.name === "commit pin");
  assert.equal(pin.pattern.test("// someproject @ d2b135c, packages/x"), true);
  assert.equal(pin.pattern.test("// someproject @ ffd1e744a5b49a15d1db9f546ca97c8422eae0ac"), true);
  // Not every ` @ ` is a pin: an email-ish or prose use must stay quiet.
  assert.equal(pin.pattern.test("// see the note @ the top of this file"), false);
  assert.equal(pin.pattern.test("// measured @ 2026-09-27"), false);
});

test("scan reports the one-based line and distinguishes a marker hit from a name hit", () => {
  const text = ["first", "// OBSERVED ON: x", "someproject's shape"].join("\n");
  const problems = scan("a.rs", text, [{ text: "someproject", mode: "attributive" }]);
  assert.deepEqual(
    problems.map(({ line, kind }) => ({ line, kind })),
    [
      { line: 2, kind: "marker" },
      { line: 3, kind: "name" },
    ],
  );
  assert.equal(problems[0].path, "a.rs");
});

test("inspect skips a binary file and a file it cannot read, and counts only what it scanned", () => {
  const files = ["text.md", "binary.png", "gone.rs"];
  const read = (path) => {
    if (path.endsWith("gone.rs")) throw new Error("ENOENT");
    if (path.endsWith("binary.png")) return " someproject";
    return "someproject's shape";
  };
  const { problems, scanned } = inspect(files, [{ text: "someproject", mode: "attributive" }], {
    read,
    root: "/root",
  });
  assert.equal(scanned, 1, "the binary and the unreadable file are not scanned");
  assert.equal(problems.length, 1);
  assert.equal(problems[0].path, "text.md");
});

test("namesFrom returns null when no source exists, which is not the same as an empty list", () => {
  // `null` must be rendered as a skip; an empty set is a failure. Conflating them is how a gate
  // reports a clean scan having compared against nothing.
  assert.equal(namesFrom("/nonexistent-root-for-this-test", NAME_SOURCES), null);
});

test("namesFrom reads slugs from the manifest this checkout has", () => {
  const found = namesFrom();
  if (found === null) {
    // A fresh clone or CI. The gate's skip path is asserted by the CLI test below.
    return;
  }
  assert.ok(found.slugs.size > 0, "a manifest that is present must yield slugs");
  for (const slug of found.slugs) assert.match(slug, /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/);
  assert.ok(found.read.length > 0);
});

test("the gate excludes itself and its test, or it would report its own examples", () => {
  assert.ok(SELF.has("scripts/check/check-name-leak.mjs"));
  assert.ok(SELF.has("scripts/check/check-name-leak.test.mjs"));
  const files = trackedFiles();
  for (const path of SELF) assert.ok(!files.includes(path), path);
});

test("the CLI refuses a tree too small to be this repository", () => {
  const run = () => ["a.rs", "b.rs"].join("\0");
  const { status, lines } = runCli([], { run });
  assert.equal(status, 1);
  assert.match(lines.join("\n"), /too few/);
});

test("the CLI announces a skipped name sweep rather than reporting a clean one", () => {
  // No manifest on this root, so only the markers can be checked. The run must say so.
  const run = () => Array.from({ length: 60 }, (_, index) => `f${index}.md`).join("\0");
  const { status, lines } = runCli([], { root: "/nonexistent-root-for-this-test", run });
  const output = lines.join("\n");
  assert.equal(status, 0);
  assert.match(output, /^SKIPPED \(name sweep\)/m);
  assert.match(output, /deliberately untracked/);
  // And it must not claim the half it could not run. This is the assertion that matters: a skip
  // that still signs off on "no tracked file names a benchmark repository" is the silent pass.
  assert.match(output, /^ok \(markers only\)/m);
  assert.ok(
    !/^ok: no tracked file names a benchmark/m.test(output),
    "a skipped sweep must not be reported as a clean sweep",
  );
});

// The local-source scaffolding, shared by every test that drives the name sweep on a throwaway
// root. Extracted when the fifth copy tripped the duplication gate: each test needs the same
// directories and fixture files built by these helpers, differing in the manifest's project list.
/** Create a temporary name-sweep root with the required scripts/check and docs directories. */
function tempRoot(label) {
  const root = mkdtempSync(join(tmpdir(), label));
  mkdirSync(join(root, "scripts", "check"), { recursive: true });
  mkdirSync(join(root, "docs"), { recursive: true });
  return root;
}

/** Write the supplied project list to the temporary root's corpus manifest. */
function writeManifest(root, projects) {
  writeFileSync(
    join(root, "scripts", "check", "corpus-projects.json"),
    JSON.stringify({ projects }),
  );
}

const MINIMAL_WATCHLIST = ["| Repository | Notes |", "| --- | --- |", ""].join("\n");

/** Write a fixture watchlist beneath root, defaulting to an empty repository table. */
function writeWatchlist(root, text = MINIMAL_WATCHLIST) {
  writeFileSync(join(root, "docs", "corpus-watchlist.local.md"), text);
}

/** 60 absent names so the work-performed floor is satisfied on a root with no real files. */
const ABSENT_NAMES = Array.from({ length: 60 }, (_, index) => `absent-${index}.md`);

/**
 * Return a temporary root with a manifest/watchlist and a runner listing 60 NUL-separated file names.
 * Optionally add the requested leak; pad the listing with absent names to satisfy the work-performed floor.
 *
 * This exists because the first version of this suite reached the sweep only by reading the
 * developer's own untracked manifest. Coverage then differed by environment -- 88.6% locally, 84.2%
 * in CI -- and the floor recorded from the local run was one CI could never meet. A branch whose
 * coverage depends on which machine ran it is not covered.
 */
function rootWithManifest({ leak = null } = {}) {
  const root = tempRoot("name-leak-");
  writeManifest(root, [
    { repo: "someowner/someproject" },
    { repo: "otherowner/core" },
    { repo: "vitejs/vite" },
  ]);
  writeWatchlist(
    root,
    ["| Repository | Notes |", "| --- | --- |", "| thirdowner/thirdproject | a note |", ""].join("\n"),
  );
  writeFileSync(join(root, "clean.md"), "nothing to see here\n");
  if (leak !== null) writeFileSync(join(root, "leaky.md"), `${leak}\n`);
  // The ones that do not exist are skipped by `inspect`, which is the behaviour the
  // binary/unreadable test above pins.
  const names = ["clean.md", ...(leak === null ? [] : ["leaky.md"])];
  while (names.length < 60) names.push(`absent-${names.length}.md`);
  return { root, run: () => names.join("\0") };
}

test("the name sweep runs, and reports the repositories and terms it derived", () => {
  const { root, run } = rootWithManifest();
  const { status, lines } = runCli([], { root, run });
  const output = lines.join("\n");
  assert.equal(status, 0, output);
  assert.match(output, /name sweep over 4 benchmark repositories/);
  assert.match(output, /corpus-projects\.json/);
  assert.match(output, /corpus-watchlist\.local\.md/);
  // `core` is generic and so is degraded; the other segments are not.
  assert.match(output, /1 matched only in attributive form/);
  assert.ok(!/^SKIPPED/m.test(output), "a manifest is present, so nothing may be skipped");
});

test("the name sweep fails a tracked file that names a benchmark repository", () => {
  const { root, run } = rootWithManifest({ leak: "the someproject shape, recorded" });
  const { status, lines } = runCli([], { root, run });
  const output = lines.join("\n");
  assert.equal(status, 1, output);
  assert.match(output, /leaky\.md:1/);
  assert.match(output, /names a benchmark repository \(someproject\)/);
});

test("the attributive mode catches the possessive and leaves the bare generic word alone", () => {
  // `core` may reach the tree only as `core's`; `the core of it` must stay quiet or this is noise.
  const quiet = rootWithManifest({ leak: "the core of the matter, at the core" });
  assert.equal(runCli([], { root: quiet.root, run: quiet.run }).status, 0);
  const loud = rootWithManifest({ leak: "core's real shape, measured" });
  const { status, lines } = runCli([], { root: loud.root, run: loud.run });
  assert.equal(status, 1);
  assert.match(lines.join("\n"), /names a benchmark repository \(core\)/);
});

test("the B1 hole is closed end to end: Measured on <generic name> fails the sweep", () => {
  // The exact sentence the backlog carried as the documented hole. Before the cue forms, this
  // passed the gate -- a short or generic name was reachable only through the possessive.
  const hole = rootWithManifest({ leak: "Measured on core: 12 findings" });
  const { status, lines } = runCli([], { root: hole.root, run: hole.run });
  assert.equal(status, 1, lines.join("\n"));
  assert.match(lines.join("\n"), /leaky\.md:1/);
  assert.match(lines.join("\n"), /names a benchmark repository \(core\)/);
});

test("legitimate tool prose about an in-scope segment stays clean end to end", () => {
  // `vite`-shaped prose -- the possessive of the TOOL this analyzer documents -- must not fail
  // the sweep; only the slug and owner forms are terms for such a segment.
  const prose = rootWithManifest({ leak: "reads Vite's own config object from vite.config.ts" });
  assert.equal(runCli([], { root: prose.root, run: prose.run }).status, 0);
  const slug = rootWithManifest({ leak: "the shape came from vitejs/vite originally" });
  const { status, lines } = runCli([], { root: slug.root, run: slug.run });
  assert.equal(status, 1, lines.join("\n"));
  assert.match(lines.join("\n"), /names a benchmark repository \(vitejs\/vite\)/);
});

test("the slug-only coverage claim counts only slugs with no retained bare term", () => {
  // Copilot review, PR #29 round 2: the status line is a coverage claim, and a standalone
  // `vite` slug RETAINS an attributive term (the never-omitted rule) -- claiming slug-only for
  // it would overstate the narrowing. Both slugs listed: no slug-only suffix, and the retained
  // term is visible in --verbose instead.
  const root = tempRoot("name-leak-standalone-");
  writeManifest(root, [
    { repo: "someowner/someproject" },
    { repo: "vitejs/vite" },
    { repo: "vite" },
  ]);
  writeWatchlist(root);
  const { status, lines } = runCli(["--verbose"], { root, run: () => ABSENT_NAMES.join("\0") });
  const output = lines.join("\n");
  assert.equal(status, 0, output);
  assert.ok(!/swept by slug only/.test(output), output);
  assert.match(output, /^ {2}term vite \(attributive\)$/m);
  // The paired case keeps the claim: its bare segment produced no retained term.
  const paired = rootWithManifest();
  const pairedOutput = runCli([], { root: paired.root, run: paired.run }).lines.join("\n");
  assert.match(pairedOutput, /1 swept by slug only as in-scope tool vocabulary/);
});

test("--verbose lists every term with the mode it will be matched in", () => {
  const { root, run } = rootWithManifest();
  const { lines } = runCli(["--verbose"], { root, run });
  const output = lines.join("\n");
  assert.match(output, /^ {2}term core \(attributive\)$/m);
  assert.match(output, /^ {2}term someproject \(substring\)$/m);
  assert.match(output, /^ {2}term someowner\/someproject \(substring\)$/m);
});

test("a manifest that lists no repositories fails rather than sweeping for nothing", () => {
  const root = tempRoot("name-leak-empty-");
  writeManifest(root, []);
  writeWatchlist(root);
  const { status, lines } = runCli([], { root, run: () => ABSENT_NAMES.join("\0") });
  assert.equal(status, 1);
  assert.match(lines.join("\n"), /found no repository slugs/);
});

test("a partial source set fails rather than sweeping a narrowed list (B1a)", () => {
  // The #27 incident: a worktree carrying the manifest but not the watchlist ran green over 6
  // slugs while the main checkout knew 8 -- the two it never swept were exactly the ones with
  // unusable terms. A checkout with ANY local name list is a corpus checkout; its set must be
  // complete.
  const root = tempRoot("name-leak-partial-");
  writeManifest(root, [{ repo: "someowner/someproject" }]);
  const missingWatchlist = runCli([], { root, run: () => ABSENT_NAMES.join("\0") });
  assert.equal(missingWatchlist.status, 1);
  const watchlistOutput = missingWatchlist.lines.join("\n");
  assert.match(watchlistOutput, /missing its local source/);
  assert.match(watchlistOutput, /corpus-watchlist\.local\.md/);
  assert.match(watchlistOutput, /#27/);

  // The mirror case: a watchlist without the manifest fails the same way.
  const solo = tempRoot("name-leak-solo-");
  writeWatchlist(
    solo,
    ["| Repository | Notes |", "| --- | --- |", "| someowner/someproject | note |", ""].join("\n"),
  );
  const missingManifest = runCli([], { root: solo, run: () => ABSENT_NAMES.join("\0") });
  assert.equal(missingManifest.status, 1);
  assert.match(missingManifest.lines.join("\n"), /corpus-projects\.json/);
});

test("namesFrom reports which expected sources are missing", () => {
  const root = tempRoot("name-leak-missing-");
  writeWatchlist(root, ["| Repository |", "| --- |", "| a/b |", ""].join("\n"));
  const found = namesFrom(root);
  assert.equal(found.slugs.size, 1);
  assert.deepEqual(found.missing, ["scripts/check/corpus-projects.json"]);
});

test("the real tree passes, and the run reports the work it performed", () => {
  const { status, lines } = runCli();
  const output = lines.join("\n");
  assert.equal(status, 0, output);
  const scanned = /scanned (\d+) tracked text file/.exec(output);
  assert.ok(scanned, output);
  assert.ok(
    Number(scanned[1]) > 100,
    `a real run must scan more than a handful of files, scanned ${scanned?.[1]}`,
  );
});

test("a generated lockfile is never swept, and the exclusion stays machine-written-only", () => {
  // The exclusion exists because package-lock.json names vitejs/vite mechanically (resolved
  // dependency URLs); sweeping it would fail every checkout installing anything touching a
  // watchlist repository. Load-bearing both ways: the lockfile's text WOULD flag if it were
  // swept, and the exclusion may not grow a hand-editable file.
  const terms = termsFromSlug("vitejs/vite");
  const lockfileText = '"resolved": "https://github.com/vitejs/vite/tarball/abc"';
  const { problems } = inspect(["package-lock.json"], terms, { read: () => lockfileText });
  assert.ok(problems.length > 0, "premise: the lockfile's resolved URLs flag when swept");

  // The filter is applied by `trackedFiles`, so the listing is driven through its injected
  // runner: the lockfile is dropped, its neighbours are kept, and nothing else is.
  const listed = trackedFiles("/nowhere", {
    run: () => "README.md\0package-lock.json\0scripts/x.mjs\0",
  });
  assert.deepEqual(listed, ["README.md", "scripts/x.mjs"]);
  assert.equal(
    GENERATED.size,
    1,
    "an exclusion added for a hand-editable file would be a hole, not a cache",
  );
});
