import { describe, expect, test } from 'vitest'

describe.each([{ n: 1 }, { n: 2 }])('object $n', ({ n }) => {
  test('child', () => expect(n).toBeGreaterThan(0))
})
