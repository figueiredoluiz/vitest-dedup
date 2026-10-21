// RECALL positive -- duplicate-test, shape: setup helper `setupObjectKeys` called at the top level.
//
// The helper registers nothing and the two tests below are identical, so a duplicate-test is wanted
// once the helper is proven not to touch what the tests compute. Today the refused call sets the
// whole file aside (not-compared). Add a third statement to one test body and the pair differs.
import { expect, test } from 'vitest'
import { setupObjectKeys } from './helper-objectkeys'

setupObjectKeys()

test('objectkeys: first check', () => {
  expect([1, 2, 3].map((n) => n * 5)).toEqual([5, 10, 15])
})

test('objectkeys: second check', () => {
  expect([1, 2, 3].map((n) => n * 5)).toEqual([5, 10, 15])
})
