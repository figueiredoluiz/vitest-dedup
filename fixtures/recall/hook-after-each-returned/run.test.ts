// RECALL control -- duplicate-test present, shape: an `afterEach` returns a function that writes
// the binding.
//
// Vitest runs what a `beforeEach` or `beforeAll` returns, but ignores what an `afterEach` returns,
// so the write never happens and the readers remain duplicates.
import { describe, test, expect, afterEach } from 'vitest'

describe('readers', () => {
  let ready = 0
  afterEach(() => () => {
    ready = 1
  })
  test('a', () => {
    expect(ready).toBe(0)
  })
  test('b', () => {
    expect(ready).toBe(0)
  })
})
