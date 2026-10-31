// Control: the same tests with the assignment outside `try`. One edit from `try-assignment`.
import { expect, test } from 'vitest'

let value = 0

/** The value a test starts from. */
function mount() {
  return 1
}

test('first', () => {
  value = mount()
  expect(value).toBe(1)
})

test('second', () => {
  value = mount()
  expect(value + 1).toBe(2)
})

test('stays comparable', () => {
  expect(1).toBe(1)
})
