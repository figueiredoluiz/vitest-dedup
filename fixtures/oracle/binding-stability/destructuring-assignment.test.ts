// A destructuring assignment writes the binding.
import { describe, expect, test } from 'vitest'
import { closed, open } from './destructuring-assignment'

describe('destructuring-assignment', () => {
  closed()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
