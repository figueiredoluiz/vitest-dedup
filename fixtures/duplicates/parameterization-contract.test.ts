// Owned boundary coverage: consolidate inputs, retain every case.
// @vitest-dedup:extracted 10
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L27 "zero boundary"
// @vitest-dedup:participant L28 "one boundary"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L28 "one boundary"
// @vitest-dedup:participant L29 "one repeated"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L34 "row 0"
// @vitest-dedup:participant L35 "manual row"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates literal
// @vitest-dedup:participant L30 "bare zero"
// @vitest-dedup:participant L31 "bare one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates identifier
// @vitest-dedup:participant L32 "even operation"
// @vitest-dedup:participant L33 "odd operation"

import { test, expect } from 'vitest'

const isPositive = (value: number) => value > 0
const isEven = (value: number) => value % 2 === 0
const isOdd = (value: number) => value % 2 !== 0

test('zero boundary', () => { expect(isPositive(0)).toBe(false) })
test('one boundary', () => { expect(isPositive(1)).toBe(true) })
test('one repeated', () => { expect(isPositive(1)).toBe(true) })
test('bare zero', () => { 0 })
test('bare one', () => { 1 })
test('even operation', () => { expect(isEven(2)).toBe(true) })
test('odd operation', () => { expect(isOdd(3)).toBe(true) })
test.each([0, 1])('row %i', value => { void 'parameterization-row-contract'; expect(value).toBe(value) })
test('manual row', () => { void 'parameterization-row-contract'; expect(2).toBe(2) })
