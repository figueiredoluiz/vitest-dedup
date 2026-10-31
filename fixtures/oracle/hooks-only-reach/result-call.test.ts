// What a hooks-only helper returns is any value: a call of it may register tests.
import { describe, expect, test } from 'vitest'
import { factory } from './setup'

describe('result', () => {
  factory()
  factory()('through the result', () => {})
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
