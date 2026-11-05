// RECALL negative -- no duplicate-test, shape: a loop over an array literal while a suite has
// replaced `Array.prototype[Symbol.iterator]`.
//
// The replaced iterator records that it ran, so suite-a sees 1 and passes while suite-b sees 0
// and fails. The analyzer assumes built-ins are not replaced and reads the loop as running nothing.
import { describe, test, expect } from 'vitest'

const state = { n: 0 }
const original = Array.prototype[Symbol.iterator]
const install = () => {
  state.n = 0
  Array.prototype[Symbol.iterator] = function (this: unknown[]) {
    Array.prototype[Symbol.iterator] = original
    state.n = 1
    return original.call(this)
  }
}
const restore = () => { Array.prototype[Symbol.iterator] = original }

describe('suite-a', () => {
  install()
  for (const v of [1]) break
  restore()
  const seen = state.n
  test('reads', () => {
    expect(seen).toBe(1)
  })
})

describe('suite-b', () => {
  install()
  restore()
  const seen = state.n
  test('reads', () => {
    expect(seen).toBe(1)
  })
})
