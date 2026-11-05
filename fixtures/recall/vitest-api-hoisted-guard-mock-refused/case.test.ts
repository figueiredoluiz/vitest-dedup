// RECALL negative -- an in-source guard does not lift a mock out of a call Vitest hoists: the
// `vi.mock` inside `vi.hoisted` makes Vitest refuse the file ("Cannot call vi.mock inside
// vi.hoisted"), so its tests are not compared. Dropping the nested `vi.mock` makes them
// a duplicate.
import { expect, vi } from 'vitest'
import { value } from './dep'

if (import.meta.vitest) {
  const { test } = import.meta.vitest
  vi.mock('./dep', () => ({ value: 'mocked' }))
  vi.hoisted(() => { if (import.meta.vitest) { vi.mock('./other') } })
  test('reads the mock', () => expect(value).toBe('mocked'))
  test('reads the mock, again', () => expect(value).toBe('mocked'))
}
