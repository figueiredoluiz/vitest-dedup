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
  switch (true) { case true: if (enabled) break; break }
  beforeEach(() => {
    state.n = 1
  })
})

describe('suite-b', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  for (const value of [1]) { if (enabled) continue; break }
  beforeEach(() => {
    state.n = 1
  })
})
