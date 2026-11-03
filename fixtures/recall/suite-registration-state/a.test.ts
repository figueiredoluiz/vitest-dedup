import { describe, test, expect } from 'vitest'
let n = 0
describe('s', () => {
  n++
  test('t', () => { expect(n).toBe(1) })
})
describe('s', () => {
  n++
  test('t', () => { expect(n).toBe(1) })
})
