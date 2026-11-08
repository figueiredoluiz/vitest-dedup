// Control: the `afterEach` restores the declared value. One edit from `after-each-write`.
import { afterEach, expect, test } from 'vitest'

let ready = 0

afterEach(() => {
  ready = 0
})

test('first', () => {
  expect(ready).toBe(0)
})

test('second', () => {
  expect(ready).toBe(0)
})
