// A call through a project namespace.
import { describe, expect, test } from 'vitest'
import * as h from './support'

describe('namespace-member', () => {
  h.cases()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
