// A possible helper throw affects later hooks even when the call is unguarded.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
// Reset for every test so another suite's hook cannot mask an absent registration.
beforeEach(() => { state.n = 0 })

function finish(skip) {
  if (skip) throw new Error("stop")
}
finish(false)

describe('suite-a', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  beforeEach(() => {
    state.n = 1
  })
})

describe('suite-b', () => {
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  beforeEach(() => {
    state.n = 1
  })
})
