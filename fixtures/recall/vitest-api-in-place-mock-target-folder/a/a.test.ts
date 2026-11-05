// RECALL negative -- cross-file-duplicate must NOT fire. Both files mock `./dep` in place through
// a binding Vitest does not hoist, and both read `a/dep` through one helper. Only a's mock targets
// that module, so a passes and b fails. Keying the target by its spelling alone would pair them.
import { test, expect } from 'vitest'
import { vi as tools } from 'vitest'
import { read } from '../shared/read'
tools.mock('./dep', () => ({ value: 'mocked' }))
test('reads', async () => expect(await read()).toBe('mocked'))
