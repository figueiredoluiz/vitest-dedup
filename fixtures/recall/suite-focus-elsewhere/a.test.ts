import { describe, test, expect } from 'vitest'
test.only('the only one', () => { expect(1).toBe(1) })
describe('s', () => {
  test('t', () => { expect(1).toBe(1) })
})
describe('s', () => {
  test('t', () => { expect(1).toBe(1) })
})
