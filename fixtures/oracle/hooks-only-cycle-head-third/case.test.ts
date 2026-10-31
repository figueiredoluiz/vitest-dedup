// A module reading a cycle's head is not hooks-only either.
import { describe, expect, test } from 'vitest'
import { around } from './c'

describe('third', () => {
  around()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
