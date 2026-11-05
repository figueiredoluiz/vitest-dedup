// RECALL negative -- no duplicate-test, shape: a suite branches on a `declare const`.
//
// `declare const on` is erased, so `if (on)` reads the global getter below, which counts its
// reads. suite-b runs first and sees 0; suite-a's branch runs the getter, so it sees 1 and fails.
// Reading `on` as the constant `true` would pair them.
import { describe, test, expect } from 'vitest'

const state = { n: 0 }
Object.defineProperty(globalThis, 'on', { configurable: true, get() { state.n += 1; return true } })
declare const on = true

describe('suite-b', () => {
  const seen = state.n
  test('reads', () => {
    expect(seen).toBe(0)
  })
})

describe('suite-a', () => {
  stop: { if (on) break stop }
  const seen = state.n
  test('reads', () => {
    expect(seen).toBe(0)
  })
})
