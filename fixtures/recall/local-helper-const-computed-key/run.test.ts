// RECALL control -- duplicate-test absent, shape: a `const` helper call whose computed key registers.
//
// The key `String(beforeEach(...))` is evaluated before `setup` runs, so `hooked` has a hook
// `plain` lacks. The helper's body does not show it; the call is refused rather than compared
// without the hook.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { value: 0 }
/** Accepts options and declares nothing. */
const setup = (_options: unknown) => {}

describe('hooked', () => {
  setup({
    [String(
      beforeEach(() => {
        state.value = 1
      }),
    )]: 0,
  })
  test('reads', () => {
    expect(state.value).toBe(1)
  })
})

describe('plain', () => {
  test('reads', () => {
    expect(state.value).toBe(1)
  })
})
