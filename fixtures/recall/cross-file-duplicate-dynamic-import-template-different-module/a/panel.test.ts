// RECALL negative -- cross-file-duplicate-dynamic-import-different-module with template specifiers.
//
// Byte-identical bodies in two directories, each loading its own `./recipes` through a constant
// template; b's holds 12 servings, so b fails. Keying the import by its spelling alone
// links the pair.
import { it, expect } from 'vitest'

it('renders the servings field', async () => {
  const { servings } = await import(`./recipes`)
  expect(servings).toBe(4)
})
