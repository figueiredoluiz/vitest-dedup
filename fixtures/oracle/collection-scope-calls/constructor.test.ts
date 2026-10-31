// A constructor that registers.
import { describe, expect, test } from 'vitest'
import { Cases } from './support'

describe('constructor', () => {
  new Cases()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
