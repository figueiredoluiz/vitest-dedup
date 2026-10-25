import { test, expect, vi } from 'vitest'
import { read } from './value'
vi.mock('./value', { 'spy': true })
test('reads the original value', () => {
  expect(read(2)).toBe(7)
  expect(vi.isMockFunction(read)).toBe(true)
  expect(read).toHaveBeenCalledWith(2)
})
