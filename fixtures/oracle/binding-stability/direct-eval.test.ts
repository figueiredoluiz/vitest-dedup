// A direct `eval` writes a binding no analysis can see.
import { describe, expect, test } from 'vitest'
import { closed, open } from './direct-eval'

describe('direct-eval', () => {
  closed()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
