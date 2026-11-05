import { describe, expect, test } from 'vitest'

describe.each([1, 2])('outer', outer => {
  test.each([outer, 1])('child', n => expect(n).toBeGreaterThan(0))
})
