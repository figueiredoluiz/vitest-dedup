// RECALL positive -- duplicate-test expected.
//
// Each test calls a different imported helper with one argument. The helpers are equal apart from
// their names, so the missing argument is `undefined` in both and the tests run the same code.
// An export is identified by what it declares, not by its name.
import { test } from 'vitest'
import { assertNonEmpty, checkActive } from './helpers'

test('accepts a plain label', () => {
  checkActive('dashboard')
})

test('accepts a plain label, again', () => {
  assertNonEmpty('dashboard')
})
