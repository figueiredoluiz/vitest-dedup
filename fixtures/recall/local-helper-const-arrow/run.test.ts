// RECALL positive -- duplicate-test expected, shape: a test declared through a local `const` arrow
// called at collection time.
//
// The binding is a `const` initialized before the call, so the call runs the arrow's body, which
// declares `hidden`; it is read like a `function` declaration's and compared with `visible`.
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
