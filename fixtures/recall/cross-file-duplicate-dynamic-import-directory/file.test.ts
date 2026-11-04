// RECALL negative -- cross-file-duplicate must NOT fire. `./dep` loads dep.ts and `./dep/` loads
// dep/index.ts, so this test passes and the other fails. Dropping the trailing `/` would make the
// two imports equal.
import { test, expect } from 'vitest'

test('reads the module', async () => {
  expect((await import('./dep')).value).toBe('file')
})
