// Two tests with identical bodies where the second sits inside a suite with `{ todo: true }`.
// Expected verdict: duplicate-test, with the report noting that the todo suite's test is inert.
// NOTE: bodies must be non-trivial, or the triviality guard excludes them from comparison.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L11 "validates input"
// @vitest-dedup:participant L17 "todo suite > validates input again" [inert]
import { describe, expect, test } from 'vitest'
import { validate } from './validate'

test('validates input', () => {
  const outcome = validate('todo-suite')
  expect(outcome.ok).toBe(true)
})

describe('todo suite', { todo: true }, () => {
  test('validates input again', () => {
    const outcome = validate('todo-suite')
    expect(outcome.ok).toBe(true)
  })
})
