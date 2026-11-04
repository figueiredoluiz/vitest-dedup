// RECALL negative -- the other half of file.test.ts: the directory spelling of the same name.
import { test, expect } from 'vitest'

test('reads the module', async () => {
  expect((await import('./dep/')).value).toBe('file')
})
