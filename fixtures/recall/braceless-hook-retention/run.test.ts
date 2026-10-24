// A single-statement local break or continue does not skip hooks after its loop.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
const enabled = true
// Reset for every test so another suite's hook cannot mask an absent registration.
beforeEach(() => { state.n = 0 })

describe('suite-a', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  while (enabled) break
  beforeEach(() => {
    state.n = 1
  })
})

describe('suite-b', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  for (const n of [1]) continue
  beforeEach(() => {
    state.n = 1
  })
})
