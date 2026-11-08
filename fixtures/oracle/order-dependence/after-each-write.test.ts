// `second` reads the 1 the `afterEach` left after `first`: a value other than the declared one.
import { afterEach, expect, test } from 'vitest'

let ready = 0

afterEach(() => {
  ready = 1
})

test('first', () => {
  expect(ready).toBe(0)
})

test('second', () => {
  expect(ready).toBe(1)
})
