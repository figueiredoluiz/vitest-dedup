// RECALL control -- duplicate-test absent, shape: a helper that returns the hook it registers.
//
// `return beforeEach(...)` runs the registration before it leaves, so `hooked` has a hook `plain`
// lacks. The returned call is read like the statement it evaluates, the hook joins `hooked`, and
// the two `reads` tests are compared and found distinct.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { value: 0 }
/** Registers a hook and returns it. */
function setup() {
  return beforeEach(() => {
    state.value = 1
  })
}

describe('hooked', () => {
  setup()

  test('reads', () => {
    expect(state.value).toBe(1)
  })
})

describe('plain', () => {
  test('reads', () => {
    expect(state.value).toBe(1)
  })
})
