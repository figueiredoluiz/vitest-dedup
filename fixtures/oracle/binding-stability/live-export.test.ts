// An export rewritten by a function its importer calls.
import { describe, expect, test } from 'vitest'
import { closed, open, swap } from './live-export'

describe('live-export', () => {
  closed()
  swap()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
