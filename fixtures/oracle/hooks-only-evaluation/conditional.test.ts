// Evaluating a callee can run code before the hook is called.
import { describe, expect, test } from 'vitest'
import { conditionalOpen, hooks } from './helpers'

describe('conditional', () => {
  hooks()
  conditionalOpen()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
