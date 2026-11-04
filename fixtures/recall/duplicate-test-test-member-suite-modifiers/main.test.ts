// A test function's `describe` is the top-level suite: `fails` chained before it does not invert
// the suite, so its child duplicates the top-level test.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=duplicate severity=error
// @vitest-dedup:participant L10 "checkout > totals the cart"
// @vitest-dedup:participant L13 "totals the cart"
import { expect, test } from 'vitest'

test.fails.describe('checkout', () => {
  test('totals the cart', () => { expect([1, 2].reduce((a, b) => a + b, 0)).toBe(3) })
})

test('totals the cart', () => { expect([1, 2].reduce((a, b) => a + b, 0)).toBe(3) })
