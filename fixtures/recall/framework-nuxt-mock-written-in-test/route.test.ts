// RECALL positive -- duplicate-test, shape: a `mockNuxtImport` factory reads a container each test
// writes before it reads.
//
// The write sits in the test body, which is that test's own compared body, so the factory is setup
// and the two identical tests below are a duplicate-test. Move the write to module level and the
// file is set aside: no compared context would record it.
import { expect, test } from 'vitest'
import { mockNuxtImport } from '@nuxt/test-utils/runtime'

const routeMock = { value: { path: '/' } }

mockNuxtImport('useRoute', () => () => routeMock.value)

test('opens the settings page', () => {
  routeMock.value = { path: '/settings' }
  expect(routeMock.value.path).toBe('/settings')
})

test('opens the settings page again', () => {
  routeMock.value = { path: '/settings' }
  expect(routeMock.value.path).toBe('/settings')
})
