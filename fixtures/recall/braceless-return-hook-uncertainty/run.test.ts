// A single-statement loop body can return before later hooks register.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
const enabled = true
// Reset for every test so another suite's hook cannot mask an absent registration.
beforeEach(() => { state.n = 0 })

describe('suite-a', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  while (enabled) return
  beforeEach(() => {
    state.n = 1
  })
})

describe('suite-b', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  while (!enabled) return
  beforeEach(() => {
    state.n = 1
  })
})
