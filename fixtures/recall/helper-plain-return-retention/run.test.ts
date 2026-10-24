// A plain helper return leaves later caller hook registrations unconditional.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
// Reset for every test so another suite's hook cannot mask an absent registration.
beforeEach(() => { state.n = 0 })

function finish(skip) {
  if (skip) return
}
finish(true)

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
