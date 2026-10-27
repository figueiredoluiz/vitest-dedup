// RECALL control -- duplicate-test absent, shape: a test declared through a local `const` arrow
// called at collection time.
//
// Vitest runs `hidden`, but only `function` declarations are inlined, so the arrow's body is not
// read. Reporting `visible` alone as complete would hide `hidden` from every finding; the call is
// refused instead and the file is set aside.
import { test, expect } from 'vitest'

const declare = () => {
  test('hidden', () => {
    expect(1).toBe(1)
  })
}

declare()

test('visible', () => {
  expect(1).toBe(1)
})
