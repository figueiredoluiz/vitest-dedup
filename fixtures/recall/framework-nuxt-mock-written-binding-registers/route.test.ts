// RECALL control -- duplicate-test absent, shape: a `mockNuxtImport` factory reads a container the
// file writes with a value that registers a test.
//
// One edit from `framework-nuxt-mock-written-binding`: the object stored in `beforeEach` carries a
// callback that calls `test`. The factory hands that object to the code under test, so the proof
// cannot place the factory and the file is set aside; the identical pair must not be reported.
import { beforeEach, expect, test } from 'vitest'
import { mockNuxtImport } from '@nuxt/test-utils/runtime'

const routeMock = { value: { path: '/' } }

mockNuxtImport('useRoute', () => () => routeMock.value)

beforeEach(() => {
  routeMock.value = { path: '/settings', run: () => test('hidden', () => {}) }
})

test('reads the current path', () => {
  expect(routeMock.value.path).toBe('/settings')
})

test('reads the current path again', () => {
  expect(routeMock.value.path).toBe('/settings')
})
