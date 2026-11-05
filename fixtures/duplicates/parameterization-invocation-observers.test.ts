// A function callback can read inputs beyond its declared parameter list.
// @vitest-dedup:extracted 16
// @vitest-dedup:not-runnable Three observer cases intentionally fail because their extra input differs from the assertion.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable Invocation observers must retain every input beyond named parameters.
// @vitest-dedup:participant L13 "argument two"
// @vitest-dedup:participant L14 "argument three"
// @vitest-dedup:expect rule=parameterization-candidate verdict=consolidatable severity=warning
// @vitest-dedup:participant L16 "retained one"
// @vitest-dedup:participant L17 "retained two"
import { test, expect } from 'vitest'

test.each([[{ a: 1 }, 2]])('argument two', function(value) { expect(value.a + value.a).toBe(2); expect(arguments[1]).toBe(2) })
test.each([[{ a: 1 }, 3]])('argument three', function(value) { expect(value.a + value.a).toBe(2); expect(arguments[1]).toBe(2) })

test('retained one', () => expect(1).toBe(1))
test('retained two', () => expect(2).toBe(2))

test.each([{ label: 'first', value: { a: 1 } }])('function first', function({ value }) { expect(value.a + value.a).toBe(2) })
test.each([{ label: 'second', value: { a: 1 } }])('function second', function({ value }) { expect(value.a + value.a).toBe(2) })
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L19 "function first"
// @vitest-dedup:participant L20 "function second"

test.each([[{ a: 1 }, 'two']])('class first', function (value) {
  class C { [arguments[1]]() {} }
  expect(value.a + value.a).toBe(2)
  expect(Object.getOwnPropertyNames(C.prototype)).toContain('two')
})
test.each([[{ a: 1 }, 'three']])('class second', function (value) {
  class C { [arguments[1]]() {} }
  expect(value.a + value.a).toBe(2)
  expect(Object.getOwnPropertyNames(C.prototype)).toContain('two')
})
const scalarValues = ['a']
test.each([[scalarValues[0], 'two']])('scalar class first', function (value) {
  class C { [arguments[1]]() {} }
  expect(value).toBe('a')
  expect(Object.getOwnPropertyNames(C.prototype)).toContain('two')
})
test.each([['a', 'three']])('scalar class second', function (value) {
  class C { [arguments[1]]() {} }
  expect(value).toBe('a')
  expect(Object.getOwnPropertyNames(C.prototype)).toContain('two')
})
test.each([{ label: 'first', value: { a: 1 } }])('class field first', function ({ value }) {
  class C { field = this }
  expect(value.a + value.a).toBe(2)
})
test.each([{ label: 'second', value: { a: 1 } }])('class field second', function ({ value }) {
  class C { field = this }
  expect(value.a + value.a).toBe(2)
})
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable A computed class key reads extra callback inputs in the enclosing frame.
// @vitest-dedup:participant L25 "class first"
// @vitest-dedup:participant L30 "class second"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable Newly proved primitive cells must preserve the same invocation observations.
// @vitest-dedup:participant L36 "scalar class first"
// @vitest-dedup:participant L41 "scalar class second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L46 "class field first"
// @vitest-dedup:participant L50 "class field second"

test.each([{ label: 'a', value: 1 }])('property first', ({ value }) => expect({ arguments: value }).toEqual({ arguments: 1 }))
test.each([{ label: 'b', value: 1 }])('property second', ({ value }) => expect({ arguments: value }).toEqual({ arguments: 1 }))
test.each([{ label: 'a', value: { arguments: 1 } }])('member first', ({ value }) => expect(value.arguments).toBe(1))
test.each([{ label: 'b', value: { arguments: 1 } }])('member second', ({ value }) => expect(value.arguments).toBe(1))
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L66 "property first"
// @vitest-dedup:participant L67 "property second"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L68 "member first"
// @vitest-dedup:participant L69 "member second"
