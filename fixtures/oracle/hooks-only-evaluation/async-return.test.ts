// An async helper's returned value is resolved, which reads `then`.
import { describe, expect, test } from 'vitest'
import { asyncClosed, asyncOpen } from './helpers'

describe('async-return', () => {
  asyncClosed()
  asyncOpen()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
