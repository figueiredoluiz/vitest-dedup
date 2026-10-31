// A direct `eval` may write any binding of its module, so nothing there is hooks-only.
import { describe, expect, test } from 'vitest'
import { closed, open } from './direct-eval'

describe('direct-eval', () => {
  closed()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
