// RECALL negative: identical bodies remain distinct under automock, factory, unmocked,
// and literal-spy contexts. This isolates mock-mode identity from the positive spy case.
// One-edit counterfactual: remove the vi.mock call from spy.test.ts; its context then
// matches plain.test.ts and the cross-file-duplicate finding must appear.
import { test, expect, vi } from 'vitest'
import { read } from './value'
vi.mock('./value')
test('accepts the exported function', () => {
  expect(typeof read).toBe('function')
  expect([undefined, 7, 9]).toContain(read(2))
})
