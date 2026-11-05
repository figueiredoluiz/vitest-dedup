// Selected operations stay significant; readonly primitive data still consolidate.
// @vitest-dedup:extracted 32
import { test, expect } from 'vitest'
const throws = [() => { throw new Error('first') }, () => { throw new Error('second') }]
const functions = [() => 'first', () => 'second']
const services = [{ run: functions[0] }, { run: functions[1] }]
const throwSelected = (index: number, expected: string) => expect(throws[index]).toThrow(expected)
const runSelected = (index: number) => services[index].run()
const invoke = (fn: () => string, unused?: unknown) => fn()
const spreadSelected = (index: number) => invoke(...[], functions[index])
const coercible = [{ [Symbol.toPrimitive]: () => 'first' }, { [Symbol.toPrimitive]: () => 'second' }]
const values = ['a', 'b']
const fields = { 0: 'a', 1: 'b' }
const compare = (value: string, expected: string) => expect(value).toBe(expected)

test('throw zero', () => expect(throws[0]).toThrow('first'))
test('throw one', () => expect(throws[1]).toThrow('second'))
test('helper throw zero', () => throwSelected(0, 'first'))
test('helper throw one', () => throwSelected(1, 'second'))
test('method zero', () => expect(services[0].run()).toBe('first'))
test('method one', () => expect(services[1].run()).toBe('second'))
test('helper method zero', () => expect(runSelected(0)).toBe('first'))
test('helper method one', () => expect(runSelected(1)).toBe('second'))
test('alias method zero', () => { const run = services[0].run; expect(run()).toBe('first') })
test('alias method one', () => { const run = services[1].run; expect(run()).toBe('second') })
test('spread zero', () => expect(invoke(...[], functions[0])).toBe('first'))
test('spread one', () => expect(invoke(...[], functions[1])).toBe('second'))
test('helper spread zero', () => expect(spreadSelected(0)).toBe('first'))
test('helper spread one', () => expect(spreadSelected(1)).toBe('second'))
test('coercion zero', () => expect(`${coercible[0]}`).toBe('first'))
test('coercion one', () => expect(`${coercible[1]}`).toBe('second'))
test('satisfy zero', () => expect(functions[0]).toSatisfy(fn => fn() === 'first'))
test('satisfy one', () => expect(functions[1]).toSatisfy(fn => fn() === 'second'))
test('primitive zero', () => expect(values[0]).toBe('a'))
test('primitive one', () => expect(values[1]).toBe('b'))
test('field zero', () => expect(fields[0]).toBe('a'))
test('field one', () => expect(fields[1]).toBe('b'))
test('later spread zero', () => compare(values[0], 'a', ...[]))
test('later spread one', () => compare(values[1], 'b', ...[]))

// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable Different selected functions execute inside the matcher.
// @vitest-dedup:participant L16 "throw zero"
// @vitest-dedup:participant L17 "throw one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable An unchanged helper executes different selected functions.
// @vitest-dedup:participant L18 "helper throw zero"
// @vitest-dedup:participant L19 "helper throw one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable Selected receivers expose different method implementations.
// @vitest-dedup:participant L20 "method zero"
// @vitest-dedup:participant L21 "method one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable A helper's receiver reaches the eventual method call.
// @vitest-dedup:participant L22 "helper method zero"
// @vitest-dedup:participant L23 "helper method one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable Aliasing a member does not turn the selected method into data.
// @vitest-dedup:participant L24 "alias method zero"
// @vitest-dedup:participant L25 "alias method one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable A preceding spread changes which parameter receives the function.
// @vitest-dedup:participant L26 "spread zero"
// @vitest-dedup:participant L27 "spread one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable The spread-position proof also applies inside an unchanged helper.
// @vitest-dedup:participant L28 "helper spread zero"
// @vitest-dedup:participant L29 "helper spread one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable Coercion may execute the selected object's conversion hook.
// @vitest-dedup:participant L30 "coercion zero"
// @vitest-dedup:participant L31 "coercion one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable A higher-order matcher invokes its selected function input.
// @vitest-dedup:participant L32 "satisfy zero"
// @vitest-dedup:participant L33 "satisfy one"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L34 "primitive zero"
// @vitest-dedup:participant L35 "primitive one"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L36 "field zero"
// @vitest-dedup:participant L37 "field one"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L38 "later spread zero"
// @vitest-dedup:participant L39 "later spread one"

const mutated = ['a']
mutated[0] = 'b'
test.each([mutated[0]])('mutated binding', value => expect(value).toBeTruthy())
test.each(['a'])('literal binding', value => expect(value).toBeTruthy())
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable Mutating an array must prevent folding its original cell into a removable duplicate.
// @vitest-dedup:participant L89 "mutated binding"
// @vitest-dedup:participant L90 "literal binding"

const getters = { get 0() { return 'first' }, get 1() { return 'second' } }
test('getter zero', () => expect(getters[0]).toBe('first'))
test('getter one', () => expect(getters[1]).toBe('second'))
const tagged = (_parts: TemplateStringsArray, fn: () => string) => fn()
const runTagged = (index: number) => tagged`${functions[index]}`
test('tag zero', () => expect(runTagged(0)).toBe('first'))
test('tag one', () => expect(runTagged(1)).toBe('second'))
const stringifiable = [{ toString() { return 'first' } }, { toString() { return 'second' } }]
const coerceSelected = (index: number) => `${stringifiable[index]}`
test('helper coercion zero', () => expect(coerceSelected(0)).toBe('first'))
test('helper coercion one', () => expect(coerceSelected(1)).toBe('second'))

// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable An own getter is executable code rather than a proved primitive cell.
// @vitest-dedup:participant L97 "getter zero"
// @vitest-dedup:participant L98 "getter one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable A helper's tag invokes the selected function.
// @vitest-dedup:participant L101 "tag zero"
// @vitest-dedup:participant L102 "tag one"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable A helper's untagged interpolation invokes selected coercion methods.
// @vitest-dedup:participant L105 "helper coercion zero"
// @vitest-dedup:participant L106 "helper coercion one"
