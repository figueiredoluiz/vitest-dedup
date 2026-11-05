import { describe, expect, test } from 'vitest'

describe.each([1, 2])('outer %s', () => {
  describe.each([3, 3])('inner %s', n => {
    test('child', () => expect(n).toBe(3))
  })
})
