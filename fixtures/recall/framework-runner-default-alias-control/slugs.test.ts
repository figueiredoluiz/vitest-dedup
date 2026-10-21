// RECALL positive -- duplicate-test, control for the default-identifier runner case.
//
// `export { local as default }` of a `const` arrow is expanded today and the pair is reported. It
// pins the alias half of the default-export shape so a fix for `export default <identifier>` cannot
// regress it. Make the runner's body differ from the hand-written test and the finding disappears.
import { expect, test } from 'vitest'
import defineSlugChecks from './runner'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  const slug = 'Home-Page'.toLowerCase()
  expect(slug).toBe('home-page')
})
