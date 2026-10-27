// RECALL positive -- duplicate-test expected, shape: a helper destructures a local named `undefined`.
//
// The local `undefined` is an object, so `{ value }` binds and the body registers `hidden`.
// `hidden` pairs with `visible`; reading the name as the global would refuse the call.
import { describe, test, expect } from 'vitest'

describe('suite', () => {
  const undefined = { value: 1 }
  /** Declares `hidden` once its options bind. */
  function declare({ value: _value }) {
    test('hidden', () => {
      expect(1).toBe(1)
    })
  }
  declare(undefined)
})

test('visible', () => {
  expect(1).toBe(1)
})
