// RECALL positive -- duplicate-test expected, shape: a helper returns a test and has a dead tail.
//
// `return test(...)` registers `hidden` and leaves, so `dead` is never registered. `hidden` pairs
// with `visible`; `dead` must not pair with either.
import { test, expect } from 'vitest'

/** Declares `hidden`; the tail never runs. */
function declare() {
  return test('hidden', () => {
    expect(1).toBe(1)
  })
  test('dead', () => {
    expect(1).toBe(1)
  })
}

declare()

test('visible', () => {
  expect(1).toBe(1)
})
