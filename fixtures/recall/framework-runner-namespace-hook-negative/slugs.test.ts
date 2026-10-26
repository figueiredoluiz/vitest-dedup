// RECALL negative -- duplicate-test, shape: the runner module's namespace is not `declare`.
//
// A namespace without `declare` compiles to code that runs at import, and this one registers a
// `beforeEach` in every file that imports the runner (`observe.test.ts` shows it running), so the
// file stays set aside as not compared. A change that admits every namespace, or drops the chain
// check, would report the pair below. Add `declare` and remove the hook and it becomes a duplicate.
import { expect, test } from 'vitest'
import { defineSlugChecks } from './runner'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  const slug = 'Home-Page'.toLowerCase()
  expect(slug).toBe('home-page')
})
