// RECALL control -- duplicate-test present, shape: the twin of `hook-after-each-write`, one edit
// away: the `afterEach` restores the declared value.
//
// Every test starts from 0 whichever ran before it, so the readers remain duplicates.
import { describe, test, expect, afterEach } from 'vitest'

describe('readers', () => {
  let ready = 0
  afterEach(() => {
    ready = 0
  })
  test('a', () => {
    expect(ready).toBe(0)
  })
  test('b', () => {
    expect(ready).toBe(0)
  })
})
