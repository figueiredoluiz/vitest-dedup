// Tests for the recall runner split. The full observe-then-judge pipeline is covered by
// `check-recall.mjs` itself; pinned here are the pure matching helpers (which the gate's failure
// diagnostics and any future evaluator classification share), the observation short-circuits that
// must not run the binary, and the judge's routing of each failure class into `failures`,
// `pendingHeld` or the pending ratchet -- the routing that the old inline loop established and
// the split must not change.
import assert from 'node:assert/strict'
import { chmodSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import {
  describeMatcher,
  findingMatches,
  findingOwners,
  judgeRecallCase,
  newTotals,
  observeRecallCase,
} from './recall-runner.mjs'

/** A one-participant-pair finding shaped like a jsonl finding record. */
function finding(overrides = {}) {
  return {
    type: 'finding',
    rule: 'duplicate-test',
    severity: 'error',
    baselined: false,
    titles: ['suite > a', 'suite > b'],
    locations: [
      { path: 'src/a.test.ts', line: 10 },
      { path: 'src/b.test.ts', line: 20 },
    ],
    ...overrides,
  }
}

/** The observation a judge test starts from: one case, two tests, complete, one finding. */
function observation(overrides = {}) {
  return {
    label: 'sample (cases/sample)',
    caseRoot: '/tmp/cases/sample',
    directoryPresent: true,
    sourceFiles: ['sample.test.ts'],
    records: [
      finding(),
      { type: 'summary', testsAnalyzed: 2, incomplete: false, unresolvedTests: 0 },
    ],
    summary: { type: 'summary', testsAnalyzed: 2, incomplete: false, unresolvedTests: 0 },
    ...overrides,
  }
}

/** The manifest case a judge test starts from, satisfied by {@link observation}. */
function testCase(overrides = {}) {
  return {
    name: 'sample',
    path: 'cases/sample',
    expectedTests: 2,
    expectedIncomplete: false,
    expectedFindings: [
      {
        rule: 'duplicate-test',
        severity: 'error',
        titles: ['suite > a', 'suite > b'],
      },
    ],
    expectedAbsent: [],
    ...overrides,
  }
}

/** A judge invocation context with fresh collectors, returned with the totals object. */
function context() {
  return { failures: [], pendingHeld: [], totals: newTotals() }
}

test('newTotals starts every counter the judge accumulates at zero', () => {
  assert.deepEqual(newTotals(), {
    detected: 0,
    expectedFindings: 0,
    knownMisses: 0,
    knownFalsePositivesTotal: 0,
    ran: 0,
    testsAnalyzed: 0,
  })
  assert.notEqual(newTotals(), newTotals())
})

test('findingMatches compares rule, optional severity, and sorted title and location multisets', () => {
  const expected = {
    rule: 'duplicate-test',
    severity: 'error',
    titles: ['suite > b', 'suite > a'],
    locations: ['src/b.test.ts:20', 'src/a.test.ts:10'],
  }
  assert.equal(findingMatches(finding(), expected), true)
  assert.equal(findingMatches(finding({ rule: 'name-collision' }), expected), false)
  assert.equal(findingMatches(finding({ severity: 'warning' }), expected), false)
  // An unspecified severity widens the match rather than requiring absence.
  assert.equal(findingMatches(finding({ severity: 'warning' }), { rule: 'duplicate-test' }), true)
  assert.equal(
    findingMatches(finding({ titles: ['suite > a', 'suite > c'] }), expected),
    false,
    'title multiset mismatch',
  )
  assert.equal(
    findingMatches(finding({ titles: ['suite > a'] }), expected),
    false,
    'title count mismatch',
  )
  assert.equal(
    findingMatches(
      finding({ locations: [{ path: 'src/a.test.ts', line: 10 }, { path: 'src/c.test.ts', line: 1 }] }),
      expected,
    ),
    false,
    'location multiset mismatch',
  )
})

test('describeMatcher renders the constraints a diagnostic names', () => {
  assert.equal(describeMatcher({ rule: 'duplicate-test' }), 'duplicate-test')
  assert.equal(describeMatcher({ rule: 'duplicate-test', severity: 'error' }), 'duplicate-test [error]')
  assert.equal(
    describeMatcher({
      rule: 'duplicate-test',
      severity: 'error',
      titles: ['suite > a', 'suite > b'],
      locations: ['src/a.test.ts:10'],
    }),
    'duplicate-test [error] {suite > a | suite > b} @ src/a.test.ts:10',
  )
})

test('findingOwners exposes the zero, one and two-owner cases the sweep distinguishes', () => {
  const narrow = { rule: 'duplicate-test', titles: ['suite > a', 'suite > b'] }
  const broad = { rule: 'duplicate-test' }
  assert.deepEqual(findingOwners(finding(), [narrow]), [narrow])
  assert.deepEqual(findingOwners(finding({ rule: 'name-collision' }), [narrow]), [])
  assert.deepEqual(findingOwners(finding(), [narrow, broad]), [narrow, broad])
})

test('observeRecallCase reports a missing directory as a fact and never runs the binary', () => {
  const scratch = mkdtempSync(join(tmpdir(), 'recall-runner-'))
  const observed = observeRecallCase({
    binary: '/nonexistent/vitest-dedup',
    root: scratch,
    testCase: { name: 'gone', path: 'cases/gone' },
  })
  assert.equal(observed.directoryPresent, false)
  assert.deepEqual(observed.sourceFiles, [])
  assert.equal(observed.records, null)
  assert.equal(observed.summary, null)
  assert.equal(observed.label, 'gone (cases/gone)')
})

test('observeRecallCase reports a sourceless directory as a fact and never runs the binary', () => {
  const scratch = mkdtempSync(join(tmpdir(), 'recall-runner-'))
  writeFileSync(join(scratch, 'notes.md'), 'not analyzable source')
  const observed = observeRecallCase({
    binary: '/nonexistent/vitest-dedup',
    root: scratch,
    testCase: { name: 'empty', path: '.' },
  })
  assert.equal(observed.directoryPresent, true)
  assert.deepEqual(observed.sourceFiles, [])
  assert.equal(observed.records, null)
  assert.equal(observed.summary, null)
})

/** A stand-in binary that prints fixed jsonl, then a summary echoing the arguments it received. */
function fakeBinary(directory, { summary = true, exitCode = 0 } = {}) {
  const path = join(directory, 'fake-binary')
  const lines = [
    `printf '%s\\n' '{"type":"finding","rule":"duplicate-test"}'`,
    summary ? `printf '{"type":"summary","testsAnalyzed":2,"args":"%s"}\\n' "$*"` : '',
    `exit ${exitCode}`,
  ]
  writeFileSync(path, `#!/bin/sh\n${lines.join('\n')}\n`)
  chmodSync(path, 0o755)
  return path
}

test('observeRecallCase runs the binary on the case and returns its records and summary', { skip: process.platform === 'win32' }, () => {
  const scratch = mkdtempSync(join(tmpdir(), 'recall-runner-'))
  const caseDir = join(scratch, 'cases', 'nested')
  mkdirSync(join(caseDir, 'deep'), { recursive: true })
  writeFileSync(join(caseDir, 'deep', 'a.spec.mts'), 'export {}')
  const observed = observeRecallCase({
    binary: fakeBinary(scratch, { exitCode: 1 }),
    root: scratch,
    testCase: { name: 'nested', path: 'cases/nested', arguments: ['--min-lines', '3'] },
  })
  assert.equal(observed.label, 'nested (cases/nested)')
  assert.equal(observed.caseRoot, caseDir)
  assert.equal(observed.directoryPresent, true)
  assert.equal(observed.sourceFiles.length, 1, 'a source file in a subdirectory counts')
  assert.deepEqual(
    observed.records.map((record) => record.type),
    ['finding', 'summary'],
  )
  assert.equal(observed.summary, observed.records[1])
  assert.equal(observed.summary.args, `${caseDir} --min-lines 3 --reporters jsonl`)
})

test('observeRecallCase returns a null summary when the run wrote none', { skip: process.platform === 'win32' }, () => {
  const scratch = mkdtempSync(join(tmpdir(), 'recall-runner-'))
  writeFileSync(join(scratch, 'a.test.ts'), 'export {}')
  const observed = observeRecallCase({
    binary: fakeBinary(scratch, { summary: false }),
    root: scratch,
    testCase: { name: 'quiet', path: '.' },
  })
  assert.equal(observed.records.length, 1)
  assert.equal(observed.summary, null)
})

test('observeRecallCase propagates an operational run failure instead of swallowing it', () => {
  const scratch = mkdtempSync(join(tmpdir(), 'recall-runner-'))
  writeFileSync(join(scratch, 'a.test.ts'), 'export {}')
  assert.throws(
    () =>
      observeRecallCase({
        binary: '/nonexistent/vitest-dedup',
        root: scratch,
        testCase: { name: 'runs', path: '.' },
      }),
    /ENOENT|vitest-dedup/,
    'a missing binary is an operational failure the gate dies on, never "zero findings"',
  )
})

test('judgeRecallCase fails a missing directory operationally and counts nothing', () => {
  const judged = context()
  judgeRecallCase(
    testCase(),
    observation({ directoryPresent: false, records: null, summary: null }),
    judged,
  )
  assert.deepEqual(judged.failures, [
    'FAIL sample (cases/sample): case directory is missing. A vanished case cannot fail its own expectations, so it has to fail here',
  ])
  assert.equal(judged.totals.ran, 0)
  assert.equal(judged.totals.testsAnalyzed, 0)
})

test('judgeRecallCase fails a missing summary record without counting the case', () => {
  const judged = context()
  judgeRecallCase(testCase(), observation({ summary: null }), judged)
  assert.deepEqual(judged.failures, ['FAIL sample (cases/sample): run produced no summary record'])
  assert.equal(judged.totals.ran, 0)
})

test('judgeRecallCase rejects a summary whose testsAnalyzed is not a countable integer', () => {
  const judged = context()
  const bad = { type: 'summary', testsAnalyzed: 'two', incomplete: false }
  judgeRecallCase(testCase(), observation({ summary: bad, records: [bad] }), judged)
  assert.equal(judged.failures.length, 1)
  assert.match(judged.failures[0], /no non-negative integer `testsAnalyzed`/)
  assert.equal(judged.totals.ran, 0)
})

test('judgeRecallCase counts a satisfied case into every total', () => {
  const judged = context()
  judgeRecallCase(testCase(), observation(), judged)
  assert.deepEqual(judged.failures, [])
  assert.deepEqual(judged.pendingHeld, [])
  assert.equal(judged.totals.ran, 1)
  assert.equal(judged.totals.testsAnalyzed, 2)
  assert.equal(judged.totals.expectedFindings, 1)
  assert.equal(judged.totals.detected, 1)
})

test('judgeRecallCase holds a pending case whose assertions still fail', () => {
  const judged = context()
  judgeRecallCase(
    testCase({ pending: 'the extraction floor this case waits on is still not implemented' }),
    observation({ summary: { type: 'summary', testsAnalyzed: 3, incomplete: false } }),
    judged,
  )
  assert.deepEqual(judged.failures, [])
  assert.equal(judged.pendingHeld.length, 1)
  assert.match(judged.pendingHeld[0], /^sample \(cases\/sample\): 1 assertion\(s\) still failing/)
  // A pending case is excluded from the recall figure: declared but not counted as desired.
  assert.equal(judged.totals.expectedFindings, 0)
  assert.equal(judged.totals.detected, 0)
})

test('judgeRecallCase fires the pending ratchet when every assertion passes', () => {
  const judged = context()
  judgeRecallCase(
    testCase({ pending: 'the extraction floor this case waits on is still not implemented' }),
    observation(),
    judged,
  )
  assert.deepEqual(
    judged.failures,
    [
      'FAIL sample (cases/sample): pending case now satisfies every declared expectation -- remove the `pending` field so the expectations become hard assertions',
    ],
  )
})

test('judgeRecallCase fails an expectation that matches no finding exactly once', () => {
  const judged = context()
  const absentTitles = { rule: 'duplicate-test', severity: 'error', titles: ['other > pair'] }
  judgeRecallCase(
    testCase({ expectedFindings: [testCase().expectedFindings[0], absentTitles] }),
    observation(),
    judged,
  )
  assert.equal(judged.failures.length, 2, 'the mismatch plus the unclassified sweep both fire')
  assert.match(judged.failures[0], /expected exactly 1 duplicate-test \[error\] \{other > pair\}, got 0/)
  // The one active finding IS claimed by the surviving expectation, so the sweep's count
  // mismatch reports overlap-shaped arithmetic, not a stranger finding -- the same note the
  // gate's inline loop printed for this shape.
  assert.match(judged.failures[1], /\(none -- a declared matcher matched twice\)/)
})

test('judgeRecallCase names a stranger finding the sweep cannot claim', () => {
  const judged = context()
  judgeRecallCase(testCase({ expectedFindings: [] }), observation(), judged)
  assert.deepEqual(judged.failures, [
    'FAIL sample (cases/sample): 1 active finding(s) against 0 declared and 0 classified as ' +
      'known false positive(s). Unclassified: duplicate-test [error] {suite > a | suite > b}',
  ])
  assert.equal(judged.totals.detected, 0)
})

test('judgeRecallCase names a matcher that matched twice when nothing is unclaimed', () => {
  const judged = context()
  const twice = finding()
  const records = [
    finding(),
    twice,
    { type: 'summary', testsAnalyzed: 2, incomplete: false },
  ]
  // One declared matcher whose titles match BOTH active findings: 2 != 1 fires the expectation
  // message, and the sweep finds nothing unclaimed -- the count mismatch is overlap, not a
  // stranger finding.
  const wide = { rule: 'duplicate-test', severity: 'error', titles: ['suite > a', 'suite > b'] }
  judgeRecallCase(
    testCase({ expectedFindings: [wide] }),
    observation({ records, summary: records[2] }),
    judged,
  )
  assert.equal(judged.failures.length, 2)
  assert.match(judged.failures[1], /\(none -- a declared matcher matched twice\)/)
})

test('judgeRecallCase fires the known-false-positive ratchet when the entry matches nothing', () => {
  const judged = context()
  judgeRecallCase(
    testCase({
      knownFalsePositives: [
        {
          id: 'fp-gone',
          reason: 'the over-broad matcher this entry classified is believed fixed or narrowed',
        },
      ],
    }),
    observation(),
    judged,
  )
  assert.equal(judged.failures.length, 1)
  assert.match(judged.failures[0], /known false positive fp-gone is no longer reported/)
  // The ratchet-firing entry still counts against the manifest baseline, so the baseline
  // mismatch the operator sees names the stale manifest, not a silent drop.
  assert.equal(judged.totals.knownFalsePositivesTotal, 1)
})

test('judgeRecallCase claims findings for a satisfied known-false-positive entry', () => {
  const judged = context()
  const entry = {
    id: 'fp-here',
    reason: 'the over-broad matcher this entry classified is believed fixed or narrowed',
    rule: 'duplicate-test',
    titles: ['suite > a', 'suite > b'],
  }
  judgeRecallCase(
    testCase({ expectedFindings: [], knownFalsePositives: [entry] }),
    observation(),
    judged,
  )
  assert.deepEqual(judged.failures, [], 'the claimed finding is classified, not unclassified')
  assert.equal(judged.totals.knownFalsePositivesTotal, 1)
})

test('judgeRecallCase fires both known-miss assertions when a miss is fixed', () => {
  const records = [
    finding(),
    finding({ rule: 'name-collision', titles: ['x > y', 'x > z'] }),
    { type: 'summary', testsAnalyzed: 2, incomplete: false },
  ]
  const entry = {
    id: 'miss-fixed',
    reason: 'a considered guess at the shape of a finding the analyzer does not emit today',
    rule: 'name-collision',
    titles: ['x > y', 'x > z'],
  }

  // The rule is also expected in this case, so the stronger claims-nothing assertion stays
  // silent; only the "is now reported" ratchet fires, and the sweep stays balanced because the
  // finding satisfies the expectation.
  const alsoExpected = context()
  judgeRecallCase(
    testCase({
      expectedFindings: [
        testCase().expectedFindings[0],
        { rule: 'name-collision', severity: 'error', titles: ['x > y', 'x > z'] },
      ],
      knownMisses: [entry],
    }),
    observation({ records, summary: records[2] }),
    alsoExpected,
  )
  assert.deepEqual(
    alsoExpected.failures.map((failure) => failure.includes('miss-fixed')),
    [true],
  )
  assert.equal(alsoExpected.totals.knownMisses, 1)
  assert.equal(alsoExpected.totals.detected, 2)

  // Nothing else expected from the rule: both miss assertions fire, plus the sweep, because the
  // finding contradicts the miss and no expectation claims it.
  const notExpected = context()
  judgeRecallCase(
    testCase({ knownMisses: [entry] }),
    observation({ records, summary: records[2] }),
    notExpected,
  )
  assert.equal(notExpected.failures.length, 3)
  assert.match(notExpected.failures[0], /known miss miss-fixed is now reported/)
  assert.match(notExpected.failures[1], /claims name-collision reports nothing here/)
  assert.match(notExpected.failures[2], /Unclassified: name-collision/)
  assert.equal(notExpected.totals.knownMisses, 1)
})

test('judgeRecallCase explains an incomplete-run mismatch in the direction that was claimed', () => {
  const complete = context()
  judgeRecallCase(
    testCase({ expectedIncomplete: true }),
    observation(),
    complete,
  )
  assert.match(complete.failures[0], /expected incomplete=true, got false\. .*a complete run means the pair WAS compared/)

  const incomplete = context()
  const summary = { type: 'summary', testsAnalyzed: 2, incomplete: true, unresolvedTests: 0 }
  judgeRecallCase(testCase(), observation({ summary, records: [finding(), summary] }), incomplete)
  assert.match(incomplete.failures[0], /expected incomplete=false, got true\. .*an incomplete run means something was excluded/)
})

test('judgeRecallCase names a missing unresolved count rather than printing undefined', () => {
  const judged = context()
  const summary = { type: 'summary', testsAnalyzed: 2, incomplete: false }
  judgeRecallCase(
    testCase({ expectedUnresolved: 1 }),
    observation({ summary, records: [finding(), summary] }),
    judged,
  )
  assert.deepEqual(judged.failures, [
    'FAIL sample (cases/sample): expected 1 unresolved test(s), got <missing>',
  ])
})

test('judgeRecallCase fails a known false positive whose count differs from expectCount', () => {
  const entry = { id: 'fp', rule: 'duplicate-test', titles: ['suite > a', 'suite > b'] }
  const implicit = context()
  judgeRecallCase(
    testCase({ expectedFindings: [], knownFalsePositives: [entry] }),
    observation({ records: [finding(), finding(), observation().summary] }),
    implicit,
  )
  assert.match(implicit.failures[0], /known false positive fp matched 2 active finding\(s\), expected 1\. Tighten/)
  assert.doesNotMatch(implicit.failures[0], /as declared by/)

  const declared = context()
  judgeRecallCase(
    testCase({ expectedFindings: [], knownFalsePositives: [{ ...entry, expectCount: 2 }] }),
    observation({ records: [finding(), observation().summary] }),
    declared,
  )
  assert.match(declared.failures[0], /matched 1 active finding\(s\), expected 2 as declared by .expectCount./)
})

test('judgeRecallCase reports an expectedAbsent match unless a known false positive claims it', () => {
  const absent = { rule: 'duplicate-test', titles: ['suite > a', 'suite > b'] }
  const reported = context()
  judgeRecallCase(testCase({ expectedAbsent: [absent] }), observation(), reported)
  assert.match(reported.failures[0], /must NOT be reported, got 1\. This is a precision regression/)

  const claimed = context()
  judgeRecallCase(
    testCase({
      expectedFindings: [],
      expectedAbsent: [absent],
      knownFalsePositives: [{ id: 'fp', ...absent }],
    }),
    observation(),
    claimed,
  )
  assert.deepEqual(claimed.failures, [])
})
