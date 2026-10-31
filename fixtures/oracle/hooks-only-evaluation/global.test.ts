// Reading a global may run an accessor.
import { describe, expect, test } from 'vitest'
import { globalClosed, globalOpen } from './helpers'

describe('global', () => {
  globalClosed()
  globalOpen()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
