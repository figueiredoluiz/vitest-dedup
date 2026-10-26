// RECALL negative -- duplicate-test, shape: the runner module destructures an object literal whose
// getter registers a test while the module loads.
//
// Destructuring reads the property, the getter runs, and the importing file gains a test the runner's
// body does not show, so the file stays set aside as not compared. A change that admits object
// literals with accessors, or destructured value bindings, would report the pair below as if the file
// held only these two tests. Make `label` a plain property and it becomes a duplicate.
import { expect, test } from 'vitest'
import { defineSlugChecks } from './runner'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  const slug = 'Home-Page'.toLowerCase()
  expect(slug).toBe('home-page')
})
