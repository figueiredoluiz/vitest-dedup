// RECALL control -- duplicate-test present, shape: the twin of `hook-after-each-write` with a
// `beforeEach` that rewrites the binding.
//
// The `afterEach` leaves 1, but the `beforeEach` sets 0 before every test, so the readers remain
// duplicates. Removing the `beforeEach` makes them order-dependent.
import { describe, test, expect, beforeEach, afterEach } from 'vitest'

describe('readers', () => {
  let ready = 0
  beforeEach(() => {
    ;[ready] = [0]
  })
  afterEach(() => {
    ready = 1
  })
  test('a', () => {
    expect(ready).toBe(0)
  })
  test('b', () => {
    expect(ready).toBe(0)
  })
})
