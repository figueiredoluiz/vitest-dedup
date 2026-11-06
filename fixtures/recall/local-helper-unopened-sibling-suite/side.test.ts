// RECALL negative -- helper calls beside a suite whose body is not opened to the expansion.
//
// A skipped suite's callback still runs while Vitest collects the file; this one throws, so Vitest
// collects nothing here. Only a bare suite with a rendered title opens its body to the sequence,
// so any other suite refuses the file's expansion and the two calls stay excluded.
//
// Load-bearing: delete the `describe.skip` block and the two calls expand and pair.
import { describe, expect, test } from 'vitest'

/** Registers one shared test. */
function makeTest() {
  test('shared', () => {
    expect(1).toBe(1)
  })
}

describe.skip('side', () => {
  throw new Error('boom')
})

describe('a', () => {
  makeTest()
})

describe('b', () => {
  makeTest()
})
