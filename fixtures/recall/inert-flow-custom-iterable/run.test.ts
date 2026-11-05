// RECALL negative -- no duplicate-test, shape: a suite loop over an iterable that runs code.
//
// Iterating `items` runs its generator, which registers a hook in suite-a; iterating `[1]` runs
// nothing. suite-a passes and suite-b fails, so reading `items` as an array would pair them.
import { describe, test, expect, beforeEach } from 'vitest'

const state = { n: 0 }
beforeEach(() => { state.n = 0 })
const hook = () => { beforeEach(() => { state.n = 1 }); return 1 }
const items = { *[Symbol.iterator]() { yield hook() } }

describe('suite-a', () => {
  for (const item of items) break
  test('reads', () => {
    expect(state.n).toBe(1)
  })
})

describe('suite-b', () => {
  for (const item of [1]) break
  test('reads', () => {
    expect(state.n).toBe(1)
  })
})
