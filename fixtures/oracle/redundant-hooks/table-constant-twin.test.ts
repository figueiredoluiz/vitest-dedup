// Runtime control for the suite-table hook claim: the identical `beforeEach` pair written
// inside a proven `describe.each` callback executes once per generated suite, before that
// suite's tests. Removing either hook keeps every test passing; the analyzer reports the pair
// once by declaration location.
import { describe, test, expect, beforeEach } from 'vitest'

let value = 0

describe.each(['admin', 'viewer'])('%s', () => {
  beforeEach(() => {
    value = 1
  })
  beforeEach(() => {
    value = 1
  })

  test('value is set', () => {
    expect(value).toBe(1)
  })
})
