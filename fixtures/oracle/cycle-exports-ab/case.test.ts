// A wrapper reached only through an import cycle, in import order a then b.
import { describe, expect, test } from 'vitest'
import { outer } from './a'
import { inner } from './b'

describe('cycle', () => {
  outer()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
