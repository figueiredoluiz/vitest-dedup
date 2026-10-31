// A function that registers, passed as a value.
import { describe, expect, test } from 'vitest'
import { cases } from './support'

describe('wrapper-as-value', () => {
  [0].forEach(cases)
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
