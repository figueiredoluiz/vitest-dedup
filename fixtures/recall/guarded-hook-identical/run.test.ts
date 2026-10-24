// RECALL control -- duplicate-test absent, shape: two identical suites whose identical hooks
// sit under the same `if` condition.
//
// The twin of `guarded-hook-differing`: whether the guards differ does not matter -- any
// guarded registration makes its scope's contexts incomparable, because the runtime
// registration depends on control flow and values this analyzer does not evaluate. Both files
// are set aside rather than compared.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }

describe('suite-a', () => {
  if (true) beforeEach(() => {
    state.n = 1
  })
  test('reads', () => {
    expect(state.n).toBe(1)
  })
})

describe('suite-b', () => {
  if (true) beforeEach(() => {
    state.n = 1
  })
  test('reads', () => {
    expect(state.n).toBe(1)
  })
})
