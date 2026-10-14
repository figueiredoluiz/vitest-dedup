#!/usr/bin/env node
// Attribution-leak gate. Asserts that no tracked file names a third-party repository this project
// used as a benchmark.
//
// Usage: `node scripts/check/check-name-leak.mjs [--verbose]`
//
// ## Why this is a gate and not a convention
//
// The external corpus is a precision/recall reference: six pinned public repositories, plus a
// watchlist, run through the analyzer to find shapes a fixture could not have invented. They are
// benchmarks, not dependencies. Naming them in the tree does two things nobody chose:
//
//   - it advertises a coupling to repositories this project does not control and cannot keep
//     green, which is exactly why they were taken off the blocking path in the first place;
//   - it pins third parties to specific findings -- "this file is where `duplicate-each-row`
//     scored 14.6% precision" -- which they never agreed to be an example of.
//
// `fixtures/observed/` keeps what those repositories taught, distilled and owned, and every
// anonymised header there maps back through `docs/corpus-provenance.local.md`, which is local. So
// the information is not lost; the attribution is simply not published.
//
// The leak was found by hand, at 34 lines across 8 `src/*.rs` files, 22 fixture headers, two
// READMEs, a CHANGELOG and the tracked manifest itself. A sweep done by hand once will not be done
// by hand again, which is the whole argument for this file.
//
// ## Two checks, because one of them cannot run everywhere
//
// **Check 1 (always runs, needs no list.)** The attribution *markers* are list-free and therefore
// checkable anywhere: a `OBSERVED ON:` header, or a `<name> @ <sha>` commit pin, is a sentence
// about somebody else's repository whatever the name in it is. Forbidding the shape catches the
// next leak even when this gate has no idea which projects are in the corpus.
//
// **Check 2 (needs the local manifest.)** The names themselves can only come from
// `scripts/check/corpus-projects.json`, and that file is deliberately untracked -- a denylist of
// the names, committed, would be the leak it exists to prevent. So this half runs wherever the
// manifest is on disk (any development checkout) and announces itself skipped where it is not
// (CI, a fresh clone).
//
// A skip here is a structural fact rather than an accident, which is the one case this repository
// accepts a gate that measured nothing: the skip is printed as prominently as a failure would be,
// names the file whose absence caused it, and Check 1 still ran. A gate that printed `ok` and
// exited 0 having compared against an empty list would be the failure mode -- hence
// `namesFrom`'s refusal to treat an unparseable manifest as "no names".
//
// A PARTIAL source set is the one absence this gate refuses to accept quietly: #27 merged from a
// worktree whose sweep never saw the watchlist, so its `ok` covered 6 slugs while the main
// checkout knew 8. Some sources present, some missing, is a corpus checkout with a hole, and the
// run fails naming what is absent rather than narrowing the sweep (see `namesFrom`).

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { runAsGate } from "./lib/gate-main.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..", "..");

/** The untracked manifest of pinned benchmark repositories, and the watchlist beside it. */
export const NAME_SOURCES = [
  "scripts/check/corpus-projects.json",
  "docs/corpus-watchlist.local.md",
];

// Attribution markers. `OBSERVED ON:` was this repository's own header for "found in this third
// party's file"; it is now `OBSERVED IN REAL CODE:`, which claims the same provenance without the
// attribution. The sha form catches `someproject @ a1b2c3d`, which is a pin on someone else's
// history.
export const MARKERS = [
  {
    name: "OBSERVED ON:",
    pattern: /OBSERVED ON:/,
    why: "names the repository a shape came from; use `OBSERVED IN REAL CODE:` and record the source in docs/corpus-provenance.local.md",
  },
  {
    name: "commit pin",
    pattern: /[A-Za-z][\w.-]* @ [0-9a-f]{7,40}\b/,
    why: "pins a third party's commit; the sha belongs in the local provenance ledger, not in the tree",
  },
];

/**
 * Files this gate does not read. Its own source states every marker it forbids, so scanning it
 * would report itself; the same is true of its test.
 */
export const SELF = new Set([
  "scripts/check/check-name-leak.mjs",
  "scripts/check/check-name-leak.test.mjs",
]);

/**
 * Generated files, never read: machine-written dependency metadata whose contents name every
 * canonical repository in the dependency tree, mechanically. `package-lock.json` resolving
 * Vitest's dependencies names `vitejs/vite` in resolved-URLs nobody wrote as prose; sweeping it
 * would fail every checkout that installs anything touching a watchlist repository, which is a
 * gate this repository would switch off. Nobody hand-writes an attribution into a lockfile --
 * the leak this gate exists for is prose, and prose is edited, not generated. Excluding a file
 * here is a claim it is machine-written end to end; hand-edited files never belong in it.
 */
export const GENERATED = new Set(["package-lock.json"]);

/** Tracked files, relative to `root`, in a stable order. */
export function trackedFiles(root = ROOT, { run = execFileSync } = {}) {
  const output = run("git", ["ls-files", "-z"], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 1 << 26,
  });
  return output
    .split("\0")
    .filter((path) => path !== "" && !SELF.has(path) && !GENERATED.has(path));
}

/**
 * For paired slugs, segments that name a tool this analyzer is ABOUT are not swept in bare form, not
 * even the possessive: `vitejs/vite` is the repository of the build tool whose config files this
 * project must document, and `docs/vitest-surface.md` legitimately says `Vite's config loader`
 * and `vite.config.*` throughout. A standalone in-scope slug is retained in attributive mode so
 * it cannot disappear from the sweep; no bare term is emitted only for paired slugs.
 *
 * The residual hole is real and recorded: `measured on vite`, `vite's real shape` -- a genuine
 * attribution in the tool's own vocabulary -- passes. MARKERS catches the `OBSERVED ON:` and
 * commit-pin spellings regardless, and the provenance ledger is the discipline that keeps the
 * names out of the tree in the first place. Accepting this hole is a measurement, not an
 * oversight: the alternative was a gate this repository would switch off.
 */
export const IN_SCOPE_SEGMENTS = new Set(["vite"]);

/**
 * Segments too generic to grep for as bare substrings. Every one is an ordinary software word that
 * some repository happens to be named after, so searching it alone reports this project's own
 * prose: the first run of this gate matched one such segment on 180 lines across 30 files, not one
 * of them a leak, which is how a gate gets switched off.
 *
 * These are generic English. Listing them leaks nothing.
 */
export const GENERIC_SEGMENTS = new Set([
  "api",
  "app",
  "cli",
  "core",
  "dev",
  "docs",
  "js",
  "kit",
  "lib",
  "main",
  "next",
  "node",
  "primitives",
  "test",
  "tests",
  "ts",
  "ui",
  "util",
  "utils",
  "vite",
  "web",
]);

/**
 * Search terms derived from a repository slug, each with the form it may be matched in.
 *
 * - `substring` -- anywhere, case-insensitively. For segments distinctive enough that a hit is a
 *   leak and not a coincidence.
 * - `attributive` -- only in the possessive (`name's real shape`), or as the full slug. The
 *   degraded mode for a segment that is generic or shorter than four characters, where a bare
 *   substring search is unusable.
 *
 * The degraded mode still catches the form the leak actually took: the hand sweep this gate was
 * written after found `zod's pair`, `tldraw's real shape`, `unocss's runtime project`. It does NOT
 * catch `Measured on <name>: 12 findings`. That hole is real, it is why `MARKERS` exists, and
 * `--verbose` prints each term's mode so the hole is never invisible.
 */
export function termsFromSlug(slug) {
  const terms = [];
  const trimmed = String(slug).trim();
  if (trimmed === "") return terms;
  if (trimmed.includes("/")) terms.push({ text: trimmed, mode: "substring" });
  for (const segment of trimmed.split("/")) {
    if (segment === "") continue;
    if (IN_SCOPE_SEGMENTS.has(segment.toLowerCase())) {
      // Skip the tool-vocabulary segment ONLY when the slug retains another term (the full
      // slug, or a distinctive owner). A standalone in-scope slug -- `vite` with no owner --
      // skipped outright would be omitted from the sweep entirely: a silent false clean
      // (Copilot review, PR #29, High). Falling through classifies it normally, substring
      // included, so a source that lists the bare name fails LOUDLY instead of vanishing.
      if (terms.length > 0) continue;
    }
    const generic =
      segment.length < 4 || GENERIC_SEGMENTS.has(segment.toLowerCase());
    terms.push({ text: segment, mode: generic ? "attributive" : "substring" });
  }
  return terms;
}

/** Regex-escapes a term, so a `.` or a `+` in a repository name matches literally. */
export function escapeTerm(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * True when `line` carries `term` in a form this gate is willing to call a leak.
 *
 * In `substring` mode, anywhere. In `attributive` mode (a segment too generic or too short to
 * grep bare -- `vite` is a substring of this project's own name), three forms, each measured
 * against the whole tree before being admitted (B1):
 *
 * - the possessive (`name's real shape`) -- the form the original hand-sweep leak took;
 * - an attribution verb with a preposition (`measured on name`, `observed in name`, ...). The
 *   preposition is REQUIRED: without it, `analyzed test`, `extracted tests`, `recorded next`
 *   matched 24 lines of this repository's own prose, none a leak. With it: zero. A cue list
 *   cannot be exhaustive (`findings on name` with no listed verb still passes) -- MARKERS is the
 *   compensating shape-level check, and the residual hole is smaller than the flood a bare
 *   substring produced (2755 lines, every one of them `vitest`).
 * - the name as a repository noun (`the name repository`, `name project`, `name repo`,
 *   `name benchmark`). Zero false positives measured; a name followed by a word that names it as
 *   a repository is attribution whatever the verb was.
 */
export function lineCarries(line, term) {
  if (term.mode === "substring") {
    return line.toLowerCase().includes(term.text.toLowerCase());
  }
  const name = escapeTerm(term.text);
  const forms = [
    `\\b${name}(?:'s|’s)\\b`,
    `\\b(?:measured|observed|found|pinned|scanned|analyzed|analysed|extracted|copied|replicated|reported|scored|recorded|taken)\\s+(?:on|in|from|against|off|with|across|inside|within)\\s+${name}\\b`,
    `\\b${name}\\s+(?:repo|repos|repository|project|benchmark)\\b`,
  ];
  return forms.some((form) => new RegExp(form, "i").test(line));
}

/**
 * Every repository slug this checkout can see. Returns `null` when no source is present, which the
 * caller must render as a skip -- distinct from an empty set, which would mean "a source was read
 * and genuinely lists nothing" and is treated as a failure.
 *
 * A PARTIAL set is also not a narrowing the caller may accept quietly: `missing` names every
 * expected source that is absent, and the caller fails on it (B1a). The #27 merge ran every gate
 * in a worktree that happened to lack the watchlist, so its `ok` covered 6 slugs while the main
 * checkout knew 8 -- the two it never swept (`vitejs/vite`, `trpc/trpc`) were exactly the ones
 * with unreadable terms. A checkout carrying any local corpus file is a corpus checkout; its set
 * must be complete or the run must say so and stop.
 */
export function namesFrom(root = ROOT, sources = NAME_SOURCES) {
  const slugs = new Set();
  const read = [];
  const missing = [];
  for (const source of sources) {
    const path = join(root, source);
    if (!existsSync(path)) {
      missing.push(source);
      continue;
    }
    read.push(source);
    const text = readFileSync(path, "utf8");
    if (source.endsWith(".json")) {
      const manifest = JSON.parse(text);
      for (const project of manifest.projects ?? []) {
        if (typeof project?.repo === "string") slugs.add(project.repo);
      }
    } else {
      // A markdown table whose first cell is the repository: `| owner/repo | ... |`.
      for (const line of text.split("\n")) {
        const match = /^\|\s*\[?`?([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+)`?\]?\s*\|/.exec(line);
        if (match) slugs.add(match[1]);
      }
    }
  }
  return read.length === 0 ? null : { slugs, read, missing };
}

/** Marker hits and name hits in one file's text. */
export function scan(path, text, terms) {
  const problems = [];
  const lines = text.split("\n");
  for (const [index, line] of lines.entries()) {
    for (const marker of MARKERS) {
      if (marker.pattern.test(line)) {
        problems.push({
          path,
          line: index + 1,
          kind: "marker",
          detail: `${marker.name} -- ${marker.why}`,
        });
      }
    }
    for (const term of terms) {
      if (lineCarries(line, term)) {
        problems.push({
          path,
          line: index + 1,
          kind: "name",
          detail: `names a benchmark repository (${term.text})`,
        });
      }
    }
  }
  return problems;
}

/**
 * Scan readable, non-NUL files for attribution/name leaks; return problems and the text-file
 * count, skipping unreadable inputs.
 */
export function inspect(files, terms, { read = readFileSync, root = ROOT } = {}) {
  const problems = [];
  let scanned = 0;
  for (const path of files) {
    let text;
    try {
      text = read(join(root, path), "utf8");
    } catch {
      // A tracked path that cannot be read as text -- a binary asset, or a file deleted in the
      // working tree. Neither can carry prose, so neither is this gate's business.
      continue;
    }
    if (text.includes(" ")) continue;
    scanned += 1;
    problems.push(...scan(path, text, terms));
  }
  return { problems, scanned };
}

/**
 * Evaluate tracked-file and local-name-source gates, returning a status and report lines
 * without exiting the process.
 */
export function runCli(argv = [], { root = ROOT, run = execFileSync } = {}) {
  const verbose = argv.includes("--verbose");
  const lines = [];
  const files = trackedFiles(root, { run });

  // Work-performed floor. `git ls-files` returning almost nothing means this ran outside a
  // checkout, or against an empty index, and a clean report would be a lie about coverage.
  if (files.length < 50) {
    return {
      status: 1,
      lines: [
        `FAIL: attribution leak gate scanned ${files.length} tracked file(s), which is too few to ` +
          "be this repository. Run it from the repository root.",
      ],
    };
  }

  const names = namesFrom(root);
  if (names !== null && names.missing.length > 0) {
    // B1a: fail rather than sweep a narrowed list. See `namesFrom` for the incident this
    // prevents; both files are deliberately untracked, so "complete the set" means copying them
    // from the main checkout, not fetching anything.
    return {
      status: 1,
      lines: [
        "FAIL: the name sweep is missing its local source(s):",
        ...names.missing.map((source) => `  ${source}`),
        "This checkout carries at least one corpus name list, so it is a corpus checkout; a " +
          "sweep over a partial list would report `ok` for leaks only the missing file can " +
          "name (exactly how #27 merged with two repositories never swept). Copy the missing " +
          "file(s) from your main checkout -- both are deliberately untracked -- and re-run.",
      ],
    };
  }
  const byText = new Map();
  if (names !== null) {
    for (const slug of names.slugs) {
      for (const term of termsFromSlug(slug)) {
        // A segment shared by two slugs keeps the *stricter* mode. Promoting it to `substring`
        // because one slug happened to list it distinctively would reintroduce the noise.
        const existing = byText.get(term.text);
        if (existing === undefined || term.mode === "attributive") {
          byText.set(term.text, term);
        }
      }
    }
  }
  const terms = [...byText.values()];

  const { problems, scanned } = inspect(files, terms, { root });

  if (names === null) {
    lines.push(
      "SKIPPED (name sweep): no local benchmark manifest on disk, so this run could not check " +
        "for repository NAMES -- only for attribution markers.",
      `  expected one of: ${NAME_SOURCES.join(", ")}`,
      "  Both are deliberately untracked: a committed denylist of the names would be the leak " +
        "this gate prevents. The sweep runs in any development checkout; CI cannot run it.",
    );
  } else if (names.slugs.size === 0) {
    return {
      status: 1,
      lines: [
        `FAIL: read ${names.read.join(", ")} and found no repository slugs in it. An empty name ` +
          "list makes the sweep vacuous, so this fails rather than reporting a clean scan.",
      ],
    };
  } else {
    const degraded = terms.filter((term) => term.mode === "attributive");
    // Slug-only means the bare tool segment produced NO retained term of its own. A standalone
    // in-scope slug keeps an attributive term (the never-omitted rule), so deriving the count
    // from the slugs alone would claim slug-only coverage while its cue forms are active
    // (Copilot review, PR #29 round 2). The status line claims only what the terms do.
    // Slug-only means the bare tool segment produced NO retained term of its own. A standalone
    // in-scope slug keeps an attributive term (the never-omitted rule), so deriving the count
    // from the slugs alone would claim slug-only coverage while its cue forms are active
    // (Copilot review, PR #29 round 2). The status line claims only what the terms do.
    const slugOnly = [...names.slugs].filter((slug) => {
      const inScope = slug
        .split("/")
        .find((segment) => IN_SCOPE_SEGMENTS.has(segment.toLowerCase()));
      return inScope !== undefined && byText.get(inScope) === undefined;
    });
    lines.push(
      `name sweep over ${names.slugs.size} benchmark repositor${names.slugs.size === 1 ? "y" : "ies"} ` +
        `(${terms.length} search terms, ${degraded.length} matched only in attributive form` +
        (slugOnly.length > 0
          ? `, ${slugOnly.length} swept by slug only as in-scope tool vocabulary`
          : "") +
        `) from ${names.read.join(", ")}`,
    );
  }

  lines.push(
    `scanned ${scanned} tracked text file(s) for ${MARKERS.length} attribution marker(s)`,
  );
  if (verbose) {
    for (const term of [...terms].sort((a, b) => a.text.localeCompare(b.text))) {
      lines.push(`  term ${term.text} (${term.mode})`);
    }
  }

  if (problems.length > 0) {
    lines.push("\nFAIL: a tracked file attributes a shape to a third-party repository");
    for (const problem of problems) {
      lines.push(`  ${problem.path}:${problem.line}  ${problem.detail}`);
    }
    lines.push(
      "\nKeep the shape, drop the attribution: say what the code looks like and why that is " +
        "realistic, and record whose repository it was in docs/corpus-provenance.local.md.",
    );
    return { status: 1, lines };
  }

  // The closing line must claim only what this run checked. With the manifest absent the name
  // sweep did not happen, and saying "no tracked file names a benchmark repository" would be a
  // verdict on a comparison that was never made.
  lines.push(
    names === null
      ? "\nok (markers only): no tracked file carries an attribution marker. Whether any file NAMES " +
          "a benchmark repository was NOT checked -- see the skip above."
      : "\nok: no tracked file names a benchmark repository or carries an attribution marker.",
  );
  return { status: 0, lines };
}

runAsGate(import.meta.url, runCli, "attribution leak");
