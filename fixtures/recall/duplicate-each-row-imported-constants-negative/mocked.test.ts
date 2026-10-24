// RECALL negative -- the imported module is mocked, so MODE is 'slow' at runtime: Vitest titles
// the rows `mocked slow` and `mocked fast`. One edit away from firing: drop the `vi.mock`.
import { expect, test, vi } from 'vitest'
import { MODE } from './mocked'

vi.mock('./mocked', () => ({ MODE: 'slow' }))

test.each([[MODE], ['fast']])('mocked %s', (value) => {
  expect(value).toBeDefined()
})
