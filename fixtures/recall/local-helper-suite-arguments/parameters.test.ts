// RECALL negative -- a helper called inside a suite whose callback parameter runs code.
//
// A parameter default is evaluated before the suite's body runs; here it throws, so Vitest fails
// collecting the file. A suite callback that takes any parameter does not open its body to the
// expansion, so nothing here is compared.
//
// Load-bearing: drop the parameter and the two calls expand and pair.
import { describe, expect, test } from 'vitest'

/** Registers one shared test. */
function makeTest() {
  test('shared', () => {
    expect(1).toBe(1)
  })
}

describe('s', ({ missing = (() => { throw new Error('boom') })() }) => {
  makeTest()
})

describe('t', () => {
  makeTest()
})
