import { describe, expect, test } from 'vitest'

const makeMarker = () => 1
describe.each([1, 1])('options', n => {
  const marker = 1
  test('child', { retry: 1 }, () => expect(n + marker).toBe(2))
})
describe.each([1, 1])('uncertain', n => {
  const marker = makeMarker()
  test('child', () => expect(n + marker).toBe(2))
})
describe.each([1, 1])('tagged', n => {
  test.each`value
  ${1}`('child $value', ({ value }) => expect(value + n).toBe(2))
})
describe.each([1, 1])('accessor', n => {
  test('child', { get retry() { return 1 } }, () => expect(n).toBe(1))
})
