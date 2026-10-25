// RECALL positive: matching literal spy registrations retain cross-file-duplicate findings.
// Both files must be comparable; the distinct-mode and unreadable-option cases cannot
// prove that two supported spy contexts still match.
import { test, expect, vi } from 'vitest'
import { read } from './value'
vi.mock('./value', { spy: true })
test('reads the original value', () => {
  expect(read(2)).toBe(7)
  expect(vi.isMockFunction(read)).toBe(true)
  expect(read).toHaveBeenCalledWith(2)
})
