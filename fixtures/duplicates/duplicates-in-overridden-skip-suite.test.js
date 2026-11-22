// Two tests with identical bodies where the second sits inside a suite whose `skip: true` a later
// duplicate key overrides with `false`. Vitest runs that suite, so neither participant is inert.
// NOTE: bodies must be non-trivial, or the triviality guard excludes them from comparison.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L11 "validates input"
// @vitest-dedup:participant L17 "overridden suite > validates input again"
import { describe, expect, test } from 'vitest'
import { validate } from './validate'

test('validates input', () => {
  const outcome = validate('overridden-suite')
  expect(outcome.ok).toBe(true)
})

describe('overridden suite', { skip: true, skip: false }, () => {
  test('validates input again', () => {
    const outcome = validate('overridden-suite')
    expect(outcome.ok).toBe(true)
  })
})
