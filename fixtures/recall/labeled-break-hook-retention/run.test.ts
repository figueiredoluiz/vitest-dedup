// Function returns escape a label; local breaks resume after their target.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
const enabled = true
// Reset for every test so another suite's hook cannot mask an absent registration.
beforeEach(() => { state.n = 0 })

describe('suite-a', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  stop: { if (enabled) break stop }
  beforeEach(() => {
    state.n = 1
  })
})

describe('suite-b', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  stop: for (const a of [1]) { inner: for (const b of [1]) { continue inner } for (const c of [1]) { if (enabled) break stop } }
  beforeEach(() => {
    state.n = 1
  })
})
