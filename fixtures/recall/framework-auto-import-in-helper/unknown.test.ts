// RECALL negative -- duplicate-test, control for the auto-import case.
//
// `useUnknownThing` is declared nowhere in the tree, so nothing proves what it does and the
// identical pair below must stay silent whether or not auto-imports are resolved. It fails a fix
// that treats every undeclared call in a helper as inert. Declare it as an empty export under
// `composables/` and the pair becomes a duplicate-test.
import { expect, test } from 'vitest'
import { setupUnknown } from './setup-unknown'

setupUnknown()

test('unknown: first check', () => {
  expect([6, 5, 4].sort()).toEqual([4, 5, 6])
})

test('unknown: second check', () => {
  expect([6, 5, 4].sort()).toEqual([4, 5, 6])
})
