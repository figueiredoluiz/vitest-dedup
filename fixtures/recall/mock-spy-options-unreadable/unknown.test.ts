import { test, expect, vi } from 'vitest'
import { read } from './value'
vi.mock('./value', { spy: Boolean(true) })
test('first unknown observer', () => {
  expect(typeof read).toBe('function')
  expect([undefined, 7, 9]).toContain(read(2))
})
test('second unknown observer', () => {
  expect(typeof read).toBe('function')
  expect([undefined, 7, 9]).toContain(read(2))
})
