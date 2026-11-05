// RECALL negative -- a renamed `tools.mock` is not hoisted, and only Vitest's hoisting transform
// rewrites an `import()` target to its path; in place the call throws ("vi.mock() expects a
// string path") and the file does not load. Writing the target as './dep' makes the tests a
// duplicate.
import { test, expect, vi as tools } from 'vitest'

tools.mock(import('./dep'), () => ({ value: 'mocked' }))
test('reads the mock', async () => expect((await import('./dep')).value).toBe('mocked'))
test('reads the mock, again', async () => expect((await import('./dep')).value).toBe('mocked'))
