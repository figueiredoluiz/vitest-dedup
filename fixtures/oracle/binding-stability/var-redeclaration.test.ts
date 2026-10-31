// A second `var` replaces the first without a write.
import { describe, expect, test } from 'vitest'
import { closed, open } from './var-redeclaration'

describe('var-redeclaration', () => {
  closed()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
