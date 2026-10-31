// The head of a cycle calls what its member could not see: `setup` is not hooks-only.
import { describe, expect, test } from 'vitest'
import { setup } from './a'

describe('head', () => {
  setup()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
