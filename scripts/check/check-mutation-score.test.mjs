// Tests for the mutation-score gate.
//
// The gate exists because a passing test suite is not evidence that the tests constrain anything.
// A gate whose own assertions are untested is the same defect one level up, so every guard in
// `check-mutation-score.mjs` is exercised here against a report crafted to violate exactly it --
// and each rejection is matched on its *message*, because five guards that all throw the same
// generic error cannot be told apart by a reader of CI output.
import assert from 'node:assert/strict'
import test from 'node:test'
import {
  MINIMUM_SCORE,
  MINIMUM_SCORED,
  SCOPE,
  TARGET_SCORE,
  checkMutationScore,
  checkScope,
  isScoreableExit,
  runCli,
  scopeArguments,
} from './check-mutation-score.mjs'

// 9 caught / 10 scored = 90%, 2 unviable, one baseline entry plus one per mutant.
/**
 * Build a mutation-report fixture with overridable counters and generated baseline/mutant
 * outcomes when none are supplied.
 */
function report(overrides = {}) {
  const merged = {
    caught: 9,
    missed: 1,
    timeout: 0,
    unviable: 2,
    success: 0,
    total_mutants: 12,
    ...overrides,
  }
  if (!merged.outcomes) {
    merged.outcomes = [
      { scenario: 'Baseline', summary: 'Success' },
      ...Array.from({ length: merged.total_mutants }, () => ({
        scenario: { Mutant: { file: 'src/analysis.rs' } },
        summary: 'CaughtMutant',
      })),
    ]
  }
  return merged
}

const SMALL = 10 // a scored-mutant floor the fixtures above can satisfy

test('accepts the minimum score and excludes unviable mutants from the denominator', () => {
  assert.equal(checkMutationScore(report(), 90, SMALL), 90)
  // Unviable mutants moving does not move the score, only the total.
  assert.equal(checkMutationScore(report({ unviable: 5, total_mutants: 15 }), 90, SMALL), 90)
})

test('rejects a score below the floor, naming both numbers', () => {
  assert.throws(() => checkMutationScore(report({ caught: 8, missed: 2 }), 90, SMALL), {
    message: /mutation score 80\.0% is below 90\.0%/,
  })
})

test('rejects a failed or absent unmutated baseline', () => {
  assert.throws(
    () =>
      checkMutationScore(
        report({ outcomes: [{ scenario: 'Baseline', summary: 'Failure' }] }),
        90,
        SMALL,
      ),
    { message: /unmutated baseline did not complete successfully/ },
  )
  assert.throws(() => checkMutationScore(report({ outcomes: [] }), 90, SMALL), {
    message: /exactly one unmutated baseline/,
  })
  assert.throws(
    () =>
      checkMutationScore(
        report({
          outcomes: [
            { scenario: 'Baseline', summary: 'Success' },
            { scenario: 'Baseline', summary: 'Success' },
          ],
        }),
        90,
        SMALL,
      ),
    { message: /exactly one unmutated baseline/ },
  )
  assert.throws(() => checkMutationScore({ ...report(), outcomes: undefined }, 90, SMALL), {
    message: /carries no outcomes array/,
  })
})

test('rejects untested mutants, which is what --check leaves behind', () => {
  assert.throws(() => checkMutationScore(report({ caught: 8, success: 1 }), 90, SMALL), {
    message: /built but not tested; run cargo-mutants without --check/,
  })
  // The shape a real `--check` run produces: nothing caught, nothing missed, everything Success.
  assert.throws(
    () =>
      checkMutationScore(
        report({ caught: 0, missed: 0, unviable: 0, success: 12, total_mutants: 12 }),
        90,
        SMALL,
      ),
    { message: /built but not tested/ },
  )
})

test('rejects a run with no viable mutants rather than dividing by zero', () => {
  assert.throws(
    () =>
      checkMutationScore(
        report({ caught: 0, missed: 0, timeout: 0, unviable: 12, total_mutants: 12 }),
        90,
        SMALL,
      ),
    { message: /did not produce any viable mutants/ },
  )
})

test('rejects a run narrower than the scope the floor was measured over', () => {
  assert.throws(() => checkMutationScore(report(), 90, 1100), {
    message: /only 10 viable mutants were scored, expected at least 1100/,
  })
})

test('rejects timeouts instead of counting them either way', () => {
  assert.throws(() => checkMutationScore(report({ caught: 9, missed: 0, timeout: 1 }), 90, SMALL), {
    message: /timed-out mutants are not acceptable/,
  })
})

test('rejects counts that do not add up, or are not non-negative integers', () => {
  assert.throws(() => checkMutationScore(report({ total_mutants: 13 }), 90, SMALL), {
    message: /do not match total_mutants/,
  })
  assert.throws(() => checkMutationScore(report({ caught: -1 }), 90, SMALL), {
    message: /caught must be a non-negative integer/,
  })
  assert.throws(() => checkMutationScore(report({ missed: 1.5 }), 90, SMALL), {
    message: /missed must be a non-negative integer/,
  })
  assert.throws(() => checkMutationScore({ outcomes: [] }, 90, SMALL), {
    message: /caught must be a non-negative integer/,
  })
  assert.throws(
    () => checkMutationScore(report({ total_mutants: 12, outcomes: [{ scenario: 'Baseline', summary: 'Success' }] }), 90, SMALL),
    { message: /one entry per mutant plus the baseline/ },
  )
})

test('rejects a nonsensical threshold, so a typo cannot disable the gate', () => {
  assert.throws(() => checkMutationScore(report(), Number.NaN, SMALL), {
    message: /minimumScore must be a percentage/,
  })
  assert.throws(() => checkMutationScore(report(), 101, SMALL), {
    message: /minimumScore must be a percentage/,
  })
  assert.throws(() => checkMutationScore(report(), -1, SMALL), {
    message: /minimumScore must be a percentage/,
  })
})

test('the score is the last thing checked, so a broken report never reports a number', () => {
  // 100% score, failed baseline: the score must not be what decides this.
  assert.throws(
    () =>
      checkMutationScore(
        report({
          caught: 10,
          missed: 0,
          outcomes: [{ scenario: 'Baseline', summary: 'Failure' }],
        }),
        90,
        SMALL,
      ),
    { message: /unmutated baseline/ },
  )
})

const mutants = (counts) =>
  Object.entries(counts).flatMap(([file, n]) => Array.from({ length: n }, () => ({ file })))

test('the scope check accepts the declared scope at or above its floors', () => {
  // Derived from SCOPE, not spelled out: widening the scope used to break this test for no reason
  // other than that it restated the file list. One mutant above each floor is the boundary worth
  // asserting anyway.
  const atFloor = Object.fromEntries(SCOPE.map((e) => [e.file, e.minimumMutants + 1]))
  const counts = checkScope(mutants(atFloor))
  assert.equal(counts.get('src/analysis.rs'), atFloor['src/analysis.rs'])
})

test('the scope check rejects a run that silently shrank or widened', () => {
  const scope = [{ file: 'a.rs', minimumMutants: 3 }]
  assert.throws(() => checkScope(mutants({ 'a.rs': 2 }), scope), {
    message: /a\.rs generated 2 mutants, expected at least 3/,
  })
  assert.throws(() => checkScope(mutants({ 'a.rs': 3, 'b.rs': 1 }), scope), {
    message: /outside the declared scope \(b\.rs\)/,
  })
  assert.throws(() => checkScope([], scope), { message: /mutants\.json is missing or empty/ })
  assert.throws(() => checkScope([{ file: '' }], scope), { message: /carries no source file/ })
})

test('the recorded floor is a ratchet below the recorded target, and the scope is one source', () => {
  assert.ok(MINIMUM_SCORE > 0 && MINIMUM_SCORE <= TARGET_SCORE)
  assert.ok(MINIMUM_SCORED > 0)
  // The workflow interpolates this; if it stops matching SCOPE the run and the gate disagree. The
  // property is the pairing, not the file list — restating the list here only made a widening look
  // like a regression.
  assert.deepEqual(scopeArguments(), SCOPE.flatMap(({ file }) => ['--file', file]))
  assert.ok(SCOPE.length > 0, 'an empty scope would mutate the whole tree, not a subset of it')
  assert.throws(() => scopeArguments([]), { message: /the mutation scope is empty/ })
  assert.deepEqual(
    SCOPE.map(({ file }) => file),
    [...new Set(SCOPE.map(({ file }) => file))],
  )
})

// --- the command line -------------------------------------------------------------------------
//
// `runCli` returns a status and its output lines instead of exiting, so every rejection is
// asserted here rather than only by launching a subprocess. The four demonstrations the gate was
// required to survive -- failed baseline, untested mutants, no viable mutants, score below the
// floor -- are the four cases below, each with its own distinct message.

// The real counts, keyed off SCOPE so that adding a file does not silently leave this fixture
// describing a scope the gate no longer declares.
const REAL_SCOPE_COUNTS = Object.fromEntries(
  SCOPE.map((entry) => [entry.file, entry.minimumMutants + 10]),
)

/**
 * Return a fixture reader that normalizes path separators and throws an ENOENT-shaped error
 * for absent files.
 */
function fakeRead(files) {
  return (path) => {
    const key = path.replaceAll('\\', '/')
    if (!(key in files)) {
      const error = new Error(`ENOENT: no such file or directory, open '${key}'`)
      error.code = 'ENOENT'
      throw error
    }
    return files[key]
  }
}

/** A report shaped like a real one: enough scored mutants to clear MINIMUM_SCORED. */
function fullReport(overrides = {}) {
  const caught = overrides.caught ?? 395
  const missed = overrides.missed ?? 746
  const timeout = overrides.timeout ?? 0
  const unviable = overrides.unviable ?? 30
  const success = overrides.success ?? 0
  const total = caught + missed + timeout + unviable + success
  return {
    caught,
    missed,
    timeout,
    unviable,
    success,
    total_mutants: total,
    outcomes: [
      { scenario: 'Baseline', summary: overrides.baseline ?? 'Success' },
      ...Array.from({ length: total }, () => ({ scenario: {}, summary: 'CaughtMutant' })),
    ],
  }
}

/**
 * Run the mutation gate against in-memory report and mutant-list fixtures, returning its
 * status and report lines.
 */
function cli(reportOverrides, { path = 'mutants.out/outcomes.json', argv = [path] } = {}) {
  const files = {
    [path]: JSON.stringify(fullReport(reportOverrides)),
    'mutants.out/mutants.json': JSON.stringify(mutants(REAL_SCOPE_COUNTS)),
  }
  return runCli(argv, { read: fakeRead(files) })
}

test('the CLI accepts a real-shaped report above the floor', () => {
  const { status, out, err } = cli({})
  assert.equal(status, 0, err.join('\n'))
  assert.match(out[0], /^ok: mutation score 34\.6% >= 34\.6%/)
  assert.match(out[0], /395\/1141 viable mutants caught, 30 unviable, 746 survived/)
  assert.match(out[0], /Target 90\.0%/)
})

test('the CLI rejects a report whose unmutated baseline failed', () => {
  const { status, err } = cli({ baseline: 'Failure' })
  assert.equal(status, 1)
  assert.match(err.join('\n'), /unmutated baseline did not complete successfully/)
})

test('the CLI rejects a report with mutants left untested (success > 0)', () => {
  const { status, err } = cli({ caught: 395, missed: 745, success: 1 })
  assert.equal(status, 1)
  assert.match(err.join('\n'), /built but not tested; run cargo-mutants without --check/)
})

test('the CLI rejects a report with no viable mutants', () => {
  const { status, err } = cli({ caught: 0, missed: 0, unviable: 1171 })
  assert.equal(status, 1)
  assert.match(err.join('\n'), /did not produce any viable mutants/)
})

test('the CLI rejects a report scoring below the floor', () => {
  const { status, err } = cli({ caught: 295, missed: 846 })
  assert.equal(status, 1)
  assert.match(err.join('\n'), /mutation score 25\.9% is below 34\.6%/)
})

test('the CLI rejects a missing report instead of reading it as a pass', () => {
  const { status, err } = runCli(['nowhere/outcomes.json'], { read: fakeRead({}) })
  assert.equal(status, 1)
  assert.match(err.join('\n'), /could not read a mutation report from nowhere\/outcomes\.json/)
})

test('the CLI rejects a report whose mutants do not match the declared scope', () => {
  const files = {
    'r/outcomes.json': JSON.stringify(fullReport({})),
    'r/mutants.json': JSON.stringify(mutants({ 'src/analysis.rs': 265 })),
  }
  const { status, err } = runCli(['r/outcomes.json'], { read: fakeRead(files) })
  assert.equal(status, 1)
  assert.match(err.join('\n'), /fingerprint\.rs generated 0 mutants/)
})

test('the CLI honours an explicit threshold argument', () => {
  assert.equal(cli({}, { argv: ['mutants.out/outcomes.json', '30'] }).status, 0)
  assert.equal(cli({}, { argv: ['mutants.out/outcomes.json', '99'] }).status, 1)
})

test('--print-scope emits exactly the arguments the workflow passes to cargo-mutants', () => {
  const { status, out } = runCli(['--print-scope'])
  assert.equal(status, 0)
  assert.equal(out.join('\n'), scopeArguments().join(' '))
  assert.match(out.join('\n'), /^--file \S+( --file \S+)*$/)
})

test('only exit 0 and exit 2 from cargo-mutants are scoreable', () => {
  assert.equal(isScoreableExit(0), true)
  assert.equal(isScoreableExit(2), true)
  for (const code of [1, 3, 4, null]) assert.equal(isScoreableExit(code), false)
})

test('--run propagates a non-scoreable cargo-mutants exit instead of scoring the leftovers', () => {
  const spawn = (command, args) => {
    assert.equal(command, 'cargo')
    assert.deepEqual(args, ['mutants', ...scopeArguments(), '--jobs', '1'])
    return { status: 4 }
  }
  const { status, err } = runCli(['--run'], { read: fakeRead({}), spawn })
  assert.equal(status, 4)
  assert.match(err.join('\n'), /exited 4, which is not a scoreable outcome/)
})

test('--run reports a cargo-mutants that could not be launched at all', () => {
  const spawn = () => ({ error: new Error('spawn cargo ENOENT') })
  const { status, err } = runCli(['--run'], { read: fakeRead({}), spawn })
  assert.equal(status, 1)
  assert.match(err.join('\n'), /could not run cargo-mutants: spawn cargo ENOENT/)
})

test('--run scores the report it just produced', () => {
  const files = {
    'mutants.out/outcomes.json': JSON.stringify(fullReport({})),
    'mutants.out/mutants.json': JSON.stringify(mutants(REAL_SCOPE_COUNTS)),
  }
  const { status, out } = runCli(['--run'], { read: fakeRead(files), spawn: () => ({ status: 2 }) })
  assert.equal(status, 0)
  assert.match(out[0], /^cargo mutants --file/)
  assert.match(out[1], /^ok: mutation score/)
})
