// RECALL negative -- duplicate-suite absent, shape: a condition in brackets skips both copies.
//
// `describe['skipIf'](true)` is the condition `describe.skipIf(true)` is: neither copy is active.
import { describe, test, expect } from 'vitest'

describe['skipIf'](true)('skipped', () => {
  test('child', () => {
    expect(1).toBe(1)
  })
})

describe['skipIf'](true)('skipped', () => {
  test('child', () => {
    expect(1).toBe(1)
  })
})

test('visible', () => {
  expect(2).toBe(2)
})
