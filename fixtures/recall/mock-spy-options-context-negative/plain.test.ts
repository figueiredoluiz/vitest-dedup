import { test, expect, vi } from 'vitest'
import { read } from './value'

test('accepts the exported function', () => {
  expect(typeof read).toBe('function')
  expect([undefined, 7, 9]).toContain(read(2))
})
