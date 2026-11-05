// RECALL negative -- the other half of one.test.ts.
import { test, expect, vi } from 'vitest'
import { value } from './dep'

test('reads', () => {
  vi.mock('./dep', () => ({ value: 'mocked' }))
  expect(value).toBe('mocked')
})
