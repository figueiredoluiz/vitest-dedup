// RECALL positive -- duplicate-test and parameterization-candidate, shape: string values holding
// lone surrogates.
//
// JavaScript strings may hold a lone surrogate, which has no UTF-8 spelling. Two strings that
// differ only in which lone surrogate they hold are different values, so the first pair is two
// cases of one table, never a duplicate. The second pair holds the same lone surrogate in identical
// bodies and is a duplicate. A reading that drops or replaces lone surrogates would merge the first
// pair into a duplicate. The table's rows differ only in their lone surrogate, so they are two
// cases, not a repeated row.
import { expect, test } from 'vitest'

test('keeps the first high surrogate', () => {
  expect('\uD800'.charCodeAt(0)).toBe(0xd800)
})

test('keeps the second high surrogate', () => {
  expect('\uD801'.charCodeAt(0)).toBe(0xd801)
})

test('measures a lone low surrogate', () => {
  expect('a\uDC00b'.length).toBe(3)
})

test('measures the same lone low surrogate', () => {
  expect('a\uDC00b'.length).toBe(3)
})

test.each(['\uD800', '\uD801'])('row %# keeps its surrogate', (value) => {
  expect(value.charCodeAt(0)).toBeGreaterThan(0xd7ff)
})
