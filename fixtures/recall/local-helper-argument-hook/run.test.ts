// RECALL control -- duplicate-test absent, shape: a helper called with an argument that registers
// a hook.
//
// The argument runs before the helper's body and registers `beforeEach` in `hooked`, which the
// body does not show. Reading only the body would drop the hook and pair the two `reads` tests;
// the call is refused and its scope set aside.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { value: 0 }
/** Ignores what it is given. */
function setup(_hook: unknown) {}

describe('hooked', () => {
  setup(
    beforeEach(() => {
      state.value = 1
    }),
  )

  test('reads', () => {
    expect(state.value).toBe(1)
  })
})

describe('plain', () => {
  test('reads', () => {
    expect(state.value).toBe(1)
  })
})
