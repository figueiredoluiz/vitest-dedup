// RECALL positive control -- duplicate-test expected, shape: a local `let` helper refused inside a
// describe, beside a duplicate pair outside it.
//
// The refusal sets aside the scope the call runs in (`inner`), not its siblings: `one` and `two`
// are still compared and reported.
import { describe, test, expect } from 'vitest'

describe('group', () => {
  let declare = (name: string) => {
    test(name, () => {
      expect(1).toBe(1)
    })
  }
  declare('hidden')

  test('inner', () => {
    expect(2).toBe(2)
  })
})

test('one', () => {
  expect(3).toBe(3)
})

test('two', () => {
  expect(3).toBe(3)
})
