// Tests for the recall label adapter. Counts over the real manifest are pinned so a case dropped
// from the adapter's view cannot shrink the population unnoticed.
import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import {
  EXPECTATION_GROUPS,
  blindRecallFiles,
  buildRecallLabels,
  readRecallManifest,
  summarizeLabels,
} from './corpus-labels.mjs'

const REAL_ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', 'fixtures', 'recall')

const labels = { origin: 'synthetic', verdict: 'generated', classifiedAt: '2026-10-20' }

/** A case with one expected finding and one expected absence. */
function makeCase(name, overrides = {}) {
  return {
    name,
    path: name,
    rule: 'duplicate-test',
    role: 'positive',
    ...labels,
    expectedTests: 2,
    expectedIncomplete: false,
    expectedFindings: [{ rule: 'duplicate-test', severity: 'error', titles: ['a', 'b'] }],
    expectedAbsent: [{ rule: 'duplicate-test-name', titles: ['a', 'b'] }],
    ...overrides,
  }
}

/** A schema-version-1 manifest holding exactly the given cases. */
function manifestOf(...cases) {
  return { schemaVersion: 1, cases }
}

/** Runs the adapter and returns the thrown message, failing the test if nothing is thrown. */
function problemsOf(manifest) {
  try {
    buildRecallLabels(manifest)
  } catch (error) {
    return error.message
  }
  return assert.fail('expected the adapter to throw')
}

test('ids are <case>#<n>, counted across the four groups in order', () => {
  const reason = 'r'.repeat(40)
  const testCase = makeCase('c1', {
    knownMisses: [{ id: 'm1', reason, rule: 'duplicate-test', severity: 'error', titles: ['x', 'y'] }],
    knownFalsePositives: [
      { id: 'f1', reason, rule: 'near-duplicate-test', severity: 'warning', titles: ['x', 'y'], expectCount: 2 },
    ],
  })
  const { expectations } = buildRecallLabels(manifestOf(testCase))
  assert.deepEqual(
    expectations.map((e) => [e.id, e.kind, e.outcome]),
    [
      ['c1#1', 'expected-finding', 'finding'],
      ['c1#2', 'expected-absent', 'silence'],
      ['c1#3', 'known-miss', 'finding'],
      ['c1#4', 'known-false-positive', 'silence'],
    ],
  )
  assert.equal(expectations[2].knownId, 'm1')
  assert.equal(expectations[2].reason, reason)
  assert.equal(expectations[3].expectCount, 2)
  assert.equal(expectations[0].knownId, null)
  assert.equal(expectations[0].severity, 'error')
  assert.equal(expectations[1].severity, null)
})

test('case labels and the case-level labels on each expectation carry through', () => {
  const pending = 'waits on a named defect in the extractor'
  const testCase = makeCase('c1', {
    origin: 'regression',
    verdict: 'reviewed',
    classifiedAt: '2026-01-02',
    pending,
    expectedUnresolved: 3,
    arguments: ['--rule', 'subsumed-test=warning'],
    role: 'variant',
  })
  const result = buildRecallLabels(manifestOf(testCase))
  assert.deepEqual(result.cases, [
    {
      name: 'c1',
      path: 'c1',
      rule: 'duplicate-test',
      role: 'variant',
      origin: 'regression',
      verdict: 'reviewed',
      classifiedAt: '2026-01-02',
      pending,
      expectedTests: 2,
      expectedIncomplete: false,
      expectedUnresolved: 3,
      arguments: ['--rule', 'subsumed-test=warning'],
    },
  ])
  for (const e of result.expectations) {
    assert.equal(e.role, 'variant')
    assert.equal(e.origin, 'regression')
    assert.equal(e.verdict, 'reviewed')
    assert.equal(e.classifiedAt, '2026-01-02')
    assert.equal(e.pending, pending)
  }
  const plain = buildRecallLabels(manifestOf(makeCase('p'))).cases[0]
  assert.equal(plain.pending, null)
  assert.equal(plain.expectedUnresolved, null)
  assert.deepEqual(plain.arguments, [])
})

test('an empty or absent cases list is refused, never labelled as zero', () => {
  for (const manifest of [undefined, null, {}, { cases: [] }, { cases: 'x' }]) {
    assert.throws(() => buildRecallLabels(manifest), /no cases/)
  }
})

test('malformed label fields fail', () => {
  assert.match(problemsOf(manifestOf(makeCase('c', { origin: undefined }))), /case c: `origin`/)
  assert.match(problemsOf(manifestOf(makeCase('c', { verdict: 'x' }))), /case c: `verdict`/)
  assert.match(problemsOf(manifestOf(makeCase('c', { classifiedAt: '2026-02-30' }))), /case c: `classifiedAt`/)
  assert.match(problemsOf(manifestOf(makeCase('c', { surprise: 1 }))), /unknown key `surprise`/)
})

test('malformed case structure fails, each field on its own', () => {
  const cases = [
    [{ rule: 'nope' }, /unknown rule "nope"/],
    [{ role: 'neutral' }, /role must be/],
    [{ expectedTests: 0 }, /expectedTests/],
    [{ expectedTests: NaN }, /expectedTests/],
    [{ expectedTests: undefined }, /expectedTests/],
    [{ expectedIncomplete: undefined }, /expectedIncomplete/],
    [{ expectedUnresolved: -1 }, /expectedUnresolved/],
    [{ pending: '' }, /pending/],
    [{ arguments: 'x' }, /arguments/],
    [{ expectedFindings: undefined }, /`expectedFindings` must be an array/],
    [{ expectedAbsent: {} }, /`expectedAbsent` must be an array/],
    [{ knownMisses: 'x' }, /`knownMisses` must be an array/],
    [{ name: '' }, /`name` must be a non-empty string/],
  ]
  for (const [overrides, pattern] of cases) {
    assert.match(problemsOf(manifestOf(makeCase('c', overrides))), pattern, JSON.stringify(overrides))
  }
})

test('malformed matchers fail', () => {
  const finding = (overrides) =>
    makeCase('c', {
      expectedFindings: [{ rule: 'duplicate-test', severity: 'error', titles: ['a', 'b'], ...overrides }],
    })
  assert.match(problemsOf(manifestOf(finding({ rule: 'nope' }))), /c#1 expectedFindings: unknown rule/)
  assert.match(problemsOf(manifestOf(finding({ titles: undefined }))), /needs `titles` or `locations`/)
  assert.match(problemsOf(manifestOf(finding({ titles: [] }))), /`titles` must be a non-empty array/)
  assert.match(problemsOf(manifestOf(finding({ titles: ['a', ''] }))), /`titles` must be a non-empty array/)
  assert.match(problemsOf(manifestOf(finding({ locations: 'x:1' }))), /`locations` must be a non-empty array/)
  assert.match(problemsOf(manifestOf(finding({ severity: undefined }))), /must declare its `severity`/)
  assert.match(problemsOf(manifestOf(makeCase('c', { expectedFindings: [null] }))), /a matcher must be an object/)
  const absentWithSeverity = makeCase('c', {
    expectedAbsent: [{ rule: 'duplicate-test', severity: 'error', titles: ['a', 'b'] }],
  })
  assert.match(problemsOf(manifestOf(absentWithSeverity)), /must not declare a severity/)
  const knownWithoutSeverity = makeCase('c', {
    knownFalsePositives: [{ id: 'f', reason: 'r', rule: 'duplicate-test', titles: ['a', 'b'] }],
  })
  assert.match(problemsOf(manifestOf(knownWithoutSeverity)), /c#3 knownFalsePositives: an asserted finding/)
})

test('known entries need an id, a real reason and a sane expectCount, with one id namespace', () => {
  const known = (overrides) => ({
    id: 'k1',
    reason: 'r'.repeat(40),
    rule: 'duplicate-test',
    severity: 'error',
    titles: ['x', 'y'],
    ...overrides,
  })
  const withMiss = (entry) => manifestOf(makeCase('c', { knownMisses: [entry] }))
  assert.doesNotThrow(() => buildRecallLabels(withMiss(known({ expectCount: 1 }))))
  assert.match(problemsOf(withMiss(known({ id: undefined }))), /every known entry needs an `id`/)
  assert.match(problemsOf(withMiss(known({ reason: 'r'.repeat(39) }))), /reason` of 40\+ characters/)
  assert.match(problemsOf(withMiss(known({ reason: undefined }))), /reason` of 40\+ characters/)
  for (const bad of [0, 1.5, NaN, '2']) {
    assert.match(problemsOf(withMiss(known({ expectCount: bad }))), /expectCount` must be a positive integer/)
  }
  const shared = makeCase('c', {
    knownMisses: [known()],
    knownFalsePositives: [known({ rule: 'near-duplicate-test', titles: ['p', 'q'] })],
  })
  assert.match(problemsOf(manifestOf(shared)), /duplicate known id k1/)
})

test('unknown matcher and root keys fail, and so do non-string arguments', () => {
  const misspelled = makeCase('c', {
    expectedFindings: [{ rule: 'duplicate-test', severity: 'error', titles: ['a', 'b'], severty: 'x' }],
  })
  assert.match(problemsOf(manifestOf(misspelled)), /unknown key `severty`/)
  assert.match(problemsOf({ ...manifestOf(makeCase('c')), knownMissBaselne: 0 }), /manifest: unknown key `knownMissBaselne`/)
  for (const bad of [[1], [null], [''], 'x']) {
    assert.match(problemsOf(manifestOf(makeCase('c', { arguments: bad }))), /arguments` must be an array of non-empty CLI tokens/)
  }
})

test('nameless cases are reported once each, not as duplicates of one another', () => {
  const message = problemsOf(manifestOf(makeCase('x', { name: undefined }), makeCase('y', { name: undefined })))
  assert.doesNotMatch(message, /duplicate case name/)
  assert.match(message, /case #0: `name` must be a non-empty string/)
  assert.match(message, /case #1: `name` must be a non-empty string/)
})

test('duplicate case names, repeated matchers and contradictions fail', () => {
  assert.match(problemsOf(manifestOf(makeCase('c'), makeCase('c'))), /duplicate case name/)

  const finding = { rule: 'duplicate-test', severity: 'error', titles: ['a', 'b'] }
  // Title order must not hide a repeat, and severity is not part of identity.
  const repeated = makeCase('c', {
    expectedFindings: [finding, { ...finding, severity: 'warning', titles: ['b', 'a'] }],
  })
  assert.match(problemsOf(manifestOf(repeated)), /c#2 expectedFindings: repeats c#1/)

  const contradiction = makeCase('c', { expectedAbsent: [{ rule: 'duplicate-test', titles: ['b', 'a'] }] })
  assert.match(problemsOf(manifestOf(contradiction)), /c#2: also expected as a finding by c#1/)

  // The same matcher in two different cases is two different labels, not a repeat.
  assert.doesNotThrow(() => buildRecallLabels(manifestOf(makeCase('c'), makeCase('d'))))
})

test('mixed-rule coverage entries resolve to a declared case', () => {
  const base = manifestOf(
    makeCase('c', {
      expectedFindings: [
        { rule: 'duplicate-test', severity: 'error', titles: ['a', 'b'] },
        { rule: 'duplicate-test-name', severity: 'warning', titles: ['a', 'b'] },
      ],
      expectedAbsent: [],
    }),
  )
  const cannot = 'never share a participant at all'
  const ok = buildRecallLabels({
    ...base,
    mixedRuleCoverage: [
      { pair: ['duplicate-test', 'duplicate-test-name'], case: 'c' },
      { pair: ['duplicate-test', 'duplicate-suite'], cannotCoOccur: cannot },
    ],
  })
  assert.deepEqual(ok.controls, [
    { pair: ['duplicate-test', 'duplicate-test-name'], caseName: 'c', cannotCoOccur: null },
    { pair: ['duplicate-test', 'duplicate-suite'], caseName: null, cannotCoOccur: cannot },
  ])
  const gone = { ...base, mixedRuleCoverage: [{ pair: ['duplicate-test', 'duplicate-test-name'], case: 'gone' }] }
  assert.match(problemsOf(gone), /names case "gone"/)
  const short = { ...base, mixedRuleCoverage: [{ pair: ['duplicate-test'], case: 'c' }] }
  assert.match(problemsOf(short), /exactly two rules/)
  assert.deepEqual(buildRecallLabels(base).controls, [])
})

test('a malformed mixed-rule control is refused, as the recall gate refuses it', () => {
  const covering = makeCase('c', {
    expectedFindings: [
      { rule: 'duplicate-test', severity: 'error', titles: ['a', 'b'] },
      { rule: 'duplicate-test-name', severity: 'warning', titles: ['a', 'b'] },
    ],
    expectedAbsent: [],
  })
  const base = manifestOf(covering)
  const pair = ['duplicate-test', 'duplicate-test-name']
  const cannot = 'never share a participant at all'
  const withControls = (...mixedRuleCoverage) => ({ ...base, mixedRuleCoverage })

  // A misspelled key leaves neither `case` nor `cannotCoOccur`; naming both is as bad.
  assert.match(problemsOf(withControls({ pair, csae: 'c' })), /exactly one of `case`/)
  assert.match(problemsOf(withControls({ pair, case: 'c', cannotCoOccur: cannot })), /exactly one of `case`/)
  assert.match(problemsOf(withControls({ pair, cannotCoOccur: 'too short' })), /reason of 20\+ characters/)
  assert.match(problemsOf(withControls({ pair, case: '' })), /must name a declared case/)
  // A valid field next to a malformed second one is refused, not returned with both.
  assert.match(problemsOf(withControls({ pair, case: 'c', cannotCoOccur: 'too short' })), /exactly one of `case`/)
  assert.match(problemsOf(withControls({ pair, cannotCoOccur: cannot, case: '' })), /exactly one of `case`/)

  // The inventory itself must be an array.
  assert.match(problemsOf({ ...base, mixedRuleCoverage: null }), /mixedRuleCoverage: must be an array/)
  // A covering case whose own findings are malformed is reported, not crashed on.
  const broken = manifestOf(makeCase('c', { expectedFindings: {} }))
  assert.match(
    problemsOf({ ...broken, mixedRuleCoverage: [{ pair, case: 'c' }] }),
    /recall label problem\(s\)[\s\S]*`expectedFindings` must be an array[\s\S]*include no .duplicate-test. finding/,
  )
  assert.match(problemsOf({ ...base, mixedRuleCoverage: { pair } }), /mixedRuleCoverage: must be an array/)

  // Both members must be distinct rules the analyzer implements.
  assert.match(problemsOf(withControls({ pair: ['duplicate-test', 'no-such-rule'], case: 'c' })), /distinct rules/)
  assert.match(problemsOf(withControls({ pair: ['duplicate-test', 'duplicate-test'], case: 'c' })), /distinct rules/)

  // A pair is recorded once, whichever way round it is written.
  assert.match(
    problemsOf(withControls({ pair, case: 'c' }, { pair: [...pair].reverse(), cannotCoOccur: cannot })),
    /recorded more than once/,
  )

  // Coverage means both rules fire in the named case.
  const oneRule = manifestOf(makeCase('c'))
  assert.match(
    problemsOf({ ...oneRule, mixedRuleCoverage: [{ pair, case: 'c' }] }),
    /include no .duplicate-test-name. finding/,
  )

  // Controls: the same entries written correctly are returned, in either pair order.
  const ok = buildRecallLabels(
    withControls({ pair, case: 'c' }, { pair: ['duplicate-test', 'duplicate-suite'], cannotCoOccur: cannot }),
  )
  assert.equal(ok.controls.length, 2)
  assert.doesNotThrow(() => buildRecallLabels(withControls({ pair: [...pair].reverse(), case: 'c' })))
})

test('every problem is reported together, not just the first', () => {
  const message = problemsOf(manifestOf(makeCase('a', { origin: 'x' }), makeCase('b', { verdict: 'x' })))
  assert.match(message, /^2 recall label problem\(s\):/)
  assert.match(message, /case a/)
  assert.match(message, /case b/)
})

test('summarizeLabels counts every kind and pending case', () => {
  const summary = summarizeLabels(
    buildRecallLabels(manifestOf(makeCase('a'), makeCase('b', { pending: 'p'.repeat(20) }))),
  )
  assert.deepEqual(summary, {
    cases: 2,
    expectations: 4,
    pendingCases: 1,
    byKind: { 'expected-finding': 2, 'expected-absent': 2, 'known-miss': 0, 'known-false-positive': 0 },
  })
  assert.deepEqual(
    Object.keys(summary.byKind),
    EXPECTATION_GROUPS.map((group) => group.kind),
  )
})

test('a recall case declares its own blind files, each inside the case', () => {
  const cases = [
    { path: 'a', blindUnderCurrent: ['run.test.ts', 'other.test.ts'] },
    { path: 'b' },
  ]
  assert.deepEqual(blindRecallFiles({ cases }), ['recall/a/run.test.ts', 'recall/a/other.test.ts'])
  for (const blind of [[], 'run.test.ts', [1], ['../escape.test.ts'], ['/abs.test.ts']]) {
    assert.throws(() => blindRecallFiles({ cases: [{ path: 'a', blindUnderCurrent: blind }] }), /blindUnderCurrent/)
  }
})

test('the real recall manifest labels to the populations the recall gate counts', () => {
  const result = readRecallManifest(REAL_ROOT)
  const summary = summarizeLabels(result)
  // Against the manifest itself, so a generator that drops a case cannot hide above the floors.
  const manifest = JSON.parse(readFileSync(join(REAL_ROOT, 'manifest.json'), 'utf8'))
  assert.equal(summary.cases, manifest.cases.length)
  assert.equal(summary.byKind['expected-finding'], manifest.cases.flatMap((c) => c.expectedFindings).length)
  assert.equal(summary.byKind['expected-absent'], manifest.cases.flatMap((c) => c.expectedAbsent).length)
  assert.equal(result.controls.length, manifest.mixedRuleCoverage.length)
  // Floors: a gate that measured less must fail, and adding a case must not edit this test.
  assert.ok(summary.cases >= 280, `${summary.cases} cases`)
  assert.ok(summary.byKind['expected-finding'] >= 237, 'expected findings')
  assert.ok(summary.byKind['expected-absent'] >= 201, 'expected absences')
  assert.ok(result.controls.length >= 36, 'controls')
  assert.equal(summary.byKind['known-miss'], 0)
  assert.equal(summary.byKind['known-false-positive'], 1)
  assert.equal(summary.pendingCases, 0)
  assert.equal(new Set(result.expectations.map((e) => e.id)).size, summary.expectations)
  assert.ok(result.cases.every((c) => c.origin === 'synthetic' && c.verdict === 'generated'))
})

test('readRecallManifest rejects an unsupported schema version and an empty manifest', () => {
  const root = mkdtempSync(join(tmpdir(), 'labels-'))
  const write = (manifest) => writeFileSync(join(root, 'manifest.json'), JSON.stringify(manifest))
  write({ schemaVersion: 2, cases: [makeCase('c')] })
  assert.throws(() => readRecallManifest(root), /schemaVersion/)
  write({ schemaVersion: 1, cases: [] })
  assert.throws(() => readRecallManifest(root), /no cases/)
  write({ schemaVersion: 1, cases: [makeCase('c')] })
  assert.equal(readRecallManifest(root).cases.length, 1)
})
