// RECALL control -- duplicate-test absent, shape: the twin of `hook-after-each-restore`, one edit
// away: a `beforeAll` sets another value before the first test.
//
// The first reader sees 1; the `afterEach` restores 0, so the second sees 0. Which value a reader
// sees depends on which test ran before it: `a` passes and `b` fails. Counting the restore alone
// reported them as error-severity duplicates.
import { describe, test, expect, beforeAll, afterEach } from 'vitest'

describe('readers', () => {
  let ready = 0
  beforeAll(() => {
    ready = 1
  })
  afterEach(() => {
    ready = 0
  })
  test('a', () => {
    expect(ready).toBe(1)
  })
  test('b', () => {
    expect(ready).toBe(1)
  })
})
