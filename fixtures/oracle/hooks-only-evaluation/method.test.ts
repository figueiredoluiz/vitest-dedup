// A method call is not resolved, so it may register tests.
import { describe, expect, test } from 'vitest'
import { hooks, methodOpen } from './helpers'

describe('method', () => {
  hooks()
  methodOpen()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
