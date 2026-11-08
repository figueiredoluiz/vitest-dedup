// RECALL control -- duplicate-test present, shape: a cleanup the `beforeEach` returns restores the
// declared value.
//
// Vitest runs the cleanup after each test, so every test starts from 0 and the readers remain
// duplicates. Writing 1 instead makes them order-dependent.
import { describe, test, expect, beforeEach } from 'vitest'

describe('readers', () => {
  let ready = 0
  beforeEach(() => () => {
    ready = 0
  })
  test('a', () => {
    expect(ready).toBe(0)
  })
  test('b', () => {
    expect(ready).toBe(0)
  })
})
