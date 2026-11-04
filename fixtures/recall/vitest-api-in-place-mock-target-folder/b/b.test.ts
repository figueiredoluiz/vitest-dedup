// RECALL negative -- the other half of a/a.test.ts: the same mock spelling, another folder.
import { test, expect } from 'vitest'
import * as framework from 'vitest'
framework.vi.mock('./dep', () => ({ value: 'mocked' }))
test('reads', async () => expect((await import('../a/dep')).value).toBe('mocked'))
