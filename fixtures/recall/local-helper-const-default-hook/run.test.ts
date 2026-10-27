// RECALL control -- duplicate-test absent, shape: a `const` helper whose parameter default
// registers a hook.
//
// `setup()` evaluates the default, which registers `beforeEach` in `hooked` before the body runs.
// Reading only the body would drop the hook and pair the two `reads` tests; the call is refused and
// its scope set aside.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { value: 0 }
const setup = (
  _hook = beforeEach(() => {
    state.value = 1
  }),
) => {}

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
