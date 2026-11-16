import { expect, test } from 'vitest'

// Vitest 3 and 4 quote and escape a `$name` string; Vitest 5 inserts it bare.
test('named abc', () => { expect(1).toBe(1) })
test("named 'abc'", () => { expect(2).toBe(2) })
test.each([{ v: 'abc' }])('named $v', ({ v }) => { expect(v).toBe('abc') })

// Vitest 3 and 4 render a `$name` zero as `+0`.
test('zero +0', () => { expect(3).toBe(3) })
test('zero 0', () => { expect(4).toBe(4) })
test.each([{ v: 0 }])('zero $v', ({ v }) => { expect(v).toBe(0) })

// Vitest 3 leaves every directive literal for a `test.for` row of `null`.
test('none %s', () => { expect(5).toBe(5) })
test('none undefined', () => { expect(6).toBe(6) })
test.for([null])('none %s', (row) => { expect(row).toBe(null) })

// Every major renders `true` alike.
test('flag true', () => { expect(7).toBe(7) })
test.each([{ v: true }])('flag $v', ({ v }) => { expect(v).toBe(true) })
