// RECALL negative -- duplicate-test, shape: default export of a `let` binding.
//
// A `let` binding is not read as a runner (only function declarations and `const` callables are),
// so the file is set aside as not compared. It must stay that way: a change that accepts every default
// identifier would report the pair below. Make the binding a `const` and it becomes a duplicate.
import { expect, test } from 'vitest'
import defineSlugChecks from './runner'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  const slug = 'Home-Page'.toLowerCase()
  expect(slug).toBe('home-page')
})
