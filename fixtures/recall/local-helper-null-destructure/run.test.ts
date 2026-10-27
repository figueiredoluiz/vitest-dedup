// RECALL control -- duplicate-test absent, shape: a helper whose destructuring parameter gets `null`.
//
// Binding `{ value }` to `null` throws before the body runs, and the caller catches it, so Vitest
// never registers `hidden`. Reading the body would pair it with `visible`; the call is refused.
import { test, expect } from 'vitest'

/** Declares `hidden` once its options bind. */
function declare({ value: _value }) {
  return test('hidden', () => {
    expect(1).toBe(1)
  })
}

try {
  declare(null)
} catch {}

test('visible', () => {
  expect(1).toBe(1)
})
