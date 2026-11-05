// RECALL negative -- no duplicate-test, shape: a getter read as a suite's branch condition.
//
// `if (o.go)` runs the getter, which registers a hook in suite-a; `if (on)` reads a constant.
// suite-a passes and suite-b fails, so reading `o.go` as a constant would pair them.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
const on = true
beforeEach(() => { state.n = 0 })
const hook = () => { beforeEach(() => { state.n = 1 }); return true }
const o = { get go() { return hook() } }

describe('suite-a', () => {
  stop: { if (o.go) break stop }
  test('reads', () => {
    expect(state.n).toBe(1)
  })
})

describe('suite-b', () => {
  stop: { if (on) break stop }
  test('reads', () => {
    expect(state.n).toBe(1)
  })
})
