// An exported hook keeps the hook rules: its timeout may run `valueOf`.
import { describe, expect, test } from 'vitest'
import { closed, open } from './exported-hook-alias'; import { coerced } from './support'

describe('exported-hook-alias', () => {
  closed()
  open(coerced as unknown as number)
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
