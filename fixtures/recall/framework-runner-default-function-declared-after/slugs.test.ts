// RECALL positive -- duplicate-test, shape: default export of a function declared later in the file.
//
// Function declarations are hoisted, so `export default defineSlugChecks` names the function below
// it. The runner's test equals the hand-written one, so the pair is a duplicate.
import { expect, test } from 'vitest'
import defineSlugChecks from './runner'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  const slug = 'Home-Page'.toLowerCase()
  expect(slug).toBe('home-page')
})
