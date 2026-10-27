// RECALL positive -- duplicate-test expected, shape: a helper that returns the suite it declares.
//
// `return describe(...)` registers the suite and its test; `group > inner` is read and compared
// with the top-level `inner`.
import { describe, test, expect } from 'vitest'

/** Declares a suite named `name` and returns it. */
function suite(name: string) {
  return describe(name, () => {
    test('inner', () => {
      expect(1).toBe(1)
    })
  })
}

suite('group')

test('inner', () => {
  expect(1).toBe(1)
})
