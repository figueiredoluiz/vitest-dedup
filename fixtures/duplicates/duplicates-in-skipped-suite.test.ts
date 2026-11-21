// Two tests with identical bodies where the second sits inside a `describe.skip` suite.
// Expected verdict: duplicate-test, with the report noting that the skipped suite's test is inert.
// NOTE: bodies must be non-trivial, or the triviality guard excludes them from comparison.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L11 "validates input"
// @vitest-dedup:participant L17 "skipped suite > validates input again" [inert]
import { describe, expect, test } from 'vitest'
import { validate } from './validate'

test('validates input', () => {
  const outcome = validate('skipped-suite')
  expect(outcome.ok).toBe(true)
})

describe.skip('skipped suite', () => {
  test('validates input again', () => {
    const outcome = validate('skipped-suite')
    expect(outcome.ok).toBe(true)
  })
})
