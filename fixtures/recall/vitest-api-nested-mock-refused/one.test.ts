// RECALL negative -- a hoisted mock call nested in a test makes Vitest refuse the file at load
// ("defined outside of the module's top level scope"), so neither file runs and neither is
// compared. Moving both `vi.mock` calls to the top level makes the pair a duplicate.
import { test, expect, vi } from 'vitest'
import { value } from './dep'

test('reads', () => {
  vi.mock('./dep', () => ({ value: 'mocked' }))
  expect(value).toBe('mocked')
})
