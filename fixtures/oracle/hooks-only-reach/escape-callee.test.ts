// A helper calling a function that reads `test` as a value is not hooks-only.
import { describe, expect, test } from 'vitest'
import { escaping } from './setup'

describe('escape', () => {
  escaping()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
