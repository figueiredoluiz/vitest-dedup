// RECALL negative -- no cross-file-duplicate, shape: a static import whose `?raw` query loads
// another module. `./recipes?raw` is the file's text, `./recipes` its exports: this test fails
// and two.test.ts passes, so dropping the query would pair them.
import { it, expect } from 'vitest'
import * as m from './recipes?raw'
it('reads', () => { expect(Object.keys(m).sort()).toEqual(['servings']) })
