// A helper reassigned after its declaration may run anything.
import { describe, expect, test } from 'vitest'
import { closed, open } from './reassigned-helper'

describe('reassigned-helper', () => {
  closed()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
