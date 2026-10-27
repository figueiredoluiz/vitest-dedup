// RECALL positive -- duplicate-test expected, shape: a suite callback returns before its last test.
//
// The callback leaves after `first`, so Vitest never registers `dead`. `first` pairs with
// `visible`; `dead` must not pair with either.
import { describe, test, expect } from 'vitest'

describe('suite', () => {
  test('first', () => {
    expect(1).toBe(1)
  })
  return
  test('dead', () => {
    expect(1).toBe(1)
  })
})

test('visible', () => {
  expect(1).toBe(1)
})
