// RECALL positive -- duplicate-test, shape: auto-imported composable called inside a setup helper.
//
// `setupApp()` calls `useFixtureClock()` with no import: the framework makes every export of
// `composables/` global, and that export has an empty body. The identical pair below is a
// duplicate-test once the call resolves to it. Today the name is an undeclared global in an
// imported helper and the file is set aside (not-compared). Add a third statement to one body and
// the pair differs.
import { expect, test } from 'vitest'
import { setupApp } from './setup'

setupApp()

test('clock: first check', () => {
  expect([3, 1, 2].sort()).toEqual([1, 2, 3])
})

test('clock: second check', () => {
  expect([3, 1, 2].sort()).toEqual([1, 2, 3])
})
