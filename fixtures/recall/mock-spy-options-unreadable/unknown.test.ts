// RECALL negative: Boolean(true) is an unreadable spy option, so both tests remain
// unresolved and no duplicate-test finding is emitted. This checks refusal of a dynamic
// option, unlike the supported-spy positive and distinct-mode cross-file cases.
// One-edit counterfactual: replace Boolean(true) with true; the two tests become
// comparable and the duplicate-test finding must appear.
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
