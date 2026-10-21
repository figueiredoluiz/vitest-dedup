// RECALL negative -- duplicate-test, shape: default export of a reassigned function declaration.
//
// The module reassigns the binding, so what the default export holds is not the declaration. The
// runner is not read and the file is set aside as not compared. Remove the reassignment and it
// becomes a duplicate.
import { expect, test } from 'vitest'
import defineSlugChecks from './runner'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  const slug = 'Home-Page'.toLowerCase()
  expect(slug).toBe('home-page')
})
