import { test, expect } from 'vitest'

test('a', () => {
  /** Return the value that satisfies this test's assertion. */
  function a() { return 1 }
  /** Return the contrasting value from the unselected declaration. */
  function b() { return 2 }
  expect(a()).toBe(1)
})
test('b', () => {
  /** Return the satisfying value from the unselected declaration. */
  function a() { return 1 }
  /** Return the value that makes this test fail despite its matching structure. */
  function b() { return 2 }
  expect(b()).toBe(1)
})
