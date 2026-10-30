// A hooks-only helper is context for the tests around it; its one-call-away twin is an excluded case.
import { describe, expect, test } from 'vitest'
import { reexported } from './barrel'
import { closed, open } from './setup'

describe('control', () => {
  closed()
  reexported()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
