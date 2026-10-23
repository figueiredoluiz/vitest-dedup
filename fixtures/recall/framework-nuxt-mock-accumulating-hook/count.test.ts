// RECALL control -- duplicate-test absent, shape: a `mockNuxtImport` factory reads a counter a hook
// increments before every test.
//
// The two tests have identical bodies, but the hook runs once per test, so the first observes 1 and
// the second 2: Vitest passes the first and fails the second, and deleting either changes the
// outcome. The pair must not be reported. (The factory reading a binding the file writes is refused
// whatever the hook does, so this also holds if that refusal is ever narrowed.)
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
