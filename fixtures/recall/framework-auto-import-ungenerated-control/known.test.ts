// RECALL variant -- duplicate-test, control for the auto-import case.
//
// The same source as framework-auto-import-in-helper without `.nuxt/imports.d.ts`, which `nuxt
// prepare` generates and a fresh checkout lacks. Nothing then says what `useFixtureClock` stands
// for, so the identical pair below is set aside (not-compared) instead of being guessed at. Add
// the generated file and it becomes the duplicate-test of the generated case.
import { expect, test } from 'vitest'
import { setupApp } from './setup'

setupApp()

test('clock: first check', () => {
  expect([3, 1, 2].sort()).toEqual([1, 2, 3])
})

test('clock: second check', () => {
  expect([3, 1, 2].sort()).toEqual([1, 2, 3])
})
