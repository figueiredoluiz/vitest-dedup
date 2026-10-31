// An export reassigned after its declaration may hold anything.
import { describe, expect, test } from 'vitest'
import { closed, open } from './reassigned-export'

describe('reassigned-export', () => {
  closed(() => {})
  open(() => {})
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
