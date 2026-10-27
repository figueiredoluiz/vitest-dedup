// RECALL positive -- duplicate-test expected, shape: a finalizer's `break` cancels a suite's `return`.
//
// `break exit` in the `finally` replaces the pending `return`, so the callback goes on and
// registers `live`. `live` pairs with `visible`; reading it as dead would drop it silently.
import { describe, test, expect } from 'vitest'

describe('suite', () => {
  exit: {
    try {
      return
    } finally {
      break exit
    }
  }
  test('live', () => {
    expect(1).toBe(1)
  })
})

test('visible', () => {
  expect(1).toBe(1)
})
