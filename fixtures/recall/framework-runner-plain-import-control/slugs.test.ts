// RECALL positive -- duplicate-test, shape: control for the import-time mock case.
//
// The runner imports a module that only declares a value, so importing it changes nothing the tests
// observe and the runner's test is compared with the hand-written one. If this stops reporting while
// the mock case stays silent, the import check refuses more than `vi` and `expect` calls.
import { expect, test } from 'vitest'
import { defineSlugChecks } from './runner'
import { slug } from './slug'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  expect(slug('Home-Page')).toBe('home-page')
})
