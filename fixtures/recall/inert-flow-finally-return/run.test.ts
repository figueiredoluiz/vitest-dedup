// RECALL negative -- no duplicate-test, shape: a finalizer's `return` cancels a suite's `break`.
//
// In suite-a the `return` in the `finally` replaces the pending `break exit`, so the hook after it
// never registers; in suite-b the finalizer's `break exit` replaces the `return` and the hook
// registers. suite-a fails and suite-b passes, so letting the `break` win in both would pair them.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
beforeEach(() => { state.n = 0 })

describe('suite-a', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  exit: { try { break exit } finally { return } }
  beforeEach(() => {
    state.n = 1
  })
})

describe('suite-b', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  exit: { try { return } finally { break exit } }
  beforeEach(() => {
    state.n = 1
  })
})
