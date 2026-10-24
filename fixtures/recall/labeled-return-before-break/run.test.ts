// A local tail jump must preserve earlier function exits and normal fallthrough.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
const enabled = true
// Reset for every test so another suite's hook cannot mask an absent registration.
beforeEach(() => { state.n = 0 })

describe('suite-a', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  stop: { if (enabled) return; break stop }
  beforeEach(() => {
    state.n = 1
  })
})

describe('suite-b', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  stop: { if (!enabled) return; break stop }
  beforeEach(() => {
    state.n = 1
  })
})
