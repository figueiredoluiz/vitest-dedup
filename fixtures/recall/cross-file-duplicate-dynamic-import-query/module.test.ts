// RECALL negative -- a query on a dynamic import names another module.
//
// `./recipes?raw` loads the file's text as a default export, so the other test fails while this
// one passes. A specifier with a query is keyed by its spelling; dropping `?raw` links the pair.
import { it, expect } from 'vitest'

it('lists the module exports', async () => {
  const loaded = await import('./recipes')
  expect(Object.keys(loaded).sort()).toEqual(['servings'])
})
