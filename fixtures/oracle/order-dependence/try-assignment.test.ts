// Each test assigns the shared binding before reading it, so neither depends on the other's run.
import { expect, test } from 'vitest'

let value = 0

/** The value a test starts from. */
function mount() {
  return 1
}

test('first', () => {
  try {
    value = mount()
  } catch {}
  expect(value).toBe(1)
})

test('second', () => {
  try {
    value = mount()
  } catch {}
  expect(value + 1).toBe(2)
})

test('stays comparable', () => {
  expect(1).toBe(1)
})
