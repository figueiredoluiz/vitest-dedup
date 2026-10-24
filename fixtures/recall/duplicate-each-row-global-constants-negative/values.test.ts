import { expect, test } from 'vitest'

test.each([
  [undefined], [null], ['undefined'], [NaN], ['NaN'],
  [Infinity], [-Infinity], ['Infinity'],
])('distinguishes %s', (value) => {
  expect([undefined, null, 'undefined', NaN, 'NaN', Infinity, -Infinity, 'Infinity']).toContainEqual(value)
})
