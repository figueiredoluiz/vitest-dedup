// RECALL control -- duplicate-test absent, shape: an inlined helper that calls the callback it is
// handed.
//
// `fn` holds whatever the caller passes, here a callback that declares `hidden`. The parameter's
// value is not read, so the call is refused and the file is set aside rather than reported
// complete without `hidden`.
import { test, expect } from 'vitest'

/** Runs `fn` while the file is collected. */
function wrap(fn: () => void) {
  fn()
}

wrap(() => {
  test('hidden', () => {
    expect(1).toBe(1)
  })
})

test('visible', () => {
  expect(1).toBe(1)
})
