// RECALL negative -- no cross-file-duplicate, shape: a named import through a `?raw` query.
// `./recipes?raw` exports no `servings`, so this test fails and two.test.ts passes; naming the
// import by the file's own export would pair them.
import { it, expect } from 'vitest'
import { servings } from './recipes?raw'
it('reads', () => { expect(servings).toBe(4) })
