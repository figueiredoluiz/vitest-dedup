// Vitest compares a hook's timeout, which may run `valueOf`.
import { describe, expect, test } from 'vitest'
import { timeoutClosed, timeoutOpen } from './helpers'; import { coerced } from './support'

describe('timeout', () => {
  timeoutClosed()
  timeoutOpen(coerced)
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
