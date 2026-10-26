// RECALL negative -- duplicate-test, shape: the runner module calls `vi.useFakeTimers()` in an
// initializer.
//
// Fake timers apply to every test in the importing file, not only the runner's (`observe.test.ts`
// shows it), and that is not part of any test's context, so the file stays set aside as not
// compared. A change that admits a `vi` call, or any call in an initializer, would report the pair
// below. Replace the call with an object literal and it becomes a duplicate.
import { expect, test } from 'vitest'
import { defineSlugChecks } from './runner'

defineSlugChecks('page')

test('page slug is lowercase', () => {
  const slug = 'Home-Page'.toLowerCase()
  expect(slug).toBe('home-page')
})
