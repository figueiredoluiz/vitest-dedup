// RECALL positive -- cross-file-duplicate expected, shape: two files import one module with the
// same `?raw` query. Both read the same text and pass, so keeping the query must not split them.
import { it, expect } from 'vitest'
import * as m from './recipes?raw'
it('reads', () => { expect(typeof m.default).toBe('string') })
