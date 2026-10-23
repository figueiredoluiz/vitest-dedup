// RECALL positive -- duplicate-test, shape: a `mockNuxtImport` factory reads a container the file
// writes in a hook.
//
// The factory returns `routeMock.value`, and `beforeEach` stores a plain object there. Every value
// that can enter `routeMock` is written in this file and registers nothing, so the factory is setup
// and the two identical tests below are a duplicate-test. The sibling case writes a value that
// registers a test; that one edit leaves the file set aside.
import { beforeEach, expect, test } from 'vitest'
import { mockNuxtImport } from '@nuxt/test-utils/runtime'

const routeMock = { value: { path: '/' } }

mockNuxtImport('useRoute', () => () => routeMock.value)

beforeEach(() => {
  routeMock.value = { path: '/settings' }
})

test('reads the current path', () => {
  expect(routeMock.value.path).toBe('/settings')
})

test('reads the current path again', () => {
  expect(routeMock.value.path).toBe('/settings')
})
