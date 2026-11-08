// RECALL control -- duplicate-test absent, shape: an `afterEach` in the readers' own suite assigns
// a value other than the declared one.
//
// The first test to run reads 0, every later one reads 1: `a` passes and `b` fails, and a retried
// test reads 1 on its second attempt. A plain hook write counted only when it built on the
// previous value, so the readers were reported as error-severity duplicates. The twin
// `hook-after-each-restore` restores the declared value instead.
import { describe, test, expect, afterEach } from 'vitest'

describe('readers', () => {
  let ready = 0
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
