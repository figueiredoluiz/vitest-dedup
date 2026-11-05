// Whole pure data cells retain their value and allocation, including repeated primitive reads.
// @vitest-dedup:extracted 10
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L17 "object a"
// @vitest-dedup:participant L18 "object b"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L20 "repeated first"
// @vitest-dedup:participant L21 "repeated second"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L23 "concat expression"
// @vitest-dedup:participant L24 "concat literal"

import { test, expect } from 'vitest'

// Object shape is table input, while callback operations remain the same.

test.each([[{ a: 1 }]])('object a', value => expect(Object.keys(value)).toEqual(['a']))
test.each([[{ b: 1 }]])('object b', value => expect(Object.keys(value)).toEqual(['b']))

test.each([{ description: 'first', expected: { a: 1 } }])('repeated first', ({ expected }) => expect(expected.a + expected.a).toBe(2))
test.each([{ description: 'second', expected: { a: 1 } }])('repeated second', ({ expected }) => expect(expected.a + expected.a).toBe(2))

test.each([['x' + 201562347225087.63]])('concat expression', value => expect(String(value)).toBeTruthy())
test.each([['x201562347225087.63']])('concat literal', value => expect(String(value)).toBeTruthy())

const read = (value: { a: number }) => value.a
test.each([[{ a: 1 }]])('manual retention table', value => expect(read(value)).toBe(1))
test('manual retention direct', () => expect(read({ a: 2 })).toBe(2))
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L27 "manual retention table"
// @vitest-dedup:participant L28 "manual retention direct"

const tokens = (value: string) => [{ text: value, type: 'token' }]
test.each(['a'])('bound expected', source => expect(tokens(source)).toEqual([{ text: source, type: 'token' }]))
test('literal expected', () => expect(tokens('b')).toEqual([{ text: 'b', type: 'token' }]))
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L34 "bound expected"
// @vitest-dedup:participant L35 "literal expected"
