#!/usr/bin/env node
// Mutation-score ratchet over the two modules where a wrong answer costs the most.
//
// Usage:
//   node scripts/check/check-mutation-score.mjs [outcomes.json] [minimumScore]
//   node scripts/check/check-mutation-score.mjs --run           # measure, then check (npm run mutants)
//   node scripts/check/check-mutation-score.mjs --print-scope   # the --file args for the run
//
// ## What this measures that nothing else here does
//
// This repository has 468 unit tests and, before this gate, no evidence that any of them would
// fail if the code were wrong. That is not a hypothetical worry: this repository has shipped five
// separate checks that passed while doing no work, and several fixes whose only test was the bug
// they fixed. Coverage cannot see either -- a line is "covered" by a test that executes it and
// asserts nothing. A mutation score is the one number
// that fails when a test does not constrain behaviour.
//
// ## The destination, and why this is not it
//
// The bar to reach and exceed is cuke-dedup's, which gates its analysis contracts at **90%**.
// Measured here on 2026-09-18 over the scope below: **34.6%**, 395 of 1,141 viable mutants caught.
// Pinning 90% today would fail on the first run, and `check-duplication.mjs` and
// `check-coverage.mjs` both carry the argument for why that is worse than no gate -- a gate that is red on arrival gets routed around, after which the
// number it guards is worse than unmeasured, because everyone has learned it does not matter.
//
// So this is a ratchet, exactly like the other two: `MINIMUM_SCORE` is what was measured, the gate
// fails on a *regression*, tightening it is a one-line edit, and `TARGET_SCORE` sits next to it so
// the direction is not in doubt. `mutants.out/caught.txt` and `missed.txt` from the last run are
// the authoritative list of which mutants survive.
//
// ## Scope is part of the number, so the gate owns it
//
// A full run is 7,068 mutants (`cargo mutants --list`, 2026-10-24; it listed 6,075 on 2026-10-18,
// and 2,927 on 2026-09-18, when a full run was estimated at ~4h on the development machine, and
// cargo-mutants must rebuild and re-run the suite for each one). The measured scope is the
// fingerprint module's files (except `classes/`) and `src/analysis.rs` -- 1,537 mutants
// enumerated. The 71 minutes at `--jobs 2` below was timed on the 1,171 mutants of the earlier
// scope. The 2026-10-24 hosted attempt at the current scope averaged 46.7s per mutant (median 44s) and was
// cancelled at the workflow's 300-minute cap with 767 of 1,537 done; that run's logs are why
// `.cargo/mutants.toml` narrows the per-mutant test run to the unittest binaries, and
// the measured phase split recorded there is what the two jobs still buy. Those are not an
// arbitrary slice: the
// fingerprint module is where every
// adjudicated false positive has lived and is the weakest
// module in the coverage gate (71.30%), and analysis.rs turns its output into the findings users
// act on. A mutant that survives in either is behaviour the product's verdicts depend on and no
// test pins.
// Everything outside `SCOPE` is therefore unmeasured, and stays unmeasured until the scope grows.
//
// `SCOPE` below is the single source of truth for that scope. `--run` (which is what
// `npm run mutants` and the workflow invoke) builds the cargo-mutants command line from it, and
// `--print-scope` emits the same `--file` arguments for anyone driving cargo-mutants by hand or
// sharding the scope. Nothing repeats the file list.
// A score is meaningless without the scope it was measured over, so the gate refuses a report
// whose mutants do not match `SCOPE` in either direction -- a missing file means the run silently
// shrank, an extra file means it silently widened, and both move the floor's meaning while leaving
// the number looking comparable.
//
// ## A floor on work performed: the structural guards are the point
//
// A mutation gate that reports a score from an empty or half-finished run is worse than none, so
// the score is the *last* thing checked. Before it:
//
//   - every count field is a non-negative integer, and the outcome counts sum to `total_mutants`
//     (a report that does not add up is a schema change, not a result);
//   - `outcomes` holds exactly one `Baseline` entry and it **succeeded**. An unmutated tree whose
//     tests fail makes every mutant "caught" -- a 100% score from a broken build;
//   - `success == 0`. A mutant with a `Success` outcome was built and not tested, which is what
//     `--check` does. A `--check` run of this scope reports 0 caught, 0 missed and would otherwise
//     divide by zero or, worse, be read as a scope with nothing to catch;
//   - `scored > 0`, and `scored` is at least `MINIMUM_SCORED`. cuke-dedup stops at `> 0`; this
//     repository's rule is that a gate floors the *work it performed*, and `> 0` passes a run
//     that matched one file and tested three mutants at 100%. The floor is the measured count minus headroom
//     for mutants that legitimately stop being generated as code is deleted;
//   - `timeout == 0`. A timeout is an unknown, not a catch, and `fingerprint.rs`'s recursion depth
//     guards make a genuine hang plausible. Note the trap this guard sprang on its own first run:
//     cargo-mutants counts a **build** timeout in the same bucket, and at the default
//     `build_timeout_multiplier = 3.0` two mutants timed out mid-build purely because another
//     process was compiling. The fix is in `.cargo/mutants.toml` (multiplier 12.0), not here --
//     the guard is right, the timeout it was tripping over was not a result.
//
// Unviable mutants (they do not compile) are excluded from the denominator, which is the standard
// definition and also the only honest one -- a mutant that cannot be built was never a test of the
// tests.
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { pathToFileURL } from 'node:url'

// The bar. cuke-dedup gates at this; not yet reachable here, so `MINIMUM_SCORE` ratchets toward it.
export const TARGET_SCORE = 90.0

// Measured 2026-09-18 over SCOPE with cargo-mutants 27.1.0: 1,171 mutants, 395 caught, 746
// survived, 30 unviable, 0 timeouts, 71 minutes at `--jobs 2`. 395/1,141 = 34.62%.
//
// Per file, because the total hides the finding: `src/analysis.rs` is **77.55%** (190/245) and
// `src/frontend/fingerprint.rs` is **22.88%** (205/896). The rule layer is tested; the IR producer
// that every rule depends on is very nearly unconstrained by `cargo test`. Widening the floor from
// a per-file pair to a single total was considered and refused for the same reason the coverage
// ratchet is per module -- a total lets the weak module slide while the strong one holds the
// average -- but it is not done *yet*,
// because at 22.88% a per-file floor would be a ratchet on a number nobody should be defending.
// Raise this in a diff; never lower it without recording why.
export const MINIMUM_SCORE = 34.6

// Floor on work performed. 1,141 viable mutants were scored; this floor catches a run that
// silently shrank while leaving room for mutants to disappear as dead code is deleted. It is not
// theoretical: the first attempt at this measurement was interrupted at 693 of 1,171 mutants, and
// its `mutants.out/outcomes.json` parses, adds up, and reports a plausible 46% -- every other
// guard in this file passes it. This floor is the only one that rejects it.
export const MINIMUM_SCORED = 1050

// The measured scope, and the per-file floor on mutants generated from it. A file that stops
// generating mutants has either been gutted or been renamed, and both should fail loudly rather
// than quietly improve the score.
//
// Widened one file at a time, deliberately. The workflow is a single job with a 300-minute cap and
// no sharding, and the 39 unmeasured files hold 5,531 mutants against the 1,537 already in scope --
// adding them all at once would most likely produce a timeout instead of a number, which is the one
// outcome worse than a narrow score.
//
// Adding a file changes what the score MEANS, so the recorded baseline is re-measured with it rather
// than compared against the old one. A score is only comparable to another score over the same scope.
//
// ## Why `vitest_config.rs` is NOT the next file, measured rather than assumed
//
// It was the obvious candidate and the attempt is recorded here because the result is a finding
// about this gate, not about that file. It is the largest of the remainder -- `cargo mutants --list`
// names **548** mutants in it, against the 483 the backlog recorded before the config work landed --
// and it is config PARSING, where a wrong answer is silent: a mis-read config does not crash, it
// produces a confident `Disabled`, or a scope two directories share, and every finding downstream
// inherits that.
//
// A partial local run (106 of its 548 mutants, cargo-mutants 27.1.0, before the machine ran out of
// disk) returned **0 caught, 2 timed out**. Both halves matter:
//
//   - **0 of 106 caught** is not a measurement artifact. `display_path` was mutated to return
//     `String::new()` for every input by hand, and all 900 tests still passed -- so the file really
//     is close to unconstrained, and the survivors are specific and actionable: nothing asserts
//     `display_path`'s output at all; `read_bounded`'s `metadata.len() > MAX_CONFIG_SOURCE_BYTES`
//     survives becoming `>=`, because no test sits exactly on the limit; `mask_non_code`,
//     `skip_trivia` and `literal_bool_at` survive almost every arithmetic and comparison mutation
//     applied to their index handling.
//
//   - **The 2 timeouts are why the file cannot enter SCOPE yet**, and they are not fixable by
//     adding a test. They are `pos += 1` becoming `pos *= 1` inside `skip_trivia`'s two inner
//     scanning loops (`while pos < limit && original[pos] != '\n'` and the block-comment scan). A
//     no-op in the body of a loop whose condition depends on the body making progress is a true
//     infinite loop, so `cargo test` never returns, and a test cannot make a non-terminating
//     program fail fast. `checkOutcomes` rejects any report containing a timeout -- deliberately,
//     see the note on `timeout_multiplier` in `.cargo/mutants.toml` -- so widening to this file
//     would make the gate permanently unable to produce a score rather than produce a worse one.
//
// That leaves a decision rather than a task, and it is recorded here instead of being made
// silently: either the timeout policy changes to count a timed-out mutant as SURVIVED (conservative
// in the right direction -- a hanging mutant is certainly not caught, so counting it missed only
// ever lowers the score, and it stays visible in the survivor list), or every file holding an
// unguarded scanning loop stays out of scope, which is most of the parsing code this gate most
// needs to reach. The second is the status quo and it is why the scope has grown one file at a
// time rather than all at once.
export const SCOPE = [
  // The former single `src/frontend/fingerprint.rs` (1,206 mutants) is now these files. The classes/
  // modules split from `fingerprint/classes.rs` stay out of scope, as that file was.
  { file: 'src/frontend/fingerprint.rs', minimumMutants: 27 }, // 29 enumerated
  { file: 'src/frontend/fingerprint/behavior.rs', minimumMutants: 207 }, // 221 enumerated
  { file: 'src/frontend/fingerprint/behavior_calls.rs', minimumMutants: 78 }, // 84 enumerated
  { file: 'src/frontend/fingerprint/context.rs', minimumMutants: 10 }, // 11 enumerated
  { file: 'src/frontend/fingerprint/dataflow.rs', minimumMutants: 212 }, // 226 enumerated
  { file: 'src/frontend/fingerprint/identity.rs', minimumMutants: 24 }, // 26 enumerated
  { file: 'src/frontend/fingerprint/imports.rs', minimumMutants: 22 }, // 24 enumerated
  { file: 'src/frontend/fingerprint/row_substitution.rs', minimumMutants: 9 }, // 10 enumerated
  { file: 'src/frontend/fingerprint/serializer.rs', minimumMutants: 203 }, // 217 enumerated
  { file: 'src/frontend/fingerprint/serializer_members.rs', minimumMutants: 280 }, // 298 enumerated
  { file: 'src/frontend/fingerprint/text.rs', minimumMutants: 29 }, // 31 enumerated
  { file: 'src/frontend/fingerprint/trivial.rs', minimumMutants: 27 }, // 29 enumerated
  { file: 'src/analysis.rs', minimumMutants: 252 }, // 269 enumerated
]

/**
 * Return cargo-mutants --file arguments for a nonempty scope; reject an empty list to prevent
 * an unintended whole-tree run.
 */
export function scopeArguments(scope = SCOPE) {
  // An empty scope is the one input whose failure mode is inverted: `cargo mutants` with no
  // `--file` mutates the whole tree, so this would not under-measure, it would launch a run far
  // past the job timeout and report nothing at all.
  assert.ok(scope.length > 0, 'the mutation scope is empty; cargo-mutants would mutate every file')
  return scope.flatMap(({ file }) => ['--file', file])
}

/** Asserts the run covered exactly the declared scope, using cargo-mutants' `mutants.json`. */
export function checkScope(mutants, scope = SCOPE) {
  assert.ok(Array.isArray(mutants) && mutants.length > 0, 'mutants.json is missing or empty')
  const counts = new Map()
  for (const mutant of mutants) {
    const file = mutant?.file
    assert.ok(typeof file === 'string' && file.length > 0, 'a mutant carries no source file')
    counts.set(file, (counts.get(file) ?? 0) + 1)
  }
  const declared = new Set(scope.map(({ file }) => file))
  const unexpected = [...counts.keys()].filter((file) => !declared.has(file)).sort()
  assert.deepEqual(
    unexpected,
    [],
    `mutants came from outside the declared scope (${unexpected.join(', ')}); the floor was measured over SCOPE, so update both together`,
  )
  for (const { file, minimumMutants } of scope) {
    const found = counts.get(file) ?? 0
    assert.ok(
      found >= minimumMutants,
      `${file} generated ${found} mutants, expected at least ${minimumMutants}`,
    )
  }
  return counts
}

/**
 * Validate counts/baseline and reject built-but-untested or timed-out mutants, then enforce
 * score and viable-sample floors; return the score percentage.
 */
export function checkMutationScore(report, minimumScore, minimumScored = MINIMUM_SCORED) {
  assert.ok(
    Number.isFinite(minimumScore) && minimumScore >= 0 && minimumScore <= 100,
    'minimumScore must be a percentage',
  )
  for (const field of ['caught', 'missed', 'timeout', 'unviable', 'success', 'total_mutants']) {
    assert.ok(
      Number.isInteger(report[field]) && report[field] >= 0,
      `${field} must be a non-negative integer`,
    )
  }
  assert.equal(
    report.caught + report.missed + report.timeout + report.unviable + report.success,
    report.total_mutants,
    'mutation outcome counts do not match total_mutants',
  )
  assert.ok(Array.isArray(report.outcomes), 'report carries no outcomes array')
  const baselines = report.outcomes.filter((outcome) => outcome?.scenario === 'Baseline')
  assert.equal(baselines.length, 1, 'report does not describe exactly one unmutated baseline')
  assert.equal(baselines[0].summary, 'Success', 'unmutated baseline did not complete successfully')
  assert.equal(
    report.outcomes.length,
    report.total_mutants + 1,
    'outcomes array does not hold one entry per mutant plus the baseline',
  )
  assert.equal(
    report.success,
    0,
    'some mutants were built but not tested; run cargo-mutants without --check',
  )
  const scored = report.caught + report.missed + report.timeout
  assert.ok(scored > 0, 'mutation run did not produce any viable mutants')
  assert.ok(
    scored >= minimumScored,
    `only ${scored} viable mutants were scored, expected at least ${minimumScored}; the run was narrower than the scope the floor was measured over`,
  )
  assert.equal(report.timeout, 0, 'timed-out mutants are not acceptable')
  const score = (report.caught / scored) * 100
  assert.ok(
    score >= minimumScore,
    `mutation score ${score.toFixed(1)}% is below ${minimumScore.toFixed(1)}%`,
  )
  return score
}

/**
 * Which cargo-mutants exit codes leave behind a report this gate is entitled to judge.
 *
 * 0 means every mutant was caught and 2 means some survived -- both are results, and survivors
 * below the ratchet are expected today, so cargo-mutants' own pass/fail is the wrong verdict.
 * Nothing else is a result: 1 is a usage error, 3 means mutants timed out, 4 means the unmutated
 * baseline failed to build or test. Reinterpreting any of those as a score is the failure mode
 * this whole gate exists to prevent, so they propagate.
 */
export function isScoreableExit(code) {
  return code === 0 || code === 2
}

/** Runs cargo-mutants over `SCOPE`. Split out, and the spawn injected, so the tests can drive
 * every exit-code branch without a two-hour build.
 *
 * Two jobs, not one: the 2026-10-24 hosted attempt at this scope averaged 46.7s per mutant (median 44s) over
 * its first 767 and was cancelled at the workflow's 300-minute cap -- a mutant nothing fails on
 * reruns the whole selected suite (narrowed to the unittest binaries by
 * `.cargo/mutants.toml`'s `additional_cargo_test_args`). Each job builds in its own copy, so
 * this doubles build directories; that is the trade, made for a run that can finish at all. */
export function runMutants(argv, { spawn = spawnSync } = {}) {
  const args = [...scopeArguments(), '--jobs', '2']
  const result = spawn('cargo', ['mutants', ...args], {
    stdio: 'inherit',
    env: {
      ...process.env,
      // cuke-dedup gets this from a `[profile.mutants]` section in Cargo.toml; that file is not
      // this gate's to edit, and debuginfo is most of the build time, the link time and the
      // 6.3 GB of `target/debug` that `copy_target = false` exists to avoid duplicating.
      CARGO_PROFILE_DEV_DEBUG: 'none',
      CARGO_PROFILE_TEST_DEBUG: 'none',
    },
  })
  if (result.error) {
    return { status: 1, message: `could not run cargo-mutants: ${result.error.message}` }
  }
  if (!isScoreableExit(result.status)) {
    return {
      status: result.status ?? 1,
      message: `cargo mutants exited ${result.status}, which is not a scoreable outcome`,
    }
  }
  return { status: 0, message: `cargo mutants ${args.join(' ')}` }
}

/**
 * The whole command line, as a pure function of its arguments.
 *
 * It returns a status and the lines to print rather than calling `process.exit` itself, because a
 * gate whose failure paths can only be observed by launching a subprocess is a gate whose failure
 * paths do not get tested -- and the failure paths are the entire product here. This shape lets
 * `check-mutation-score.test.mjs` assert each rejection *and* lets the coverage gate measure the
 * result, instead of the file having to be exempted in `JS_NOT_MEASURED`.
 */
export function runCli(argv, { read = readFileSync, spawn = spawnSync } = {}) {
  const out = []
  const err = []
  if (argv.includes('--print-scope')) {
    out.push(scopeArguments().join(' '))
    return { status: 0, out, err }
  }
  if (argv.includes('--run')) {
    const run = runMutants(argv, { spawn })
    if (run.status !== 0) {
      err.push(run.message)
      return { status: run.status, out, err }
    }
    out.push(run.message)
  }
  const positional = argv.filter((argument) => !argument.startsWith('--'))
  const path = positional[0] || join('mutants.out', 'outcomes.json')
  const minimumScore = Number(positional[1] ?? MINIMUM_SCORE)
  let report
  try {
    report = JSON.parse(read(path, 'utf8'))
  } catch (error) {
    // A missing report must never read as a pass: a gate whose report never arrived measured
    // nothing, and "nothing failed" is not the same claim as "the work happened".
    err.push(`could not read a mutation report from ${path}: ${error.message}`)
    return { status: 1, out, err }
  }
  try {
    checkScope(JSON.parse(read(join(dirname(path), 'mutants.json'), 'utf8')))
  } catch (error) {
    err.push(`mutation scope check failed: ${error.message}`)
    return { status: 1, out, err }
  }
  let score
  try {
    score = checkMutationScore(report, minimumScore)
  } catch (error) {
    err.push(`mutation score check failed: ${error.message}`)
    return { status: 1, out, err }
  }
  const scored = report.caught + report.missed + report.timeout
  out.push(
    `ok: mutation score ${score.toFixed(1)}% >= ${minimumScore.toFixed(1)}% ` +
      `(${report.caught}/${scored} viable mutants caught, ${report.unviable} unviable, ` +
      `${report.missed} survived). Target ${TARGET_SCORE.toFixed(1)}%.`,
  )
  return { status: 0, out, err }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const { status, out, err } = runCli(process.argv.slice(2))
  for (const line of out) console.log(line)
  for (const line of err) console.error(line)
  process.exit(status)
}
