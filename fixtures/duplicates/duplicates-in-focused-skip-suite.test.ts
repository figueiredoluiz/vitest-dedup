// Two tests with identical bodies where the second sits inside a `describe.skip.only` suite.
// Expected verdict: duplicate-test, with neither test inert: the focus overrides the suite's skip.
// NOTE: bodies must be non-trivial, or the triviality guard excludes them from comparison.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L11 "validates input"
// @vitest-dedup:participant L17 "focused suite > validates input again"
import { describe, expect, test } from 'vitest'
import { validate } from './validate'

test('validates input', () => {
  const outcome = validate('focused-suite')
  expect(outcome.ok).toBe(true)
})

describe.skip.only('focused suite', () => {
  test('validates input again', () => {
    const outcome = validate('focused-suite')
    expect(outcome.ok).toBe(true)
  })
})
