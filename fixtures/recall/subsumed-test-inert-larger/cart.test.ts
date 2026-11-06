// RECALL negative -- subsumed-test where the larger test never runs.
//
// `test.skip` and `test.todo` with a callback register a test Vitest does not run, so neither
// covers the smaller test beside it: removing the smaller one would leave its events unchecked.
//
// Load-bearing: drop `.skip` from `reads the count and total` and that pair is one
// `subsumed-test` finding.
import { expect, test } from 'vitest'

const cart = { count: 2, total: 30, currency: 'EUR', items: 3 }

test('reads the count', () => {
  expect(cart.count).toBe(2)
})

test.skip('reads the count and total', () => {
  expect(cart.count).toBe(2)
  expect(cart.total).toBe(30)
})

test('reads the currency', () => {
  expect(cart.currency).toBe('EUR')
})

test.todo('reads the currency and items', () => {
  expect(cart.currency).toBe('EUR')
  expect(cart.items).toBe(3)
})
