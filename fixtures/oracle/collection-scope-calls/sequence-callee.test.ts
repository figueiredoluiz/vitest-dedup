// A sequence expression as callee.
import { describe, expect, test } from 'vitest'
import { cases } from './support'

describe('sequence-callee', () => {
  (0, cases)()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
