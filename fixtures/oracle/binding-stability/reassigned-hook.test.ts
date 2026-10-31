// A hook alias reassigned after its declaration may run anything.
import { describe, expect, test } from 'vitest'
import { closed, open } from './reassigned-hook'

describe('reassigned-hook', () => {
  closed()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
