// A cycle back through the test file is followed: `cases` is a wrapper. The task sits on the call's
// line either way, so only the pinned site tells a cut cycle from a followed one.
import { describe, expect, test } from 'vitest'
import { cases } from './cases'

export const register = test

describe('back', () => {
  cases()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
