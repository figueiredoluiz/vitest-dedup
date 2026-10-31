// A reassigned export stays unknown through a barrel.
import { describe, expect, test } from 'vitest'
import { closed, open } from './barrel'

describe('reassigned-export-barrel', () => {
  closed(() => {})
  open(() => {})
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
