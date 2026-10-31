// A rewritten alias stays rewritten across an export; `kept` is the alias untouched.
import { expect, test } from 'vitest'
import { api, kept } from './rewritten-namespace-export'

kept.beforeEach(() => {})
api.beforeEach(() => {})
test('stays comparable', () => {
  expect(1).toBe(1)
})
