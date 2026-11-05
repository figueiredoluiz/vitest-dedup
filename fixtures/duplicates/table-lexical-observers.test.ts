// Arrow callbacks inherit their receiver and arguments instead of reading table inputs.
// @vitest-dedup:extracted 4
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L11 "lexical this first"
// @vitest-dedup:participant L12 "lexical this second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L13 "lexical arguments first"
// @vitest-dedup:participant L14 "lexical arguments second"
import { test, expect } from 'vitest'

test.each([1])('lexical this first', () => expect(this).toBe(this))
test.each([2])('lexical this second', () => expect(this).toBe(this))
test.for([3])('lexical arguments first', () => expect(typeof arguments).toBe(typeof arguments))
test.for([4])('lexical arguments second', () => expect(typeof arguments).toBe(typeof arguments))
