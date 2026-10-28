// RECALL positive -- duplicate-test expected, shape: a `forEach` over a `const` array registers a
// test per element.
//
// Vitest runs the callback once per element, so `loop a` and `loop b` are registered and pair with
// `outside`, reported once for their shared declaration site. They do not pair with each other.
import { test, expect } from 'vitest'

const names = ['a', 'b']
names.forEach((name) => {
  test(`loop ${name}`, () => {
    expect(1).toBe(1)
  })
})

test('outside', () => {
  expect(1).toBe(1)
})
