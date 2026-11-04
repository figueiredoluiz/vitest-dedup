// RECALL negative -- the DYNAMIC twin of cross-file-duplicate-same-specifier-different-module.
//
// Byte-identical bodies in two directories, each `await import('./recipes')`-ing its OWN module;
// b's holds 12 servings, so b fails. A relative `import()` is keyed by its spelling and the
// importing file's directory, so the two stay apart.
//
// The extraction floor is the other half of the assertion: if either file stopped being analyzed
// this case would go silent for the wrong reason.
import { it, expect } from 'vitest'

it('renders the servings field', async () => {
  const { servings } = await import('./recipes')
  expect(servings).toBe(4)
})
