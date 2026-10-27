// RECALL control -- duplicate-test absent, shape: an async `const` arrow that declares a test after
// an `await`.
//
// The promise never settles during collection, so Vitest never registers `hidden`. Reading the
// body would pair it with `visible`; the call is refused instead.
import { test, expect } from 'vitest'

/** Declares `hidden` once the promise settles. */
const declare = async () => {
  await new Promise(() => {})
  test('hidden', () => {
    expect(1).toBe(1)
  })
}

declare()

test('visible', () => {
  expect(1).toBe(1)
})
