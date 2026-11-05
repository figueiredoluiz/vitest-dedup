// Runtime control for the plain-receiver property claim: the identical `beforeEach` pair over
// a proven plain object executes the write before the test reads it. Removing either hook
// leaves the property empty and fails the test; the analyzer reports the pair.
import { test, expect, beforeEach } from 'vitest'

const audit = { mode: '' }

beforeEach(() => {
  audit.mode = 'primary'
})

beforeEach(() => {
  audit.mode = 'primary'
})

test('the write happened', () => {
  expect(audit.mode).toBe('primary')
})
