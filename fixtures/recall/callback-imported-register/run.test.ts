// RECALL control -- duplicate-test absent, shape: an imported function handed to `forEach` registers.
//
// Vitest calls `register` once per element, so `hidden` is registered twice and pairs with
// `visible`. The callback is not read, so the file is refused rather than reported complete.
import { test, expect } from 'vitest'
import { register } from './register'

;[1, 2].forEach(register)

test('visible', () => {
  expect(1).toBe(1)
})
