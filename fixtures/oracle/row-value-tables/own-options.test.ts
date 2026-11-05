import { describe, expect, test } from 'vitest'

let counter = 0
describe.each([1, 1])('own accessor', { get retry() { return counter++ } }, n => {
  test('child', () => expect(n).toBe(1))
})
describe.each([1, 1])('own literal control', { retry: 1 }, n => {
  test('child', () => expect(n).toBe(1))
})
test('option reads', () => expect(counter).toBe(4))
