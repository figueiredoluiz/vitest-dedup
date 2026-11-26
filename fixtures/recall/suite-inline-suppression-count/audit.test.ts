import { describe, expect, test } from 'vitest'

describe('audit', () => {
  // vitest-dedup-disable-next-line separate audit context
  test('reads one', () => expect(1).toBe(1))
  // vitest-dedup-disable-next-line separate audit context
  test('reads two', () => expect(2).toBe(2))
})

describe('audit', () => {
  // vitest-dedup-disable-next-line separate audit context
  test('reads one', () => expect(1).toBe(1))
  // vitest-dedup-disable-next-line separate audit context
  test('reads two', () => expect(2).toBe(2))
})
