// RECALL control -- duplicate-test absent, shape: a `forEach` callback registers tests over an array
// other code changes.
//
// Vitest runs the callback once per element, so `hidden` is registered and pairs with `visible`.
// The array is changed before the loop, so the analyzer cannot know its elements and refuses the
// file rather than reporting it complete.
import { test, expect } from 'vitest'

const rows = [1]
rows.push(2)
rows.forEach(() => {
  test('hidden', () => {
    expect(1).toBe(1)
  })
})

test('visible', () => {
  expect(1).toBe(1)
})
