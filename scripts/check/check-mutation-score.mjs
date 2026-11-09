#!/usr/bin/env node
// Mutation-score ratchet over the selected, explicitly bounded Rust scope.
//
// Usage:
//   node scripts/check/check-mutation-score.mjs [outcomes.json] [minimumScore]
//   node scripts/check/check-mutation-score.mjs --run           # measure, then check (npm run mutants)
//   node scripts/check/check-mutation-score.mjs --print-scope   # the --file args for the run
//
// ## What this measures that nothing else here does
//
// At the original measurement, the repository had 468 unit tests and no evidence that they would
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
// cargo-mutants rebuilds and re-runs the suite for each mutant, so each scope below is a
// measured slice with its own floors. Historical whole-tree counts do not describe current scope.
// The earlier analyzer's fingerprint and analysis scope was deleted with it; a rules-layer scope
// waits on a measured run.
// Everything outside `SCOPE` is therefore unmeasured, and stays unmeasured until the scope grows.
//
// `SCOPE` selects the default; named scopes have their own lists and floors. `--run` (what
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
//   - `timeout == 0`. A timeout is an unknown, not a catch, and recursive analysis's depth
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

// The normal forms of the next core, measured as their own scope with their own floors. Selected
// with `--scope=next-core` (`npm run mutants:next-core`), and the default.
export const NEXT_CORE = {
  scope: [
    { file: 'src/normal.rs', minimumMutants: 70 }, // 74 enumerated
    { file: 'src/normal/form.rs', minimumMutants: 122 }, // 129 enumerated
    { file: 'src/normal/identity.rs', minimumMutants: 76 }, // 80 enumerated
    { file: 'src/normal/rows.rs', minimumMutants: 77 }, // 81 enumerated
  ],
  // 2026-10-29: 267 caught, 84 missed, 12 unviable, 0 timeouts in 64 minutes at `--jobs 2`;
  // 267/351 = 76.07%.
  minimumScore: 76.0,
  minimumScored: 320,
}

// The default scope: the normal forms of the next core, the one scope the weekly workflow runs.
export const SCOPE = NEXT_CORE.scope
export const MINIMUM_SCORE = NEXT_CORE.minimumScore
export const MINIMUM_SCORED = NEXT_CORE.minimumScored

// The behavior trace, measured as its own scope for the same reason the normal forms are: it
// feeds NC16 and NC17, whose claims will be exactly as strong as the tests that constrain this
// code. Selected with `--scope=trace` (`npm run mutants:trace`). This scope is manual: the weekly
// workflow runs the next-core scope, not the trace scope.
export const TRACE = {
  // The per-file and viable-count floors sit ~5% under the enumerated counts, as NEXT_CORE's
  // do: a cleanup that removes a surviving mutant raises the score and must not fail the
  // count checks.
  scope: [
    { file: 'src/behavior.rs', minimumMutants: 298 }, // 314 enumerated
    { file: 'src/behavior/dataflow.rs', minimumMutants: 128 }, // 135 enumerated
  ],
  // 2026-11-01: 246 caught, 180 missed, 23 unviable, 0 timeouts in 85 minutes at `--jobs 4`;
  // 246/426 = 57.75%. The survivors are dominated by depth-arithmetic equivalents, which the
  // floor absorbs; the match-arm deletions with observable events are pinned by the trace's
  // controls and can only raise the score.
  minimumScore: 57.7,
  minimumScored: 405,
}

/** The scope `argv` selects, with the floors it is checked against. */
export function selectScope(argv) {
  const named = argv.find((argument) => argument.startsWith('--scope'))
  if (named === undefined) return { scope: SCOPE, minimumScore: MINIMUM_SCORE, minimumScored: MINIMUM_SCORED }
  if (named === '--scope=next-core') return NEXT_CORE
  if (named === '--scope=trace') return TRACE
  assert.fail(`unknown scope ${named}; the named scopes are --scope=next-core and --scope=trace`)
}

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
  const args = [...scopeArguments(selectScope(argv).scope), '--jobs', '2']
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
 * Parses the optional threshold argument. A blank value is NaN, which `checkMutationScore` rejects,
 * rather than the 0 that `Number('')` would give and every score would satisfy.
 *
 * @param {string} raw
 * @returns {number}
 */
function parseThreshold(raw) {
  return raw.trim() === '' ? Number.NaN : Number(raw)
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
  let selected
  try {
    selected = selectScope(argv)
  } catch (error) {
    err.push(error.message)
    return { status: 1, out, err }
  }
  if (argv.includes('--print-scope')) {
    out.push(scopeArguments(selected.scope).join(' '))
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
  const minimumScore = positional[1] === undefined ? selected.minimumScore : parseThreshold(positional[1])
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
    checkScope(JSON.parse(read(join(dirname(path), 'mutants.json'), 'utf8')), selected.scope)
  } catch (error) {
    err.push(`mutation scope check failed: ${error.message}`)
    return { status: 1, out, err }
  }
  let score
  try {
    score = checkMutationScore(report, minimumScore, selected.minimumScored)
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
