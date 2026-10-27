// RECALL control -- duplicate-test absent, shape: a generator helper called at collection time.
//
// Calling a generator creates an iterator and runs none of its body, so Vitest never registers
// `hidden`. Reading the body would pair it with `visible`; the call is refused instead.
import { test, expect } from 'vitest'

/** Declares `hidden` only if iterated. */
function* declare() {
  test('hidden', () => {
    expect(1).toBe(1)
  })
}

declare()

test('visible', () => {
  expect(1).toBe(1)
})
