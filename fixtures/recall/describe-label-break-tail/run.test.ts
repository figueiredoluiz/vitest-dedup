// RECALL positive -- duplicate-test expected, shape: a labeled block breaks out before its `return`.
//
// `break out` leaves the block first, so the `return` never runs and the callback registers `live`.
// `live` pairs with `visible`; counting the unreachable `return` would drop it silently.
import { describe, test, expect } from 'vitest'

describe('suite', () => {
  out: {
    break out
    return
  }
  test('live', () => {
    expect(1).toBe(1)
  })
})

test('visible', () => {
  expect(1).toBe(1)
})
