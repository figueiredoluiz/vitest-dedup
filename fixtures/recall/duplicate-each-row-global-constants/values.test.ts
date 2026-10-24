import { expect, test } from 'vitest'

const UNDEFINED = undefined
const NAN = NaN
const INFINITY = Infinity

test.each([
  [undefined, undefined], [UNDEFINED, undefined],
  [NaN, NaN], [NAN, NaN],
  [Infinity, Infinity], [INFINITY, Infinity],
  [-Infinity, -Infinity], [-INFINITY, -Infinity],
])('reads %s', (value, expected) => {
  expect(Object.is(value, expected)).toBe(true)
})
