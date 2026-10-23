// RECALL positive control -- duplicate-test expected, shape: a local helper whose object is
// written through a member expression before the call.
//
// One edit from a plain local helper: `register.extra = 1` writes a property of the function
// object, not the binding, so the declaration is still what the call runs and the helper must
// stay inlined. Pins the boundary of the reassignment refusal: a write through a member must
// not refuse it.
import { test, expect } from 'vitest'

/** Registers `old body`; the member write does not change that. */
function register() {
  test('old body', () => {
    expect(1).toBe(1)
  })
}

register.extra = 1

register()

test('independent', () => {
  expect(1).toBe(1)
})
