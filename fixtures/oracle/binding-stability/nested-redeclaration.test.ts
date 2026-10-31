// A `var` in a nested block replaces the outer declaration.
import { describe, expect, test } from 'vitest'
import { closed, open } from './nested-redeclaration'

describe('nested-redeclaration', () => {
  closed()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
