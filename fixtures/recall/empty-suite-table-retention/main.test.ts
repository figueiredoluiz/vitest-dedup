// RECALL retention -- a suite table proved to have no rows registers nothing inside it.
//
// Vitest runs a `describe.each([])` callback zero times, so the two `settles` tests below never
// register (measured on Vitest 5.0.5: only `kept first` and `kept second` are collected). They have
// identical bodies, so an analyzer that reads them as tests reports a duplicate pair that cannot
// exist. The pair outside the tables is a real duplicate and must still be reported.
//
// Named weakening: give either table one row and its `settles` test registers once per row.
import { describe, expect, test } from 'vitest'

describe.each([])('first copy %s', () => {
  test('settles', () => {
    expect([1, 2].length).toBe(2)
  })
})

describe('outer', () => {
  describe.each([])('second copy %s', () => {
    test('settles', () => {
      expect([1, 2].length).toBe(2)
    })
  })
})

test('kept first', () => {
  expect('kept').toHaveLength(4)
})

test('kept second', () => {
  expect('kept').toHaveLength(4)
})
