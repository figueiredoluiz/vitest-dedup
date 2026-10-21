// RECALL positive -- duplicate-test, shape: auto-imported helper called directly at the top level.
//
// `useFixtureClock` has no import: the framework makes every export of `composables/` global, and
// its body is empty. A direct call to an undeclared name is not refused today, so the identical
// pair is compared and reported; that is right once the call resolves to the empty export. The
// case pins the direct form so a fix for the call-inside-a-helper form
// (framework-auto-import-in-helper) cannot regress it. Add a third statement to one body and the
// pair differs.
import { expect, test } from 'vitest'

useFixtureClock()

test('clock: first check', () => {
  expect([3, 1, 2].sort()).toEqual([1, 2, 3])
})

test('clock: second check', () => {
  expect([3, 1, 2].sort()).toEqual([1, 2, 3])
})
