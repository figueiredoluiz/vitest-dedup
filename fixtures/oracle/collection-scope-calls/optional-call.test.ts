// An optional call of an imported function.
import { describe, expect, test } from 'vitest'
import { cases } from './support'

describe('optional-call', () => {
  cases?.()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
