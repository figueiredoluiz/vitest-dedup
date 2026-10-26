// Runtime evidence for this case: importing the runner switched this file's own tests to fake
// timers.
import { expect, test, vi } from 'vitest'
import { defineSlugChecks } from './runner'

defineSlugChecks('observe')

test('the runner module switched this file to fake timers', () => {
  expect(vi.isFakeTimers()).toBe(true)
})
