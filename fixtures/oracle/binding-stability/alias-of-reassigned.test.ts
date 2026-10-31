// A copy of a reassigned hook alias holds what was written.
import { describe, expect, test } from 'vitest'
import { closed, open } from './alias-of-reassigned'

describe('alias-of-reassigned', () => {
  closed()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
