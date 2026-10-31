// A module of an import cycle reads the other before it is known: `a` is not hooks-only, in either import order.
import { describe, expect, test } from 'vitest'
import { a } from './a'
import { b } from './b'

describe('cycle', () => {
  a(true)
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
