// Runtime evidence for this case: the runner's namespace registered a `beforeEach` in this file,
// which ran once for the runner's test and once for this one.
import { expect, test } from 'vitest'
import { defineSlugChecks } from './runner'

defineSlugChecks('observe')

test('the runner module registered a hook for this file', () => {
  expect((globalThis as any).slugRuns).toBe(2)
})
