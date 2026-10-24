// A helper return cannot skip hooks registered after its call.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
// Reset for every test so another suite's hook cannot mask an absent registration.
beforeEach(() => { state.n = 0 })

function finish(skip) {
  if (skip) return
}
for (const skip of [true]) finish(skip)

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
