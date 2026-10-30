// Unit tests for the hooks-only differential check's cases and judgement.
import assert from 'node:assert/strict'
import test from 'node:test'

import { CALL_LINE, generateCases, judge } from './hooks-differential.mjs'

/** One open and one closed case, as Vitest and the analyzer report them when the check passes. */
function passing() {
  const cases = [
    { id: 'open-x', open: true },
    { id: 'closed-y', open: false },
  ]
  const vitest = new Map([
    ['open-x/case.test.ts', { tests: ['case > hidden', 'case > visible'] }],
    ['closed-y/case.test.ts', { tests: ['case > visible'] }],
  ])
  const located = { compared: ['open-x/case.test.ts', 'closed-y/case.test.ts'], disagreements: [] }
  const sites = new Map([
    ['open-x/case.test.ts', [{ kind: 'wrapper', line: CALL_LINE }]],
    ['closed-y/case.test.ts', [{ kind: 'hook-wrapper', line: CALL_LINE }]],
  ])
  return { cases, vitest, located, sites, known: [] }
}

/** The judgement of `input`. */
const run = ({ cases, vitest, located, sites, known }) => judge(cases, vitest, located, sites, known)

test('a sound open case and a hooks-only closed case pass', () => {
  const result = run(passing())
  assert.deepEqual(result.failures, [])
  assert.deepEqual([result.open, result.closed, result.unsound.length], [1, 1, 0])
})

test('an open call not read as wrapper fails unless it is listed, and a listed case that passes fails', () => {
  const input = passing()
  // The hidden task may carry no line and be matched to a module load: the call's site decides.
  input.sites.set('open-x/case.test.ts', [{ kind: 'hook-wrapper', line: CALL_LINE }, { kind: 'module-load', line: 2 }])
  assert.match(run(input).failures.join('\n'), /open-x: Vitest collects a test the analyzer counts nowhere/)
  assert.deepEqual(run({ ...input, known: [{ id: 'open-x', reason: 'r' }] }).failures, [])
  assert.match(run({ ...passing(), known: [{ id: 'open-x', reason: 'r' }] }).failures.join('\n'), /open-x: listed as failing and now passes/)
})

test('a missing site other than the call fails on its own; the call is judged by its site', () => {
  const input = passing()
  input.located.disagreements.push({ path: 'open-x/case.test.ts', kind: 'missing-site', item: 'test@5' })
  input.located.disagreements.push({ path: 'open-x/case.test.ts', kind: 'missing-site', item: `test@${CALL_LINE}` })
  const result = run({ ...input, known: [{ id: 'open-x', reason: 'r' }] })
  assert.deepEqual(result.unsound, [])
  assert.match(result.failures.join('\n'), /open-x: no site for test@5, which no route hides/)
  assert.doesNotMatch(result.failures.join('\n'), /no site for test@4/)
  assert.match(result.failures.join('\n'), /open-x: listed as failing and now passes/)
})

test('a listed case Vitest stops collecting is reported as uncollected, not as fixed', () => {
  const input = passing()
  input.located.compared = ['closed-y/case.test.ts']
  const failures = run({ ...input, known: [{ id: 'open-x', reason: 'r' }] }).failures.join('\n')
  assert.match(failures, /open-x: not compared/)
  assert.doesNotMatch(failures, /now passes/)
})

test('a route Vitest collects no hidden test from fails, since it proves nothing', () => {
  const input = passing()
  input.vitest.set('open-x/case.test.ts', { tests: ['case > visible'] })
  assert.match(run(input).failures.join('\n'), /open-x: Vitest collected no hidden test/)
})

test('a closed case not read as hook-wrapper, or collecting a test, fails', () => {
  const input = passing()
  input.sites.set('closed-y/case.test.ts', [{ kind: 'wrapper', line: CALL_LINE }])
  assert.match(run(input).failures.join('\n'), /closed-y: the call is \["wrapper"\], not hook-wrapper/)
  const moved = passing()
  moved.sites.set('closed-y/case.test.ts', [{ kind: 'hook-wrapper', line: CALL_LINE + 1 }])
  assert.match(run(moved).failures.join('\n'), /closed-y: the call is \[\]/)
  const collecting = passing()
  collecting.vitest.set('closed-y/case.test.ts', { tests: ['case > hidden', 'case > visible'] })
  assert.match(run(collecting).failures.join('\n'), /closed-y: a closed case collected/)
})

test('a case Vitest did not compare fails instead of passing unmeasured', () => {
  const input = passing()
  input.located.compared = ['closed-y/case.test.ts']
  assert.match(run(input).failures.join('\n'), /open-x: not compared/)
})

test('the cases are fixed, distinct, and call the helper on the call line', () => {
  const cases = generateCases()
  assert.deepEqual(cases, generateCases())
  assert.equal(new Set(cases.map((c) => c.id)).size, cases.length)
  assert.ok(cases.filter((c) => c.open).length >= 100, 'routes x contexts')
  assert.ok(cases.filter((c) => !c.open).length >= 30, 'closed controls x contexts')
  for (const { id, files } of cases) {
    assert.ok(files['support.ts'] && files['helper.ts'], id)
    assert.match(files['case.test.ts'].split('\n')[CALL_LINE - 1], /^\s+setup[(`]/, id)
  }
})
