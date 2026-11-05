import { describe, expect, test } from 'vitest'

const scoped = describe
scoped('derived', () => {
  test.each([[1], [1]])('child', n => expect(n).toBe(1))
})
describe('direct', () => {
  test.each([[1], [1]])('child', n => expect(n).toBe(1))
})
