// Tests for the documentation-accuracy gate.
//
// The rule this file exists under: **no test that cannot fail**. For every check in
// `check-doc-accuracy.mjs` there is a demonstration here of the input that makes it fail, written
// as the defect it was built for rather than as a generic sad path -- a ```json block with a
// trailing comma, a ```json block with an ellipsis, "3.4 GB" for 3,642,000,000 bytes (the exact
// sentence a review caught), and a scan that reaches no files.
//
// The other half is the one that decides whether this gate survives contact with prose: each of
// the unit-claim rules is exercised against figures **already in the tree**, because a rounding
// rule that rejects `947 MB` for 946,814,976 bytes, or `2 KB` for 2,048, is a rule that would be
// switched off inside a week. Those are assertions that the gate stays *quiet*, and they are as
// load-bearing as the ones that assert it speaks up.
import assert from "node:assert/strict";
import test from "node:test";
import {
  ASSOCIATION_BAND,
  FLOORS,
  adjudicate,
  adjudicateAgainstConstant,
  candidateBytes,
  checkFence,
  claimsIn,
  constantUnitTies,
  docLines,
  evaluateByteExpression,
  inspect,
  runCli,
  segment,
  tolerance,
  workPerformedProblems,
} from "./check-doc-accuracy.mjs";

// ---------------------------------------------------------------------------------------------
// Helpers: run one document through the real pipeline, so these tests exercise the segmenter,
// the paragraph window and the adjudicator together rather than a convenient inner function.
// ---------------------------------------------------------------------------------------------

/**
 * Inspect one supplied documentation file and return all accuracy problems reported for it.
 */
function problemsFor(path, source) {
  return inspect([{ path, source }]).problems;
}

/**
 * Inspect a supplied file, assert exactly one documentation problem, and return that problem.
 */
function onlyProblem(path, source) {
  const problems = problemsFor(path, source);
  assert.equal(
    problems.length,
    1,
    `expected exactly one problem, got: ${JSON.stringify(problems, null, 2)}`,
  );
  return problems[0];
}

/** Adjudicates one claim in `text` -- the first one -- against that same text's byte figures. */
function verdictFor(text, index = 0) {
  const claims = claimsIn(text);
  assert.ok(
    claims.length > index,
    `no claim #${index} found in ${JSON.stringify(text)}`,
  );
  return adjudicate(claims[index], candidateBytes(text));
}

// ---------------------------------------------------------------------------------------------
// Check 1 -- a fence must be what it claims.
// ---------------------------------------------------------------------------------------------

test("a ```json fence with a trailing comma fails", () => {
  const problem = onlyProblem(
    "docs/example.md",
    [
      "# Report",
      "",
      "```json",
      "{",
      '  "schemaVersion": "0.1",',
      '  "findings": [],',
      "}",
      "```",
      "",
    ].join("\n"),
  );
  assert.match(
    problem,
    /^docs\/example\.md:3: ```json block does not parse as JSON/,
  );
});

test("a ```json fence with an ellipsis placeholder fails, and says it is a schematic", () => {
  // The defect verbatim: the `--debug-memory` record was published under a ```json label with
  // `…` standing in for the per-field breakdown, so pasting it into a parser failed.
  const problem = onlyProblem(
    "README.md",
    [
      "```json",
      '{"type":"memory","fields":{"body":…,"scalars":0}}',
      "```",
    ].join("\n"),
  );
  assert.match(problem, /```json block does not parse as JSON/);
  assert.match(problem, /bare `…`\/`\.\.\.` placeholders in value position/);
  assert.match(problem, /label the block ```text/);
});

test("an ASCII `...` placeholder fails the same way", () => {
  const problem = onlyProblem(
    "src/reporters/sarif.rs",
    [
      "//! ```json",
      '//! { "runs": [{ "tool": { "driver": { "rules": [ ... ] } } }] }',
      "//! ```",
    ].join("\n"),
  );
  assert.match(problem, /bare `…`\/`\.\.\.` placeholders in value position/);
});

test("a ```json fence carrying two records is named as a JSON Lines stream, not a JSON document", () => {
  const problem = onlyProblem(
    "docs/reports.md",
    [
      "```json",
      '{"type":"finding","rule":"duplicate-test"}',
      '{"type":"summary","errors":1}',
      "```",
    ].join("\n"),
  );
  assert.match(problem, /Each of its 2 lines parses on its own/);
  assert.match(problem, /label it ```jsonl/);
});

test("a ```jsonl fence fails on the one line that does not parse, naming that line", () => {
  const problem = onlyProblem(
    "src/reporters/jsonl.rs",
    [
      "//! ```jsonl",
      '//! {"type":"finding","rule":"duplicate-test"}',
      '//! {"type":"summary","errors":1,}',
      "//! ```",
    ].join("\n"),
  );
  assert.match(problem, /```jsonl line 3 does not parse as JSON/);
});

test("a valid ```json document and a valid ```jsonl stream both pass", () => {
  assert.deepEqual(
    problemsFor(
      "docs/ok.md",
      ["```json", '{"a": [1, 2], "b": null}', "```"].join("\n"),
    ),
    [],
  );
  assert.deepEqual(
    problemsFor(
      "docs/ok.md",
      ["```jsonl", '{"a":1}', "", '{"b":2}', "```"].join("\n"),
    ),
    [],
  );
});

test("an empty JSON-family fence fails rather than vacuously passing", () => {
  assert.match(
    onlyProblem("docs/ok.md", ["```json", "```"].join("\n")),
    /block is empty/,
  );
});

test("an unterminated JSON fence fails rather than swallowing the rest of the file", () => {
  const problem = onlyProblem("docs/ok.md", ["```json", '{"a": 1}'].join("\n"));
  assert.match(problem, /is never closed/);
});

test("fences in other languages are not parsed as JSON", () => {
  assert.deepEqual(
    problemsFor(
      "README.md",
      ["```sh", "vitest-dedup . --reporters json", "```"].join("\n"),
    ),
    [],
  );
  assert.deepEqual(
    problemsFor(
      "README.md",
      ["```text", '{"fields":{"body":…}}', "```"].join("\n"),
    ),
    [],
    "a schematic labelled ```text is prose and makes no machine-format promise",
  );
});

test("a fence indented inside a list item is still found and still checked", () => {
  // `docs/vitest-surface.md` writes fences three spaces in, under numbered list items.
  const problem = onlyProblem(
    "docs/vitest-surface.md",
    ["1. Example:", "", "   ```json", '   {"a": 1,}', "   ```", ""].join("\n"),
  );
  assert.match(problem, /```json block does not parse as JSON/);
});

test("Rust fences are read only inside doc comments, and code cannot continue one", () => {
  const source = [
    "//! ```json",
    '//! {"a": 1}',
    "//! ```",
    "fn f() {",
    '    let s = "```json";',
    '    let t = "{ not json";',
    "}",
    "",
  ].join("\n");
  const { fences } = segment("src/lib.rs", source);
  assert.equal(fences.length, 1);
  assert.equal(fences[0].language, "json");
  assert.deepEqual(problemsFor("src/lib.rs", source), []);
});

// ---------------------------------------------------------------------------------------------
// Check 2 -- a unit conversion must be true under the base it names.
// ---------------------------------------------------------------------------------------------

test('"3.4 GB" for 3,642,000,000 bytes fails: 3.4 is the binary figure', () => {
  // Verbatim reproduction of the sentence a review caught, in the shape it was written in.
  const problem = onlyProblem(
    "README.md",
    "At the measured 3,642 bytes retained per test, a million-test monorepo costs 3.4 GB.",
  );
  assert.match(problem, /"3\.4 GB" is wrong under the base it names/);
  assert.match(problem, /3,642,000,000 bytes is 3\.642 GB/);
  assert.match(
    problem,
    /3\.4 is the binary figure -- write "3\.4 GiB", or restate it as 3\.6 GB/,
  );
});

test('"3.4 GiB" for the same figure passes: the base it names is the one it is correct under', () => {
  assert.equal(
    verdictFor(
      "At the measured 3,642 bytes retained per test, a million-test monorepo costs 3.4 GiB.",
    ).verdict,
    "ok",
  );
});

test('"3.64 GB (3.39 GiB)", as the tree states it, passes under both bases', () => {
  const text =
    "At the measured 3,642 bytes retained per test a million-test monorepo costs 3.64 GB (3.39 GiB).";
  assert.equal(verdictFor(text, 0).verdict, "ok");
  assert.equal(verdictFor(text, 1).verdict, "ok");
  assert.deepEqual(problemsFor("docs/reports.md", text), []);
});

test('"2 KB" for 2,048 bytes passes: 2.048 rounded to the precision written is 2', () => {
  // The example that makes a fixed percentage tolerance unusable: 2.048 vs 2 is 2.4% off, which
  // any percentage tight enough to catch "3.4 GB" would reject.
  const result = verdictFor(
    "The target is 2,048 bytes per test, which is 2 KB per test.",
  );
  assert.equal(result.verdict, "ok");
  assert.equal(result.bytes, 2_048);
});

test('"947 MB" for 946,814,976 bytes is decimally correct and passes', () => {
  // Not reachable from the tree -- the source figure is not written down anywhere, so the gate
  // skips the real sentence -- but the rounding rule itself has to accept it, or the first time
  // someone writes the denominator down this gate turns red on a correct number.
  const result = adjudicate(claimsIn("peak was 947 MB")[0], [946_814_976]);
  assert.equal(result.verdict, "ok");
});

test('"947 MiB" for 946,814,976 bytes fails: that is the decimal figure', () => {
  // The same measurement with the unit swapped. 946,814,976 bytes is 902.96 MiB.
  const result = adjudicate(claimsIn("peak was 947 MiB")[0], [946_814_976]);
  assert.equal(result.verdict, "wrong-base");
  assert.match(result.message, /947 is the decimal figure -- write "947 MB"/);
});

test("a claim no byte figure in its paragraph explains is skipped, not guessed at", () => {
  // `947 MB` as the tree actually writes it: a peak-RSS measurement with no denominator nearby.
  const text =
    "Peak memory was cut 947 MB -> 149 MB on a real 24,147-test codebase.";
  assert.equal(verdictFor(text).verdict, "skipped");
  assert.deepEqual(problemsFor("docs/reports.md", text), []);
});

test("a byte figure in a different paragraph does not reach the claim", () => {
  const source = [
    "The measurement is 3,642 bytes per test.",
    "",
    "A million tests cost 3.4 GB.",
  ].join("\n");
  assert.deepEqual(
    problemsFor("README.md", source),
    [],
    "two paragraphs, so there is nothing to derive from",
  );
});

test("a byte figure in an adjacent table cell does reach the claim", () => {
  // The window has to be the paragraph, not the sentence or the cell: README states the
  // measurement in one cell and derives the projection in the next.
  const source = [
    "| | Number | What it is |",
    "| --- | --- | --- |",
    "| **Measured** | 3,642 bytes/test | A million tests at that rate is 3.4 GB. |",
  ].join("\n");
  assert.match(
    onlyProblem("README.md", source),
    /"3\.4 GB" is wrong under the base it names/,
  );
});

test("an approximate claim gets twice the rounding tolerance, and no more", () => {
  assert.equal(tolerance("3.3", 3.3, false), 0.05);
  assert.equal(tolerance("3.3", 3.3, true), 0.1);
  // 3,230 bytes reads as 3.23 KB: 0.07 from "3.3", outside the exact tolerance and inside the
  // approximate one. Its binary reading, 3.154, is outside both, so this exercises the tolerance
  // and not the wrong-base path.
  const exact = claimsIn("the frame is 3.3 KB")[0];
  assert.equal(exact.approximate, false);
  assert.equal(adjudicate(exact, [3230]).verdict, "skipped");
  const approximate = claimsIn("the measured frame is ~3.3 KB")[0];
  assert.equal(approximate.approximate, true);
  assert.equal(adjudicate(approximate, [3230]).verdict, "ok");
});

test("the 0.5% relative floor keeps a long figure from being held to an exact conversion", () => {
  // 3.64 GB written to two decimals has a half-ulp of 0.005; the floor widens it to 0.0182 so
  // that a figure whose true value is 3.642 is not a failure.
  assert.equal(tolerance("3.64", 3.64, false), 3.64 * 0.005);
});

test("the association band keeps an unrelated figure in the same paragraph out of the count", () => {
  // A one-significant-digit claim has a rounding tolerance a quarter of its own size, so without
  // the band "2 MiB" would count as checked against any figure it happened to share a paragraph
  // with. 1,713,472 bytes is 1.63 MiB -- 18% away, outside the band.
  assert.ok(ASSOCIATION_BAND < 0.18);
  assert.equal(
    adjudicate(claimsIn("the 2 MiB a libtest thread gets")[0], [1_713_472])
      .verdict,
    "skipped",
  );
});

test("the multiplier has to be a count of something, not any number in sight", () => {
  const counted = candidateBytes(
    "3,642 bytes per test across a 24,147-test codebase",
  );
  assert.ok(counted.includes(3642 * 24147));
  const uncounted = candidateBytes(
    "3,642 bytes per test, and the depth limit is 24,147",
  );
  assert.ok(!uncounted.includes(3642 * 24147));
});

test("adjudicate asserts the type before it compares", () => {
  // `undefined <= tolerance` is `false`, so an unvalidated claim would be silently reported as
  // skipped rather than as the parse failure it is.
  assert.throws(
    () =>
      adjudicate(
        { value: undefined, digits: "1", unit: { bytes: 1e3 } },
        [1000],
      ),
    TypeError,
  );
  assert.throws(() => adjudicate(undefined, [1000]), TypeError);
});

// ---------------------------------------------------------------------------------------------
// The work-performed floor.
// ---------------------------------------------------------------------------------------------

test("a scan that reaches no files fails", () => {
  const result = runCli([], {
    list: () => [],
    read: () => assert.fail("nothing should be read"),
  });
  assert.equal(result.status, 1);
  const output = result.lines.join("\n");
  assert.match(output, /FAIL: documentation accuracy/);
  assert.match(
    output,
    new RegExp(`work-performed floor: files = 0, below the floor of ${FLOORS.files}`),
  );
  assert.match(output, /work-performed floor: fences = 0/);
  assert.match(output, /work-performed floor: unitClaims = 0/);
});

test("a scan that reaches files but finds no fences and no unit claims fails", () => {
  const result = runCli([], {
    list: () => Array.from({ length: 40 }, (_, i) => `docs/${i}.md`),
    read: () =>
      "Prose with no fenced blocks and no unit claims in it at all.\n",
  });
  assert.equal(result.status, 1);
  const output = result.lines.join("\n");
  assert.match(
    output,
    new RegExp(`work-performed floor: fences = 0, below the floor of ${FLOORS.fences}`),
  );
  assert.match(
    output,
    new RegExp(`work-performed floor: unitClaims = 0, below the floor of ${FLOORS.unitClaims}`),
  );
  assert.doesNotMatch(output, /work-performed floor: files/);
});

test("every floor is checked for type before it is compared", () => {
  for (const name of Object.keys(FLOORS)) {
    const stats = Object.fromEntries(
      Object.entries(FLOORS).map(([key, value]) => [key, value + 1]),
    );
    delete stats[name];
    const problems = workPerformedProblems(stats);
    assert.equal(problems.length, 1);
    assert.match(
      problems[0],
      new RegExp(`${name} is undefined, not a non-negative integer`),
    );
  }
});

test("the real tree passes, and reports how much of it the unit check could reach", () => {
  const result = runCli([]);
  assert.equal(result.status, 0, result.lines.join("\n"));
  const output = result.lines.join("\n");
  assert.match(output, /unit claims\s+\d+ seen, \d+ checked/);
  assert.match(output, /ok: every JSON-family fence parses/);
});

// ---------------------------------------------------------------------------------------------
// Review findings on PR #13. Each of the three was reproduced before it was fixed, and each case
// below fails against the code as it stood.
// ---------------------------------------------------------------------------------------------

test("a directional qualifier is a bound, not an approximation", () => {
  // `under`/`over`/`at most`/`at least` used to sit in the approximate set, which only widened a
  // SYMMETRIC tolerance -- so every claim here passed against 3,642,000,000 bytes (3.642 GB),
  // and every one of them is false. A gate that accepts a false claim is worse than silence.
  const bytes = [3642000000];
  for (const text of [
    "at 3,642,000,000 bytes that is under 3.6 GB",
    "at 3,642,000,000 bytes that is over 3.7 GB",
    "at 3,642,000,000 bytes that is at most 3.6 GB",
    "at 3,642,000,000 bytes that is at least 3.7 GB",
  ]) {
    const [claim] = claimsIn(text);
    assert.ok(claim, `no claim parsed from: ${text}`);
    assert.notEqual(
      adjudicate(claim, bytes).verdict,
      "ok",
      `must not accept: ${text}`,
    );
  }
  // The same sentences, made true, still pass -- so this rejects falsehood rather than bounds.
  for (const text of [
    "at 3,642,000,000 bytes that is under 3.7 GB",
    "at 3,642,000,000 bytes that is over 3.6 GB",
    "at 3,642,000,000 bytes that is at most 3.7 GB",
    "at 3,642,000,000 bytes that is at least 3.6 GB",
    // On rounding: 3.642 exceeds 3.64 by less than the relative floor.
    "at 3,642,000,000 bytes that is under 3.64 GB",
  ]) {
    const [claim] = claimsIn(text);
    assert.equal(
      adjudicate(claim, bytes).verdict,
      "ok",
      `must accept: ${text}`,
    );
  }
});

test("a content line starting with a fence marker does not close the block", () => {
  // FENCE's info-string group is `[^\s`]*`, which matches EMPTY against a marker followed by a
  // backtick -- so this line closed the block and `{oops` was never validated.
  const source = [
    "```json",
    '{"a": 1}',
    "``` `inline` in prose",
    "{oops not json",
    "```",
  ].join("\n");
  const { fences } = segment("probe.md", source);
  assert.equal(fences.length, 1, "the block is one fence, not two");
  assert.equal(
    checkFence(fences[0]).length,
    1,
    "and its malformed content is reported rather than skipped",
  );
});

test("a block-form Rust doc comment is scanned like a line-form one", () => {
  // `/** */` and `/*! */` projected to null, so every fence and claim inside one was invisible.
  // No block comment exists in the tree today, which is why this had to close before one does.
  const source = [
    "/** Doc block.",
    " *",
    " * ```json",
    " * {bad json here}",
    " * ```",
    " */",
    "pub fn f() {}",
    "/*! One-liner: 2,048 bytes is 2 KiB. */",
  ].join("\n");
  const projected = docLines("x.rs", source);
  assert.ok(
    projected.includes("```json"),
    `a fence inside a block comment must be projected: ${JSON.stringify(projected)}`,
  );
  assert.ok(
    projected.some((line) => line?.includes("2,048 bytes is 2 KiB")),
    "a one-line block comment is projected too",
  );
  const { fences } = segment("x.rs", source);
  assert.equal(
    fences.flatMap(checkFence).length,
    1,
    "and its malformed JSON is reported",
  );
});

// ---------------------------------------------------------------------------------------------
// Second review round on PR #13.
// ---------------------------------------------------------------------------------------------

test("a backtick inside a backtick-fence info string does not open a fence", () => {
  // CommonMark forbids it precisely so inline code is not mistaken for an opener. Treating it as
  // one swallowed the prose after it and reported "block is never closed" -- a FALSE failure,
  // which is the one outcome this gate must never produce.
  const { fences } = segment(
    "p.md",
    ["```json`text`", "", "Ordinary prose, not JSON."].join("\n"),
  );
  assert.equal(fences.length, 0, "inline code is not a fence");

  // The rule is marker-specific: a tilde fence has no such restriction.
  const tilde = segment("p.md", ["~~~json`x`", '{"a": 1}', "~~~"].join("\n"));
  assert.equal(
    tilde.fences.length,
    1,
    "a tilde fence may carry a backtick in its info string",
  );

  // And an ordinary fence is unaffected.
  const plain = segment("p.md", ["```json", '{"a": 1}', "```"].join("\n"));
  assert.equal(plain.fences.length, 1);
  assert.equal(checkFence(plain.fences[0]).length, 0);
});

test("a Rust block comment is tracked by nesting depth, not by the first close", () => {
  // Rust block comments nest. An inner `*/` used to end the outer doc comment, after which the
  // rest projected to null and its fences were skipped entirely.
  const nested = docLines(
    "x.rs",
    [
      "/** Doc.",
      " * /* inner */",
      " * ```json",
      " * {bad}",
      " * ```",
      " */",
    ].join("\n"),
  );
  assert.ok(
    nested.includes("```json"),
    `the fence after a nested comment must survive: ${JSON.stringify(nested)}`,
  );

  // And a comment can close part-way through a line, with code after it. Anchoring on `*/$` left
  // the scanner believing it was still inside, so real Rust downstream was read as prose.
  const midLine = docLines(
    "y.rs",
    ["/** Doc. */ pub fn f() {}", 'let s = "```json";'].join("\n"),
  );
  assert.equal(
    midLine[1],
    null,
    `code after a closed comment is not documentation: ${midLine}`,
  );

  // An ordinary (non-doc) block comment is not documentation either, at any depth.
  const ordinary = docLines(
    "z.rs",
    ["/* plain /* nested */ still plain */", "/// real doc"].join("\n"),
  );
  assert.deepEqual(ordinary, [null, "real doc"]);
});

test("a doc marker inside a Rust string literal is not documentation", () => {
  // The blocking defect. Rust raw strings hold anything, so this is valid source -- and read
  // without lexical context its `///` lines projected as documentation, the fence opened, `{bad}`
  // failed to parse, and the gate rejected a correct file. A gate that fails valid source gets
  // switched off; one that misses a defect only misses a defect.
  //
  // Not introduced by the block-comment scanner: the original line-form-only regex matched `///`
  // inside a raw string in exactly the same way.
  const blocking = [
    'const EXAMPLE: &str = r#"',
    "/// ```json",
    "/// {bad}",
    "/// ```",
    '"#;',
  ];
  const { fences } = segment("x.rs", blocking.join("\n"));
  assert.equal(fences.length, 0, "a fence inside a raw string is not a fence");
  assert.deepEqual(docLines("x.rs", blocking.join("\n")), [
    null,
    null,
    null,
    null,
    null,
  ]);

  // Every string form that can hide one, and the real doc comment after each, which must survive.
  const cases = [
    ["raw", ['let a = r#"', "/// fake", '"#;', "/// real"]],
    ["raw, many hashes", ['let a = r###"', "/// fake", '"###;', "//! real"]],
    ["normal, spanning lines", ['let a = "', "/// fake", '";', "/// real"]],
    ["escaped quote", ['let a = "x\\" /// fake";', "/// real"]],
    ["byte string", ['let a = br#"', "/// fake", '"#;', "/// real"]],
    [
      "block-comment opener in a string",
      ['let a = "/* not a comment";', "/// real"],
    ],
  ];
  for (const [name, lines] of cases) {
    assert.deepEqual(
      docLines("x.rs", lines.join("\n")).filter((line) => line !== null),
      ["real"],
      `${name}: only the real doc comment is documentation`,
    );
  }

  // A lifetime is not a char literal, and mistaking one for an unterminated literal would
  // swallow the rest of the file.
  assert.deepEqual(
    docLines("x.rs", ["fn f<'a>(x: &'a str) {}", "/// real"].join("\n")).filter(
      Boolean,
    ),
    ["real"],
  );
});

// ---------------------------------------------------------------------------------------------
// Check 2b -- a byte constant's doc comment is tied to the constant itself.
//
// The failure this exists for is drift: the doc comment and the literal beneath it are the same
// fact written twice, and nothing made them agree. So the test that matters is the one where they
// disagree, and it is written first.
// ---------------------------------------------------------------------------------------------

test("a doc comment that disagrees with its own byte constant fails", () => {
  const source = [
    "/// Per-file size limit before a discovered file is skipped.",
    "///",
    "/// 8 MiB comfortably covers even large generated test files.",
    "pub const MAX_SOURCE_FILE_BYTES: u64 = 4 * 1024 * 1024;",
  ].join("\n");

  const ties = constantUnitTies("src/discovery.rs", source);
  assert.equal(ties.length, 1, "the constant and its comment are one tie");
  const result = adjudicateAgainstConstant(ties[0]);
  assert.equal(
    result.verdict,
    "mismatch",
    "halving the constant without touching the sentence above it is the drift this catches",
  );
  assert.match(result.message, /MAX_SOURCE_FILE_BYTES/);
  assert.match(result.message, /4,194,304 bytes is 4 MiB/);

  // And it reaches the gate's problem list, not just the adjudicator.
  const { problems } = inspect([{ path: "src/discovery.rs", source }]);
  assert.equal(problems.length, 1, `${JSON.stringify(problems)}`);
  assert.match(problems[0], /^src\/discovery\.rs:3: /, "the failure quotes the sentence, not the declaration");
});

test("a mismatch FAILS rather than skipping -- the difference from check 2", () => {
  // Check 2 skips when no candidate reproduces the claim, because it cannot tell an unassociated
  // figure from a wrong one. Here the declaration establishes the association, so silence would
  // be a check that can never fail. This test is what stops a future simplification from
  // "fixing" the noise by returning `skipped`.
  const wrong = [
    "/// 1 MiB is the ceiling.",
    "const MAX_THING_BYTES: usize = 8 * 1024 * 1024;",
  ].join("\n");
  assert.equal(adjudicateAgainstConstant(constantUnitTies("x.rs", wrong)[0]).verdict, "mismatch");

  // The same figures through check 2's adjudicator: no byte figure is written in the prose, so
  // there is nothing to associate and it stays quiet. Both behaviours are correct for their own
  // mechanism, which is why 2b is a separate one.
  const claim = claimsIn("1 MiB is the ceiling.")[0];
  assert.equal(adjudicate(claim, candidateBytes("1 MiB is the ceiling.")).verdict, "skipped");
});

test("a KB/KiB conflation against a constant is named as one", () => {
  // The figure has to be written precisely enough for the two bases to be distinguishable at all:
  // "8 MB" for 8,388,608 bytes is 8.39 decimally, and half an ulp at one significant figure is
  // 0.5, so it passes -- correctly, because that is what "I rounded this" means. The KB/KiB
  // defect only becomes *detectable* at the precision the original review caught it: the 3.39 /
  // 3.64 pair, where the bases are 7.4% apart and the claim is written to two decimals.
  const source = [
    "/// 3.39 GB of it.",
    "const MAX_THING_BYTES: usize = 3_642_000_000;",
  ].join("\n");
  const result = adjudicateAgainstConstant(constantUnitTies("x.rs", source)[0]);
  assert.equal(result.verdict, "wrong-base", result.message);
  assert.match(result.message, /write "3\.39 GiB"/);

  // The sibling of that same claim is the true one and must stay quiet.
  const right = [
    "/// 3.39 GiB of it.",
    "const MAX_THING_BYTES: usize = 3_642_000_000;",
  ].join("\n");
  assert.equal(
    adjudicateAgainstConstant(constantUnitTies("x.rs", right)[0]).verdict,
    "ok",
  );
});

test("a doc comment naming several figures is left to the paragraph scanner", () => {
  // The measured case: `MAX_DEPTH_STACK_BUDGET_BYTES` documents a 2 MiB libtest stack, then says
  // the constant is *half* of it. Tying every figure in that comment to the constant reports
  // correct prose as wrong, which is how a gate gets switched off.
  const source = [
    "/// Derived from the 2 MiB a `libtest` thread gets by default -- halved, so the recursion",
    "/// may occupy at most half of it, leaving ~183 KiB of room.",
    "#[cfg(test)]",
    "const MAX_DEPTH_STACK_BUDGET_BYTES: usize = 1024 * 1024;",
  ].join("\n");
  assert.deepEqual(constantUnitTies("x.rs", source), [], "two figures, so no tie is claimed");

  // One figure and the same attribute line: the attribute must not break the association.
  const single = [
    "/// 1 MiB of stack.",
    "#[cfg(test)]",
    "const MAX_DEPTH_STACK_BUDGET_BYTES: usize = 1024 * 1024;",
  ].join("\n");
  assert.equal(constantUnitTies("x.rs", single).length, 1, "an attribute between the two is not a gap");
});

test("only `*_BYTES` constants are admitted, and only literal arithmetic", () => {
  const notBytes = ["/// 4 KB of them.", "const MAX_PROJECTS: usize = 256;"].join("\n");
  assert.deepEqual(constantUnitTies("x.rs", notBytes), [], "256 projects is not 4 KB of anything");

  const notLiteral = [
    "/// 8 MiB.",
    "const MAX_THING_BYTES: usize = OTHER_LIMIT * 2;",
  ].join("\n");
  assert.deepEqual(constantUnitTies("x.rs", notLiteral), [], "a reference is not a figure this can read");

  assert.equal(evaluateByteExpression("8 * 1024 * 1024"), 8_388_608);
  assert.equal(evaluateByteExpression("2_000"), 2000);
  assert.equal(evaluateByteExpression("(4 + 1) * 1024"), 5120);
  // A shift is real Rust and is NOT read: accepting only what the parser proves it understands is
  // why a wrong value can never be invented here.
  assert.equal(evaluateByteExpression("1 << 20"), null);
  assert.equal(evaluateByteExpression("compute()"), null);
  assert.equal(evaluateByteExpression("0"), null);
  assert.equal(evaluateByteExpression(""), null);
  assert.equal(evaluateByteExpression(undefined), null);
});

test("markdown is never scanned for constants", () => {
  const source = ["/// 8 MiB.", "const MAX_THING_BYTES: usize = 4 * 1024 * 1024;"].join("\n");
  assert.deepEqual(constantUnitTies("README.md", source), [], "a .md file holds prose, not declarations");
});

test("a tied claim is not also counted as skipped", () => {
  // Reporting one sentence as both "checked against a constant" and "skipped for want of a byte
  // figure" would overstate the gate's coverage in the direction that flatters it.
  const source = [
    "/// Per-file size limit.",
    "///",
    "/// 8 MiB covers it.",
    "pub const MAX_SOURCE_FILE_BYTES: u64 = 8 * 1024 * 1024;",
  ].join("\n");
  const { stats, skipped, checked, problems } = inspect([{ path: "src/discovery.rs", source }]);
  assert.deepEqual(problems, []);
  assert.equal(stats.constantClaims, 1);
  assert.equal(stats.unitClaims, 0, "the tied claim left the paragraph scanner's denominator");
  assert.deepEqual(skipped, []);
  assert.equal(checked.length, 1);
  assert.match(checked[0], /MAX_SOURCE_FILE_BYTES = 8,388,608 bytes/);
});

test("the constant floor fails when the tie count falls", () => {
  // The silent-weakening path: the admission rule is "exactly one unit figure", so adding a
  // second sentence to a tied comment drops it out of the check and leaves a green run behind.
  // The floor is what turns that into a failure.
  assert.equal(FLOORS.constantClaims, 2, "the measured tie count, not a round number");
  const problems = workPerformedProblems({
    files: 40,
    fences: 38,
    jsonFamilyFences: 6,
    unitClaims: 43,
    checkedUnitClaims: 10,
    constantClaims: 1,
  });
  assert.equal(problems.length, 1);
  assert.match(problems[0], /constantClaims = 1, below the floor of 2/);

  // Absent, not merely low: `undefined < 2` is false, which is how a floor passes on a field
  // nobody reported.
  const absent = workPerformedProblems({
    files: 40,
    fences: 38,
    jsonFamilyFences: 6,
    unitClaims: 43,
    checkedUnitClaims: 10,
  });
  assert.equal(absent.length, 1);
  assert.match(absent[0], /constantClaims is undefined, not a non-negative integer/);
});

test("the real tree's byte constants agree with their own doc comments", () => {
  // The gate running over the actual repository, not a fabricated source. If this ever goes red,
  // a doc comment and a constant have drifted apart and that is the finding.
  const { status, lines } = runCli([]);
  assert.equal(status, 0, lines.join("\n"));
  assert.match(lines.join("\n"), /byte constants 2 doc comment\(s\) tied/);
});
