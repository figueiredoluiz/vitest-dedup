// RECALL positive -- one helper called twice with each of two arguments.
//
// Vitest registers `value 1` twice and `value 2` twice from one written declaration: four
// runtime cases and two `duplicate-test` pairs, one per argument.
//
// Load-bearing: the `value 2` pair is a separate claim from the `value 1` pair; delete one
// `run(2)` and only the `value 1` pair remains.
import { expect, test } from 'vitest'

/** Registers one test for `v`. */
function run(v: number) {
  test(`value ${v}`, () => {
    expect(v).toBe(v)
  })
}

run(1)
run(1)
run(2)
run(2)
