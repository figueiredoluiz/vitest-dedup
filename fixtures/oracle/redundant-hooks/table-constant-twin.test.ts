// Runtime control for the suite-table hook claim: the identical `beforeEach` pair written
// inside a proven `describe.each` callback executes once per generated suite, in registration
// order, before that suite's tests. Removing either hook breaks the order the test asserts;
// the analyzer reports the pair once by declaration location.
import { describe, test, expect, beforeEach } from 'vitest'

const seen: string[] = []

describe.each([
  { role: 'admin' },
  { role: 'viewer' },
])('$role', ({ role }) => {
  beforeEach(() => {
    seen.push(`first:${role}`)
  })
  beforeEach(() => {
    seen.push(`second:${role}`)
  })

  test('hooks ran in order', () => {
    expect(seen.slice(-2)).toEqual([`first:${role}`, `second:${role}`])
  })
})
