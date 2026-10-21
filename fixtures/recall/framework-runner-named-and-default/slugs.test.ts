// RECALL positive -- duplicate-test, shape: `export const f = ...; export default f`.
//
// The usual way one function is offered by name and as the default. The default import reads the
// same runner as the named one, so the pair is a duplicate.
import { expect, test } from 'vitest'
import defineSlugChecks from './runner'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  const slug = 'Home-Page'.toLowerCase()
  expect(slug).toBe('home-page')
})
