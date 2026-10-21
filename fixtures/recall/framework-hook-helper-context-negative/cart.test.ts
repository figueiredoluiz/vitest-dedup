// RECALL negative -- duplicate-test, control for the hook-helper cases.
//
// The bodies are identical, but only the first suite runs under `seedCart()`'s `beforeEach`, which
// fills the cart the assertion reads. They are different tests, so no finding is wanted. The
// helper is read today (inline hook, no module state), so the pair is compared and found distinct.
// It fails a fix that marks every hook-registering helper inert. Call `seedCart()` in both suites,
// or neither, and the pair becomes a duplicate-test.
import { describe, expect, test } from 'vitest'
import { seedCart } from './seed'

describe('seeded cart', () => {
  seedCart()

  test('counts the cart', () => {
    expect(JSON.parse(localStorage.getItem('cart') ?? '[]')).toHaveLength(1)
  })
})

describe('plain cart', () => {
  test('counts the cart, plain', () => {
    expect(JSON.parse(localStorage.getItem('cart') ?? '[]')).toHaveLength(1)
  })
})
