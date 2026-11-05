import { describe, expect, test } from 'vitest'

let count = 0
describe.each([1, 1])('concise effects', n =>
  test('child', { retry: count++ }, () => expect(n).toBe(1)))

describe.each([1, 1])('nested effects', n => {
  describe('inner', () =>
    test('child', { retry: count++ }, () => expect(n).toBe(1)))
})

describe.each([1, 1])('block control', n => {
  test('child', { retry: 1 }, () => expect(n).toBe(1))
})
test('collection count', () => expect(count).toBe(4))
