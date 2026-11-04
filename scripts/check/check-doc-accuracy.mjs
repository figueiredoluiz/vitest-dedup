#!/usr/bin/env node
// Documentation-accuracy gate. Two mechanical checks over every tracked markdown file and every
// Rust doc comment: a fenced block must be what its label claims, and a unit conversion must be
// true under the base it names.
//
// Usage: `node scripts/check/check-doc-accuracy.mjs [--verbose]`
//
// ## The gap this closes
//
// `tests/docs_contract.rs` and the `[status: current]` tripwire already make prose about
// *behaviour* falsifiable: they parse the documented tables at runtime and drive the real binary
// and the real reporters against what they parsed. Nothing checked prose about *arithmetic*, and
// that is where the last four review rounds' findings moved to as the findings in code fell to
// nearly zero. Two arrived in one day, both written during the pull request that shipped them:
//
//   - a fenced block labelled ```json whose values were `…` placeholders, so a reader who pasted
//     it into a parser got a syntax error from documentation that advertised a machine format;
//   - "a million tests costs **3.4 GB**" at 3,642 bytes per test. Decimally that is 3.64 GB; 3.4
//     is the *binary* figure, 3.39 GiB, rounded up. The sentence read as self-consistent only
//     because the budget beside it called 2,048 bytes "2 KB" -- the document was silently using
//     KB to mean KiB, so every number in it was off by the base.
//
// Both are mechanical. Neither is caught by reading carefully, which is what had been tried.
//
// ## Check 1 -- a fence must be what it claims
//
// Every ```json block must `JSON.parse` as one document. Every ```jsonl / ```ndjson block must
// parse as one JSON document **per non-empty line**, which is what this tool's own `jsonl`
// reporter emits (`src/reporters/jsonl.rs`) and therefore what its documentation has to be able
// to show.
//
// `jsonl` is checked rather than skipped on purpose. Skipping it would have been the convenient
// choice and it would have left the format this tool exists to emit as the one format nothing
// verifies. It also gives the honest label to blocks that carry two records: a stream of records
// is not a JSON document, and labelling it ```json is the same defect as the ellipsis -- it fails
// in a parser.
//
// Blocks that are deliberately schematic -- the `--debug-memory` record with `…` standing in for
// a per-field breakdown nobody needs spelled out -- are legitimate. They are labelled ```text,
// which is true, instead of ```json, which is not. That relabelling is the fix this check
// enforces, not a hole in it: a schematic is prose, and prose gets no machine-format promise.
//
// This check is deterministic and has no tolerance. Its false-positive rate is whatever
// `JSON.parse` says.
//
// ## Check 2 -- a unit conversion must be true under the base it names
//
// A decimal unit (kB/MB/GB/TB) is checked against powers of 1000; a binary unit (KiB/MiB/GiB/TiB)
// against powers of 1024. "3.4 GB" for 3,642,000,000 bytes fails -- decimally it is 3.64 -- while
// "3.4 GiB" for the same figure passes.
//
// The whole difficulty is noise. A gate that cries wolf on prose gets switched off, which is
// strictly worse than not having it, so two decisions are made in the quiet direction:
//
// **Association is by agreement, not by proximity.** A claim is only *checked* when some byte
// figure in the same paragraph reproduces it under one base or the other. A claim no nearby byte
// figure explains is **skipped and counted**, never guessed at: `947 MB` in a prose paragraph is a
// peak-RSS measurement whose source number (946,814,976) is not written down anywhere in the
// tree, so this gate has nothing to check it against and says so, rather than inventing a
// denominator. The run prints claims seen against claims checked, so the coverage of this check
// is itself a reported number and not an assumption.
//
// The consequence is the failure this catches: a claim that matches the byte figure under the
// *other* base is exactly the KB/KiB conflation above, and two bases that agree within tolerance
// by coincidence is not a thing that happens -- they differ by 2.4% at kilo, 4.9% at mega and
// 7.4% at giga, all far outside the tolerance below.
//
// **Tolerance is significant-figure rounding, not a percentage.** "2 KB" for 2,048 bytes is
// correct: 2.048 rounded to the precision the author wrote is 2. "3.4 GB" for 3,642,000,000 is
// not: 3.642 rounded to one decimal is 3.6. So the tolerance is half a unit in the claim's last
// written place -- doubled where the author marked the figure as approximate (`~`, "about",
// "roughly") -- with a 0.5% relative floor so that a long figure like "3.64 GB" is not held to
// an unreasonably exact conversion. Half an ulp is what "I rounded this" means; a fixed
// percentage would either reject `2 KB` or accept `3.4 GB`, and this gate exists to do neither.
//
// A paragraph is the window because that is the unit a derivation is written in. `README.md`
// states 3,642 bytes/test in one table cell and "A million tests at that rate is 3.64 GB" in the
// next; a sentence-sized window would have had nothing to check the claim against, which is the
// one place this gate had to reach.
//
// ## What is deliberately NOT checked
//
// Not every factual claim in prose. That set is unbounded, a gate over it could never stay green,
// and a gate nobody can keep green gets deleted with the next red build. Two mechanical checks
// that hold are worth more than a broad one that is switched off in a month.
//
// ## This gate asserts a floor on work performed
//
// The repository rule, broken inside four of this repository's own gates: a check that measured
// nothing must fail, not pass quietly. A documentation scanner is unusually good at this, because
// "every fence is valid" and "no fences were found" print identically -- one bad glob, one
// renamed directory, one `git ls-files` returning nothing in a shallow checkout, and this file
// becomes a 40ms no-op that reports success forever. So the run asserts floors on files scanned,
// fences found, JSON-family fences actually parsed, unit claims seen, and unit claims actually
// checked. Every floor is below the measured value with slack, and every one of them fails loud.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { runAsGate } from "./lib/gate-main.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

// ---------------------------------------------------------------------------------------------
// Work-performed floors. Measured on the tree that introduced this gate: 39 files, 38 fenced
// blocks of which 6 are JSON-family, 40 unit claims of which 10 could be tied to a byte figure.
// Each floor sits below its measured value with slack, because these exist to catch a scan that
// collapsed -- not to pin an inventory that every new paragraph would have to update.
// ---------------------------------------------------------------------------------------------
// Lowered 2026-10-01 when the documentation was trimmed and the manual changelog removed: the
// corpus genuinely shrank, so the counts did too (unit claims 29 -> 24, checked 5 -> 3). These
// floors exist to catch a SCAN that collapsed, not to stop documentation getting shorter on
// purpose, so they follow a deliberate reduction rather than blocking it. They do not follow an
// accidental one: anything that drops a claim without an edit here still fails.
// Lowered 2026-11-04 when the earlier analyzer core was deleted: its fingerprint module held one
// of the three checked claims (checked 3 -> 2).
export const FLOORS = {
  files: 25,
  fences: 20,
  jsonFamilyFences: 4,
  unitClaims: 24,
  checkedUnitClaims: 2,
  // Check 2b's floor, and the reason it is at the measured value rather than below it. The other
  // floors here guard against a scan that collapsed; this one guards against the check quietly
  // narrowing, which is a different failure with the same symptom. A tie is admitted only while
  // its doc comment names exactly ONE unit figure, so adding a second sentence to
  // `discovery.rs`'s comment would drop it out of the check and leave a green run behind.
  // Measured 2026-09-29: 2 ties, `MAX_SOURCE_FILE_BYTES` and `MAX_BASELINE_BYTES`. Raise this
  // when a third byte constant grows a unit figure; never lower it to make a run pass.
  constantClaims: 2,
};

// ---------------------------------------------------------------------------------------------
// Fence extraction. One scanner for both languages: a markdown file is its own text, and a Rust
// file is projected down to its doc comments (`//!` and `///`) with line numbers preserved, so a
// fenced example inside a module header is read exactly like one in a `.md` file. Rust lines that
// are not doc comments become holes, which end a paragraph and close nothing -- a fence cannot
// span ordinary code.
// ---------------------------------------------------------------------------------------------

/** Projects a source file to the lines this gate reads: `null` for a line it does not read. */
export function docLines(path, source) {
  const raw = source.split("\n");
  if (path.endsWith(".rs")) {
    // Line form (`///`, `//!`) and block form (`/** */`, `/*! */`) both. Only the line form was
    // projected at first, which made every fence and unit claim inside a block comment invisible
    // -- a silent bypass that would have opened the moment anyone wrote one.
    //
    // This is a scanner rather than a per-line regex because two things a regex got wrong are
    // both real. Rust block comments **nest**, so an inner `*/` must not end the outer comment
    // (the rest of the doc then projected to null and its fences were skipped). And a comment
    // can close part-way through a line, with code after it -- a regex anchored on `*/$` left
    // the scanner believing it was still inside, so ordinary Rust code downstream was read as
    // prose and could raise a false failure. Depth tracking handles both, and every delimiter on
    // a line is processed in order.
    let docDepth = 0;
    let codeDepth = 0;
    // A string literal is not code and not documentation, and the scanner has to know the
    // difference. Rust lets a raw string hold anything at all, so this is valid source:
    //
    //     const EXAMPLE: &str = r#"
    //     /// ```json
    //     /// {bad}
    //     "#;
    //
    // Read without lexical context, those `///` lines project as documentation, the fence opens,
    // `{bad}` fails to parse, and the gate rejects a correct file. A gate that fails valid source
    // is worse than one that misses a defect: the first gets switched off, the second only misses
    // something. (The defect predates the block-comment scanner -- the original line-form-only
    // regex matched `///` inside a raw string exactly the same way -- so this is fixed for both
    // forms at once, not just for the newer one.)
    //
    // Only the string state needs to survive a line break: a normal string may span lines and a
    // raw string routinely does. `null` means "not in a string".
    let str = null;
    return raw.map((line) => {
      let index = 0;
      let text = "";
      while (index < line.length) {
        if (str) {
          if (str.hashes === null) {
            // Normal string: a backslash escapes whatever follows, including a quote.
            if (line[index] === "\\") index += 2;
            else if (line[index] === '"') {
              str = null;
              index += 1;
            } else index += 1;
          } else {
            // Raw string: no escapes at all; it ends at a quote followed by its own hash count.
            if (
              line[index] === '"' &&
              line.slice(index + 1, index + 1 + str.hashes) ===
                "#".repeat(str.hashes)
            ) {
              index += 1 + str.hashes;
              str = null;
            } else index += 1;
          }
          continue;
        }
        if (codeDepth > 0) {
          // Inside an ordinary (non-doc) block comment: consume, tracking nesting, emit nothing.
          if (line.startsWith("/*", index)) {
            codeDepth += 1;
            index += 2;
          } else if (line.startsWith("*/", index)) {
            codeDepth -= 1;
            index += 2;
          } else {
            index += 1;
          }
          continue;
        }
        if (docDepth > 0) {
          if (line.startsWith("/*", index)) {
            docDepth += 1;
            text += "/*";
            index += 2;
          } else if (line.startsWith("*/", index)) {
            docDepth -= 1;
            index += 2;
            // Closed the outermost doc comment: whatever follows on this line is code.
            if (docDepth === 0) break;
            text += "*/";
          } else {
            text += line[index];
            index += 1;
          }
          continue;
        }
        // A string or char literal starts here: enter it, so nothing inside is read as a
        // comment. Checked BEFORE the comment markers, because `"// not a comment"` is a string.
        const opener = STRING_OPENER.exec(line.slice(index));
        if (opener) {
          // `opener[2]` is the run of `#` for a raw string; a normal string has none and is
          // marked by `hashes: null` so escape handling can tell the two apart.
          str = (opener[1] ?? "").includes("r")
            ? { hashes: opener[2].length }
            : { hashes: null };
          index += opener[0].length;
          continue;
        }
        if (line[index] === "'") {
          // A char literal, or a lifetime. `'a` is a lifetime and must not open anything; a char
          // literal is `'x'` or an escape, so it is only a literal when a closing quote follows
          // within the next few characters.
          const char = /^'(\\.|[^'\\])'/.exec(line.slice(index));
          index += char ? char[0].length : 1;
          continue;
        }
        // `/**/` is an empty comment, not a doc opener -- check it before `/**`.
        if (line.startsWith("/**/", index)) {
          index += 4;
          continue;
        }
        if (line.startsWith("/**", index) || line.startsWith("/*!", index)) {
          docDepth = 1;
          index += 3;
          continue;
        }
        if (line.startsWith("/*", index)) {
          codeDepth = 1;
          index += 2;
          continue;
        }
        const lineForm = /^(\/\/!|\/\/\/)( ?)(.*)$/.exec(line.slice(index));
        if (lineForm) return lineForm[3];
        // An ordinary `//` comment is not documentation, and neither is code.
        if (line.startsWith("//", index)) return null;
        index += 1;
      }
      // Strip the conventional leading `* ` of a continuation line, keeping the rest verbatim so
      // fence indentation inside a block comment survives.
      const projected = text.replace(/^\s*\*( ?)/, "");
      return projected.trim() === "" && text.trim() === "" ? null : projected;
    });
  }
  return raw;
}

const FENCE = /^(\s{0,3})(`{3,}|~{3,})[ \t]*([^\s`]*)/;

// A closing fence carries no info string -- CommonMark forbids one -- so it must be the marker
// and nothing but whitespace. Reusing FENCE to detect the close was wrong: its info-string group
// is `[^\s`]*`, which matches empty against a line whose marker is followed by a BACKTICK, so a
// content line like "``` `inline` in prose" closed the block early and everything after it went
// unvalidated. That is a silent pass on malformed JSON, which is the failure this gate exists to
// prevent.
const CLOSE_FENCE = /^(\s{0,3})(`{3,}|~{3,})[ \t]*$/;

// The start of a Rust string literal, with every prefix that can carry one: `r`/`b`/`br` and the
// C-string forms `c`/`cr`. Group 2 is the run of `#` that a raw string's terminator must repeat.
const STRING_OPENER = /^(br|rb|cr|rc|r|b|c)?(#*)"/;

/**
 * Splits projected lines into fenced blocks and paragraphs.
 *
 * A paragraph is a run of consecutive readable, non-blank lines outside any fence, broken by a
 * blank line, by a hole (a non-doc line in Rust), and by a markdown heading, which starts a new
 * one. Fence delimiters belong to neither: they are structure, not prose.
 */
export function segment(path, source) {
  const lines = docLines(path, source);
  const fences = [];
  const paragraphs = [];
  let open = null;
  let current = null;

  const endParagraph = () => {
    if (current && current.text.trim() !== "") paragraphs.push(current);
    current = null;
  };

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (line === null) {
      if (open) {
        // A fence that ran into ordinary code was never closed. Report it rather than swallowing
        // the rest of the file into a block that will then "parse" as nothing.
        fences.push({ ...open, unterminated: true });
        open = null;
      }
      endParagraph();
      continue;
    }
    if (open) {
      const close = CLOSE_FENCE.exec(line);
      if (
        close &&
        close[2][0] === open.marker[0] &&
        close[2].length >= open.marker.length
      ) {
        fences.push(open);
        open = null;
      } else {
        open.content.push(line);
      }
      continue;
    }
    const fence = FENCE.exec(line);
    // CommonMark forbids a backtick inside a BACKTICK fence's info string, precisely so that a
    // line of inline code is not mistaken for an opener. Without this check, "```json`text`"
    // opened a `json` block, swallowed the prose after it and reported "block is never closed"
    // -- a false failure, which is the one thing this gate must not produce. A tilde fence has
    // no such restriction, so the rule is marker-specific.
    if (
      fence &&
      fence[2][0] === "`" &&
      line.slice(fence[0].length - fence[3].length).includes("`")
    ) {
      // Not a fence at all: fall through and treat the line as ordinary prose.
    } else if (fence) {
      endParagraph();
      open = {
        path,
        line: i + 1,
        marker: fence[2],
        language: fence[3].toLowerCase(),
        indent: fence[1].length,
        content: [],
        unterminated: false,
      };
      continue;
    }
    if (line.trim() === "" || /^\s{0,3}#{1,6}\s/.test(line)) {
      endParagraph();
      if (line.trim() !== "") {
        // A heading is its own paragraph: it can carry a claim, and it never continues the one
        // above it.
        paragraphs.push({ path, line: i + 1, text: line });
      }
      continue;
    }
    if (current === null) current = { path, line: i + 1, text: line };
    else current.text += `\n${line}`;
  }
  if (open) fences.push({ ...open, unterminated: true });
  endParagraph();
  return { fences, paragraphs };
}

// ---------------------------------------------------------------------------------------------
// Check 1: a fence must be what it claims.
// ---------------------------------------------------------------------------------------------

export const JSON_FAMILY = new Set(["json", "jsonl", "ndjson"]);

/** Trims the common indentation a fence inside a list item carries, then drops trailing blanks. */
function fenceBody(fence) {
  const indent = fence.indent;
  return fence.content.map((line) =>
    line.startsWith(" ".repeat(indent)) ? line.slice(indent) : line,
  );
}

/**
 * A placeholder that a reader would have to edit out before the block parses. Reported by name
 * because `JSON.parse`'s own message for a bare `…` ("Unexpected token") does not say that the
 * block is a schematic wearing a machine-format label, which is the actual defect.
 */
function placeholderHint(text) {
  if (/(:|\[|,)\s*(…|\.\.\.)/.test(text)) {
    return (
      "it contains bare `…`/`...` placeholders in value position. A schematic is not JSON: " +
      "label the block ```text, or write values that parse."
    );
  }
  return null;
}

export function checkFence(fence) {
  const problems = [];
  const at = `${fence.path}:${fence.line}`;
  if (!JSON_FAMILY.has(fence.language)) return problems;
  if (fence.unterminated) {
    problems.push(`${at}: \`\`\`${fence.language} block is never closed.`);
    return problems;
  }
  const body = fenceBody(fence);
  const text = body.join("\n");
  const nonEmpty = body
    .map((line, index) => ({ line, index }))
    .filter((entry) => entry.line.trim() !== "");

  if (nonEmpty.length === 0) {
    problems.push(`${at}: \`\`\`${fence.language} block is empty.`);
    return problems;
  }

  if (fence.language === "json") {
    try {
      JSON.parse(text);
    } catch (error) {
      const hint = placeholderHint(text);
      const linesParseIndividually =
        nonEmpty.length > 1 &&
        nonEmpty.every((entry) => {
          try {
            JSON.parse(entry.line);
            return true;
          } catch {
            return false;
          }
        });
      const detail = hint
        ? ` ${hint}`
        : linesParseIndividually
          ? ` Each of its ${nonEmpty.length} lines parses on its own, so this is a JSON Lines ` +
            "stream, not a JSON document: label it ```jsonl."
          : "";
      problems.push(
        `${at}: \`\`\`json block does not parse as JSON -- ${error.message}.${detail}`,
      );
    }
    return problems;
  }

  // jsonl / ndjson: one document per line, and more than zero of them.
  for (const entry of nonEmpty) {
    try {
      JSON.parse(entry.line);
    } catch (error) {
      const hint = placeholderHint(entry.line);
      problems.push(
        `${at}: \`\`\`${fence.language} line ${fence.line + 1 + entry.index} does not parse as ` +
          `JSON -- ${error.message}.${hint ? ` ${hint}` : ""}`,
      );
    }
  }
  return problems;
}

// ---------------------------------------------------------------------------------------------
// Check 2: a unit conversion must be true under the base it names.
// ---------------------------------------------------------------------------------------------

export const UNITS = {
  kb: { bytes: 1e3, base: "decimal", sibling: "KiB" },
  mb: { bytes: 1e6, base: "decimal", sibling: "MiB" },
  gb: { bytes: 1e9, base: "decimal", sibling: "GiB" },
  tb: { bytes: 1e12, base: "decimal", sibling: "TiB" },
  kib: { bytes: 1024, base: "binary", sibling: "KB" },
  mib: { bytes: 1024 ** 2, base: "binary", sibling: "MB" },
  gib: { bytes: 1024 ** 3, base: "binary", sibling: "GB" },
  tib: { bytes: 1024 ** 4, base: "binary", sibling: "TB" },
};

// A number as prose writes one, and only where a number starts: the `(?<![\w.])` guard stops
// `4096` inside an identifier or a version string from being read as a figure.
const NUMBER = String.raw`(?<![\w.])(?:\d{1,3}(?:,\d{3})+(?:\.\d+)?|\d+(?:\.\d+)?)`;
// Case-insensitive, and the binary spellings come first so `KiB` matches as `kib` rather than
// as `k` followed by stray text. `kb` and `KB` are the same claim, and a gate that matched only
// the canonical casing would carry a hole it could never report -- an unmatched claim is
// indistinguishable from no claim.
const CLAIM = new RegExp(
  String.raw`(${NUMBER})[  ]?(KiB|MiB|GiB|TiB|kB|MB|GB|TB)\b`,
  "gi",
);
const ANCHOR = new RegExp(String.raw`(${NUMBER})[ -]?bytes?\b`, "gi");
const COUNTED = new RegExp(
  String.raw`(${NUMBER})\s*([kKmM])?[ -]?(tests?|files?|cases?|bodies|views?|declarations?|frames?|hooks?|records?|suites?)\b`,
  "g",
);
const SCALE_WORD = /\b(?:a|one)\s+(thousand|million|billion)\b/gi;
const SCALE_NUMBER = new RegExp(
  String.raw`(${NUMBER})[ -](thousand|million|billion)\b`,
  "gi",
);
const SCALES = { thousand: 1e3, million: 1e6, billion: 1e9, k: 1e3, m: 1e6 };
// An author who wrote `~3.6 GB` rounded, and the tolerance doubles to say so.
const APPROXIMATE =
  /(~|≈|\babout\b|\bapprox\w*\b|\broughly\b|\baround\b|\bnearly\b)\s*$/i;

// A *bound* is not an approximation, and treating it as one accepted false claims. `under`,
// `over`, `at most` and `at least` used to sit in APPROXIMATE, which only widened a SYMMETRIC
// tolerance -- so against 3,642,000,000 bytes this gate passed `under 3.6 GB`, `over 3.7 GB`,
// `at most 3.6 GB` and `at least 3.7 GB`, every one of them false. A gate that accepts a false
// claim is worse than one that says nothing, so these are now compared in the direction they
// name.
/** Absorbs float noise, so a long figure is not held to an unreasonably exact conversion. */
const RELATIVE_FLOOR = 0.005;

const UPPER_BOUND =
  /(\bunder\b|\bbelow\b|\bup to\b|\bat most\b|\bno more than\b|\bless than\b|\bfewer than\b|\bwithin\b)\s*$/i;
const LOWER_BOUND =
  /(\bover\b|\babove\b|\bat least\b|\bno less than\b|\bmore than\b|\bgreater than\b)\s*$/i;

/**
 * How close a byte figure has to be, under one base or the other, before this gate treats a claim
 * as *derived from* it. Wider than the gap between the two bases (2.4% at kilo, 4.9% at mega,
 * 7.4% at giga) so a base confusion is always inside it, and narrow enough that an unrelated
 * figure sharing a paragraph is not.
 *
 * Without it the gate would report a coverage figure it has not earned: a one-significant-digit
 * claim like "2 MiB" has a rounding tolerance of ±0.5 MiB -- a quarter of its own value -- so it
 * "agrees with" almost any figure in sight. Those were passes, not false alarms, but a claim
 * counted as checked against a number it was never derived from makes the reported
 * claims-checked count a lie, and this gate's honesty about its own coverage is the reason its
 * skips can be trusted.
 */
export const ASSOCIATION_BAND = 0.12;

/** `"3,642"` -> `3642`. Returns `null` for anything that is not a finite number. */
export function parseFigure(text) {
  const value = Number(String(text).replace(/,/g, ""));
  return Number.isFinite(value) ? value : null;
}

/** Digits after the decimal point as written, which is the precision the author claimed. */
function writtenPrecision(text) {
  const dot = String(text).indexOf(".");
  return dot === -1 ? 0 : String(text).length - dot - 1;
}

/**
 * Half a unit in the claim's last written place, doubled for an explicitly approximate figure,
 * with a 0.5% relative floor. See the header for why this is not a percentage.
 */
export function tolerance(text, value, approximate) {
  const halfUlp = 0.5 * 10 ** -writtenPrecision(text);
  return Math.max(
    halfUlp * (approximate ? 2 : 1),
    Math.abs(value) * RELATIVE_FLOOR,
  );
}

/** Every `N bytes` figure written in the paragraph. These are the only things claims derive from. */
export function anchorsIn(text) {
  const values = new Set();
  for (const match of text.matchAll(ANCHOR)) {
    const value = parseFigure(match[1]);
    if (value !== null && value > 0) values.add(value);
  }
  return [...values];
}

/**
 * Multipliers a derivation in this paragraph could plausibly use: counts of things, never bare
 * numbers. "a million-test monorepo", "24,147-test", "33k bodies", "1M tests". Restricting to
 * counted nouns is what keeps the candidate set small enough that agreement means something.
 */
export function multipliersIn(text) {
  const values = new Set([1]);
  for (const match of text.matchAll(COUNTED)) {
    const value = parseFigure(match[1]);
    if (value === null || value <= 0) continue;
    const scale = match[2] ? SCALES[match[2].toLowerCase()] : 1;
    values.add(value * scale);
  }
  for (const match of text.matchAll(SCALE_WORD))
    values.add(SCALES[match[1].toLowerCase()]);
  for (const match of text.matchAll(SCALE_NUMBER)) {
    const value = parseFigure(match[1]);
    if (value !== null && value > 0)
      values.add(value * SCALES[match[2].toLowerCase()]);
  }
  return [...values];
}

/** Byte totals this paragraph could be talking about: each anchor, scaled by each count. */
export function candidateBytes(text) {
  const anchors = anchorsIn(text);
  const multipliers = multipliersIn(text);
  const values = new Set();
  for (const anchor of anchors) {
    for (const multiplier of multipliers) {
      values.add(anchor * multiplier);
      if (multiplier >= 1000) values.add(anchor / multiplier);
    }
  }
  return [...values];
}

/** Every `N <unit>` claim in the paragraph, with the position that lets us quote it back. */
export function claimsIn(text) {
  const claims = [];
  for (const match of text.matchAll(CLAIM)) {
    const value = parseFigure(match[1]);
    const unit = UNITS[match[2].toLowerCase()];
    if (value === null || value <= 0 || !unit) continue;
    const before = text.slice(Math.max(0, match.index - 24), match.index);
    claims.push({
      text: match[0],
      digits: match[1],
      value,
      unitName: match[2],
      unit,
      approximate: APPROXIMATE.test(before),
      // `'upper'`, `'lower'`, or `null` for a plain equality claim.
      bound: UPPER_BOUND.test(before)
        ? "upper"
        : LOWER_BOUND.test(before)
          ? "lower"
          : null,
    });
  }
  return claims;
}

/**
 * Adjudicates one claim against the byte figures its paragraph offers.
 *
 * Returns `'ok'` when some candidate reproduces it under the base it named, `'skipped'` when no
 * candidate reproduces it under either base -- this gate then has nothing to say and says nothing
 * -- and a problem string when a candidate reproduces it only under the OTHER base, which is the
 * KB-meaning-KiB defect this check exists for.
 *
 * Order matters: the own-base test runs first, so a low-precision claim whose tolerance is wide
 * enough to cover both bases (`2 KB` for 2,048 bytes) is correct rather than ambiguous.
 */
export function adjudicate(claim, candidates) {
  if (typeof claim?.value !== "number" || !Number.isFinite(claim.value)) {
    throw new TypeError(
      `claim value is ${JSON.stringify(claim?.value)}, not a finite number`,
    );
  }
  const tol = tolerance(claim.digits, claim.value, claim.approximate);
  const sibling = UNITS[claim.unit.sibling.toLowerCase()];
  const near = (bytes, unit) =>
    Math.abs(claim.value - bytes / unit.bytes) <=
    claim.value * ASSOCIATION_BAND;
  const usable = candidates.filter(
    (bytes) =>
      typeof bytes === "number" &&
      Number.isFinite(bytes) &&
      bytes > 0 &&
      (near(bytes, claim.unit) || near(bytes, sibling)),
  );

  // A bounded claim is checked in the direction it names, not for equality. The tolerance is
  // still allowed as slack, so `under 3.64 GB` for 3.642 GB passes on rounding while
  // `under 3.6 GB` for the same figure -- 0.042 out, far past the 0.5% relative floor -- does
  // not. Association is unchanged: a bound so loose that no byte figure in the paragraph comes
  // within `ASSOCIATION_BAND` of it is skipped, exactly as a loose equality claim is.
  if (claim.bound) {
    // NOT the half-ulp tolerance. Half an ulp encodes "I rounded this", which is the right
    // allowance for an equality claim and the wrong one for a bound: at `3.6` it is 0.05, wide
    // enough to accept `under 3.6 GB` for 3.642 GB, which is the false claim this branch exists
    // to reject. Only the relative floor is allowed, to absorb float noise rather than a whole
    // rounding step -- so `under 3.64 GB` still passes for 3.642 and `under 3.6 GB` does not.
    const slack = Math.abs(claim.value) * RELATIVE_FLOOR;
    const satisfied = usable.find((bytes) => {
      const actual = bytes / claim.unit.bytes;
      return claim.bound === "upper"
        ? actual <= claim.value + slack
        : actual >= claim.value - slack;
    });
    if (satisfied !== undefined) return { verdict: "ok", bytes: satisfied };
    const witness = usable[0];
    if (witness === undefined) return { verdict: "skipped" };
    const actual = witness / claim.unit.bytes;
    const direction = claim.bound === "upper" ? "at most" : "at least";
    return {
      verdict: "bound-violated",
      bytes: witness,
      message:
        `"${claim.text}" claims ${direction} ${claim.value} ${claim.unitName}, but ` +
        `${witness.toLocaleString("en-US")} bytes is ${actual.toPrecision(4)} ${claim.unitName} ` +
        `(${claim.unit.base}, ${claim.unit.bytes.toLocaleString("en-US")} bytes per unit). ` +
        `Restate the bound, or the figure it is about.`,
    };
  }

  const own = usable.find(
    (bytes) => Math.abs(claim.value - bytes / claim.unit.bytes) <= tol,
  );
  if (own !== undefined) return { verdict: "ok", bytes: own };

  const other = usable.find(
    (bytes) => Math.abs(claim.value - bytes / sibling.bytes) <= tol,
  );
  if (other !== undefined) {
    return {
      verdict: "wrong-base",
      bytes: other,
      message:
        `"${claim.text}" is wrong under the base it names: ${other.toLocaleString("en-US")} bytes ` +
        `is ${(other / claim.unit.bytes).toPrecision(4)} ${claim.unitName} ` +
        `(${claim.unit.base}, ${claim.unit.bytes.toLocaleString("en-US")} bytes per unit). ` +
        `${claim.value} is the ${sibling.base} figure -- write "${claim.value} ${claim.unit.sibling}", ` +
        `or restate it as ${Number((other / claim.unit.bytes).toFixed(writtenPrecision(claim.digits)))} ` +
        `${claim.unitName}.`,
    };
  }
  return { verdict: "skipped" };
}

// ---------------------------------------------------------------------------------------------
// Check 2b -- a byte constant's doc comment is tied to the constant itself.
//
// Check 2 associates a claim with a byte figure written in the same paragraph, and counts the
// claims it cannot associate rather than guessing at them. That honesty has a cost: a claim whose
// source figure is a Rust *constant* has no prose figure to find, so it was skipped forever.
// `src/discovery.rs` says "8 MiB" above `MAX_SOURCE_FILE_BYTES` and `src/modes.rs` says it above
// `MAX_BASELINE_BYTES`; nothing tied either sentence to the literal beneath it, so either could
// have been halved without a gate noticing.
//
// This reads code, not prose, which is why it is a separate mechanism rather than a wider window
// on the paragraph scanner. Two admission rules keep it from inventing associations:
//
// **The constant must be named `*_BYTES`.** Its value is then definitionally a byte count, so a
// unit figure in its doc comment is about bytes rather than about something else the comment
// happens to mention. A `MAX_PROJECTS: usize = 256` whose doc mentions "4 KB" of anything is not
// this mechanism's business.
//
// **Its doc comment must name exactly one unit figure.** A doc comment naming several is left to
// the paragraph scanner, because the constant explains only one of them and this mechanism cannot
// tell which. `MAX_DEPTH_STACK_BUDGET_BYTES` is the measured case and the reason the rule exists:
// its comment names "2 MiB" (a libtest thread's stack), "1,835,008 bytes", "183 KiB" and
// "861,504 bytes", while the constant is half the first of them. Tying every figure in that
// comment to the constant would report the correct prose as wrong.
//
// Where check 2 skips on a mismatch -- no candidate reproduces the claim, so it has nothing to
// say -- this one **fails**. That difference is the whole point: the constant is not a candidate
// among several, it is the source figure, so a claim it does not reproduce is a wrong claim. A
// mechanism that skipped instead could never fail, which is documentation wearing a gate's name.
//
// The admission rules are themselves a silent-weakening path: adding a second figure to
// `discovery.rs`'s comment would quietly drop it out of this check. `FLOORS.constantClaims` is
// what closes that -- the tied count is asserted, not merely reported.
// ---------------------------------------------------------------------------------------------

/** A `*_BYTES` constant is the only kind whose doc-comment unit figure is unambiguously its own. */
const BYTE_CONSTANT =
  /^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?const[ \t]+([A-Z][A-Z0-9_]*_BYTES)[ \t]*:[ \t]*[A-Za-z0-9_]+[ \t]*=[ \t]*([^;]+);/;

/** Lines allowed between a doc comment and the `const` it documents. */
const CONST_ATTRIBUTE = /^[ \t]*#!?\[[^\]]*\][ \t]*$/;

/**
 * Evaluates the literal arithmetic a byte constant is written as: `8 * 1024 * 1024`, `1024 *
 * 1024`, `2_000`, `(4 + 1) * 1024`.
 *
 * Deliberately not `eval`/`Function` over repository text, and deliberately not a regex that
 * accepts whatever it matches: the charset is validated first, then a two-level parser handles
 * `+` and `*` with parentheses. Anything else -- a shift, a `const fn` call, a reference to
 * another constant -- returns `null`, and the constant is left untied rather than guessed at.
 *
 * @returns {number|null} the value, or `null` when the expression is not plain literal arithmetic.
 */
export function evaluateByteExpression(expression) {
  if (typeof expression !== "string") return null;
  const text = expression.replace(/_/g, "").trim();
  if (text === "" || !/^[\d\s()*+]+$/.test(text)) return null;

  const tokens = text.match(/\d+|[()*+]/g);
  if (!tokens) return null;
  let at = 0;

  // sum := product ('+' product)* ; product := atom ('*' atom)* ; atom := digits | '(' sum ')'
  const sum = () => {
    let value = product();
    if (value === null) return null;
    while (tokens[at] === "+") {
      at += 1;
      const right = product();
      if (right === null) return null;
      value += right;
    }
    return value;
  };
  const product = () => {
    let value = atom();
    if (value === null) return null;
    while (tokens[at] === "*") {
      at += 1;
      const right = atom();
      if (right === null) return null;
      value *= right;
    }
    return value;
  };
  const atom = () => {
    const token = tokens[at];
    if (token === undefined) return null;
    if (token === "(") {
      at += 1;
      const inner = sum();
      if (inner === null || tokens[at] !== ")") return null;
      at += 1;
      return inner;
    }
    if (!/^\d+$/.test(token)) return null;
    at += 1;
    return Number(token);
  };

  const value = sum();
  if (value === null || at !== tokens.length) return null;
  return Number.isSafeInteger(value) && value > 0 ? value : null;
}

/**
 * Every `*_BYTES` constant in `source` whose doc comment names exactly one unit figure.
 *
 * @returns {Array<{name: string, bytes: number, claim: object, line: number, docStart: number,
 *   docEnd: number}>} `line` is 1-based and points at the doc line carrying the claim, so a
 *   failure quotes the sentence rather than the declaration.
 */
export function constantUnitTies(path, source) {
  if (!path.endsWith(".rs")) return [];
  const lines = source.split("\n");
  const ties = [];

  let docStart = null;
  let docText = [];

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (/^[ \t]*\/\/[\/!]/.test(line)) {
      if (docStart === null) docStart = i;
      docText.push(line.replace(/^[ \t]*\/\/[\/!]!?/, ""));
      continue;
    }
    if (docStart !== null && CONST_ATTRIBUTE.test(line)) continue;
    if (docStart !== null) {
      const match = line.match(BYTE_CONSTANT);
      if (match) {
        const bytes = evaluateByteExpression(match[2]);
        const claims = claimsIn(docText.join("\n"));
        // Exactly one, per the admission rule above. Zero is the ordinary case (most byte
        // constants explain themselves in bytes); more than one is the ambiguity this refuses.
        if (bytes !== null && claims.length === 1) {
          const claim = claims[0];
          const offset = docText.findIndex((text) => text.includes(claim.text));
          ties.push({
            name: match[1],
            bytes,
            claim,
            docStart: docStart + 1,
            docEnd: i,
            line: docStart + 1 + (offset === -1 ? 0 : offset),
          });
        }
      }
    }
    docStart = null;
    docText = [];
  }
  return ties;
}

/**
 * Adjudicates one claim against the constant that *is* its source figure.
 *
 * Unlike [`adjudicate`], there is no `skipped` verdict and no association band: the tie is already
 * established by the declaration, so the only outcomes are agreement and a named disagreement.
 * The sibling-base message is kept separate from the plain mismatch because they call for
 * different edits -- one is a KB/KiB conflation, the other is a number that drifted.
 */
export function adjudicateAgainstConstant(tie) {
  const { claim, bytes, name } = tie;
  const tol = tolerance(claim.digits, claim.value, claim.approximate);
  const own = bytes / claim.unit.bytes;
  if (Math.abs(claim.value - own) <= tol) return { verdict: "ok", bytes };

  const sibling = UNITS[claim.unit.sibling.toLowerCase()];
  const asSibling = bytes / sibling.bytes;
  if (Math.abs(claim.value - asSibling) <= tolerance(claim.digits, claim.value, claim.approximate))
    return {
      verdict: "wrong-base",
      bytes,
      message:
        `"${claim.text}" is wrong under the base it names: ${name} is ` +
        `${bytes.toLocaleString("en-US")} bytes, which is ${own.toPrecision(4)} ` +
        `${claim.unitName} (${claim.unit.base}, ${claim.unit.bytes.toLocaleString("en-US")} ` +
        `bytes per unit). ${claim.value} is the ${sibling.base} figure -- write ` +
        `"${claim.value} ${claim.unit.sibling}".`,
    };

  return {
    verdict: "mismatch",
    bytes,
    message:
      `"${claim.text}" does not describe ${name}, the constant it documents: ` +
      `${bytes.toLocaleString("en-US")} bytes is ` +
      `${Number(own.toFixed(writtenPrecision(claim.digits)))} ${claim.unitName}. ` +
      "One of the two moved without the other; the doc comment and the literal beneath it are " +
      "the same fact written twice.",
  };
}

// ---------------------------------------------------------------------------------------------
// Driving the checks over the tree.
// ---------------------------------------------------------------------------------------------

/** Tracked markdown and Rust sources, relative to `root`, in a stable order. */
export function trackedDocs(root = ROOT, { run = execFileSync } = {}) {
  const output = run("git", ["ls-files", "-z", "--", "*.md", "*.rs"], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 1 << 24,
  });
  return output.split("\0").filter((path) => path !== "");
}

export function inspect(documents) {
  const problems = [];
  const stats = {
    files: 0,
    fences: 0,
    jsonFamilyFences: 0,
    unitClaims: 0,
    checkedUnitClaims: 0,
    constantClaims: 0,
  };
  const checked = [];
  const skipped = [];

  for (const { path, source } of documents) {
    stats.files += 1;
    const { fences, paragraphs } = segment(path, source);
    for (const fence of fences) {
      stats.fences += 1;
      if (JSON_FAMILY.has(fence.language)) stats.jsonFamilyFences += 1;
      problems.push(...checkFence(fence));
    }

    // Check 2b first, so the paragraph scanner below can tell a claim it could not associate from
    // one the declaration already answered. Reporting the same sentence as both "checked against a
    // constant" and "skipped for want of a byte figure" would make the coverage line -- the number
    // that says how much of this gate is real -- untrue in the direction that flatters it.
    const ties = constantUnitTies(path, source);
    for (const tie of ties) {
      stats.constantClaims += 1;
      const result = adjudicateAgainstConstant(tie);
      const where = `${path}:${tie.line}`;
      checked.push(
        `${where}: ${tie.claim.text} <- ${tie.name} = ${result.bytes.toLocaleString("en-US")} bytes`,
      );
      // Named rather than tested for, so a future verdict cannot be silently ignored.
      if (result.verdict !== "ok") problems.push(`${where}: ${result.message}`);
    }

    for (const paragraph of paragraphs) {
      const claims = claimsIn(paragraph.text);
      if (claims.length === 0) continue;
      const candidates = candidateBytes(paragraph.text);
      for (const claim of claims) {
        const tied = ties.some(
          (tie) =>
            tie.claim.text === claim.text &&
            paragraph.line >= tie.docStart &&
            paragraph.line <= tie.docEnd,
        );
        if (tied) continue;
        stats.unitClaims += 1;
        const result = adjudicate(claim, candidates);
        const where = `${path}:${paragraph.line}`;
        if (result.verdict === "skipped") {
          skipped.push(`${where}: ${claim.text}`);
          continue;
        }
        stats.checkedUnitClaims += 1;
        checked.push(
          `${where}: ${claim.text} <- ${result.bytes.toLocaleString("en-US")} bytes`,
        );
        // Every non-ok, non-skipped verdict is a problem. Listing them by name rather than
        // testing for one keeps a future verdict from being silently ignored.
        if (result.verdict !== "ok")
          problems.push(`${where}: ${result.message}`);
      }
    }
  }
  return { problems, stats, checked, skipped };
}

/**
 * The work-performed floor. Type first, then the bound: `stats.fences < 20` is not a floor when
 * the field is absent, because `undefined < 20` is `false` and the floor passes.
 */
export function workPerformedProblems(stats, floors = FLOORS) {
  const problems = [];
  for (const [name, floor] of Object.entries(floors)) {
    const value = stats?.[name];
    if (!Number.isInteger(value) || value < 0) {
      problems.push(
        `work-performed floor: ${name} is ${JSON.stringify(value)}, not a non-negative integer. ` +
          "The scan did not report what it did, which is never a pass.",
      );
      continue;
    }
    if (value < floor) {
      problems.push(
        `work-performed floor: ${name} = ${value}, below the floor of ${floor}. A documentation ` +
          "gate that found nothing looks exactly like one that found nothing wrong; this is the " +
          "first case. Check that `git ls-files` sees the tree and that the fence and unit " +
          "patterns still match how these documents are written.",
      );
    }
  }
  return problems;
}

export function runCli(
  argv,
  { root = ROOT, read = readFileSync, list = trackedDocs } = {},
) {
  const verbose = argv.includes("--verbose");
  const lines = [];
  const paths = list(root);
  const documents = paths.map((path) => ({
    path,
    source: read(resolve(root, path), "utf8"),
  }));

  const { problems, stats, checked, skipped } = inspect(documents);
  const failures = [...problems, ...workPerformedProblems(stats)];

  if (verbose) {
    lines.push("checked unit claims:");
    for (const entry of checked) lines.push(`  ${entry}`);
    lines.push(
      "unit claims with no byte figure in their paragraph, and no constant that documents them " +
        "(not checked):",
    );
    for (const entry of skipped) lines.push(`  ${entry}`);
  }

  lines.push(
    `\n${stats.files} tracked document(s): markdown files and Rust doc comments\n` +
      `  fences        ${stats.fences} fenced blocks, ${stats.jsonFamilyFences} of them JSON-family ` +
      "(json/jsonl/ndjson), each parsed\n" +
      `  unit claims   ${stats.unitClaims} seen, ${stats.checkedUnitClaims} checked against a byte ` +
      `figure in the same paragraph, ${stats.unitClaims - stats.checkedUnitClaims} skipped for ` +
      "want of one (--verbose lists them)\n" +
      `  byte constants ${stats.constantClaims} doc comment(s) tied to the \`*_BYTES\` constant ` +
      "they document, where a mismatch fails rather than skipping",
  );

  if (failures.length > 0) {
    lines.push("\nFAIL: documentation accuracy");
    for (const failure of failures) lines.push(`  ${failure}`);
    return { status: 1, lines };
  }
  lines.push(
    "\nok: every JSON-family fence parses as the format it advertises, every unit claim this " +
      "gate could tie to a byte figure is correct under the base it names, and every byte " +
      "constant agrees with the unit figure in its own doc comment.",
  );
  return { status: 0, lines };
}

runAsGate(import.meta.url, runCli, "documentation accuracy");
