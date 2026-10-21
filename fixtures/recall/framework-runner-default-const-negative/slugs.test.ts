// RECALL negative -- duplicate-test, shape: runner exported through a default identifier.
//
// The runner's test and the hand-written one compute different slugs, so no finding is wanted
// before or after the default-identifier export becomes readable. It fails a fix that reports every
// pair involving a newly readable runner. Replace the body below with the runner's and it becomes
// a duplicate-test error.
import { expect, test } from 'vitest'
import defineSlugChecks from './runner'

defineSlugChecks('page')

test('page slug drops the suffix', () => {
  const slug = 'Home-Page'.toLowerCase().split('-')[0]
  expect(slug).toBe('home')
})
