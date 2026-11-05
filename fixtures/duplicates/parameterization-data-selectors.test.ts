// Computed reads inside the same helper preserve that helper's source identity.
// Numeric data selectors may become row inputs, but selected callees stay significant.
// @vitest-dedup:extracted 10
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L25 "sum one"
// @vitest-dedup:participant L26 "sum two"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L28 "index zero"
// @vitest-dedup:participant L29 "index one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable Selecting a different callee is an operation change.
// @vitest-dedup:participant L31 "callee zero"
// @vitest-dedup:participant L32 "callee one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable A local alias still selects a different callee.
// @vitest-dedup:participant L34 "alias zero"
// @vitest-dedup:participant L35 "alias one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable An unchanged helper can still call an unknown matcher.
// @vitest-dedup:participant L38 "matcher one"
// @vitest-dedup:participant L39 "matcher two"
import { test, expect } from 'vitest'
const sum = (xs: number[]) => { let total = 0; for (let i = 0; i < xs.length; i++) total += xs[i]; return total }
const values = ['a', 'b']
test('sum one', () => expect(sum([1])).toBe(1))
test('sum two', () => expect(sum([2])).toBe(2))

test('index zero', () => expect(values[0]).toBe('a'))
test('index one', () => expect(values[1]).toBe('b'))
const functions = [() => 0, () => 1]
test('callee zero', () => expect(functions[0]()).toBe(0))
test('callee one', () => expect(functions[1]()).toBe(1))

test('alias zero', () => { const call = functions[0]; expect(call()).toBe(0) })
test('alias one', () => { const call = functions[1]; expect(call()).toBe(1) })
const matcher = ['to', 'Be'].join('')
const check = (value: number) => expect(value)[matcher](value)
test('matcher one', () => check(1))
test('matcher two', () => check(2))
