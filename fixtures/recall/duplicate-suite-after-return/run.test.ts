// RECALL negative -- duplicate-suite absent, shape: a suite callback returns before its copy.
//
// The callback leaves after the first `inner`, so Vitest never registers the second.
import { describe, test, expect } from 'vitest'

describe('outer', () => {
  describe('inner', () => {
    test('child', () => {
      expect(1).toBe(1)
    })
  })
  return
  describe('inner', () => {
    test('child', () => {
      expect(1).toBe(1)
    })
  })
})
