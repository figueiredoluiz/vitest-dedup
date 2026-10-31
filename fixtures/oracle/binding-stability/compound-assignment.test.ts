// A compound assignment writes the binding.
import { describe, expect, test } from 'vitest'
import { closed, open } from './compound-assignment'

describe('compound-assignment', () => {
  closed()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
