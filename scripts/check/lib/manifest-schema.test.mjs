// Tests for the recall manifest label schema. Every rejection test starts from a valid case and
// breaks one thing, so each fails only if that one check is removed.
import assert from 'node:assert/strict'
import test from 'node:test'

import {
  CASE_KEYS,
  isCalendarDate,
  ORIGINS,
  ROOT_KEYS,
  validateCaseLabels,
  validateManifestLabels,
  validateRootKeys,
  VERDICTS,
} from './manifest-schema.mjs'

const valid = () => ({
  name: 'a-case',
  path: 'a-case',
  origin: 'synthetic',
  verdict: 'generated',
  classifiedAt: '2026-10-20',
})

test('a case with valid labels yields no failures, for every origin and verdict', () => {
  for (const origin of ORIGINS) {
    for (const verdict of VERDICTS) {
      assert.deepEqual(validateCaseLabels({ ...valid(), origin, verdict }, 0), [])
    }
  }
})

test('each label field is required and rejects a value outside its set', () => {
  for (const [field, bad] of [
    ['origin', undefined],
    ['origin', 'Synthetic'],
    ['origin', null],
    ['verdict', undefined],
    ['verdict', 'approved'],
    ['classifiedAt', undefined],
    ['classifiedAt', 20261020],
  ]) {
    const failures = validateCaseLabels({ ...valid(), [field]: bad }, 0)
    assert.equal(failures.length, 1, `${field}=${String(bad)}`)
    assert.match(failures[0], new RegExp(`^FAIL case a-case: \`${field}\``))
  }
})

test('classifiedAt accepts only a real calendar date', () => {
  for (const good of ['2026-10-20', '2024-02-29']) assert.equal(isCalendarDate(good), true, good)
  const bad = ['2026-02-30', '2025-02-29', '2026-13-01', '2026-1-1', '2026-10-20T00:00:00Z', '', null, undefined, NaN, {}]
  for (const value of bad) assert.equal(isCalendarDate(value), false, String(value))
})

test('an unknown key fails and is named; every known key passes', () => {
  const failures = validateCaseLabels({ ...valid(), verdct: 'reviewed' }, 0)
  assert.equal(failures.length, 1)
  assert.match(failures[0], /unknown key `verdct`/)
  const everyKnown = Object.fromEntries([...CASE_KEYS].map((key) => [key, undefined]))
  assert.deepEqual(validateCaseLabels({ ...everyKnown, ...valid() }, 0), [])
})

test('an unknown matcher key fails in every matcher group, naming the group and position', () => {
  const matcher = { rule: 'duplicate-test', titles: ['a', 'b'] }
  for (const field of ['expectedFindings', 'expectedAbsent', 'knownMisses', 'knownFalsePositives']) {
    const failures = validateCaseLabels({ ...valid(), [field]: [matcher, { ...matcher, titels: [] }] }, 0)
    assert.equal(failures.length, 1, field)
    assert.match(failures[0], new RegExp(`${field}\\[1\\]: unknown key \`titels\``))
    assert.deepEqual(validateCaseLabels({ ...valid(), [field]: [matcher, null, 'x', []] }, 0), [], field)
  }
  assert.deepEqual(validateCaseLabels({ ...valid(), expectedFindings: 'not-a-list' }, 0), [])
})

test('an unknown manifest root key fails; every known root key passes', () => {
  const rootKeys = Object.fromEntries([...ROOT_KEYS].map((key) => [key, undefined]))
  assert.deepEqual(validateRootKeys(rootKeys), [])
  const failures = validateManifestLabels({ cases: [valid()], minimumRecal: 1 })
  assert.equal(failures.length, 1)
  assert.match(failures[0], /^FAIL manifest: unknown key `minimumRecal`/)
})

test('a case with no usable name is addressed by position, and a non-object case fails', () => {
  assert.match(validateCaseLabels({ ...valid(), name: '', origin: 'x' }, 7)[0], /^FAIL case #7:/)
  for (const bad of [null, 'a-case', 3, []]) {
    assert.deepEqual(validateCaseLabels(bad, 2), ['FAIL case #2: a case must be an object'])
  }
})

test('manifest validation covers every case and tolerates an unreadable cases field', () => {
  const broken = { ...valid(), name: 'broken', verdict: 'nope' }
  const failures = validateManifestLabels({ cases: [valid(), broken, { ...valid(), name: 'b', origin: 1 }] })
  assert.equal(failures.length, 2)
  assert.match(failures[0], /case broken/)
  assert.match(failures[1], /case b/)
  for (const manifest of [undefined, null, {}, { cases: 'x' }, { cases: [] }]) {
    assert.deepEqual(validateManifestLabels(manifest), [])
  }
})
