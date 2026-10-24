import { expect, test } from 'vitest'
test.each([[1 / -Infinity], [-0]])('signed %s', value => {
  expect(Object.is(value, -0)).toBe(true)
})
