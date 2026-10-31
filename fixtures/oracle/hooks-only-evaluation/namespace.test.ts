// A call through a project namespace is not resolved, so it may register tests.
import { describe, expect, test } from 'vitest'
import { hooks, namespaceOpen } from './helpers'

describe('namespace', () => {
  hooks()
  namespaceOpen()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
