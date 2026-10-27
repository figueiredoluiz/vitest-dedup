// RECALL control -- duplicate-test absent, shape: a `forEach` callback registers tests.
//
// Vitest runs the callback once per element, so `hidden` is registered twice and pairs with
// `visible`. The callback is not read, so the file is refused rather than reported complete.
import { test, expect } from 'vitest'

;[1, 2].forEach(() => {
  test('hidden', () => {
    expect(1).toBe(1)
  })
})

test('visible', () => {
  expect(1).toBe(1)
})
