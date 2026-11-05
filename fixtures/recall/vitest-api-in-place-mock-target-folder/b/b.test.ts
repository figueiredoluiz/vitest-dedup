// RECALL negative -- the other half of a/a.test.ts: the same mock spelling, another folder.
import { test, expect } from 'vitest'
import * as framework from 'vitest'
import { read } from '../shared/read'
framework.vi.mock('./dep', () => ({ value: 'mocked' }))
test('reads', async () => expect(await read()).toBe('mocked'))
