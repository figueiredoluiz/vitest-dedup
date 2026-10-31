// Control that stays excluded: one callback shared by two tests, so its writes are the other's.
import { expect, test } from 'vitest'

let runs = 0

/** Counts its runs. */
function counts() {
  runs += 1
  expect(runs).toBeGreaterThan(0)
}

test('first', counts)
test('second', counts)

test('stays comparable', () => {
  expect(1).toBe(1)
})
