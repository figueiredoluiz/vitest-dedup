// RECALL positive -- duplicate-test, shape: setup helper `setupLocalConst` called at the top level.
//
// The helper registers nothing and the two tests below are identical, so a duplicate-test is wanted
// once the helper is proven not to touch what the tests compute. Today the refused call sets the
// whole file aside (not-compared). Add a third statement to one test body and the pair differs.
import { expect, test } from 'vitest'
import { setupLocalConst } from './helper-localconst'

setupLocalConst()

test('localconst: first check', () => {
  expect([1, 2, 3].map((n) => n * 8)).toEqual([8, 16, 24])
})

test('localconst: second check', () => {
  expect([1, 2, 3].map((n) => n * 8)).toEqual([8, 16, 24])
})
