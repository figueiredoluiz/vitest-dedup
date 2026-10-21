// RECALL positive -- duplicate-test, shape: runner exported through a default identifier.
//
// `defineSlugChecks('page')` registers a test whose body equals the hand-written test below, so
// the pair is a duplicate once the runner is read. Today the default-identifier export is refused
// as unreadable and the whole file is set aside (`not-compared`). Delete the hand-written test and
// nothing is left to pair with.
import { expect, test } from 'vitest'
import defineSlugChecks from './runner'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  const slug = 'Home-Page'.toLowerCase()
  expect(slug).toBe('home-page')
})
