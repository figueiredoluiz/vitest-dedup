// RECALL control -- duplicate-test absent, shape: a helper argument read through a getter that
// registers a hook.
//
// Reading `input.value` runs the getter, which registers `beforeEach` in `hooked`. A member read
// is not admitted as inert, so the scan follows the getter and the call is refused.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { value: 0 }
const input = {
  get value() {
    beforeEach(() => {
      state.value = 1
    })
    return 0
  },
}

/** Ignores what it is given. */
function setup(_value: unknown) {}

describe('hooked', () => {
  setup(input.value)

  test('reads', () => {
    expect(state.value).toBe(1)
  })
})

describe('plain', () => {
  test('reads', () => {
    expect(state.value).toBe(1)
  })
})
