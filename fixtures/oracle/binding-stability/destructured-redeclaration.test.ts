// A `var` pattern replaces the first declaration.
import { describe, expect, test } from 'vitest'
import { closed, open } from './destructured-redeclaration'

describe('destructured-redeclaration', () => {
  closed()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
