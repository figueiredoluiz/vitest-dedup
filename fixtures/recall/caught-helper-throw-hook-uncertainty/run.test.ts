// A helper throw the caller catches lets collection continue without the later hooks; keep that
// uncertainty.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
// Reset for every test so another suite's hook cannot mask an absent registration.
beforeEach(() => { state.n = 0 })

/** Throws when asked to stop. */
function finish(skip) {
  if (skip) throw new Error('stop')
}
try {
  finish(false)
} catch {}

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
