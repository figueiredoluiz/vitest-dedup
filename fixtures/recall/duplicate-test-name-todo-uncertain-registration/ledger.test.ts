// RECALL negative -- duplicate-test-name.
//
// A todo registered from a function the analysis cannot place has no proven name, so it collides
// with nothing, even where Vitest registers it: the `.then` callback runs during collection here,
// but a function decides when, and whether, it registers. A todo whose arguments are spread has no
// readable callback either, so it stays unresolved instead of named.
//
// Load-bearing: unwrap `Promise.resolve().then(() => ...)` so the second `syncs the ledger` is
// written at the top level, and the pair becomes one `duplicate-test-name` warning, as it does for
// two plain todos.
//
// `closes the ledger` is compared, so the file is not blind and its silence is a verdict.
import { expect, test } from 'vitest'

test('closes the ledger', () => {
  expect([1, 2]).toHaveLength(2)
})

test('syncs the ledger')
Promise.resolve().then(() => test('syncs the ledger'))

test('exports the ledger', ...[])
test('exports the ledger', ...[])
