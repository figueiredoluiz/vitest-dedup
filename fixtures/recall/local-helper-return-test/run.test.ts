// RECALL positive -- duplicate-test expected, shape: a helper that returns the test it declares.
//
// `return test(...)` registers `hidden` before it leaves; it is read like the statement it
// evaluates and compared with `visible`.
import { test, expect } from 'vitest'

/** Declares `hidden` and returns it. */
function declare() {
  return test('hidden', () => {
    expect(1).toBe(1)
  })
}

declare()

test('visible', () => {
  expect(1).toBe(1)
})
