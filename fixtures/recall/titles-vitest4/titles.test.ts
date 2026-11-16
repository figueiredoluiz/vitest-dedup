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

// Vitest 3 interpolates `$` again in what `%s` inserted: `'$1'` collects as `''a''` there.
test("again '$1'", () => { expect(8).toBe(8) })
test.each([["'$1'", 'a']])('again %s', (value) => { expect(value).toBe("'$1'") })

// Vitest 3 and 4 cut a `$name` value past 40 units: this bigint to its first 38 and `…n`.
test('big 10000000000000000000000000000000000000000000000000n', () => { expect(9).toBe(9) })
test.each([{ v: 10000000000000000000000000000000000000000000000000n }])('big $v', ({ v }) => { expect(v > 0n).toBe(true) })
test('edge 100000000000000000000000000000000000000n', () => { expect(10).toBe(10) })
test.each([{ v: 100000000000000000000000000000000000000n }])('edge $v', ({ v }) => { expect(v > 0n).toBe(true) })

// A title without `%` or `$` reads the same for a nullish `test.for` row on every major.
test.for([null])('plain', () => { expect(11).toBe(11) })
test('plain', () => { expect(12).toBe(12) })

// Vitest 3 leaves a directive past the last argument literal; 4 and 5 insert `undefined`.
test('over abc undefined', () => { expect(13).toBe(13) })
test.each([['abc']])('over %s %s', (v) => { expect(v).toHaveLength(3) })

// Vitest 3 appends a row item a literal `%` counts; 4 and 5 do not.
test("pct 100% 'abc'", () => { expect(14).toBe(14) })
test.each([{ v: 'abc' }])('pct 100% $v', ({ v }) => { expect(v.length).toBe(3) })

// Vitest 3 reads `$` after `%` substitution: `$0%s` of `x` collects as `$0x` there.
test("'x'x", () => { expect(15).toBe(15) })
test.each([['x']])('$0%s', (v) => { expect(v).toHaveLength(1) })

// Vitest 3.0 leaves a positional `$0` literal and later 3.x releases substitute it.
test("case 'x'", () => { expect(16).toBe(16) })
test.each([['x']])('case $0', (v) => { expect(v).toBe('x') })

// Vitest 3.0 leaves `%$` literal and appends the row; later releases print the 1-based index.
test("idx 1 'abc'", () => { expect(17).toBe(17) })
test.each([{ v: 'abc' }])('idx %$ $v', ({ v }) => { expect(v.startsWith('a')).toBe(true) })
