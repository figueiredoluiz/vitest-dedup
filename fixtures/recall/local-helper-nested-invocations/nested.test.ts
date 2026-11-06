// RECALL positive -- one helper call that runs another helper twice.
//
// `outer()` calls `run()` twice, so Vitest registers `s > shared` twice from the same written
// declaration: two runtime cases, one per nested call, and one `duplicate-test` pair.
//
// Load-bearing: delete one `run()` from `outer` and the pair goes.
import { describe, expect, test } from 'vitest'

/** Registers one shared test. */
function run() {
  test('shared', () => {
    expect(1).toBe(1)
  })
}

/** Registers the shared test twice. */
function outer() {
  run()
  run()
}

describe('s', () => {
  outer()
})
