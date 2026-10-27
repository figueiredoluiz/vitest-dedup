// RECALL positive -- duplicate-test expected, shape: a finalizer's `break` stays inside its own loop.
//
// The `break` only leaves the loop, so the pending `return` completes and `dead` is never
// registered. `hidden` pairs with `visible`; `dead` must not pair with either.
import { test, expect } from 'vitest'

/** Declares `hidden`; the tail never runs. */
function declare() {
  try {
    return test('hidden', () => {
      expect(1).toBe(1)
    })
  } finally {
    for (;;) {
      break
    }
  }
  test('dead', () => {
    expect(1).toBe(1)
  })
}

declare()

test('visible', () => {
  expect(1).toBe(1)
})
