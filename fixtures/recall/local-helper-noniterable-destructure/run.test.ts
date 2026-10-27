// RECALL control -- duplicate-test absent, shape: a helper array-destructures a non-iterable.
//
// Binding `[value]` to `{}` throws before the body runs, and the caller catches it, so Vitest never
// registers `hidden`. Reading the body would pair it with `visible`; the call is refused.
import { test, expect } from 'vitest'

/** Declares `hidden` once its row binds. */
function declare([_value]) {
  return test('hidden', () => {
    expect(1).toBe(1)
  })
}

try {
  declare({})
} catch {}

test('visible', () => {
  expect(1).toBe(1)
})
