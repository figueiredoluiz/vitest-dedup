// A named callback after a conditional exit must refuse comparison, just like an inline one.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
// Reset for every test so another suite's hook cannot mask an absent registration.
beforeEach(() => { state.n = 0 })

describe('suite-a', () => {
  function reset() { state.n = 1 }
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  switch (true) {
    case true: return
  }
  beforeEach(reset)
})

describe('suite-b', () => {
  function reset() { state.n = 1 }
  test('reads', () => {
    expect(state.n).toBe(1)
  })
  switch (false) {
    case true: return
  }
  beforeEach(reset)
})
