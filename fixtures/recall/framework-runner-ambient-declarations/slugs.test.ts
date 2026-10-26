// RECALL positive -- duplicate-test, shape: the runner module holds `declare global`,
// `declare namespace` (plain and exported) and `declare enum`.
//
// Ambient declarations are erased before the module runs, so the runner registers exactly what its
// body says and its test is compared with the hand-written one below. Treating any of them as a
// setup effect sets the whole file aside as not compared. Make the runner's body differ and the
// finding disappears.
import { expect, test } from 'vitest'
import { defineSlugChecks } from './runner'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  const slug = 'Home-Page'.toLowerCase()
  expect(slug).toBe('home-page')
})
