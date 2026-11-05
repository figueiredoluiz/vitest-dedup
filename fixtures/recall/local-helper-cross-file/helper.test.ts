// RECALL positive -- cross-file-duplicate expected, shape: a file whose only test a same-file
// helper registers. The call is a registration, not module setup, so this file keys like
// direct.test.ts, which writes the same test itself; both pass.
import { test, expect } from 'vitest'

function declare() {
  test('adds', () => {
    expect(1 + 1).toBe(2)
  })
}

declare()
