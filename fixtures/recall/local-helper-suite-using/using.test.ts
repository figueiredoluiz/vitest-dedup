// RECALL negative -- helper calls in a suite body that holds a `using` declaration.
//
// `using` disposes its value when the suite body ends, which runs code; this value is not
// disposable, so Vitest fails collecting the file. A suite body holding one does not close, so
// the two calls stay excluded.
//
// Load-bearing: write `const` for `using` and the two calls expand and pair.
import { describe, expect, test } from 'vitest'

/** Registers one shared test. */
function run() {
  test('shared', () => {
    expect(1).toBe(1)
  })
}

describe('s', () => {
  using resource = {}
  run()
  run()
})
