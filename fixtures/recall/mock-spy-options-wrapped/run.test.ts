// RECALL positive: TypeScript wrappers erase before Vitest reads the literal spy option.
// One-edit counterfactual: replace true with Boolean(true); the file becomes unresolved.
import { expect, test, vi } from 'vitest'
import { read } from './value'

vi.mock('./value', { spy: true as const })

test('first wrapped spy', () => {
  expect(read(2)).toBe(7)
})

test('second wrapped spy', () => {
  expect(read(2)).toBe(7)
})
