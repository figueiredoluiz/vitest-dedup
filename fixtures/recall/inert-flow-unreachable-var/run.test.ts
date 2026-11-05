// RECALL negative -- no duplicate-test, shape: a hoisted `var` in a block a suite never reaches.
//
// `var x` binds `x` in suite-a's callback even though its initializer never runs, so suite-a reads
// `undefined` and fails while suite-b passes. Dropping the dead block as inert would pair them.
import { describe, test, expect } from 'vitest'

describe('suite-a', () => {
  out: { break out; { var x = 1 } }
  test('reads', () => {
    expect(x).toBe(1)
  })
})

describe('suite-b', () => {
  var x = 1
  test('reads', () => {
    expect(x).toBe(1)
  })
})
