// Implicit invocation reads retain row inputs even when no cell is bound.
// @vitest-dedup:extracted 22
// @vitest-dedup:not-runnable Eight contrast cases deliberately fail because their invocation input differs.
import { test, expect } from 'vitest'

test.each([[1]])('spread first', function () { expect(arguments[0]).toBe(1) })
test.each([[2]])('spread second', function () { expect(arguments[0]).toBe(1) })

test.each([{ value: 10 }])('whole first', function () { expect(arguments[0].value).toBe(10) })
test.each([{ value: 11 }])('whole second', function () { expect(arguments[0].value).toBe(10) })

test.for([[20]])('for first', function () { expect(arguments[0][0]).toBe(20) })
test.for([[21]])('for second', function () { expect(arguments[0][0]).toBe(20) })

test.each([{ value: 40 }])('empty pattern first', function ({}) { expect(arguments[0].value).toBe(40) })
test.each([{ value: 41 }])('empty pattern second', function ({}) { expect(arguments[0].value).toBe(40) })

test.each([{ value: 50 }])('missing property first', function ({ missing }) { expect(arguments[0].value).toBe(50) })
test.each([{ value: 51 }])('missing property second', function ({ missing }) { expect(arguments[0].value).toBe(50) })

test.each([[90]])('length first', function () { expect(arguments.length).toBe(1) })
test.each([[90, 91]])('length second', function () { expect(arguments.length).toBe(1) })

test.each([['a']])('class key first', function () { class C { [arguments[0]]() {} } expect(Object.getOwnPropertyNames(C.prototype)).toContain('a') })
test.each([['b']])('class key second', function () { class C { [arguments[0]]() {} } expect(Object.getOwnPropertyNames(C.prototype)).toContain('a') })

test.each([[100]])('wrapped first', (function () { expect(arguments[0]).toBe(100) }) as any)
test.each([[101]])('wrapped second', (function () { expect(arguments[0]).toBe(100) }) as any)

test.each([[60]])('same input first', function () { expect(arguments[0]).toBe(60) })
test.each([[60]])('same input second', function () { expect(arguments[0]).toBe(60) })

test.each([[70]])('unused input first', function () { expect(true).toBeTruthy() })
test.each([[71]])('unused input second', function () { expect(true).toBeTruthy() })

test.each([[81]])('nested frame first', function () { /** Reads this helper's own arguments. */ function local() { return arguments[0] } expect(local(80)).toEqual(80) })
test.each([[82]])('nested frame second', function () { /** Reads this helper's own arguments. */ function local() { return arguments[0] } expect(local(80)).toEqual(80) })

// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates modifier
// @vitest-dedup:participant L6 "spread first"
// @vitest-dedup:participant L7 "spread second"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates modifier
// @vitest-dedup:participant L9 "whole first"
// @vitest-dedup:participant L10 "whole second"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates modifier
// @vitest-dedup:participant L12 "for first"
// @vitest-dedup:participant L13 "for second"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates modifier
// @vitest-dedup:participant L15 "empty pattern first"
// @vitest-dedup:participant L16 "empty pattern second"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates modifier
// @vitest-dedup:participant L18 "missing property first"
// @vitest-dedup:participant L19 "missing property second"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates modifier
// @vitest-dedup:participant L21 "length first"
// @vitest-dedup:participant L22 "length second"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable Computed class-key evaluation has no behavior fingerprint. The counted final-outcome regression retains three required positives and rejects this false flat duplicate; restoring the tagged-only guard reproduces it.
// @vitest-dedup:participant L24 "class key first"
// @vitest-dedup:participant L25 "class key second"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates modifier
// @vitest-dedup:participant L27 "wrapped first"
// @vitest-dedup:participant L28 "wrapped second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L30 "same input first"
// @vitest-dedup:participant L31 "same input second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L33 "unused input first"
// @vitest-dedup:participant L34 "unused input second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L36 "nested frame first"
// @vitest-dedup:participant L37 "nested frame second"
