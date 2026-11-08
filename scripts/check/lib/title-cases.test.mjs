// Unit tests for the generated title cases: generation, spelling, shrinking and the verdict.
import assert from 'node:assert/strict'
import test from 'node:test'

import { generate, holdsLongValue, LONG_VALUES, random, shrinkCandidates, spellCase, spellRow, verdict } from './title-cases.mjs'

/** A test record the analyzer emits, proved unless `proved` is false. */
const record = (row, title, proved = true) => ({ type: 'test', row, title, comparisonKeys: { name: proved ? 'key' : null } })
const collected = (tests, collectErrors = []) => ({ tests, collectErrors })

test('the same seed draws the same cases, and another seed others', () => {
  const draw = (seed) => Array.from({ length: 50 }, ((next) => () => generate(next))(random(seed)))
  assert.deepEqual(draw(7), draw(7))
  assert.notDeepEqual(draw(7), draw(8))
})

test('generated cases keep a piece, a row, and a key in every object row', () => {
  const next = random(1)
  for (let i = 0; i < 500; i++) {
    const { api, pattern, rows } = generate(next)
    assert.ok(['each', 'for'].includes(api))
    assert.ok(pattern.length >= 1 && rows.length >= 1)
    for (const row of rows) if (row.kind !== 'array') assert.ok(row.cells.length >= 1)
  }
})

test('a case spells one registration on line 2, its pattern a JSON string', () => {
  const source = spellCase({ api: 'for', pattern: ['a "', '$0'], rows: [{ kind: 'array', cells: [[null, '1'], [null, "'x'"]] }] })
  assert.equal(source, 'import { test } from \'vitest\'\ntest.for([[1, \'x\']])("a \\"$0", () => {})\n')
  assert.equal(spellRow({ kind: 'object', cells: [['a', '1'], ["'a.b'", 'null']] }), "{ a: 1, 'a.b': null }")
  assert.equal(spellRow({ kind: 'scalar', cells: [[null, '-0']] }), '-0')
})

test('shrinking steps are each one change smaller and never empty the case', () => {
  const testCase = { api: 'for', pattern: ['%s', ' ', '$a'], rows: [{ kind: 'object', cells: [['a', "'two'"], ['b', '1n']] }, { kind: 'scalar', cells: [[null, 'NaN']] }] }
  const candidates = shrinkCandidates(testCase)
  assert.ok(candidates.some((c) => c.rows.length === 1))
  assert.ok(candidates.some((c) => c.pattern.length === 2))
  assert.ok(candidates.some((c) => c.api === 'each'))
  assert.ok(candidates.some((c) => c.rows[0].cells.length === 1))
  assert.ok(candidates.some((c) => c.rows[1]?.cells[0][1] === '0'))
  for (const c of candidates) {
    assert.ok(c.pattern.length >= 1 && c.rows.length >= 1)
    for (const row of c.rows) if (row.kind !== 'array') assert.ok(row.cells.length >= 1)
  }
  const minimal = { api: 'each', pattern: ['a'], rows: [{ kind: 'scalar', cells: [[null, '0']] }] }
  assert.deepEqual(shrinkCandidates(minimal).map((c) => c.rows[0].cells[0][1]), ["'x'"])
})

test('a refused title is allowed, a proved one must match, rows are compared in row order', () => {
  assert.equal(verdict(collected(['a 0', 'a 1']), [record(1, 'a 1'), record(0, 'a 0')]).status, 'agree')
  assert.equal(verdict(collected(['a 0']), [record(0, 'a %#', false)]).status, 'agree')
  const wrong = verdict(collected(['a 0', 'a 1']), [record(0, 'a 0'), record(1, 'a 2')])
  assert.equal(wrong.status, 'title')
  assert.deepEqual(wrong.mismatches, [{ row: 1, vitest: 'a 1', analyzer: 'a 2' }])
  assert.equal(verdict(collected(['a', 'b']), [record(0, 'a')]).status, 'count')
})

test('a case Vitest cannot collect is skipped only when the analyzer proves nothing for it', () => {
  const thrown = collected([], [{ message: 'Do not know how to serialize a BigInt' }])
  assert.equal(verdict(thrown, [record(0, '%j', false)]).status, 'vitest-error')
  assert.equal(verdict(undefined, []).status, 'vitest-error')
  assert.equal(verdict(thrown, [record(0, '1n')]).status, 'phantom')
  assert.equal(verdict(collected([]), [record(0, 'x')]).status, 'phantom')
})

test('only long values rerun under another display cut', () => {
  assert.ok(holdsLongValue({ rows: [{ kind: 'array', cells: [[null, '1'], [null, LONG_VALUES[2]]] }] }))
  assert.ok(!holdsLongValue({ rows: [{ kind: 'array', cells: [[null, "'x'"]] }] }))
})
