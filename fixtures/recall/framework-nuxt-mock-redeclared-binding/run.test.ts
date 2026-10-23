// RECALL control -- duplicate-test absent, shape: a `mockNuxtImport` factory reads a redeclared
// `var` whose second declaration registers a test.
//
// The factory returns `run`, and the second `var run` replaces the harmless first value with one
// that calls `test`. Reading only the first declaration would place the factory and report the
// identical pair below; the file must be set aside. Drop the second declaration and the pair is a
// duplicate-test.
import { expect, test } from 'vitest'
import { mockNuxtImport } from '@nuxt/test-utils/runtime'

var run = () => {}
var run = () => test('hidden', () => {})

mockNuxtImport('useRun', () => () => run)

test('reads the value', () => {
  expect(1).toBe(1)
})

test('reads the value again', () => {
  expect(1).toBe(1)
})
