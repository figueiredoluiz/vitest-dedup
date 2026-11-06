// RECALL positive -- duplicate-test-name.
//
// Tests with nothing to compare still register their full titles. Two empty callbacks, a todo
// beside a `test.todo`, two tests with options only, a todo beside an empty test in one suite, an
// empty test beside a written one: each pair shares a full title, so each is one finding. Nothing is
// compared by body: the empty tests and todos never pair as `duplicate-test`.
//
// Load-bearing: rename either member of a pair and its finding goes.
import { describe, expect, test } from 'vitest'

test('loads the cart', () => {})
test('loads the cart', () => {})

test('saves the cart')
test.todo('saves the cart')

test('clears the cart', { timeout: 1000 })
test('clears the cart', { timeout: 1000 })

describe('checkout', () => {
  test('applies a coupon')
  test('applies a coupon', () => {})
})

test('totals the cart', () => {})
test('totals the cart', () => {
  expect(1 + 1).toBe(2)
})
