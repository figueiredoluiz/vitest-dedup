// Unit tests for the next-engine duplicate rules' pairwise judgment, over fabricated
// observations: a required pair missed, held by a known entry, gone stale; a declared silence
// linked; an undeclared pair linked; a cluster's pair expansion; and the floors.
import assert from 'node:assert/strict'
import test from 'node:test'

import {
  edgeKey,
  expectationEdges,
  judge,
  judgeRecall,
  knownProblems,
  pairEdges,
  pendingByRule,
  PORTED_RULES,
  strayFindings,
} from './next-duplicates.mjs'

/** One finding record as the JSONL report spells it. */
function finding(rule, locations) {
  return {
    type: 'finding',
    rule,
    severity: 'warning',
    locations: locations.map(([path, line]) => ({ path, line })),
    titles: locations.map(() => 't'),
    inert: locations.map(() => false),
  }
}

/** One parsed expectation, the shape `parseFixture` returns. */
function expectation(rule, participants, { pending = null } = {}) {
  return {
    rule,
    verdict: rule === 'none' ? 'distinct' : 'redundant',
    severity: rule === 'none' ? 'none' : 'warning',
    pending,
    participants: participants.map(([file, line]) => ({ file, line, title: 't', inert: false })),
  }
}

test('a pair finding links exactly its two participants, by rule', () => {
  const linked = pairEdges([
    finding('duplicate-test', [
      ['fixtures/d/a.test.ts', 5],
      ['fixtures/d/b.test.ts', 9],
    ]),
  ])
  assert.deepEqual([...linked.keys()], [edgeKey('a.test.ts:5', 'b.test.ts:9')])
  assert.deepEqual([...linked.get(edgeKey('a.test.ts:5', 'b.test.ts:9'))], ['duplicate-test'])
})

test('a cluster finding covers every member pair', () => {
  const linked = pairEdges([
    finding(
      'duplicate-test',
      ['a', 'b', 'c', 'd', 'e'].map((file, index) => [`${file}.test.ts`, index + 1]),
    ),
  ])
  assert.equal(linked.size, 10, '5 members: 5*4/2 pairs')
})

test('a rule the next engine does not port links nothing here', () => {
  const linked = pairEdges([
    finding('redundant-hook', [
      ['a.test.ts', 1],
      ['b.test.ts', 2],
    ]),
  ])
  assert.equal(linked.size, 0)
})

test('strayFindings names the findings of unported rules only', () => {
  const records = [
    finding('duplicate-test', [
      ['a.test.ts', 1],
      ['a.test.ts', 2],
    ]),
    finding('subsumed-test', [
      ['a.test.ts', 3],
      ['a.test.ts', 4],
    ]),
  ]
  assert.deepEqual(strayFindings(records).map((record) => record.rule), ['subsumed-test'])
  assert.equal(PORTED_RULES.has('subsumed-test'), false)
})

test('a positive expectation requires every participant pair; none forbids the rest', () => {
  const fixtures = new Map([
    [
      'pair.test.ts',
      {
        expectations: [
          expectation('duplicate-test', [
            ['pair.test.ts', 5],
            ['pair.test.ts', 12],
          ]),
          expectation('none', [
            ['pair.test.ts', 5],
            ['pair.test.ts', 20],
          ]),
          expectation('none', [
            ['pair.test.ts', 5],
            ['pair.test.ts', 12],
          ]),
        ],
      },
    ],
  ])
  const { required, forbidden } = expectationEdges(fixtures)
  const pair = edgeKey('pair.test.ts:5', 'pair.test.ts:12')
  assert.deepEqual(
    required.map((r) => [r.edge, r.rule]),
    [[pair, 'duplicate-test']],
  )
  assert.deepEqual(
    forbidden.map((f) => f.edge),
    [edgeKey('pair.test.ts:5', 'pair.test.ts:20')],
    'the pair the positive expectation requires is not also forbidden',
  )
})

test('pending expectations and unported rules claim nothing', () => {
  const fixtures = new Map([
    [
      'pair.test.ts',
      {
        expectations: [
          expectation(
            'duplicate-test',
            [
              ['pair.test.ts', 5],
              ['pair.test.ts', 12],
            ],
            { pending: 'waits on the dataflow gap' },
          ),
          expectation('parameterization-candidate', [
            ['pair.test.ts', 5],
            ['pair.test.ts', 20],
          ]),
        ],
      },
    ],
  ])
  const { required, forbidden } = expectationEdges(fixtures)
  assert.equal(required.length, 0)
  assert.equal(forbidden.length, 0)
})

test('a missed required pair fails, holds while listed, and its entry goes stale when it passes', () => {
  const required = [
    { edge: edgeKey('a.test.ts:5', 'a.test.ts:9'), rule: 'duplicate-test', from: 'd.test.ts' },
  ]
  const known = [
    {
      id: `miss|fixtures/d|${edgeKey('a.test.ts:5', 'a.test.ts:9')}|duplicate-test`,
      reason: 'the pair needs destructured-literal dataflow',
    },
  ]
  const nothingLinked = judge({ required, forbidden: [], linked: new Map(), known, scope: 'fixtures/d' })
  assert.deepEqual(nothingLinked.failures, [], 'the miss is listed, so it is held')
  assert.equal(nothingLinked.held.length, 1)
  assert.equal(nothingLinked.consumed.size, 1)

  const nowLinked = judge({
    required,
    forbidden: [],
    linked: new Map([[edgeKey('a.test.ts:5', 'a.test.ts:9'), new Set(['duplicate-test'])]]),
    known,
    scope: 'fixtures/d',
  })
  assert.deepEqual(nowLinked.failures, [])
  assert.equal(nowLinked.consumed.size, 0, 'the entry no longer applies, so the caller fails it')
})

test('an unlisted miss fails naming what was linked instead', () => {
  const outcome = judge({
    required: [
      { edge: edgeKey('a.test.ts:5', 'a.test.ts:9'), rule: 'duplicate-test', from: 'd.test.ts' },
    ],
    forbidden: [],
    linked: new Map([[edgeKey('a.test.ts:5', 'a.test.ts:9'), new Set(['cross-file-duplicate'])]]),
    known: [],
    scope: 'fixtures/d',
  })
  assert.equal(outcome.failures.length, 1)
  assert.match(outcome.failures[0].message, /cross-file-duplicate/)
})

test('a declared silence the next engine links fails unless a known entry holds it', () => {
  const forbidden = [{ edge: edgeKey('a.test.ts:5', 'a.test.ts:9'), from: 'nc.test.ts' }]
  const edge = edgeKey('a.test.ts:5', 'a.test.ts:9')
  const unlisted = judge({
    required: [],
    forbidden,
    linked: new Map([[edge, new Set(['duplicate-test'])]]),
    known: [],
    scope: 'fixtures/d',
  })
  assert.equal(unlisted.failures.length, 1)
  const listed = judge({
    required: [],
    forbidden,
    linked: new Map([[edge, new Set(['duplicate-test'])]]),
    known: [{ id: `link|fixtures/d|${edge}`, reason: 'identical text in identical context links' }],
    scope: 'fixtures/d',
  })
  assert.deepEqual(listed.failures, [])
  assert.equal(listed.held.length, 1)
})

test('a pair no expectation declares fails as extra, whatever links it', () => {
  const outcome = judge({
    required: [],
    forbidden: [],
    linked: new Map([[edgeKey('x.test.ts:1', 'y.test.ts:2'), new Set(['cross-file-duplicate'])]]),
    known: [],
    scope: 'fixtures/observed',
  })
  assert.equal(outcome.failures.length, 1)
  assert.match(outcome.failures[0].message, /no fixture expectation declares/)
})

test('a scope with a required pair is the floor; the gate adds the rest', () => {
  const outcome = judge({ required: [], forbidden: [], linked: new Map(), known: [], scope: 'd' })
  assert.deepEqual(outcome.failures, [])
  assert.equal(outcome.consumed.size, 0)
})

test('a group of three is required as a chain, and its third pair is declared, not unclaimed', () => {
  const header = new Map([
    ['d/a.test.ts', { expectations: [expectation('duplicate-test', [['a.test.ts', 12], ['a.test.ts', 15], ['a.test.ts', 9]])] }],
  ])
  const { required, declared, forbidden } = expectationEdges(header)
  assert.equal(required.length, 2, 'neighbours only')
  assert.equal(declared.length, 3)
  const chain = finding('duplicate-test', [['d/a.test.ts', 9], ['d/a.test.ts', 12]])
  const next = finding('duplicate-test', [['d/a.test.ts', 12], ['d/a.test.ts', 15]])
  const outcome = judge({ required, declared, forbidden, linked: pairEdges([chain, next]), known: [], scope: 'd' })
  assert.deepEqual(outcome.failures, [])
  const cluster = finding('duplicate-test', [['d/a.test.ts', 9], ['d/a.test.ts', 12], ['d/a.test.ts', 15]])
  assert.deepEqual(judge({ required, declared, forbidden, linked: pairEdges([cluster]), known: [], scope: 'd' }).failures, [])
  const broken = judge({ required, declared, forbidden, linked: pairEdges([chain]), known: [], scope: 'd' })
  assert.equal(broken.failures.length, 1, 'a participant left out of the chain is a miss')
})

test('files chain in byte order, the way the engine orders a group', () => {
  const header = new Map([
    ['a.test.ts', { expectations: [expectation('cross-file-duplicate', [['a.test.ts', 3], ['B.test.ts', 3], ['c.test.ts', 3]])] }],
  ])
  const { required, declared, forbidden } = expectationEdges(header)
  const linked = pairEdges([
    finding('cross-file-duplicate', [['d/B.test.ts', 3], ['d/a.test.ts', 3]]),
    finding('cross-file-duplicate', [['d/a.test.ts', 3], ['d/c.test.ts', 3]]),
  ])
  assert.deepEqual(judge({ required, declared, forbidden, linked, known: [], scope: 'd' }).failures, [])

  // UTF-8 puts U+FF01 before U+1F407; UTF-16 code units put them the other way round.
  const wide = new Map([
    ['a.test.ts', { expectations: [expectation('cross-file-duplicate', [['a.test.ts', 3], ['\u{1F407}.test.ts', 3], ['\uFF01.test.ts', 3]])] }],
  ])
  const chain = expectationEdges(wide)
  const emitted = pairEdges([
    finding('cross-file-duplicate', [['d/a.test.ts', 3], ['d/\uFF01.test.ts', 3]]),
    finding('cross-file-duplicate', [['d/\uFF01.test.ts', 3], ['d/\u{1F407}.test.ts', 3]]),
  ])
  assert.deepEqual(judge({ ...chain, linked: emitted, known: [], scope: 'd' }).failures, [])
})

test('a pair an expectation names fails when the other rule links it too', () => {
  const header = new Map([['a.test.ts', { expectations: [expectation('duplicate-test', [['a.test.ts', 3], ['a.test.ts', 6]])] }]])
  const { required, declared, forbidden } = expectationEdges(header)
  const expected = finding('duplicate-test', [['d/a.test.ts', 3], ['d/a.test.ts', 6]])
  const other = finding('cross-file-duplicate', [['d/a.test.ts', 3], ['d/a.test.ts', 6]])
  const clean = judge({ required, declared, forbidden, linked: pairEdges([expected]), known: [], scope: 'd' })
  assert.deepEqual(clean.failures, [])
  const outcome = judge({ required, declared, forbidden, linked: pairEdges([expected, other]), known: [], scope: 'd' })
  assert.equal(outcome.failures.length, 1)
  assert.match(outcome.failures[0].message, /cross-file-duplicate linked .* another rule/)
  const held = [{ id: outcome.failures[0].id, reason: 'r' }]
  assert.deepEqual(judge({ required, declared, forbidden, linked: pairEdges([expected, other]), known: held, scope: 'd' }).failures, [])
})

/** A recall finding of `rule` at `lines` of `a.test.ts`, with the title the next engine renders. */
const located = (rule, lines) => ({ type: 'finding', rule, severity: 'error', titles: ['row %i'], locations: lines.map((line) => ({ path: 'a.test.ts', line })) })

/** The summary record of a run that analyzed `testsAnalyzed` tests. */
const summary = (testsAnalyzed) => ({ type: 'summary', testsAnalyzed })

test('a case that analyzed another number of tests is a delta carrying the count, so a loss changes it', () => {
  const { failures } = judgeRecall({ name: 'c', expectedTests: 2 }, [summary(1)])
  assert.deepEqual(failures.map((f) => f.id), ['recall-tests|c|all|1'])
  assert.match(failures[0].message, /analyzed 1 test\(s\), and the case expects 2/)
  assert.deepEqual(judgeRecall({ name: 'c', expectedTests: 2 }, []).failures.map((f) => f.id), ['recall-tests|c|all|undefined'])
})

test('a recall expectation is met by rule and locations, whatever title the next engine renders', () => {
  const matcher = { rule: 'duplicate-test', titles: ['row 1'], locations: ['a.test.ts:3', 'a.test.ts:6'] }
  const outcome = judgeRecall({ name: 'c', expectedTests: 2, expectedFindings: [matcher] }, [summary(2), located('duplicate-test', [6, 3])])
  assert.deepEqual(outcome, { failures: [], judged: 1 })
})

test('a recall expectation of a rule not ported is not judged, and a titled matcher keeps its titles', () => {
  const other = { name: 'c', expectedFindings: [{ rule: 'redundant-hook' }] }
  assert.deepEqual(judgeRecall({ ...other, expectedTests: 1 }, [summary(1)]), { failures: [], judged: 0 })
  const titled = { name: 'c', expectedFindings: [{ rule: 'duplicate-test', titles: ['row 1'] }] }
  const { failures } = judgeRecall({ ...titled, expectedTests: 2 }, [summary(2), located('duplicate-test', [3, 6])])
  assert.deepEqual(failures.map((f) => f.id.split('|')[0]).sort(), ['recall-extra', 'recall-miss'])
  assert.equal(failures.find((f) => f.id.startsWith('recall-miss')).id, 'recall-miss|c|duplicate-test|row 1')
})

test('a missing, a forbidden and an unexpected finding each carry their own stable id', () => {
  const matcher = { rule: 'duplicate-test', locations: ['a.test.ts:3', 'a.test.ts:6'] }
  const missed = judgeRecall({ name: 'c', expectedTests: 2, expectedFindings: [matcher] }, [summary(2)])
  assert.equal(missed.failures[0].id, 'recall-miss|c|duplicate-test|a.test.ts:3,a.test.ts:6')
  const absent = judgeRecall({ name: 'c', expectedTests: 2, expectedAbsent: [matcher] }, [summary(2), located('duplicate-test', [3, 6])])
  assert.deepEqual(absent.failures.map((f) => f.id), ['recall-absent|c|duplicate-test|a.test.ts:3,a.test.ts:6'])
  const extra = judgeRecall({ name: 'c', expectedTests: 2 }, [summary(2), located('cross-file-duplicate', [9, 12])])
  assert.deepEqual(extra.failures.map((f) => f.id), ['recall-extra|c|cross-file-duplicate|a.test.ts:12,a.test.ts:9'])
  assert.match(extra.failures[0].message, /FAIL recall c: the next engine reported cross-file-duplicate/)
})

test('a known miss is not required, and consumes the finding that meets it', () => {
  const matcher = { rule: 'duplicate-test', locations: ['a.test.ts:3', 'a.test.ts:6'] }
  const testCase = { name: 'c', expectedTests: 2, knownMisses: [matcher] }
  assert.deepEqual(judgeRecall(testCase, [summary(2)]).failures, [])
  assert.deepEqual(judgeRecall(testCase, [summary(2), located('duplicate-test', [3, 6])]).failures, [])
})

test('expectations waiting on a port are counted per rule, without the ported rules or `none`', () => {
  const expectations = [{ rule: 'duplicate-suite' }, { rule: 'duplicate-suite' }, { rule: 'redundant-hook' }, { rule: 'duplicate-test' }, { rule: 'none' }]
  assert.deepEqual(pendingByRule(expectations), { 'duplicate-suite': 2, 'redundant-hook': 1 })
})

test('a known entry needs an id and a reason', () => {
  assert.deepEqual(knownProblems([{ id: 'a', reason: 'b' }]), [])
  assert.deepEqual(knownProblems({}), ['the known list must be a list of cases'])
  assert.deepEqual(knownProblems([{ id: 'a', reason: ' ' }, { reason: 'b' }, null]), [
    'known entry 0 needs an id and a reason',
    'known entry 1 needs an id and a reason',
    'known entry 2 needs an id and a reason',
  ])
})

test('name findings require exact titles even when locations match', () => {
  const expected = { rule: 'duplicate-test-name', locations: ['a.test.ts:3', 'a.test.ts:6'], titles: ['row 1', 'row 1'] }
  const record = { ...located('duplicate-test-name', [3, 6]), titles: ['row 1', 'row 1'] }
  const testCase = { name: 'names', expectedTests: 4, expectedFindings: [expected, { ...expected, titles: ['row 2', 'row 2'] }] }
  assert.deepEqual(judgeRecall(testCase, [summary(4), record, { ...record, titles: ['row 2', 'row 2'] }]).failures, [])
  const missing = judgeRecall(testCase, [summary(4)])
  assert.equal(new Set(missing.failures.map(failure => failure.id)).size, 2)
  const collapsed = judgeRecall(testCase, [summary(4), record])
  assert.equal(collapsed.failures.length, 1)
  assert.match(collapsed.failures[0].id, /^recall-miss/)
  const wrong = judgeRecall(testCase, [summary(4), record, { ...record, titles: ['row 3', 'row 3'] }])
  assert.equal(wrong.failures.length, 2)
  const extra = judgeRecall({ name: 'names', expectedTests: 4 }, [summary(4), record, { ...record, titles: ['row 2', 'row 2'] }])
  assert.equal(new Set(extra.failures.map(failure => failure.id)).size, 2)
})
