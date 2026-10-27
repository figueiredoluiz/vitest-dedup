// RECALL control -- duplicate-test absent, shape: a helper that destructures an argument whose
// getter registers a hook.
//
// Binding `{ value }` reads `input.value` before the body runs, and the getter registers
// `beforeEach` in `hooked`. The argument is only a name, but destructuring reads through it, so it
// is scanned, the getter is found and the call is refused.
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

/** Reads `value` from what it is given. */
function setup({ value: _value }: { value: number }) {}

describe('hooked', () => {
  setup(input)

  test('reads', () => {
    expect(state.value).toBe(1)
  })
})

describe('plain', () => {
  test('reads', () => {
    expect(state.value).toBe(1)
  })
})
