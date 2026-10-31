import { expect, test } from 'vitest'

const label = 'constant'
test(label, () => expect(11).toBe(11))
test('constant', () => expect(12).toBe(12))
test(`array ${[1, null, [2, 3]]}`, () => expect(13).toBe(13))
test('array 1,,2,3', () => expect(14).toBe(14))
test.each([{ label: 'value', count: 2 }])('$label $count', () => expect(15).toBe(15))
test('value 2', () => expect(16).toBe(16))
test.for([['A', 1], ['B', 2]])('for %s %i', () => expect(17).toBe(17))
test('for A 1', () => expect(18).toBe(18))
test('for B 2', () => expect(19).toBe(19))
test.each([7, 8])('index %#/%$', () => expect(20).toBe(20))
test('index 0/1', () => expect(21).toBe(21))
test('index 1/2', () => expect(22).toBe(22))
test(/** Supplies the name without executing. */ function namedTitle() {}, () => expect(23).toBe(23))
test('namedTitle', () => expect(24).toBe(24))
// Formatting signed zero is not ordinary JavaScript String conversion.
test.each([[-0]])('zero %s', () => expect(25).toBe(25))
test('zero 0', () => expect(26).toBe(26))
// Numeric prefix parsing must not treat .5 as an integer zero.
test.each([['.5']])('integer %i', () => expect(27).toBe(27))
test('integer 0', () => expect(28).toBe(28))
// Runtime calls and mutable values cannot provide a proven static name.
test(`called ${String(1)}`, () => expect(29).toBe(29))
test('called 1', () => expect(30).toBe(30))
let changing = 'before'
changing = 'after'
test(changing, () => expect(31).toBe(31))
test('before', () => expect(32).toBe(32))
// Unsupported directives and field syntax cannot be copied into a supposed runtime name.
test.each([[1]])('inspect %O', () => expect(33).toBe(33))
test('inspect %O', () => expect(34).toBe(34))
test.each([{ a: 'x', b: 'y', 'a$b': 'z' }])('fields $a$b', () => expect(35).toBe(35))
test('fields xy', () => expect(36).toBe(36))
test.each([{ 'a.b': 'dot' }])('dotted $a.b', () => expect(37).toBe(37))
test('dotted dot', () => expect(38).toBe(38))
test.each([{ value: 'abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrs' }])('long $value', () => expect(39).toBe(39))
test('long abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrs', () => expect(40).toBe(40))
