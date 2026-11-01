// RECALL positive -- duplicate-test expected, shape: a binding read at or before its destructuring.
//
// The behavior trace cannot move the object's read to these uses. It must still read the file:
// each pair is identical, and a failed trace must not cost the file.
import { test } from 'vitest'

/** Takes any value. */
declare function use(...values: unknown[]): void

const shared = { a: 'a' }

test('before one', () => {
  use(a)
  const { a } = shared
  use(a)
})

test('before two', () => {
  use(a)
  const { a } = shared
  use(a)
})

test('within one', () => {
  const { a, [a]: b } = shared
  use(a, b)
})

test('within two', () => {
  const { a, [a]: b } = shared
  use(a, b)
})
