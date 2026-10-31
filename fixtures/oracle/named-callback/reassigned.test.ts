// Control that stays excluded: the declaration is rewritten before it runs.
import { expect, test } from 'vitest'

/** Checks a value until it is rewritten. */
function check() {
  expect(1 + 1).toBe(2)
}
// @ts-ignore
check = () => expect(2).toBe(2)

test('by name', check)

test('stays comparable', () => {
  expect(1).toBe(1)
})
