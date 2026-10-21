// RECALL negative -- duplicate-test, shape: default export of an imported binding.
//
// `export default inner` forwards a default import, a further hop that this reader does not follow,
// so the runner is not read and the file is set aside as not compared. Import `./inner` directly
// and it becomes a duplicate.
import { expect, test } from 'vitest'
import defineSlugChecks from './runner'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  const slug = 'Home-Page'.toLowerCase()
  expect(slug).toBe('home-page')
})
