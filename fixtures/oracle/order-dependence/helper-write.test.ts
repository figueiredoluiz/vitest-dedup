// `reads` passes only after `bumps` has run: the write is inside a helper a test calls.
import { expect, test } from 'vitest'

let count = 0

/** Writes the shared binding. */
function bump() {
  count++
}

test('bumps', () => {
  bump()
  expect(1).toBe(1)
})

test('reads', () => {
  expect(count).toBe(1)
})

test('stays comparable', () => {
  expect(1).toBe(1)
})
