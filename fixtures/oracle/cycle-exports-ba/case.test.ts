// A wrapper reached only through an import cycle, in import order b then a.
import { describe, expect, test } from 'vitest'
import { inner } from './b'
import { outer } from './a'

describe('cycle', () => {
  outer()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
