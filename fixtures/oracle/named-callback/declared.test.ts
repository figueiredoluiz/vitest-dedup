// A function declaration passed by name, never written: it can be compared.
import { expect, test } from 'vitest'

/** Checks a value. */
function check() {
  expect(1 + 1).toBe(2)
}

test('by name', check)

test('stays comparable', () => {
  expect(1).toBe(1)
})
