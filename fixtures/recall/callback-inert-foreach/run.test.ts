// RECALL positive -- duplicate-test expected, shape: a `forEach` callback that registers nothing.
//
// The callback only fills an array and reads no parameter, so the file stays comparable and `first`
// pairs with `second`.
import { test, expect } from 'vitest'

const seen: number[] = []
;[1, 2].forEach(() => {
  seen.push(seen.length)
})

test('first', () => {
  expect(1).toBe(1)
})

test('second', () => {
  expect(1).toBe(1)
})
