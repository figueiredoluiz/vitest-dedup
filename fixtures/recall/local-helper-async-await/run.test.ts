// RECALL control -- duplicate-test absent, shape: an async helper that registers a hook after an
// `await`.
//
// Whether a registration after `await` happens during collection depends on when the promise
// settles, which the analyzer does not model. The call is refused instead of guessing.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { value: 0 }
/** Waits, then registers a hook. */
async function setup() {
  await Promise.resolve()
  beforeEach(() => {
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
