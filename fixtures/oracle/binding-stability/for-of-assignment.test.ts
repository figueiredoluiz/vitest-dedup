// A `for of` head writes the binding.
import { describe, expect, test } from 'vitest'
import { closed, open } from './for-of-assignment'

describe('for-of-assignment', () => {
  closed()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
