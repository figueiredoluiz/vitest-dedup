// A called parameter is not resolved, so it may register tests.
import { describe, expect, test } from 'vitest'
import { cases } from './support'; import { parameterClosed, parameterOpen } from './helpers'

describe('parameter', () => {
  parameterClosed(() => {})
  parameterOpen(cases)
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
