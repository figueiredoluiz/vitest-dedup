// RECALL control -- duplicate-test absent, shape: a `mockNuxtImport` factory reads a counter a hook
// increments before every test.
//
// The two tests have identical bodies, but the hook runs once per test, so the first observes 1 and
// the second 2: Vitest passes the first and fails the second, and deleting either changes the
// outcome. The pair must not be reported. One edit from `framework-nuxt-mock-written-binding`: a
// hook that resets `state.count = 1` instead keeps the pair a duplicate-test.
import { beforeEach, expect, test } from 'vitest'
import { mockNuxtImport } from '@nuxt/test-utils/runtime'

const state = { count: 0 }

mockNuxtImport('useCount', () => () => state.count)

beforeEach(() => state.count++)

test('reads the count', () => {
  expect(state.count).toBe(1)
})

test('reads the count again', () => {
  expect(state.count).toBe(1)
})
