// RECALL control -- duplicate-test absent, shape: a helper whose parameter default registers a
// hook.
//
// `setup()` evaluates the default, which registers `beforeEach` in `hooked` before the body runs.
// Reading only the body would drop the hook and pair the two `reads` tests; the call is refused.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { value: 0 }
/** Registers a hook through its default, then does nothing. */
function setup(
  _hook = beforeEach(() => {
    state.value = 1
  }),
) {}

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
