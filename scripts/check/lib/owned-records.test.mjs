// Controls for version dispatch and rule-specific interpretation of owned evidence.
import assert from 'node:assert/strict'
import test from 'node:test'
import { classifyEligibility, evaluateCase } from './corpus-evaluation.mjs'
import { buildRecallLabels } from './corpus-labels.mjs'
import { checkExpectation } from './isolation.mjs'
import { classifyOwnedEligibility, isolationRecord, validateOwnedRecord } from './owned-records.mjs'

/** A complete owned test with independently variable rule keys. */
function record(patch = {}) {
  return {
    type: 'test', schemaVersion: '0.2', engine: 'owned', path: 'case.test.ts', line: 1, column: 1,
    title: 'a', row: null, comparable: true, exclusion: null, traceComplete: true, ordinaryEvents: 2,
    comparisonKeys: Object.fromEntries(['duplicate', 'crossFile', 'parameterization', 'similarity', 'subsumption', 'name', 'rowClass', 'eventSet'].map((key) => [key, '1111111111111111'])),
    views: { body: '1111111111111111', shape: '2222222222222222', trace: '3333333333333333', context: '4444444444444444', modifiers: '5555555555555555' },
    ...patch,
  }
}

/** Distinct locations and titles resolve to two executions in the evaluator. */
function pair() { return [record(), record({ line: 2, title: 'b' })] }

test('owned eligibility uses the actual rule key, not independent body views', () => {
  const [a, b] = pair()
  b.comparisonKeys = { ...b.comparisonKeys, duplicate: '6666666666666666' }
  assert.deepEqual(classifyOwnedEligibility('duplicate-test', [a, b]), { eligibility: 'eligible', differs: ['comparisonKeys.duplicate'] })
  const expectation = { rule: 'duplicate-test', titles: ['a', 'b'], locations: null }
  assert.deepEqual(classifyEligibility(expectation, [a, b]), { eligibility: 'eligible', differs: ['comparisonKeys.duplicate'] })
  assert.deepEqual(classifyOwnedEligibility('parameterization-candidate', [a, b]), { eligibility: 'eligible', differs: [] })
})

test('an incomplete trace does not disable flat-form comparisons', () => {
  const [a, b] = pair()
  b.traceComplete = false
  b.views = { ...b.views, trace: null }
  assert.equal(classifyOwnedEligibility('duplicate-test', [a, b]).eligibility, 'eligible')
  for (const rule of ['near-duplicate-test', 'subsumed-test', 'parameterization-candidate']) assert.equal(classifyOwnedEligibility(rule, [a, b]).eligibility, 'not-compared')
})

test('rows require one declaration and all the actual row keys', () => {
  const [a, b] = pair()
  a.row = 0; b.row = 1
  assert.equal(classifyOwnedEligibility('duplicate-each-row', [a, b]).eligibility, 'not-compared')
  b.line = a.line
  assert.equal(classifyOwnedEligibility('duplicate-each-row', [a, b]).eligibility, 'eligible')
  b.comparisonKeys = { ...b.comparisonKeys, duplicate: '7777777777777777' }
  assert.deepEqual(classifyOwnedEligibility('duplicate-each-row', [a, b]).differs, ['comparisonKeys.duplicate'])
  assert.equal(classifyOwnedEligibility('duplicate-test', [a, b]).eligibility, 'not-compared')
})

test('unsupported versions, absent keys and excluded proofs fail explicitly', () => {
  assert.throws(() => validateOwnedRecord(record({ schemaVersion: '0.3' })), /unsupported/)
  assert.throws(() => validateOwnedRecord(record({ comparisonKeys: {} })), /comparisonKeys/)
  assert.throws(() => validateOwnedRecord(record({ comparable: false, exclusion: 'unresolved-context' })), /excluded/)
  const r = record({ comparable: false, exclusion: 'unresolved-context', views: null })
  r.comparisonKeys = Object.fromEntries(Object.keys(r.comparisonKeys).map((key) => [key, null]))
  validateOwnedRecord(r)
  assert.equal(classifyOwnedEligibility('duplicate-test', [r, record({ line: 2 })]).eligibility, 'not-compared')
})

test('isolation projection keeps unavailable views unavailable without inventing rule proof', () => {
  const r = record(); r.views = { ...r.views, trace: null }
  const projected = isolationRecord(r)
  assert.equal(projected.alpha, r.views.body)
  assert.equal(projected.behavior, null)
  assert.equal(projected.comparisonKeys, r.comparisonKeys)
  assert.equal(projected.contextKey, r.views.context)
})

test('a hook body view cannot invent a redundancy effect proof', () => {
  const hook = { type: 'hook', schemaVersion: '0.2', engine: 'owned', path: 'case.test.ts', line: 1, column: 1, kind: 'beforeEach', comparisonKeys: { effects: null }, views: { body: '1111111111111111', shape: '2222222222222222', trace: null } }
  assert.equal(classifyOwnedEligibility('redundant-hook', [hook, { ...hook, line: 2 }]).eligibility, 'not-compared')
  assert.equal(isolationRecord(hook).alpha, hook.views.body)
  assert.equal(classifyOwnedEligibility('duplicate-test', [hook, record()]).eligibility, 'unresolved')
})

test('owned schema validation rejects malformed source and eligibility metadata', () => {
  for (const patch of [{ engine: 'future' }, { schemaVersion: '0.7', engine: undefined }, { path: null }, { line: 0 }, { column: 0 }, { title: null }, { ordinaryEvents: -1 }, { ordinaryEvents: 0.5 }, { row: -1 }, { traceComplete: undefined }, { views: {} }]) {
    assert.throws(() => validateOwnedRecord(record(patch)), Error)
  }
  const r = record(); r.comparisonKeys = { ...r.comparisonKeys, duplicate: null }
  assert.throws(() => validateOwnedRecord(r), /exact comparison keys/)
  assert.throws(() => validateOwnedRecord(record({ comparable: false })), /comparability/)
  const legacy = { type: 'test', schemaVersion: '0.1', alpha: 'old' }
  assert.equal(isolationRecord(legacy), legacy)
})

test('owned name proof and trace proof have independent populations', () => {
  const [a, b] = pair()
  b.ordinaryEvents = 0
  b.comparisonKeys = { ...b.comparisonKeys, name: null }
  assert.equal(classifyOwnedEligibility('near-duplicate-test', [a, b]).eligibility, 'not-compared')
  assert.equal(classifyOwnedEligibility('duplicate-test-name', [a, b]).eligibility, 'not-compared')
  assert.equal(classifyOwnedEligibility('duplicate-test', [a, b]).eligibility, 'eligible')
  assert.equal(classifyOwnedEligibility('duplicate-suite', [a, b]).eligibility, 'unresolved')
  assert.equal(classifyOwnedEligibility('duplicate-test', [a, { ...b, engine: undefined, schemaVersion: '0.1' }]).eligibility, 'unresolved')
})

test('proved owned hook effect keys retain kind as a separate comparison dimension', () => {
  const hook = { type: 'hook', schemaVersion: '0.2', engine: 'owned', path: 'case.test.ts', line: 1, column: 1, kind: 'beforeEach', comparisonKeys: { effects: '1111111111111111' }, views: null }
  assert.deepEqual(classifyOwnedEligibility('redundant-hook', [hook, { ...hook, line: 2 }]), { eligibility: 'eligible', differs: [] })
  assert.deepEqual(classifyOwnedEligibility('redundant-hook', [hook, { ...hook, line: 2, kind: 'afterEach' }]), { eligibility: 'eligible', differs: ['kind'] })
  assert.throws(() => validateOwnedRecord({ ...hook, kind: null }), /lifecycle/)
})

test('the corpus evaluator accepts a complete owned stream and rejects a partial or malformed one', () => {
  const caseInput = { name: 'owned', path: 'owned', role: 'positive', rule: 'duplicate-test', origin: 'synthetic', verdict: 'generated', classifiedAt: '2026-10-20', expectedTests: 2, expectedIncomplete: false, expectedAbsent: [], expectedFindings: [{ rule: 'duplicate-test', severity: 'error', titles: ['a', 'b'] }] }
  const labels = buildRecallLabels({ schemaVersion: 1, cases: [caseInput] })
  const finding = { type: 'finding', rule: 'duplicate-test', severity: 'error', titles: ['a', 'b'], locations: [{ path: 'case.test.ts', line: 1 }, { path: 'case.test.ts', line: 2 }] }
  const summary = { type: 'summary', testsAnalyzed: 2, incomplete: false, unresolvedTests: 0, recordCount: 1 }
  const obs = { label: 'owned', directoryPresent: true, sourceFiles: ['case.test.ts'], records: [...pair(), finding, summary], summary }
  const result = evaluateCase(labels.cases[0], labels.expectations, obs)
  assert.equal(result.extraction.tests, 2)
  assert.equal(result.expectations[0].status, 'satisfied')
  assert.throws(() => evaluateCase(labels.cases[0], labels.expectations, { ...obs, records: [record(), finding, summary] }), /stream is partial/)
  const bad = record({ comparisonKeys: {} })
  assert.throws(() => evaluateCase(labels.cases[0], labels.expectations, { ...obs, records: [bad, pair()[1], finding, summary] }), /comparisonKeys/)
})

/** Two suites with aligned children and a real first-child structural difference. */
function suiteRecords() {
  return ['left > first', 'left > second', 'right > first', 'right > second'].map((title, index) => record({
    title, line: [2, 3, 6, 7][index],
    views: { ...record().views, shape: index === 2 ? '6666666666666666' : record().views.shape },
  }))
}

/** Exercise actual participant resolution and the suite relation on a complete four-child stream. */
function suiteResult(records) {
  assert.equal(records.length, 4)
  const projected = records.map(isolationRecord)
  const index = {
    byTitle: new Map(projected.map((r) => [`${r.path}\n${r.title}`, [r]])),
    byFile: new Map([['case.test.ts', projected]]), byHookLine: new Map(), count: projected.length,
  }
  const expectation = { rule: 'none', verdict: 'distinct', participants: [{ file: 'case.test.ts', line: 1, title: 'left' }, { file: 'case.test.ts', line: 5, title: 'right' }], isolates: { dimensions: ['suite-children'], unchecked: false, unstateable: false, notIsolable: false, reason: null }, isolationViolation: null, pending: null }
  const source = "describe('left', () => {\n  test('first', () => {});\n  test('second', () => {});\n});\ndescribe('right', () => {\n  test('first', () => {});\n  test('second', () => {});\n});\n"
  return checkExpectation(expectation, index, () => source)
}

test('suite isolation retains genuine structural differences without requiring unused trace views', () => {
  const records = suiteRecords()
  for (const r of records) { r.traceComplete = false; r.views.trace = null }
  assert.equal(suiteResult(records).status, 'ok')
  records[2].views.shape = records[0].views.shape
  assert.equal(suiteResult(records).status, 'fail', 'complete equal shapes cannot prove isolation')
})

test('suite isolation refuses unavailable owned evidence on every child and both sides', () => {
  for (const field of ['shape', 'context', 'modifiers']) {
    for (const affected of [[1], [3], [1, 3]]) {
      const records = suiteRecords()
      for (const index of affected) records[index].views[field] = null
      const result = suiteResult(records)
      assert.equal(result.status, 'fail', `${field} missing at ${affected}: ${JSON.stringify(result)}`)
      assert.match(result.message, /unavailable/)
    }
  }
})

test('suite isolation refuses empty or missing legacy fields rather than treating them as differences', () => {
  for (const field of ['structural', 'contextKey', 'modifierKey']) {
    for (const value of [undefined, '']) {
      const records = suiteRecords().map((r) => ({ ...isolationRecord(r), engine: undefined, schemaVersion: '0.1' }))
      records[1][field] = value
      records[3][field] = value
      assert.equal(suiteResult(records).status, 'fail')
    }
  }
})
