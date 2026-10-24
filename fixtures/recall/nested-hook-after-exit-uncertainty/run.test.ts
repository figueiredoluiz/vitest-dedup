// A parent exit before nested suites leaves their hook registration uncertain.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
// Reset for every test so another suite's hook cannot mask an absent registration.
beforeEach(() => { state.n = 0 })

describe('outer', () => {
  switch (false) { case true: return }
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
})
