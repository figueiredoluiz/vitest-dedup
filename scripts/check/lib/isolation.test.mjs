// Unit tests for the parts of `isolation.mjs` that do not need a built binary.
//
// `collectFingerprintRecords` shells out to the analyzer, so it is exercised end to end by
// `check-fixture-isolation.mjs`. What is tested here is the indexing decision that gate depends on
// and cannot observe: that a file/line key holds EVERY hook declared there, not the last one.
import assert from 'node:assert/strict'
import test from 'node:test'
import { checkExpectation } from './isolation.mjs'

/**
 * The indexing `collectFingerprintRecords` performs for `"hook"` records, in isolation.
 *
 * Kept as a local mirror rather than exported from the module: the module's own version is reached
 * through a subprocess run of the analyzer, and the property under test is about the Map shape, not
 * about the analyzer. If the two ever diverge, `check-fixture-isolation.mjs` fails loudly against a
 * real fixture -- this test exists to catch the shape regression first, with a readable message.
 */
function indexHooks(records) {
  const byHookLine = new Map()
  for (const record of records) {
    if (record.type !== 'hook') continue
    const key = `${record.path}\n${record.line}`
    if (!byHookLine.has(key)) byHookLine.set(key, [])
    byHookLine.get(key).push(record)
  }
  return byHookLine
}

test('two hooks declared on one line are both retained', () => {
  // `beforeEach(() => {...}); afterEach(() => {...})` on one line is two declarations. The
  // extractor records each with its own `call.span`, so they share a line and differ in column,
  // kind and body. A Map keyed on line alone kept only the last, which let a participant naming
  // the beforeEach be rejected for the afterEach's kind -- or, for two hooks of the same kind,
  // silently compared against the wrong body.
  const index = indexHooks([
    { type: 'hook', path: 't.test.ts', line: 4, column: 1, kind: 'beforeEach', alpha: 'aaa' },
    { type: 'hook', path: 't.test.ts', line: 4, column: 39, kind: 'afterEach', alpha: 'bbb' },
  ])
  const onLine4 = index.get('t.test.ts\n4')
  assert.equal(onLine4.length, 2, 'both hooks on the line must survive indexing')
  assert.deepEqual(
    onLine4.map((record) => record.kind),
    ['beforeEach', 'afterEach'],
    'records must stay in stream order so resolution is deterministic',
  )
})

test('hooks on different lines stay separate', () => {
  const index = indexHooks([
    { type: 'hook', path: 't.test.ts', line: 4, column: 1, kind: 'beforeEach', alpha: 'aaa' },
    { type: 'hook', path: 't.test.ts', line: 8, column: 1, kind: 'beforeEach', alpha: 'bbb' },
  ])
  assert.equal(index.get('t.test.ts\n4').length, 1)
  assert.equal(index.get('t.test.ts\n8').length, 1)
})

test('test records are not indexed as hooks', () => {
  const index = indexHooks([{ type: 'test', path: 't.test.ts', line: 4, title: 'a' }])
  assert.equal(index.size, 0, 'only "hook" records belong in the hook index')
})

// ---------------------------------------------------------------------------------------------
// the unstateable stale-marker ratchet
// ---------------------------------------------------------------------------------------------
//
// `checkExpectation` resolves participants against an index of analyzer records. These tests
// build that index synthetically so the ratchet's two outcomes can be pinned without a binary:
// the opt-out HOLDS while the `context` relation cannot hold (today's near-context fixtures),
// and FAILS the moment it can (the day near-duplicate normalization equates the phrasings), so
// the restatement each reason records is forced by the gate rather than remembered by a reader.

/**
 * Build a fixture whose isolation claim requires an explicit missing-view justification and
 * restatement.
 */
function unstateableExpectation() {
  return {
    rule: 'none',
    verdict: 'distinct',
    participants: [
      { file: 't.test.ts', line: 4, title: 'reads the limit', inert: false },
      { file: 't.test.ts', line: 9, title: 'reads the limit', inert: false },
    ],
    isolates: {
      dimensions: [],
      unchecked: false,
      unstateable: true,
      notIsolable: false,
      reason: 'MISSING VIEW: a phrasing-insensitive body normalization',
    },
    isolationViolation: null,
    pending: null,
  }
}

/** One `record`-backed index with two tests titled `reads the limit` at the declared lines. */
function indexOver([first, second]) {
  const records = [
    { type: 'test', path: 't.test.ts', line: 4, title: 'reads the limit', trivial: false, comparable: true, ...first },
    { type: 'test', path: 't.test.ts', line: 9, title: 'reads the limit', trivial: false, comparable: true, ...second },
  ]
  return {
    byTitle: new Map([['t.test.ts\nreads the limit', records]]),
    byFile: new Map([['t.test.ts', records]]),
    byHookLine: new Map(),
    count: 2,
  }
}

const readSource = () => ''

test('an unstateable expectation holds while the context relation cannot hold', () => {
  const result = checkExpectation(unstateableExpectation(), indexOver([
    { alpha: 'a1', structural: 's1', behavior: 'b1', contextKey: 'c1', modifierKey: 'm' },
    { alpha: 'a2', structural: 's2', behavior: 'b2', contextKey: 'c2', modifierKey: 'm' },
  ]), readSource)
  assert.equal(result.status, 'unstateable')
  assert.match(result.message, /MISSING VIEW/)
})

test('an unstateable expectation fails the moment the context relation holds for its participants', () => {
  // Every reason in the corpus records the same exit: "restate as plain `context` when the
  // missing view lands". Bodies agreeing at alpha/structural/behavior with contexts still
  // differing is exactly that state, so the gate must demand the restatement, not hold the
  // opt-out open forever.
  const result = checkExpectation(unstateableExpectation(), indexOver([
    { alpha: 'same', structural: 'same', behavior: 'same', contextKey: 'c1', modifierKey: 'm' },
    { alpha: 'same', structural: 'same', behavior: 'same', contextKey: 'c2', modifierKey: 'm' },
  ]), readSource)
  assert.equal(result.status, 'fail')
  assert.match(result.message, /restates?\b|`isolates context`/)
})

test('an unstateable expectation fails when its participants lose the context distinction', () => {
  // The claim rests on the contexts DIFFERING; if a context-folding regression collapses the
  // two contextKeys, the opt-out must not absorb it -- the silence would mean something else
  // entirely and no `context` restatement could ever hold.
  const result = checkExpectation(unstateableExpectation(), indexOver([
    { alpha: 'a1', structural: 's1', behavior: 'b1', contextKey: 'same', modifierKey: 'm' },
    { alpha: 'a2', structural: 's2', behavior: 'b2', contextKey: 'same', modifierKey: 'm' },
  ]), readSource)
  assert.equal(result.status, 'fail')
  assert.match(result.message, /contextKey/)
})

test('an unstateable expectation fails when a participant is no longer comparable', () => {
  // A participant whose context never resolved was never weighed against anything; an opt-out
  // may not launder that never-compared silence into a held isolation claim.
  const result = checkExpectation(unstateableExpectation(), indexOver([
    { alpha: 'a1', structural: 's1', behavior: 'b1', contextKey: 'c1', modifierKey: 'm', comparable: false },
    { alpha: 'a2', structural: 's2', behavior: 'b2', contextKey: 'c2', modifierKey: 'm' },
  ]), readSource)
  assert.equal(result.status, 'fail')
  assert.match(result.message, /comparable/)
})

test('an unstateable expectation fails when its participants drift apart in modifiers', () => {
  // The claim is "differ only in phrasing and context"; a modifier difference (.skip, .each
  // identity) is a second variable the fixture does not declare, the same drift the equal-half
  // guards reject for checked dimensions. Holding the opt-out open over it would keep counting
  // a fixture that no longer isolates what its reason says.
  const result = checkExpectation(unstateableExpectation(), indexOver([
    { alpha: 'a1', structural: 's1', behavior: 'b1', contextKey: 'c1', modifierKey: 'm1' },
    { alpha: 'a2', structural: 's2', behavior: 'b2', contextKey: 'c2', modifierKey: 'm2' },
  ]), readSource)
  assert.equal(result.status, 'fail')
  assert.match(result.message, /modifierKey/)
})
