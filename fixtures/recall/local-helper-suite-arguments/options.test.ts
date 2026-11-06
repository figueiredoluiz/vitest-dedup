// RECALL negative -- a helper called inside a suite whose third argument runs code.
//
// The suite's third argument is evaluated while the suite registers; here it throws, so Vitest
// fails collecting the file and neither `shared` test registers. A suite opens its body to the
// expansion only with exactly a title and a callback, so nothing here is compared.
//
// Load-bearing: delete the third argument and the two calls expand and pair.
import { describe, expect, test } from 'vitest'

/** Registers one shared test. */
function makeTest() {
  test('shared', () => {
    expect(1).toBe(1)
  })
}

describe('s', () => {
  makeTest()
}, (() => { throw new Error('boom') })())

describe('t', () => {
  makeTest()
})
