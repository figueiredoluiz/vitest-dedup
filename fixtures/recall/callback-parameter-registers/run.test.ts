// RECALL control -- duplicate-test absent, shape: a `forEach` callback calls the registration API
// it is handed.
//
// Vitest runs the callback with `test`, so `hidden` is registered and pairs with `visible`. The
// callback reads only its parameter, whose value the analyzer cannot see, so the file is refused
// rather than reported complete.
import { test, expect } from 'vitest'

;[test].forEach((declare) => {
  declare('hidden', () => {
    expect(1).toBe(1)
  })
})

test('visible', () => {
  expect(1).toBe(1)
})
