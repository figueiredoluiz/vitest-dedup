import { expect, test } from 'vitest'

// The second assertion deliberately fails: these bodies do not have the same behavior.
test('reads a surrogate code unit', () => {
  expect('\uD800'.charCodeAt(0)).toBe(55296)
})
test('reads replacement character text', () => {
  expect('\uFFFDd800'.charCodeAt(0)).toBe(55296)
})

test('retains the first surrogate body', () => {
  expect('a\uDC00b'.length).toBe(3)
})
test('retains the second surrogate body', () => {
  expect('a\uDC00b'.length).toBe(3)
})

test.each(['\uD800', '\uFFFDd800'])('distinct value row %#', value => {
  expect(value.charCodeAt(0)).toBeGreaterThan(0)
})
test.each([{ '\uD800': 1 }, { '\uFFFDd800': 1 }])('distinct key row %#', value => {
  expect(Object.keys(value)[0].charCodeAt(0)).toBeGreaterThan(0)
})
