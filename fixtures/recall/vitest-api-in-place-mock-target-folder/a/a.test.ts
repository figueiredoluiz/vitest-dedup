// RECALL negative -- cross-file-duplicate must NOT fire. Both files mock `./dep` in place through
// a binding Vitest does not hoist, and both read `../a/dep`. Only a's mock targets that module, so
// a passes and b fails. Writing b's target as `../a/dep` would make the two equal.
import { test, expect } from 'vitest'
import { vi as tools } from 'vitest'
tools.mock('./dep', () => ({ value: 'mocked' }))
test('reads', async () => expect((await import('../a/dep')).value).toBe('mocked'))
