// RECALL control -- duplicate-test absent, shape: a `const` generator function called at
// collection time.
//
// Calling a generator creates an iterator and runs none of its body, so Vitest never registers
// `hidden`. Reading the body would pair it with `visible`; the call is refused instead.
import { test, expect } from 'vitest'

const declare = function* () {
  test('hidden', () => {
    expect(1).toBe(1)
  })
}

declare()

test('visible', () => {
  expect(1).toBe(1)
})
