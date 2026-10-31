// Control: the helper writes a local. One edit from `helper-write`.
import { expect, test } from 'vitest'

const count = 0

/** Writes a local. */
function bump() {
  let local = 0
  local++
  return local
}

test('bumps', () => {
  bump()
  expect(1).toBe(1)
})

test('reads', () => {
  expect(count).toBe(0)
})

test('stays comparable', () => {
  expect(1).toBe(1)
})
